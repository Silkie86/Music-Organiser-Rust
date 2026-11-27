//! The Edit strip along the bottom of Sounds (the 1.1 plan's light sample edits, mock-up 8, Solomon's "Build as shown"
//! of 27 Nov 2026). E opens it on the picked sound, which is read whole (or, for a sound longer than
//! [`edit_copy::EDIT_MOST_SECONDS`], the part picked on its card) and edited by a list of edits ([`mo_audio::edit`]):
//! trim to a part picked on the strip's waveform, fades (its yellow corner handles, or the tools across a part),
//! reverse, normalise, gain and a loop's crossfade. Undo and redo step back and forth through the lists the edits
//! made. Play plays the edit (only the part picked, with one); holding O, or the A/B button, plays the original from
//! the same moment. The edit is dragged out, or saved (Ctrl+S), as a copy in the export folder's Edited sounds: the
//! file itself never changes (rule 2).
//!
//! The reading and the edit's making are done on a thread of their own, newest wins (rule 12); the edit's outline is
//! kept fine enough to draw any part of it at once, so zooming in waits for nothing. Each sound's edits are kept while
//! the app is open, so going to another sound and back finds them where they were.
//!
//! Its Slices cut the edit for a drum machine's pads ([`slices`], mock-up 9).

use std::cell::RefCell;
use std::fmt::Write as _;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::SystemTime;

use mo_app::background::{self, Newest};
use mo_app::edit_copy::{self, Source, Unread};
use mo_app::log;
use mo_app::shortcuts::Shortcuts;
use mo_audio::AudioData;
use mo_audio::edit::{self, Curve, Edit, Span, Way};
use mo_audio::hits::USUAL_RISE_DB;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{FEED, HERE, PAGE, drag_out, part, play_in_memory, player};
use crate::{AppWindow, EditChip, EditMark, SoundEdit, SoundsData};

