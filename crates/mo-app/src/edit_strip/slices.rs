//! The Edit strip's Slices with no screen (slice to pads: the 1.1 plan's light sample edits, mock-up 9, Solomon's
//! "Build as shown" of 27 Nov 2026). The scissors (Alt+S) cut the edit at its hits, as big as the Sensitivity asks, or
//! on the beats of a sound whose tempo is known: a line at each cut, the slices numbered from 1. A line is dragged to
//! move it (it snaps to the hits; Shift for none); a double-click takes a line away, or adds one where there's none.
//! A click in a slice plays it, and a press in one that moves carries that slice out of the app. Send hands the
//! slices to the strip's host ([`Host::send`]), which puts each in a file of its own in a folder for this slicing in
//! the export folder's Edited sounds ([`edit_copy::slices_to_keep`]) and then on a Drum Machine in Bitwig, one to a
//! pad from C1 (the Kits' way in), or just opens their folder.
//!
//! While the Slices are on, undo and redo step through the cuts, and the edit's own tools wait: the cuts are always of
//! the edit as it is. Each sound's slicing is kept with its edits, so going to another sound and back finds it.

use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hasher as _};
use std::sync::Arc;
use std::time::SystemTime;

use mo_audio::AudioData;
use mo_audio::edit::{Edit, Span};
use mo_audio::hits;

use super::{Chip, History, Host, Made, Opened, Strip, carry, play_edit, show, stop, view};
use crate::edit_copy::{self, LEAST_SLICE_SECONDS, MOST_SLICES, Source};
use crate::{sound_part, to_bitwig};

/// The ids of the Slices' actions in the shortcut list. Saving them in their folder is the strip's own Save.
pub const SLICES: &str = "sounds.editSlices";
pub const TO_BITWIG: &str = "sounds.editSlicesBitwig";

/// How many pads the strip's foot names before it skips to the last.
const PADS_NAMED: usize = 8;

/// How much further off than a press takes hold of a line a hit pulls a line to it: 8 px to the press's 5, as a picked
/// part's edges.
const SNAP_OVER_HOLD: f64 = 1.6;

/// What the menus call the choices: the cuts at the hits ("Hits (20)", with how many), or on the beats, so many
/// apart; and how far each slice runs.
const HITS: &str = "Hits";
const ON_BEATS: [(&str, f64); 4] =
    [("Every bar", 4.0), ("Every beat", 1.0), ("Every half beat", 0.5), ("Every quarter beat", 0.25)];
const TO_NEXT_HIT: &str = "to its next hit";
const TO_NEXT_CUT: &str = "to the next cut";
const TO_END: &str = "to the sound's end";

/// What the Send button says until there are slices to count.
const SEND: &str = "Send the slices";

/// What the Send menu's folder line says until there are slices to count.
const TO_FOLDER: &str = "A numbered folder";

/// What the cuts go by.
#[derive(Clone, Copy, Debug, PartialEq)]
enum CutAt {
    /// The sound's hits, those as big as the sensitivity asks.
    Hits,
    /// The beats of a sound whose tempo is known, this many apart (4 is a bar).
    Beats(f64),
}

/// How a sound is sliced: the choices, and where the cuts are (the choices put them there; a line moved, added or
/// taken away by hand changes them).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Slicing {
    cut_at: CutAt,
    /// How small a hit gets a cut, from 0 (only the biggest) to 1 (the smallest too): [`hits::rise_for`].
    sensitivity: f32,
    /// Each slice runs on to the sound's end, not just to the next cut.
    to_end: bool,
    /// Where each slice starts, in seconds into the edit, in order.
    cuts: Vec<f64>,
}

impl Default for Slicing {
    fn default() -> Self {
        Slicing { cut_at: CutAt::Hits, sensitivity: 0.5, to_end: false, cuts: Vec::new() }
    }
}

impl Slicing {
    /// Whether its choices are the ones a sound starts with (wherever its cuts are).
    fn as_usual(&self) -> bool {
        let usual = Slicing::default();
        (self.cut_at, self.sensitivity, self.to_end) == (usual.cut_at, usual.sensitivity, usual.to_end)
    }

    /// The slice `i` (from 0) of a sound `seconds` long: from and to, in seconds.
    fn slice(&self, i: usize, seconds: f64) -> Option<(f64, f64)> {
        let from = *self.cuts.get(i)?;
        let till = if self.to_end { seconds } else { self.cuts.get(i + 1).copied().unwrap_or(seconds) };
        (till > from).then_some((from, till))
    }

    /// The slice the moment `at` is in: the one that starts at the last cut at or before it (none before the first
    /// cut).
    fn slice_at(&self, at: f64, seconds: f64) -> Option<usize> {
        if at >= seconds {
            return None;
        }
        self.cuts.iter().rposition(|c| *c <= at)
    }

    /// The cut nearest `at`, if it's no further off than `within` (seconds).
    fn cut_near(&self, at: f64, within: f64) -> Option<usize> {
        self.cuts
            .iter()
            .enumerate()
            .filter(|(_, c)| (**c - at).abs() <= within)
            .min_by(|a, b| (a.1 - at).abs().total_cmp(&(b.1 - at).abs()))
            .map(|(i, _)| i)
    }

    /// With the cut `cut` moved to `to`, or as near it as the cuts either side (and the sound's start and end) leave
    /// room for a slice; none when it doesn't move.
    fn moved(&self, cut: usize, to: f64, seconds: f64) -> Option<Slicing> {
        let now = *self.cuts.get(cut)?;
        let low = if cut == 0 { 0.0 } else { self.cuts[cut - 1] + LEAST_SLICE_SECONDS };
        let high = self.cuts.get(cut + 1).copied().unwrap_or(seconds) - LEAST_SLICE_SECONDS;
        if low > high || !to.is_finite() {
            return None;
        }
        let to = to.clamp(low, high);
        ((to - now).abs() > 1e-9).then(|| {
            let mut next = self.clone();
            next.cuts[cut] = to;
            next
        })
    }

    /// With a cut more at `at`; none when there's no room for one there (too near another or the end, or there are
    /// [`MOST_SLICES`] already).
    fn with_cut(&self, at: f64, seconds: f64) -> Option<Slicing> {
        let room = at.is_finite()
            && at >= 0.0
            && at <= seconds - LEAST_SLICE_SECONDS
            && self.cuts.len() < MOST_SLICES
            && self.cuts.iter().all(|c| (c - at).abs() >= LEAST_SLICE_SECONDS);
        room.then(|| {
            let mut next = self.clone();
            let before = next.cuts.partition_point(|c| *c < at);
            next.cuts.insert(before, at);
            next
        })
    }

    /// With the cut `cut` taken away; none for the last one left (one slice always stays).
    fn without_cut(&self, cut: usize) -> Option<Slicing> {
        (self.cuts.len() > 1 && cut < self.cuts.len()).then(|| {
            let mut next = self.clone();
            next.cuts.remove(cut);
            next
        })
    }
}

/// A sound's Slices: on or off, and how it's sliced.
#[derive(Default)]
pub(super) struct Sliced {
    pub(super) on: bool,
    /// The edits of the edit the cuts are in: with others, it's sliced afresh. None until it's first sliced.
    of: Option<Vec<Edit>>,
    pub(super) history: History<Slicing>,
}

