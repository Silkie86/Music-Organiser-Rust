//! Send to Bitwig: one sound on a Sampler on a new track, or a kit on a Drum Machine, in the project open in Bitwig
//! (the C# app's `BitwigSend.SoundAsync`, `KitAsync` and `KitPads`, with the Sounds page's words for how each went).
//! Bitwig needs "Let the app make changes" turned on.
//!
//! Each send connects, checks the link's greeting, and keeps every change in the project Bitwig had open as it began
//! ([`Link::stay_in_open_project`]): if another project comes to the front part way, the send stops there rather
//! than going on in that one. The app makes one change at a time (sends, auditions, plays and their stops), in the
//! order they came, so one send's new track never lands in the middle of another's; and from extension 3.21 each
//! step finds the new track by its ID, however tracks move meanwhile. Sends never take anything out of a project:
//! only an audition's own track or clip is ever deleted ([`super::audition`]).
//!
//! A send fails, with nothing changed, when Bitwig can't take it; once it may have changed something, it gives back
//! what happened instead, with words for what may not have (Bitwig may have added a track all the same).

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

use mo_formats::paths::file_name;

use super::audition::same_project;
use super::link::{CHANGE_TIMEOUT, CONNECT_TIMEOUT, PLAY_START_NEEDS, TIMEOUT};
use super::{DRUM_MACHINE_NAME, DeviceId, Error, Hello, Inserted, Link, PORT, SongLoop};

/// The app's sends wait here for their turn, so they go in one at a time, in the order they came.
pub(super) static SENDS: Queue = Queue::new();

/// A place in the line for the app's next change in Bitwig, taken now and waited on later ([`Ticket::wait`]): a
/// screen takes it on the window's thread as the button is pressed, so a Stop and the Play pressed after it go in
/// that order, whichever of their threads runs first (the 6 Nov review's D-fp3). Plays, auditions and their stops
/// take one (sends take theirs themselves).
pub fn ticket() -> Ticket<'static> {
    SENDS.ticket()
}

/// Turns in the order they were asked for, as at a counter: each send takes the next number and waits until it's
/// called. (A lock on its own lets whichever waiting thread the system picks go next: on Windows, not the first.)
pub(super) struct Queue {
    numbers: Mutex<Numbers>,
    called: Condvar,
}

struct Numbers {
    /// The number the next send takes.
    next: u64,
    /// The number whose turn it is.
    now: u64,
    /// Numbers given up before their turn came: skipped when it does.
    given_up: BTreeSet<u64>,
}

impl Numbers {
    /// Calls the next number still waiting.
    fn call_next(&mut self) {
        self.now += 1;
        while self.given_up.remove(&self.now) {
            self.now += 1;
        }
    }
}

impl Queue {
    const fn new() -> Queue {
        Queue { numbers: Mutex::new(Numbers { next: 0, now: 0, given_up: BTreeSet::new() }), called: Condvar::new() }
    }

    fn numbers(&self) -> std::sync::MutexGuard<'_, Numbers> {
        self.numbers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes the next number, without waiting.
    pub(super) fn ticket(&self) -> Ticket<'_> {
        let mut numbers = self.numbers();
        let number = numbers.next;
        numbers.next += 1;
        Ticket { queue: self, number, waited: false }
    }

    /// Takes the next number and waits for its turn, which lasts until what this gives back goes.
    pub(super) fn wait_turn(&self) -> Turn<'_> {
        self.ticket().wait()
    }
}

/// A number in the line, not yet called ([`ticket`]). Waiting on it gives the turn; dropping it unused gives the place
/// up without waiting, so the ones after it never wait for it.
pub struct Ticket<'a> {
    queue: &'a Queue,
    number: u64,
    /// Waited on, so its turn gives the place up instead.
    waited: bool,
}

impl<'a> Ticket<'a> {
    /// Waits for this number's turn, behind every one taken before it.
    pub fn wait(mut self) -> Turn<'a> {
        let (queue, mine) = (self.queue, self.number);
        self.waited = true;
        let mut numbers = queue.numbers();
        while numbers.now != mine {
            numbers = queue.called.wait(numbers).unwrap_or_else(PoisonError::into_inner);
        }
        Turn(queue)
    }
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        if self.waited {
            return;
        }
        let mine = self.number;
        let mut numbers = self.queue.numbers();
        if numbers.now == mine {
            numbers.call_next();
            drop(numbers);
            self.queue.called.notify_all();
        } else {
            numbers.given_up.insert(mine);
        }
    }
}