mod slices;

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
pub(super) const EDIT: &str = "sounds.edit";
const UNDO: &str = "sounds.editUndo";
const REDO: &str = "sounds.editRedo";
const SAVE: &str = "sounds.editSave";
const ORIGINAL: &str = "sounds.editOriginal";
const TRIM: &str = "sounds.editTrim";
const FADE_IN: &str = "sounds.editFadeIn";
const FADE_OUT: &str = "sounds.editFadeOut";
const REVERSE: &str = "sounds.editReverse";
const NORMALISE: &str = "sounds.editNormalise";
const LOUDER: &str = "sounds.editLouder";
const QUIETER: &str = "sounds.editQuieter";
const LOOP: &str = "sounds.editLoop";

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
struct Made {
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

#[derive(Default)]
struct Strip {
    open: bool,
    sound: Option<Opened>,
    /// Each sound's edits and slicing, kept while the app is open, the newest last.
    kept: Vec<Kept>,
    /// The sounds found too long to edit whole on reading them, though their analysis didn't say so (it has no
    /// length yet): the part picked on their card is read instead.
    long: Vec<i64>,
    /// Counts the reads and makes asked for: only the newest's answer is taken.
    asked: u64,
    worker: Option<Newest<Job>>,
    /// The key that hears the original, as the shortcut list has it ("O").
    original_key: String,
    /// Slices are on their way to Bitwig or a folder: Send waits until they're there.
    sending: bool,
}

/// A read or a make, for the strip's thread.
struct Job {
    ask: u64,
    id: i64,
    work: Work,
    window: slint::Weak<AppWindow>,
}

enum Work {
    Read(Source),
    Make { original: Arc<AudioData>, edits: Vec<Edit> },
}

/// What came back from the strip's thread.
enum Answer {
    Read(Result<(Arc<AudioData>, Option<SystemTime>), Unread>),
    Made(Result<Made, String>),
}

thread_local! {
    static STRIP: RefCell<Strip> = RefCell::new(Strip::default());
}

pub(super) fn wire(app: &AppWindow) {
    let data = app.global::<SoundEdit>();
    let weak = app.as_weak();
    data.on_tool(move |id| {
        if let Some(app) = weak.upgrade() {
            run_action(&app, &id);
        }
    });
    let weak = app.as_weak();
    data.on_gain_stepped(move |by| {
        if let Some(app) = weak.upgrade() {
            gain(&app, if by < 0 { -GAIN_STEP_DB } else { GAIN_STEP_DB });
        }
    });
    let weak = app.as_weak();
    data.on_gain_reset(move || {
        if let Some(app) = weak.upgrade() {
            gain_to(&app, 0.0);
        }
    });
    let weak = app.as_weak();
    data.on_fade_moving(move |fade_in, at| {
        if let Some(app) = weak.upgrade() {
            fade_moving(&app, fade_in, f64::from(at));
        }
    });
    let weak = app.as_weak();
    data.on_fade_let_go(move |fade_in| {
        if let Some(app) = weak.upgrade() {
            fade_let_go(&app, fade_in);
        }
    });
    data.on_snap(snap);
    data.on_wave_pressed(|| {
        STRIP.with(|s| {
            if let Some(o) = s.borrow_mut().sound.as_mut() {
                o.pressed = true;
            }
        });
    });
    let weak = app.as_weak();
    data.on_picked(move |from, to| {
        if let Some(app) = weak.upgrade() {
            picked(&app, f64::from(from), f64::from(to));
        }
    });
    let weak = app.as_weak();
    data.on_seek(move |at| {
        if let Some(app) = weak.upgrade() {
            seek(&app, f64::from(at));
        }
    });
    let weak = app.as_weak();
    data.on_stop(move || {
        if let Some(app) = weak.upgrade()
            && PAGE.with(|p| p.borrow().play_edit)
        {
            super::stop(&app);
        }
    });
    let weak = app.as_weak();
    data.on_drag_edit(move || {
        if let Some(app) = weak.upgrade() {
            drag(&app, false);
        }
    });
    let weak = app.as_weak();
    data.on_drag_part(move || {
        if let Some(app) = weak.upgrade() {
            drag(&app, true);
        }
    });
    let weak = app.as_weak();
    data.on_drag_let_go(move || {
        if let Some(app) = weak.upgrade() {
            crate::drag::let_go(&app);
        }
    });
    let weak = app.as_weak();
    data.on_hear_original(move |on| {
        if let Some(app) = weak.upgrade() {
            hear_original(&app, on);
        }
    });
    let weak = app.as_weak();
    data.on_zoom(move |on| {
        if let Some(app) = weak.upgrade() {
            zoom(&app, on);
        }
    });
    let weak = app.as_weak();
    data.on_close(move || {
        if let Some(app) = weak.upgrade() {
            close(&app);
        }
    });
    slices::wire(app);
}

/// The strip's tips, with each tool's key from the shortcut list (rule 15).
pub(super) fn show_keys(app: &AppWindow, keys: &Shortcuts) {
    let data = app.global::<SoundEdit>();
    let key = |id: &str| SharedString::from(keys.key_of(id));
    data.set_edit_key(key(EDIT));
    data.set_undo_key(key(UNDO));
    data.set_redo_key(key(REDO));
    data.set_save_key(key(SAVE));
    data.set_original_key(key(ORIGINAL));
    data.set_trim_key(key(TRIM));
    data.set_fade_in_key(key(FADE_IN));
    data.set_fade_out_key(key(FADE_OUT));
    data.set_reverse_key(key(REVERSE));
    data.set_normalise_key(key(NORMALISE));
    data.set_louder_key(key(LOUDER));
    data.set_quieter_key(key(QUIETER));
    data.set_loop_key(key(LOOP));
    data.set_slices_key(key(slices::SLICES));
    data.set_bitwig_key(key(slices::TO_BITWIG));
    STRIP.with(|s| s.borrow_mut().original_key = keys.key_of(ORIGINAL).to_owned());
}

/// Runs one of the strip's actions (its key, its button or its line on the card's menu), if it's one of them: whether
/// it did anything (`None` for any other action). E opens and closes the strip while a sound is picked; the rest work
/// while it's open on a sound read and ready (the gain and fade keys too), so with it closed their keys go on.
/// The Slices' own ([`slices`]) are among them.
pub(super) fn run_action(app: &AppWindow, id: &str) -> Option<bool> {
    if id == EDIT {
        return Some(toggle(app));
    }
    if !id.starts_with("sounds.edit") {
        return None;
    }
    if !ready() {
        return Some(false);
    }
    // With the Slices on, the strip's tools are theirs: undo and redo step through the cuts, Save saves the slices,
    // and the edit's own tools wait until they're off (the cuts are of the edit as it is).
    let slicing = STRIP.with(|s| s.borrow().sound.as_ref().is_some_and(slices::on));
    Some(match id {
        slices::SLICES => slices::toggle(app),
        slices::TO_BITWIG => slices::send(app, slices::To::Bitwig),
        ORIGINAL => hear_original(app, true),
        UNDO if slicing => slices::step(app, true),
        REDO if slicing => slices::step(app, false),
        SAVE if slicing => slices::send(app, slices::To::Folder),
        _ if slicing => false,
        UNDO => step_history(app, true),
        REDO => step_history(app, false),
        SAVE => save(),
        TRIM => trim(app),
        FADE_IN => fade_tool(app, true),
        FADE_OUT => fade_tool(app, false),
        REVERSE => change(app, |o, _| {
            let span = o.pick.map(|p| span_of(o, p));
            push(&o.edits.now, Edit::Reverse(span))
        }),
        NORMALISE => change(app, |o, _| {
            // Twice in a row does nothing more.
            if o.edits.now.last() == Some(&Edit::Normalise { peak_db: 0.0 }) {
                None
            } else {
                push(&o.edits.now, Edit::Normalise { peak_db: 0.0 })
            }
        }),
        LOUDER => gain(app, GAIN_STEP_DB),
        QUIETER => gain(app, -GAIN_STEP_DB),
        LOOP => {
            let done = change(app, |o, audio| {
                let span = o.pick.map(|p| span_of(o, p));
                let frames = (LOOP_CROSS_SECONDS * f64::from(audio.sample_rate)).round() as usize;
                push(&o.edits.now, Edit::LoopCrossfade { span, frames })
            });
            if done {
                forget_place(app);
            }
            done
        }
        _ => return None,
    })
}

/// Whether the strip is open on a sound that's read and ready to edit.
fn ready() -> bool {
    STRIP.with(|s| {
        let s = s.borrow();
        s.open && s.sound.as_ref().is_some_and(|o| o.original.is_some())
    })
}

/// E (or the card's Edit): opens the strip on the picked sound, or closes it. Whether it did either (nothing with no
/// sound picked).
fn toggle(app: &AppWindow) -> bool {
    if STRIP.with(|s| s.borrow().open) {
        close(app);
        return true;
    }
    open(app)
}

/// Opens the strip on the picked sound (the right-click menu's Edit: the click picked it), or keeps it open on it.
/// Whether there was one to open it on.
pub(super) fn open(app: &AppWindow) -> bool {
    if PAGE.with(|p| p.borrow().picked.is_none()) {
        return false;
    }
    STRIP.with(|s| s.borrow_mut().open = true);
    app.global::<SoundEdit>().set_shown(true);
    load(app);
    true
}

/// Closes the strip. Its sound's edits are kept; what was read is let go.
pub(super) fn close(app: &AppWindow) {
    let was_open = STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let was = s.open;
        s.open = false;
        stash(&mut s);
        s.asked += 1;
        was
    });
    if !was_open {
        return;
    }
    if PAGE.with(|p| p.borrow().play_edit) {
        super::stop(app);
    }
    app.global::<SoundEdit>().set_shown(false);
    show(app);
}