impl Sliced {
    /// Whether there's anything of it to keep when the strip goes to another sound: the Slices on, a change to
    /// undo or redo, or choices of his own (an edit made since then cut the sound afresh by them, with nothing to
    /// undo: they're still his).
    pub(super) fn worth_keeping(&self) -> bool {
        self.on || self.history.has_steps() || !self.history.now.as_usual()
    }
}

/// A press on the Slices' waveform, while the button is down.
pub(super) struct Press {
    /// Where it was, and how near a line counts as on it (seconds into the edit).
    at: f64,
    within: f64,
    shift: bool,
    on: On,
}

/// What a press is on, and what has come of it.
#[derive(Clone, Copy)]
enum On {
    /// A line (by its cut): `moved` once the mouse has gone far enough to be dragging it.
    Line { cut: usize, moved: bool },
    /// A slice: `carried` once the mouse has gone far enough to carry it out of the app.
    Slice { slice: usize, carried: bool },
    /// Before the first cut; or a double-click, which has done its work.
    Nothing,
}

/// Where slices go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum To {
    /// On a Drum Machine in Bitwig, one to a pad from C1.
    Bitwig,
    /// Just their folder, numbered.
    Folder,
}

/// Slices for the strip's host to send ([`Host::send`]): what [`edit_copy::slices_to_keep`] makes their files from.
pub struct Slices {
    /// The sound they're of, by its id in the host's list.
    pub id: i64,
    /// What the strip read of it, and when its file was last changed as it was read.
    pub source: Source,
    pub changed: Option<SystemTime>,
    /// The edit they're cut from, as it sounds.
    pub audio: Arc<AudioData>,
    /// What that edit is of what was read, in words that are others for other edits: with the cuts, it names the
    /// slices' folder.
    pub what: String,
    /// Each slice, in frames of `audio`.
    pub spans: Vec<Span>,
    /// The sound's name, for the words about them.
    pub name: String,
}

/// A cut on the Slices' waveform: where (0 to 1 across the part on show), the number of the slice it starts, and how
/// much room there is to the next line (0 to 1 across), for its number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub at: f32,
    pub number: usize,
    pub room: f32,
}

/// The Slices as the strip shows them.
#[derive(Clone, Debug, PartialEq)]
pub struct Showing {
    /// On: the edit's tools give way to the slicing's.
    pub on: bool,
    /// The cuts on show, the slice picked (0 to 1 across; none unless `slice_to` is past `slice_from`), and where the
    /// first cut is (what's before it is in no slice).
    pub cuts: Vec<Cut>,
    pub slice_from: f32,
    pub slice_to: f32,
    pub first_cut: f32,
    /// What the cuts go by ("Hits (20)", "Every beat"…) and the choices; whether it's the hits (the sensitivity then
    /// counts); how big a hit must be, 0 (only the biggest) to 1 (the smallest too); and how far each slice runs.
    pub cut_at: String,
    pub cut_at_choices: Vec<String>,
    pub cut_at_hits: bool,
    pub sensitivity: f32,
    pub each: String,
    pub each_choices: Vec<String>,
    /// The Send button's words ("Send 20 slices") and whether there's anything to send (and none on their way); each
    /// slice's pad, by its note's name as Bitwig has it ("C1", "C#1"…), for its menu; what the menu's folder line says
    /// ("A numbered folder, 01 … 20"), and where the slices are saved.
    pub send_text: String,
    pub can_send: bool,
    pub pads: Vec<String>,
    pub to_folder_text: String,
    pub saved_in: String,
    /// A number that's another whenever the lines the mouse can take hold of are others (where the cuts are, and the
    /// stretch of the sound that's shown): what [`near`] says is asked again then, though the mouse hasn't moved.
    pub cuts_mark: i32,
}

impl Default for Showing {
    /// The Slices off.
    fn default() -> Self {
        let usual = Slicing::default();
        Showing {
            on: false,
            cuts: Vec::new(),
            slice_from: 0.0,
            slice_to: 0.0,
            first_cut: 0.0,
            cut_at: String::new(),
            cut_at_choices: Vec::new(),
            cut_at_hits: usual.cut_at == CutAt::Hits,
            sensitivity: usual.sensitivity,
            each: String::new(),
            each_choices: Vec::new(),
            send_text: SEND.to_owned(),
            can_send: false,
            pads: Vec::new(),
            to_folder_text: TO_FOLDER.to_owned(),
            saved_in: String::new(),
            cuts_mark: 0,
        }
    }
}

/// Where the cuts of the edit `made` go by `cut_at` (the hits, as small as `sensitivity` takes; or the beats at
/// `bpm`, which fall back on the hits with no tempo known).
fn cuts_for(made: &Made, bpm: Option<f64>, cut_at: CutAt, sensitivity: f32) -> Vec<f64> {
    let seconds = made.audio.seconds();
    match (cut_at, bpm) {
        (CutAt::Beats(beats), Some(bpm)) => edit_copy::cuts_at_beats(seconds, bpm, beats),
        _ => edit_copy::cuts_at_hits(&made.all_hits, hits::rise_for(sensitivity), seconds),
    }
}

/// Whether the sound's Slices are on.
pub(super) fn on(o: &Opened) -> bool {
    o.sliced.as_ref().is_some_and(|s| s.on)
}

/// The slicing on show: with the Slices on, and the cuts in the edit as it's made now. None while an edit made with
/// the Slices off is still being made: the edit on show then is the one before it, and so are its cuts.
fn now(o: &Opened) -> Option<(&Slicing, &Made)> {
    let sliced = o.sliced.as_ref().filter(|s| s.on)?;
    let made = o.made.as_ref().filter(|m| m.edits == o.edits.now && sliced.of.as_ref() == Some(&m.edits))?;
    Some((&sliced.history.now, made))
}

/// The slice picked, from and to in seconds into the edit.
pub(super) fn picked(o: &Opened) -> Option<(f64, f64)> {
    let (slicing, made) = now(o)?;
    slicing.slice(o.slice?, made.audio.seconds())
}

/// Whether there's a change to the cuts to undo, and one to redo.
pub(super) fn steps(o: &Opened) -> (bool, bool) {
    o.sliced.as_ref().map_or((false, false), |s| (!s.history.undo.is_empty(), !s.history.redo.is_empty()))
}

/// The edit was made (or the Slices turned on): with the Slices on, it's sliced, by the choices there were, unless
/// its cuts are in this same edit already. An edit still being made of the edits as they are now waits for that.
pub(super) fn edit_made(o: &mut Opened) {
    let Some(made) = o.made.as_ref() else { return };
    let Some(sliced) = o.sliced.as_mut().filter(|s| s.on) else { return };
    if made.edits != o.edits.now {
        return;
    }
    let seconds = made.audio.seconds();
    let fits = sliced.history.now.cuts.iter().all(|c| *c < seconds);
    if sliced.of.as_ref() == Some(&made.edits) && fits {
        return;
    }
    let was = &sliced.history.now;
    let now = Slicing { cuts: cuts_for(made, o.bpm, was.cut_at, was.sensitivity), ..was.clone() };
    sliced.history = History { now, ..History::default() };
    sliced.of = Some(made.edits.clone());
    o.slice = None;
    o.slice_press = None;
}

/// The scissors: turns the sound's Slices on, or off again (its cuts wait until they're back on). Whether it did.
pub(super) fn toggle(host: &dyn Host) -> bool {
    let done = host.strip().borrow_mut().sound.as_mut().is_some_and(|o| {
        let sliced = o.sliced.get_or_insert_with(Sliced::default);
        sliced.history.end_live();
        sliced.on = !sliced.on;
        o.slice = None;
        o.slice_press = None;
        o.dragging = None;
        edit_made(o);
        true
    });
    if done {
        show(host);
    }
    done
}

