//! Dragging presets and curves off the Presets page onto a track in a DAW, or into a folder (`crate::drag` runs the
//! drag; Solomon, 13 Nov 2026: a dragged preset only worked when it was a Bitwig preset, and "both presets and curves
//! should be draggable"): a line of either list, or the picked one by its card's handle.
//!
//! Bitwig only loads a preset dropped on a track when it's one of its own, so a preset goes as a Bitwig preset
//! wherever one can be made (made from a preset Bitwig saved, as Send to Bitwig makes one: `mo_presets::drag`), else
//! as its own file, for the plugin's window, another DAW or a folder. A curve goes as its curve file, made in the
//! app's Curves folder (X3-15). Nothing is ever written over: a made preset goes into a drag folder of its own in Temp,
//! which the app clears later (rule 28, `mo_app::drag_out`).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mo_app::drag_out::{self, Payload};
use mo_app::log;
use mo_app::presets::{self as logic, Item};
use mo_presets::bitwig::{self as bw, Templates};
use mo_presets::curves::{self, Curve};
use mo_presets::drag::{self as carried, Carried, File};
use mo_presets::library::Source;
use slint::ComponentHandle;

use super::{PAGE, export_setup, templates_now, without_template};
use crate::drag::{self, Drag, Outcome};
use crate::{AppWindow, PresetsData, Theme};

/// Where the preset or curve was picked up, so it's dimmed there while it's carried and glows there once it has landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot {
    /// A line of the sounds list, by its place among the lines shown.
    Row(i32),
    /// The picked sound's card, by its handle.
    Picked,
    /// A line of the curves list, by its place among the lines shown.
    CurveRow(i32),
    /// The picked curve's card, by its handle.
    CurvePicked,
}

pub(super) fn wire(app: &AppWindow) {
    let data = app.global::<PresetsData>();
    let weak = app.as_weak();
    data.on_drag_row(move |row| {
        let Some(app) = weak.upgrade() else { return };
        let at = PAGE.with(|p| usize::try_from(row).ok().and_then(|r| p.borrow().shown.get(r).copied()));
        if let Some(at) = at {
            carry_preset(&app, at, Spot::Row(row));
        }
    });
    let weak = app.as_weak();
    data.on_drag_picked(move || {
        let Some(app) = weak.upgrade() else { return };
        if let Some(at) = PAGE.with(|p| p.borrow().picked) {
            carry_preset(&app, at, Spot::Picked);
        }
    });
    let weak = app.as_weak();
    data.on_curve_drag_row(move |row| {
        let Some(app) = weak.upgrade() else { return };
        let curve = PAGE.with(|p| {
            let p = p.borrow();
            usize::try_from(row).ok().and_then(|r| p.curve_shown.get(r)).and_then(|&at| p.curves.get(at)).cloned()
        });
        if let Some(curve) = curve {
            carry_curve(&app, curve, Spot::CurveRow(row));
        }
    });
    let weak = app.as_weak();
    data.on_curve_drag(move || {
        let Some(app) = weak.upgrade() else { return };
        let curve = PAGE.with(|p| {
            let p = p.borrow();
            p.curve_picked.as_deref().and_then(|id| p.curves.iter().find(|c| c.id == id)).cloned()
        });
        if let Some(curve) = curve {
            carry_curve(&app, curve, Spot::CurvePicked);
        }
    });
    let weak = app.as_weak();
    data.on_drag_let_go(move || {
        if let Some(app) = weak.upgrade() {
            drag::let_go(&app);
        }
    });
    let weak = app.as_weak();
    data.on_curve_drag_let_go(move || {
        if let Some(app) = weak.upgrade() {
            drag::let_go(&app);
        }
    });
}

/// The line at `at` in the page's list, as it shows (a part fix made since it was read laid over it), and the colour
/// its line's dot is drawn in, as the card takes it.
fn item(app: &AppWindow, at: usize) -> Option<(Item, u32)> {
    let item = PAGE.with(|p| {
        let p = p.borrow();
        p.items.get(at).map(|i| i.as_fixed(&p.fixed_now).into_owned())
    })?;
    let tint = tint(app, logic::line(&item).colour);
    Some((item, tint))
}

/// For the screenshots: what dragging the line at `row` of the list shown carries (its card), without reading or
/// writing anything: a Bitwig preset where its plugin's format can be one. None when there's no line there.
pub fn card_at_row(app: &AppWindow, row: usize) -> Option<Payload> {
    let at = PAGE.with(|p| p.borrow().shown.get(row).copied())?;
    let (item, tint) = item(app, at)?;
    let bitwig = !bw::state_format(&item.row.extension).is_empty();
    let name = format!("{}{}", item.row.name, if bitwig { ".bwpreset" } else { item.row.extension.as_str() });
    let as_if = Carried { file: File::Made { name, bytes: Vec::new() }, bitwig, why_not: None };
    let out = drag_out::preset(as_if, &item.row.name, &item.row.plugin, tint);
    Some(Payload { files: Vec::new(), cards: out.cards, caption: out.caption, numbered: false })
}

