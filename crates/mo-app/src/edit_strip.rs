//! The Edit strip with no screen (the 1.1 plan's light sample edits, mock-up 8, Solomon's "Build as shown" of 27 Nov
//! 2026): the sound open in it, its edits and their undo, the part picked on it, the fades as they're dragged, what
//! plays, and everything the strip shows. The app's Sounds page and the plugin's panel each keep one [`Strip`], tell it
//! what the mouse and the keys did, and copy what it shows ([`Showing`]) onto their own screen, so the strip works the
//! same in both. What differs between them is the [`Host`]'s: the list a sound is picked in, the player, the thread
//! the work is done on, and where a drag, a save or a send goes.
//!
//! E opens it on the picked sound, which is read whole (or, for a sound longer than [`edit_copy::EDIT_MOST_SECONDS`],
//! the part picked on its card) and edited by a list of edits ([`mo_audio::edit`]): trim to a part picked on the
//! strip's waveform, fades (its yellow corner handles, or the tools across a part), reverse, normalise, gain and a
//! loop's crossfade. Undo and redo step back and forth through the lists the edits made. Play plays the edit (only the
//! part picked, with one); holding O, or the A/B button, plays the original from the same moment. The edit is dragged
//! out, or saved (Ctrl+S), as a copy in the export folder's Edited sounds: the file itself never changes (rule 2).
//!
//! The reading and the edit's making are done away from the window's thread, newest wins (rule 12); the edit's outline
//! is kept fine enough to draw any part of it at once, so zooming in waits for nothing. Each sound's edits are kept
//! while the app is open, so going to another sound and back finds them where they were.
//!
//! Its Slices cut the edit for a drum machine's pads ([`slices`], mock-up 9).

use std::cell::RefCell;
use std::fmt::Write as _;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use mo_audio::AudioData;
use mo_audio::edit::{self, Curve, Edit, Span, Way};
use mo_audio::hits::USUAL_RISE_DB;

use crate::edit_copy::{self, Source, Unread};
use crate::{log, sound_part};

pub mod slices;

/// How many lines across the strip's waveform is.
const POINTS: usize = 600;

/// How fine the edit's outline is kept: the loudest sample of every quarter of a millisecond (half a million steps for
/// two minutes, 2 MB), so even a zoom in on a short part draws at once.
const STEP_SECONDS: f64 = 0.000_25;

/// A gain step, up or down (dB).
const GAIN_STEP_DB: f32 = 0.5;

/// The fade the fade-in tool makes at the start with no part picked (seconds): short, so a hit keeps its attack.
const SHORT_FADE_IN: f64 = 0.010;

/// The fade the fade-out tool makes at the end with no part picked: a tenth of the sound, at most a second.
const FADE_OUT_SHARE: f64 = 0.1;
const FADE_OUT_MOST: f64 = 1.0;

/// How long a loop's crossfade is (seconds); the edit makes it no more than half the loop.
const LOOP_CROSS_SECONDS: f64 = 0.020;

/// The shape of the fades the corner handles make: equal power, so a fade out stays full and then falls away, as a
/// sound dies, and a fade in keeps the attack.
const CORNER: Curve = Curve::EqualPower;

/// The shape of a fade across a part picked: a straight line.
const ACROSS: Curve = Curve::Linear;

/// How many sounds' edits are kept while the app is open.
const KEPT: usize = 64;

/// The ids of the strip's actions in the shortcut list.
pub const EDIT: &str = "sounds.edit";
pub const UNDO: &str = "sounds.editUndo";
pub const REDO: &str = "sounds.editRedo";
pub const SAVE: &str = "sounds.editSave";
pub const ORIGINAL: &str = "sounds.editOriginal";
pub const TRIM: &str = "sounds.editTrim";
pub const FADE_IN: &str = "sounds.editFadeIn";
pub const FADE_OUT: &str = "sounds.editFadeOut";
pub const REVERSE: &str = "sounds.editReverse";
pub const NORMALISE: &str = "sounds.editNormalise";
pub const LOUDER: &str = "sounds.editLouder";
pub const QUIETER: &str = "sounds.editQuieter";
pub const LOOP: &str = "sounds.editLoop";

/// What the strip asks of the screen it's on (the app's Sounds page, the plugin's panel): the sound picked in its
/// list, somewhere to show itself, a thread to read and make on, a player, and somewhere for what's dragged out, saved
/// or sent to go.
///
/// The host keeps the strip's state ([`Host::strip`]). Nothing here holds it borrowed while it asks the host for
/// anything, so a host may come back in as it's asked (a play that stops may show the strip again).
pub trait Host {
    /// The strip's state, which the host keeps: one strip to a window.
    fn strip(&self) -> &RefCell<Strip>;

    /// The sound picked in the host's list, which the strip opens on; none with none picked.
    fn picked(&self) -> Option<Picked>;

    /// The part picked on the picked sound outside the strip (on its card), from and to in seconds into it: where the
    /// strip starts on a sound read whole, and all that's read of a sound too long to edit whole.
    fn card_part(&self) -> Option<(f64, f64)>;

    /// Shows the strip as it is now.
    fn show(&self, showing: Showing);

    /// Does `work` away from the window's thread, the newest asked winning, and hands what [`answer`] makes of it to
    /// [`arrived`] with the same `ask` and `id`, back on the window's thread. (A host with no other thread to do it
    /// on may do both before it comes back, as the app does for its screenshots.)
    fn work(&self, ask: u64, id: i64, work: Work);

    /// Plays `audio` as the picked sound's, from `from` seconds into it and only as far as `till` when there is one:
    /// the strip's play.
    fn play(&self, audio: Arc<AudioData>, from: f64, till: Option<f64>);

    /// Plays `audio` in place of the strip's play, from where `place` puts the moment that play has got to (seconds
    /// into what plays, to seconds into `audio`). Whether it did: not once the strip's play has stopped, or another
    /// sound has started.
    fn swap(&self, audio: Arc<AudioData>, place: &dyn Fn(f64) -> f64) -> bool;

    /// Whether the strip's play is what plays now.
    fn plays(&self) -> bool;

    /// Whether the picked sound plays now: the strip's play, or the sound itself from its card.
    fn picked_plays(&self) -> bool;

    /// Stops what plays.
    fn stop(&self);

    /// The picked sound was played on purpose (Play, not a click along its waveform): the app counts that towards
    /// how liked it is.
    fn played(&self);

    /// Carries `what` of the picked sound out of the app, picked up on the strip.
    fn carry(&self, what: Carried);

    /// The mouse was let go before what's carried got going: it's called off.
    fn let_go(&self);

    /// Saves `edit` as a copy in Edited sounds ([`edit_copy::edit_to_keep`]), away from the window's thread, and says
    /// how it went. Whether it was taken.
    fn save(&self, edit: Saving) -> bool;

    /// Sends `slices`: each a file of its own in a folder for this slicing in Edited sounds
    /// ([`edit_copy::slices_to_keep`]), then put on a Drum Machine in Bitwig, or their folder opened; away from the
    /// window's thread, saying how it went. Whether they're on their way: Send then waits until the host calls
    /// [`slices::sent`] (once they're there or couldn't be; never before this comes back).
    fn send(&self, to: slices::To, slices: slices::Slices) -> bool;
}

/// The sound picked in a host's list, as the strip takes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Picked {
    pub id: i64,
    pub path: PathBuf,
    /// How long it lasts, as its analysis says (0 when that isn't known yet).
    pub seconds: f64,
    pub name: String,
    /// Its type's colour, as the host's screens number them.
    pub colour: i32,
    /// Its tempo, when that's known.
    pub bpm: Option<f64>,
}

/// What the strip hands its host to carry out of the app.
pub enum Carried {
    /// The sound's own file, whole.
    Whole,
    /// A part of the sound's own file, from and to in seconds into it.
    Part(f64, f64),
    /// The edits of what the strip read (`source`, read as `audio` when its file was last changed `changed`): a copy
    /// of the sound as they leave it.
    Edit { source: Source, audio: Arc<AudioData>, changed: Option<SystemTime>, edits: Vec<Edit> },
}

/// An edit to save as a copy: what the strip read (`source`, as `original`, when its file was last changed
/// `changed`), its edits, and the sound's name for the words about it.
pub struct Saving {
    pub source: Source,
    pub original: Arc<AudioData>,
    pub changed: Option<SystemTime>,
    pub edits: Vec<Edit>,
    pub name: String,
}

/// Something with undo and redo: what it is now, and what undo and redo go back and forth to. A drag changes it as it
/// goes ([`History::live`]) and is one step once it ends.
#[derive(Clone, Debug, Default, PartialEq)]
struct History<T> {
    now: T,
    undo: Vec<T>,
    redo: Vec<T>,
    /// What it was as a drag started, while one goes on.
    live: Option<T>,
}