/// Esc with the Slices on: the slice picked goes; with none, the Slices go off. Whether it did either.
pub(super) fn escape(o: &mut Opened) -> bool {
    let Some(sliced) = o.sliced.as_mut().filter(|s| s.on) else { return false };
    // A line being dragged stays where it has got to, one step for undo: the press is over.
    sliced.history.end_live();
    o.slice_press = None;
    if o.slice.take().is_none() {
        sliced.on = false;
    }
    true
}

/// Undo (`back`) or redo of the cuts.
pub(super) fn step(host: &dyn Host, back: bool) -> bool {
    let stepped = host.strip().borrow_mut().sound.as_mut().is_some_and(|o| {
        let Some(sliced) = o.sliced.as_mut().filter(|s| s.on) else { return false };
        let stepped = if back { sliced.history.undo() } else { sliced.history.redo() };
        if stepped {
            o.slice = None;
            o.slice_press = None;
        }
        stepped
    });
    if stepped {
        show(host);
    }
    stepped
}

/// Makes the slicing on show what `next` makes of it (given the edit, and the sound's tempo): a step for undo, or for
/// `live` part of a drag's (a slider on its way), which is one step once it ends ([`end_live`]). Whether it changed.
fn change(host: &dyn Host, live: bool, next: impl FnOnce(&Slicing, &Made, Option<f64>) -> Slicing) -> bool {
    let changed = host.strip().borrow_mut().sound.as_mut().is_some_and(|o| {
        let Some((slicing, made)) = now(o) else { return false };
        let next = next(slicing, made, o.bpm);
        if next == *slicing {
            return false;
        }
        let cuts_moved = next.cuts != slicing.cuts;
        let Some(sliced) = o.sliced.as_mut() else { return false };
        if live {
            sliced.history.live(next);
        } else {
            sliced.history.change(next);
        }
        // The slices are others now: the one picked was one of the old ones.
        if cuts_moved {
            o.slice = None;
        }
        true
    });
    if changed {
        show(host);
    }
    changed
}

/// A drag that changed the slicing as it went is over (the Sensitivity slider let go): it's one step for undo.
pub fn end_live(host: &dyn Host) {
    if let Some(sliced) = host.strip().borrow_mut().sound.as_mut().and_then(|o| o.sliced.as_mut()) {
        sliced.history.end_live();
    }
    show(host);
}

/// "Cut at" picked from its menu, by its line's words: the cuts go where that puts them (the same one again puts
/// them back there, after lines moved by hand).
pub fn cut_at_picked(host: &dyn Host, choice: &str) {
    let cut_at = if choice.starts_with(HITS) {
        CutAt::Hits
    } else if let Some((_, beats)) = ON_BEATS.iter().find(|(name, _)| *name == choice) {
        CutAt::Beats(*beats)
    } else {
        return;
    };
    change(host, false, |slicing, made, bpm| Slicing {
        cut_at,
        cuts: cuts_for(made, bpm, cut_at, slicing.sensitivity),
        ..slicing.clone()
    });
}

/// The Sensitivity slider at `to` (0 to 1), on its way: the cuts are at the hits that big.
pub fn sensitivity_moved(host: &dyn Host, to: f32) {
    let to = if to.is_finite() { to.clamp(0.0, 1.0) } else { 0.5 };
    change(host, true, |slicing, made, bpm| Slicing {
        sensitivity: to,
        cuts: cuts_for(made, bpm, slicing.cut_at, to),
        ..slicing.clone()
    });
}

/// "Each slice" picked from its menu: to the next cut, or to the sound's end.
pub fn each_picked(host: &dyn Host, choice: &str) {
    let to_end = choice == TO_END;
    change(host, false, |slicing, _, _| Slicing { to_end, ..slicing.clone() });
}

/// A press on the Slices' waveform of the sound `o`, `at` (0 to 1 across what's shown): on a line no further off
/// than `within`, to drag it; in a slice, to play it or carry it out; `shift` for no snapping to the hits.
fn press(o: &mut Opened, at: f64, within: f64, shift: bool) {
    let (from, to) = view(o);
    let long = (to - from).max(1e-9);
    let Some((slicing, made)) = now(o) else { return };
    let (at, within) = (from + at.clamp(0.0, 1.0) * long, within.max(0.0) * long);
    let on = match slicing.cut_near(at, within) {
        Some(cut) => On::Line { cut, moved: false },
        None => {
            slicing.slice_at(at, made.audio.seconds()).map_or(On::Nothing, |slice| On::Slice { slice, carried: false })
        }
    };
    o.slice_press = Some(Press { at, within, shift, on });
}

/// A press on the Slices' waveform, `at` (0 to 1 across what's shown), a line within `within` of it taken hold of;
/// `shift` for no snapping to the hits.
pub fn down(strip: &mut Strip, at: f64, within: f64, shift: bool) {
    if let Some(o) = strip.sound.as_mut() {
        press(o, at, within, shift);
    }
}

/// What the mouse moving with the button down came to.
#[derive(Debug, PartialEq)]
enum Dragged {
    Nothing,
    /// A line moved: the strip shows it.
    Line,
    /// A slice (from and to, in seconds) is carried out of the app.
    Carry((f64, f64)),
}

/// The mouse moved with the button down on the sound `o`, to `at` (0 to 1 across what's shown); `far` once it has
/// gone too far from the press to be a click. A line follows it (to the hit nearby, unless `shift`); a slice is
/// carried out of the app.
fn drag(o: &mut Opened, at: f64, shift: bool, far: bool) -> Dragged {
    let (from, to) = view(o);
    let Some((slicing, made)) = now(o) else { return Dragged::Nothing };
    let seconds = made.audio.seconds();
    let Some((on, within)) = o.slice_press.as_ref().map(|press| (press.on, press.within)) else {
        return Dragged::Nothing;
    };
    match on {
        On::Line { cut, moved } if moved || far => {
            let at = from + at.clamp(0.0, 1.0) * (to - from);
            let at = if shift {
                at
            } else {
                let hits: Vec<f64> = made.all_hits.iter().map(|(hit, _)| *hit).collect();
                sound_part::snap_to(&hits, at, within * SNAP_OVER_HOLD)
            };
            let next = slicing.moved(cut, at, seconds);
            if let Some(press) = o.slice_press.as_mut() {
                press.on = On::Line { cut, moved: true };
            }
            let (Some(next), Some(sliced)) = (next, o.sliced.as_mut()) else { return Dragged::Nothing };
            sliced.history.live(next);
            Dragged::Line
        }
        On::Slice { slice, carried: false } if far => {
            let Some(span) = slicing.slice(slice, seconds) else { return Dragged::Nothing };
            if let Some(press) = o.slice_press.as_mut() {
                press.on = On::Slice { slice, carried: true };
            }
            o.slice = Some(slice);
            Dragged::Carry(span)
        }
        _ => Dragged::Nothing,
    }
}

/// The mouse moved with the button down, to `at` (0 to 1 across what's shown); `shift` for no snapping to the hits,
/// `far` once it has gone too far from the press to be a click.
pub fn moved(host: &dyn Host, at: f64, shift: bool, far: bool) {
    let dragged = host.strip().borrow_mut().sound.as_mut().map_or(Dragged::Nothing, |o| drag(o, at, shift, far));
    match dragged {
        Dragged::Nothing => {}
        Dragged::Line => show(host),
        Dragged::Carry(span) => {
            show(host);
            carry(host, Some(span));
        }
    }
}