/// A change's turn: the app's other changes in Bitwig wait until it goes. The next number is called when this goes,
/// also when the change stopped with a panic. The audition and play steps take one, so a caller may do more inside
/// the same turn (put back what couldn't be stopped, say) before the next change goes.
pub struct Turn<'a>(&'a Queue);

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.0.numbers().call_next();
        self.0.called.notify_all();
    }
}

/// Where to find the link in Bitwig, and how long to wait for it. [`Bitwig::default`] is the link's own port on this
/// computer; tests point it at a stand-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bitwig {
    pub port: u16,
    /// How long to wait for the link to take the connection, and then for its greeting.
    pub connect_timeout: Duration,
    /// How long to wait for an answer that only reads.
    pub timeout: Duration,
    /// How long to wait for each change.
    pub change_timeout: Duration,
}

impl Default for Bitwig {
    fn default() -> Bitwig {
        Bitwig { port: PORT, connect_timeout: CONNECT_TIMEOUT, timeout: TIMEOUT, change_timeout: CHANGE_TIMEOUT }
    }
}

impl Bitwig {
    /// Connects to the link and reads its greeting.
    pub fn connect(&self) -> Result<Link, Error> {
        let mut link = Link::connect(self.port, self.connect_timeout)?;
        link.timeout = self.timeout;
        link.change_timeout = self.change_timeout;
        Ok(link)
    }

    /// What Bitwig has open and whether a send would go in now ([`Hello::can_send`]), without changing anything:
    /// for a screen to show before the button is pressed. It connects each time; something that asks every few
    /// seconds keeps a [`Link`] and asks [`Link::refresh`] instead.
    pub fn check(&self) -> Result<Hello, Error> {
        Ok(self.connect()?.hello().clone())
    }

    /// Puts one sound into the project open in Bitwig: a new track named `name`, with the sound on a Sampler. Waits
    /// for the sends asked for before it to finish first.
    pub fn send_sound(&self, path: impl AsRef<Path>, name: &str) -> Result<SentSound, Error> {
        let _turn = SENDS.wait_turn();
        let mut link = self.connect()?;
        link.stay_in_open_project();
        on_new_track(link, path.as_ref(), name, Load::Sound)
    }

    /// Puts one preset (a `.bwpreset`, or any file Bitwig loads as a device) into the project open in Bitwig: a new
    /// track named `name`, with the preset's device on it. Waits for the sends asked for before it to finish first, so
    /// presets and sounds go in one at a time, in the order they were asked for.
    pub fn send_preset(&self, path: impl AsRef<Path>, name: &str) -> Result<SentSound, Error> {
        let _turn = SENDS.wait_turn();
        let mut link = self.connect()?;
        link.stay_in_open_project();
        on_new_track(link, path.as_ref(), name, Load::Preset)
    }

    /// Puts a MIDI part into the project open in Bitwig: a new instrument track named `name`, with the MIDI file at
    /// `path` as a clip in its first launcher slot (the C# MIDI page's Send to Bitwig). Waits for the sends asked for
    /// before it to finish first.
    pub fn send_part(&self, path: impl AsRef<Path>, name: &str) -> Result<SentPart, Error> {
        let _turn = SENDS.wait_turn();
        let mut link = self.connect()?;
        link.stay_in_open_project();
        part(&mut link, path.as_ref(), name)
    }

    /// [`Bitwig::send_part`] for a part found for a song: once its clip is in, Bitwig's play start moves to the first
    /// of the song's bars it was found for (`bars.start`), so it marks where the clip goes in the arranger: the
    /// controller API can't put it there, and a clip dragged down from the launcher is dropped where you let go.
    /// Only in the song's own project (by its name, as [`Bitwig::play_with_song`] tells it), and from the extension
    /// 3.23: [`SentPart::mark`] says whether it's there. A Bitwig that's playing plays on.
    pub fn send_part_for_song(&self, path: impl AsRef<Path>, name: &str, bars: &SongLoop) -> Result<SentPart, Error> {
        let _turn = SENDS.wait_turn();
        let mut link = self.connect()?;
        link.stay_in_open_project();
        let mut sent = part(&mut link, path.as_ref(), name)?;
        if sent.slot.is_some() {
            sent.mark = mark(&mut link, bars);
        }
        Ok(sent)
    }

