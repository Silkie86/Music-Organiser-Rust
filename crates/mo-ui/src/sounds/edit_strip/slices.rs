//! The Edit strip's Slices (slice to pads: the 1.1 plan's light sample edits, mock-up 9, Solomon's "Build as shown" of
//! 27 Nov 2026). The scissors (Alt+S) cut the edit at its hits, as big as the Sensitivity asks, or on the beats of a
//! sound whose tempo is known: a line at each cut, the slices numbered from 1. A line is dragged to move it (it snaps
//! to the hits; Shift for none); a double-click takes a line away, or adds one where there's none. A click in a slice
//! plays it, and a press in one that moves carries that slice out of the app. Send puts the slices, each a file of its
//! own in a folder for this slicing in the export folder's Edited sounds ([`edit_copy::slices_to_keep`]), on a Drum
//! Machine in Bitwig, one to a pad from C1 (the Kits' way in), or just opens their folder.
//!
//! While the Slices are on, undo and redo step through the cuts, and the edit's own tools wait: the cuts are always of
//! the edit as it is. Each sound's slicing is kept with its edits, so going to another sound and back finds it.

use std::fmt::Write as _;
use std::panic::AssertUnwindSafe;

use mo_app::edit_copy::{self, LEAST_SLICE_SECONDS, MOST_SLICES};
use mo_app::{background, log, to_bitwig};
use mo_audio::edit::Edit;
use mo_audio::hits;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::super::{FEED, HERE, PAGE, did, kit, part};
use super::{History, Made, Opened, STRIP, carry, play_edit, view};
use crate::{AppWindow, EditChip, SliceCut, SoundEdit};

/// The ids of the Slices' actions in the shortcut list. Saving them in their folder is the strip's own Save.
pub(super) const SLICES: &str = "sounds.editSlices";
pub(super) const TO_BITWIG: &str = "sounds.editSlicesBitwig";

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
pub(super) enum To {
    /// On a Drum Machine in Bitwig, one to a pad from C1.
    Bitwig,
    /// Just their folder, numbered.
    Folder,
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
pub(super) fn toggle(app: &AppWindow) -> bool {
    let done = STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let Some(o) = s.sound.as_mut() else { return false };
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
        super::show(app);
    }
    done
}

/// Esc with the Slices on: the slice picked goes; with none, the Slices go off. Whether it did either.
pub(super) fn escape(o: &mut Opened) -> bool {
    let Some(sliced) = o.sliced.as_mut().filter(|s| s.on) else { return false };
    o.slice_press = None;
    if o.slice.take().is_none() {
        sliced.history.end_live();
        sliced.on = false;
    }
    true
}

/// Undo (`back`) or redo of the cuts.
pub(super) fn step(app: &AppWindow, back: bool) -> bool {
    let stepped = STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let Some(o) = s.sound.as_mut() else { return false };
        let Some(sliced) = o.sliced.as_mut().filter(|s| s.on) else { return false };
        let stepped = if back { sliced.history.undo() } else { sliced.history.redo() };
        if stepped {
            o.slice = None;
            o.slice_press = None;
        }
        stepped
    });
    if stepped {
        super::show(app);
    }
    stepped
}

/// Makes the slicing on show what `next` makes of it (given the edit, and the sound's tempo): a step for undo, or for
/// `live` part of a drag's (a slider on its way), which is one step once it ends ([`end_live`]). Whether it changed.
fn change(app: &AppWindow, live: bool, next: impl FnOnce(&Slicing, &Made, Option<f64>) -> Slicing) -> bool {
    let changed = STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let Some(o) = s.sound.as_mut() else { return false };
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
        super::show(app);
    }
    changed
}

/// A drag that changed the slicing as it went is over: it's one step for undo.
fn end_live(app: &AppWindow) {
    STRIP.with(|s| {
        if let Some(sliced) = s.borrow_mut().sound.as_mut().and_then(|o| o.sliced.as_mut()) {
            sliced.history.end_live();
        }
    });
    super::show(app);
}