impl<T: Clone + PartialEq> History<T> {
    /// Makes `next` what it is, with what it was kept for undo; what was undone is gone.
    fn change(&mut self, next: T) {
        self.end_live();
        if next == self.now {
            return;
        }
        self.undo.push(std::mem::replace(&mut self.now, next));
        self.redo.clear();
    }

    /// Makes `next` what it is while a drag goes on, without a step for undo: the step is made as it ends
    /// ([`History::end_live`]), back to what it was as it started.
    fn live(&mut self, next: T) {
        if self.live.is_none() {
            self.live = Some(self.now.clone());
        }
        self.now = next;
    }

    /// The drag ended: one step for undo, if it changed anything.
    fn end_live(&mut self) {
        if let Some(before) = self.live.take()
            && before != self.now
        {
            self.undo.push(before);
            self.redo.clear();
        }
    }

    fn undo(&mut self) -> bool {
        self.end_live();
        let Some(before) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.now, before));
        true
    }

    fn redo(&mut self) -> bool {
        self.end_live();
        let Some(after) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.now, after));
        true
    }

    /// Whether there's anything to undo or redo.
    fn has_steps(&self) -> bool {
        !(self.undo.is_empty() && self.redo.is_empty() && self.live.is_none())
    }
}

/// One sound's edits: the list applied now, and the lists undo and redo go back and forth to.
type Edits = History<Vec<Edit>>;

/// The edit as it sounds, made from the original and a list of edits.
pub struct Made {
    edits: Vec<Edit>,
    audio: Arc<AudioData>,
    /// Its outline: the loudest sample, on any channel, of each step [`STEP_SECONDS`] long (as near as whole frames go;
    /// `step` is how long each is).
    steps: Arc<[f32]>,
    step: f64,
    /// Where its hits are (seconds), those that rise by [`USUAL_RISE_DB`] or more.
    hits: Arc<[f64]>,
    /// Every hit, with how much it rises (dB): where the Slices' cuts go, as big as their sensitivity asks.
    all_hits: Arc<[(f64, f32)]>,
}

/// The sound the strip edits.
struct Opened {
    id: i64,
    source: Source,
    name: String,
    colour: i32,
    bpm: Option<f64>,
    /// Read and ready (none while it's read, or when it couldn't be: `note` says why).
    original: Option<Arc<AudioData>>,
    /// When its file was last changed as it was read: a copy is made only of the file as it still is.
    changed: Option<SystemTime>,
    note: String,
    edits: Edits,
    made: Option<Made>,
    /// The part picked on the strip's waveform, in seconds into the edit.
    pick: Option<(f64, f64)>,
    /// What the waveform shows, from and to in seconds into the edit: the whole of it for none.
    zoom: Option<(f64, f64)>,
    /// A fade's handle being dragged: the fade in's (`true`) or the fade out's, and where it is (seconds into the edit).
    dragging: Option<(bool, f64)>,
    /// The original is heard while O (or A/B) is held; `true` when the hold started the play (it stops when let go),
    /// `false` when it took over from the edit playing (the edit comes back when let go).
    held: Option<bool>,
    /// A press on the strip's waveform was on this sound: a pick made with it is this sound's alone.
    pressed: bool,
    /// Its Slices, once they've been on: whether they're on now, and where the cuts are.
    sliced: Option<slices::Sliced>,
    /// The slice picked (its number less one), heard as it's clicked.
    slice: Option<usize>,
    /// What a press on the Slices' waveform is doing, while the button is down.
    slice_press: Option<slices::Press>,
}

/// A sound's edits and slicing, kept while the app is open: by its id and what was read of it (a long sound's part).
struct Kept {
    id: i64,
    source: Source,
    edits: Edits,
    sliced: Option<slices::Sliced>,
}

/// The strip's state: one to a window, kept by its [`Host`].
#[derive(Default)]
pub struct Strip {
    open: bool,
    sound: Option<Opened>,
    /// Each sound's edits and slicing, kept while the app is open, the newest last.
    kept: Vec<Kept>,
    /// The sounds found too long to edit whole on reading them, though their analysis didn't say so (it has no
    /// length yet): the part picked on their card is read instead.
    long: Vec<i64>,
    /// Counts the reads and makes asked for: only the newest's answer is taken.
    asked: u64,
    /// The key that hears the original, as the shortcut list has it ("O").
    original_key: String,
    /// Slices are on their way to Bitwig or a folder: Send waits until they're there.
    sending: bool,
}

/// A read or a make, for the host's thread ([`Host::work`]).
pub enum Work {
    Read(Source),
    Make { original: Arc<AudioData>, edits: Vec<Edit> },
}

/// What came back from the host's thread ([`answer`]).
pub enum Answer {
    Read(Result<(Arc<AudioData>, Option<SystemTime>), Unread>),
    Made(Result<Made, String>),
}

/// A mark on the strip's ruler: where (0 to 1 across the waveform), what it says, and whether it's a bar's (brighter).
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub at: f32,
    pub text: String,
    pub bar: bool,
}

/// One edit made so far, as a chip under the waveform; the last one (the one Undo takes back) is lit. With the Slices
/// on, the chips are the pads ("1 → C1").
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub text: String,
    pub last: bool,
}

/// Everything the strip shows, for its host to copy onto the screen. All of it is given every time: what a sound still
/// being read can't say yet is empty, so nothing of the sound before it lingers.
#[derive(Clone, Debug, PartialEq)]
pub struct Showing {
    /// Open: the strip is on show.
    pub open: bool,
    /// Where copies go, for the tips ("the export folder's Edited sounds").
    pub folder_text: String,
    /// The sound it edits: its name, its type's colour, and what's picked ("0.47 s picked · 1 beat", or how long it is
    /// with nothing picked).
    pub name: String,
    pub colour: i32,
    pub picked_text: String,
    /// Why there's nothing to edit (it couldn't be read, it's too long): shown where the waveform goes. Empty for none.
    pub note: String,
    /// Read and ready to edit.
    pub ready: bool,
    /// The edit's waveform over the part on show (the whole of it unless zoomed in), a path `points` across and 100
    /// high; how long that part lasts and where it starts (seconds); and the hits as marks along its foot, a path 1000
    /// across and 10 high.
    pub shape: String,
    pub points: usize,
    pub view_seconds: f32,
    pub view_from: f32,
    pub zoomed: bool,
    pub hit_marks: String,
    /// The part picked on it (0 to 1 across the part on show; none unless `pick_to` is past `pick_from`), and whether
    /// there's a part to zoom in on (with the Slices on, the slice picked).
    pub has_part: bool,
    pub pick_from: f32,
    pub pick_to: f32,
    /// The fades' handles, 0 to 1 across (where the fade in ends and the fade out starts), each shown only while its
    /// end of the sound is in sight; and their curves, paths 1000 across and 100 high.
    pub fade_in: f32,
    pub fade_out: f32,
    pub fade_in_shown: bool,
    pub fade_out_shown: bool,
    pub fade_in_curve: String,
    pub fade_out_curve: String,
    pub ruler: Vec<Mark>,
    pub chips: Vec<Chip>,
    pub can_undo: bool,
    pub can_redo: bool,
    /// Any edits made now (undo may have taken them all back): something to save.
    pub has_edits: bool,
    /// The original is what's heard (O, or A/B, is held).
    pub original: bool,
    /// The gain of the part picked (or the whole sound), as the stepper shows it ("Gain −1.5 dB"), and whether it's
    /// been changed from none.
    pub gain_text: String,
    pub gain_changed: bool,
    /// What the drag handle carries ("Drag the edit").
    pub drag_text: String,
    /// The Slices.
    pub slices: slices::Showing,
}

impl Default for Showing {
    /// The strip with no sound in it.
    fn default() -> Self {
        Showing {
            open: false,
            folder_text: folder_text(),
            name: String::new(),
            colour: 0,
            picked_text: String::new(),
            note: String::new(),
            ready: false,
            shape: String::new(),
            points: 0,
            view_seconds: 0.0,
            view_from: 0.0,
            zoomed: false,
            hit_marks: String::new(),
            has_part: false,
            pick_from: 0.0,
            pick_to: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            fade_in_shown: false,
            fade_out_shown: false,
            fade_in_curve: String::new(),
            fade_out_curve: String::new(),
            ruler: Vec::new(),
            chips: Vec::new(),
            can_undo: false,
            can_redo: false,
            has_edits: false,
            original: false,
            gain_text: gain_text(0.0),
            gain_changed: false,
            drag_text: "Drag the edit".to_owned(),
            slices: slices::Showing::default(),
        }
    }
}

/// Sets the key that hears the original, as the shortcut list has it ("O"): letting go of it stops the original
/// ([`key_up`]).
pub fn set_original_key(strip: &mut Strip, key: &str) {
    key.clone_into(&mut strip.original_key);
}

