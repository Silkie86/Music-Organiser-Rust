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
//! Its screen is worked out here too ([`screen`]; the look of Solomon's "Build it like this" of 27 Nov 2026, pictures
//! 12 to 14): the waveform as one picture, which takes a fade's shape while its handle is still being dragged, the
//! level as it plays, what would clip, and how a save went, so the strip answers the hand that's on it.
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

pub mod screen;
pub mod slices;

use screen::{Column, Outline, Room, Wave};

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

/// The level meter's foot, in dB under full scale (its top is full scale): a level that low lights nothing.
const METER_FOOT_DB: f32 = -42.0;

/// How far up the meter (0 to 1) its lights turn amber: the last 6 dB under full scale.
pub const METER_AMBER: f32 = 1.0 - 6.0 / 42.0;

/// How fast a level falls back on the meter, in meters a second: 40 dB a second.
const METER_FALL: f32 = 40.0 / 42.0;

/// How much of the sound a tick of the meter looks at when it can't tell how far the play has come since the last one
/// (the first tick, a play moved elsewhere), and the most it takes as one tick's (seconds).
const METER_TICK: f64 = 0.04;
const METER_MOST: f64 = 0.25;

/// How far over full scale a peak may be before it counts as over (the rounding of a normalise).
const OVER: f32 = 1.0 + 1e-4;

/// A sound no louder than this is silence: the meter has no number for it.
const SILENCE: f32 = 1e-5;

/// How near a hit an edge of the part picked is on it (seconds): a snap puts it right there.
const ON_HIT: f64 = 1e-4;

/// What the Save button says until a copy is asked for.
const SAVE_COPY: &str = "Save copy";

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
    /// into what plays, to seconds into `audio`). `held`: it's the original, heard for as long as its key is held,
    /// which plays on past where the strip's play was to stop. Anything else is the edit (made again, or back after
    /// the original) and stops there still: at the `till` of [`Host::play`], which is in the edit's own seconds (an
    /// edit of another length is never swapped in: the play stops instead). Whether it did: not once the strip's
    /// play has stopped, or another sound has started.
    fn swap(&self, audio: Arc<AudioData>, place: &dyn Fn(f64) -> f64, held: bool) -> bool;

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
    /// how it went with [`saved`] (its `ask`, and the copy or why there's none), back on the window's thread. Whether
    /// it was taken: one that is always gets its [`saved`], or Save would wait for good.
    fn save(&self, edit: Saving) -> bool;

    /// Shows the saved copy `copy` in its folder.
    fn reveal(&self, copy: PathBuf);

    /// Sends `slices`: each a file of its own in a folder for this slicing in Edited sounds
    /// ([`edit_copy::slices_to_keep`]), then put on a Drum Machine in Bitwig, or their folder opened; away from the
    /// window's thread, saying how it went. Whether they're on their way: Send then counts along as the host says how
    /// many have their file ([`slices::made`]), and waits until it calls [`slices::sent`] (once they're there or
    /// couldn't be), both back on the window's thread.
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
/// `changed`), its edits, and the sound's name for the words about it. `ask` is this save's number, for [`saved`].
pub struct Saving {
    pub ask: u64,
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
    /// Its outline, fine enough to draw any stretch of it at once ([`screen`]).
    outline: Arc<Outline>,
    /// Where its hits are (seconds), those that rise by [`USUAL_RISE_DB`] or more.
    hits: Arc<[f64]>,
    /// Every hit, with how much it rises (dB): where the Slices' cuts go, as big as their sensitivity asks.
    all_hits: Arc<[(f64, f32)]>,
}

/// A sound as it was read to edit: its samples, when its file was last changed, and its outline.
pub struct Read {
    audio: Arc<AudioData>,
    changed: Option<SystemTime>,
    outline: Arc<Outline>,
}

impl Read {
    /// `audio`, as read from a file last changed `changed`, with its outline worked out. Never on the window's thread.
    fn of(audio: AudioData, changed: Option<SystemTime>) -> Read {
        let outline = Arc::new(Outline::of(&audio));
        Read { audio: Arc::new(audio), changed, outline }
    }
}

/// The strip's play of a sound, as its ticks follow it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Played {
    /// Where it started, in seconds into the edit: what has played since is lit.
    from: f64,
    /// Where it has got to, in seconds into the edit: where it started, until its first tick.
    place: f64,
    /// Where the player was at the last tick (seconds into what plays): none before the first, or once what plays is
    /// another sound (the original in the edit's place).
    last: Option<f64>,
    /// How far up the meter each side is lit (0 to 1).
    level: [f32; 2],
}

impl Played {
    /// A play that starts `from` seconds into the edit.
    fn starting(from: f64) -> Played {
        Played { from, place: from, last: None, level: [0.0; 2] }
    }
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
    /// The original's outline, read with it.
    outline: Option<Arc<Outline>>,
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
    /// The slice picked (its number less one), heard as it's clicked; and how many times one has been picked.
    slice: Option<usize>,
    slice_picks: i32,
    /// What a press on the Slices' waveform is doing, while the button is down.
    slice_press: Option<slices::Press>,
    /// The strip's play of it, while that plays.
    played: Option<Played>,
}

impl Opened {
    /// The sound `id` (what's read of it, its name, its type's colour and its tempo), as it is before it's read:
    /// with no edits, nothing picked and nothing playing.
    fn new(id: i64, source: Source, name: String, colour: i32, bpm: Option<f64>) -> Opened {
        Opened {
            id,
            source,
            name,
            colour,
            bpm,
            original: None,
            outline: None,
            changed: None,
            note: String::new(),
            edits: Edits::default(),
            made: None,
            pick: None,
            zoom: None,
            dragging: None,
            held: None,
            pressed: false,
            sliced: None,
            slice: None,
            slice_picks: 0,
            slice_press: None,
            played: None,
        }
    }
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
    /// The slices on their way to Bitwig or a folder: Send waits until they're there.
    sending: Option<slices::Sending>,
    /// The room its screen has, as its host laid it out (none until it has).
    room: Option<Room>,
    /// The waveform's columns as last worked out, kept while they're still the ones to show.
    columns: Option<Columns>,
    /// The screen's waveform as last shown: handed out again while nothing in it has changed, so a host draws its
    /// picture only when it's given another.
    wave: Option<Arc<Wave>>,
    /// The copy last asked for (being saved, saved, or why it couldn't be), and how many have been.
    copy: Option<Copy>,
    saves: u64,
}

/// The columns of the strip's waveform, and what they were worked out for: the edit made (by its outline, which is
/// another for every make), whether it's the original under it that's shown, the stretch on show, and how many columns
/// (a column each `per` seconds, the first starting at `from`).
struct Columns {
    of: Arc<Outline>,
    original: bool,
    from: f64,
    per: f64,
    columns: Arc<[Column]>,
}

/// A copy of a sound's edit, asked for with Save: whose (the sound, what was read of it, and the edits), and how far
/// it has got.
struct Copy {
    ask: u64,
    id: i64,
    source: Source,
    edits: Vec<Edit>,
    state: CopyState,
}

enum CopyState {
    Saving,
    Saved(PathBuf),
    Failed(String),
}

/// A read or a make, for the host's thread ([`Host::work`]).
pub enum Work {
    Read(Source),
    Make { original: Arc<AudioData>, outline: Arc<Outline>, edits: Vec<Edit> },
}

/// What came back from the host's thread ([`answer`]).
pub enum Answer {
    Read(Result<Read, Unread>),
    Made(Result<Made, String>),
}

/// A mark on the strip's ruler: where (0 to 1 across the waveform), what it says, and whether it's a bar's (brighter).
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub at: f32,
    pub text: String,
    pub bar: bool,
}

/// One edit made so far, as a chip under the waveform; the last one (the one Undo takes back) is lit. None with the
/// Slices on: the strip's foot then says which slice is picked.
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    pub text: String,
    pub last: bool,
}

/// The level meter beside the strip's screen: how far up it each side is lit (left, right; 0 to 1 of it, its top is
/// full scale), the highest each side gets (a light of its own, held above), that as a number ("−3.0", "+6.0": dB
/// against full scale; empty for silence), and whether it goes over full scale.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Levels {
    pub level: [f32; 2],
    pub hold: [f32; 2],
    pub peak_text: String,
    pub over: bool,
}

/// Everything the strip shows, for its host to copy onto the screen. All of it is given every time: what a sound still
/// being read can't say yet is empty, so nothing of the sound before it lingers.
#[derive(Clone, Debug, PartialEq)]
pub struct Showing {
    /// Open: the strip is on show.
    pub open: bool,
    /// Where copies go, for the tips ("the export folder's Edited sounds").
    pub folder_text: String,
    /// The sound it edits: its name, its type's colour, and what it is ("3.75 s · 128 BPM · 2 bars"; with the Slices
    /// on, how many there are and the one picked).
    pub name: String,
    pub colour: i32,
    pub facts: String,
    /// Its tempo, for the length of a part as it's picked ([`part_text`]): 0 when that isn't known.
    pub bpm: f32,
    /// Why there's nothing to edit (it couldn't be read, it's too long): shown where the waveform goes. Empty for none.
    pub note: String,
    /// Read and ready to edit.
    pub ready: bool,
    /// The screen's waveform over the part on show (the whole of it unless zoomed in), for [`screen::scene`]: the same
    /// one again while nothing in it has changed, and none until the host has said what room its screen has
    /// ([`laid_out`]). How long the part on show lasts and where it starts (seconds).
    pub wave: Option<Arc<Wave>>,
    pub view_seconds: f32,
    pub view_from: f32,
    pub zoomed: bool,
    /// The part picked on it (0 to 1 across the part on show; none unless `pick_to` is past `pick_from`), whether
    /// each of its edges is on a hit (that hit's mark is lit), and whether there's a part to zoom in on (with the
    /// Slices on, the slice picked).
    pub has_part: bool,
    pub pick_from: f32,
    pub pick_to: f32,
    pub pick_on_hits: (bool, bool),
    /// The fades' handles, 0 to 1 across (where the fade in ends and the fade out starts), each shown only while its
    /// end of the sound is in sight; their curves, paths 1000 across and 100 high (none for a fade that isn't there);
    /// what each takes away, the same way, a shape to shade; and, while one's handle is dragged, how long it is now
    /// ("Fade out 620 ms").
    pub fade_in: f32,
    pub fade_out: f32,
    pub fade_in_shown: bool,
    pub fade_out_shown: bool,
    pub fade_in_curve: String,
    pub fade_out_curve: String,
    pub fade_in_shade: String,
    pub fade_out_shade: String,
    pub fade_text: String,
    pub ruler: Vec<Mark>,
    pub chips: Vec<Chip>,
    pub can_undo: bool,
    pub can_redo: bool,
    /// Any edits made now (undo may have taken them all back): something to save.
    pub has_edits: bool,
    /// The original is what's heard (O, or A/B, is held).
    pub original: bool,
    /// The strip's play, while it plays: where it started and where it has got to, as [`Playing`] has them (the
    /// host's ticker moves them on from here).
    pub played: Option<(f32, f32)>,
    /// The gain of the part picked (or the whole sound), as the stepper shows it ("−1.5 dB"), and whether it's been
    /// changed from none.
    pub gain_text: String,
    pub gain_changed: bool,
    /// The edit goes over full scale somewhere: its tops are red, and Normalise would bring it back.
    pub too_loud: bool,
    /// The level meter: the edit's own loudest moment while nothing plays, the level as it plays while it does.
    pub levels: Levels,
    /// What the drag handle carries ("Drag the edit", "Drag the part"; with the Slices on, "Drag slice 5"), and
    /// whether there's anything for it to carry: with the Slices on, only once a slice is picked.
    pub drag_text: String,
    pub can_drag: bool,
    /// The Save button: what it says ("Save copy", "Saving…", "Saved"), whether it can be pressed (saved, it still
    /// can: a copy that has gone since is made again), and whether the copy of these very edits is saved. Beside it,
    /// how the last save went, in two lines: "Saved in Edited sounds" over the copy's name, or (`save_failed`)
    /// "Couldn't save the copy" over why. Empty for none.
    pub save_text: String,
    pub can_save: bool,
    pub saved: bool,
    pub save_note: String,
    pub save_detail: String,
    pub save_failed: bool,
    /// The part of the sound's own file the strip is on, from and to as shares of the file (0 its start, 1 its end),
    /// for a host that shows the file's own waveform beside the strip (the plugin's list): where the part picked came
    /// from, through the edits (the slice picked, with the Slices on); with none picked, where the edit as a whole
    /// did, when a trim or a loop keeps less than all of the file. None when it's the whole file; and none for a long
    /// sound only a part of which was read, as how long its file is isn't known here.
    pub file_part: Option<(f32, f32)>,
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
            facts: String::new(),
            bpm: 0.0,
            note: String::new(),
            ready: false,
            wave: None,
            view_seconds: 0.0,
            view_from: 0.0,
            zoomed: false,
            has_part: false,
            pick_from: 0.0,
            pick_to: 0.0,
            pick_on_hits: (false, false),
            fade_in: 0.0,
            fade_out: 0.0,
            fade_in_shown: false,
            fade_out_shown: false,
            fade_in_curve: String::new(),
            fade_out_curve: String::new(),
            fade_in_shade: String::new(),
            fade_out_shade: String::new(),
            fade_text: String::new(),
            ruler: Vec::new(),
            chips: Vec::new(),
            can_undo: false,
            can_redo: false,
            has_edits: false,
            original: false,
            played: None,
            gain_text: db_text(0.0),
            gain_changed: false,
            too_loud: false,
            levels: Levels::default(),
            drag_text: "Drag the edit".to_owned(),
            can_drag: true,
            save_text: SAVE_COPY.to_owned(),
            can_save: false,
            saved: false,
            save_note: String::new(),
            save_detail: String::new(),
            save_failed: false,
            file_part: None,
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
    let was_open = shut(&mut host.strip().borrow_mut());
    if !was_open {
        return;
    }
    stop(host);
    show(host);
}