/// Esc's steps on the page, before the card's: the slice picked goes, then the Slices (back to the edit's tools); the
/// part picked on the strip goes, then the strip. Whether it did one.
pub(super) fn escape(app: &AppWindow) -> bool {
    let (open, had_part) = STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let open = s.open;
        let had = s.sound.as_mut().is_some_and(|o| slices::escape(o) || o.pick.take().is_some());
        (open, had)
    });
    if !open {
        return false;
    }
    if had_part {
        show(app);
    } else {
        close(app);
    }
    true
}

/// Another sound was picked: the strip, if it's open, edits that one now (the last one's edits are kept).
pub(super) fn sound_picked(app: &AppWindow) {
    if STRIP.with(|s| s.borrow().open) {
        load(app);
    }
}

/// The part picked on the card changed: a sound too long to edit whole is edited a part at a time, so the strip
/// takes the new part (the last part's edits are kept, and come back with it).
pub(super) fn card_part_changed(app: &AppWindow) {
    let reload = STRIP.with(|s| {
        let s = s.borrow();
        s.open && s.sound.as_ref().is_some_and(|o| source_now().is_some_and(|(id, now)| id == o.id && now != o.source))
    });
    if reload {
        load(app);
    }
}

/// The picked sound's id and what the strip reads of it: the whole sound; or, for one longer than
/// [`edit_copy::EDIT_MOST_SECONDS`], the part picked on its card (none, and then there's nothing to read).
fn source_now() -> Option<(i64, Source)> {
    let (id, path, seconds) = PAGE.with(|p| {
        let p = p.borrow();
        let s = p.picked.and_then(|at| p.list.sound(at))?;
        Some((s.id, std::path::PathBuf::from(&s.path), s.analysis.as_ref().map_or(0.0, |a| a.seconds)))
    })?;
    let span = if is_long(id, seconds) { part::picked_seconds() } else { None };
    Some((id, Source { path, span }))
}

/// Whether the sound `id` (`seconds` long, as its analysis says, or 0 for not known) is too long to edit whole.
fn is_long(id: i64, seconds: f64) -> bool {
    seconds > edit_copy::EDIT_MOST_SECONDS || STRIP.with(|s| s.borrow().long.contains(&id))
}