/// Runs one of the strip's actions (its key, its button or its line on the card's menu), if it's one of them: whether
/// it did anything (`None` for any other action). E opens and closes the strip while a sound is picked; the rest work
/// while it's open on a sound read and ready (the gain and fade keys too), so with it closed their keys go on.
/// The Slices' own ([`slices`]) are among them.
pub fn run_action(host: &dyn Host, id: &str) -> Option<bool> {
    if id == EDIT {
        return Some(toggle(host));
    }
    if !id.starts_with("sounds.edit") {
        return None;
    }
    // With the Slices on, the strip's tools are theirs: undo and redo step through the cuts, Save saves the slices,
    // and the edit's own tools wait until they're off (the cuts are of the edit as it is).
    let (ready, slicing) = {
        let s = host.strip().borrow();
        (ready(&s), s.sound.as_ref().is_some_and(slices::on))
    };
    if !ready {
        return Some(false);
    }
    Some(match id {
        slices::SLICES => slices::toggle(host),
        slices::TO_BITWIG => slices::send(host, slices::To::Bitwig),
        ORIGINAL => hear_original(host, true),
        UNDO if slicing => slices::step(host, true),
        REDO if slicing => slices::step(host, false),
        SAVE if slicing => slices::send(host, slices::To::Folder),
        _ if slicing => false,
        UNDO => step_history(host, true),
        REDO => step_history(host, false),
        SAVE => save(host),
        TRIM => trim(host),
        FADE_IN => fade_tool(host, true),
        FADE_OUT => fade_tool(host, false),
        REVERSE => change(host, |o, _| {
            let span = o.pick.map(|p| span_of(o, p));
            push(&o.edits.now, Edit::Reverse(span))
        }),
        NORMALISE => change(host, |o, _| {
            // Twice in a row does nothing more.
            if o.edits.now.last() == Some(&Edit::Normalise { peak_db: 0.0 }) {
                None
            } else {
                push(&o.edits.now, Edit::Normalise { peak_db: 0.0 })
            }
        }),
        LOUDER => gain(host, GAIN_STEP_DB),
        QUIETER => gain(host, -GAIN_STEP_DB),
        LOOP => {
            let done = change(host, |o, audio| {
                let span = o.pick.map(|p| span_of(o, p));
                let frames = (LOOP_CROSS_SECONDS * f64::from(audio.sample_rate)).round() as usize;
                push(&o.edits.now, Edit::LoopCrossfade { span, frames })
            });
            if done {
                forget_place(host);
            }
            done
        }
        _ => return None,
    })
}

/// Whether the strip is open on a sound that's read and ready to edit.
fn ready(s: &Strip) -> bool {
    s.open && s.sound.as_ref().is_some_and(|o| o.original.is_some())
}

/// E (or the card's Edit): opens the strip on the picked sound, or closes it. Whether it did either (nothing with no
/// sound picked).
fn toggle(host: &dyn Host) -> bool {
    if host.strip().borrow().open {
        close(host);
        return true;
    }
    open(host)
}

/// Opens the strip on the picked sound (the right-click menu's Edit: the click picked it), or keeps it open on it.
/// Whether there was one to open it on.
pub fn open(host: &dyn Host) -> bool {
    if host.picked().is_none() {
        return false;
    }
    host.strip().borrow_mut().open = true;
    load(host);
    true
}

/// Closes the strip. Its sound's edits are kept; what was read is let go.
pub fn close(host: &dyn Host) {
    let was_open = {
        let mut s = host.strip().borrow_mut();
        let was = s.open;
        s.open = false;
        stash(&mut s);
        s.asked += 1;
        was
    };
    if !was_open {
        return;
    }
    stop(host);
    show(host);
}

/// Stops the strip's own play, if that's what plays (a right-click on its waveform; the strip closing).
pub fn stop(host: &dyn Host) {
    if host.plays() {
        host.stop();
    }
}

/// Esc's steps on the page, before the card's: the slice picked goes, then the Slices (back to the edit's tools); the
/// part picked on the strip goes, then the strip. Whether it did one.
pub fn escape(host: &dyn Host) -> bool {
    let (open, had_part) = {
        let mut s = host.strip().borrow_mut();
        let open = s.open;
        let had = s.sound.as_mut().is_some_and(|o| slices::escape(o) || o.pick.take().is_some());
        (open, had)
    };
    if !open {
        return false;
    }
    if had_part {
        show(host);
    } else {
        close(host);
    }
    true
}

/// Another sound was picked: the strip, if it's open, edits that one now (the last one's edits are kept).
pub fn sound_picked(host: &dyn Host) {
    let open = host.strip().borrow().open;
    if open {
        load(host);
    }
}

/// The part picked on the card changed: a sound too long to edit whole is edited a part at a time, so the strip
/// takes the new part (the last part's edits are kept, and come back with it).
pub fn card_part_changed(host: &dyn Host) {
    let open_on = {
        let s = host.strip().borrow();
        s.sound.as_ref().filter(|_| s.open).map(|o| (o.id, o.source.clone()))
    };
    let Some((id, source)) = open_on else { return };
    if source_now(host).is_some_and(|(picked, now)| picked.id == id && now != source) {
        load(host);
    }
}

/// The picked sound and what the strip reads of it: the whole sound; or, for one longer than
/// [`edit_copy::EDIT_MOST_SECONDS`], the part picked on its card (none, and then there's nothing to read).
fn source_now(host: &dyn Host) -> Option<(Picked, Source)> {
    let picked = host.picked()?;
    let span = if is_long(host, picked.id, picked.seconds) { host.card_part() } else { None };
    let source = Source { path: picked.path.clone(), span };
    Some((picked, source))
}

/// Whether the sound `id` (`seconds` long, as its analysis says, or 0 for not known) is too long to edit whole.
fn is_long(host: &dyn Host, id: i64, seconds: f64) -> bool {
    seconds > edit_copy::EDIT_MOST_SECONDS || host.strip().borrow().long.contains(&id)
}

/// Opens the picked sound in the strip: the one open already stays as it is; another is read (and the last one's
/// edits are kept).
fn load(host: &dyn Host) {
    let Some((picked, source)) = source_now(host) else {
        close(host);
        return;
    };
    let Picked { id, name, colour, bpm, seconds, .. } = picked;
    let same = host.strip().borrow().sound.as_ref().is_some_and(|o| o.id == id && o.source == source);
    if same {
        show(host);
        return;
    }
    // The last one's edit playing stops: what plays next is this one's.
    stop(host);
    // Too long, and no part picked on its card: nothing to read.
    let too_long = is_long(host, id, seconds) && source.span.is_none();
    let ask = {
        let mut s = host.strip().borrow_mut();
        stash(&mut s);
        let (edits, sliced) = s
            .kept
            .iter()
            .position(|k| k.id == id && k.source == source)
            .map(|at| {
                let k = s.kept.remove(at);
                (k.edits, k.sliced)
            })
            .unwrap_or_default();
        s.asked += 1;
        s.sound = Some(Opened {
            id,
            source: source.clone(),
            name,
            colour,
            bpm,
            original: None,
            changed: None,
            note: if too_long { Unread::TooLong.to_string() } else { String::new() },
            edits,
            made: None,
            pick: None,
            zoom: None,
            dragging: None,
            held: None,
            pressed: false,
            sliced,
            slice: None,
            slice_press: None,
        });
        s.asked
    };
    show(host);
    if !too_long {
        host.work(ask, id, Work::Read(source));
    }
}

/// Puts the open sound's edits and slicing with the kept ones (only a sound with edits, with something to undo or
/// redo, or with Slices worth keeping), the newest last, and lets the sound go.
fn stash(s: &mut Strip) {
    let Some(mut o) = s.sound.take() else { return };
    s.kept.retain(|k| !(k.id == o.id && k.source == o.source));
    // A line let go of as the sound changed is where it was dragged to.
    if let Some(sliced) = o.sliced.as_mut() {
        sliced.history.end_live();
    }
    let sliced = o.sliced.filter(slices::Sliced::worth_keeping);
    if !o.edits.now.is_empty() || o.edits.has_steps() || sliced.is_some() {
        s.kept.push(Kept { id: o.id, source: o.source, edits: o.edits, sliced });
        if s.kept.len() > KEPT {
            s.kept.remove(0);
        }
    }
}

/// Does `work`, on the host's thread: what [`arrived`] is handed.
pub fn answer(work: Work) -> Answer {
    match work {
        Work::Read(source) => {
            Answer::Read(edit_copy::read_to_edit(&source).map(|read| (Arc::new(read.audio), read.changed)))
        }
        Work::Make { original, edits } => Answer::Made(
            mo_catch::catch_crash(AssertUnwindSafe(|| make(&original, edits)))
                .map_err(|_| "Couldn't make the edit: the log says why.".to_owned()),
        ),
    }
}

