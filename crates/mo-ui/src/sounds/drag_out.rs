//! Dragging sounds off the Sounds page into a DAW or a folder (`crate::drag` runs the drag): a tile, a line of any
//! list view, a pad of the kit builder or a "Sounds like this" line. Each goes out as its own file, where it is on
//! disk: never a copy, never moved (rule 2).
//!
//! The C# app counted a drag towards how liked a sound is. Here `sounds.db` is only read until the page is trusted, so
//! drags aren't written down yet.

use std::path::PathBuf;

use mo_app::drag_out::{self, Payload, SoundItem};
use mo_app::sounds as list;
use slint::ComponentHandle;

use super::{PAGE, kit};
use crate::drag::{self, Drag, Outcome};
use crate::{AppWindow, SoundsData, Theme};

/// Where the sound was picked up, so it's dimmed there while it's carried and glows there once it has landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot {
    /// A tile or a line, by its row in the view on show.
    Row(i32),
    /// A pad, by its place.
    Pad(i32),
    /// A "Sounds like this" line, by its sound's place in the library.
    Alike(i32),
}

pub(super) fn wire(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    let weak = app.as_weak();
    data.on_drag_row(move |row| {
        let Some(app) = weak.upgrade() else { return };
        let at = PAGE.with(|p| usize::try_from(row).ok().and_then(|r| p.borrow().entry(r)));
        if let Some(at) = at {
            carry(&app, at, Spot::Row(row));
        }
    });
    let weak = app.as_weak();
    data.on_drag_pad(move |pad| {
        let Some(app) = weak.upgrade() else { return };
        if let Some(at) = kit::pad_place(pad) {
            carry(&app, at, Spot::Pad(pad));
        }
    });
    let weak = app.as_weak();
    data.on_drag_alike(move |place| {
        let Some(app) = weak.upgrade() else { return };
        if let Ok(at) = usize::try_from(place) {
            carry(&app, at, Spot::Alike(place));
        }
    });
    let weak = app.as_weak();
    data.on_drag_let_go(move || {
        if let Some(app) = weak.upgrade() {
            drag::let_go(&app);
        }
    });
}

/// The sound at `at` in the library, as a drag carries it: its file and its line on the card.
fn item(app: &AppWindow, at: usize) -> Option<SoundItem> {
    PAGE.with(|p| {
        let p = p.borrow();
        let s = p.list.sound(at)?;
        let card = drag_out::sound_card(s, tint(app, list::type_colour(&s.sound_type)));
        Some(SoundItem { path: PathBuf::from(&s.path), card })
    })
}

/// For the screenshots: what dragging the tile or line at `row` of the view on show carries (its card), without
/// looking for its file. None when there's no sound there.
pub fn card_at_row(app: &AppWindow, row: i32) -> Option<Payload> {
    let at = PAGE.with(|p| usize::try_from(row).ok().and_then(|r| p.borrow().entry(r)))?;
    drag_out::sounds(vec![item(app, at)?])
}

/// Drags the sound at `at` in the library out of the app, picked up at `spot`.
fn carry(app: &AppWindow, at: usize, spot: Spot) {
    let Some(item) = item(app, at) else { return };
    let under_way = drag::start(
        app,
        Drag {
            make: Box::new(move || {
                // Looked for here, off the window's thread: the file may be on a drive that's slow to wake.
                if !item.path.is_file() {
                    return Err(format!(
                        "{} isn't on this computer any more, so there's nothing to drag.",
                        item.card.title
                    ));
                }
                drag_out::sounds(vec![item]).ok_or_else(|| "There's nothing to drag.".to_owned())
            }),
            done: Box::new(move |app, outcome| {
                lift(app, spot, false);
                if outcome == Outcome::Landed {
                    drag::glow(app, move |app, on| glow(app, spot, on));
                }
            }),
        },
    );
    if under_way {
        lift(app, spot, true);
    }
}

/// Dims the sound at `spot` while it's carried, or brings it back.
fn lift(app: &AppWindow, spot: Spot, on: bool) {
    let data = app.global::<SoundsData>();
    match spot {
        Spot::Row(i) => data.set_lifted_row(if on { i } else { -1 }),
        Spot::Pad(i) => data.set_lifted_pad(if on { i } else { -1 }),
        Spot::Alike(i) => data.set_lifted_alike(if on { i } else { -1 }),
    }
}

/// Lights the glow round the sound at `spot` once it has landed, or lets it fade.
fn glow(app: &AppWindow, spot: Spot, on: bool) {
    let data = app.global::<SoundsData>();
    match spot {
        Spot::Row(i) => data.set_landed_row(if on { i } else { -1 }),
        Spot::Pad(i) => data.set_landed_pad(if on { i } else { -1 }),
        Spot::Alike(i) => data.set_landed_alike(if on { i } else { -1 }),
    }
}

/// A sound type's colour (by its place in the theme's list, as `list::type_colour` gives it), as the card takes it:
/// 0xRRGGBB.
fn tint(app: &AppWindow, colour: i32) -> u32 {
    let c = app.global::<Theme>().invoke_sound_type(colour);
    u32::from_be_bytes([0, c.red(), c.green(), c.blue()])
}