/// Opens the picked sound in the strip: the one open already stays as it is; another is read (and the last one's
/// edits are kept).
fn load(app: &AppWindow) {
    let Some((id, source)) = source_now() else {
        close(app);
        return;
    };
    let (name, colour, bpm, seconds) = PAGE.with(|p| {
        let p = p.borrow();
        let s = p.picked.and_then(|at| p.list.sound(at));
        let name = s.map(|s| s.name().to_owned()).unwrap_or_default();
        let bpm = s.and_then(|s| s.analysis.as_ref()).and_then(|a| a.bpm).filter(|b| *b > 0.0);
        let seconds = s.and_then(|s| s.analysis.as_ref()).map_or(0.0, |a| a.seconds);
        (name, app.global::<SoundsData>().get_picked().colour, bpm, seconds)
    });
    let same = STRIP.with(|s| s.borrow().sound.as_ref().is_some_and(|o| o.id == id && o.source == source));
    if same {
        show(app);
        return;
    }
    // The last one's edit playing stops: what plays next is this one's.
    if PAGE.with(|p| p.borrow().play_edit) {
        super::stop(app);
    }
    // Too long, and no part picked on its card: nothing to read.
    let too_long = is_long(id, seconds) && source.span.is_none();
    let ask = STRIP.with(|s| {
        let mut s = s.borrow_mut();
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
    });
    show(app);
    if !too_long {
        ask_work(app, ask, id, Work::Read(source));
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

/// Hands `work` to the strip's thread (started at the first ask), or does it here for the screenshots, which have no
/// event loop to send it back with.
fn ask_work(app: &AppWindow, ask: u64, id: i64, work: Work) {
    if HERE.with(|h| *h.borrow()) {
        let answer = answer(work);
        arrived(app, ask, id, answer);
        return;
    }
    let job = Job { ask, id, work, window: app.as_weak() };
    STRIP.with(|s| {
        let mut s = s.borrow_mut();
        if s.worker.is_none() {
            match Newest::start("Edit sound", do_job) {
                Ok(worker) => s.worker = Some(worker),
                Err(e) => {
                    log::error("Sounds", &format!("Couldn't start the Edit strip's thread: {e}"));
                    return;
                }
            }
        }
        if let Some(worker) = &s.worker {
            worker.ask(job);
        }
    });
}

/// One job on the strip's thread.
fn do_job(job: Job) {
    let Job { ask, id, work, window } = job;
    background::lower_priority();
    let answer = answer(work);
    let _ = window.upgrade_in_event_loop(move |app| arrived(&app, ask, id, answer));
}

fn answer(work: Work) -> Answer {
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
fn arrived(app: &AppWindow, ask: u64, id: i64, answer: Answer) {
    let fresh = STRIP.with(|s| {
        let s = s.borrow();
        s.asked == ask && s.sound.as_ref().is_some_and(|o| o.id == id)
    });
    if !fresh {
        return;
    }
    match answer {
        Answer::Read(Ok((audio, changed))) => {
            // A part picked on the card of a sound read whole is where the strip starts, while it has no edits.
            let card_part = part::picked_seconds();
            STRIP.with(|s| {
                if let Some(o) = s.borrow_mut().sound.as_mut() {
                    if o.source.span.is_none() && o.edits.now.is_empty() {
                        o.pick = card_part.filter(|(from, to)| *to <= audio.seconds() + 1e-6 && from < to);
                    }
                    o.original = Some(audio);
                    o.changed = changed;
                    o.note.clear();
                }
            });
            make_again(app);
        }
        Answer::Read(Err(why)) => {
            let too_long = why == Unread::TooLong;
            STRIP.with(|s| {
                let mut s = s.borrow_mut();
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
            });
            if !too_long {
                log::warning("Sounds", &format!("The Edit strip couldn't open the sound: {why}"));
            } else if part::picked_seconds().is_some() {
                // A part is picked on its card already: that's read.
                load(app);
                return;
            }
        }
        Answer::Made(Ok(made)) => took_made(app, made),
        Answer::Made(Err(why)) => STRIP.with(|s| {
            if let Some(o) = s.borrow_mut().sound.as_mut() {
                o.note = why;
            }
        }),
    }
    show(app);
}

/// Asks for the edit to be made again from the edits as they are now, unless it's made of them already.
fn make_again(app: &AppWindow) {
    let job = STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let o = s.sound.as_ref()?;
        let original = o.original.clone()?;
        if o.made.as_ref().is_some_and(|m| m.edits == o.edits.now) {
            return None;
        }
        let (id, edits) = (o.id, o.edits.now.clone());
        s.asked += 1;
        Some((s.asked, id, Work::Make { original, edits }))
    });
    if let Some((ask, id, work)) = job {
        ask_work(app, ask, id, work);
    }
}

/// The edit was made: the strip shows it. A part picked or a zoom past its new end goes; the edit playing goes on
/// from the same moment in the new one when it's as long, and stops when it isn't (a trim or a loop changed where
/// everything is).
fn took_made(app: &AppWindow, made: Made) {
    let seconds = made.audio.seconds();
    let audio = made.audio.clone();
    let longer_or_shorter = STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let Some(o) = s.sound.as_mut() else { return false };
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
        changed
    });
    let (edit_plays, play, held) = PAGE.with(|p| {
        let p = p.borrow();
        (p.play_edit && p.playing.is_some(), p.play, STRIP.with(|s| s.borrow().sound.as_ref().and_then(|o| o.held)))
    });
    if !edit_plays || held.is_some() {
        return;
    }
    if longer_or_shorter {
        super::stop(app);
        return;
    }
    // Swapped in where the edit has got to, unless it has stopped or another sound has started meanwhile.
    let key = play.to_string();
    let _ = player().play_from(audio, key.clone(), |now| match now {
        Some((k, at)) if k == key => Ok(at),
        _ => Err(()),
    });
}

/// Applies `next` (what `edit` makes of the open sound's edits, given its original; none for no change) as the new
/// list of edits, kept for undo, and makes the edit again. Whether there was a change.
fn change(app: &AppWindow, edit: impl FnOnce(&Opened, &AudioData) -> Option<Vec<Edit>>) -> bool {
    let changed = STRIP.with(|s| {
        let mut s = s.borrow_mut();
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
    });
    if changed {
        make_again(app);
        show(app);
    }
    changed
}

/// `edits` with `edit` after them.
fn push(edits: &[Edit], edit: Edit) -> Option<Vec<Edit>> {
    let mut next = edits.to_vec();
    next.push(edit);
    Some(next)
}