/// The edit of `original` by `edits`, its outline and its hits.
fn make(original: &AudioData, edits: Vec<Edit>) -> Made {
    let audio = if edits.is_empty() { original.clone() } else { edit::apply(original, &edits) };
    let rate = f64::from(audio.sample_rate.max(1));
    let each = ((rate * STEP_SECONDS).round() as usize).max(1);
    let frames = audio.channels.iter().map(Vec::len).min().unwrap_or(0);
    let steps: Arc<[f32]> = (0..frames.div_ceil(each))
        .map(|i| {
            let (from, to) = (i * each, ((i + 1) * each).min(frames));
            audio.channels.iter().map(|c| c[from..to].iter().fold(0.0f32, |m, s| m.max(s.abs()))).fold(0.0, f32::max)
        })
        .collect();
    let all_hits: Arc<[(f64, f32)]> = edit_copy::hits_in(&audio).into();
    let hits: Arc<[f64]> = all_hits.iter().filter(|(_, rise)| *rise >= USUAL_RISE_DB).map(|(at, _)| *at).collect();
    Made { edits, audio: Arc::new(audio), steps, step: each as f64 / rate, hits, all_hits }
}

/// A read or a make came back: taken only while it's the newest asked for, of the sound still open.
pub fn arrived(host: &dyn Host, ask: u64, id: i64, answer: Answer) {
    let fresh = {
        let s = host.strip().borrow();
        s.asked == ask && s.sound.as_ref().is_some_and(|o| o.id == id)
    };
    if !fresh {
        return;
    }
    match answer {
        Answer::Read(Ok((audio, changed))) => {
            // A part picked on the card of a sound read whole is where the strip starts, while it has no edits.
            let card_part = host.card_part();
            if let Some(o) = host.strip().borrow_mut().sound.as_mut() {
                if o.source.span.is_none() && o.edits.now.is_empty() {
                    o.pick = card_part.filter(|(from, to)| *to <= audio.seconds() + 1e-6 && from < to);
                }
                o.original = Some(audio);
                o.changed = changed;
                o.note.clear();
            }
            make_again(host);
        }
        Answer::Read(Err(why)) => {
            let too_long = why == Unread::TooLong;
            {
                let mut s = host.strip().borrow_mut();
                // Its analysis didn't say: from now on the part picked on its card is read.
                if too_long && !s.long.contains(&id) {
                    s.long.push(id);
                    if s.long.len() > KEPT {
                        s.long.remove(0);
                    }
                }
                if let Some(o) = s.sound.as_mut() {
                    o.note = why.to_string();
                }
            }
            if !too_long {
                log::warning("Sounds", &format!("The Edit strip couldn't open the sound: {why}"));
            } else if host.card_part().is_some() {
                // A part is picked on its card already: that's read.
                load(host);
                return;
            }
        }
        Answer::Made(Ok(made)) => took_made(host, made),
        Answer::Made(Err(why)) => {
            if let Some(o) = host.strip().borrow_mut().sound.as_mut() {
                o.note = why;
            }
        }
    }
    show(host);
}

/// Asks for the edit to be made again from the edits as they are now, unless it's made of them already.
fn make_again(host: &dyn Host) {
    let job = {
        let mut s = host.strip().borrow_mut();
        let work = s.sound.as_ref().and_then(|o| {
            let original = o.original.clone()?;
            if o.made.as_ref().is_some_and(|m| m.edits == o.edits.now) {
                return None;
            }
            Some((o.id, Work::Make { original, edits: o.edits.now.clone() }))
        });
        work.map(|(id, work)| {
            s.asked += 1;
            (s.asked, id, work)
        })
    };
    if let Some((ask, id, work)) = job {
        host.work(ask, id, work);
    }
}

/// The edit was made: the strip shows it. A part picked or a zoom past its new end goes; the edit playing goes on
/// from the same moment in the new one when it's as long, and stops when it isn't (a trim or a loop changed where
/// everything is).
fn took_made(host: &dyn Host, made: Made) {
    let seconds = made.audio.seconds();
    let audio = made.audio.clone();
    let (longer_or_shorter, held) = {
        let mut s = host.strip().borrow_mut();
        let Some(o) = s.sound.as_mut() else { return };
        let was = o.made.as_ref().map(|m| m.audio.len());
        let changed = was.is_some_and(|n| n != made.audio.len());
        if changed || o.pick.is_some_and(|(_, to)| to > seconds + 1e-6) {
            o.pick = None;
        }
        if changed || o.zoom.is_some_and(|(_, to)| to > seconds + 1e-6) {
            o.zoom = None;
        }
        o.made = Some(made);
        // Made: a make that failed before is past.
        o.note.clear();
        // The Slices, if they're on, are of this edit now.
        slices::edit_made(o);
        (changed, o.held)
    };
    if !host.plays() || held.is_some() {
        return;
    }
    if longer_or_shorter {
        host.stop();
        return;
    }
    // Swapped in where the edit has got to, unless it has stopped or another sound has started meanwhile.
    let _ = host.swap(audio, &|at| at);
}

/// Applies `next` (what `edit` makes of the open sound's edits, given its original; none for no change) as the new
/// list of edits, kept for undo, and makes the edit again. Whether there was a change.
fn change(host: &dyn Host, edit: impl FnOnce(&Opened, &AudioData) -> Option<Vec<Edit>>) -> bool {
    let changed = changed_by(&mut host.strip().borrow_mut(), edit);
    if changed {
        make_again(host);
        show(host);
    }
    changed
}

/// [`change`]'s step on the strip itself: whether the open sound's edits changed.
fn changed_by(s: &mut Strip, edit: impl FnOnce(&Opened, &AudioData) -> Option<Vec<Edit>>) -> bool {
    let Some(o) = s.sound.as_mut() else { return false };
    let Some(original) = o.original.clone() else { return false };
    let Some(next) = edit(o, &original) else { return false };
    let before = o.edits.now.clone();
    o.edits.change(next);
    if o.edits.now == before {
        return false;
    }
    forget_place_if_moved(o, &original, &before);
    true
}

/// `edits` with `edit` after them.
fn push(edits: &[Edit], edit: Edit) -> Option<Vec<Edit>> {
    let mut next = edits.to_vec();
    next.push(edit);
    Some(next)
}

/// Undo (`back`) or redo.
fn step_history(host: &dyn Host, back: bool) -> bool {
    let stepped = {
        let mut s = host.strip().borrow_mut();
        s.sound.as_mut().is_some_and(|o| {
            let before = o.edits.now.clone();
            let stepped = if back { o.edits.undo() } else { o.edits.redo() };
            if stepped && let Some(original) = o.original.clone() {
                forget_place_if_moved(o, &original, &before);
            }
            stepped
        })
    };
    if stepped {
        make_again(host);
        show(host);
    }
    stepped
}

/// The edits changed from `before` to a sound of another length (a trim or a loop, or one undone): the part picked
/// and the zoom are of the sound as it was, so they go at once, before the edit is made again (a tool used meanwhile
/// would put them in the wrong place).
fn forget_place_if_moved(o: &mut Opened, original: &AudioData, before: &[Edit]) {
    if length_after(original, before) != length_after(original, &o.edits.now) {
        o.pick = None;
        o.zoom = None;
    }
}

/// How long `original` is (frames) after `edits`.
fn length_after(original: &AudioData, edits: &[Edit]) -> usize {
    edit::lengths(original.len(), edits).last().copied().unwrap_or(original.len())
}

/// A trim or a loop: the part picked and the zoom are of the sound before it.
fn forget_place(host: &dyn Host) {
    if let Some(o) = host.strip().borrow_mut().sound.as_mut() {
        o.pick = None;
        o.zoom = None;
    }
    show(host);
}

/// Keeps only the part picked.
fn trim(host: &dyn Host) -> bool {
    let done = change(host, |o, _| {
        let span = span_of(o, o.pick?);
        push(&o.edits.now, Edit::Trim(span))
    });
    if done {
        forget_place(host);
    }
    done
}

/// The fade-in tool (`true`) or the fade-out: across the part picked, or with none, a corner fade of the usual
/// length.
fn fade_tool(host: &dyn Host, fade_in: bool) -> bool {
    change(host, |o, original| {
        let way = if fade_in { Way::In } else { Way::Out };
        if let Some(p) = o.pick {
            return push(&o.edits.now, Edit::Fade { span: span_of(o, p), way, curve: ACROSS });
        }
        let seconds = edit_seconds(o, original);
        let long = if fade_in { SHORT_FADE_IN } else { (seconds * FADE_OUT_SHARE).min(FADE_OUT_MOST) };
        Some(with_corner(o, original, fade_in, long))
    })
}