/// [`close`]'s step on the strip itself, with nothing told or shown: whether it was open.
fn shut(s: &mut Strip) -> bool {
    let was_open = s.open;
    s.open = false;
    stash(s);
    s.asked += 1;
    // A save that failed is past once the strip is closed.
    if s.copy.as_ref().is_some_and(|c| matches!(c.state, CopyState::Failed(_))) {
        s.copy = None;
    }
    was_open
}

/// Puts the strip away when its host goes and another will take it up (the plugin's window closes, and the strip waits
/// for the next one): closed, each sound's edits kept, and nothing left that was the old host's. That's the room its
/// screen had and the waveform shown there; and a save or a send still on its way, whose answer has nobody to come
/// back to (it's finished all the same: the copy is made, and Save finds it there the next time).
pub fn put_away(strip: &mut Strip) {
    shut(strip);
    (strip.room, strip.columns, strip.wave) = (None, None, None);
    if strip.copy.as_ref().is_some_and(|c| matches!(c.state, CopyState::Saving)) {
        strip.copy = None;
    }
    strip.sending = None;
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

/// The sound the strip is open on, by its id: none while it's closed. A host whose list goes on by itself (the
/// plugin's) goes by this to see when another sound is picked there ([`sound_picked`]).
pub fn open_on(strip: &Strip) -> Option<i64> {
    strip.sound.as_ref().filter(|_| strip.open).map(|o| o.id)
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
        let note = if too_long { Unread::TooLong.to_string() } else { String::new() };
        s.sound = Some(Opened { note, edits, sliced, ..Opened::new(id, source.clone(), name, colour, bpm) });
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
    // Slices of it still on their way are worth keeping its slicing for: where they went is kept with it.
    let on_way = s.sending.as_ref().is_some_and(|sending| sending.is_of(o.id, &o.source));
    let sliced = o.sliced.filter(|sliced| on_way || sliced.worth_keeping());
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
            Answer::Read(edit_copy::read_to_edit(&source).map(|read| Read::of(read.audio, read.changed)))
        }
        Work::Make { original, outline, edits } => Answer::Made(
            mo_catch::catch_crash(AssertUnwindSafe(|| make(&original, &outline, edits)))
                .map_err(|_| "Couldn't make the edit: the log says why.".to_owned()),
        ),
    }
}

/// The edit of `original` (whose outline is `outline`) by `edits`, with its outline and its hits. With no edits it's
/// the original itself: nothing is copied.
fn make(original: &Arc<AudioData>, outline: &Arc<Outline>, edits: Vec<Edit>) -> Made {
    let (audio, outline) = if edits.is_empty() {
        (original.clone(), outline.clone())
    } else {
        let audio = edit::apply(original, &edits);
        let outline = Arc::new(Outline::of(&audio));
        (Arc::new(audio), outline)
    };
    let all_hits: Arc<[(f64, f32)]> = edit_copy::hits_in(&audio).into();
    let hits: Arc<[f64]> = all_hits.iter().filter(|(_, rise)| *rise >= USUAL_RISE_DB).map(|(at, _)| *at).collect();
    Made { edits, audio, outline, hits, all_hits }
}

/// For the tests: the edit of `audio` by `edits`.
#[cfg(test)]
fn make_of(audio: &AudioData, edits: Vec<Edit>) -> Made {
    let read = Read::of(audio.clone(), None);
    make(&read.audio, &read.outline, edits)
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
        Answer::Read(Ok(Read { audio, changed, outline })) => {
            // A part picked on the card of a sound read whole is where the strip starts, while it has no edits.
            let card_part = host.card_part();
            if let Some(o) = host.strip().borrow_mut().sound.as_mut() {
                if o.source.span.is_none() && o.edits.now.is_empty() {
                    o.pick = card_part.filter(|(from, to)| *to <= audio.seconds() + 1e-6 && from < to);
                }
                o.original = Some(audio);
                o.outline = Some(outline);
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
            let (original, outline) = (o.original.clone()?, o.outline.clone()?);
            if o.made.as_ref().is_some_and(|m| m.edits == o.edits.now) {
                return None;
            }
            Some((o.id, Work::Make { original, outline, edits: o.edits.now.clone() }))
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
    let _ = host.swap(audio, &|at| at, false);
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
            // What plays from here on is lit.
            o.played = Some(Played::starting(from));
            Some(audio)
        })
    };
    if let Some(audio) = audio {
        host.play(audio, from, till);
    }
}

/// O held (`on`) or let go, or the A/B button: the original plays from the moment the edit has got to, and the edit
/// comes back from the same moment as it's let go. With nothing playing, the original plays while it's held (from the
/// start of the part picked, or of the slice picked with the Slices on, if there is one: where that stretch starts in
/// the original, which is where its end came from if the edit reversed it). Whether it did anything.
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
            // What plays is another sound from here: the meter takes it up afresh.
            if let Some(played) = o.played.as_mut() {
                played.last = None;
            }
        }
    };
    if on {
        // Held already (the key repeats).
        if held.is_some() {
            return true;
        }
        if plays && host.swap(original.clone(), &before, true) {
            set_held(Some(false));
        } else {
            // The stretch of the original the part (all of the edit, with none) came from, from its earlier end:
            // through a reverse that's where the part's last moment came from.
            let (from, to) = pick.unwrap_or((0.0, edited.seconds()));
            let start = before(from).min(before((to - 1.0 / rate).max(from)));
            if let Some(o) = host.strip().borrow_mut().sound.as_mut() {
                o.played = Some(Played::starting(after(start)));
            }
            host.play(original, start, None);
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
                    let _ = host.swap(edited, &after, false);
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

/// Where the strip's play has got to, for its host to show: the play head and where the play started (both across
/// what the strip's waveform shows, 0 to 1 while they're in sight and outside that while they aren't; what's between
/// them is lit), and how far up the meter each side is lit. And where the moment heard is in the sound's own file (0
/// its start, 1 its end), for a host that shows the file's own waveform too ([`Showing::file_part`]): where it came
/// from, through the edits, and where the moment the play started at came from (what has played is between the two:
/// through a reverse the play runs from right to left there); none for a long sound only a part of which was read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Playing {
    pub at: f32,
    pub from: f32,
    pub level: [f32; 2],
    pub in_file: Option<f32>,
    pub from_in_file: Option<f32>,
}

/// What a tick of the host's ticker comes to, while the strip's play plays ([`tick`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ticked {
    /// Nothing to show yet (the player hasn't the play, or the sound has gone): it goes on.
    Waiting,
    /// It goes on, and is here.
    At(Playing),
    /// A play of the part picked has come to the part's end: it stops.
    Ended,
}

/// The host's ticker, while the strip's play plays: `at` is where the player is (seconds into what it plays; none
/// until it has the play), and `till` where the play stops (seconds into the edit: the end of the part it's of). The
/// strip's waveform shows where the edit has got to and its meter how loud that is; a play of the part picked stops
/// at its end.
pub fn tick(strip: &mut Strip, at: Option<f64>, till: Option<f64>) -> Ticked {
    let Some(at) = at else { return Ticked::Waiting };
    let Some(o) = strip.sound.as_mut() else { return Ticked::Waiting };
    let (from, to) = view(o);
    let (Some(made), Some(original)) = (o.made.as_ref(), o.original.as_ref()) else { return Ticked::Waiting };
    let held = o.held.is_some();
    let rate = f64::from(original.sample_rate.max(1));
    let frame = (at * rate).round() as usize;
    // What's heard, and where that is in the edit: hearing the original, where the moment it has got to went.
    let (heard, place) = if held {
        (original, edit::place_after(original.len(), &made.edits, frame) as f64 / rate)
    } else {
        (&made.audio, at)
    };
    if !held && till.is_some_and(|till| place >= till) {
        return Ticked::Ended;
    }
    // And in the sound's own file: the original is that file, read whole; the edit's moment came from somewhere in it.
    let whole = original.len();
    let before = |frame: usize| edit::place_before(whole, &made.edits, frame);
    let of_file = |frame: usize| (frame as f64 / whole.max(1) as f64).clamp(0.0, 1.0) as f32;
    let read_whole = o.source.span.is_none();
    let in_file = read_whole.then(|| of_file(if held { frame } else { before(frame) }));
    // A play whose start wasn't seen is lit from here on.
    let played = o.played.get_or_insert(Played::starting(place));
    // Where it started is kept as a moment of the edit, whichever of the two plays.
    let from_in_file = read_whole.then(|| of_file(before((played.from * rate).round() as usize)));
    // The meter: as loud as what has played since the last tick gets, or falling back from where it was.
    let since = played.last.filter(|last| *last <= at).map_or(METER_TICK, |last| (at - last).min(METER_MOST));
    let loud = loudest(heard, at - since, at);
    let fall = METER_FALL * since as f32;
    for (level, loud) in played.level.iter_mut().zip(loud) {
        *level = meter_place(loud).max(*level - fall);
    }
    played.last = Some(at);
    played.place = place;
    let across = |t: f64| across(t, (from, to));
    Ticked::At(Playing { at: across(place), from: across(played.from), level: played.level, in_file, from_in_file })
}

/// Where the moment `t` is across a stretch on show (from and to, all in seconds): 0 to 1 inside it, and no further
/// outside it than its own width again, which is out of sight either way.
fn across(t: f64, (from, to): (f64, f64)) -> f32 {
    ((t - from) / (to - from).max(1e-9)).clamp(-1.0, 2.0) as f32
}