/// For the screenshots: what dragging the curve at `row` of the list shown carries (its card), without writing its
/// file. None when there's no curve there.
pub fn curve_card_at_row(row: usize) -> Option<Payload> {
    let card = PAGE.with(|p| {
        let p = p.borrow();
        p.curve_shown.get(row).and_then(|&at| p.curves.get(at)).map(drag_out::curve_card)
    })?;
    Some(Payload { files: Vec::new(), cards: vec![card], caption: "1 curve".into(), numbered: false })
}

/// Drags the preset at `at` in the page's list out of the app, picked up at `spot`: made on the drag's own thread,
/// as a Bitwig preset where one can be made ([`carried::from_save`], [`carried::from_file`]).
fn carry_preset(app: &AppWindow, at: usize, spot: Spot) {
    let Some((item, tint)) = item(app, at) else { return };
    let (_, templates, bitwig_folders, _) = export_setup();
    let under_way = drag::start(
        app,
        Drag {
            make: Box::new(move || {
                let carried = carry_of(&item, templates, &bitwig_folders)?;
                if let Some(why) = &carried.why_not {
                    log::info(
                        "Presets",
                        &format!("{} went out as it is, not as a Bitwig preset: {why}", item.row.name),
                    );
                }
                drag_out::preset(carried, &item.row.name, &item.row.plugin, tint)
                    .write(&std::env::temp_dir())
                    .map_err(|e| format!("Couldn't make the file to drag for {}: {e}.", item.row.name))
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

/// What dragging `item` carries. The Bitwig presets to make one from are the page's own, looked for again when they
/// don't have one for its plugin's format, so "save a preset in Bitwig, then try again" works without a restart, as
/// for an export (H-2). An error is what to tell the user instead.
fn carry_of(item: &Item, templates: Option<Arc<Templates>>, bitwig_folders: &[PathBuf]) -> Result<Carried, String> {
    // Only a VST2 or VST3 preset can be made a Bitwig one: for anything else there's nothing to look for.
    let templates = (!bw::state_format(&item.row.extension).is_empty())
        .then(|| templates_now(templates, bitwig_folders, |t| without_template(&[item], t).is_empty()));
    let name = &item.row.name;
    match &item.row.source {
        Source::File { path } => {
            carried::from_file(Path::new(path), &item.row.plugin, templates.as_deref()).map_err(|e| match e.kind() {
                ErrorKind::NotFound => format!("{name} isn't on this computer any more, so there's nothing to drag."),
                _ => format!("{name} couldn't be read to drag it: {e}."),
            })
        }
        Source::Song { save, key } => carried::from_save(save, key, templates.as_deref())
            .map_err(|e| format!("{name} couldn't be read from its song again: {e}")),
    }
}

/// Drags `curve` out of the app as its curve file (made in the app's Curves folder on the drag's own thread), onto an
/// automation lane in Bitwig, picked up at `spot`.
fn carry_curve(app: &AppWindow, curve: Curve, spot: Spot) {
    let folder = PAGE.with(|p| p.borrow().data_dir.join(curves::FOLDER));
    let under_way = drag::start(
        app,
        Drag {
            make: Box::new(move || Ok(drag_out::curve(curves::file(&curve, &folder)?, drag_out::curve_card(&curve)))),
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

/// Dims what's carried at `spot`, or brings it back.
fn lift(app: &AppWindow, spot: Spot, on: bool) {
    let data = app.global::<PresetsData>();
    match spot {
        Spot::Row(i) => data.set_lifted_row(if on { i } else { -1 }),
        Spot::Picked => data.set_lifted_picked(on),
        Spot::CurveRow(i) => data.set_curve_lifted_row(if on { i } else { -1 }),
        Spot::CurvePicked => data.set_curve_lifted(on),
    }
}

/// Lights the glow round what's carried at `spot` once it has landed, or lets it fade.
fn glow(app: &AppWindow, spot: Spot, on: bool) {
    let data = app.global::<PresetsData>();
    match spot {
        Spot::Row(i) => data.set_landed_row(if on { i } else { -1 }),
        Spot::Picked => data.set_landed_picked(on),
        Spot::CurveRow(i) => data.set_curve_landed_row(if on { i } else { -1 }),
        Spot::CurvePicked => data.set_curve_landed(on),
    }
}

/// A part's colour (by its place in the theme's list, as the line's dot takes it), as the card takes it: 0xRRGGBB.
fn tint(app: &AppWindow, colour: i32) -> u32 {
    let c = app.global::<Theme>().invoke_sound_type(colour);
    u32::from_be_bytes([0, c.red(), c.green(), c.blue()])
}