    /// Puts a kit's pads ([`kit_pads`]) on a Drum Machine in the project open in Bitwig: a new Drum Machine on a new
    /// track named `name`, or the one selected in Bitwig, where pads that already have something on them are left
    /// alone. A pad whose sound isn't where the app last saw it is left out. Waits for the sends asked for before it
    /// to finish first.
    pub fn send_kit(&self, name: &str, pads: &[Pad], target: KitTarget) -> Result<SentKit, Error> {
        let _turn = SENDS.wait_turn();
        let mut link = self.connect()?;
        link.stay_in_open_project();
        kit(&mut link, name, pads, target)
    }
}

/// [`Bitwig::send_sound`] through the link's own port: one sound on a Sampler on a new track named `name`. Waits for
/// Bitwig, so call it on a background thread, never the window's.
pub fn send_sound(path: impl AsRef<Path>, name: &str) -> Result<SentSound, Error> {
    Bitwig::default().send_sound(path, name)
}

/// [`Bitwig::send_part`] through the link's own port. Waits for Bitwig, so call it on a background thread, never the
/// window's.
pub fn send_part(path: impl AsRef<Path>, name: &str) -> Result<SentPart, Error> {
    Bitwig::default().send_part(path, name)
}

/// [`Bitwig::send_preset`] through the link's own port: one preset on a new track named `name`. Waits for Bitwig, so
/// call it on a background thread, never the window's.
pub fn send_preset(path: impl AsRef<Path>, name: &str) -> Result<SentSound, Error> {
    Bitwig::default().send_preset(path, name)
}

/// [`Bitwig::send_kit`] through the link's own port. Waits for Bitwig, so call it on a background thread, never the
/// window's.
pub fn send_kit(name: &str, pads: &[Pad], target: KitTarget) -> Result<SentKit, Error> {
    Bitwig::default().send_kit(name, pads, target)
}

/// What went into Bitwig for one sound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentSound {
    /// The sound's name, which its track has.
    pub name: String,
    /// The new track's place in Bitwig's project listing; none when Bitwig didn't say it made one (see `problem`).
    pub track: Option<usize>,
    /// What didn't go to plan, in words without a full stop ("Bitwig didn't load Kick.wav"), or none.
    pub problem: Option<String>,
}

impl SentSound {
    /// Whether it went in as asked: on a Sampler, on its new track.
    pub fn went_in(&self) -> bool {
        self.problem.is_none()
    }

    /// The headline for the activity card: "Kick 909 is in Bitwig", "Kick 909 didn't go in cleanly", or (with no
    /// track for it) "Kick 909 didn't go into Bitwig".
    pub fn title(&self) -> String {
        match (&self.problem, self.track) {
            (None, _) => format!("{} is in Bitwig", self.name),
            (Some(_), Some(_)) => format!("{} didn't go in cleanly", self.name),
            (Some(_), None) => format!("{} didn't go into Bitwig", self.name),
        }
    }

    /// The line under it: where it went, or what went wrong.
    pub fn detail(&self) -> String {
        match &self.problem {
            None => SOUND_PLACE.to_owned(),
            Some(problem) => format!("{problem}."),
        }
    }
}

/// Where a sent sound goes, for the card while it's sent and once it's in.
pub const SOUND_PLACE: &str = "On a Sampler, on a new track";

/// The card's headline while a sound is sent: "Putting Kick 909 into Bitwig".
pub fn sending_sound(name: &str) -> String {
    format!("Putting {name} into Bitwig")
}

/// The card's headline when a sound couldn't be sent at all; the line under it is the error.
pub const SOUND_FAILED: &str = "Couldn't put it into Bitwig";

/// Puts one sound into the project open in Bitwig, through `link`, as if it were dropped on a new track.
/// What a send puts on its new track: a sound (on a Sampler) or a preset (its own device).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Load {
    Sound,
    Preset,
}

