//! What you do with sounds that says you like them, kept as the C# app kept it (stage 3 of "the sound library grows
//! again", review finding X1-own1): a play you meant (a double-click, Space or the Play button, Ctrl+K's Play), a sound
//! dragged out that landed, and a sound or kit that went into Bitwig. Each goes into the app's own sound library on a
//! thread of its own ([`mo_app::sound_notes`]), never waiting here; meanwhile the page shows it at once: History lists
//! the play first, and the liked bars and why count it (every bar against the best liked, as the C# app rescored
//! them). The order Most liked sorts by, the galaxy's Light by liked and Ctrl+K catch up the next time the sounds are
//! read, as the C# app's order did (it scored them again but didn't sort again).
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
use mo_sounds::liking::LikedWeights;
use mo_sounds::{Sound, SoundActivity, event};
use slint::ComponentHandle;

use super::{HISTORY, HISTORY_LENGTH, PAGE, Page, liked_why, show_lists};
use crate::{AppWindow, SoundsData};

/// How long the app waits as it closes for what's been done to be kept: the library may be busy with its backup copy.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// The number of what's done when it isn't kept (for the screenshots, or with no writer): never in the library.
pub(super) const NOT_KEPT: u64 = u64::MAX;

/// Something done with a sound on the page, as it was noted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Noted {
    /// Its note's number ([`Noter::note`]), or [`NOT_KEPT`].
    number: u64,
    id: i64,
    kind: &'static str,
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
            "The app closed before the last plays, drags or sends, or changes to favourites, stars, types or tags, were \
             kept",
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

/// `kind` was done with the sounds `ids` just now: kept, and shown at once.
fn did(app: &AppWindow, ids: &[i64], kind: &'static str) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let number = p.noter.as_ref().and_then(|n| n.note(ids, kind)).unwrap_or(NOT_KEPT);
        take(&mut p, ids, kind, number, Local::now().naive_local());
    });
    shown_again(app);
}

/// The page takes `kind` done with the sounds `ids` (each once, and only those it lists), noted as `number`, `now`:
/// counted on their liked bars, and a play first in History (not while History is on show: the sound you played stays
/// under your pointer until you come back to it).
pub(super) fn take(p: &mut Page, ids: &[i64], kind: &'static str, number: u64, now: NaiveDateTime) {
    let mut taken = Vec::with_capacity(ids.len());
    for &id in ids {
        let Some(&at) = p.places.get(&id) else { continue };
        if taken.contains(&id) {
            continue;
        }
        taken.push(id);
        count(p.did.entry(id).or_default(), kind);
        p.noted.push(Noted { number, id, kind, at: now });
        if kind == event::PLAY {
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
}

/// The sounds were read again (`holds`: the notes what was read holds, when it was read with the writer held off):
/// what's been done since goes on showing, and what they hold already isn't counted twice. Without `holds` the
/// library's word is taken.
pub(super) fn read_again(p: &mut Page, holds: Option<&Holds>) {
    match holds {
        Some(holds) => p.noted.retain(|n| !holds.has(n.number)),
        None => p.noted.clear(),
    }
    p.did.clear();
    p.history_behind = None;
    let mut played = None;
    for n in &p.noted {
        let Some(&at) = p.places.get(&n.id) else { continue };
        count(p.did.entry(n.id).or_default(), n.kind);
        if n.kind == event::PLAY {
            let now = first(played.as_deref().unwrap_or(&p.played[..]), at, n.at);
            played = Some(now);
        }
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

/// One more of `kind` done.
fn count(a: &mut SoundActivity, kind: &str) {
    match kind {
        event::PLAY => a.plays += 1,
        event::DRAG => a.drags += 1,
        event::SEND => a.sends += 1,
        _ => {}
    }
}

/// The lists, the liked bars and the picked sound's card's why, with what was just done.
fn shown_again(app: &AppWindow) {
    show_lists(app);
    let data = app.global::<SoundsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        if let Some(at) = p.picked {
            let mut card = data.get_picked();
            card.liked_why = liked_why(&p, at).into();
            data.set_picked(card);
        }
    });
}

/// What's been done with the sound `s` since it was read, if anything.
pub(super) fn since<'a>(p: &'a Page, s: &Sound) -> Option<&'a SoundActivity> {
    p.did.get(&s.id)
}

/// The sound at `at`'s Most liked score with what's been done with it since it was read (and whether you made it a
/// favourite since: [`super::yours::liked_more`]), if scores are known yet.
pub(super) fn score_now(p: &Page, at: usize) -> Option<f64> {
    let score = p.list.liked_score(at)?;
    let more = p.list.sound(at).and_then(|s| since(p, s)).map_or(0.0, |d| more(&p.weights, d));
    Some(score + more + super::yours::liked_more(p, at))
}

/// The best liked sound's score now: one done with since, or made a favourite since, may have overtaken it, and the
/// best may be no favourite now.
pub(super) fn best_now(p: &Page) -> Option<f64> {
    let changed = |id: &i64| p.did.contains_key(id) || super::yours::favourite_changed(p, *id);
    let best = p.list.liked_best_but(|at| p.list.sound(at).is_some_and(|s| changed(&s.id)))?;
    let since = p.did.keys().chain(super::yours::favourites_changed(p));
    Some(since.filter_map(|id| score_now(p, *p.places.get(id)?)).fold(best, f64::max))
}

/// How much what's been done since (`d`, all of it this year) adds to a sound's Most liked score.
fn more(w: &LikedWeights, d: &SoundActivity) -> f64 {
    w.play * f64::from(d.plays) + w.drag * f64::from(d.drags) + w.send * f64::from(d.sends)
}