/// How far up the meter (0 to 1) a level is: 1 is full scale, where the meter tops out, and [`METER_FOOT_DB`] its
/// foot.
fn meter_place(level: f32) -> f32 {
    if level > 0.0 { ((20.0 * level.log10() - METER_FOOT_DB) / -METER_FOOT_DB).clamp(0.0, 1.0) } else { 0.0 }
}

/// The loudest sample on the left and on the right of `audio` from `from` to `to` seconds (the one channel's twice,
/// for a sound with one): nothing, for a stretch with no samples in it.
fn loudest(audio: &AudioData, from: f64, to: f64) -> [f32; 2] {
    let rate = f64::from(audio.sample_rate.max(1));
    let side = |ch: &Vec<f32>| {
        let first = ((from * rate).floor().max(0.0) as usize).min(ch.len());
        let past = ((to * rate).ceil().max(0.0) as usize).clamp(first, ch.len());
        ch[first..past].iter().filter(|s| s.is_finite()).fold(0.0f32, |most, s| most.max(s.abs()))
    };
    let left = audio.channels.first().map_or(0.0, side);
    [left, audio.channels.get(1).map_or(left, side)]
}

/// Saves the edit as a copy in Edited sounds: the host says how it went ([`saved`]), and the strip shows it while the
/// edits are still the ones saved. Whether there was an edit to save, and the host took it.
fn save(host: &dyn Host) -> bool {
    let edit = {
        let mut s = host.strip().borrow_mut();
        let ask = s.saves + 1;
        let edit = s.sound.as_ref().and_then(|o| {
            let original = o.original.clone().filter(|_| !o.edits.now.is_empty())?;
            let (source, changed, edits, name) = (o.source.clone(), o.changed, o.edits.now.clone(), o.name.clone());
            Some((o.id, Saving { ask, source, original, changed, edits, name }))
        });
        let Some((id, edit)) = edit else { return false };
        // The copy of these very edits is on its way already.
        let same = |c: &Copy| c.id == id && c.source == edit.source && c.edits == edit.edits;
        if s.copy.as_ref().is_some_and(|c| matches!(c.state, CopyState::Saving) && same(c)) {
            return true;
        }
        s.saves = ask;
        let (source, edits) = (edit.source.clone(), edit.edits.clone());
        s.copy = Some(Copy { ask, id, source, edits, state: CopyState::Saving });
        edit
    };
    let ask = edit.ask;
    let taken = host.save(edit);
    if !taken {
        // Not taken: nothing is being saved.
        let mut s = host.strip().borrow_mut();
        if s.copy.as_ref().is_some_and(|c| c.ask == ask) {
            s.copy = None;
        }
    }
    show(host);
    taken
}

/// A save came back ([`Host::save`]): the copy it made, or why there's none. Only the last one asked for counts.
pub fn saved(host: &dyn Host, ask: u64, how: Result<PathBuf, String>) {
    {
        let mut s = host.strip().borrow_mut();
        let Some(copy) = s.copy.as_mut().filter(|c| c.ask == ask) else { return };
        copy.state = match how {
            Ok(path) => CopyState::Saved(path),
            Err(why) => CopyState::Failed(why),
        };
    }
    show(host);
}

/// The copy of `o`'s edits as they are now, if that's the one last asked for.
fn copy_of<'a>(o: &Opened, copy: Option<&'a Copy>) -> Option<&'a Copy> {
    copy.filter(|c| c.id == o.id && c.source == o.source && c.edits == o.edits.now)
}

