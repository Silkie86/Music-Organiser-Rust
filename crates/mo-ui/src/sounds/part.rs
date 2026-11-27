//! A part picked on the picked sound's card (the 1.1 plan's light sample edits): a press and drag across its waveform
//! picks one, its edges snapping to the sound's hits (Shift: no snap). The part is what Play plays, and what the card's
//! handle carries out of the app (Ctrl as it's pressed: the whole file) as a copy in the export folder's
//! `Edited sounds` ([`mo_app::edit_copy`]); a press inside it that moves carries it out too. Esc, a click outside it
//! or picking another sound lets it go. The original is never changed (rule 2).
//!
//! The hits are found once the mouse comes over the waveform (or presses on it), on a thread of their own (rule 12),
//! and kept for the last few sounds.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use mo_app::background::Newest;
use mo_app::{edit_copy, log};
use mo_audio::hits::USUAL_RISE_DB;
use slint::{ComponentHandle, SharedString};

use super::PAGE;
use crate::{AppWindow, SoundsData};

/// How many sounds' hits are kept: a few hundred places each at the most.
const KEEP: usize = 16;

/// A part picked: whose (the sound's id), and from and to, 0 to 1 of the way through its waveform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Part {
    pub id: i64,
    pub from: f64,
    pub to: f64,
}

/// The hits of a sound to find: whose, from which file.
struct Job {
    id: i64,
    path: PathBuf,
    window: slint::Weak<AppWindow>,
}

thread_local! {
    static PART: Cell<Option<Part>> = const { Cell::new(None) };
    /// The sound whose waveform was last pressed on: a part picked on it goes to it alone, not to another sound picked
    /// while the button was still down (by the arrow keys).
    static PRESSED: Cell<Option<i64>> = const { Cell::new(None) };
    /// Each sound's hits found, by id, as places from 0 to 1 of the way through it (those that rise by
    /// [`USUAL_RISE_DB`] or more), the newest last.
    static KEPT: RefCell<VecDeque<(i64, Arc<[f64]>)>> = const { RefCell::new(VecDeque::new()) };
    /// The sound whose hits were asked for last, so the mouse coming over its waveform again doesn't ask again.
    static ASKED: Cell<Option<i64>> = const { Cell::new(None) };
    /// The thread that finds them, started at the first ask.
    static READER: RefCell<Option<Newest<Job>>> = const { RefCell::new(None) };
}

pub(super) fn wire(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    let weak = app.as_weak();
    data.on_pick_part(move |from, to| {
        if let Some(app) = weak.upgrade() {
            pick(&app, f64::from(from), f64::from(to));
        }
    });
    data.on_snap_part(snap);
    let weak = app.as_weak();
    data.on_wants_hits(move || {
        if let Some(app) = weak.upgrade() {
            ask_hits(&app);
        }
    });
    let weak = app.as_weak();
    data.on_part_pressed(move || {
        PRESSED.set(picked().map(|(id, _)| id));
        if let Some(app) = weak.upgrade() {
            ask_hits(&app);
        }
    });
}

/// The picked sound's id, and how long it lasts (0 when that isn't known yet).
fn picked() -> Option<(i64, f64)> {
    PAGE.with(|p| {
        let p = p.borrow();
        let s = p.picked.and_then(|at| p.list.sound(at))?;
        Some((s.id, s.analysis.as_ref().map_or(0.0, |a| a.seconds)))
    })
}

/// The part picked on the sound `id`'s card, if one is.
pub(super) fn of(id: i64) -> Option<Part> {
    PART.get().filter(|p| p.id == id)
}

/// The part picked on the picked sound's card, in seconds into it, if one is.
pub(super) fn picked_seconds() -> Option<(f64, f64)> {
    let (id, seconds) = picked()?;
    let part = of(id)?;
    Some((part.from * seconds, part.to * seconds))
}

/// Picks the part of the picked sound from `from` to `to` (0 to 1 of the way through), or lets it go when it's too
/// short to be one (`pick-part(0, 0)`). Only on the sound whose waveform was pressed on: another sound picked while the
/// button was down keeps what it had.
fn pick(app: &AppWindow, from: f64, to: f64) {
    let Some((id, seconds)) = picked() else { return };
    if PRESSED.get() != Some(id) {
        return;
    }
    let (from, to) = (from.min(to).clamp(0.0, 1.0), from.max(to).clamp(0.0, 1.0));
    let part = ((to - from) * seconds >= edit_copy::LEAST_PART_SECONDS).then_some(Part { id, from, to });
    PART.set(part);
    show(app);
    super::edit_strip::card_part_changed(app);
}

