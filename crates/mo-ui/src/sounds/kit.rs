//! The Sounds page's kit builder: random kits that still make sense (`mo_sounds::kits`), rolled and exported off the
//! window's thread. Rolling only reads the library; exporting copies the pads' samples into a new folder of its own
//! (the samples themselves are only read, rule 2).

use std::cell::RefCell;
use std::collections::HashSet;
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
    /// Each pad's own number, beside `slots`, so a roll's answer finds its pad even after pads were added or taken
    /// out meanwhile.
    ids: Vec<u64>,
    next_id: u64,
    /// What the next roll fills: every unlocked pad, and these pads by number (re-rolls and added pads), kept until a
    /// roll that covers them comes back, so a newer ask never loses an older one.
    full: bool,
    pending: HashSet<u64>,
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
        let slots = kits::default_drum_kit();
        Kit {
            ids: (0..slots.len() as u64).collect(),
            next_id: slots.len() as u64,
            full: false,
            pending: HashSet::new(),
            slots,
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
    static ROLLING: RefCell<Option<Newest<(RollAsk, bool)>>> = const { RefCell::new(None) };
}

/// A roll: everything its thread needs. `slots` holds only what this roll fills, unlocked, and the sounds it must
/// keep clear of, as locked slots: other pads' sounds, and a re-rolled pad's sound so it gets a new one.
struct RollAsk {
    id: u64,
    list: Arc<SoundList>,
    index: Arc<SoundIndex>,
    slots: Vec<KitSlot>,
    /// The pad number of each of `slots`, or `None` for one only there to be kept clear of.
    pads: Vec<Option<u64>>,
    options: kits::Options,
}

struct Rolled {
    id: u64,
    /// Each pad rolled, by number, with its new sound.
    sounds: Vec<(u64, Option<i64>)>,
    seed: i32,
    full: bool,
}

impl RollAsk {
    fn answer(self) -> Rolled {
        let empty = std::collections::HashMap::new();
        let usage = self.list.usage_map().unwrap_or(&empty);
        let mut slots = self.slots;
        let seed =
            background::on_pool(|| kits::roll(self.list.sounds(), usage, &self.index, &mut slots, &self.options));
        let sounds = self.pads.iter().zip(&slots).filter_map(|(pad, s)| pad.map(|id| (id, s.sound))).collect();
        Rolled { id: self.id, sounds, seed, full: false }
    }
}

impl Kit {
    /// What the next roll fills, as a [`RollAsk`]'s slots and pad numbers.
    fn to_roll(&self) -> (Vec<KitSlot>, Vec<Option<u64>>) {
        let mut slots = Vec::new();
        let mut pads = Vec::new();
        let mut keep_clear = Vec::new();
        for (slot, &id) in self.slots.iter().zip(&self.ids) {
            let rolls = (self.full && !slot.locked) || self.pending.contains(&id);
            if rolls {
                slots.push(KitSlot { locked: false, sound: None, ..slot.clone() });
                pads.push(Some(id));
            }
            if let Some(sound) = slot.sound {
                // A re-rolled pad stays clear of its own sound; a full roll may pick it again.
                if !rolls || !self.full {
                    keep_clear.push(sound);
                }
            }
        }
        for sound in keep_clear {
            slots.push(KitSlot { name: String::new(), types: Vec::new(), locked: true, sound: Some(sound) });
            pads.push(None);
        }
        (slots, pads)
    }