fn on_new_track(mut link: Link, path: &Path, name: &str, load: Load) -> Result<SentSound, Error> {
    let link = &mut link;
    link.hello().can_send()?;
    let shown = path.to_string_lossy().into_owned();
    let file = file_name(&shown).to_owned();
    if !path.is_file() {
        return Err(Error::Missing { file, path: shown });
    }
    let mut sent = SentSound { name: name.to_owned(), track: None, problem: None };
    let track = match link.new_track(name) {
        Ok(track) => track,
        // The sound isn't in: the link may have stopped after Bitwig added the track (another project came to the
        // front, say), or its answer didn't come.
        Err(e) => {
            sent.problem = Some(format!("{}. There may be an empty track for it in Bitwig", sentence(&e)));
            return Ok(sent);
        }
    };
    sent.track = Some(track.index);
    sent.problem = match link.insert_file_on(&track, &shown) {
        Ok(Inserted { added: false, .. }) => Some(format!("Bitwig didn't load {file}")),
        Ok(Inserted { device: Some(device), .. })
            if load == Load::Sound && !device.name.is_empty() && !device.name.to_lowercase().contains("sampler") =>
        {
            Some(format!("Bitwig put it on {} rather than a Sampler", device.name))
        }
        Ok(_) => None,
        // Bitwig may have put it on all the same: the link gave up waiting, or the connection went.
        Err(e) => {
            let what = if load == Load::Sound { "sound" } else { "preset" };
            Some(format!("The track is in Bitwig, but the {what} may not be on it: {}", sentence(&e)))
        }
    };
    Ok(sent)
}

/// What went into Bitwig for one MIDI part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentPart {
    /// The part's name, which its track has.
    pub name: String,
    /// The new track's place in Bitwig's project listing; none when Bitwig didn't say it made one (see `problem`).
    pub track: Option<usize>,
    /// The launcher slot its clip is in; none when it isn't, or may not be (see `problem`).
    pub slot: Option<usize>,
    /// What didn't go to plan, in words without a full stop, or none.
    pub problem: Option<String>,
    /// Whether Bitwig's play start marks where the part goes in its song ([`Bitwig::send_part_for_song`]).
    pub mark: Mark,
}

/// Whether Bitwig's play start marks the first of the song's bars a sent part was found for ([`SentPart::mark`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Mark {
    /// It wasn't asked for: the part wasn't sent for a song, or its clip didn't go in.
    #[default]
    NotAsked,
    /// The play start is at the first of the song's bars.
    Moved,
    /// Bitwig has another project open than the song's (its name; "" when it didn't say), where the bars are
    /// somewhere else: the play start is where it was.
    OtherProject(String),
    /// The extension is older than 3.23 (what answered): the play start is where it was.
    OldExtension(String),
    /// Bitwig didn't move it, or may not have: why, in words without a full stop.
    NotMoved(String),
}

impl SentPart {
    /// Whether it went in as asked: a clip on its new track.
    pub fn went_in(&self) -> bool {
        self.problem.is_none()
    }

    /// The headline for the activity card: "Keys · Pattern 3 is in Bitwig", "… didn't go in cleanly", or (with no
    /// track for it) "… didn't go into Bitwig".
    pub fn title(&self) -> String {
        match (&self.problem, self.track) {
            (None, _) => format!("{} is in Bitwig", self.name),
            (Some(_), Some(_)) => format!("{} didn't go in cleanly", self.name),
            (Some(_), None) => format!("{} didn't go into Bitwig", self.name),
        }
    }

    /// The line under it: where it went and what to do next, or what went wrong.
    pub fn detail(&self) -> String {
        match &self.problem {
            None => PART_IN.to_owned(),
            Some(problem) => format!("{problem}."),
        }
    }
}

/// Where a sent part goes, for the card while it's sent.
pub const PART_PLACE: &str = "As a clip on a new track";
/// The line under a part that went in.
pub const PART_IN: &str =
    "As a clip on a new track, at the project's tempo. Put an instrument on the track to hear it.";

/// The card's headline while a part is sent: "Putting Keys · Pattern 3 into Bitwig".
pub fn sending_part(name: &str) -> String {
    format!("Putting {name} into Bitwig")
}

/// The card's headline when a part couldn't be sent at all; the line under it is the error.
pub const PART_FAILED: &str = "Couldn't put the part into Bitwig";

/// Puts one MIDI part into the project open in Bitwig, through `link`: a new track, then the file as a clip on it.
fn part(link: &mut Link, path: &Path, name: &str) -> Result<SentPart, Error> {
    link.hello().can_send()?;
    let shown = path.to_string_lossy().into_owned();
    if !path.is_file() {
        return Err(Error::Missing { file: file_name(&shown).to_owned(), path: shown });
    }
    let mut sent = SentPart { name: name.to_owned(), track: None, slot: None, problem: None, mark: Mark::NotAsked };
    let track = match link.new_track(name) {
        Ok(track) => track,
        Err(e) => {
            sent.problem = Some(format!("{}. There may be an empty track for it in Bitwig", sentence(&e)));
            return Ok(sent);
        }
    };
    sent.track = Some(track.index);
    match link.add_clip_on(&track, &shown) {
        Ok(slot) => sent.slot = Some(slot),
        // Bitwig may have made the clip all the same: the link gave up waiting, or the connection went.
        Err(e) => {
            sent.problem = Some(format!("The track is in Bitwig, but the clip may not be on it: {}", sentence(&e)))
        }
    }
    Ok(sent)
}