/// Lets the part go. Whether one was on show on the card (Esc's step): a part of a sound that's no longer picked (the
/// sounds were read again, say) goes without a word.
pub(super) fn let_go(app: &AppWindow) -> bool {
    let shown = picked().and_then(|(id, _)| of(id)).is_some();
    if PART.take().is_some() {
        show(app);
        super::edit_strip::card_part_changed(app);
    }
    shown
}

/// Forgets the part picked, with no sound picked any more (the sounds were read again).
pub(super) fn forget() {
    PART.set(None);
}

/// The sound `id` has been picked: another sound's part goes, and its hits show if they've been found.
pub(super) fn sound_picked(app: &AppWindow, id: i64) {
    if PART.get().is_some_and(|p| p.id != id) {
        PART.set(None);
    }
    show(app);
}

/// Shows the part picked (none, unless it's the picked sound's) and the picked sound's hits on its card.
fn show(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    let picked = picked();
    let part = picked.and_then(|(id, seconds)| of(id).map(|p| (p, seconds)));
    match part {
        Some((p, seconds)) => {
            data.set_part_from(p.from as f32);
            data.set_part_to(p.to as f32);
            data.set_part_text(length_text((p.to - p.from) * seconds).into());
        }
        None => {
            data.set_part_from(0.0);
            data.set_part_to(0.0);
            data.set_part_text(SharedString::new());
        }
    }
    let hits = picked.and_then(|(id, _)| kept(id));
    data.set_part_hits(hits.map_or_else(SharedString::new, |h| marks(&h).into()));
}

/// How long a part lasts, as the card's handle says it: "0.47 s", "12.3 s", "1:05". Rounded before the way it's
/// written is chosen, so 9.996 s reads "10.0 s" and 59.96 s "1:00".
pub(super) fn length_text(seconds: f64) -> String {
    let hundredths = (seconds * 100.0).round() / 100.0;
    let tenths = (seconds * 10.0).round() / 10.0;
    if tenths >= 60.0 {
        let whole = seconds.round() as u64;
        format!("{}:{:02}", whole / 60, whole % 60)
    } else if hundredths >= 10.0 {
        format!("{tenths:.1} s")
    } else {
        format!("{hundredths:.2} s")
    }
}

/// The hits at `at` (0 to 1) as short lines along a waveform's foot, a path 1000 across and 10 high. Hits closer than
/// a few tenths of a percent of the width to the last one shown are left out: on the card they'd be one mark.
pub(super) fn marks(at: &[f64]) -> String {
    let mut path = String::new();
    let mut last = f64::NEG_INFINITY;
    for &a in at {
        let x = a * 1000.0;
        if x - last < 4.0 {
            continue;
        }
        last = x;
        let _ = write!(path, "M {x:.1} 10 L {x:.1} 0 ");
    }
    path
}

/// `s`'s hits, once found.
fn kept(id: i64) -> Option<Arc<[f64]>> {
    KEPT.with(|k| k.borrow().iter().find(|(i, _)| *i == id).map(|(_, at)| Arc::clone(at)))
}

/// Keeps `at` as the sound `id`'s hits, the newest, letting the oldest go past [`KEEP`].
fn keep(id: i64, at: Arc<[f64]>) {
    KEPT.with(|k| {
        let mut k = k.borrow_mut();
        k.retain(|(i, _)| *i != id);
        k.push_back((id, at));
        while k.len() > KEEP {
            k.pop_front();
        }
    });
}

/// Where a part's edge goes with the mouse at `at` (0 to 1 of the way through the picked sound): on the nearest of its
/// hits no further away than `within`, else at `at` (and always with `within` 0: Shift).
fn snap(at: f32, within: f32) -> f32 {
    let Some(hits) = picked().and_then(|(id, _)| kept(id)) else { return at };
    snap_to(&hits, f64::from(at), f64::from(within)) as f32
}

