//! The Sounds page's kit builder: random kits that still make sense (`mo_sounds::kits`), rolled and exported off the
//! window's thread. Rolling only reads the library; exporting copies the pads' samples into a new folder of its own
//! (the samples themselves are only read, rule 2).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use chrono::Local;
use mo_app::background::{self, Newest};
use mo_app::log;
use mo_app::sounds::{self as list, SoundList};
use mo_sounds::index::SoundIndex;
use mo_sounds::kits::{self, ExportOptions, KitSlot};
use mo_sounds::music::NOTE_NAMES;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::{FEED, HERE, PAGE, Page, pick, play_picked, strings};
use crate::{AppWindow, PadLine, SoundsData};

/// The Randomness choices, for the C# app's slider from 0 (the best fits) to 1 (anything that fits).
const RANDOMNESS: [(&str, f64); 4] = [("Best fits", 0.1), ("Some", 0.5), ("Lots", 0.75), ("Anything", 1.0)];

/// The kit builder's part of the page.
pub(super) struct Kit {
    pub(super) slots: Vec<KitSlot>,
    pub(super) options: kits::Options,
    pub(super) export: ExportOptions,
    /// The seed of the kit on show, for its name.
    pub(super) seed: i32,
    /// Counts the rolls asked for, so an older one that arrives late is dropped.
    pub(super) asked: u64,
    pub(super) busy: bool,
    pub(super) status: String,
    /// The C# app's export folder setting, read from `library.db` (kits go in its Kits folder).
    pub(super) export_folder: Option<String>,
}

impl Default for Kit {
    fn default() -> Kit {
        Kit {
            slots: kits::default_drum_kit(),
            options: kits::Options::default(),
            export: ExportOptions::default(),
            seed: 0,
            asked: 0,
            busy: false,
            status: "Roll a kit to fill the pads.".to_owned(),
            export_folder: None,
        }
    }
}

thread_local! {
    static PADS: Rc<VecModel<PadLine>> = Rc::new(VecModel::default());
    static ROLLING: RefCell<Option<Newest<RollAsk>>> = const { RefCell::new(None) };
}

/// A roll: everything its thread needs. `only` rolls just that pad, keeping the others.
struct RollAsk {
    id: u64,
    list: Arc<SoundList>,
    index: Arc<SoundIndex>,
    slots: Vec<KitSlot>,
    options: kits::Options,
    only: Option<usize>,
}

struct Rolled {
    id: u64,
    slots: Vec<KitSlot>,
    seed: i32,
    only: Option<usize>,
}

impl RollAsk {
    fn answer(self) -> Rolled {
        let empty = std::collections::HashMap::new();
        let usage = self.list.usage_map().unwrap_or(&empty);
        let mut slots = self.slots;
        // One pad: every other filled pad is held still for this roll only.
        let held: Vec<bool> = slots.iter().map(|s| s.locked).collect();
        if let Some(only) = self.only {
            for (i, s) in slots.iter_mut().enumerate() {
                s.locked = i != only;
            }
            if let Some(s) = slots.get_mut(only) {
                s.locked = false;
            }
        }
        let seed =
            background::on_pool(|| kits::roll(self.list.sounds(), usage, &self.index, &mut slots, &self.options));
        for (s, locked) in slots.iter_mut().zip(held) {
            s.locked = locked;
        }
        Rolled { id: self.id, slots, seed, only: self.only }
    }
}