/// The edits with the corner fade in (`fade_in`) or out made `seconds` long: the one there is changed (or taken away
/// at none), or a new one added.
fn with_corner(o: &Opened, original: &AudioData, fade_in: bool, seconds: f64) -> Vec<Edit> {
    let rate = f64::from(original.sample_rate.max(1));
    let n = edit::lengths(original.len(), &o.edits.now).last().copied().unwrap_or(0);
    let (fade_in_now, fade_out_now) = corners(original.len(), &o.edits.now);
    // Never past the other fade.
    let other = if fade_in { fade_out_now } else { fade_in_now }.map_or(0, |(_, len)| len);
    let frames = ((seconds * rate).round() as usize).min(n.saturating_sub(other));
    let fade = |frames: usize| {
        if fade_in {
            Edit::Fade { span: Span::new(0, frames), way: Way::In, curve: CORNER }
        } else {
            Edit::Fade { span: Span::new(n - frames, n), way: Way::Out, curve: CORNER }
        }
    };
    let mut next = o.edits.now.clone();
    match (if fade_in { fade_in_now } else { fade_out_now }, frames >= 2) {
        (Some((at, _)), true) => next[at] = fade(frames),
        (Some((at, _)), false) => {
            next.remove(at);
        }
        (None, true) => next.push(fade(frames)),
        (None, false) => {}
    }
    next
}

/// A corner fade among a sound's edits: its place in the list, and how long it is (frames).
type Corner = Option<(usize, usize)>;

/// The corner fades among `edits` (of a sound `frames` long before them): a fade in from the start, and a fade out to
/// the end, made since the last edit that moved where everything is (a trim, a reverse or a loop; one of those leaves
/// its fades baked in).
fn corners(frames: usize, edits: &[Edit]) -> (Corner, Corner) {
    let lengths = edit::lengths(frames, edits);
    let (mut fade_in, mut fade_out) = (None, None);
    for (i, (e, &n)) in edits.iter().zip(&lengths).enumerate() {
        match *e {
            Edit::Fade { span, way: Way::In, curve: CORNER } if span.start == 0 => {
                if let Some(s) = span.within(n) {
                    fade_in = Some((i, s.len()));
                }
            }
            Edit::Fade { span, way: Way::Out, curve: CORNER } if span.end >= n => {
                if let Some(s) = span.within(n) {
                    fade_out = Some((i, s.len()));
                }
            }
            Edit::Trim(_) | Edit::Reverse(_) | Edit::LoopCrossfade { .. } => (fade_in, fade_out) = (None, None),
            _ => {}
        }
    }
    (fade_in, fade_out)
}

/// A fade's handle dragged to `at` (0 to 1 across what's shown; the fade in's for `true`): its curve follows at once;
/// the edit is made as it's let go.
pub fn fade_moving(host: &dyn Host, fade_in: bool, at: f64) {
    if let Some(o) = host.strip().borrow_mut().sound.as_mut() {
        let (from, to) = view(o);
        o.dragging = Some((fade_in, from + at.clamp(0.0, 1.0) * (to - from)));
    }
    show(host);
}

/// A fade's handle let go: the edit is made with the fade as long as it was dragged.
pub fn fade_let_go(host: &dyn Host, fade_in: bool) {
    let dragged = host.strip().borrow_mut().sound.as_mut().and_then(|o| o.dragging.take());
    let Some((which, at)) = dragged.filter(|(which, _)| *which == fade_in) else {
        show(host);
        return;
    };
    let changed = change(host, |o, original| {
        let seconds = edit_seconds(o, original);
        let long = if which { at } else { seconds - at };
        Some(with_corner(o, original, which, long.max(0.0)))
    });
    if !changed {
        show(host);
    }
}

/// The gain stepper clicked: the part picked (the whole sound with none) is turned down a step for `by` under 0, else
/// up.
pub fn gain_stepped(host: &dyn Host, by: i32) -> bool {
    gain(host, if by < 0 { -GAIN_STEP_DB } else { GAIN_STEP_DB })
}

/// The gain stepper's number clicked: back to no gain.
pub fn gain_reset(host: &dyn Host) -> bool {
    gain_to(host, 0.0)
}

/// Turns the part picked (the whole sound with none) up or down by `db` more.
fn gain(host: &dyn Host, db: f32) -> bool {
    let now = host.strip().borrow().sound.as_ref().map_or(0.0, gain_now);
    gain_to(host, ((now + db) * 2.0).round() / 2.0)
}

/// The gain of the part picked (the whole sound with none) as the stepper has it: the last edit's, when it's a gain of
/// that same part, else none.
fn gain_now(o: &Opened) -> f32 {
    let span = o.pick.map(|p| span_of(o, p));
    match o.edits.now.last() {
        Some(Edit::Gain { db, span: s }) if *s == span => *db,
        _ => 0.0,
    }
}

/// Sets the stepper's gain to `db`: the last edit, when it's a gain of the same part, is changed (or taken away at
/// none); else a new one is added. Each step is one undo.
fn gain_to(host: &dyn Host, db: f32) -> bool {
    change(host, |o, _| {
        let span = o.pick.map(|p| span_of(o, p));
        let mut next = o.edits.now.clone();
        if matches!(next.last(), Some(Edit::Gain { span: s, .. }) if *s == span) {
            next.pop();
        }
        if db.abs() >= 0.01 {
            next.push(Edit::Gain { db, span });
        }
        Some(next)
    })
}

/// How long the edit is (seconds), as its edits now make it (it may still be being made).
fn edit_seconds(o: &Opened, original: &AudioData) -> f64 {
    length_after(original, &o.edits.now) as f64 / f64::from(original.sample_rate.max(1))
}

/// The frames of the part `p` (seconds) of the edit.
fn span_of(o: &Opened, p: (f64, f64)) -> Span {
    let rate = f64::from(o.original.as_ref().map_or(1, |a| a.sample_rate.max(1)));
    Span::new((p.0 * rate).round() as usize, (p.1 * rate).round() as usize)
}

/// What the waveform shows, from and to in seconds into the edit.
fn view(o: &Opened) -> (f64, f64) {
    let seconds =
        o.made.as_ref().map(|m| m.audio.seconds()).or(o.original.as_ref().map(|a| a.seconds())).unwrap_or(0.0);
    o.zoom.unwrap_or((0.0, seconds))
}

/// The part of the edit that's picked: the part picked on its waveform; with the Slices on, the slice picked.
fn part_now(o: &Opened) -> Option<(f64, f64)> {
    if slices::on(o) { slices::picked(o) } else { o.pick }
}

/// Zooms in on the part picked (`true`), a little either side of it, or back out to the whole sound.
pub fn zoom(host: &dyn Host, on: bool) {
    if let Some(o) = host.strip().borrow_mut().sound.as_mut() {
        o.zoom = None;
        if on && let Some((from, to)) = part_now(o) {
            let (_, end) = view(o);
            let room = ((to - from) * 0.1).max(0.01);
            o.zoom = Some(((from - room).max(0.0), (to + room).min(end)));
        }
    }
    show(host);
}

/// A press on the strip's waveform: a part picked with it is the sound's that's open now ([`part_picked`]).
pub fn wave_pressed(strip: &mut Strip) {
    if let Some(o) = strip.sound.as_mut() {
        o.pressed = true;
    }
}

/// A part picked on the strip's waveform, from and to 0 to 1 across what's shown; let go when it's too short to be one
/// (a click outside it).
pub fn part_picked(host: &dyn Host, from: f64, to: f64) {
    if let Some(o) = host.strip().borrow_mut().sound.as_mut()
        // Pressed on another sound's waveform (the arrow keys picked this one while the button was down).
        && std::mem::take(&mut o.pressed)
    {
        let (start, end) = view(o);
        let place = |f: f64| start + f.clamp(0.0, 1.0) * (end - start);
        let (from, to) = (place(from.min(to)), place(from.max(to)));
        o.pick = (to - from >= edit_copy::LEAST_PART_SECONDS).then_some((from, to));
    }
    show(host);
}

/// Where a part's edge goes with the mouse at `at` (0 to 1 across what's shown): on the nearest hit no further than
/// `within`, else at `at`.
pub fn snap(strip: &Strip, at: f32, within: f32) -> f32 {
    let Some(o) = strip.sound.as_ref() else { return at };
    let Some(made) = o.made.as_ref() else { return at };
    let (from, to) = view(o);
    let long = (to - from).max(1e-9);
    let hits: Vec<f64> = made.hits.iter().map(|h| (h - from) / long).collect();
    sound_part::snap_to(&hits, f64::from(at), f64::from(within)) as f32
}

/// A click on the strip's waveform, `at` (0 to 1 across what's shown): the edit plays from there, to the end of the
/// part picked when it's inside it, else to its end.
pub fn seek(host: &dyn Host, at: f64) {
    let place = {
        let s = host.strip().borrow();
        s.sound.as_ref().map(|o| {
            let (start, end) = view(o);
            (start + at.clamp(0.0, 1.0) * (end - start), o.pick)
        })
    };
    let Some((from, pick)) = place else { return };
    let till = pick.filter(|(a, b)| from >= *a && from < *b).map(|(_, b)| b);
    play_edit(host, from, till);
}