/// [`snap`]'s work, on hits `at` (0 to 1, in order).
pub(super) fn snap_to(hits: &[f64], at: f64, within: f64) -> f64 {
    if within <= 0.0 || !at.is_finite() {
        return at;
    }
    hits.iter()
        .copied()
        .filter(|h| (h - at).abs() <= within)
        .min_by(|a, b| (a - at).abs().total_cmp(&(b - at).abs()))
        .unwrap_or(at)
}

/// Finds the picked sound's hits, unless they're kept or being found, or it's too long to have them
/// ([`edit_copy::HITS_MOST_SECONDS`]).
fn ask_hits(app: &AppWindow) {
    let Some((id, seconds)) = picked() else { return };
    if seconds <= 0.0 || seconds > edit_copy::HITS_MOST_SECONDS || kept(id).is_some() || ASKED.get() == Some(id) {
        return;
    }
    let Some(path) = PAGE.with(|p| {
        let p = p.borrow();
        p.picked.and_then(|at| p.list.sound(at)).map(|s| PathBuf::from(&s.path))
    }) else {
        return;
    };
    // The screenshots have no event loop to send them back with.
    if super::HERE.with(|h| *h.borrow()) {
        return;
    }
    ASKED.set(Some(id));
    let job = Job { id, path, window: app.as_weak() };
    READER.with(|r| {
        let mut r = r.borrow_mut();
        if r.is_none() {
            match Newest::start("Find hits", find) {
                Ok(reader) => *r = Some(reader),
                Err(e) => {
                    log::warning("Sounds", &format!("Couldn't start finding a sound's hits: {e}"));
                    ASKED.set(None);
                    return;
                }
            }
        }
        if let Some(reader) = r.as_ref() {
            reader.ask(job);
        }
    });
}

/// Finds a sound's hits, on the reader's thread, and hands them to the card.
fn find(job: Job) {
    let Job { id, path, window } = job;
    let at: Arc<[f64]> = match edit_copy::hits_of(&path) {
        Ok(hits) if hits.seconds > 0.0 => hits
            .at
            .iter()
            .filter(|(_, rise)| *rise >= USUAL_RISE_DB)
            .map(|(at, _)| (at / hits.seconds).clamp(0.0, 1.0))
            .collect(),
        Ok(_) => Arc::default(),
        Err(why) => {
            // Kept as none, so it isn't read again and again: the part is picked without snapping.
            log::warning("Sounds", &format!("Couldn't find the hits: {why}"));
            Arc::default()
        }
    };
    let _ = slint::invoke_from_event_loop(move || {
        keep(id, at);
        if ASKED.get() == Some(id) {
            ASKED.set(None);
        }
        if let Some(app) = window.upgrade() {
            show(&app);
        }
    });
}

/// For the screenshots: the picked sound's hits, at `at` (0 to 1), as if they'd been found.
pub fn hits_for_screens(app: &AppWindow, at: &[f64]) {
    if let Some((id, _)) = picked() {
        keep(id, at.into());
        show(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edge_snaps_to_the_nearest_hit_within_reach_only() {
        let hits = [0.1, 0.25, 0.3];
        assert_eq!(snap_to(&hits, 0.27, 0.05), 0.25);
        assert_eq!(snap_to(&hits, 0.29, 0.05), 0.3);
        assert_eq!(snap_to(&hits, 0.5, 0.05), 0.5, "none near");
        assert_eq!(snap_to(&hits, 0.27, 0.0), 0.27, "Shift: no snap");
        assert_eq!(snap_to(&[], 0.27, 0.05), 0.27);
    }

    #[test]
    fn a_parts_length_reads_short() {
        assert_eq!(length_text(0.46875), "0.47 s");
        assert_eq!(length_text(12.34), "12.3 s");
        assert_eq!(length_text(65.2), "1:05");
        // Rounded first, then written.
        assert_eq!(length_text(9.996), "10.0 s");
        assert_eq!(length_text(59.96), "1:00");
        assert_eq!(length_text(59.94), "59.9 s");
    }

    #[test]
    fn marks_close_together_are_one() {
        assert_eq!(marks(&[0.1, 0.1005, 0.5]), "M 100.0 10 L 100.0 0 M 500.0 10 L 500.0 0 ");
        assert_eq!(marks(&[]), "");
    }
}