/// Wires the kit builder's buttons and chips.
pub(super) fn wire(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    data.set_pads(ModelRc::from(PADS.with(|m| m.clone()) as Rc<dyn Model<Data = PadLine>>));
    data.set_kit_keys(strings(NOTE_NAMES));
    data.set_kit_randomnesses(strings(RANDOMNESS.map(|r| r.0)));
    let weak = app.as_weak();
    data.on_kit_roll(move || {
        if let Some(app) = weak.upgrade() {
            roll(&app, None);
        }
    });
    let weak = app.as_weak();
    data.on_pad_roll(move |i| {
        if let (Some(app), Ok(i)) = (weak.upgrade(), usize::try_from(i)) {
            roll(&app, Some(i));
        }
    });
    let weak = app.as_weak();
    data.on_kit_toggle(move |what| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let k = &mut p.borrow_mut().kit;
            match what.as_str() {
                "unused" => k.options.prefer_unused = !k.options.prefer_unused,
                "pack" => k.options.same_pack = !k.options.same_pack,
                "elektron" => k.export.elektron_ready = !k.export.elektron_ready,
                "mono" => k.export.mono = !k.export.mono,
                "normalise" => k.export.normalise = !k.export.normalise,
                _ => {}
            }
        });
        show(&app);
    });
    let weak = app.as_weak();
    data.on_kit_choose(move |what, value| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let k = &mut p.borrow_mut().kit;
            match what.as_str() {
                "key" => k.options.key_tonic = NOTE_NAMES.iter().position(|n| *n == value.as_str()).map(|i| i as i32),
                "randomness" => {
                    if let Some(r) = RANDOMNESS.iter().find(|r| r.0 == value.as_str()) {
                        k.options.randomness = r.1;
                    }
                }
                _ => {}
            }
        });
        show(&app);
    });
    let weak = app.as_weak();
    data.on_kit_add(move |t| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let k = &mut p.borrow_mut().kit;
            let n = k.slots.iter().filter(|s| s.name.starts_with(t.as_str())).count();
            let name = if n == 0 { t.to_string() } else { format!("{t} {}", n + 1) };
            k.slots.push(KitSlot { name, types: vec![t.to_string()], locked: false, sound: None });
        });
        let last = PAGE.with(|p| p.borrow().kit.slots.len() - 1);
        roll(&app, Some(last));
    });
    let weak = app.as_weak();
    data.on_pad_remove(move |i| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let k = &mut p.borrow_mut().kit;
            if let Ok(i) = usize::try_from(i)
                && i < k.slots.len()
            {
                k.slots.remove(i);
            }
        });
        show(&app);
    });
    let weak = app.as_weak();
    data.on_pad_lock(move |i| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            if let Some(s) = usize::try_from(i).ok().and_then(|i| p.kit.slots.get_mut(i)) {
                s.locked = !s.locked;
            }
        });
        show(&app);
    });
    let weak = app.as_weak();
    data.on_pad_pick(move |i| {
        let Some(app) = weak.upgrade() else { return };
        let Some(at) = pad_place(i) else { return };
        pick(&app, at);
        if PAGE.with(|p| p.borrow().autoplay) {
            play_picked(&app, 0.0);
        }
    });
    let weak = app.as_weak();
    data.on_pad_play(move |i| {
        let Some(app) = weak.upgrade() else { return };
        let Some(at) = pad_place(i) else { return };
        pick(&app, at);
        play_picked(&app, 0.0);
    });
    let weak = app.as_weak();
    data.on_kit_export(move || {
        if let Some(app) = weak.upgrade() {
            export(&app);
        }
    });
}

/// The place in the list of the sound on pad `i`.
fn pad_place(i: i32) -> Option<usize> {
    PAGE.with(|p| {
        let p = p.borrow();
        let id = p.kit.slots.get(usize::try_from(i).ok()?)?.sound?;
        p.places.get(&id).copied()
    })
}

/// Rolls the kit (or just pad `only`) on its own thread, newest roll first.
pub(super) fn roll(app: &AppWindow, only: Option<usize>) {
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let index = p.index.clone();
        let list = p.list.clone();
        let k = &mut p.kit;
        let Some(index) = index.filter(|_| !list.is_empty()) else {
            k.status = "Listening to your sounds first: their fingerprints are still being read.".to_owned();
            return None;
        };
        k.asked += 1;
        k.busy = true;
        Some(RollAsk {
            id: k.asked,
            list,
            index,
            slots: k.slots.clone(),
            options: kits::Options { seed: 0, ..k.options.clone() },
            only,
        })
    });
    show(app);
    let Some(ask) = ask else { return };
    if HERE.with(|h| *h.borrow()) {
        let rolled = ask.answer();
        rolled_in(app, rolled);
        return;
    }
    ROLLING.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            match Newest::start("Kit builder", move |ask: RollAsk| {
                let rolled = ask.answer();
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = weak.upgrade() {
                        rolled_in(&app, rolled);
                    }
                });
            }) {
                Ok(worker) => *w = Some(worker),
                Err(e) => log::error("Sounds", &format!("Couldn't start the kit builder's thread: {e}")),
            }
        }
        if let Some(worker) = w.as_ref() {
            worker.ask(ask);
        }
    });
}

fn rolled_in(app: &AppWindow, r: Rolled) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let k = &mut p.kit;
        if k.asked != r.id {
            return;
        }
        k.busy = false;
        match r.only {
            // Just that pad: the others may have changed (locked, taken out) while it rolled.
            Some(i) => {
                if let (Some(new), Some(old)) = (r.slots.get(i), k.slots.get_mut(i))
                    && new.name == old.name
                {
                    old.sound = new.sound;
                }
            }
            None if r.slots.len() == k.slots.len() => {
                for (old, new) in k.slots.iter_mut().zip(r.slots) {
                    old.sound = new.sound;
                }
                k.seed = r.seed;
            }
            None => {}
        }
        let empty = k.slots.iter().filter(|s| s.sound.is_none()).count();
        k.status = if empty > 0 {
            format!("{empty} pad{} found no sound of the right type yet.", if empty == 1 { "" } else { "s" })
        } else {
            format!("Kit #{}", k.seed)
        };
    });
    show(app);
}