/// Play on the page with the strip open on the picked sound: the edit plays (the part picked, or the slice, with
/// one), or stops. Whether it was the strip's to do.
pub fn toggle_play(host: &dyn Host) -> bool {
    let open_on = {
        let s = host.strip().borrow();
        s.sound.as_ref().filter(|o| s.open && o.made.is_some()).map(|o| (o.id, part_now(o)))
    };
    let Some((id, pick)) = open_on else { return false };
    if host.picked().map(|picked| picked.id) != Some(id) {
        return false;
    }
    // The picked sound playing (its edit, or itself from the card) stops; else the edit plays, a play you meant.
    if host.picked_plays() {
        host.stop();
    } else {
        play_edit(host, pick.map_or(0.0, |(from, _)| from), pick.map(|(_, to)| to));
        host.played();
    }
    true
}

/// Plays the edit from `from` seconds, as far as `till`.
fn play_edit(host: &dyn Host, from: f64, till: Option<f64>) {
    let audio = {
        let mut s = host.strip().borrow_mut();
        s.sound.as_mut().and_then(|o| {
            let audio = o.made.as_ref()?.audio.clone();
            o.held = None;
            Some(audio)
        })
    };
    if let Some(audio) = audio {
        host.play(audio, from, till);
    }
}

/// O held (`on`) or let go, or the A/B button: the original plays from the moment the edit has got to, and the edit
/// comes back from the same moment as it's let go. With nothing playing, the original plays while it's held (from the
/// start of the part picked, or of the slice picked with the Slices on, if there is one). Whether it did anything.
pub fn hear_original(host: &dyn Host, on: bool) -> bool {
    let opened = {
        let s = host.strip().borrow();
        s.sound.as_ref().filter(|_| s.open).and_then(|o| {
            let made = o.made.as_ref()?;
            Some((o.original.clone()?, made.audio.clone(), made.edits.clone(), part_now(o), o.held))
        })
    };
    let Some((original, edited, edits, pick, held)) = opened else { return false };
    let rate = f64::from(original.sample_rate.max(1));
    let frames = original.len();
    let before = |seconds: f64| edit::place_before(frames, &edits, (seconds * rate).round() as usize) as f64 / rate;
    let after = |seconds: f64| edit::place_after(frames, &edits, (seconds * rate).round() as usize) as f64 / rate;
    let plays = host.plays();
    let set_held = |held: Option<bool>| {
        if let Some(o) = host.strip().borrow_mut().sound.as_mut() {
            o.held = held;
        }
    };
    if on {
        // Held already (the key repeats).
        if held.is_some() {
            return true;
        }
        if plays && host.swap(original.clone(), &before) {
            set_held(Some(false));
        } else {
            host.play(original, before(pick.map_or(0.0, |(from, _)| from)), None);
            set_held(Some(true));
        }
    } else {
        match held {
            None => return false,
            // Started by the hold: it stops with it.
            Some(true) => {
                set_held(None);
                if plays {
                    host.stop();
                }
            }
            Some(false) => {
                set_held(None);
                if plays {
                    let _ = host.swap(edited, &after);
                }
            }
        }
    }
    show(host);
    true
}

/// A key let go, by its name as the shortcut list has it (with no Ctrl, Shift or Alt): O stops the original.
pub fn key_up(host: &dyn Host, name: &str) {
    let ours = {
        let s = host.strip().borrow();
        // Its last key: with Ctrl or Shift in its shortcut, those may be let go first.
        !name.is_empty() && s.original_key.rsplit('+').next() == Some(name)
    };
    if ours {
        hear_original(host, false);
    }
}

/// What a tick of the host's ticker comes to, while the strip's play plays ([`tick`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ticked {
    /// Nothing to show yet (the player hasn't the play, or the sound has gone): it goes on.
    Waiting,
    /// It goes on, and is here: 0 to 1 across what the strip's waveform shows.
    At(f32),
    /// A play of the part picked has come to the part's end: it stops.
    Ended,
}

/// The host's ticker, while the strip's play plays: `at` is where the player is (seconds into what it plays; none
/// until it has the play), and `till` where the play stops (seconds into the edit: the end of the part it's of). The
/// strip's waveform shows where the edit has got to; a play of the part picked stops at its end.
pub fn tick(strip: &Strip, at: Option<f64>, till: Option<f64>) -> Ticked {
    let Some(at) = at else { return Ticked::Waiting };
    let Some(o) = strip.sound.as_ref() else { return Ticked::Waiting };
    let (Some(made), Some(original)) = (o.made.as_ref(), o.original.as_ref()) else { return Ticked::Waiting };
    let held = o.held.is_some();
    // Hearing the original: where that is in the edit.
    let at = if held {
        let rate = f64::from(original.sample_rate.max(1));
        edit::place_after(original.len(), &made.edits, (at * rate).round() as usize) as f64 / rate
    } else {
        at
    };
    if !held && till.is_some_and(|till| at >= till) {
        return Ticked::Ended;
    }
    let (from, to) = view(o);
    Ticked::At(((at - from) / (to - from).max(1e-9)).clamp(0.0, 1.0) as f32)
}

/// Saves the edit as a copy in Edited sounds; the host says how it went. Whether there was an edit to save.
fn save(host: &dyn Host) -> bool {
    let edit = {
        let s = host.strip().borrow();
        s.sound.as_ref().and_then(|o| {
            Some(Saving {
                source: o.source.clone(),
                original: o.original.clone()?,
                changed: o.changed,
                edits: o.edits.now.clone(),
                name: o.name.clone(),
            })
        })
    };
    let Some(edit) = edit.filter(|edit| !edit.edits.is_empty()) else { return false };
    host.save(edit)
}

/// Drags the edit out of the app (its part, for `part`: a press inside the part picked on the strip that moved), as a
/// copy in Edited sounds. With no edits (and no part), the sound itself goes, as from the card.
pub fn drag(host: &dyn Host, part: bool) {
    let pick = host.strip().borrow().sound.as_ref().and_then(|o| o.pick);
    if part && pick.is_none() {
        return;
    }
    carry(host, pick.filter(|_| part));
}

/// Drags the edit out of the app, or only `part` of it (from and to, seconds into the edit: the part picked, or a
/// slice), as a copy in Edited sounds. With no edits and no part, the sound itself goes, as from the card.
fn carry(host: &dyn Host, part: Option<(f64, f64)>) {
    let opened = {
        let s = host.strip().borrow();
        s.sound.as_ref().and_then(|o| Some((o.source.clone(), o.original.clone()?, o.changed, o.edits.now.clone())))
    };
    let Some((source, original, changed, mut edits)) = opened else { return };
    if let Some(p) = part {
        let rate = f64::from(original.sample_rate.max(1));
        edits.push(Edit::Trim(Span::new((p.0 * rate).round() as usize, (p.1 * rate).round() as usize)));
    }
    let what = if !edits.is_empty() {
        Carried::Edit { source, audio: original, changed, edits }
    } else if let Some((from, to)) = source.span {
        Carried::Part(from, to)
    } else {
        Carried::Whole
    };
    host.carry(what);
}

/// Shows the strip as it is now.
pub fn show(host: &dyn Host) {
    let showing = showing(&host.strip().borrow());
    host.show(showing);
}