/// What the button coming up came to.
#[derive(Debug, PartialEq)]
enum Released {
    Nothing,
    /// A line was dragged: the strip shows where it stays.
    Line,
    /// A click on a slice (or on its line): it's picked and played.
    Play(usize),
    /// A slice carried that hadn't got going is let go.
    LetGo,
}

/// The button came up on the sound `o` (`cut_short`: the window lost the mouse). A line dragged stays where it is,
/// one step for undo; a click on a slice (or on its line) picks it and plays it; a slice carried that hadn't got
/// going is let go.
fn release(o: &mut Opened, cut_short: bool) -> Released {
    let Some(press) = o.slice_press.take() else { return Released::Nothing };
    match press.on {
        On::Line { moved: true, .. } => {
            if let Some(sliced) = o.sliced.as_mut() {
                sliced.history.end_live();
            }
            Released::Line
        }
        On::Line { cut: slice, moved: false } | On::Slice { slice, carried: false } if !cut_short => {
            Released::Play(slice)
        }
        On::Slice { carried: true, .. } => Released::LetGo,
        _ => Released::Nothing,
    }
}

/// The button came up (`cut_short`: the window lost the mouse).
pub fn up(host: &dyn Host, cut_short: bool) {
    let released = host.strip().borrow_mut().sound.as_mut().map_or(Released::Nothing, |o| release(o, cut_short));
    match released {
        Released::Nothing => {}
        Released::Line => show(host),
        Released::Play(slice) => play(host, slice),
        Released::LetGo => host.let_go(),
    }
}

/// Picks the slice `slice` (from 0) and plays it.
fn play(host: &dyn Host, slice: usize) {
    let span = host.strip().borrow_mut().sound.as_mut().and_then(|o| {
        let (slicing, made) = now(o)?;
        let span = slicing.slice(slice, made.audio.seconds())?;
        o.slice = Some(slice);
        Some(span)
    });
    let Some((from, till)) = span else { return };
    show(host);
    play_edit(host, from, Some(till));
}

/// A double-click on the sound `o`, where the press was: on a line, the line goes; elsewhere, there's a cut there
/// (on the hit nearby, unless Shift is held). Whether the cuts changed.
///
/// The toolkit calls any second press soon after a click, close by, a double-click as it's let go, however far the
/// mouse went in between, and says so before it says the button came up. So a click on a line and then, at once, a
/// drag of it (or of a slice, out of the app) comes here too: that's a drag, left for the button coming up to end.
fn double_click(o: &mut Opened) -> bool {
    let Some((slicing, made)) = now(o) else { return false };
    let Some(press) = o.slice_press.as_ref() else { return false };
    if matches!(press.on, On::Line { moved: true, .. } | On::Slice { carried: true, .. }) {
        return false;
    }
    let (at, within, shift) = (press.at, press.within, press.shift);
    let next = match slicing.cut_near(at, within) {
        Some(cut) => slicing.without_cut(cut),
        None => {
            let at = if shift {
                at
            } else {
                let hits: Vec<f64> = made.all_hits.iter().map(|(hit, _)| *hit).collect();
                sound_part::snap_to(&hits, at, within * SNAP_OVER_HOLD)
            };
            slicing.with_cut(at, made.audio.seconds())
        }
    };
    // The button coming up is no click.
    if let Some(press) = o.slice_press.as_mut() {
        press.on = On::Nothing;
    }
    let (Some(next), Some(sliced)) = (next, o.sliced.as_mut()) else { return false };
    sliced.history.change(next);
    o.slice = None;
    true
}

/// A double-click on the Slices' waveform, where the press was.
pub fn double(host: &dyn Host) {
    let changed = host.strip().borrow_mut().sound.as_mut().is_some_and(double_click);
    if !changed {
        return;
    }
    // The first of the two clicks played a slice that's another now.
    stop(host);
    show(host);
}

/// Whether a cut's line is within `within` of `at` (0 to 1 across what's shown): the mouse there takes hold of it.
pub fn near(strip: &Strip, at: f64, within: f64) -> bool {
    let Some(o) = strip.sound.as_ref() else { return false };
    let Some((slicing, _)) = now(o) else { return false };
    let (from, to) = view(o);
    let long = (to - from).max(1e-9);
    slicing.cut_near(from + at.clamp(0.0, 1.0) * long, within.max(0.0) * long).is_some()
}

/// Each slice of the edit `made` as `slicing` cuts it, in frames.
fn spans(slicing: &Slicing, made: &Made) -> Vec<Span> {
    let rate = f64::from(made.audio.sample_rate.max(1));
    let cuts: Vec<usize> = slicing.cuts.iter().map(|c| (c * rate).round() as usize).collect();
    edit_copy::slice_spans(&cuts, made.audio.len(), slicing.to_end)
}

/// Sends the slices: the host puts each in a file of its own, saved in a folder for this slicing in Edited sounds
/// (made now, or the ones made before), then on a Drum Machine in Bitwig, or opens their folder; Send waits until
/// they're there ([`sent`]). Whether there were slices to send.
pub(super) fn send(host: &dyn Host, to: To) -> bool {
    let ready = {
        let s = host.strip().borrow();
        s.sound.as_ref().filter(|_| !s.sending).and_then(|o| {
            let (slicing, made) = now(o)?;
            let spans = spans(slicing, made);
            if spans.is_empty() {
                return None;
            }
            Some(Slices {
                id: o.id,
                source: o.source.clone(),
                changed: o.changed,
                audio: made.audio.clone(),
                what: format!("{:?}", made.edits),
                spans,
                name: o.name.clone(),
            })
        })
    };
    let Some(slices) = ready else { return false };
    if host.send(to, slices) {
        host.strip().borrow_mut().sending = true;
        show(host);
    }
    true
}

/// The slices handed to the host ([`Host::send`]) are where they were sent, or couldn't be: Send comes back.
pub fn sent(host: &dyn Host) {
    host.strip().borrow_mut().sending = false;
    show(host);
}

/// A number that's another whenever the lines the mouse can take hold of are others: where the cuts are, and the
/// stretch of the sound that's shown.
fn cuts_mark(cuts: &[f64], from: f64, to: f64) -> i32 {
    let mut mark = DefaultHasher::new();
    for at in cuts.iter().chain([&from, &to]) {
        mark.write_u64(at.to_bits());
    }
    let mark = mark.finish();
    (mark ^ (mark >> 32)) as i32
}