/// The words about the saved copy clicked: the host shows it in its folder.
pub fn show_saved(host: &dyn Host) {
    let path = {
        let s = host.strip().borrow();
        s.sound.as_ref().and_then(|o| match &copy_of(o, s.copy.as_ref())?.state {
            CopyState::Saved(path) => Some(path.clone()),
            _ => None,
        })
    };
    if let Some(path) = path {
        host.reveal(path);
    }
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

/// The drag handle picked up: it carries what its words say, the part picked or with none the edit (the sound itself,
/// with no edits either). With the Slices on, the slice picked, and nothing until one is.
pub fn drag_handle(host: &dyn Host) {
    let part = {
        let s = host.strip().borrow();
        let Some(o) = s.sound.as_ref() else { return };
        if slices::on(o) {
            let Some(slice) = slices::picked(o) else { return };
            Some(slice)
        } else {
            o.pick
        }
    };
    carry(host, part);
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
    let plays = host.plays();
    let showing = showing(&mut host.strip().borrow_mut(), plays);
    host.show(showing);
}

/// The host laid the strip's screen out, or its size or how many pixels there are to a point changed: the room its
/// waveform has from now on. The strip shows itself again when that's another room.
pub fn laid_out(host: &dyn Host, room: Room) {
    {
        let mut s = host.strip().borrow_mut();
        if s.room == Some(room) {
            return;
        }
        s.room = Some(room);
    }
    show(host);
}

/// The strip as it is now. `plays`: its play is what plays ([`Host::plays`]).
fn showing(s: &mut Strip, plays: bool) -> Showing {
    let mut show = Showing { open: s.open, ..Showing::default() };
    let Strip { sound, columns, wave, room, copy, sending, .. } = s;
    let Some(o) = sound.as_mut() else {
        (*columns, *wave) = (None, None);
        return show;
    };
    // A play that has stopped lights nothing any more.
    if !plays {
        o.played = None;
    }
    let o = &*o;
    // With the Slices on, undo and redo are the cuts', and there are no chips: the foot says which slice is picked.
    let slicing = slices::on(o);
    (show.can_undo, show.can_redo) =
        if slicing { slices::steps(o) } else { (!o.edits.undo.is_empty(), !o.edits.redo.is_empty()) };
    show.name.clone_from(&o.name);
    show.colour = o.colour;
    show.bpm = o.bpm.map_or(0.0, |bpm| bpm as f32);
    show.note.clone_from(&o.note);
    show.has_edits = !o.edits.now.is_empty();
    let rate = f64::from(o.original.as_ref().map_or(1, |a| a.sample_rate.max(1)));
    show.chips = if slicing { Vec::new() } else { chips(&o.edits.now, rate) };
    show.slices = slices::showing(o, sending.as_ref());
    show.original = o.held.is_some();
    let gain = gain_now(o);
    show.gain_text = db_text(gain);
    show.gain_changed = gain != 0.0;
    show.drag_text = if slicing {
        slices::drag_text(o)
    } else if o.pick.is_some() {
        "Drag the part".to_owned()
    } else if o.edits.now.is_empty() {
        "Drag the sound".to_owned()
    } else {
        "Drag the edit".to_owned()
    };
    show.can_drag = !slicing || slices::picked(o).is_some();
    // The Save button, and how the save of these very edits went.
    show.can_save = o.original.is_some() && show.has_edits;
    match copy_of(o, copy.as_ref()).map(|c| &c.state) {
        Some(CopyState::Saving) => {
            show.save_text = "Saving\u{2026}".to_owned();
            show.can_save = false;
        }
        Some(CopyState::Saved(path)) => {
            show.save_text = "Saved".to_owned();
            show.saved = true;
            show.save_note = format!("Saved in {}", edit_copy::FOLDER);
            show.save_detail = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        }
        Some(CopyState::Failed(why)) => {
            show.save_note = "Couldn't save the copy".to_owned();
            show.save_detail.clone_from(why);
            show.save_failed = true;
        }
        None => {}
    }
    let (Some(original), Some(made)) = (o.original.as_ref(), o.made.as_ref()) else {
        (*columns, *wave) = (None, None);
        return show;
    };
    show.ready = o.note.is_empty();
    show.file_part = file_part(o, original, made);
    let held = o.held.is_some();
    let seconds = made.audio.seconds();
    let (from, to) = view(o);
    let long = (to - from).max(1e-9);
    let across = |t: f64| ((t - from) / long) as f32;
    show.played =
        o.played.map(|played| (self::across(played.from, (from, to)), self::across(played.place, (from, to))));
    show.view_seconds = long as f32;
    show.view_from = from as f32;
    show.zoomed = o.zoom.is_some();
    // With the Slices on: how many there are, and the one picked (which the zoom goes to).
    show.facts = if slicing { slices::heading(o, seconds) } else { facts(seconds, o.bpm) };
    let on_hit = |t: f64| made.hits.iter().any(|hit| (hit - t).abs() < ON_HIT);
    match o.pick {
        _ if slicing => show.has_part = slices::picked(o).is_some(),
        Some((a, b)) => {
            show.has_part = true;
            (show.pick_from, show.pick_to) = (across(a), across(b));
            show.pick_on_hits = (on_hit(a), on_hit(b));
        }
        None => {}
    }
    // The corner fades, how long each is: as the edit on show has them; as the edits have them now, when they're all
    // that an edit still being made changes; or where a handle is being dragged. The waveform takes their shape at
    // once, with no wait for the edit to be made.
    let was = corner_seconds(original.len(), &made.edits, rate);
    let only_fades = made.edits != o.edits.now
        && length_after(original, &o.edits.now) == made.audio.len()
        && without_corners(original.len(), &made.edits) == without_corners(original.len(), &o.edits.now);
    let mut now = if only_fades { corner_seconds(original.len(), &o.edits.now, rate) } else { was };
    match o.dragging {
        Some((true, at)) => now.0 = at.min(seconds - now.1).max(0.0),
        Some((false, at)) => now.1 = (seconds - at.max(now.0)).max(0.0),
        None => {}
    }
    let (fade_in, fade_out) = (now.0, seconds - now.1);
    show.fade_in = across(fade_in);
    show.fade_out = across(fade_out);
    // A handle that's being dragged stays, wherever it has got to: taken away, its drag would never end.
    show.fade_in_shown = from <= 1e-9 || fade_in > from || matches!(o.dragging, Some((true, _)));
    show.fade_out_shown = to >= seconds - 1e-9 || fade_out < to || matches!(o.dragging, Some((false, _)));
    (show.fade_in_curve, show.fade_in_shade) = fade_paths(across(0.0), across(fade_in), true);
    (show.fade_out_curve, show.fade_out_shade) = fade_paths(across(fade_out), across(seconds), false);
    if let Some((fade_in, _)) = o.dragging.filter(|_| !slicing) {
        // The edits as they'd be with the handle let go here: its chip counts along.
        let next = with_corner(o, original, fade_in, if fade_in { now.0 } else { now.1 });
        show.fade_text = fade_text(&next, original.len(), rate, fade_in);
        show.chips = chips(&next, rate);
    }
    // The meter: what's heard (the original, while that's held) at its loudest; and as it plays, the level it has
    // got to.
    let heard = o.outline.as_ref().filter(|_| held).unwrap_or(&made.outline);
    let hold = heard.peaks().map(meter_place);
    show.levels = Levels {
        level: o.played.map_or(hold, |played| played.level),
        hold,
        peak_text: peak_text(heard.peak()),
        over: heard.peak() > OVER,
    };
    show.too_loud = made.outline.peak() > OVER;
    // The ruler has as many marks as the screen has room for, a number beside each: all of them until the host has
    // said what room there is. Then the screen's waveform.
    let room = room.filter(Room::fits);
    let most = room.map_or(MOST_MARKS, |room| ((room.width - 2.0 * room.inset) / MARK_ROOM).clamp(1.0, MOST_MARKS));
    show.ruler = ruler(from, to, o.bpm, most);
    let Some(room) = room else {
        (*columns, *wave) = (None, None);
        return show;
    };
    let made_columns = columns_now(columns, &room, o, (original, made), (from, to));
    let (shown, before) = if held || now == was {
        (made_columns, None)
    } else {
        let per = room.per_column(to - from);
        let (shown, changed) = screen::refaded(&made_columns, (from, per, seconds), was, now);
        // While the handle is dragged, how the sound was shows behind what it's becoming.
        let before = changed.filter(|_| o.dragging.is_some()).map(|(first, past)| {
            (first, made_columns[first..past].iter().map(|c| (c.top, c.bottom)).collect::<Arc<[_]>>())
        });
        (shown.into(), before)
    };
    // The hits' marks along the foot: not with the Slices on, when the slices' numbers are there.
    let hits = if slicing {
        Vec::new()
    } else {
        made.hits.iter().filter(|hit| **hit >= from && **hit <= to).map(|hit| across(*hit)).collect()
    };
    let marks = show.ruler.iter().map(|mark| (mark.at, mark.bar)).collect();
    let next = Wave { room, columns: shown, before, original: held, marks, hits };
    // The same one again while nothing in it has changed: the host draws its picture only when it's given another.
    let next = match wave.take() {
        Some(last) if *last == next => last,
        _ => Arc::new(next),
    };
    *wave = Some(next.clone());
    show.wave = Some(next);
    show
}

/// [`Showing::file_part`] for the sound `o`, read as `original` and edited to `made`.
fn file_part(o: &Opened, original: &AudioData, made: &Made) -> Option<(f32, f32)> {
    if o.source.span.is_some() {
        return None;
    }
    let (frames, rate) = (original.len(), f64::from(original.sample_rate.max(1)));
    let part = match part_now(o) {
        Some((from, to)) => Span::new((from * rate).round() as usize, (to * rate).round() as usize),
        None => Span { start: 0, end: made.audio.len() },
    };
    let from = edit::came_from(frames, &made.edits, part)?;
    // All of the file: there's no part of it to mark.
    if from.start == 0 && from.end >= frames {
        return None;
    }
    let share = |frame: usize| (frame as f64 / frames.max(1) as f64) as f32;
    Some((share(from.start), share(from.end)))
}

/// The waveform's columns for the sound `o` (read as `original`, its edit `made`) over the stretch on show, a column
/// to each of the room's pixels: the edit's; or, while the original is held, the original's, each moment of it where
/// the edit has it. Worked out once and kept (`kept`) until the edit, the stretch or the room is another.
fn columns_now(
    kept: &mut Option<Columns>,
    room: &Room,
    o: &Opened,
    (original, made): (&Arc<AudioData>, &Made),
    (from, to): (f64, f64),
) -> Arc<[Column]> {
    let held = o.held.is_some();
    let (count, per) = (room.columns(), room.per_column(to - from));
    if let Some(c) = kept.as_ref()
        && Arc::ptr_eq(&c.of, &made.outline)
        && c.original == held
        && c.from == from
        && c.per == per
        && c.columns.len() == count
    {
        return c.columns.clone();
    }
    let columns: Arc<[Column]> = match o.outline.as_ref().filter(|_| held) {
        Some(outline) => {
            let rate = f64::from(original.sample_rate.max(1));
            let (frames, seconds) = (original.len(), made.audio.seconds());
            // Where a moment of the edit came from in the original: nowhere, outside the edit.
            let place = |at: f64| {
                if (0.0..=seconds).contains(&at) {
                    edit::place_before(frames, &made.edits, (at * rate).round() as usize) as f64 / rate
                } else {
                    f64::NAN
                }
            };
            screen::columns(original, outline, from, per, count, &place)
        }
        None => screen::columns(&made.audio, &made.outline, from, per, count, &|at| at),
    }
    .into();
    *kept = Some(Columns { of: made.outline.clone(), original: held, from, per, columns: columns.clone() });
    columns
}

/// How long the corner fades among `edits` are, in and out (seconds at `rate` frames a second; 0 for none), of a
/// sound `frames` long before them.
fn corner_seconds(frames: usize, edits: &[Edit], rate: f64) -> (f64, f64) {
    let (fade_in, fade_out) = corners(frames, edits);
    let seconds = |corner: Corner| corner.map_or(0.0, |(_, long)| long as f64 / rate);
    (seconds(fade_in), seconds(fade_out))
}

/// `edits` (of a sound `frames` long before them) without their corner fades: two lists that leave the same differ
/// only in those.
fn without_corners(frames: usize, edits: &[Edit]) -> Vec<Edit> {
    let (fade_in, fade_out) = corners(frames, edits);
    let corner = |i: usize| [fade_in, fade_out].iter().any(|c| c.is_some_and(|(at, _)| at == i));
    edits.iter().enumerate().filter(|(i, _)| !corner(*i)).map(|(_, e)| *e).collect()
}

/// A number of dB with its sign: "−3.0", "+2.0", "0.0".
fn signed(db: f32) -> String {
    if db.abs() < 0.05 {
        "0.0".to_owned()
    } else if db < 0.0 {
        format!("\u{2212}{:.1}", -db)
    } else {
        format!("+{db:.1}")
    }
}

/// A gain as the stepper says it: "−1.5 dB", "+2.0 dB", "0.0 dB".
fn db_text(db: f32) -> String {
    format!("{} dB", signed(db))
}

/// A gain as its chip says it: "Gain −1.5 dB".
fn gain_text(db: f32) -> String {
    format!("Gain {}", db_text(db))
}

/// The loudest a sound gets (1 is full scale) as the meter's foot says it, in dB against full scale: "−3.0", "+6.0";
/// nothing for silence.
fn peak_text(peak: f32) -> String {
    if peak > SILENCE { signed(20.0 * peak.log10()) } else { String::new() }
}

/// What a sound is, under its name: how long, and with its tempo known that and the beats or bars it comes to
/// ("3.75 s · 128 BPM · 2 bars").
fn facts(seconds: f64, bpm: Option<f64>) -> String {
    let mut text = sound_part::length_text(seconds);
    if let Some(bpm) = bpm.filter(|bpm| *bpm > 0.0) {
        let _ = write!(text, " \u{b7} {} BPM", (bpm * 10.0).round() / 10.0);
        beats(&mut text, seconds, bpm);
    }
    text
}

/// How long a part of the sound is, as the badge over it says it: "0.94 s", and with the tempo known (`bpm` over 0)
/// "0.94 s · 2 beats".
pub fn part_text(seconds: f64, bpm: f64) -> String {
    let mut text = sound_part::length_text(seconds);
    if bpm > 0.0 {
        beats(&mut text, seconds, bpm);
    }
    text
}

/// Adds the beats `seconds` come to at `bpm` to `text`: " · 1 beat", " · 4 bars" for whole bars from two up,
/// " · 1.5 beats"; nothing for less than a tenth of one.
fn beats(text: &mut String, seconds: f64, bpm: f64) {
    let beats = seconds * bpm / 60.0;
    let whole = beats.round();
    if whole >= 1.0 && (beats - whole).abs() < 0.08 {
        let bars = whole / 4.0;
        if whole >= 8.0 && bars.fract() == 0.0 {
            let _ = write!(text, " \u{b7} {bars} bars");
        } else {
            let _ = write!(text, " \u{b7} {whole} beat{}", if whole == 1.0 { "" } else { "s" });
        }
    } else if beats >= 0.1 {
        let _ = write!(text, " \u{b7} {beats:.1} beats");
    }
}

/// `edits` as chips: "Fade in 6 ms", "Gain −1.5 dB"…, the last one lit.
fn chips(edits: &[Edit], rate: f64) -> Vec<Chip> {
    let count = edits.len();
    edits.iter().enumerate().map(|(i, e)| Chip { text: chip_text(e, rate), last: i + 1 == count }).collect()
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

/// What a fade's handle says while it's dragged, of `edits` as they'd be with it let go there (of a sound `frames`
/// long before them): "Fade out 620 ms"; or, back in its corner, "No fade out".
fn fade_text(edits: &[Edit], frames: usize, rate: f64, fade_in: bool) -> String {
    let (fade_in_now, fade_out_now) = corners(frames, edits);
    let way = if fade_in { "in" } else { "out" };
    match if fade_in { fade_in_now } else { fade_out_now } {
        Some((_, long)) => format!("Fade {way} {}", span_length(long as f64 / rate)),
        None => format!("No fade {way}"),
    }
}

/// A corner fade over the waveform, as two paths 1000 across and 100 high: its curve, from silence at the bottom at
/// `from` to full at the top at `to` (0 to 1 across, either may be out of sight) for a fade in, or the other way for
/// a fade out; and what it takes away, the shape between the curve and the top. Both empty for a fade that isn't
/// there.
fn fade_paths(from: f32, to: f32, fade_in: bool) -> (String, String) {
    if to - from <= 1e-6 {
        return (String::new(), String::new());
    }
    let mut curve = String::new();
    const STEPS: usize = 24;
    for i in 0..=STEPS {
        let x = i as f32 / STEPS as f32;
        let level = if fade_in { CORNER.gain(x) } else { CORNER.gain(1.0 - x) };
        let px = (from + (to - from) * x) * 1000.0;
        let py = 100.0 - level * 100.0;
        let _ = write!(curve, "{}{px:.1} {py:.1}", if i == 0 { "M" } else { " L" });
    }
    // Up to the top at the fade's silent end, and back along the curve.
    let corner = if fade_in { from } else { to } * 1000.0;
    let shade = format!("{curve} L{corner:.1} 0.0 Z");
    (curve, shade)
}

/// The most marks the ruler ever has, and how far apart its marks are at the least (points): room for a number beside
/// each ("12.3").
const MOST_MARKS: f64 = 18.0;
const MARK_ROOM: f64 = 40.0;

/// The ruler's marks between `from` and `to` seconds: bars and beats from the start when the tempo is known ("1",
/// "1.2", "1.3", "1.4", "2"…), else seconds; never more than `most` of them (or one more, at the very end).
fn ruler(from: f64, to: f64, bpm: Option<f64>, most: f64) -> Vec<Mark> {
    let long = to - from;
    if long <= 0.0 {
        return Vec::new();
    }
    let mut marks = Vec::new();
    let mut mark = |t: f64, text: String, bar: bool| {
        marks.push(Mark { at: ((t - from) / long) as f32, text, bar });
    };
    match bpm {
        Some(bpm) => {
            let beat = 60.0 / bpm;
            // Every beat, every other beat, or every so many bars.
            let beats_each = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0]
                .into_iter()
                .find(|n| long / (beat * n) <= most)
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
                .find(|e| long / e <= most)
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
    arrived(host, ask, id, Answer::Read(Ok(Read::of(audio, None))));
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
        let read = Read::of(sound(frames), None);
        Opened {
            original: Some(read.audio),
            outline: Some(read.outline),
            edits: Edits { now: edits, ..Edits::default() },
            ..Opened::new(1, Source { path: "a.wav".into(), span: None }, "a".into(), 0, None)
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
        let texts = |marks: &[Mark]| marks.iter().map(|m| m.text.clone()).collect::<Vec<_>>();
        // 120 BPM: a beat is half a second.
        let marks = ruler(0.0, 2.0, Some(120.0), MOST_MARKS);
        assert_eq!(texts(&marks), ["1", "1.2", "1.3", "1.4", "2"]);
        assert!(marks[0].bar && !marks[1].bar);
        assert!((marks[4].at - 1.0).abs() < 1e-6);
        // Long: every so many bars, never too many.
        assert!(ruler(0.0, 120.0, Some(120.0), MOST_MARKS).len() <= 18);
        let marks = ruler(0.25, 1.0, None, MOST_MARKS);
        assert!(marks.len() <= 18 && marks.first().is_some_and(|m| m.at >= 0.0));
        assert_eq!(texts(&ruler(0.0, 3.0, None, MOST_MARKS))[..3], ["0.0", "0.2", "0.4"]);
        // A narrow screen has room for fewer: four bars at 128 BPM count every other beat with room for 13 marks, and
        // only the bars with room for 6.
        let bars = 16.0 * 60.0 / 128.0;
        assert_eq!(ruler(0.0, bars, Some(128.0), MOST_MARKS).len(), 17);
        assert_eq!(texts(&ruler(0.0, bars, Some(128.0), 13.0)), ["1", "1.3", "2", "2.3", "3", "3.3", "4", "4.3", "5"]);
        assert_eq!(texts(&ruler(0.0, bars, Some(128.0), 6.0)), ["1", "2", "3", "4", "5"]);
        assert_eq!(texts(&ruler(0.0, 3.0, None, 6.0)), ["0.0", "0.5", "1.0", "1.5", "2.0", "2.5", "3.0"]);
    }

    #[test]
    fn lengths_read_in_beats_with_the_tempo_known() {
        assert_eq!(part_text(0.47, 0.0), "0.47 s");
        assert_eq!(part_text(0.469, 128.0), "0.47 s · 1 beat");
        assert_eq!(part_text(0.9375, 128.0), "0.94 s · 2 beats");
        assert_eq!(part_text(7.5, 128.0), "7.50 s · 4 bars");
        assert_eq!(part_text(0.7, 128.0), "0.70 s · 1.5 beats");
        assert_eq!(facts(3.75, Some(128.0)), "3.75 s · 128 BPM · 2 bars");
        assert_eq!(facts(2.0, Some(90.5)), "2.00 s · 90.5 BPM · 3 beats");
        assert_eq!(facts(1.2, None), "1.20 s");
        assert_eq!(db_text(-1.5), "\u{2212}1.5 dB");
        assert_eq!(db_text(0.0), "0.0 dB");
        assert_eq!(gain_text(2.0), "Gain +2.0 dB");
        assert_eq!(peak_text(0.5), "\u{2212}6.0");
        assert_eq!(peak_text(2.0), "+6.0");
        assert_eq!((peak_text(1.0), peak_text(0.0)), ("0.0".to_owned(), String::new()));
        assert_eq!(
            chip_text(&Edit::Fade { span: Span::new(0, 6), way: Way::In, curve: CORNER }, 1_000.0),
            "Fade in 6 ms"
        );
    }

    #[test]
    fn a_fades_curve_runs_from_silence_to_full_with_what_it_takes_away_above_it() {
        let (curve, shade) = fade_paths(0.0, 0.5, true);
        assert!(curve.starts_with("M0.0 100.0") && curve.ends_with("500.0 0.0"), "{curve}");
        assert_eq!(shade, format!("{curve} L0.0 0.0 Z"));
        let (curve, shade) = fade_paths(0.5, 1.0, false);
        assert!(curve.starts_with("M500.0 0.0") && curve.ends_with("1000.0 100.0"), "{curve}");
        assert_eq!(shade, format!("{curve} L1000.0 0.0 Z"));
        assert_eq!(fade_paths(0.3, 0.3, true), (String::new(), String::new()));
    }

    #[test]
    fn the_edit_is_the_original_itself_until_there_are_edits() {
        let mut a = sound(4_000);
        a.channels[0][1_500] = -0.9;
        let read = Read::of(a, None);
        let made = make(&read.audio, &read.outline, Vec::new());
        assert!(Arc::ptr_eq(&made.audio, &read.audio) && Arc::ptr_eq(&made.outline, &read.outline));
        assert!((made.outline.peak() - 0.9).abs() < 1e-6);
        let (high, low) = made.outline.steps_for_tests();
        assert_eq!((high.len(), high[1_500], low[1_500]), (4_000, 0.0, -0.9));
        // With edits it's a sound of its own, and its outline is the edit's: a gain shows.
        let made = make(&read.audio, &read.outline, vec![Edit::Gain { db: -6.0, span: None }]);
        assert!(!Arc::ptr_eq(&made.audio, &read.audio) && !Arc::ptr_eq(&made.outline, &read.outline));
        assert!((made.outline.peak() - 0.451).abs() < 1e-3, "a gain shows: {}", made.outline.peak());
    }

    #[test]
    fn two_lists_that_differ_only_in_their_corner_fades_leave_the_same_without_them() {
        let fade = |frames: usize| Edit::Fade { span: Span::new(1_000 - frames, 1_000), way: Way::Out, curve: CORNER };
        let gain = Edit::Gain { db: -3.0, span: None };
        assert_eq!(without_corners(1_000, &[gain, fade(100)]), [gain]);
        assert_eq!(without_corners(1_000, &[fade(400), gain]), without_corners(1_000, &[gain]));
        assert_eq!(corner_seconds(1_000, &[gain, fade(100)], 1_000.0), (0.0, 0.1));
        // A fade a trim has baked in is no corner's any more: it stays.
        let baked = [fade(100), Edit::Trim(Span::new(0, 950))];
        assert_eq!(without_corners(1_000, &baked), baked);
        assert_eq!(corner_seconds(1_000, &baked, 1_000.0), (0.0, 0.0));
    }

    fn room() -> Room {
        Room { width: 120.0, height: 100.0, inset: 10.0, band: 20.0, top: 24.0, bottom: 96.0, scale: 1.0 }
    }

    /// A strip open on a sound `frames` long with `edits`, read and made, with room for a waveform 100 columns wide.
    fn strip(frames: usize, edits: Vec<Edit>) -> Strip {
        let mut o = opened(frames, edits.clone());
        o.made = Some(make(o.original.as_ref().unwrap(), o.outline.as_ref().unwrap(), edits));
        Strip { open: true, sound: Some(o), room: Some(room()), ..Strip::default() }
    }

    /// Makes the open sound's edit again from its edits as they are now, as a make that came back does.
    fn made_again(s: &mut Strip) {
        let o = s.sound.as_mut().unwrap();
        o.made = Some(make(o.original.as_ref().unwrap(), o.outline.as_ref().unwrap(), o.edits.now.clone()));
    }

    #[test]
    fn the_waveform_is_handed_out_again_until_something_in_it_changes() {
        let mut s = strip(1_000, Vec::new());
        let wave = showing(&mut s, false).wave.expect("there's room: a waveform");
        assert_eq!((wave.columns.len(), wave.original, wave.before.is_none()), (100, false, true));
        // From this far out (10 ms a column) it's drawn the same above and below, as far as it gets either way.
        let first = wave.columns[0];
        assert!((first.top, first.bottom) == (0.5, -0.5) && (first.body - 0.6).abs() < 1e-6, "{first:?}");
        // The ruler's marks go behind it: as many as this room has space for a number beside (half a second apart),
        // the whole seconds' brighter.
        assert_eq!((wave.marks.len(), wave.marks[0], wave.marks[1]), (3, (0.0, true), (0.5, false)));
        // Shown again with nothing changed: the very same one, so the host draws nothing.
        assert!(Arc::ptr_eq(&showing(&mut s, false).wave.unwrap(), &wave));
        // Zoomed in on its first half: another.
        s.sound.as_mut().unwrap().zoom = Some((0.0, 0.5));
        let zoomed = showing(&mut s, false).wave.unwrap();
        assert!(!Arc::ptr_eq(&zoomed, &wave) && zoomed.marks != wave.marks);
        // Another room: as many columns as it has pixels.
        s.room = Some(Room { width: 220.0, ..room() });
        assert_eq!(showing(&mut s, false).wave.unwrap().columns.len(), 200);
        // No room a waveform fits in: none, and nothing kept of the last.
        s.room = Some(Room { width: 12.0, ..room() });
        assert!(showing(&mut s, false).wave.is_none() && s.columns.is_none() && s.wave.is_none());
        s.room = None;
        assert!(showing(&mut s, false).wave.is_none());
        // A strip with no sound in it keeps nothing either.
        let mut s = strip(1_000, Vec::new());
        assert!(showing(&mut s, false).wave.is_some() && s.columns.is_some());
        s.sound = None;
        assert!(showing(&mut s, false).wave.is_none() && s.columns.is_none() && s.wave.is_none());
    }

    #[test]
    fn a_fade_reshapes_the_waveform_while_its_handle_is_dragged() {
        let mut s = strip(1_000, Vec::new());
        let plain = showing(&mut s, false).wave.unwrap();
        // The fade out's handle dragged back to 0.6 s: the last 0.4 s fall away at once, with no edit made.
        s.sound.as_mut().unwrap().dragging = Some((false, 0.6));
        let dragged = showing(&mut s, false);
        let wave = dragged.wave.clone().unwrap();
        assert_eq!(wave.columns[60], plain.columns[60]);
        assert!(wave.columns[61].top < 0.5 && wave.columns[99].top < 0.05, "{:?}", wave.columns[99]);
        // How it was shows behind, over the columns the fade changes.
        let (first, before) = wave.before.clone().expect("how it was");
        assert_eq!((first, before.len(), before[0]), (61, 39, (0.5, -0.5)));
        assert_eq!(dragged.fade_text, "Fade out 400 ms");
        assert_eq!(dragged.chips, [Chip { text: "Fade out 400 ms".into(), last: true }]);
        assert!((dragged.fade_out - 0.6).abs() < 1e-6 && dragged.fade_out_curve.starts_with("M600.0 0.0"));
        assert!(dragged.fade_out_shade.ends_with(" L1000.0 0.0 Z") && dragged.fade_in_shade.is_empty());
        // Let go: the edits have the fade before the edit is made with it. The waveform keeps its shape meanwhile,
        // with nothing behind it now, and the handle stays where it was let go.
        let o = s.sound.as_mut().unwrap();
        o.dragging = None;
        o.edits.change(vec![Edit::Fade { span: Span::new(600, 1_000), way: Way::Out, curve: CORNER }]);
        let waiting = showing(&mut s, false);
        let kept = waiting.wave.clone().unwrap();
        assert!(kept.columns == wave.columns && kept.before.is_none() && waiting.fade_text.is_empty());
        assert!((waiting.fade_out - 0.6).abs() < 1e-6);
        // Made: the edit's own columns, as good as the same.
        made_again(&mut s);
        let made = showing(&mut s, false).wave.unwrap();
        let apart =
            made.columns.iter().zip(kept.columns.iter()).map(|(a, b)| (a.top - b.top).abs()).fold(0.0, f32::max);
        assert!(apart < 0.02, "{apart}");
        // Dragged back into its corner: no fade, the sound back up, and the fade there was behind it.
        s.sound.as_mut().unwrap().dragging = Some((false, 1.0));
        let none = showing(&mut s, false);
        assert_eq!(none.fade_text, "No fade out");
        assert!(none.chips.is_empty());
        let back = none.wave.unwrap();
        assert!((back.columns[80].top - 0.5).abs() < 0.02, "{:?}", back.columns[80]);
        assert_eq!(back.before.as_ref().map(|(first, _)| *first), Some(61));
        // Another edit than a fade waiting to be made: the waveform waits for it.
        let o = s.sound.as_mut().unwrap();
        o.dragging = None;
        o.edits.change(vec![o.edits.now[0], Edit::Reverse(None)]);
        assert!(showing(&mut s, false).wave.unwrap().columns == made.columns);
    }

    #[test]
    fn the_original_shows_in_grey_while_it_is_held() {
        let mut s = strip(1_000, vec![Edit::Gain { db: -6.0, span: None }, Edit::Trim(Span::new(500, 1_000))]);
        let edit = showing(&mut s, false);
        let wave = edit.wave.clone().unwrap();
        assert!(!wave.original && wave.columns[0].top < 0.3, "{:?}", wave.columns[0]);
        s.sound.as_mut().unwrap().held = Some(true);
        let held = showing(&mut s, true);
        let original = held.wave.clone().unwrap();
        // The half second the trim kept, as it was before the gain; and the meter reads the original too.
        assert!(held.original && original.original && original.columns.len() == 100);
        assert!((original.columns[50].top - 0.5).abs() < 1e-6, "{:?}", original.columns[50]);
        assert_eq!((edit.levels.peak_text.as_str(), held.levels.peak_text.as_str()), ("\u{2212}12.0", "\u{2212}6.0"));
        // Let go: the edit again, with nothing worked out afresh but its columns.
        s.sound.as_mut().unwrap().held = None;
        assert!(showing(&mut s, false).wave.unwrap().columns == wave.columns);
    }

    #[test]
    fn the_meter_reads_decibels_up_to_full_scale() {
        assert_eq!(meter_place(1.0), 1.0);
        assert_eq!(meter_place(4.0), 1.0, "over full scale is its top");
        assert_eq!((meter_place(0.0), meter_place(f32::NAN), meter_place(0.001)), (0.0, 0.0, 0.0));
        // The last 6 dB under full scale are amber.
        assert!((meter_place(0.501_187) - METER_AMBER).abs() < 1e-4);
        // Left and right apart, from the samples themselves.
        let both =
            AudioData { sample_rate: RATE, channels: vec![vec![0.25; 100], vec![-0.5; 100]], ..AudioData::default() };
        assert_eq!(loudest(&both, 0.01, 0.02), [0.25, 0.5]);
        assert_eq!(loudest(&both, 0.5, 0.6), [0.0, 0.0], "past its end");
        assert_eq!(loudest(&both, 0.05, 0.05), [0.0, 0.0], "no time at all");
        assert_eq!(loudest(&sound(100), -1.0, 0.05), [0.5, 0.5], "one channel is both sides");
    }

    #[test]
    fn the_meter_shows_the_edits_loudest_then_the_level_as_it_plays() {
        // Half a second at 0.5 and then a second of silence, turned up 12 dB: about 2, well over full scale.
        let mut s = strip(1_500, Vec::new());
        {
            let o = s.sound.as_mut().unwrap();
            let mut quiet = sound(1_500);
            quiet.channels[0][500..].fill(0.0);
            let read = Read::of(quiet, None);
            (o.original, o.outline) = (Some(read.audio), Some(read.outline));
            o.edits.change(vec![Edit::Gain { db: 12.0, span: None }]);
        }
        made_again(&mut s);
        let rest = showing(&mut s, false);
        assert!(rest.too_loud && rest.levels.over);
        assert_eq!((rest.levels.level, rest.levels.hold), ([1.0; 2], [1.0; 2]));
        assert_eq!(rest.levels.peak_text, "+6.0");
        // It plays from 0.2 s: the meter starts from nothing, and takes the level of what has played.
        s.sound.as_mut().unwrap().played = Some(Played::starting(0.2));
        assert_eq!(showing(&mut s, true).levels.level, [0.0; 2]);
        let Ticked::At(playing) = tick(&mut s, Some(0.3), None) else { panic!("it plays") };
        assert!((playing.at - 0.2).abs() < 1e-6 && (playing.from - 0.2 / 1.5).abs() < 1e-6, "{playing:?}");
        assert_eq!(playing.level, [1.0; 2]);
        // Shown meanwhile (a tool used as it plays): the meter stays where the play has it.
        assert_eq!(showing(&mut s, true).levels.level, [1.0; 2]);
        // Into the silence: it falls back, 40 dB a second, over what has played since.
        let Ticked::At(playing) = tick(&mut s, Some(0.6), None) else { panic!("it plays") };
        assert_eq!(playing.level, [1.0; 2], "0.3 s to 0.6 s still has the sound in it");
        let Ticked::At(playing) = tick(&mut s, Some(0.7), None) else { panic!("it plays") };
        assert!((playing.level[0] - (1.0 - METER_FALL * 0.1)).abs() < 1e-5, "{:?}", playing.level);
        let Ticked::At(same) = tick(&mut s, Some(0.7), None) else { panic!("it plays") };
        assert_eq!(same.level, playing.level, "no time gone by: no fall");
        // A long wait between two ticks counts as a quarter of a second at most.
        let Ticked::At(later) = tick(&mut s, Some(1.4), None) else { panic!("it plays") };
        assert!((later.level[0] - (playing.level[0] - METER_FALL * 0.25)).abs() < 1e-5, "{:?}", later.level);
        // A play of a part ends at the part's end; one whose start wasn't seen is lit from where it's first seen.
        assert_eq!(tick(&mut s, Some(0.5), Some(0.4)), Ticked::Ended);
        s.sound.as_mut().unwrap().played = None;
        let Ticked::At(found) = tick(&mut s, Some(0.75), None) else { panic!("it plays") };
        assert_eq!((found.at, found.from), (0.5, 0.5));
        assert_eq!(tick(&mut s, None, None), Ticked::Waiting);
        // Stopped: the meter is back at the edit's loudest, and nothing is lit.
        assert_eq!(showing(&mut s, false).levels.level, [1.0; 2]);
        assert!(s.sound.as_ref().unwrap().played.is_none());
    }

    #[test]
    fn the_part_the_strip_is_on_is_found_in_the_sounds_own_file() {
        let near = |part: Option<(f32, f32)>, (from, to): (f32, f32)| {
            let (a, b) = part.expect("a part of the file");
            assert!((a - from).abs() < 1e-6 && (b - to).abs() < 1e-6, "{a} to {b}, not {from} to {to}");
        };
        // No edits and nothing picked: all of the file, so there's no part of it to mark.
        let mut s = strip(1_000, Vec::new());
        assert_eq!(showing(&mut s, false).file_part, None);
        // A part picked: that part of the file.
        s.sound.as_mut().unwrap().pick = Some((0.25, 0.5));
        near(showing(&mut s, false).file_part, (0.25, 0.5));
        // Trimmed to its second half: the edit is that half of the file, and a part picked in it is as far in again.
        let mut s = strip(1_000, vec![Edit::Trim(Span::new(500, 1_000))]);
        near(showing(&mut s, false).file_part, (0.5, 1.0));
        s.sound.as_mut().unwrap().pick = Some((0.1, 0.2));
        near(showing(&mut s, false).file_part, (0.6, 0.7));
        // And then played backwards: the part came from the other end of what the trim kept.
        let mut s = strip(1_000, vec![Edit::Trim(Span::new(500, 1_000)), Edit::Reverse(None)]);
        s.sound.as_mut().unwrap().pick = Some((0.1, 0.2));
        near(showing(&mut s, false).file_part, (0.8, 0.9));
        // A gain or a fade moves nothing: still all of the file.
        let mut s = strip(1_000, vec![Edit::Gain { db: -3.0, span: None }]);
        assert_eq!(showing(&mut s, false).file_part, None);
        // Only a part of a long sound was read: where that is in its file isn't the strip's to say.
        s.sound.as_mut().unwrap().pick = Some((0.1, 0.2));
        s.sound.as_mut().unwrap().source.span = Some((30.0, 31.0));
        assert_eq!(showing(&mut s, false).file_part, None);
        // Nothing read yet: nothing to say.
        s.sound.as_mut().unwrap().made = None;
        assert_eq!(showing(&mut s, false).file_part, None);
    }

    #[test]
    fn the_play_is_found_in_the_sounds_own_file_too() {
        let in_file = |s: &mut Strip, at: f64| match tick(s, Some(at), None) {
            Ticked::At(playing) => playing.in_file,
            other => panic!("it plays: {other:?}"),
        };
        // The second half of the file, backwards: a tenth of a second into the edit is its frame 100, which was frame
        // 399 of the half the trim kept, and 899 of the file.
        let mut s = strip(1_000, vec![Edit::Trim(Span::new(500, 1_000)), Edit::Reverse(None)]);
        assert!((in_file(&mut s, 0.1).expect("in the file") - 0.899).abs() < 1e-6);
        // The original held: what plays is the file itself.
        s.sound.as_mut().unwrap().held = Some(true);
        assert!((in_file(&mut s, 0.1).expect("in the file") - 0.1).abs() < 1e-6);
        // Past its end is its end.
        assert_eq!(in_file(&mut s, 7.0), Some(1.0));
        // Only a part of a long sound was read: there's nowhere in its file to say.
        s.sound.as_mut().unwrap().source.span = Some((30.0, 31.0));
        assert_eq!(in_file(&mut s, 0.1), None);
        // Where the play started is found the same way. Through the reverse it runs from right to left in the file:
        // from frame 899 down to 699, two tenths of a second on.
        let mut s = strip(1_000, vec![Edit::Trim(Span::new(500, 1_000)), Edit::Reverse(None)]);
        let Ticked::At(start) = tick(&mut s, Some(0.1), None) else { panic!("it plays") };
        let Ticked::At(later) = tick(&mut s, Some(0.3), None) else { panic!("it plays") };
        let near = |a: Option<f32>, b: f32| (a.expect("in the file") - b).abs() < 1e-6;
        assert!(near(start.from_in_file, 0.899) && near(later.from_in_file, 0.899) && near(later.in_file, 0.699));
    }

    #[test]
    fn a_strip_put_away_keeps_its_edits_and_nothing_of_the_host_it_had() {
        let host = Fake::on(strip(1_000, vec![Edit::Reverse(None)]));
        show(&host);
        assert!(host.strip.borrow().wave.is_some());
        assert_eq!(open_on(&host.strip.borrow()), Some(1));
        // A copy saved, and then another on its way as the host goes.
        assert_eq!(run_action(&host, SAVE), Some(true));
        saved(&host, 1, Ok("Edited sounds/a (edit 1f).wav".into()));
        let mut s = host.strip.take();
        let asked = s.asked;
        put_away(&mut s);
        assert!(!s.open && s.sound.is_none() && open_on(&s).is_none());
        assert!(s.room.is_none() && s.columns.is_none() && s.wave.is_none());
        // Its edits wait with the kept ones, and an answer still on its way for the old host is nobody's.
        assert_eq!(s.kept.len(), 1);
        assert_eq!(s.kept[0].edits.now, [Edit::Reverse(None)]);
        assert!(s.asked > asked);
        // The copy that was saved is still known as saved; one still being saved isn't waited for.
        assert!(s.copy.as_ref().is_some_and(|c| matches!(c.state, CopyState::Saved(_))));
        s.copy.as_mut().unwrap().state = CopyState::Saving;
        put_away(&mut s);
        assert!(s.copy.is_none());
        // Put away again, or with nothing in it: nothing to do.
        put_away(&mut Strip::default());
    }

    /// The plugin keeps the strip from one window to the next, on whichever thread the DAW opens each on.
    #[test]
    fn a_strip_can_wait_for_its_next_host_on_another_thread() {
        fn sendable<T: Send>() {}
        sendable::<Strip>();
    }

    /// A host that only keeps what it's asked.
    #[derive(Default)]
    struct Fake {
        strip: RefCell<Strip>,
        shown: RefCell<Option<Showing>>,
        plays: std::cell::Cell<bool>,
        /// Where the last play started (seconds into what it played).
        played_from: std::cell::Cell<Option<f64>>,
        /// The swaps asked for: whether each was the original, held.
        swaps: RefCell<Vec<bool>>,
        takes_saves: std::cell::Cell<bool>,
        saving: RefCell<Vec<u64>>,
        revealed: RefCell<Vec<PathBuf>>,
        carried: RefCell<Vec<&'static str>>,
        /// The sends asked for (where to, and how many slices), and whether it takes them.
        takes_sends: std::cell::Cell<bool>,
        sends: RefCell<Vec<(slices::To, usize)>>,
    }

    impl Fake {
        fn on(strip: Strip) -> Fake {
            let host = Fake { strip: RefCell::new(strip), ..Fake::default() };
            host.takes_saves.set(true);
            host
        }

        /// The strip as it was last shown.
        fn shown(&self) -> Showing {
            self.shown.borrow().clone().expect("shown")
        }
    }

    impl Host for Fake {
        fn strip(&self) -> &RefCell<Strip> {
            &self.strip
        }
        fn picked(&self) -> Option<Picked> {
            None
        }
        fn card_part(&self) -> Option<(f64, f64)> {
            None
        }
        fn show(&self, showing: Showing) {
            *self.shown.borrow_mut() = Some(showing);
        }
        fn work(&self, _: u64, _: i64, _: Work) {}
        fn play(&self, _: Arc<AudioData>, from: f64, _: Option<f64>) {
            self.plays.set(true);
            self.played_from.set(Some(from));
        }
        fn swap(&self, _: Arc<AudioData>, _: &dyn Fn(f64) -> f64, held: bool) -> bool {
            self.swaps.borrow_mut().push(held);
            self.plays.get()
        }
        fn plays(&self) -> bool {
            self.plays.get()
        }
        fn picked_plays(&self) -> bool {
            self.plays.get()
        }
        fn stop(&self) {
            self.plays.set(false);
        }
        fn played(&self) {}
        fn carry(&self, what: Carried) {
            self.carried.borrow_mut().push(match what {
                Carried::Whole => "the sound",
                Carried::Part(..) => "its part",
                Carried::Edit { .. } => "an edit",
            });
        }
        fn let_go(&self) {}
        fn save(&self, edit: Saving) -> bool {
            self.saving.borrow_mut().push(edit.ask);
            self.takes_saves.get()
        }
        fn reveal(&self, copy: PathBuf) {
            self.revealed.borrow_mut().push(copy);
        }
        fn send(&self, to: slices::To, slices: slices::Slices) -> bool {
            self.sends.borrow_mut().push((to, slices.spans.len()));
            self.takes_sends.get()
        }
    }

    #[test]
    fn a_save_shows_how_it_went_while_the_edits_are_the_ones_saved() {
        let host = Fake::on(strip(1_000, vec![Edit::Reverse(None)]));
        show(&host);
        let now = host.shown();
        assert_eq!((now.save_text.as_str(), now.can_save, now.saved), ("Save copy", true, false));
        assert_eq!(run_action(&host, SAVE), Some(true));
        assert_eq!(*host.saving.borrow(), [1]);
        assert_eq!((host.shown().save_text.as_str(), host.shown().can_save), ("Saving\u{2026}", false));
        // Asked again meanwhile: that copy is on its way already.
        assert_eq!(run_action(&host, SAVE), Some(true));
        assert_eq!(host.saving.borrow().len(), 1);
        // An answer to some other save is nobody's.
        saved(&host, 7, Err("no".into()));
        assert_eq!(host.shown().save_text, "Saving\u{2026}");
        saved(&host, 1, Ok("Edited sounds/a (edit 1f).wav".into()));
        let now = host.shown();
        assert_eq!((now.save_text.as_str(), now.saved, now.can_save), ("Saved", true, true));
        assert_eq!((now.save_note.as_str(), now.save_detail.as_str()), ("Saved in Edited sounds", "a (edit 1f).wav"));
        // Saved, and Save can still be pressed: a copy that has gone since is made again (the host knows if it's there).
        assert_eq!(run_action(&host, SAVE), Some(true));
        assert_eq!(*host.saving.borrow(), [1, 2]);
        assert_eq!(host.shown().save_text, "Saving\u{2026}");
        saved(&host, 2, Ok("Edited sounds/a (edit 1f).wav".into()));
        assert!(host.shown().saved);
        show_saved(&host);
        assert_eq!(*host.revealed.borrow(), [PathBuf::from("Edited sounds/a (edit 1f).wav")]);
        // Another edit: that copy isn't of this one, and there's none to show.
        assert_eq!(run_action(&host, NORMALISE), Some(true));
        let now = host.shown();
        assert!(!now.saved && now.can_save && now.save_note.is_empty() && now.save_text == "Save copy");
        show_saved(&host);
        assert_eq!(host.revealed.borrow().len(), 1);
        // Undone: these are the edits that copy is of.
        assert_eq!(run_action(&host, UNDO), Some(true));
        assert!(host.shown().saved);
        // A save the host doesn't take leaves nothing waiting.
        host.takes_saves.set(false);
        assert_eq!(run_action(&host, REDO), Some(true));
        assert_eq!(run_action(&host, SAVE), Some(false));
        assert!(host.shown().can_save && host.shown().save_note.is_empty());
        // One that fails says why, and Save can be tried again; closing the strip puts that behind it.
        host.takes_saves.set(true);
        assert_eq!(run_action(&host, SAVE), Some(true));
        saved(&host, 4, Err("The disk is full.".into()));
        let failed = host.shown();
        assert!(failed.save_failed && failed.can_save && !failed.saved);
        assert_eq!(
            (failed.save_note.as_str(), failed.save_detail.as_str()),
            ("Couldn't save the copy", "The disk is full.")
        );
        close(&host);
        assert!(host.strip.borrow().copy.is_none());
        // With no edits there's nothing to save.
        let host = Fake::on(strip(1_000, Vec::new()));
        assert_eq!(run_action(&host, SAVE), Some(false));
        assert!(host.saving.borrow().is_empty());
    }

    #[test]
    fn the_drag_handle_carries_the_part_picked_or_the_edit() {
        let host = Fake::on(strip(1_000, Vec::new()));
        let words = || {
            show(&host);
            host.shown().drag_text
        };
        assert_eq!(words(), "Drag the sound");
        drag_handle(&host);
        host.strip.borrow_mut().sound.as_mut().unwrap().pick = Some((0.1, 0.4));
        assert_eq!(words(), "Drag the part");
        drag_handle(&host);
        host.strip.borrow_mut().sound.as_mut().unwrap().pick = None;
        assert_eq!(run_action(&host, REVERSE), Some(true));
        assert_eq!(words(), "Drag the edit");
        drag_handle(&host);
        // A part of a sound with no edits is a trim of it: an edit all the same.
        assert_eq!(*host.carried.borrow(), ["the sound", "an edit", "an edit"]);
        assert!(host.shown().can_drag);
    }

    #[test]
    fn with_the_slices_on_the_drag_handle_carries_the_slice_picked() {
        // A sound with two hits: two slices once the scissors are on.
        let mut strip = strip(1_000, Vec::new());
        let o = strip.sound.as_mut().unwrap();
        o.pick = Some((0.1, 0.4));
        let made = o.made.as_mut().unwrap();
        made.all_hits = [(0.0, 40.0), (0.5, 40.0)].into();
        made.hits = [0.0, 0.5].into();
        let host = Fake::on(strip);
        show(&host);
        assert_eq!(host.shown().wave.expect("a wave").hits, [0.0, 0.5], "the hits' marks, with the Slices off");
        assert_eq!(run_action(&host, slices::SLICES), Some(true));
        let now = host.shown();
        // None picked: nothing to carry (the part picked before isn't the Slices'), and no hits' marks, as the
        // slices' numbers are along the foot.
        assert_eq!((now.drag_text.as_str(), now.can_drag, now.slices.slice), ("Drag a slice", false, 0));
        assert!(now.wave.expect("a wave").hits.is_empty());
        drag_handle(&host);
        assert!(host.carried.borrow().is_empty());
        // A pad pressed picks its slice and plays it; the handle then carries that slice, a trim of the sound.
        slices::pad_pressed(&host, 1);
        assert_eq!(host.played_from.get(), Some(0.5));
        let now = host.shown();
        assert_eq!((now.drag_text.as_str(), now.can_drag, now.slices.slice), ("Drag slice 2", true, 2));
        drag_handle(&host);
        assert_eq!(*host.carried.borrow(), ["an edit"]);
        // A pad with no slice on it does nothing. The same pad again is one more pick of the same slice: the pads
        // turn back to its page.
        let picks = host.shown().slices.picks;
        slices::pad_pressed(&host, 7);
        assert_eq!((host.shown().slices.slice, host.shown().slices.picks), (2, picks));
        slices::pad_pressed(&host, 1);
        assert_eq!((host.shown().slices.slice, host.shown().slices.picks), (2, picks + 1));
        // The Slices off again: the part picked before is what it carries.
        assert_eq!(run_action(&host, slices::SLICES), Some(true));
        let now = host.shown();
        assert_eq!((now.drag_text.as_str(), now.can_drag), ("Drag the part", true));
        assert_eq!(now.wave.expect("a wave").hits, [0.0, 0.5]);
    }

    /// A sound two seconds long with a hit every half second, read and made: the one with this `id` in the host's
    /// list.
    fn four_hits(id: i64) -> Opened {
        let mut o = opened(2_000, Vec::new());
        (o.id, o.source.path) = (id, format!("{id}.wav").into());
        let mut made = make(o.original.as_ref().unwrap(), o.outline.as_ref().unwrap(), Vec::new());
        made.all_hits = [(0.0, 40.0), (0.5, 40.0), (1.0, 40.0), (1.5, 40.0)].into();
        o.made = Some(made);
        o
    }

    /// A strip open on such a sound, on a host that takes sends, with the scissors on: four slices.
    fn four_slices() -> Fake {
        let host = Fake::on(Strip { open: true, sound: Some(four_hits(1)), room: Some(room()), ..Strip::default() });
        host.takes_sends.set(true);
        assert_eq!(run_action(&host, slices::SLICES), Some(true));
        host
    }

    /// Opens `next` in place of the open sound, as picking another in the list does once it's read: the one that was
    /// open is put by with its edits and slicing, and `next` gets back what was put by of it.
    fn open_instead(host: &Fake, mut next: Opened) {
        {
            let mut s = host.strip.borrow_mut();
            stash(&mut s);
            if let Some(at) = s.kept.iter().position(|k| k.id == next.id && k.source == next.source) {
                let kept = s.kept.remove(at);
                (next.edits, next.sliced) = (kept.edits, kept.sliced);
            }
            s.sound = Some(next);
        }
        show(host);
    }

    /// What the strip last showed of a send: the button's words, whether it can be pressed, how far along its bar is,
    /// whether the slices are there, how many pads are filled, and whether the next is being made.
    fn send_shown(host: &Fake) -> (String, bool, Option<f32>, bool, usize, bool) {
        let now = host.shown().slices;
        (now.send_text, now.can_send, now.sending, now.sent, now.made, now.making)
    }

    #[test]
    fn a_send_counts_the_slices_along_and_says_where_they_are() {
        let host = four_slices();
        assert_eq!(send_shown(&host), ("Send 4 slices".into(), true, None, false, 0, false));
        // Sent to Bitwig: the button counts along as the host makes them, and their pads fill.
        assert_eq!(run_action(&host, slices::TO_BITWIG), Some(true));
        assert_eq!(*host.sends.borrow(), [(slices::To::Bitwig, 4)]);
        assert_eq!(send_shown(&host), ("Making 1 of 4".into(), false, Some(0.0), false, 0, true));
        // Asked again meanwhile: they're on their way already.
        assert_eq!(run_action(&host, slices::TO_BITWIG), Some(false));
        assert_eq!(host.sends.borrow().len(), 1);
        slices::made(&host, 2);
        assert_eq!(send_shown(&host), ("Making 3 of 4".into(), false, Some(0.5), false, 2, true));
        // A count that gets there late never takes one back.
        slices::made(&host, 1);
        assert_eq!(send_shown(&host), ("Making 3 of 4".into(), false, Some(0.5), false, 2, true));
        // All made (a count past the last is all of them): it's Bitwig's turn.
        slices::made(&host, 9);
        assert_eq!(send_shown(&host), ("Waiting for Bitwig".into(), false, Some(1.0), false, 4, false));
        // They're in: the button says so, and can be pressed again; their pads stay filled.
        slices::sent(&host, true);
        assert_eq!(send_shown(&host), ("In Bitwig".into(), true, None, true, 4, false));
        // Cut another way, they're other slices, still to send; back as they went, it says so again.
        slices::each_picked(&host, "to the sound's end");
        assert_eq!(send_shown(&host), ("Send 4 slices".into(), true, None, false, 0, false));
        assert_eq!(run_action(&host, UNDO), Some(true));
        assert_eq!(send_shown(&host), ("In Bitwig".into(), true, None, true, 4, false));
        // To their folder (Save, with the Slices on): it counts along as far as the last, and says "Saved".
        assert_eq!(run_action(&host, SAVE), Some(true));
        assert_eq!(*host.sends.borrow(), [(slices::To::Bitwig, 4), (slices::To::Folder, 4)]);
        assert_eq!(send_shown(&host), ("Making 1 of 4".into(), false, Some(0.0), false, 0, true));
        slices::made(&host, 4);
        assert_eq!(send_shown(&host), ("Making 4 of 4".into(), false, Some(1.0), false, 4, false));
        slices::sent(&host, true);
        assert_eq!(send_shown(&host), ("Saved".into(), true, None, true, 4, false));
    }

    #[test]
    fn a_send_not_taken_or_of_other_slices_leaves_send_as_it_was() {
        let host = four_slices();
        // The host can't take them: Send is as it was.
        host.takes_sends.set(false);
        assert_eq!(run_action(&host, slices::TO_BITWIG), Some(true));
        assert_eq!(host.sends.borrow().len(), 1);
        assert_eq!(send_shown(&host), ("Send 4 slices".into(), true, None, false, 0, false));
        // A count, or an end, with no send on its way is nobody's.
        slices::made(&host, 3);
        slices::sent(&host, true);
        assert_eq!(send_shown(&host), ("Send 4 slices".into(), true, None, false, 0, false));
        // Taken, and the sound cut another way while they're on their way: these aren't the slices being sent, so
        // nothing counts along here, and Send waits for that send to end.
        host.takes_sends.set(true);
        assert_eq!(run_action(&host, slices::TO_BITWIG), Some(true));
        slices::made(&host, 1);
        slices::each_picked(&host, "to the sound's end");
        assert_eq!(send_shown(&host), ("Send 4 slices".into(), false, None, false, 0, false));
        slices::made(&host, 4);
        assert_eq!(send_shown(&host), ("Send 4 slices".into(), false, None, false, 0, false));
        // They went: Send comes back for these, and the ones that went are remembered as they were cut.
        slices::sent(&host, true);
        assert_eq!(send_shown(&host), ("Send 4 slices".into(), true, None, false, 0, false));
        assert_eq!(run_action(&host, UNDO), Some(true));
        assert_eq!(send_shown(&host), ("In Bitwig".into(), true, None, true, 4, false));
        // Sent again, and that one fails: its card says so, and here they're still where the first send put them.
        assert_eq!(run_action(&host, slices::TO_BITWIG), Some(true));
        assert_eq!(send_shown(&host), ("Making 1 of 4".into(), false, Some(0.0), false, 0, true));
        slices::sent(&host, false);
        assert_eq!(send_shown(&host), ("In Bitwig".into(), true, None, true, 4, false));
        // A send that fails with none before it leaves Send as it was.
        slices::each_picked(&host, "to the sound's end");
        assert_eq!(run_action(&host, SAVE), Some(true));
        slices::sent(&host, false);
        assert_eq!(send_shown(&host), ("Send 4 slices".into(), true, None, false, 0, false));
    }

    #[test]
    fn a_send_that_ends_with_another_sound_open_is_kept_with_its_own_sound() {
        let host = four_slices();
        assert_eq!(run_action(&host, slices::TO_BITWIG), Some(true));
        slices::made(&host, 2);
        assert_eq!(send_shown(&host), ("Making 3 of 4".into(), false, Some(0.5), false, 2, true));
        // Another sound opened meanwhile, cut just the same: the slices on their way aren't its own, so nothing
        // counts along on it, and its Send waits for them.
        open_instead(&host, four_hits(2));
        assert_eq!(run_action(&host, slices::SLICES), Some(true));
        assert_eq!(send_shown(&host), ("Send 4 slices".into(), false, None, false, 0, false));
        slices::made(&host, 4);
        assert_eq!(send_shown(&host), ("Send 4 slices".into(), false, None, false, 0, false));
        // They're in Bitwig: this sound's Send comes back, and it has sent nothing itself.
        slices::sent(&host, true);
        assert_eq!(send_shown(&host), ("Send 4 slices".into(), true, None, false, 0, false));
        // Back on the first sound, its slices are where they went.
        open_instead(&host, four_hits(1));
        assert_eq!(send_shown(&host), ("In Bitwig".into(), true, None, true, 4, false));
        // And a send that ends once its sound's slicing is no longer kept is nobody's: nothing is said of it.
        assert_eq!(run_action(&host, slices::TO_BITWIG), Some(true));
        host.strip.borrow_mut().sound = Some(four_hits(2));
        slices::sent(&host, true);
        assert_eq!(run_action(&host, slices::SLICES), Some(true));
        assert_eq!(send_shown(&host), ("Send 4 slices".into(), true, None, false, 0, false));
    }

    #[test]
    fn where_the_slices_went_is_kept_with_the_slices_off_and_another_sound_open() {
        let host = four_slices();
        assert_eq!(run_action(&host, slices::TO_BITWIG), Some(true));
        // The Slices off and another sound opened while they're on their way: its slicing, as usual as it is, is
        // kept for them. Then they're in Bitwig.
        assert_eq!(run_action(&host, slices::SLICES), Some(true));
        assert!(!host.shown().slices.on);
        open_instead(&host, four_hits(2));
        slices::sent(&host, true);
        // Back on the first sound with its Slices on again: they're the ones that went.
        open_instead(&host, four_hits(1));
        assert!(!host.shown().slices.on);
        assert_eq!(run_action(&host, slices::SLICES), Some(true));
        assert_eq!(send_shown(&host), ("In Bitwig".into(), true, None, true, 4, false));
        // Put by once more with the Slices off and nothing on its way: where they went is still kept.
        assert_eq!(run_action(&host, slices::SLICES), Some(true));
        open_instead(&host, four_hits(2));
        open_instead(&host, four_hits(1));
        assert_eq!(run_action(&host, slices::SLICES), Some(true));
        assert_eq!(send_shown(&host), ("In Bitwig".into(), true, None, true, 4, false));
        // A sound with its Slices off, as usual and never sent, isn't kept at all.
        assert_eq!(host.strip.borrow().kept.len(), 0);
    }

    #[test]
    fn a_play_is_lit_from_where_it_started() {
        let host = Fake::on(strip(1_000, Vec::new()));
        // A click 0.3 of the way along plays from there.
        seek(&host, 0.3);
        assert!(host.plays.get());
        // Shown before its first tick: the play head is where it starts.
        show(&host);
        assert_eq!(host.shown().played, Some((0.3, 0.3)));
        let Ticked::At(playing) = tick(&mut host.strip.borrow_mut(), Some(0.5), None) else { panic!("it plays") };
        assert_eq!((playing.from, playing.at), (0.3, 0.5));
        show(&host);
        assert_eq!(host.shown().played, Some((0.3, 0.5)));
        // Zoomed in past where it started: that's out of sight, and what's in sight up to the play head is lit.
        host.strip.borrow_mut().sound.as_mut().unwrap().zoom = Some((0.4, 0.9));
        show(&host);
        let (from, at) = host.shown().played.expect("it plays on");
        assert!(from < 0.0 && (at - 0.2).abs() < 1e-6, "{from} {at}");
        host.strip.borrow_mut().sound.as_mut().unwrap().zoom = None;
        // O held: the original takes over, and the meter takes it up where it is.
        assert!(hear_original(&host, true));
        let played = host.strip.borrow().sound.as_ref().unwrap().played.expect("still playing");
        assert_eq!((played.from, played.last), (0.3, None));
        assert!(host.shown().original);
        // Stopped: nothing's lit at the next show.
        host.stop();
        show(&host);
        assert!(host.strip.borrow().sound.as_ref().unwrap().played.is_none());
        assert_eq!(host.shown().played, None);
        assert!(hear_original(&host, false));
        // O held with nothing playing: the original plays, from the start of the part picked.
        host.strip.borrow_mut().sound.as_mut().unwrap().pick = Some((0.25, 0.5));
        assert!(hear_original(&host, true));
        let played = host.strip.borrow().sound.as_ref().unwrap().played.expect("the original plays");
        assert_eq!((played.from, host.played_from.get()), (0.25, Some(0.25)));
    }

    #[test]
    fn a_swap_says_whether_it_is_the_original_held() {
        // A host that ends a play of the part itself, to the frame, lets the original play on past the part's end
        // while it's held; the edit swapped back stops there again, and so does the edit made again as it plays.
        let host = Fake::on(strip(1_000, vec![Edit::Gain { db: -6.0, span: None }]));
        seek(&host, 0.3);
        assert!(host.plays.get());
        assert!(hear_original(&host, true));
        assert!(hear_original(&host, false));
        assert_eq!(*host.swaps.borrow(), [true, false]);
        let again = {
            let s = host.strip.borrow();
            let o = s.sound.as_ref().unwrap();
            make(o.original.as_ref().unwrap(), o.outline.as_ref().unwrap(), o.edits.now.clone())
        };
        took_made(&host, again);
        assert_eq!(*host.swaps.borrow(), [true, false, false]);
    }

    #[test]
    fn the_original_of_a_reversed_edit_is_held_from_where_the_part_came_from() {
        // The whole sound reversed, nothing playing, O held: the original from its start, which is where the edit's
        // end came from (not from its last moment, where the edit's start came from and nothing would be heard).
        let host = Fake::on(strip(1_000, vec![Edit::Reverse(None)]));
        assert!(hear_original(&host, true));
        assert_eq!(host.played_from.get(), Some(0.0));
        // What it plays is the edit's end: the play head starts there and runs back.
        let played = host.strip.borrow().sound.as_ref().unwrap().played.expect("the original plays");
        assert_eq!(played.from, 0.999);
        assert!(hear_original(&host, false));
        // With a part picked: the stretch that part came from, 0.5 to 0.75 s of the original.
        host.strip.borrow_mut().sound.as_mut().unwrap().pick = Some((0.25, 0.5));
        assert!(hear_original(&host, true));
        assert_eq!(host.played_from.get(), Some(0.5));
        let played = host.strip.borrow().sound.as_ref().unwrap().played.expect("the original plays");
        assert_eq!(played.from, 0.499);
    }

    #[test]
    fn a_fades_handle_stays_while_it_is_dragged_out_of_sight() {
        // A fade in of 0.4 s, zoomed in on 0.2 to 0.9 s: its handle is in sight.
        let fade = Edit::Fade { span: Span::new(0, 400), way: Way::In, curve: CORNER };
        let host = Fake::on(strip(1_000, vec![fade]));
        host.strip.borrow_mut().sound.as_mut().unwrap().zoom = Some((0.2, 0.9));
        show(&host);
        assert!(host.shown().fade_in_shown && !host.shown().fade_out_shown);
        // Dragged to the screen's left edge, where a fade that short ends out of sight: the handle stays while it's
        // held (taken away, nothing would ever let it go), and is gone once it's let go there.
        fade_moving(&host, true, 0.0);
        assert!(host.shown().fade_in_shown);
        assert!(host.strip.borrow().sound.as_ref().unwrap().dragging.is_some());
        fade_let_go(&host, true);
        assert!(host.strip.borrow().sound.as_ref().unwrap().dragging.is_none());
        assert!(!host.shown().fade_in_shown);
    }

    #[test]
    fn the_room_laid_out_shows_the_strip_when_it_is_another() {
        let host = Fake::on(Strip { room: None, ..strip(1_000, Vec::new()) });
        show(&host);
        assert!(host.shown().wave.is_none(), "no room yet");
        laid_out(&host, room());
        assert_eq!(host.shown().wave.expect("a room").room, room());
        // The same room again: nothing to show.
        *host.shown.borrow_mut() = None;
        laid_out(&host, room());
        assert!(host.shown.borrow().is_none());
    }
}