/// "Cut at" picked from its menu, by its line's words: the cuts go where that puts them (the same one again puts
/// them back there, after lines moved by hand).
fn cut_at_picked(app: &AppWindow, choice: &str) {
    let cut_at = if choice.starts_with(HITS) {
        CutAt::Hits
    } else if let Some((_, beats)) = ON_BEATS.iter().find(|(name, _)| *name == choice) {
        CutAt::Beats(*beats)
    } else {
        return;
    };
    change(app, false, |slicing, made, bpm| Slicing {
        cut_at,
        cuts: cuts_for(made, bpm, cut_at, slicing.sensitivity),
        ..slicing.clone()
    });
}

/// The Sensitivity slider at `to` (0 to 1), on its way: the cuts are at the hits that big.
fn sensitivity_moved(app: &AppWindow, to: f32) {
    let to = if to.is_finite() { to.clamp(0.0, 1.0) } else { 0.5 };
    change(app, true, |slicing, made, bpm| Slicing {
        sensitivity: to,
        cuts: cuts_for(made, bpm, slicing.cut_at, to),
        ..slicing.clone()
    });
}

/// "Each slice" picked from its menu: to the next cut, or to the sound's end.
fn each_picked(app: &AppWindow, choice: &str) {
    let to_end = choice == TO_END;
    change(app, false, |slicing, _, _| Slicing { to_end, ..slicing.clone() });
}

/// A press on the Slices' waveform, `at` (0 to 1 across what's shown): on a line no further off than `within`, to
/// drag it; in a slice, to play it or carry it out; `shift` for no snapping to the hits.
fn down(at: f64, within: f64, shift: bool) {
    STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let Some(o) = s.sound.as_mut() else { return };
        let (from, to) = view(o);
        let long = (to - from).max(1e-9);
        let Some((slicing, made)) = now(o) else { return };
        let (at, within) = (from + at.clamp(0.0, 1.0) * long, within.max(0.0) * long);
        let on = match slicing.cut_near(at, within) {
            Some(cut) => On::Line { cut, moved: false },
            None => slicing
                .slice_at(at, made.audio.seconds())
                .map_or(On::Nothing, |slice| On::Slice { slice, carried: false }),
        };
        o.slice_press = Some(Press { at, within, shift, on });
    });
}

/// The mouse moved with the button down, to `at` (0 to 1 across what's shown); `far` once it has gone too far from
/// the press to be a click. A line follows it (to the hit nearby, unless `shift`); a slice is carried out of the app.
fn moved(app: &AppWindow, at: f64, shift: bool, far: bool) {
    enum Then {
        Nothing,
        Show,
        Carry((f64, f64)),
    }
    let then = STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let Some(o) = s.sound.as_mut() else { return Then::Nothing };
        let (from, to) = view(o);
        let Some((slicing, made)) = now(o) else { return Then::Nothing };
        let seconds = made.audio.seconds();
        let Some((on, within)) = o.slice_press.as_ref().map(|press| (press.on, press.within)) else {
            return Then::Nothing;
        };
        match on {
            On::Line { cut, moved } if moved || far => {
                let at = from + at.clamp(0.0, 1.0) * (to - from);
                let at = if shift {
                    at
                } else {
                    let hits: Vec<f64> = made.all_hits.iter().map(|(hit, _)| *hit).collect();
                    part::snap_to(&hits, at, within * SNAP_OVER_HOLD)
                };
                let next = slicing.moved(cut, at, seconds);
                if let Some(press) = o.slice_press.as_mut() {
                    press.on = On::Line { cut, moved: true };
                }
                let (Some(next), Some(sliced)) = (next, o.sliced.as_mut()) else { return Then::Nothing };
                sliced.history.live(next);
                Then::Show
            }
            On::Slice { slice, carried: false } if far => {
                let Some(span) = slicing.slice(slice, seconds) else { return Then::Nothing };
                if let Some(press) = o.slice_press.as_mut() {
                    press.on = On::Slice { slice, carried: true };
                }
                o.slice = Some(slice);
                Then::Carry(span)
            }
            _ => Then::Nothing,
        }
    });
    match then {
        Then::Nothing => {}
        Then::Show => super::show(app),
        Then::Carry(span) => {
            super::show(app);
            carry(app, Some(span));
        }
    }
}