/// Undo (`back`) or redo.
fn step_history(app: &AppWindow, back: bool) -> bool {
    let stepped = STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let Some(o) = s.sound.as_mut() else { return false };
        let before = o.edits.now.clone();
        let stepped = if back { o.edits.undo() } else { o.edits.redo() };
        if stepped && let Some(original) = o.original.clone() {
            forget_place_if_moved(o, &original, &before);
        }
        stepped
    });
    if stepped {
        make_again(app);
        show(app);
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
fn forget_place(app: &AppWindow) {
    STRIP.with(|s| {
        if let Some(o) = s.borrow_mut().sound.as_mut() {
            o.pick = None;
            o.zoom = None;
        }
    });
    show(app);
}

/// Keeps only the part picked.
fn trim(app: &AppWindow) -> bool {
    let done = change(app, |o, _| {
        let span = span_of(o, o.pick?);
        push(&o.edits.now, Edit::Trim(span))
    });
    if done {
        forget_place(app);
    }
    done
}

/// The fade-in tool (`true`) or the fade-out: across the part picked, or with none, a corner fade of the usual
/// length.
fn fade_tool(app: &AppWindow, fade_in: bool) -> bool {
    change(app, |o, original| {
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

/// A fade's handle dragged to `at` (0 to 1 across what's shown): its curve follows at once; the edit is made as it's
/// let go.
fn fade_moving(app: &AppWindow, fade_in: bool, at: f64) {
    STRIP.with(|s| {
        if let Some(o) = s.borrow_mut().sound.as_mut() {
            let (from, to) = view(o);
            o.dragging = Some((fade_in, from + at.clamp(0.0, 1.0) * (to - from)));
        }
    });
    show(app);
}

fn fade_let_go(app: &AppWindow, fade_in: bool) {
    let dragged = STRIP.with(|s| s.borrow_mut().sound.as_mut().and_then(|o| o.dragging.take()));
    let Some((which, at)) = dragged.filter(|(which, _)| *which == fade_in) else {
        show(app);
        return;
    };
    let changed = change(app, |o, original| {
        let seconds = edit_seconds(o, original);
        let long = if which { at } else { seconds - at };
        Some(with_corner(o, original, which, long.max(0.0)))
    });
    if !changed {
        show(app);
    }
}

/// Turns the part picked (the whole sound with none) up or down by `db` more.
fn gain(app: &AppWindow, db: f32) -> bool {
    let now = STRIP.with(|s| s.borrow().sound.as_ref().map_or(0.0, gain_now));
    gain_to(app, ((now + db) * 2.0).round() / 2.0)
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
fn gain_to(app: &AppWindow, db: f32) -> bool {
    change(app, |o, _| {
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
fn zoom(app: &AppWindow, on: bool) {
    STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let Some(o) = s.sound.as_mut() else { return };
        o.zoom = None;
        if on && let Some((from, to)) = part_now(o) {
            let (_, end) = view(o);
            let room = ((to - from) * 0.1).max(0.01);
            o.zoom = Some(((from - room).max(0.0), (to + room).min(end)));
        }
    });
    show(app);
}

/// A part picked on the strip's waveform, from and to 0 to 1 across what's shown; let go when it's too short to be one
/// (a click outside it).
fn picked(app: &AppWindow, from: f64, to: f64) {
    STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let Some(o) = s.sound.as_mut() else { return };
        // Pressed on another sound's waveform (the arrow keys picked this one while the button was down).
        if !std::mem::take(&mut o.pressed) {
            return;
        }
        let (start, end) = view(o);
        let place = |f: f64| start + f.clamp(0.0, 1.0) * (end - start);
        let (from, to) = (place(from.min(to)), place(from.max(to)));
        o.pick = (to - from >= edit_copy::LEAST_PART_SECONDS).then_some((from, to));
    });
    show(app);
}

/// Where a part's edge goes with the mouse at `at` (0 to 1 across what's shown): on the nearest hit no further than
/// `within`, else at `at`.
fn snap(at: f32, within: f32) -> f32 {
    STRIP.with(|s| {
        let s = s.borrow();
        let Some(o) = s.sound.as_ref() else { return at };
        let Some(made) = o.made.as_ref() else { return at };
        let (from, to) = view(o);
        let long = (to - from).max(1e-9);
        let hits: Vec<f64> = made.hits.iter().map(|h| (h - from) / long).collect();
        part::snap_to(&hits, f64::from(at), f64::from(within)) as f32
    })
}

/// A click on the strip's waveform, `at` (0 to 1 across what's shown): the edit plays from there, to the end of the
/// part picked when it's inside it, else to its end.
fn seek(app: &AppWindow, at: f64) {
    let Some((from, pick)) = STRIP.with(|s| {
        let s = s.borrow();
        let o = s.sound.as_ref()?;
        let (start, end) = view(o);
        Some((start + at.clamp(0.0, 1.0) * (end - start), o.pick))
    }) else {
        return;
    };
    let till = pick.filter(|(a, b)| from >= *a && from < *b).map(|(_, b)| b);
    play_edit(app, from, till);
}

/// Play on the page with the strip open on the picked sound: the edit plays (the part picked, or the slice, with
/// one), or stops. Whether it was the strip's to do.
pub(super) fn toggle_play(app: &AppWindow) -> bool {
    let picked_id = PAGE.with(|p| {
        let p = p.borrow();
        p.picked.and_then(|at| p.list.sound(at)).map(|s| s.id)
    });
    let Some(pick) = STRIP.with(|s| {
        let s = s.borrow();
        s.sound.as_ref().filter(|o| s.open && o.made.is_some() && Some(o.id) == picked_id).map(part_now)
    }) else {
        return false;
    };
    // The picked sound playing (its edit, or itself from the card) stops; else the edit plays, a play you meant.
    let (playing_picked, at) = PAGE.with(|p| {
        let p = p.borrow();
        (p.playing.is_some_and(|(j, _)| Some(j) == p.picked), p.picked)
    });
    if playing_picked {
        super::stop(app);
    } else {
        play_edit(app, pick.map_or(0.0, |(from, _)| from), pick.map(|(_, to)| to));
        if let Some(at) = at {
            super::did::played(app, at);
        }
    }
    true
}

/// Plays the edit from `from` seconds, as far as `till`.
fn play_edit(app: &AppWindow, from: f64, till: Option<f64>) {
    let Some(audio) = STRIP.with(|s| s.borrow().sound.as_ref().and_then(|o| o.made.as_ref().map(|m| m.audio.clone())))
    else {
        return;
    };
    let Some(at) = PAGE.with(|p| p.borrow().picked) else { return };
    STRIP.with(|s| {
        if let Some(o) = s.borrow_mut().sound.as_mut() {
            o.held = None;
        }
    });
    play_in_memory(app, at, audio, from, till);
}

/// O held (`on`) or let go, or the A/B button: the original plays from the moment the edit has got to, and the edit
/// comes back from the same moment as it's let go. With nothing playing, the original plays while it's held (from the
/// start of the part picked, or of the slice picked with the Slices on, if there is one). Whether it did anything.
fn hear_original(app: &AppWindow, on: bool) -> bool {
    let Some((original, edited, edits, pick, held)) = STRIP.with(|s| {
        let s = s.borrow();
        let o = s.sound.as_ref().filter(|_| s.open)?;
        let made = o.made.as_ref()?;
        Some((o.original.clone()?, made.audio.clone(), made.edits.clone(), part_now(o), o.held))
    }) else {
        return false;
    };
    let rate = f64::from(original.sample_rate.max(1));
    let frames = original.len();
    let before = |seconds: f64| edit::place_before(frames, &edits, (seconds * rate).round() as usize) as f64 / rate;
    let after = |seconds: f64| edit::place_after(frames, &edits, (seconds * rate).round() as usize) as f64 / rate;
    let (plays, play) = PAGE.with(|p| {
        let p = p.borrow();
        (p.play_edit && p.playing.is_some(), p.play)
    });
    let key = play.to_string();
    let set_held = |held: Option<bool>| {
        STRIP.with(|s| {
            if let Some(o) = s.borrow_mut().sound.as_mut() {
                o.held = held;
            }
        });
    };
    if on {
        // Held already (the key repeats).
        if held.is_some() {
            return true;
        }
        let took_over = plays
            && player()
                .play_from(original.clone(), key.clone(), |now| match now {
                    Some((k, at)) if k == key => Ok(before(at)),
                    _ => Err(()),
                })
                .is_ok();
        if took_over {
            set_held(Some(false));
        } else if let Some(at) = PAGE.with(|p| p.borrow().picked) {
            play_in_memory(app, at, original, before(pick.map_or(0.0, |(from, _)| from)), None);
            set_held(Some(true));
        }
    } else {
        match held {
            None => return false,
            // Started by the hold: it stops with it.
            Some(true) => {
                set_held(None);
                if plays {
                    super::stop(app);
                }
            }
            Some(false) => {
                set_held(None);
                if plays {
                    let _ = player().play_from(edited, key.clone(), |now| match now {
                        Some((k, at)) if k == key => Ok(after(at)),
                        _ => Err(()),
                    });
                }
            }
        }
    }
    show(app);
    true
}

/// A key let go: O stops the original.
pub(super) fn key_up(app: &AppWindow, text: &str) {
    let key = STRIP.with(|s| s.borrow().original_key.clone());
    let name = crate::key_name(text, false, false, false, false);
    // Its last key: with Ctrl or Shift in its shortcut, those may be let go first.
    if !name.is_empty() && key.rsplit('+').next() == Some(name.as_str()) {
        hear_original(app, false);
    }
}

/// The page's ticker, while the strip's play plays: `at` is where the player is (seconds into what it plays; none
/// until it has the play). The strip's waveform shows where the edit has got to. Whether it goes on: a play of the part
/// picked stops at its end.
pub(super) fn tick(app: &AppWindow, at: Option<f64>) -> bool {
    let Some(at) = at else { return true };
    let till = PAGE.with(|p| p.borrow().play_part.map(|(_, end)| end));
    let shown = STRIP.with(|s| {
        let s = s.borrow();
        let o = s.sound.as_ref()?;
        let made = o.made.as_ref()?;
        let original = o.original.as_ref()?;
        // Hearing the original: where that is in the edit.
        let at = if o.held.is_some() {
            let rate = f64::from(original.sample_rate.max(1));
            edit::place_after(original.len(), &made.edits, (at * rate).round() as usize) as f64 / rate
        } else {
            at
        };
        let (from, to) = view(o);
        Some((at, ((at - from) / (to - from).max(1e-9)).clamp(0.0, 1.0), o.held.is_some()))
    });
    let Some((at, place, held)) = shown else { return true };
    if !held && till.is_some_and(|till| at >= till) {
        return false;
    }
    app.global::<SoundEdit>().set_played(place as f32);
    true
}

/// The strip's play started (`on`) or stopped.
pub(super) fn show_playing(app: &AppWindow, on: bool) {
    let data = app.global::<SoundEdit>();
    data.set_playing(on);
    if !on {
        data.set_played(0.0);
    }
}

/// Saves the edit as a copy in Edited sounds, on a thread of its own; a card says how it went.
fn save() -> bool {
    let Some((source, original, changed, edits, name)) = STRIP.with(|s| {
        let s = s.borrow();
        let o = s.sound.as_ref()?;
        Some((o.source.clone(), o.original.clone()?, o.changed, o.edits.now.clone(), o.name.clone()))
    }) else {
        return false;
    };
    if edits.is_empty() {
        return false;
    }
    let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return false };
    let job = feed.start(&format!("Saving the edit of {name}"), "");
    if HERE.with(|h| *h.borrow()) {
        return true;
    }
    let made = background::spawn("Save edit", move || {
        background::lower_priority();
        let saved =
            mo_catch::catch_crash(AssertUnwindSafe(|| edit_copy::edit_to_keep(&source, &original, changed, &edits)))
                .unwrap_or_else(|_| Err("Something went wrong: the log says what.".to_owned()));
        match saved {
            Ok(copy) => {
                let file = copy.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                log::info("Sounds", &format!("Saved the edit of {name} as {}", copy.display()));
                job.done_as(&format!("Saved the edit of {name}"), &format!("{file}, in {}", edit_copy::FOLDER));
            }
            Err(why) => {
                log::warning("Sounds", &format!("Couldn't save the edit of {name}: {why}"));
                job.failed(&why);
            }
        }
    });
    if let Err(e) = made {
        log::error("Sounds", &format!("Couldn't start saving the edit: {e}"));
    }
    true
}

/// Drags the edit out of the app (its part, for `part`: a press inside the part picked on the strip that moved), as a
/// copy in Edited sounds. With no edits (and no part), the sound itself goes, as from the card.
fn drag(app: &AppWindow, part: bool) {
    let pick = STRIP.with(|s| s.borrow().sound.as_ref().and_then(|o| o.pick));
    if part && pick.is_none() {
        return;
    }
    carry(app, pick.filter(|_| part));
}

/// Drags the edit out of the app, or only `part` of it (from and to, seconds into the edit: the part picked, or a
/// slice), as a copy in Edited sounds. With no edits and no part, the sound itself goes, as from the card.
fn carry(app: &AppWindow, part: Option<(f64, f64)>) {
    let Some(at) = PAGE.with(|p| p.borrow().picked) else { return };
    let Some((source, original, changed, mut edits)) = STRIP.with(|s| {
        let s = s.borrow();
        let o = s.sound.as_ref()?;
        Some((o.source.clone(), o.original.clone()?, o.changed, o.edits.now.clone()))
    }) else {
        return;
    };
    if let Some(p) = part {
        let rate = f64::from(original.sample_rate.max(1));
        edits.push(Edit::Trim(Span::new((p.0 * rate).round() as usize, (p.1 * rate).round() as usize)));
    }
    let what = if !edits.is_empty() {
        drag_out::Carried::Edit { source, audio: original, changed, edits }
    } else if let Some((from, to)) = source.span {
        drag_out::Carried::Part(from, to)
    } else {
        drag_out::Carried::Whole
    };
    drag_out::carry_edit(app, at, what);
}

/// Shows the strip as it is now.
fn show(app: &AppWindow) {
    let data = app.global::<SoundEdit>();
    STRIP.with(|s| {
        let s = s.borrow();
        data.set_folder_text(folder_text().into());
        let Some(o) = s.sound.as_ref() else {
            data.set_ready(false);
            data.set_name(SharedString::new());
            data.set_note(SharedString::new());
            data.set_shape(SharedString::new());
            data.set_chips(ModelRc::default());
            data.set_ruler(ModelRc::default());
            data.set_can_undo(false);
            data.set_can_redo(false);
            data.set_has_edits(false);
            data.set_has_part(false);
            slices::show_none(&data);
            return;
        };
        // With the Slices on, undo and redo are the cuts', and the chips name the pads.
        let slicing = slices::on(o);
        let (can_undo, can_redo) =
            if slicing { slices::steps(o) } else { (!o.edits.undo.is_empty(), !o.edits.redo.is_empty()) };
        data.set_name(o.name.as_str().into());
        data.set_colour(o.colour);
        data.set_note(o.note.as_str().into());
        data.set_can_undo(can_undo);
        data.set_can_redo(can_redo);
        data.set_has_edits(!o.edits.now.is_empty());
        data.set_chips(ModelRc::new(VecModel::from(if slicing { slices::pad_chips(o) } else { chips(o) })));
        slices::show(&data, o, s.sending);
        data.set_original(o.held.is_some());
        let gain = gain_now(o);
        data.set_gain_text(gain_text(gain).into());
        data.set_gain_changed(gain != 0.0);
        data.set_drag_text(if o.edits.now.is_empty() { "Drag the sound" } else { "Drag the edit" }.into());
        let (Some(original), Some(made)) = (o.original.as_ref(), o.made.as_ref()) else {
            data.set_ready(false);
            data.set_has_part(false);
            data.set_picked_text(SharedString::new());
            return;
        };
        data.set_ready(o.note.is_empty());
        let seconds = made.audio.seconds();
        let (from, to) = view(o);
        let long = (to - from).max(1e-9);
        let across = |t: f64| ((t - from) / long) as f32;
        // The waveform over what's shown, at full scale (so a gain shows), its lines the loudest of their share.
        let first = ((from / made.step).floor() as usize).min(made.steps.len());
        let past = ((to / made.step).ceil() as usize).clamp(first, made.steps.len());
        let lines = super::whole_wave::lines(&made.steps[first..past], POINTS);
        data.set_shape(wave_path(&lines).into());
        data.set_points(lines.len() as i32);
        data.set_view_seconds(long as f32);
        data.set_view_from(from as f32);
        data.set_zoomed(o.zoom.is_some());
        let hits: Vec<f64> = made.hits.iter().filter(|h| **h >= from && **h <= to).map(|h| (h - from) / long).collect();
        data.set_hit_marks(part::marks(&hits).into());
        match o.pick {
            // The Slices have their own: how many, and the one picked (which the zoom goes to).
            _ if slicing => {
                data.set_has_part(slices::picked(o).is_some());
                data.set_pick_from(0.0);
                data.set_pick_to(0.0);
                data.set_picked_text(slices::heading(o, seconds).into());
            }
            Some((a, b)) => {
                data.set_has_part(true);
                data.set_pick_from(across(a));
                data.set_pick_to(across(b));
                data.set_picked_text(picked_text(b - a, o.bpm).into());
            }
            None => {
                data.set_has_part(false);
                data.set_pick_from(0.0);
                data.set_pick_to(0.0);
                data.set_picked_text(part::length_text(seconds).into());
            }
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
        data.set_fade_in(across(fade_in));
        data.set_fade_out(across(fade_out));
        data.set_fade_in_shown(from <= 1e-9 || fade_in > from);
        data.set_fade_out_shown(to >= seconds - 1e-9 || fade_out < to);
        data.set_fade_in_curve(fade_curve(across(0.0), across(fade_in), true).into());
        data.set_fade_out_curve(fade_curve(across(fade_out), across(seconds), false).into());
        data.set_ruler(ModelRc::new(VecModel::from(ruler(from, to, o.bpm))));
    });
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
    let mut text = format!("{} picked", part::length_text(seconds));
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
fn chips(o: &Opened) -> Vec<EditChip> {
    let rate = f64::from(o.original.as_ref().map_or(1, |a| a.sample_rate.max(1)));
    let count = o.edits.now.len();
    o.edits
        .now
        .iter()
        .enumerate()
        .map(|(i, e)| EditChip { text: chip_text(e, rate).into(), last: i + 1 == count })
        .collect()
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
    if seconds < 1.0 { format!("{} ms", (seconds * 1000.0).round()) } else { part::length_text(seconds) }
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
fn ruler(from: f64, to: f64, bpm: Option<f64>) -> Vec<EditMark> {
    let long = to - from;
    if long <= 0.0 {
        return Vec::new();
    }
    let mut marks = Vec::new();
    let mut mark = |t: f64, text: String, bar: bool| {
        marks.push(EditMark { at: ((t - from) / long) as f32, text: text.into(), bar });
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
/// it), all at once.
pub fn for_screens(app: &AppWindow, audio: AudioData, part: Option<(f64, f64)>, tools: &[&str]) {
    if !STRIP.with(|s| s.borrow().open) {
        toggle(app);
    }
    let (ask, id) = STRIP.with(|s| {
        let s = s.borrow();
        (s.asked, s.sound.as_ref().map_or(0, |o| o.id))
    });
    arrived(app, ask, id, Answer::Read(Ok((Arc::new(audio), None))));
    for id in tools {
        run_action(app, id);
    }
    if let Some((a, b)) = part {
        STRIP.with(|s| {
            if let Some(o) = s.borrow_mut().sound.as_mut() {
                let seconds = o.made.as_ref().map_or(0.0, |m| m.audio.seconds());
                o.pick = Some((a * seconds, b * seconds));
            }
        });
    }
    show(app);
}

pub use slices::state_for_screens as slices_state_for_screens;

/// For the screenshots: whether the strip is open, ready, and how many edits it has.
pub fn state_for_screens() -> (bool, bool, usize) {
    STRIP.with(|s| {
        let s = s.borrow();
        let ready = s.sound.as_ref().is_some_and(|o| o.made.is_some());
        (s.open, ready, s.sound.as_ref().map_or(0, |o| o.edits.now.len()))
    })
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