/// Moves Bitwig's play start to the first of the song's bars, for a part just sent there
/// ([`Bitwig::send_part_for_song`]).
fn mark(link: &mut Link, bars: &SongLoop) -> Mark {
    // Only where the link keeps the step to the song's own project (`onlyIn`): the bars are somewhere else in any
    // other.
    if !link.only_in().is_some_and(|open| same_project(open, &bars.project)) {
        return Mark::OtherProject(link.hello().project.clone());
    }
    if !link.hello().at_least(PLAY_START_NEEDS) {
        return Mark::OldExtension(link.hello().answering());
    }
    match link.move_play_start(bars.start) {
        Ok(()) => Mark::Moved,
        Err(e) => Mark::NotMoved(sentence(&e)),
    }
}

/// A sound for a kit, from the kit builder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KitSound {
    /// The pad's name ("Kick", "Percussion 2").
    pub name: String,
    /// The kind of sound the pad is for ("Kick", "Closed hat"): the kit builder's first type for the pad.
    pub kind: String,
    /// The sample's full path.
    pub path: String,
}

/// A sample for one of a Drum Machine's pads: the MIDI note the pad plays on, the sample, and the pad's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pad {
    /// 36 is C1 (Bitwig's name for it), the kick's; 0 to 127, one sound to a note.
    pub note: u8,
    pub path: String,
    /// The name to give the pad; empty leaves Bitwig's (the sample's).
    pub name: String,
}

/// General MIDI's drum notes, which drum MIDI files and Bitwig's own kits follow: kick on C1 (36), snare on D1, clap
/// on D#1, closed hat on F#1, tom on A1, open hat on A#1, crash on C#2.
const DRUM_NOTES: [(&str, u8); 8] = [
    ("Kick", 36),
    ("Snare", 38),
    ("Clap", 39),
    ("Closed hat", 42),
    ("Hi-hat", 42),
    ("Tom", 45),
    ("Open hat", 46),
    ("Cymbal", 49),
];

/// The first note the pads take, C1.
const FIRST_PAD: u8 = 36;

/// Where each sound goes on a Drum Machine: the General MIDI note for its kind (or, with no kind, its name), so drum
/// MIDI plays the right pads, and the rest on the free notes from C1 up, in the order given. A second sound of a kind
/// takes a free note. Sounds past the last note (127) are left out. Sorted by note.
pub fn kit_pads(sounds: &[KitSound]) -> Vec<Pad> {
    let mut taken = BTreeSet::new();
    let mut notes: Vec<Option<u8>> = sounds
        .iter()
        .map(|s| {
            let kind = if s.kind.is_empty() { &s.name } else { &s.kind };
            let gm = DRUM_NOTES.iter().find(|(k, _)| k.eq_ignore_ascii_case(kind)).map(|&(_, note)| note);
            gm.filter(|&note| taken.insert(note))
        })
        .collect();
    let mut next = FIRST_PAD;
    for note in notes.iter_mut().filter(|n| n.is_none()) {
        while taken.contains(&next) && next < 127 {
            next += 1;
        }
        if taken.contains(&next) {
            break;
        }
        *note = Some(next);
        taken.insert(next);
    }
    let mut pads: Vec<Pad> = sounds
        .iter()
        .zip(notes)
        .filter_map(|(s, note)| Some(Pad { note: note?, path: s.path.clone(), name: s.name.clone() }))
        .collect();
    pads.sort_by_key(|p| p.note);
    pads
}

/// The kit's name in Bitwig: "Kit #12" for a kit rolled from seed 12, as the C# app named them; "Kit" without one.
pub fn kit_name(seed: i32) -> String {
    if seed > 0 { format!("Kit #{seed}") } else { "Kit".to_owned() }
}

/// Where a kit goes in Bitwig.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KitTarget {
    /// A new Drum Machine, by Bitwig's ID for it ([`super::drum_machine`]), on a new track named after the kit.
    NewDrumMachine(DeviceId),
    /// The Drum Machine selected in Bitwig, leaving pads that already have sounds alone: what's left once Bitwig
    /// didn't add a Drum Machine for the ID ([`SentKit::drum_machine`]), until the app restarts. It's what's
    /// selected when the kit's turn comes: a send just before it selects its own new track.
    Selected,
}