/// The button came up (`cut_short`: the window lost the mouse). A line dragged stays where it is, one step for undo;
/// a click on a slice (or on its line) picks it and plays it; a slice carried that hadn't got going is let go.
fn up(app: &AppWindow, cut_short: bool) {
    enum Then {
        Nothing,
        Show,
        Play(usize),
        LetGo,
    }
    let then = STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let Some(o) = s.sound.as_mut() else { return Then::Nothing };
        let Some(press) = o.slice_press.take() else { return Then::Nothing };
        match press.on {
            On::Line { moved: true, .. } => {
                if let Some(sliced) = o.sliced.as_mut() {
                    sliced.history.end_live();
                }
                Then::Show
            }
            On::Line { cut: slice, moved: false } | On::Slice { slice, carried: false } if !cut_short => {
                Then::Play(slice)
            }
            On::Slice { carried: true, .. } => Then::LetGo,
            _ => Then::Nothing,
        }
    });
    match then {
        Then::Nothing => {}
        Then::Show => super::show(app),
        Then::Play(slice) => play(app, slice),
        Then::LetGo => crate::drag::let_go(app),
    }
}

/// Picks the slice `slice` (from 0) and plays it.
fn play(app: &AppWindow, slice: usize) {
    let span = STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let o = s.sound.as_mut()?;
        let (slicing, made) = now(o)?;
        let span = slicing.slice(slice, made.audio.seconds())?;
        o.slice = Some(slice);
        Some(span)
    });
    let Some((from, till)) = span else { return };
    super::show(app);
    play_edit(app, from, Some(till));
}