/// Copies the kit into a new folder under the export folder's Kits, off the window's thread, saying on a card how it
/// went. Never writes over a kit already there.
fn export(app: &AppWindow) {
    let Some((slots, list, folder, name, options)) = PAGE.with(|p| {
        let p = p.borrow();
        let k = &p.kit;
        if k.slots.iter().all(|s| s.sound.is_none()) {
            return None;
        }
        let name = format!("Kit {} #{}", Local::now().format("%Y-%m-%d %H%M"), k.seed);
        Some((k.slots.clone(), p.list.clone(), kits_folder(k.export_folder.as_deref()), name, k.export))
    }) else {
        PAGE.with(|p| p.borrow_mut().kit.status = "Roll a kit first.".to_owned());
        show(app);
        return;
    };
    let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return };
    let weak = app.as_weak();
    let started = background::spawn("Kit export", move || {
        let job = feed.start("Exporting a kit", &name);
        let done = background::on_pool(|| {
            let get = |id: i64| list.find(id).and_then(|i| list.sound(i));
            kits::export(&slots, get, &folder, &name, options)
        });
        let status = match done {
            Ok(e) => {
                let shown = e.folder.display().to_string();
                job.done(&shown);
                format!("Exported to {shown}")
            }
            Err(e) => {
                log::warning("Sounds", &format!("Kit export failed: {e}"));
                job.failed(&e.to_string());
                format!("Export failed: {e}")
            }
        };
        let _ = slint::invoke_from_event_loop(move || {
            PAGE.with(|p| p.borrow_mut().kit.status = status);
            if let Some(app) = weak.upgrade() {
                show(&app);
            }
        });
    });
    if let Err(e) = started {
        log::error("Sounds", &format!("Couldn't start the kit export: {e}"));
    }
}

/// Where kits go: the export folder (the C# app's setting, else Documents\Music Organiser Exports), then Kits.
fn kits_folder(setting: Option<&str>) -> PathBuf {
    let root = match setting.filter(|f| !f.trim().is_empty()) {
        Some(f) => PathBuf::from(f),
        None => documents().join("Music Organiser Exports"),
    };
    root.join("Kits")
}

fn documents() -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    home.map_or_else(|| Path::new(".").to_owned(), |h| h.join("Documents"))
}

/// The pads and choices as they are now; also when what's picked or playing changes.
pub(super) fn show(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let k = &p.kit;
        data.set_kit_status(k.status.clone().into());
        data.set_kit_busy(k.busy);
        data.set_kit_unused(k.options.prefer_unused);
        data.set_kit_pack(k.options.same_pack);
        data.set_kit_elektron(k.export.elektron_ready);
        data.set_kit_mono(k.export.mono);
        data.set_kit_normalise(k.export.normalise);
        let key = k.options.key_tonic.and_then(|t| usize::try_from(t).ok()).and_then(|t| NOTE_NAMES.get(t));
        data.set_kit_key(key.copied().unwrap_or_default().into());
        let r = RANDOMNESS
            .iter()
            .min_by(|a, b| (a.1 - k.options.randomness).abs().total_cmp(&(b.1 - k.options.randomness).abs()));
        data.set_kit_randomness(r.map_or("", |r| r.0).into());
        data.set_kit_types(strings(p.types.iter().map(|t| t.0.as_str())));
    });
    redraw();
}

/// The pad lines again.
pub(super) fn redraw() {
    let lines: Vec<PadLine> = PAGE.with(|p| {
        let p = p.borrow();
        p.kit.slots.iter().map(|s| pad_line(&p, s)).collect()
    });
    PADS.with(|m| m.set_vec(lines));
}

fn pad_line(p: &Page, slot: &KitSlot) -> PadLine {
    let at = slot.sound.and_then(|id| p.places.get(&id).copied());
    let Some((at, s)) = at.and_then(|at| p.list.sound(at).map(|s| (at, s))) else {
        return PadLine {
            name: slot.name.clone().into(),
            sound: "Empty pad".into(),
            detail: "Roll a kit to fill it".into(),
            colour: 0,
            filled: false,
            locked: slot.locked,
            picked: false,
            playing: false,
        };
    };
    let used = match p.list.used_in(at) {
        Some(0) => " · unused".to_owned(),
        Some(n) => format!(" · used in {n}"),
        None => String::new(),
    };
    PadLine {
        name: slot.name.clone().into(),
        sound: s.name().into(),
        detail: format!("{} · {}{used}", s.sound_type, s.folder()).into(),
        colour: list::type_colour(&s.sound_type),
        filled: true,
        locked: slot.locked,
        picked: p.picked == Some(at),
        playing: p.playing.is_some_and(|(i, _)| i == at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kits_go_in_the_export_folder_else_documents() {
        assert_eq!(kits_folder(Some("E:\\Exports")), PathBuf::from("E:\\Exports").join("Kits"));
        assert!(kits_folder(None).ends_with(Path::new("Music Organiser Exports").join("Kits")));
        assert!(kits_folder(Some("  ")).ends_with(Path::new("Music Organiser Exports").join("Kits")));
    }
}