/// What happened to a kit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentKit {
    /// The kit's name, which its track has.
    pub name: String,
    /// Where it was sent.
    pub target: KitTarget,
    /// Its new track's place in the project listing; none when it went on the selected Drum Machine, or Bitwig
    /// didn't say it made the track (see `problem`).
    pub track: Option<usize>,
    /// False when Bitwig didn't add a Drum Machine to the new track for the ID: send to [`KitTarget::Selected`]
    /// from then on.
    pub drum_machine: bool,
    /// The pads that got their sound.
    pub filled: Vec<Pad>,
    /// Pads that already had something on them, left alone.
    pub skipped: Vec<Pad>,
    /// Pads Bitwig didn't fill (all of them when it didn't add a Drum Machine, or didn't show its pads in time).
    /// When the link couldn't say how the pads went, they're in none of these lists, and `problem` says why.
    pub failed: Vec<Pad>,
    /// Pads whose sound isn't where the app last saw it, left out.
    pub missing: Vec<Pad>,
    /// What stopped it part way, in words without a full stop, or none.
    pub problem: Option<String>,
}

impl SentKit {
    /// Whether every pad went on a Drum Machine as asked (pads that already had sounds aside).
    pub fn went_in(&self) -> bool {
        self.drum_machine
            && self.problem.is_none()
            && self.failed.is_empty()
            && self.missing.is_empty()
            && !self.filled.is_empty()
    }

    /// The headline for the activity card: "Kit #12 is in Bitwig", "Kit #12 didn't go into Bitwig" when no sound
    /// went on a pad, or "Kit #12 didn't go in cleanly".
    pub fn title(&self) -> String {
        // No pad got its sound: none did as far as the link could tell, or the link didn't get as far as the pads.
        let none_went_on = self.filled.is_empty()
            && (self.problem.is_none()
                || !self.failed.is_empty()
                || (self.track.is_none() && matches!(self.target, KitTarget::NewDrumMachine(_))));
        if !self.drum_machine {
            "Bitwig didn't add a Drum Machine".to_owned()
        } else if self.went_in() {
            format!("{} is in Bitwig", self.name)
        } else if none_went_on {
            format!("{} didn't go into Bitwig", self.name)
        } else {
            format!("{} didn't go in cleanly", self.name)
        }
    }

    /// Where the kit went and which sound is on which pad, with Bitwig's note names: "Kit #12 is on a Drum Machine on
    /// a new track in Bitwig: Kick on C1, Snare on D1." Then the pads left alone, those left out, and those Bitwig
    /// didn't load.
    pub fn detail(&self) -> String {
        if !self.drum_machine {
            return "Add one to a track yourself, select it, and send the kit again.".to_owned();
        }
        let on = |pads: &[Pad]| pads.iter().map(|p| format!("{} on {}", p.name, note_name(p.note))).collect::<Vec<_>>();
        let named =
            |pads: &[Pad]| pads.iter().map(|p| format!("{} ({})", p.name, note_name(p.note))).collect::<Vec<_>>();
        let place = match self.target {
            KitTarget::Selected => "the Drum Machine selected in Bitwig",
            KitTarget::NewDrumMachine(_) => "a Drum Machine on a new track in Bitwig",
        };
        let mut parts = Vec::new();
        if !self.filled.is_empty() {
            parts.push(format!("{} is on {place}: {}.", self.name, on(&self.filled).join(", ")));
        } else if self.problem.is_none() {
            parts.push(format!("Nothing went on {place}."));
        }
        if !self.skipped.is_empty() {
            let pads = named(&self.skipped).join(", ");
            parts.push(format!("The pads for {pads} already had sounds, so they were left alone."));
        }
        if !self.missing.is_empty() {
            let pads = named(&self.missing).join(", ");
            parts.push(if self.missing.len() == 1 {
                format!("{pads} isn't where the app last saw it, so it was left out.")
            } else {
                format!("{pads} aren't where the app last saw them, so they were left out.")
            });
        }
        if let Some(problem) = &self.problem {
            parts.push(format!("{problem}."));
        } else if !self.failed.is_empty() {
            parts.push(format!("Bitwig didn't load {}.", named(&self.failed).join(", ")));
        }
        parts.join(" ")
    }
}

/// The card's headline and line while a kit is sent: "Sending the kit to Bitwig", "10 pads".
pub fn sending_kit(pads: usize) -> (String, String) {
    let line = if pads == 1 { "1 pad".to_owned() } else { format!("{pads} pads") };
    ("Sending the kit to Bitwig".to_owned(), line)
}