/// The Slices of the sound `o` as they are now (`sending`: slices are on their way, so Send waits).
pub(super) fn showing(o: &Opened, sending: bool) -> Showing {
    let Some(sliced) = o.sliced.as_ref().filter(|s| s.on) else { return Showing::default() };
    let slicing = &sliced.history.now;
    let ready = now(o);
    // What the cuts go by: the hits, with how many cuts they give; or the beats, for a sound whose tempo is known.
    let at_hits = match (slicing.cut_at, ready) {
        (CutAt::Hits, _) => slicing.cuts.len(),
        (CutAt::Beats(_), Some((_, made))) => cuts_for(made, None, CutAt::Hits, slicing.sensitivity).len(),
        (CutAt::Beats(_), None) => 0,
    };
    let hits_line = format!("{HITS} ({at_hits})");
    let cut_at = match slicing.cut_at {
        CutAt::Hits => hits_line.clone(),
        CutAt::Beats(beats) => ON_BEATS.iter().find(|(_, b)| *b == beats).map_or(HITS, |(name, _)| *name).to_owned(),
    };
    let mut cut_at_choices = vec![hits_line];
    if o.bpm.is_some() {
        cut_at_choices.extend(ON_BEATS.iter().map(|(name, _)| (*name).to_owned()));
    }
    let to_next = if slicing.cut_at == CutAt::Hits { TO_NEXT_HIT } else { TO_NEXT_CUT };
    let mut show = Showing {
        on: true,
        cut_at,
        cut_at_choices,
        cut_at_hits: slicing.cut_at == CutAt::Hits,
        sensitivity: slicing.sensitivity,
        each: if slicing.to_end { TO_END } else { to_next }.to_owned(),
        each_choices: vec![to_next.to_owned(), TO_END.to_owned()],
        ..Showing::default()
    };
    // Not sliced yet (the edit is still being made): no cuts to show or send.
    let Some((_, made)) = ready else { return show };
    let (from, to) = view(o);
    show.cuts_mark = cuts_mark(&slicing.cuts, from, to);
    let long = (to - from).max(1e-9);
    let across = |t: f64| ((t - from) / long) as f32;
    // The cuts in sight, each with the room to the next line (its number shows where there's room for it).
    let shown: Vec<(usize, f64)> =
        slicing.cuts.iter().copied().enumerate().filter(|(_, cut)| *cut >= from && *cut <= to).collect();
    show.cuts = shown
        .iter()
        .enumerate()
        .map(|(k, &(i, cut))| {
            let next = shown.get(k + 1).map_or(to, |&(_, next)| next);
            Cut { at: across(cut), number: i + 1, room: across(next) - across(cut) }
        })
        .collect();
    show.first_cut = slicing.cuts.first().map_or(0.0, |first| across(*first).clamp(0.0, 1.0));
    (show.slice_from, show.slice_to) =
        picked(o).map_or((0.0, 0.0), |(a, b)| (across(a).clamp(0.0, 1.0), across(b).clamp(0.0, 1.0)));
    let slices = slicing.cuts.len();
    show.pads = (0..slices).map(to_bitwig::slice_pad_name).collect();
    show.can_send = slices > 0 && !sending;
    show.send_text = format!("Send {}", to_bitwig::slices_text(slices));
    show.to_folder_text =
        if slices > 1 { format!("{TO_FOLDER}, 01 \u{2026} {slices:02}") } else { TO_FOLDER.to_owned() };
    // Where they're saved: the folder this slicing has, by its name.
    let folder = edit_copy::slices_folder_name(&o.source, &format!("{:?}", made.edits), &spans(slicing, made));
    show.saved_in = format!("Saved in {}: {folder}", edit_copy::FOLDER);
    show
}

/// The pads as chips along the strip's foot: "1 → C1", "2 → C#1"…, the first few and then the last; the picked
/// slice's is lit.
pub(super) fn pad_chips(o: &Opened) -> Vec<Chip> {
    let slices = now(o).map_or(0, |(slicing, _)| slicing.cuts.len());
    let pad = |i: usize| format!("{} \u{2192} {}", i + 1, to_bitwig::slice_pad_name(i));
    let mut chips: Vec<Chip> =
        (0..slices.min(PADS_NAMED)).map(|i| Chip { text: pad(i), last: o.slice == Some(i) }).collect();
    if slices > PADS_NAMED {
        let last = slices - 1;
        let lit = o.slice.is_some_and(|picked| picked >= PADS_NAMED);
        let text = match o.slice.filter(|_| lit) {
            // The one picked, when it's past those named.
            Some(picked) if picked < last => format!("\u{2026} {} \u{2026} {}", pad(picked), pad(last)),
            _ => format!("\u{2026} {}", pad(last)),
        };
        chips.push(Chip { text, last: lit });
    }
    chips
}

/// The strip's heading with the Slices on: "20 slices · 2 bars", and the slice picked ("· slice 5 is 0.23 s"). How
/// long the sound is, until it's sliced.
pub(super) fn heading(o: &Opened, seconds: f64) -> String {
    let Some((slicing, _)) = now(o) else { return sound_part::length_text(seconds) };
    let mut text = format!("{} \u{b7} {}", to_bitwig::slices_text(slicing.cuts.len()), length_words(seconds, o.bpm));
    if let (Some(slice), Some((from, till))) = (o.slice, picked(o)) {
        let _ = write!(text, " \u{b7} slice {} is {}", slice + 1, sound_part::length_text(till - from));
    }
    text
}

/// How long a sound is, in whole bars when its tempo is known and it's as good as that ("2 bars"), else as the card
/// says it ("3.75 s").
fn length_words(seconds: f64, bpm: Option<f64>) -> String {
    if let Some(bpm) = bpm {
        let bars = seconds * bpm / 60.0 / 4.0;
        let whole = bars.round();
        if whole >= 1.0 && (bars - whole).abs() < 0.02 {
            return format!("{whole} bar{}", if whole == 1.0 { "" } else { "s" });
        }
    }
    sound_part::length_text(seconds)
}

/// For the screenshots: how many cuts the sound's Slices have (none with them off), and the slice picked.
pub fn state_for_screens(strip: &Strip) -> (usize, Option<usize>) {
    let Some(o) = strip.sound.as_ref() else { return (0, None) };
    (now(o).map_or(0, |(slicing, _)| slicing.cuts.len()), o.slice)
}

#[cfg(test)]
mod tests {
    use super::super::{Edits, make, stash};
    use super::*;

    fn slicing(cuts: &[f64]) -> Slicing {
        Slicing { cuts: cuts.to_vec(), ..Slicing::default() }
    }

    /// The edit of `audio` with `edits`, made, with its hits at `hits` (seconds, and how much each rises).
    fn made(audio: &AudioData, edits: &[Edit], hits: &[(f64, f32)]) -> Made {
        Made { all_hits: hits.into(), ..make(audio, edits.to_vec()) }
    }