/// The strip as it is now.
fn showing(s: &Strip) -> Showing {
    let mut show = Showing { open: s.open, ..Showing::default() };
    let Some(o) = s.sound.as_ref() else { return show };
    // With the Slices on, undo and redo are the cuts', and the chips name the pads.
    let slicing = slices::on(o);
    (show.can_undo, show.can_redo) =
        if slicing { slices::steps(o) } else { (!o.edits.undo.is_empty(), !o.edits.redo.is_empty()) };
    show.name.clone_from(&o.name);
    show.colour = o.colour;
    show.note.clone_from(&o.note);
    show.has_edits = !o.edits.now.is_empty();
    show.chips = if slicing { slices::pad_chips(o) } else { chips(o) };
    show.slices = slices::showing(o, s.sending);
    show.original = o.held.is_some();
    let gain = gain_now(o);
    show.gain_text = gain_text(gain);
    show.gain_changed = gain != 0.0;
    show.drag_text = if o.edits.now.is_empty() { "Drag the sound" } else { "Drag the edit" }.to_owned();
    let (Some(original), Some(made)) = (o.original.as_ref(), o.made.as_ref()) else { return show };
    show.ready = o.note.is_empty();
    let seconds = made.audio.seconds();
    let (from, to) = view(o);
    let long = (to - from).max(1e-9);
    let across = |t: f64| ((t - from) / long) as f32;
    // The waveform over what's shown, at full scale (so a gain shows), its lines the loudest of their share.
    let first = ((from / made.step).floor() as usize).min(made.steps.len());
    let past = ((to / made.step).ceil() as usize).clamp(first, made.steps.len());
    let lines = sound_part::lines(&made.steps[first..past], POINTS);
    show.shape = wave_path(&lines);
    show.points = lines.len();
    show.view_seconds = long as f32;
    show.view_from = from as f32;
    show.zoomed = o.zoom.is_some();
    let hits: Vec<f64> = made.hits.iter().filter(|h| **h >= from && **h <= to).map(|h| (h - from) / long).collect();
    show.hit_marks = sound_part::marks(&hits);
    match o.pick {
        // The Slices have their own: how many, and the one picked (which the zoom goes to).
        _ if slicing => {
            show.has_part = slices::picked(o).is_some();
            show.picked_text = slices::heading(o, seconds);
        }
        Some((a, b)) => {
            show.has_part = true;
            show.pick_from = across(a);
            show.pick_to = across(b);
            show.picked_text = picked_text(b - a, o.bpm);
        }
        None => show.picked_text = sound_part::length_text(seconds),
    }
    // The corner fades: as the edits make them, or where a handle is being dragged.
    let rate = f64::from(original.sample_rate.max(1));
    let (fade_in, fade_out) = corners(original.len(), &made.edits);
    let mut fade_in = fade_in.map_or(0.0, |(_, n)| n as f64 / rate);
    let mut fade_out = seconds - fade_out.map_or(0.0, |(_, n)| n as f64 / rate);
    match o.dragging {
        Some((true, at)) => fade_in = at.min(fade_out),
        Some((false, at)) => fade_out = at.max(fade_in),
        None => {}
    }
    show.fade_in = across(fade_in);
    show.fade_out = across(fade_out);
    show.fade_in_shown = from <= 1e-9 || fade_in > from;
    show.fade_out_shown = to >= seconds - 1e-9 || fade_out < to;
    show.fade_in_curve = fade_curve(across(0.0), across(fade_in), true);
    show.fade_out_curve = fade_curve(across(fade_out), across(seconds), false);
    show.ruler = ruler(from, to, o.bpm);
    show
}

/// "Gain −1.5 dB", "Gain +2.0 dB", "Gain 0.0 dB".
fn gain_text(db: f32) -> String {
    if db.abs() < 0.01 {
        "Gain 0.0 dB".to_owned()
    } else if db < 0.0 {
        format!("Gain \u{2212}{:.1} dB", -db)
    } else {
        format!("Gain +{db:.1} dB")
    }
}

/// What's picked, as the strip's heading says it: "0.47 s picked · 1 beat" (with the tempo known).
fn picked_text(seconds: f64, bpm: Option<f64>) -> String {
    let mut text = format!("{} picked", sound_part::length_text(seconds));
    if let Some(bpm) = bpm {
        let beats = seconds * bpm / 60.0;
        let whole = beats.round();
        if whole >= 1.0 && (beats - whole).abs() < 0.08 {
            let bars = whole / 4.0;
            if whole >= 8.0 && bars.fract() == 0.0 {
                let _ = write!(text, " · {bars} bar{}", if bars == 1.0 { "" } else { "s" });
            } else {
                let _ = write!(text, " · {whole} beat{}", if whole == 1.0 { "" } else { "s" });
            }
        } else if beats >= 0.1 {
            let _ = write!(text, " · {beats:.1} beats");
        }
    }
    text
}

/// The edits as chips: "Fade in 6 ms", "Gain −1.5 dB"…, the last one lit.
fn chips(o: &Opened) -> Vec<Chip> {
    let rate = f64::from(o.original.as_ref().map_or(1, |a| a.sample_rate.max(1)));
    let count = o.edits.now.len();
    o.edits.now.iter().enumerate().map(|(i, e)| Chip { text: chip_text(e, rate), last: i + 1 == count }).collect()
}

fn chip_text(e: &Edit, rate: f64) -> String {
    let long = |s: Span| span_length(s.len() as f64 / rate);
    match *e {
        Edit::Trim(s) => format!("Trim to {}", long(s)),
        Edit::Fade { span, way: Way::In, .. } => format!("Fade in {}", long(span)),
        Edit::Fade { span, way: Way::Out, .. } => format!("Fade out {}", long(span)),
        Edit::Reverse(None) => "Reverse".to_owned(),
        Edit::Reverse(Some(s)) => format!("Reverse {}", long(s)),
        Edit::Gain { db, .. } => gain_text(db),
        Edit::Normalise { .. } => "Normalise".to_owned(),
        Edit::LoopCrossfade { .. } => "Loop crossfade".to_owned(),
    }
}

/// A short length: "6 ms" under a second, else as the card says it ("0.47 s" never comes up here: "470 ms").
fn span_length(seconds: f64) -> String {
    if seconds < 1.0 { format!("{} ms", (seconds * 1000.0).round()) } else { sound_part::length_text(seconds) }
}

/// The lines of a waveform at full scale, as a path `lines.len()` across and 100 high: each a line up and down from
/// the middle, as long as its loudest sample (full scale is the whole height).
fn wave_path(lines: &[f32]) -> String {
    let mut out = String::with_capacity(lines.len() * 16);
    for (i, p) in lines.iter().enumerate() {
        let half = (f64::from(*p) * 50.0).clamp(0.5, 50.0);
        let _ = write!(out, "M{i}.5 {:.1}V{:.1}", 50.0 - half, 50.0 + half);
    }
    out
}

/// A corner fade's curve over the waveform, a path 1000 across and 100 high: from silence at the bottom at `from` to
/// full at the top at `to` (0 to 1 across, either may be out of sight), a fade in; or the other way, a fade out. None
/// for a fade that isn't there.
fn fade_curve(from: f32, to: f32, fade_in: bool) -> String {
    if to - from <= 1e-6 {
        return String::new();
    }
    let mut out = String::new();
    const STEPS: usize = 24;
    for i in 0..=STEPS {
        let x = i as f32 / STEPS as f32;
        let level = if fade_in { CORNER.gain(x) } else { CORNER.gain(1.0 - x) };
        let px = (from + (to - from) * x) * 1000.0;
        let py = 100.0 - level * 100.0;
        let _ = write!(out, "{}{px:.1} {py:.1}", if i == 0 { "M" } else { " L" });
    }
    out
}

/// The ruler's marks between `from` and `to` seconds: bars and beats from the start when the tempo is known ("1",
/// "1.2", "1.3", "1.4", "2"…), else seconds; never more than about a dozen and a half.
fn ruler(from: f64, to: f64, bpm: Option<f64>) -> Vec<Mark> {
    let long = to - from;
    if long <= 0.0 {
        return Vec::new();
    }
    let mut marks = Vec::new();
    let mut mark = |t: f64, text: String, bar: bool| {
        marks.push(Mark { at: ((t - from) / long) as f32, text, bar });
    };
    const MOST: f64 = 18.0;
    match bpm {
        Some(bpm) => {
            let beat = 60.0 / bpm;
            // Every beat, or every so many bars.
            let beats_each = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0]
                .into_iter()
                .find(|n| long / (beat * n) <= MOST)
                .unwrap_or(256.0);
            let each = beat * beats_each;
            let mut k = (from / each).ceil() as i64;
            while (k as f64) * each <= to + 1e-9 {
                let beats = (k as f64 * beats_each).round() as i64;
                let (bar, in_bar) = (beats / 4 + 1, beats % 4 + 1);
                if in_bar == 1 {
                    mark(k as f64 * each, bar.to_string(), true);
                } else {
                    mark(k as f64 * each, format!("{bar}.{in_bar}"), false);
                }
                k += 1;
            }
        }
        None => {
            let each = [0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0]
                .into_iter()
                .find(|e| long / e <= MOST)
                .unwrap_or(60.0);
            let decimals = if each >= 1.0 {
                0
            } else if each >= 0.1 {
                1
            } else if each >= 0.01 {
                2
            } else {
                3
            };
            let mut k = (from / each).ceil() as i64;
            while (k as f64) * each <= to + 1e-9 {
                let t = k as f64 * each;
                let whole = (t - t.round()).abs() < 1e-9;
                mark(t, format!("{t:.decimals$}"), whole);
                k += 1;
            }
        }
    }
    marks
}

/// Where copies go, as the tips say it: the export folder's Edited sounds.
fn folder_text() -> String {
    format!("the export folder's {}", edit_copy::FOLDER)
}

/// For the screenshots: opens the strip on the picked sound, as if `audio` were what its file held (the made-up sounds
/// have none), makes `tools`' edits (by their ids in the shortcut list) and picks a part of it (from and to, 0 to 1 of
/// it), all at once: the host does the work as it's asked for ([`Host::work`]), with no thread.
pub fn for_screens(host: &dyn Host, audio: AudioData, part: Option<(f64, f64)>, tools: &[&str]) {
    let open = host.strip().borrow().open;
    if !open {
        toggle(host);
    }
    let (ask, id) = {
        let s = host.strip().borrow();
        (s.asked, s.sound.as_ref().map_or(0, |o| o.id))
    };
    arrived(host, ask, id, Answer::Read(Ok((Arc::new(audio), None))));
    for id in tools {
        run_action(host, id);
    }
    if let Some((a, b)) = part
        && let Some(o) = host.strip().borrow_mut().sound.as_mut()
    {
        let seconds = o.made.as_ref().map_or(0.0, |m| m.audio.seconds());
        o.pick = Some((a * seconds, b * seconds));
    }
    show(host);
}