/// The card's headline when a kit couldn't be sent at all; the line under it is the error.
pub const KIT_FAILED: &str = "Couldn't send the kit";

/// What the link says, in the extension's and the script's words, when the selected device can't take a kit, before
/// it puts anything on it.
const NOT_ON_THE_SELECTED: [&str; 3] = [
    "No device is selected in Bitwig. Select a Drum Machine and send the kit again.",
    "The selected device isn't a Drum Machine. Select one and send the kit again.",
    "Bitwig didn't show the Drum Machine's pads from the lowest note.",
];

/// Puts a kit's pads on a Drum Machine in the project open in Bitwig, through `link` (see [`Bitwig::send_kit`]).
fn kit(link: &mut Link, name: &str, pads: &[Pad], target: KitTarget) -> Result<SentKit, Error> {
    link.hello().can_send()?;
    if pads.is_empty() {
        return Err(Error::NoPads);
    }
    check_pads(pads)?;
    let (pads, missing): (Vec<Pad>, Vec<Pad>) = pads.iter().cloned().partition(|p| Path::new(&p.path).is_file());
    if pads.is_empty()
        && let Some(gone) = missing.first()
    {
        let (file, path) = (file_name(&gone.path).to_owned(), gone.path.clone());
        return Err(match missing.len() {
            1 => Error::Missing { file, path },
            sounds => Error::KitMissing { sounds, file, path },
        });
    }
    let mut sent = SentKit {
        name: name.to_owned(),
        target,
        track: None,
        drum_machine: true,
        filled: Vec::new(),
        skipped: Vec::new(),
        failed: Vec::new(),
        missing,
        problem: None,
    };
    let on = match target {
        KitTarget::Selected => None,
        KitTarget::NewDrumMachine(id) => {
            let track = match link.new_track(name) {
                Ok(track) => track,
                // Nothing went on a Drum Machine: the link may have stopped after Bitwig added the track, or its
                // answer didn't come.
                Err(e) => {
                    let why = sentence(&e);
                    sent.problem = Some(format!("{why}. There may be an empty track for the kit in Bitwig"));
                    return Ok(sent);
                }
            };
            sent.track = Some(track.index);
            match link.insert_device_on(&track, id) {
                // An ID Bitwig doesn't know adds nothing; one read wrongly adds some other device. One Bitwig hasn't
                // named yet goes ahead: the link says when it isn't a Drum Machine (below).
                Ok(Inserted { added: true, index: Some(at), device })
                    if device.as_ref().is_none_or(|d| d.name.is_empty() || d.name == DRUM_MACHINE_NAME) =>
                {
                    let named = device.is_some_and(|d| d.name == DRUM_MACHINE_NAME);
                    Some((track, at, named))
                }
                Ok(_) => {
                    sent.drum_machine = false;
                    sent.failed = pads;
                    return Ok(sent);
                }
                // Bitwig may have added it all the same: the link gave up waiting, or the connection went.
                Err(e) => {
                    let why = sentence(&e);
                    sent.problem =
                        Some(format!("The kit's track is in Bitwig, but the Drum Machine may not be on it: {why}"));
                    return Ok(sent);
                }
            }
        }
    };
    let filling = match &on {
        Some((track, device, _)) => link.fill_pads_on(&pads, track, *device),
        None => link.fill_pads(&pads, None),
    };
    match filling {
        Ok(filled) => {
            let of = |notes: &[u8]| pads.iter().filter(|p| notes.contains(&p.note)).cloned().collect::<Vec<_>>();
            sent.filled = of(&filled.filled);
            sent.skipped = of(&filled.skipped);
            sent.failed = of(&filled.failed);
        }
        // Nothing went on the selected device: Bitwig's words say why (it isn't a Drum Machine, say).
        Err(Error::Bitwig(words)) if on.is_none() && NOT_ON_THE_SELECTED.contains(&words.as_str()) => {
            return Err(Error::Bitwig(words));
        }
        // The link waited for the new device's pads and they didn't show, so it put nothing on them. For a device
        // Bitwig named Drum Machine, Bitwig was slow; for one it hadn't named, the ID may have given something else.
        Err(Error::Bitwig(words))
            if on.as_ref().is_some_and(|(_, d, _)| {
                // The track's place as the link found it, by its ID: it may have moved since it was made.
                words.starts_with(&format!("Device {d} on track ")) && words.ends_with(" isn't a Drum Machine.")
            }) =>
        {
            if on.as_ref().is_some_and(|(_, _, named)| *named) {
                sent.problem = Some(
                    "Bitwig added the Drum Machine but didn't show its pads in time, so the sounds didn't go on it"
                        .into(),
                );
            } else {
                sent.drum_machine = false;
            }
            sent.failed = pads;
        }
        // Some of the sounds may have gone on all the same: the link can stop after it put them on their pads.
        Err(e) => {
            let why = sentence(&e);
            sent.problem = Some(match target {
                KitTarget::NewDrumMachine(_) => {
                    format!("The Drum Machine is in Bitwig, but the sounds may not be on it: {why}")
                }
                KitTarget::Selected => format!("The sounds may not be on the Drum Machine selected in Bitwig: {why}"),
            });
        }
    }
    Ok(sent)
}

