//! Your say about sounds, on the picked sound's card (stage 5 of "the sound library grows again", review finding
//! X1-own1): a favourite (its heart, or Ctrl+D), your own type, and tags (added in the card's box, or Ctrl+T; taken
//! away by their ×), as the C# app's Sound Lab kept them; not its stars, as the heart is the one way to like a sound
//! (Solomon's pick, 10 Nov 2026). Each goes into the app's own sound library through the one writer of what's done
//! with sounds ([`mo_app::sound_notes`]), never waiting here. Meanwhile the page shows it at once, over the sounds as
//! they were read ([`SoundList::with_say`]): on the tiles, lines and card, in the filters and their counts, in the
//! type order and the search. What Most liked's order, Rediscover, Kits, the galaxy's colours and Ctrl+K make of it
//! catches up the next time the sounds are read.
//!
//! The sounds read again while the app is open hold every change up to a number (they're read with the writer held
//! off, as for plays: [`super::did`]), and the page goes on showing those after it ([`read_again`]).

use std::collections::HashMap;
use std::sync::Arc;

use mo_app::sound_notes::Holds;
use mo_app::sounds::{Now, SoundList, YourSay, by_letters};
use mo_sounds::compact::yours::Change;
use mo_sounds::{Sound, Word, types};
use slint::ComponentHandle;

use super::did::NOT_KEPT;
use super::{PAGE, Page};
use crate::{AppWindow, SoundsData};

/// One change you made, as it was noted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Said {
    /// Its note's number ([`mo_app::sound_notes::Noter::note_yours`]), or [`NOT_KEPT`].
    number: u64,
    id: i64,
    change: Change,
}

/// The types you can give a sound, for the card's Your type menu: every type the app knows, as the C# app's were.
pub(super) fn choices() -> Vec<&'static str> {
    types::all().filter(|t| *t != types::UNKNOWN).collect()
}

/// The picked sound a favourite, or not.
pub(super) fn favourite(app: &AppWindow) {
    change(app, |now| Some(Change::Favourite(!now.favourite)));
}

/// Your own type for the picked sound: `own` ("" takes yours away, so its name's type shows again).
pub(super) fn own_type(app: &AppWindow, own: &str) {
    let own = own.trim();
    change(app, |now| {
        let own = (!own.is_empty()).then(|| own.to_owned());
        (now.own_type != own.as_deref()).then_some(Change::Type(own))
    });
}

/// A tag added to the picked sound. One it has already (in any capitals) isn't added twice, but it's noted all the
/// same: one the C# app's model suggested becomes yours.
pub(super) fn tag(app: &AppWindow, tag: &str) {
    let tag = tag.trim();
    change(app, |_| (!tag.is_empty()).then(|| Change::Tag(tag.to_owned())));
}

/// A tag taken away from the picked sound.
pub(super) fn untag(app: &AppWindow, tag: &str) {
    let tag = tag.trim();
    change(app, |now| has(now.tags, tag).then(|| Change::Untag(tag.to_owned())));
}

/// Whether `tags` hold `tag`, in any capitals (as the Tag filter matches them).
fn has(tags: &[String], tag: &str) -> bool {
    tags.iter().any(|t| t.eq_ignore_ascii_case(tag))
}

/// The change `what` makes to the picked sound as it is now (if any): kept, and shown at once.
fn change(app: &AppWindow, what: impl FnOnce(Now) -> Option<Change>) {
    let changed = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let now = p.list.now(p.picked?)?;
        let id = now.sound.id;
        let change = what(now)?;
        let number = p.noter.as_ref().and_then(|n| n.note_yours(id, change.clone())).unwrap_or(NOT_KEPT);
        take(&mut p, id, change, number);
        Some(())
    });
    if changed.is_some() {
        shown_again(app);
    }
}