    /// A sound two seconds long, open in the strip and made: a big hit every half second, and a small one after the
    /// first.
    fn opened() -> Opened {
        let mut audio = AudioData { sample_rate: 1_000, channels: vec![vec![0.5; 2_000]], ..AudioData::default() };
        audio.file_seconds = audio.seconds();
        let hits = [(0.0, 40.0), (0.25, 8.0), (0.5, 20.0), (1.0, 20.0), (1.5, 20.0)];
        Opened {
            id: 1,
            source: Source { path: "a.wav".into(), span: None },
            name: "a".into(),
            colour: 0,
            bpm: None,
            made: Some(made(&audio, &[], &hits)),
            original: Some(Arc::new(audio)),
            changed: None,
            note: String::new(),
            edits: Edits::default(),
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

    /// The scissors, as [`toggle`] turns them on.
    fn slices_on(o: &mut Opened) {
        o.sliced.get_or_insert_with(Sliced::default).on = true;
        edit_made(o);
    }

    fn cuts(o: &Opened) -> Vec<f64> {
        now(o).map(|(slicing, _)| slicing.cuts.clone()).unwrap_or_default()
    }

    #[test]
    fn the_edit_is_cut_at_its_hits_and_afresh_once_it_is_another_edit() {
        let mut o = opened();
        assert!(!on(&o) && now(&o).is_none(), "off until the scissors");
        slices_on(&mut o);
        // The usual sensitivity leaves the small hit out.
        assert_eq!(cuts(&o), [0.0, 0.5, 1.0, 1.5]);
        o.slice = Some(1);
        assert_eq!(picked(&o), Some((0.5, 1.0)));
        // A line moved by hand stays where it is when the same edit is made again.
        let moved = now(&o).unwrap().0.moved(1, 0.6, 2.0).unwrap();
        o.sliced.as_mut().unwrap().history.change(moved);
        edit_made(&mut o);
        assert_eq!(cuts(&o), [0.0, 0.6, 1.0, 1.5]);
        assert_eq!(steps(&o), (true, false));
        // Off, an edit, and on again before it's made: no cuts to show or send (those there were are the old
        // edit's).
        o.sliced.as_mut().unwrap().on = false;
        o.edits.change(vec![Edit::Reverse(None)]);
        slices_on(&mut o);
        assert!(on(&o) && now(&o).is_none() && picked(&o).is_none());
        // Made: it's cut afresh, by the choices there were, with nothing of the old cuts to undo.
        let audio = o.original.clone().unwrap();
        o.made = Some(made(&audio, &o.edits.now, &[(0.2, 20.0), (1.2, 20.0)]));
        edit_made(&mut o);
        assert_eq!(cuts(&o), [0.2, 1.2]);
        assert_eq!((o.slice, steps(&o)), (None, (false, false)));
    }

    #[test]
    fn the_cuts_wait_while_the_slices_are_off() {
        let mut o = opened();
        slices_on(&mut o);
        let added = now(&o).unwrap().0.with_cut(0.25, 2.0).unwrap();
        o.sliced.as_mut().unwrap().history.change(added);
        o.sliced.as_mut().unwrap().on = false;
        assert!(now(&o).is_none() && pad_chips(&o).is_empty());
        slices_on(&mut o);
        assert_eq!(cuts(&o), [0.0, 0.25, 0.5, 1.0, 1.5]);
        assert_eq!(steps(&o), (true, false), "and its steps for undo");
    }

    #[test]
    fn esc_lets_the_slice_picked_go_then_the_slices() {
        let mut o = opened();
        assert!(!escape(&mut o), "with the Slices off, Esc is the strip's own");
        slices_on(&mut o);
        o.slice = Some(2);
        assert!(escape(&mut o) && o.slice.is_none() && on(&o));
        assert!(escape(&mut o) && !on(&o));
        assert!(!escape(&mut o));
    }

    /// A press at `seconds` into the two-second sound (all of it on show), a line within 20 ms of it taking hold.
    fn press_at(o: &mut Opened, seconds: f64, shift: bool) {
        press(o, seconds / 2.0, 0.01, shift);
    }

    #[test]
    fn a_click_on_a_line_then_at_once_a_drag_of_it_is_a_drag() {
        let mut o = opened();
        slices_on(&mut o);
        // A click on the second line picks its slice and plays it.
        press_at(&mut o, 0.5, true);
        assert_eq!(release(&mut o, false), Released::Play(1));
        // At once, a press on it again: not far yet, it's still a click; further, the line follows the mouse (Shift:
        // to no hit nearby).
        press_at(&mut o, 0.5, true);
        assert_eq!(drag(&mut o, 0.26, true, false), Dragged::Nothing);
        assert_eq!(drag(&mut o, 0.3, true, true), Dragged::Line);
        assert_eq!(cuts(&o), [0.0, 0.6, 1.0, 1.5]);
        // The toolkit calls that second press a double-click as it's let go: the line stays, and it's still a drag.
        assert!(!double_click(&mut o));
        assert_eq!(cuts(&o), [0.0, 0.6, 1.0, 1.5]);
        assert_eq!(steps(&o), (false, false), "no step until it's let go");
        assert_eq!(release(&mut o, false), Released::Line);
        assert_eq!(steps(&o), (true, false), "one step for undo");
        assert!(o.sliced.as_mut().unwrap().history.undo());
        assert_eq!(cuts(&o), [0.0, 0.5, 1.0, 1.5]);
    }

    #[test]
    fn a_click_in_a_slice_then_at_once_a_carry_of_it_adds_no_cut() {
        let mut o = opened();
        slices_on(&mut o);
        press_at(&mut o, 0.7, false);
        assert_eq!(release(&mut o, false), Released::Play(1));
        press_at(&mut o, 0.7, false);
        assert_eq!(drag(&mut o, 0.36, false, false), Dragged::Nothing, "not far yet: still a click");
        assert_eq!(drag(&mut o, 0.4, false, true), Dragged::Carry((0.5, 1.0)));
        assert_eq!(o.slice, Some(1), "the slice carried is the one picked");
        assert_eq!(drag(&mut o, 0.45, false, true), Dragged::Nothing, "carried once");
        // The toolkit's double-click as it's let go: no cut where the press was.
        assert!(!double_click(&mut o));
        assert_eq!(cuts(&o), [0.0, 0.5, 1.0, 1.5]);
        assert_eq!(release(&mut o, false), Released::LetGo);
        assert_eq!(steps(&o), (false, false));
    }

    #[test]
    fn a_double_click_takes_a_line_away_or_adds_a_cut() {
        let mut o = opened();
        slices_on(&mut o);
        // On a line: the first click plays its slice; the second takes the line away, and its letting go is no click.
        press_at(&mut o, 0.5, false);
        assert_eq!(release(&mut o, false), Released::Play(1));
        press_at(&mut o, 0.5, false);
        assert!(double_click(&mut o));
        assert_eq!(cuts(&o), [0.0, 1.0, 1.5]);
        assert_eq!(release(&mut o, false), Released::Nothing);
        // Between lines: a cut there, on the hit nearby (the small one a quarter of a second in)...
        press_at(&mut o, 0.26, false);
        assert!(double_click(&mut o));
        assert_eq!(cuts(&o), [0.0, 0.25, 1.0, 1.5]);
        assert_eq!(release(&mut o, false), Released::Nothing);
        // ...or just where the press was, with Shift held.
        press_at(&mut o, 0.52, true);
        assert!(double_click(&mut o));
        assert_eq!(cuts(&o), [0.0, 0.25, 0.52, 1.0, 1.5]);
        assert_eq!(release(&mut o, false), Released::Nothing);
        // Where the hit nearby has a line already there's no room for one: nothing changes, and it's still no click.
        press_at(&mut o, 0.275, false);
        assert!(!double_click(&mut o));
        assert_eq!(cuts(&o), [0.0, 0.25, 0.52, 1.0, 1.5]);
        assert_eq!(release(&mut o, false), Released::Nothing);
        // Each was a step for undo.
        let history = &mut o.sliced.as_mut().unwrap().history;
        assert_eq!(history.undo.len(), 3);
        assert!(history.undo() && history.undo() && history.undo() && !history.undo());
        assert_eq!(cuts(&o), [0.0, 0.5, 1.0, 1.5]);
    }

    #[test]
    fn a_press_before_the_first_cut_or_cut_short_plays_nothing() {
        let mut o = opened();
        slices_on(&mut o);
        // The first line taken away: what comes before the first cut is in no slice.
        let without = now(&o).unwrap().0.without_cut(0).unwrap();
        o.sliced.as_mut().unwrap().history.change(without);
        press_at(&mut o, 0.2, false);
        assert_eq!(drag(&mut o, 0.3, false, true), Dragged::Nothing);
        assert_eq!(release(&mut o, false), Released::Nothing);
        // The window lost the mouse: a press in a slice is no click, and a line dragged stays where it got to.
        press_at(&mut o, 0.7, false);
        assert_eq!(release(&mut o, true), Released::Nothing);
        press_at(&mut o, 1.0, true);
        assert_eq!(drag(&mut o, 0.55, true, true), Dragged::Line);
        assert_eq!(release(&mut o, true), Released::Line);
        assert_eq!(cuts(&o), [0.5, 1.1, 1.5]);
        // With no press, the button coming up and the mouse moving are nothing.
        assert_eq!(release(&mut o, false), Released::Nothing);
        assert_eq!(drag(&mut o, 0.5, false, true), Dragged::Nothing);
        assert!(!double_click(&mut o));
    }

    #[test]
    fn a_line_dragged_snaps_to_the_hit_nearby_unless_shift_is_held() {
        let mut o = opened();
        slices_on(&mut o);
        // The second line, dragged to near the small hit a quarter of a second in.
        press_at(&mut o, 0.5, false);
        assert_eq!(drag(&mut o, 0.13, false, true), Dragged::Line);
        assert_eq!(cuts(&o), [0.0, 0.25, 1.0, 1.5], "on the hit");
        assert_eq!(drag(&mut o, 0.13, true, true), Dragged::Line);
        assert_eq!(cuts(&o), [0.0, 0.26, 1.0, 1.5], "where the mouse is");
        // Back where it began: no step for undo.
        assert_eq!(drag(&mut o, 0.25, true, true), Dragged::Line);
        assert_eq!(release(&mut o, false), Released::Line);
        assert_eq!((cuts(&o), steps(&o)), (vec![0.0, 0.5, 1.0, 1.5], (false, false)));
    }

    #[test]
    fn esc_while_a_line_is_dragged_leaves_it_there_with_a_step_for_undo() {
        let mut o = opened();
        slices_on(&mut o);
        o.slice = Some(2);
        press_at(&mut o, 1.0, true);
        assert_eq!(drag(&mut o, 0.55, true, true), Dragged::Line);
        assert_eq!(steps(&o), (false, false), "no step while it's dragged");
        // Esc lets the slice picked go, and the line stays where it got to.
        assert!(escape(&mut o) && on(&o) && o.slice.is_none());
        assert_eq!(cuts(&o), [0.0, 0.5, 1.1, 1.5]);
        assert_eq!(steps(&o), (true, false));
        // The press is over: the mouse moving on, and the button coming up, do nothing more.
        assert_eq!(drag(&mut o, 0.6, true, true), Dragged::Nothing);
        assert!(!double_click(&mut o));
        assert_eq!(release(&mut o, false), Released::Nothing);
        assert_eq!(cuts(&o), [0.0, 0.5, 1.1, 1.5]);
        // With no slice picked, Esc turns the Slices off: the step waits with the cuts.
        press_at(&mut o, 0.5, true);
        assert_eq!(drag(&mut o, 0.3, true, true), Dragged::Line);
        assert!(escape(&mut o) && !on(&o));
        assert_eq!(release(&mut o, false), Released::Nothing);
        slices_on(&mut o);
        assert_eq!(cuts(&o), [0.0, 0.6, 1.1, 1.5]);
        assert_eq!(o.sliced.as_ref().unwrap().history.undo.len(), 2);
    }

    #[test]
    fn slices_with_choices_of_his_own_are_worth_keeping() {
        assert!(!Sliced::default().worth_keeping());
        // On; or off with a change to undo.
        assert!(Sliced { on: true, ..Sliced::default() }.worth_keeping());
        let mut stepped = Sliced::default();
        stepped.history.change(slicing(&[0.5]));
        assert!(stepped.worth_keeping());
        // Off, and cut afresh by an edit since (so nothing to undo): the cuts alone are nothing to keep, as the
        // scissors make them again; his choices are.
        let cut = |now: Slicing| Sliced { history: History { now, ..History::default() }, ..Sliced::default() };
        assert!(!cut(slicing(&[0.0, 0.5])).worth_keeping());
        assert!(cut(Slicing { sensitivity: 0.8, ..Slicing::default() }).worth_keeping());
        assert!(cut(Slicing { to_end: true, ..Slicing::default() }).worth_keeping());
        assert!(cut(Slicing { cut_at: CutAt::Beats(1.0), ..Slicing::default() }).worth_keeping());
    }

    #[test]
    fn going_to_another_sound_keeps_the_slices_worth_keeping() {
        // Slices turned on and off again, with nothing changed: nothing of the sound to keep.
        let mut o = opened();
        slices_on(&mut o);
        o.sliced.as_mut().unwrap().on = false;
        let mut strip = Strip { sound: Some(o), ..Strip::default() };
        stash(&mut strip);
        assert!(strip.sound.is_none() && strip.kept.is_empty());
        // A sensitivity of his own, off: kept, with no edits.
        let mut o = opened();
        slices_on(&mut o);
        let sliced = o.sliced.as_mut().unwrap();
        sliced.history = History { now: Slicing { sensitivity: 0.8, ..slicing(&[0.0]) }, ..History::default() };
        sliced.on = false;
        strip.sound = Some(o);
        stash(&mut strip);
        assert_eq!(strip.kept.len(), 1);
        assert_eq!(strip.kept[0].sliced.as_ref().map(|s| s.history.now.sensitivity), Some(0.8));
        // A line being dragged as the sound goes is where it was dragged to, one step for undo; the sound's kept
        // once, the newest of it.
        let mut o = opened();
        slices_on(&mut o);
        press_at(&mut o, 0.5, true);
        assert_eq!(drag(&mut o, 0.3, true, true), Dragged::Line);
        strip.sound = Some(o);
        stash(&mut strip);
        assert_eq!(strip.kept.len(), 1);
        let kept = strip.kept[0].sliced.as_ref().unwrap();
        assert_eq!(kept.history.now.cuts, [0.0, 0.6, 1.0, 1.5]);
        assert!(kept.on && kept.history.undo.len() == 1 && kept.history.live.is_none());
    }

    #[test]
    fn the_cuts_mark_is_another_for_other_lines_or_another_stretch_shown() {
        let mark = cuts_mark(&[0.0, 0.5, 1.0], 0.0, 2.0);
        assert_eq!(mark, cuts_mark(&[0.0, 0.5, 1.0], 0.0, 2.0));
        assert_ne!(mark, cuts_mark(&[0.0, 0.6, 1.0], 0.0, 2.0), "a line moved");
        assert_ne!(mark, cuts_mark(&[0.0, 0.5], 0.0, 2.0), "the last line taken away");
        assert_ne!(mark, cuts_mark(&[0.5, 1.0], 0.0, 2.0), "the line at the sound's start taken away");
        assert_ne!(mark, cuts_mark(&[0.0, 0.5, 1.0], 0.5, 2.0), "zoomed in");
        assert_ne!(mark, cuts_mark(&[0.0, 0.5, 1.0], 0.0, 1.5));
        assert_ne!(cuts_mark(&[], 0.0, 2.0), cuts_mark(&[0.0], 0.0, 2.0));
    }

    #[test]
    fn the_foot_names_the_first_pads_and_the_last_and_the_heading_counts_the_slices() {
        let mut o = opened();
        // 120 BPM: two seconds is a bar.
        o.bpm = Some(120.0);
        slices_on(&mut o);
        let twenty = Slicing { cuts: (0..20).map(|i| f64::from(i) * 0.1).collect(), ..Slicing::default() };
        o.sliced.as_mut().unwrap().history.change(twenty);
        let texts = |o: &Opened| pad_chips(o).iter().map(|c| c.text.clone()).collect::<Vec<_>>();
        let lit = |o: &Opened| pad_chips(o).iter().position(|c| c.last);
        let chips = texts(&o);
        assert_eq!(chips.len(), PADS_NAMED + 1);
        assert_eq!(chips[0], "1 \u{2192} C1");
        assert_eq!(chips[7], "8 \u{2192} G1");
        assert_eq!(chips[8], "\u{2026} 20 \u{2192} G2");
        assert_eq!(lit(&o), None);
        assert_eq!(heading(&o, 2.0), "20 slices \u{b7} 1 bar");
        o.slice = Some(4);
        assert_eq!(lit(&o), Some(4));
        assert_eq!(heading(&o, 2.0), "20 slices \u{b7} 1 bar \u{b7} slice 5 is 0.10 s");
        // One picked past those named is named too, before the last.
        o.slice = Some(12);
        assert_eq!(texts(&o)[8], "\u{2026} 13 \u{2192} C2 \u{2026} 20 \u{2192} G2");
        assert_eq!(lit(&o), Some(8));
        o.slice = Some(19);
        assert_eq!(texts(&o)[8], "\u{2026} 20 \u{2192} G2");
        assert_eq!(lit(&o), Some(8));
        // Few enough to name them all: no last one set apart.
        let three = Slicing { cuts: vec![0.0, 0.5, 1.0], ..Slicing::default() };
        o.sliced.as_mut().unwrap().history.change(three);
        assert_eq!(texts(&o), ["1 \u{2192} C1", "2 \u{2192} C#1", "3 \u{2192} D1"]);
    }

    #[test]
    fn a_drag_is_one_step_for_undo() {
        let mut h = History::<Slicing>::default();
        h.live(slicing(&[0.1]));
        h.live(slicing(&[0.2]));
        assert!(h.undo.is_empty() && h.has_steps(), "no step until it ends");
        h.end_live();
        assert_eq!(h.undo.len(), 1);
        assert!(h.undo() && h.now == Slicing::default());
        assert!(h.redo() && h.now == slicing(&[0.2]));
        // One that ends where it began is no step.
        h.live(slicing(&[0.3]));
        h.live(slicing(&[0.2]));
        h.end_live();
        assert_eq!(h.undo.len(), 1);
        // Undo in the middle of one ends it first: it's the step undone.
        h.live(slicing(&[0.4]));
        assert!(h.undo());
        assert_eq!(h.now, slicing(&[0.2]));
        assert!(h.redo() && h.now == slicing(&[0.4]));
    }

    #[test]
    fn a_slice_runs_to_the_next_cut_or_to_the_end() {
        let s = slicing(&[0.5, 1.0, 1.5]);
        assert_eq!(s.slice(0, 2.0), Some((0.5, 1.0)));
        assert_eq!(s.slice(2, 2.0), Some((1.5, 2.0)));
        assert_eq!(s.slice(3, 2.0), None);
        let to_end = Slicing { to_end: true, ..s.clone() };
        assert_eq!(to_end.slice(0, 2.0), Some((0.5, 2.0)));
        // A moment is in the slice that starts at the last cut before it; before the first cut it's in none.
        assert_eq!(s.slice_at(0.2, 2.0), None);
        assert_eq!(s.slice_at(0.5, 2.0), Some(0));
        assert_eq!(s.slice_at(1.2, 2.0), Some(1));
        assert_eq!(s.slice_at(1.99, 2.0), Some(2));
        assert_eq!(s.slice_at(2.0, 2.0), None);
        assert_eq!(to_end.slice_at(1.2, 2.0), Some(1));
    }

    #[test]
    fn a_press_takes_hold_of_the_nearest_line_within_reach() {
        let s = slicing(&[0.5, 0.52, 1.5]);
        assert_eq!(s.cut_near(0.505, 0.02), Some(0));
        assert_eq!(s.cut_near(0.515, 0.02), Some(1));
        assert_eq!(s.cut_near(0.8, 0.02), None);
        assert_eq!(s.cut_near(1.49, 0.02), Some(2));
    }

    #[test]
    fn a_line_moves_only_as_far_as_its_neighbours_leave_a_slice() {
        let s = slicing(&[0.5, 1.0, 1.5]);
        assert_eq!(s.moved(1, 1.2, 2.0).unwrap().cuts, [0.5, 1.2, 1.5]);
        // Past the next line: as near it as leaves a slice between.
        let near = s.moved(1, 1.9, 2.0).unwrap().cuts[1];
        assert!((near - (1.5 - LEAST_SLICE_SECONDS)).abs() < 1e-9);
        let near = s.moved(1, 0.0, 2.0).unwrap().cuts[1];
        assert!((near - (0.5 + LEAST_SLICE_SECONDS)).abs() < 1e-9);
        // The first goes back to the start, the last no nearer the end than a slice.
        assert_eq!(s.moved(0, -1.0, 2.0).unwrap().cuts[0], 0.0);
        let last = s.moved(2, 5.0, 2.0).unwrap().cuts[2];
        assert!((last - (2.0 - LEAST_SLICE_SECONDS)).abs() < 1e-9);
        // Where it is already, a cut that isn't there, or nowhere: no move.
        assert!(s.moved(1, 1.0, 2.0).is_none());
        assert!(s.moved(3, 1.0, 2.0).is_none());
        assert!(s.moved(1, f64::NAN, 2.0).is_none());
        // No room between its neighbours at all.
        let tight = slicing(&[0.5, 0.53, 0.56]);
        assert!(tight.moved(1, 0.54, 2.0).is_none());
    }

    #[test]
    fn a_cut_is_added_in_its_place_and_taken_away() {
        let s = slicing(&[0.5, 1.5]);
        assert_eq!(s.with_cut(1.0, 2.0).unwrap().cuts, [0.5, 1.0, 1.5]);
        assert_eq!(s.with_cut(0.1, 2.0).unwrap().cuts, [0.1, 0.5, 1.5]);
        // Too near another, too near the end, before the start: none.
        assert!(s.with_cut(0.51, 2.0).is_none());
        assert!(s.with_cut(1.99, 2.0).is_none());
        assert!(s.with_cut(-0.1, 2.0).is_none());
        let full = slicing(&(0..MOST_SLICES).map(|i| i as f64).collect::<Vec<_>>());
        assert!(full.with_cut(0.5, 100.0).is_none(), "no more than the most");
        assert_eq!(s.without_cut(0).unwrap().cuts, [1.5]);
        assert!(s.without_cut(2).is_none());
        // One slice always stays.
        assert!(slicing(&[0.5]).without_cut(0).is_none());
    }

    #[test]
    fn the_length_reads_in_bars_when_it_is_whole_bars() {
        // 128 BPM: a bar is 1.875 s.
        assert_eq!(length_words(3.75, Some(128.0)), "2 bars");
        assert_eq!(length_words(1.875, Some(128.0)), "1 bar");
        assert_eq!(length_words(3.0, Some(128.0)), "3.00 s");
        assert_eq!(length_words(3.75, None), "3.75 s");
    }
}