/// Pads the link takes: each on a MIDI note of its own, from 0 to 127, as [`kit_pads`] gives them.
fn check_pads(pads: &[Pad]) -> Result<(), Error> {
    let mut notes = BTreeSet::new();
    for pad in pads {
        if pad.note > 127 {
            return Err(Error::Invalid(format!("A pad's note is a MIDI note from 0 to 127, not {}.", pad.note)));
        }
        if !notes.insert(pad.note) {
            let note = note_name(pad.note);
            return Err(Error::Invalid(format!("Two sounds for the pad on {note}: a pad takes one.")));
        }
    }
    Ok(())
}

/// A MIDI note's name as Bitwig shows it, middle C (60) being C3: 36 is "C1", 39 "D#1".
pub fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let name = NAMES.get(usize::from(note % 12)).copied().unwrap_or("C");
    format!("{name}{}", i32::from(note / 12) - 2)
}

/// An error's words to go inside a sentence: without the full stop.
pub(super) fn sentence(e: &Error) -> String {
    e.to_string().trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::Instant;

    #[test]
    fn sends_take_their_turns_in_the_order_they_came() {
        let queue = Arc::new(Queue::new());
        let (went, order) = mpsc::channel();
        let first = queue.wait_turn();
        for n in 1..=6u64 {
            let (theirs, went) = (queue.clone(), went.clone());
            thread::spawn(move || {
                let _turn = theirs.wait_turn();
                let _ = went.send(n);
            });
            // Each takes its number before the next one asks.
            let deadline = Instant::now() + Duration::from_secs(10);
            while queue.numbers.lock().unwrap().next <= n {
                assert!(Instant::now() < deadline, "send {n} didn't take a number");
                thread::sleep(Duration::from_millis(1));
            }
        }
        drop(first);
        let came: Vec<u64> = (0..6).map(|_| order.recv_timeout(Duration::from_secs(10)).unwrap()).collect();
        assert_eq!(came, [1, 2, 3, 4, 5, 6]);

        // A send that stopped with a panic gives up its turn all the same.
        let theirs = queue.clone();
        let stopped = thread::spawn(move || {
            let _turn = theirs.wait_turn();
            panic!("a send that went wrong (on purpose, for this test)");
        });
        assert!(stopped.join().is_err());
        let theirs = queue.clone();
        thread::spawn(move || {
            let _turn = theirs.wait_turn();
            let _ = went.send(7);
        });
        assert_eq!(order.recv_timeout(Duration::from_secs(10)).unwrap(), 7);
    }

    #[test]
    fn a_ticket_keeps_its_place_and_one_given_up_is_skipped() {
        let queue = Queue::new();
        let first = queue.ticket();
        let given_up = queue.ticket();
        let third = queue.ticket();
        let (went, order) = mpsc::channel();
        thread::scope(|s| {
            let went_too = went.clone();
            // The third waits first, but goes after the first.
            s.spawn(move || {
                let _turn = third.wait();
                let _ = went_too.send(3);
            });
            thread::sleep(Duration::from_millis(20));
            drop(given_up);
            let turn = first.wait();
            let _ = went.send(1);
            drop(turn);
        });
        let came: Vec<u64> = (0..2).map(|_| order.recv_timeout(Duration::from_secs(10)).unwrap()).collect();
        assert_eq!(came, [1, 3]);
        // A ticket given up as its turn comes calls the next at once.
        let now = queue.ticket();
        let after = queue.ticket();
        drop(now);
        drop(after.wait());
        assert!(queue.numbers().given_up.is_empty());
    }
}