/// For the screenshots: whether the strip is open, ready, and how many edits it has.
pub fn state_for_screens(strip: &Strip) -> (bool, bool, usize) {
    let ready = strip.sound.as_ref().is_some_and(|o| o.made.is_some());
    (strip.open, ready, strip.sound.as_ref().map_or(0, |o| o.edits.now.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 1_000;

    fn sound(frames: usize) -> AudioData {
        let mut a = AudioData { sample_rate: RATE, channels: vec![vec![0.5; frames]], ..AudioData::default() };
        a.file_seconds = a.seconds();
        a
    }

    fn opened(frames: usize, edits: Vec<Edit>) -> Opened {
        Opened {
            id: 1,
            source: Source { path: "a.wav".into(), span: None },
            name: "a".into(),
            colour: 0,
            bpm: None,
            original: Some(Arc::new(sound(frames))),
            changed: None,
            note: String::new(),
            edits: Edits { now: edits, ..Edits::default() },
            made: None,
            pick: None,
            zoom: None,
            dragging: None,
            held: None,
            pressed: false,
            sliced: None,
            slice: None,
            slice_press: None,
        }
    }

    /// A change to another length (a trim, or one undone) lets the part picked and the zoom go at once, before the
    /// edit is made again; a gain keeps them.
    #[test]
    fn a_change_of_length_lets_the_part_and_zoom_go_at_once() {
        let mut o = opened(1_000, Vec::new());
        let original = o.original.clone().unwrap();
        let picked = |o: &mut Opened| {
            o.pick = Some((0.1, 0.2));
            o.zoom = Some((0.05, 0.25));
        };
        picked(&mut o);
        let before = o.edits.now.clone();
        o.edits.change(vec![Edit::Gain { db: -3.0, span: None }]);
        forget_place_if_moved(&mut o, &original, &before);
        assert!(o.pick.is_some() && o.zoom.is_some());
        let before = o.edits.now.clone();
        o.edits.change(vec![Edit::Gain { db: -3.0, span: None }, Edit::Trim(Span::new(100, 600))]);
        forget_place_if_moved(&mut o, &original, &before);
        assert!(o.pick.is_none() && o.zoom.is_none());
        assert!((edit_seconds(&o, &original) - 0.5).abs() < 1e-9, "as the edits now make it, before it's made");
        picked(&mut o);
        let before = o.edits.now.clone();
        assert!(o.edits.undo());
        forget_place_if_moved(&mut o, &original, &before);
        assert!(o.pick.is_none() && o.zoom.is_none());
    }

    #[test]
    fn undo_and_redo_step_through_the_lists() {
        let mut e = Edits::default();
        e.change(vec![Edit::Reverse(None)]);
        e.change(vec![Edit::Reverse(None), Edit::Normalise { peak_db: 0.0 }]);
        // No change: nothing to undo for it.
        e.change(vec![Edit::Reverse(None), Edit::Normalise { peak_db: 0.0 }]);
        assert_eq!(e.undo.len(), 2);
        assert!(e.undo());
        assert_eq!(e.now, vec![Edit::Reverse(None)]);
        assert!(e.undo() && e.now.is_empty() && !e.undo());
        assert!(e.redo() && e.redo() && !e.redo());
        assert_eq!(e.now.len(), 2);
        // A new edit after an undo: what was undone is gone.
        e.undo();
        e.change(vec![Edit::Reverse(None), Edit::Gain { db: -1.5, span: None }]);
        assert!(e.redo.is_empty());
    }

    #[test]
    fn a_corner_fade_is_changed_in_place_and_taken_away_at_none() {
        let o = opened(1_000, Vec::new());
        let original = sound(1_000);
        let next = with_corner(&o, &original, true, 0.1);
        assert_eq!(next, vec![Edit::Fade { span: Span::new(0, 100), way: Way::In, curve: CORNER }]);
        let o = opened(1_000, vec![next[0], Edit::Gain { db: -3.0, span: None }]);
        // Dragged longer: the same edit, longer; the gain after it stays.
        let next = with_corner(&o, &original, true, 0.25);
        assert_eq!(next[0], Edit::Fade { span: Span::new(0, 250), way: Way::In, curve: CORNER });
        assert_eq!(next.len(), 2);
        // A fade out never runs into the fade in.
        let o = opened(1_000, next);
        let next = with_corner(&o, &original, false, 5.0);
        assert_eq!(next[2], Edit::Fade { span: Span::new(250, 1_000), way: Way::Out, curve: CORNER });
        // Dragged back to nothing: gone.
        let o = opened(1_000, next);
        let next = with_corner(&o, &original, true, 0.0);
        assert!(!next.iter().any(|e| matches!(e, Edit::Fade { way: Way::In, .. })));
        assert_eq!(next.len(), 2);
    }

    #[test]
    fn corner_fades_are_the_ones_made_since_the_last_move() {
        let fade_in = Edit::Fade { span: Span::new(0, 10), way: Way::In, curve: CORNER };
        let fade_out = Edit::Fade { span: Span::new(900, 1_000), way: Way::Out, curve: CORNER };
        assert_eq!(corners(1_000, &[fade_in, fade_out]), (Some((0, 10)), Some((1, 100))));
        // A trim bakes them in.
        assert_eq!(corners(1_000, &[fade_in, Edit::Trim(Span::new(5, 500))]), (None, None));
        // A fade across a part in the middle isn't a corner's.
        let across = Edit::Fade { span: Span::new(100, 200), way: Way::In, curve: ACROSS };
        assert_eq!(corners(1_000, &[across]), (None, None));
    }

    #[test]
    fn the_ruler_counts_bars_and_beats_or_seconds() {
        // 120 BPM: a beat is half a second.
        let marks = ruler(0.0, 2.0, Some(120.0));
        let texts: Vec<&str> = marks.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["1", "1.2", "1.3", "1.4", "2"]);
        assert!(marks[0].bar && !marks[1].bar);
        assert!((marks[4].at - 1.0).abs() < 1e-6);
        // Long: every so many bars, never too many.
        assert!(ruler(0.0, 120.0, Some(120.0)).len() <= 18);
        let marks = ruler(0.25, 1.0, None);
        assert!(marks.len() <= 18 && marks.first().is_some_and(|m| m.at >= 0.0));
        assert_eq!(
            ruler(0.0, 3.0, None).iter().map(|m| m.text.as_str()).collect::<Vec<_>>()[..3],
            ["0.0", "0.2", "0.4"]
        );
    }

    #[test]
    fn what_is_picked_reads_in_beats_with_the_tempo_known() {
        assert_eq!(picked_text(0.47, None), "0.47 s picked");
        assert_eq!(picked_text(0.469, Some(128.0)), "0.47 s picked · 1 beat");
        assert_eq!(picked_text(7.5, Some(128.0)), "7.50 s picked · 4 bars");
        assert_eq!(picked_text(0.7, Some(128.0)), "0.70 s picked · 1.5 beats");
        assert_eq!(gain_text(-1.5), "Gain \u{2212}1.5 dB");
        assert_eq!(gain_text(2.0), "Gain +2.0 dB");
        assert_eq!(
            chip_text(&Edit::Fade { span: Span::new(0, 6), way: Way::In, curve: CORNER }, 1_000.0),
            "Fade in 6 ms"
        );
    }

    #[test]
    fn a_fades_curve_runs_from_silence_to_full() {
        let curve = fade_curve(0.0, 0.5, true);
        assert!(curve.starts_with("M0.0 100.0") && curve.ends_with("500.0 0.0"), "{curve}");
        let curve = fade_curve(0.5, 1.0, false);
        assert!(curve.starts_with("M500.0 0.0") && curve.ends_with("1000.0 100.0"), "{curve}");
        assert!(fade_curve(0.3, 0.3, true).is_empty());
    }

    #[test]
    fn the_outline_is_the_loudest_of_each_step_at_full_scale() {
        let mut a = sound(4_000);
        a.channels[0][1_500] = -0.9;
        let made = make(&a, Vec::new());
        assert_eq!(made.steps.len(), 4_000);
        assert!((made.steps[1_500] - 0.9).abs() < 1e-6);
        let made = make(&a, vec![Edit::Gain { db: -6.0, span: None }]);
        assert!(made.steps[0] < 0.26, "a gain shows: {}", made.steps[0]);
        let path = wave_path(&[1.0, 0.0]);
        assert_eq!(path, "M0.5 0.0V100.0M1.5 49.5V50.5");
    }
}
