//! What you do with sounds that says you like them, kept as the C# app kept it (stage 3 of "the sound library grows
//! again", review finding X1-own1): a play you meant (a double-click, Space or the Play button, Ctrl+K's Play), a sound
//! dragged out that landed, and a sound or kit that went into Bitwig. Each goes into the app's own sound library on a
//! thread of its own ([`mo_app::sound_notes`]), never waiting here; meanwhile History lists the play first at once.
//! The order Most liked sorts by, the galaxy's Light by liked and Ctrl+K count it the next time the sounds are read,
//! as the C# app's order did (it didn't sort again).
//!
//! The sounds read again while the app is open (the rest after the first few, or after a scan kept new ones) are read
//! with the writer held off ([`Noter::hold`]): they hold every note up to a number, and the page goes on showing those
//! after it ([`read_again`]).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use mo_app::log;
use mo_app::sound_notes::{Holds, Noter};
use mo_sounds::event;

use super::{HISTORY, HISTORY_LENGTH, PAGE, Page, show_lists};
use crate::AppWindow;

/// How long the app waits as it closes for what's been done to be kept: the library may be busy with its backup copy.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// The number of what's done when it isn't kept (for the screenshots, or with no writer): never in the library.
pub(super) const NOT_KEPT: u64 = u64::MAX;

/// A play on the page, as it was noted (only a play shows on the page: [`take`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Noted {
    /// Its note's number ([`Noter::note`]), or [`NOT_KEPT`].
    number: u64,
    id: i64,
    at: NaiveDateTime,
}

/// What's done with sounds from now on is kept in the app's own sound library beside `sounds_db`. The writer, for the
/// reads to hold off while they read ([`Noter::hold`]).
pub(super) fn connect(sounds_db: &Path) -> Arc<Noter> {
    let noter = Arc::new(Noter::start(sounds_db.to_path_buf()));
    PAGE.with(|p| p.borrow_mut().noter = Some(noter.clone()));
    noter
}

/// As the app closes: what's been done and said and not kept yet is kept now, waiting a moment at most.
pub fn finish_up() {
    let Some(noter) = PAGE.with(|p| p.borrow_mut().noter.take()) else { return };
    if !noter.finish_within(Some(CLOSE_WAIT)) {
        log::warning(
            "Sounds",
            "The app closed before the last plays, drags or sends, or changes to favourites, types or tags, were kept",
        );
    }
}

/// You played the sound at `at` in the list, meaning to (not a try-out: a single click, Autoplay, the arrow keys or the
/// galaxy's sweep): one more play, and History lists it first.
pub(super) fn played(app: &AppWindow, at: usize) {
    let id = PAGE.with(|p| p.borrow().list.sound(at).map(|s| s.id));
    if let Some(id) = id {
        did(app, &[id], event::PLAY);
    }
}

/// The sound with id `id` was dragged out of the app and landed (in a DAW, a folder or another app).
pub(super) fn dragged(app: &AppWindow, id: i64) {
    did(app, &[id], event::DRAG);
}

/// The sounds with ids `ids` went into Bitwig (a sound on a Sampler, or a kit on a Drum Machine).
pub(super) fn sent(app: &AppWindow, ids: &[i64]) {
    did(app, ids, event::SEND);
}

/// `kind` was done with the sounds `ids` just now: kept, and a play shown at once.
fn did(app: &AppWindow, ids: &[i64], kind: &'static str) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let number = p.noter.as_ref().and_then(|n| n.note(ids, kind)).unwrap_or(NOT_KEPT);
        take(&mut p, ids, kind, number, Local::now().naive_local());
    });
    // Only a play shows on the page ([`take`]).
    if kind == event::PLAY {
        show_lists(app);
    }
}

/// The page takes `kind` done with the sounds `ids` (each once, and only those it lists), noted as `number`, `now`.
/// Only a play shows on the page, first in History (not while History is on show: the sound you played stays under
/// your pointer until you come back to it); a drag or a send counts the next time the sounds are read.
pub(super) fn take(p: &mut Page, ids: &[i64], kind: &'static str, number: u64, now: NaiveDateTime) {
    if kind != event::PLAY {
        return;
    }
    let mut taken = Vec::with_capacity(ids.len());
    for &id in ids {
        let Some(&at) = p.places.get(&id) else { continue };
        if taken.contains(&id) {
            continue;
        }
        taken.push(id);
        p.noted.push(Noted { number, id, at: now });
        let played = first(&p.played, at, now);
        let was = std::mem::replace(&mut p.played, Arc::new(played));
        if p.view == HISTORY {
            p.history_behind.get_or_insert(was);
        } else {
            p.history = p.list.played(&p.played, &p.filters);
            p.history_behind = None;
        }
    }
}

/// The sounds were read again (`holds`: the notes what was read holds, when it was read with the writer held off):
/// the plays since go on showing first in History, and what they hold already isn't taken twice. Without `holds` the
/// library's word is taken.
pub(super) fn read_again(p: &mut Page, holds: Option<&Holds>) {
    match holds {
        Some(holds) => p.noted.retain(|n| !holds.has(n.number)),
        None => p.noted.clear(),
    }
    p.history_behind = None;
    let mut played = None;
    for n in &p.noted {
        let Some(&at) = p.places.get(&n.id) else { continue };
        played = Some(first(played.as_deref().unwrap_or(&p.played[..]), at, n.at));
    }
    if let Some(played) = played {
        p.played = Arc::new(played);
    }
}

/// `played` with the sound at `at`, played at `when`, first (and only there).
fn first(played: &[(usize, NaiveDateTime)], at: usize, when: NaiveDateTime) -> Vec<(usize, NaiveDateTime)> {
    let mut now = Vec::with_capacity(played.len() + 1);
    now.push((at, when));
    now.extend(played.iter().copied().filter(|&(i, _)| i != at));
    now.truncate(HISTORY_LENGTH);
    now
}