    /// A name for a new pad of type `t` that no pad has: "Kick", else "Kick 2", "Kick 3"…
    fn new_name(&self, t: &str) -> String {
        let taken = |n: &str| self.slots.iter().any(|s| s.name == n);
        if !taken(t) {
            return t.to_owned();
        }
        (2..).map(|n| format!("{t} {n}")).find(|n| !taken(n)).unwrap_or_else(|| t.to_owned())
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
        let Some(app) = weak.upgrade() else { return };
        let id = PAGE.with(|p| usize::try_from(i).ok().and_then(|i| p.borrow().kit.ids.get(i).copied()));
        if let Some(id) = id {
            roll(&app, Some(id));
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
        let id = PAGE.with(|p| {
            let k = &mut p.borrow_mut().kit;
            let name = k.new_name(&t);
            k.slots.push(KitSlot { name, types: vec![t.to_string()], locked: false, sound: None });
            let id = k.next_id;
            k.next_id += 1;
            k.ids.push(id);
            id
        });
        roll(&app, Some(id));
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
                let id = k.ids.remove(i);
                k.pending.remove(&id);
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

/// Rolls the kit (every unlocked pad), or adds pad number `pad` to what the next roll fills, on its own thread,
/// newest ask first: each ask carries everything still waiting, so none is lost.
pub(super) fn roll(app: &AppWindow, pad: Option<u64>) {
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        // The fingerprints are only for the vibe, so rolling doesn't wait for them.
        let index = p.index.clone().unwrap_or_default();
        let list = p.list.clone();
        let k = &mut p.kit;
        match pad {
            Some(id) => {
                k.pending.insert(id);
            }
            None => k.full = true,
        }
        if list.is_empty() {
            k.status = "Your sounds are still being read.".to_owned();
            return None;
        }
        k.asked += 1;
        k.busy = true;
        let (slots, pads) = k.to_roll();
        Some((
            RollAsk { id: k.asked, list, index, slots, pads, options: kits::Options { seed: 0, ..k.options.clone() } },
            k.full,
        ))
    });
    show(app);
    let Some((ask, full)) = ask else { return };
    if HERE.with(|h| *h.borrow()) {
        let rolled = Rolled { full, ..ask.answer() };
        rolled_in(app, rolled);
        return;
    }
    let started = ROLLING.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let weak = app.as_weak();
            match Newest::start("Kit builder", move |(ask, full): (RollAsk, bool)| {
                let rolled = Rolled { full, ..ask.answer() };
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
        match w.as_ref() {
            Some(worker) => {
                worker.ask((ask, full));
                true
            }
            None => false,
        }
    });
    if !started {
        PAGE.with(|p| {
            let k = &mut p.borrow_mut().kit;
            k.busy = false;
            k.status = "The kit builder couldn't start: see the log.".to_owned();
        });
        show(app);
    }
}

fn rolled_in(app: &AppWindow, r: Rolled) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let k = &mut p.kit;
        if k.asked != r.id {
            return;
        }
        k.busy = false;
        for (id, sound) in r.sounds {
            let Some(i) = k.ids.iter().position(|&x| x == id) else { continue };
            let waited = k.pending.remove(&id);
            // A pad locked while it rolled keeps its sound; a re-roll with nothing new keeps the old one.
            if let Some(slot) = k.slots.get_mut(i)
                && (waited || !slot.locked)
                && (sound.is_some() || !waited)
            {
                slot.sound = sound;
            }
        }
        if r.full {
            k.full = false;
            k.seed = r.seed;
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
    let feed = FEED.with(|f| f.borrow().clone());
    PAGE.with(|p| p.borrow_mut().kit.status = "Exporting the kit…".to_owned());
    show(app);
    let weak = app.as_weak();
    let started = background::spawn("Kit export", move || {
        let job = feed.map(|f| f.start("Exporting a kit", &name));
        let done = background::on_pool(|| {
            let get = |id: i64| list.find(id).and_then(|i| list.sound(i));
            kits::export(&slots, get, &folder, &name, options)
        });
        let status = match done {
            Ok(e) => {
                let shown = e.folder.display().to_string();
                if let Some(job) = job {
                    job.done(&shown);
                }
                format!("Exported to {shown}")
            }
            Err(e) => {
                log::warning("Sounds", &format!("Kit export failed: {e}"));
                if let Some(job) = job {
                    job.failed(&e.to_string());
                }
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

/// The Documents folder, where Windows really keeps it (it may have moved, to OneDrive say).
fn documents() -> PathBuf {
    #[cfg(windows)]
    if let Some(d) = known_folders::get_known_folder_path(known_folders::KnownFolder::Documents) {
        return d;
    }
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
    let Some(lines) = PAGE.with(|p| {
        let p = p.borrow();
        (p.view == super::KITS).then(|| p.kit.slots.iter().map(|s| pad_line(&p, s)).collect::<Vec<_>>())
    }) else {
        return;
    };
    // Only the lines that changed, so a click's line isn't made again under the mouse (a double-click still lands).
    PADS.with(|m| {
        if m.row_count() != lines.len() {
            m.set_vec(lines);
            return;
        }
        for (i, line) in lines.into_iter().enumerate() {
            if m.row_data(i).as_ref() != Some(&line) {
                m.set_row_data(i, line);
            }
        }
    });
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
    fn new_pads_get_names_no_pad_has() {
        let mut k = Kit::default();
        assert_eq!(k.new_name("Kick"), "Kick 2");
        assert_eq!(k.new_name("Shaker"), "Shaker");
        // "Percussion" taken out, "Percussion 2" still there: a new one is "Percussion", not a second "Percussion 2".
        k.slots.retain(|s| s.name != "Percussion");
        assert_eq!(k.new_name("Percussion"), "Percussion");
        assert_eq!(k.new_name("Percussion 2"), "Percussion 2 2");
    }

    #[test]
    fn a_re_roll_fills_only_its_pad_and_keeps_clear_of_every_sound_on_show() {
        let mut k = Kit::default();
        k.slots[0].sound = Some(10);
        k.slots[1].sound = Some(11);
        k.slots[1].locked = true;
        k.pending.insert(k.ids[0]);
        let (slots, pads) = k.to_roll();
        // Pad 0 rolls, empty; then its old sound and pad 1's are kept clear; empty pads aren't touched.
        assert_eq!(pads, vec![Some(k.ids[0]), None, None]);
        assert_eq!(slots[0].sound, None);
        assert!(!slots[0].locked);
        assert_eq!(
            slots[1..].iter().map(|s| (s.locked, s.sound)).collect::<Vec<_>>(),
            vec![(true, Some(10)), (true, Some(11))]
        );
        // A full roll fills every unlocked pad, and may pick their old sounds again; locked ones are kept clear.
        k.pending.clear();
        k.full = true;
        let (slots, pads) = k.to_roll();
        assert_eq!(pads.iter().filter(|p| p.is_some()).count(), k.slots.len() - 1);
        assert_eq!(slots.iter().filter(|s| s.locked).map(|s| s.sound).collect::<Vec<_>>(), vec![Some(11)]);
    }

    #[test]
    fn kits_go_in_the_export_folder_else_documents() {
        assert_eq!(kits_folder(Some("E:\\Exports")), PathBuf::from("E:\\Exports").join("Kits"));
        assert!(kits_folder(None).ends_with(Path::new("Music Organiser Exports").join("Kits")));
        assert!(kits_folder(Some("  ")).ends_with(Path::new("Music Organiser Exports").join("Kits")));
    }
}