/// A double-click, where the press was: on a line, the line goes; elsewhere, there's a cut there (on the hit nearby,
/// unless Shift is held).
fn double(app: &AppWindow) {
    let changed = STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let Some(o) = s.sound.as_mut() else { return false };
        let Some((slicing, made)) = now(o) else { return false };
        let Some((at, within, shift)) = o.slice_press.as_ref().map(|press| (press.at, press.within, press.shift))
        else {
            return false;
        };
        let next = match slicing.cut_near(at, within) {
            Some(cut) => slicing.without_cut(cut),
            None => {
                let at = if shift {
                    at
                } else {
                    let hits: Vec<f64> = made.all_hits.iter().map(|(hit, _)| *hit).collect();
                    part::snap_to(&hits, at, within * SNAP_OVER_HOLD)
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
    });
    if !changed {
        return;
    }
    // The first of the two clicks played a slice that's another now.
    if PAGE.with(|p| p.borrow().play_edit) {
        super::super::stop(app);
    }
    super::show(app);
}

/// Whether a cut's line is within `within` of `at` (0 to 1 across what's shown): the mouse there takes hold of it.
fn near(at: f64, within: f64) -> bool {
    STRIP.with(|s| {
        let s = s.borrow();
        let Some(o) = s.sound.as_ref() else { return false };
        let Some((slicing, _)) = now(o) else { return false };
        let (from, to) = view(o);
        let long = (to - from).max(1e-9);
        slicing.cut_near(from + at.clamp(0.0, 1.0) * long, within.max(0.0) * long).is_some()
    })
}

/// Sends the slices: each a file of its own, saved in a folder for this slicing in Edited sounds (made now, or the
/// ones made before), then put on a Drum Machine in Bitwig, or their folder opened. On a thread of its own; a card
/// says how it went, and Send waits until it's done. Whether there were slices to send.
pub(super) fn send(app: &AppWindow, to: To) -> bool {
    let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return false };
    let here = HERE.with(|h| *h.borrow());
    let ready = STRIP.with(|s| {
        let mut s = s.borrow_mut();
        let s = &mut *s;
        if s.sending {
            return None;
        }
        let o = s.sound.as_ref()?;
        let (slicing, made) = now(o)?;
        let rate = f64::from(made.audio.sample_rate.max(1));
        let cuts: Vec<usize> = slicing.cuts.iter().map(|c| (c * rate).round() as usize).collect();
        let spans = edit_copy::slice_spans(&cuts, made.audio.len(), slicing.to_end);
        if spans.is_empty() {
            return None;
        }
        // The screenshots: nothing is written and nothing is sent.
        s.sending = !here;
        Some((
            o.id,
            o.source.clone(),
            made.audio.clone(),
            o.changed,
            format!("{:?}", made.edits),
            spans,
            o.name.clone(),
        ))
    });
    let Some((id, source, audio, changed, what, spans, name)) = ready else { return false };
    if here {
        return true;
    }
    let slices = to_bitwig::slices_text(spans.len());
    let job = match to {
        To::Bitwig => to_bitwig::slices_card(&feed, &name, spans.len()),
        To::Folder => feed.start(&format!("Saving {slices} of {name}"), ""),
    };
    super::show(app);
    let setting = PAGE.with(|p| p.borrow().kit.drum_machine.clone());
    let weak = app.as_weak();
    let started = background::spawn("Slices", move || {
        background::lower_priority();
        let saved = mo_catch::catch_crash(AssertUnwindSafe(|| {
            edit_copy::slices_to_keep(&source, &audio, changed, &what, &spans)
        }))
        .unwrap_or_else(|_| Err("Something went wrong: the log says what.".to_owned()));
        let went_in = match (saved, to) {
            (Err(why), _) => {
                log::warning("Sounds", &format!("Couldn't make the slices of {name}: {why}"));
                job.failed(&why);
                false
            }
            (Ok(files), To::Folder) => {
                let folder = files.first().and_then(|f| f.parent()).map(std::path::Path::to_path_buf);
                let named = folder
                    .as_ref()
                    .and_then(|f| f.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                log::info("Sounds", &format!("Saved {slices} of {name} in {named}"));
                job.done_as(&format!("Saved {slices} of {name}"), &format!("{named}, in {}", edit_copy::FOLDER));
                if let Some(folder) = folder {
                    crate::files::open_folder(&feed, &folder.to_string_lossy());
                }
                false
            }
            // A panic in it is logged as a crash and fails the card as it goes; Send still comes back.
            (Ok(files), To::Bitwig) => mo_catch::catch_crash(AssertUnwindSafe(|| {
                let pads = to_bitwig::slice_pads(&files);
                let bitwig = to_bitwig::Bitwig::default();
                to_bitwig::send_slices(job, &bitwig, &kit::KITS_TO_BITWIG, &name, &pads, setting.as_deref())
            }))
            .unwrap_or(false),
        };
        let _ = slint::invoke_from_event_loop(move || {
            STRIP.with(|s| s.borrow_mut().sending = false);
            if let Some(app) = weak.upgrade() {
                super::show(&app);
                // Its slices went into Bitwig: that counts towards how liked the sound is, as a send of it does.
                if went_in {
                    did::sent(&app, &[id]);
                }
            }
        });
    });
    // Its card was taken into the thread, so it fails by itself.
    if let Err(e) = started {
        log::error("Sounds", &format!("Couldn't start sending the slices: {e}"));
        STRIP.with(|s| s.borrow_mut().sending = false);
        super::show(app);
    }
    true
}

pub(super) fn wire(app: &AppWindow) {
    let data = app.global::<SoundEdit>();
    let weak = app.as_weak();
    data.on_cut_at_picked(move |choice| {
        if let Some(app) = weak.upgrade() {
            cut_at_picked(&app, &choice);
        }
    });
    let weak = app.as_weak();
    data.on_sensitivity_moved(move |to| {
        if let Some(app) = weak.upgrade() {
            sensitivity_moved(&app, to);
        }
    });
    let weak = app.as_weak();
    data.on_sensitivity_released(move |_| {
        if let Some(app) = weak.upgrade() {
            end_live(&app);
        }
    });
    let weak = app.as_weak();
    data.on_each_picked(move |choice| {
        if let Some(app) = weak.upgrade() {
            each_picked(&app, &choice);
        }
    });
    data.on_slice_down(|at, within, shift| down(f64::from(at), f64::from(within), shift));
    let weak = app.as_weak();
    data.on_slice_move(move |at, shift, far| {
        if let Some(app) = weak.upgrade() {
            moved(&app, f64::from(at), shift, far);
        }
    });
    let weak = app.as_weak();
    data.on_slice_up(move |cut_short| {
        if let Some(app) = weak.upgrade() {
            up(&app, cut_short);
        }
    });
    let weak = app.as_weak();
    data.on_slice_double(move || {
        if let Some(app) = weak.upgrade() {
            double(&app);
        }
    });
    data.on_slice_near(|at, within| near(f64::from(at), f64::from(within)));
}

/// The strip with no Slices on show.
pub(super) fn show_none(data: &SoundEdit) {
    data.set_slicing(false);
    data.set_cuts(ModelRc::default());
    data.set_pads(ModelRc::default());
    data.set_slice_from(0.0);
    data.set_slice_to(0.0);
    data.set_first_cut(0.0);
    data.set_can_send(false);
}

/// The Slices of the sound `o` as they are now (`sending`: slices are on their way, so Send waits).
pub(super) fn show(data: &SoundEdit, o: &Opened, sending: bool) {
    let Some(sliced) = o.sliced.as_ref().filter(|s| s.on) else {
        show_none(data);
        return;
    };
    let slicing = &sliced.history.now;
    let ready = now(o);
    data.set_slicing(true);
    data.set_sensitivity(slicing.sensitivity);
    data.set_cut_at_hits(slicing.cut_at == CutAt::Hits);
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
    let mut choices: Vec<SharedString> = vec![hits_line.into()];
    if o.bpm.is_some() {
        choices.extend(ON_BEATS.iter().map(|(name, _)| SharedString::from(*name)));
    }
    data.set_cut_at(cut_at.into());
    data.set_cut_at_choices(ModelRc::new(VecModel::from(choices)));
    let to_next = if slicing.cut_at == CutAt::Hits { TO_NEXT_HIT } else { TO_NEXT_CUT };
    data.set_each(if slicing.to_end { TO_END } else { to_next }.into());
    data.set_each_choices(ModelRc::new(VecModel::from(vec![SharedString::from(to_next), SharedString::from(TO_END)])));
    // Not sliced yet (the edit is still being made): no cuts to show or send.
    let Some((_, made)) = ready else {
        data.set_cuts(ModelRc::default());
        data.set_pads(ModelRc::default());
        data.set_slice_from(0.0);
        data.set_slice_to(0.0);
        data.set_first_cut(0.0);
        data.set_can_send(false);
        data.set_send_text("Send the slices".into());
        return;
    };
    let (from, to) = view(o);
    let long = (to - from).max(1e-9);
    let across = |t: f64| ((t - from) / long) as f32;
    // The cuts in sight, each with the room to the next line (its number shows where there's room for it).
    let shown: Vec<(usize, f64)> =
        slicing.cuts.iter().copied().enumerate().filter(|(_, cut)| *cut >= from && *cut <= to).collect();
    let cuts: Vec<SliceCut> = shown
        .iter()
        .enumerate()
        .map(|(k, &(i, cut))| {
            let next = shown.get(k + 1).map_or(to, |&(_, next)| next);
            SliceCut {
                at: across(cut),
                number: i32::try_from(i + 1).unwrap_or(i32::MAX),
                room: across(next) - across(cut),
            }
        })
        .collect();
    data.set_cuts(ModelRc::new(VecModel::from(cuts)));
    data.set_first_cut(slicing.cuts.first().map_or(0.0, |first| across(*first).clamp(0.0, 1.0)));
    let (slice_from, slice_to) =
        picked(o).map_or((0.0, 0.0), |(a, b)| (across(a).clamp(0.0, 1.0), across(b).clamp(0.0, 1.0)));
    data.set_slice_from(slice_from);
    data.set_slice_to(slice_to);
    let slices = slicing.cuts.len();
    let pads: Vec<SharedString> = (0..slices).map(|i| to_bitwig::slice_pad_name(i).into()).collect();
    data.set_pads(ModelRc::new(VecModel::from(pads)));
    data.set_can_send(slices > 0 && !sending);
    data.set_send_text(format!("Send {}", to_bitwig::slices_text(slices)).into());
    data.set_to_folder_text(
        if slices > 1 { format!("A numbered folder, 01 \u{2026} {slices:02}") } else { "A numbered folder".to_owned() }
            .into(),
    );
    // Where they're saved: the folder this slicing has, by its name.
    let rate = f64::from(made.audio.sample_rate.max(1));
    let frames: Vec<usize> = slicing.cuts.iter().map(|c| (c * rate).round() as usize).collect();
    let spans = edit_copy::slice_spans(&frames, made.audio.len(), slicing.to_end);
    let folder = edit_copy::slices_folder_name(&o.source, &format!("{:?}", made.edits), &spans);
    data.set_saved_in(format!("Saved in {}: {folder}", edit_copy::FOLDER).into());
}

/// The pads as chips along the strip's foot: "1 → C1", "2 → C#1"…, the first few and then the last; the picked
/// slice's is lit.
pub(super) fn pad_chips(o: &Opened) -> Vec<EditChip> {
    let slices = now(o).map_or(0, |(slicing, _)| slicing.cuts.len());
    let pad = |i: usize| format!("{} \u{2192} {}", i + 1, to_bitwig::slice_pad_name(i));
    let mut chips: Vec<EditChip> =
        (0..slices.min(PADS_NAMED)).map(|i| EditChip { text: pad(i).into(), last: o.slice == Some(i) }).collect();
    if slices > PADS_NAMED {
        let last = slices - 1;
        let lit = o.slice.is_some_and(|picked| picked >= PADS_NAMED);
        let text = match o.slice.filter(|_| lit) {
            // The one picked, when it's past those named.
            Some(picked) if picked < last => format!("\u{2026} {} \u{2026} {}", pad(picked), pad(last)),
            _ => format!("\u{2026} {}", pad(last)),
        };
        chips.push(EditChip { text: text.into(), last: lit });
    }
    chips
}

/// The strip's heading with the Slices on: "20 slices · 2 bars", and the slice picked ("· slice 5 is 0.23 s"). How
/// long the sound is, until it's sliced.
pub(super) fn heading(o: &Opened, seconds: f64) -> String {
    let Some((slicing, _)) = now(o) else { return part::length_text(seconds) };
    let mut text = format!("{} \u{b7} {}", to_bitwig::slices_text(slicing.cuts.len()), length_words(seconds, o.bpm));
    if let (Some(slice), Some((from, till))) = (o.slice, picked(o)) {
        let _ = write!(text, " \u{b7} slice {} is {}", slice + 1, part::length_text(till - from));
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
    part::length_text(seconds)
}

/// For the screenshots: how many cuts the sound's Slices have (none with them off), and the slice picked.
pub fn state_for_screens() -> (usize, Option<usize>) {
    STRIP.with(|s| {
        let s = s.borrow();
        let Some(o) = s.sound.as_ref() else { return (0, None) };
        (now(o).map_or(0, |(slicing, _)| slicing.cuts.len()), o.slice)
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mo_audio::AudioData;

    use super::super::{Edits, Source, make};
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

    #[test]
    fn the_foot_names_the_first_pads_and_the_last_and_the_heading_counts_the_slices() {
        let mut o = opened();
        // 120 BPM: two seconds is a bar.
        o.bpm = Some(120.0);
        slices_on(&mut o);
        let twenty = Slicing { cuts: (0..20).map(|i| f64::from(i) * 0.1).collect(), ..Slicing::default() };
        o.sliced.as_mut().unwrap().history.change(twenty);
        let texts = |o: &Opened| pad_chips(o).iter().map(|c| c.text.to_string()).collect::<Vec<_>>();
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