/// The lists, the filters and the picked sound's card, with what you just said: the tiles and lines at once, and the
/// list as the filters see it now (a sound no longer a favourite leaves Favourites) once it's worked out again.
pub(super) fn shown_again(app: &AppWindow) {
    super::show_lists(app);
    let data = app.global::<SoundsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        if let Some(at) = p.picked {
            data.set_picked(super::picked_sound(&p, at));
            super::show_picked_tags(&data, &p);
        }
    });
    super::refresh(app);
}

/// The page takes `change` to the sound `id` (only one it lists), noted as `number`: over the sound as it was read,
/// from now on.
pub(super) fn take(p: &mut Page, id: i64, change: Change, number: u64) {
    if !p.places.contains_key(&id) {
        return;
    }
    known(p, &change);
    p.said.push(Said { number, id, change });
    keep_showing(p);
}

/// The sounds were read again (`holds`: the notes what was read holds, when it was read with the writer held off):
/// your say since, or not kept yet, goes on showing, and what they hold already is taken from them. Without `holds`
/// the library's word is taken.
pub(super) fn read_again(p: &mut Page, holds: Option<&Holds>) {
    match holds {
        Some(holds) => p.said.retain(|s| !holds.has(s.number)),
        None => p.said.clear(),
    }
    for s in std::mem::take(&mut p.said) {
        known(p, &s.change);
        p.said.push(s);
    }
    keep_showing(p);
}

/// The page's list with your say (`p.said`) over its sounds: after it changes, and after the list was made again (with
/// the use counts in, say).
pub(super) fn keep_showing(p: &mut Page) {
    if p.said.is_empty() && !p.list.has_say() {
        return;
    }
    let say = Arc::new(say_of(&p.list, &p.places, &p.said));
    p.list = Arc::new(p.list.with_say(say));
}

/// Each sound's say, from `said` in order, over the sound as `list` holds it (at its place in `places`).
fn say_of(list: &SoundList, places: &HashMap<i64, usize>, said: &[Said]) -> HashMap<i64, YourSay> {
    let mut say: HashMap<i64, YourSay> = HashMap::new();
    for s in said {
        let Some(sound) = places.get(&s.id).and_then(|&at| list.sound(at)) else { continue };
        add(say.entry(s.id).or_default(), sound, &s.change);
    }
    say
}

/// `change` over `say` (what's been said about `sound` before it).
fn add(say: &mut YourSay, sound: &Sound, change: &Change) {
    match change {
        Change::Favourite(on) => say.favourite = Some(*on),
        // The page gives no stars: the heart is the one way to like a sound.
        Change::Stars(_) => {}
        Change::Type(own) => {
            say.own_type = Some(own.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(Word::new));
        }
        Change::Tag(tag) => {
            let tags = say.tags.get_or_insert_with(|| sound.tags.clone());
            let tag = tag.trim();
            if !tag.is_empty() && !has(tags, tag) {
                tags.push(tag.to_owned());
                // As the library sorts them (byte by byte), so the card's chips don't move when it's read again.
                tags.sort_unstable();
            }
        }
        Change::Untag(tag) => {
            let tag = tag.trim();
            say.tags.get_or_insert_with(|| sound.tags.clone()).retain(|t| !t.eq_ignore_ascii_case(tag));
        }
    }
}

/// A type or tag `change` brings that the filters don't know yet joins their choices (at the end of the types, among
/// the tags in order). One no sound has any more stays until the sounds are read again, counting none.
fn known(p: &mut Page, change: &Change) {
    match change {
        Change::Type(Some(own)) => {
            let own = own.trim();
            if !own.is_empty() && !p.types.iter().any(|(t, _)| t.eq_ignore_ascii_case(own)) {
                p.types.push((own.to_owned(), 1));
            }
        }
        Change::Tag(tag) => {
            let tag = tag.trim();
            if !tag.is_empty() && !has(&p.tags, tag) {
                let at = p.tags.partition_point(|t| by_letters(t, tag).is_lt());
                p.tags.insert(at, tag.to_owned());
            }
        }
        _ => {}
    }
}
