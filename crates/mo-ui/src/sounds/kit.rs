//! The Sounds page's kit builder: random kits that still make sense (`mo_sounds::kits`), rolled, exported and sent to
//! Bitwig off the window's thread. Rolling only reads the library; exporting copies the pads' samples into a new
//! folder of its own (the samples themselves are only read, rule 2); sending puts them on a Drum Machine in the project
//! open in Bitwig.

use std::cell::RefCell;
use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use chrono::Local;
use mo_app::background::{self, Newest};
use mo_app::sounds::{self as list, SoundList};
use mo_app::{log, to_bitwig};
use mo_sounds::index::SoundIndex;
use mo_sounds::kits::{self, ExportOptions, KitSlot};
use mo_sounds::music::NOTE_NAMES;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use super::{FEED, HERE, PAGE, Page, pick, play_picked, strings};
use crate::{AppWindow, PadLine, SoundsData};

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
    /// The kit is on its way to Bitwig: Send to Bitwig waits meanwhile, as in the C# app.
    pub(super) sending: bool,
    /// The kit is being copied into its folder: Export kit waits meanwhile, so a second click doesn't copy it again.
    pub(super) exporting: bool,
    /// The Drum Machine's ID the C# app kept in `library.db`, if it kept one.
    pub(super) drum_machine: Option<String>,
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
            sending: false,
            exporting: false,
            drum_machine: None,
        }
    }
}

/// Where kits go in Bitwig while the app runs: a new Drum Machine, or the one selected in Bitwig once Bitwig didn't add
/// one for the ID.
static KITS_TO_BITWIG: to_bitwig::Kits = to_bitwig::Kits::new();

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
            let rolls = (self.full && !slot.kept()) || self.pending.contains(&id);
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

    /// Locks pad `i`, or unlocks it. Gives whether a full roll on its way must be asked again: it was worked out with
    /// the pads locked as they were, so it could give another pad the sound this one keeps now, or leave this one as
    /// it was though it's unlocked now. An empty pad is filled either way, so its lock changes nothing there.
    fn toggle_lock(&mut self, i: usize) -> bool {
        let Some(slot) = self.slots.get_mut(i) else { return false };
        slot.locked = !slot.locked;
        self.busy && self.full && slot.sound.is_some()
    }

    /// Takes in roll `r`'s sounds, if it's the newest roll asked for.
    fn take_rolled(&mut self, r: Rolled) {
        if self.asked != r.id {
            return;
        }
        self.busy = false;
        for (id, sound) in r.sounds {
            let Some(i) = self.ids.iter().position(|&x| x == id) else { continue };
            let waited = self.pending.remove(&id);
            // A pad locked with a sound while it rolled keeps it; a re-roll with nothing new keeps the old one.
            if let Some(slot) = self.slots.get_mut(i)
                && (waited || !slot.kept())
                && (sound.is_some() || !waited)
            {
                slot.sound = sound;
            }
        }
        if r.full {
            self.full = false;
            self.seed = r.seed;
        }
        let empty = self.slots.iter().filter(|s| s.sound.is_none()).count();
        self.status = if empty > 0 {
            format!("{empty} pad{} found no sound of the right type yet.", if empty == 1 { "" } else { "s" })
        } else {
            to_bitwig::kit_name(self.seed)
        };
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
            if what.as_str() == "key" {
                k.options.key_tonic = NOTE_NAMES.iter().position(|n| *n == value.as_str()).map(|i| i as i32);
            }
        });
        show(&app);
    });
    let weak = app.as_weak();
    data.on_kit_randomness_set(move |v| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| p.borrow_mut().kit.options.randomness = f64::from(v.clamp(0.0, 1.0)));
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
        let Ok(i) = usize::try_from(i) else { return };
        // The newest roll wins: one asked again as the pads are locked now takes the place of the one on its way.
        if PAGE.with(|p| p.borrow_mut().kit.toggle_lock(i)) {
            roll(&app, None);
        } else {
            show(&app);
        }
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
    let weak = app.as_weak();
    data.on_kit_to_bitwig(move || {
        if let Some(app) = weak.upgrade() {
            send_to_bitwig(&app);
        }
    });
}

/// The place in the list of the sound on pad `i`.
pub(super) fn pad_place(i: i32) -> Option<usize> {
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
                let id = ask.id;
                let rolled = background::guarded(|| Rolled { full, ..ask.answer() });
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(app) = weak.upgrade() else { return };
                    match rolled {
                        Ok(rolled) => rolled_in(&app, rolled),
                        // A roll that broke lets go of Rolling… and says so, if it's still the newest ask.
                        Err(broke) => {
                            PAGE.with(|p| {
                                let k = &mut p.borrow_mut().kit;
                                if k.asked == id {
                                    k.busy = false;
                                    k.status = format!("The kit builder couldn't roll: {broke}.");
                                }
                            });
                            show(&app);
                        }
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
    PAGE.with(|p| p.borrow_mut().kit.take_rolled(r));
    show(app);
}

/// What an export copies: the pads, the sounds they're from, the export folder setting, the new folder's name, and
/// how the samples are written.
type ToExport = (Vec<KitSlot>, Arc<SoundList>, Option<String>, String, ExportOptions);

/// Copies the kit into a new folder under the export folder's Kits, off the window's thread, saying on a card how it
/// went. Never writes over a kit already there.
fn export(app: &AppWindow) {
    let ready = PAGE.with(|p| ready_to_export(&mut p.borrow_mut()));
    show(app);
    let Some((slots, list, setting, name, options)) = ready else { return };
    let feed = FEED.with(|f| f.borrow().clone());
    let weak = app.as_weak();
    let started = background::spawn("Kit export", move || {
        let job = feed.map(|f| f.start("Exporting a kit", &name));
        // A panic in it is logged as a crash and fails the card as it goes; Export kit still comes back.
        let status = mo_catch::catch_crash(AssertUnwindSafe(|| {
            // Worked out here: finding the Documents folder can wait on the disk.
            let folder = kits_folder(setting.as_deref());
            let done = background::on_pool(|| {
                let get = |id: i64| list.find(id).and_then(|i| list.sound(i));
                kits::export(&slots, get, &folder, &name, options)
            });
            match done {
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
            }
        }))
        .unwrap_or_else(|_| "The kit couldn't be exported: see the log.".to_owned());
        let _ = slint::invoke_from_event_loop(move || {
            PAGE.with(|p| {
                let k = &mut p.borrow_mut().kit;
                k.exporting = false;
                k.status = status;
            });
            if let Some(app) = weak.upgrade() {
                show(&app);
            }
        });
    });
    if let Err(e) = started {
        log::error("Sounds", &format!("Couldn't start the kit export: {e}"));
        PAGE.with(|p| {
            let k = &mut p.borrow_mut().kit;
            k.exporting = false;
            k.status = "The kit couldn't be exported: see the log.".to_owned();
        });
        show(app);
    }
}

/// What an export of the kit on show copies, with the kit marked as exporting; or `None`, with the kit's line saying
/// why where there's something to say: there's nothing on the pads, or the kit is being exported already.
fn ready_to_export(p: &mut Page) -> Option<ToExport> {
    let k = &mut p.kit;
    // A second click while the kit is copied would copy it again, into a second folder.
    if k.exporting {
        return None;
    }
    if k.slots.iter().all(|s| s.sound.is_none()) {
        k.status = "Roll a kit first.".to_owned();
        return None;
    }
    k.exporting = true;
    k.status = "Exporting the kit…".to_owned();
    let name = format!("Kit {} #{}", Local::now().format("%Y-%m-%d %H%M"), k.seed);
    Some((k.slots.clone(), p.list.clone(), k.export_folder.clone(), name, k.export))
}

/// Puts the kit on a Drum Machine in the project open in Bitwig, off the window's thread: on a new track named after
/// the kit ("Kit #12"), each sound on the pad General MIDI drum parts play (kick on C1, snare on D1, closed hat on F#1)
/// and the rest on the free pads. A card and the line under the buttons say how it went. The button waits while it
/// goes.
fn send_to_bitwig(app: &AppWindow) {
    let Some(feed) = FEED.with(|f| f.borrow().clone()) else { return };
    let ready = PAGE.with(|p| {
        let p = p.borrow();
        let k = &p.kit;
        // A kit on its way waits for Bitwig; one rolling isn't the kit on show yet.
        if k.sending || k.busy {
            return Err(None);
        }
        let pads = to_bitwig::kit_pads(&to_bitwig::kit_sounds(&k.slots, &p.list));
        if pads.is_empty() {
            return Err(Some("Roll a kit first."));
        }
        Ok((pads, to_bitwig::kit_name(k.seed), k.drum_machine.clone(), k.asked))
    });
    let (pads, name, setting, asked) = match ready {
        Ok(ready) => ready,
        Err(why) => {
            if let Some(why) = why {
                PAGE.with(|p| p.borrow_mut().kit.status = why.to_owned());
                show(app);
            }
            return;
        }
    };
    let job = to_bitwig::kit_card(&feed, pads.len());
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.kit.sending = true;
        p.kit.status = "Sending the kit to Bitwig…".to_owned();
    });
    show(app);
    let weak = app.as_weak();
    let started = background::spawn("Kit to Bitwig", move || {
        let to = to_bitwig::Bitwig::default();
        // A panic in it is logged as a crash and fails the card as it goes; the button still comes back.
        let line = mo_catch::catch_crash(AssertUnwindSafe(|| {
            to_bitwig::send_kit(job, &to, &KITS_TO_BITWIG, &name, &pads, setting.as_deref())
        }))
        .unwrap_or_else(|_| "The kit couldn't be sent: see the log.".to_owned());
        let _ = slint::invoke_from_event_loop(move || {
            PAGE.with(|p| {
                let mut p = p.borrow_mut();
                p.kit.sending = false;
                // A kit rolled since says what it is; the card still says how the send went.
                if p.kit.asked == asked {
                    p.kit.status = line;
                }
            });
            if let Some(app) = weak.upgrade() {
                show(&app);
            }
        });
    });
    // Its card was taken into the thread, so it fails by itself.
    if let Err(e) = started {
        log::error("Sounds", &format!("Couldn't start sending the kit to Bitwig: {e}"));
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.kit.sending = false;
            p.kit.status = "The kit couldn't be sent: see the log.".to_owned();
        });
        show(app);
    }
}

/// Where kits go: the export folder (the C# app's setting, else Documents\Music Organiser Exports), then Kits.
fn kits_folder(setting: Option<&str>) -> PathBuf {
    let root = match setting.filter(|f| !f.trim().is_empty()) {
        Some(f) => PathBuf::from(f),
        None => crate::files::documents().join("Music Organiser Exports"),
    };
    root.join("Kits")
}

/// The pads and choices as they are now; also when what's picked or playing changes.
pub(super) fn show(app: &AppWindow) {
    let data = app.global::<SoundsData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let k = &p.kit;
        data.set_kit_status(k.status.clone().into());
        data.set_kit_busy(k.busy);
        data.set_kit_sending(k.sending);
        data.set_kit_exporting(k.exporting);
        data.set_kit_unused(k.options.prefer_unused);
        data.set_kit_pack(k.options.same_pack);
        data.set_kit_elektron(k.export.elektron_ready);
        data.set_kit_mono(k.export.mono);
        data.set_kit_normalise(k.export.normalise);
        let key = k.options.key_tonic.and_then(|t| usize::try_from(t).ok()).and_then(|t| NOTE_NAMES.get(t));
        data.set_kit_key(key.copied().unwrap_or_default().into());
        data.set_kit_randomness(k.options.randomness as f32);
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
    use std::path::Path;

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
    fn a_locked_empty_pad_is_filled_by_a_roll() {
        let mut k = Kit::default();
        k.slots[0].locked = true;
        k.slots[1].locked = true;
        k.slots[1].sound = Some(11);
        k.full = true;
        let (_, pads) = k.to_roll();
        // The empty pad rolls though it's locked; the one locked with a sound doesn't.
        assert!(pads.contains(&Some(k.ids[0])));
        assert!(!pads.contains(&Some(k.ids[1])));
        // Its sound comes in; the pad locked with a sound keeps its own, whatever the answer says.
        k.asked = 3;
        k.take_rolled(Rolled { id: 3, sounds: vec![(k.ids[0], Some(20)), (k.ids[1], Some(21))], seed: 7, full: true });
        assert_eq!((k.slots[0].sound, k.slots[1].sound), (Some(20), Some(11)));
        assert!(!k.full);
        assert_eq!(k.seed, 7);
        // An older roll's answer changes nothing.
        k.take_rolled(Rolled { id: 2, sounds: vec![(k.ids[0], Some(30))], seed: 8, full: true });
        assert_eq!(k.slots[0].sound, Some(20));
    }

    #[test]
    fn a_lock_changed_while_a_full_roll_is_on_its_way_asks_it_again() {
        let mut k = Kit::default();
        k.slots[0].sound = Some(10);
        // No roll on its way: only the lock changes.
        assert!(!k.toggle_lock(0));
        assert!(k.slots[0].locked);
        // A full roll on its way was worked out with pad 0 locked: unlocked now, it's asked again; so it is when
        // pad 0 is locked again, so no other pad gets its sound. An empty pad's lock changes nothing in a roll.
        (k.busy, k.full) = (true, true);
        assert!(k.toggle_lock(0));
        assert!(k.toggle_lock(0));
        assert!(!k.toggle_lock(1));
        // A re-roll of one pad already keeps clear of every sound on show.
        k.full = false;
        assert!(!k.toggle_lock(0));
        assert!(!k.toggle_lock(99));
    }

    #[test]
    fn a_kit_is_exported_once_at_a_time() {
        let mut p = Page::default();
        assert!(ready_to_export(&mut p).is_none());
        assert_eq!(p.kit.status, "Roll a kit first.");
        p.kit.slots[0].sound = Some(10);
        p.kit.export_folder = Some(r"E:\Exports".into());
        let Some((slots, _, setting, _, _)) = ready_to_export(&mut p) else { panic!("nothing to export") };
        assert_eq!((slots[0].sound, setting.as_deref()), (Some(10), Some(r"E:\Exports")));
        assert!(p.kit.exporting);
        // A second click while it's copied does nothing.
        assert!(ready_to_export(&mut p).is_none());
        assert_eq!(p.kit.status, "Exporting the kit…");
    }

    #[test]
    fn kits_go_in_the_export_folder_else_documents() {
        assert_eq!(kits_folder(Some("E:\\Exports")), PathBuf::from("E:\\Exports").join("Kits"));
        assert!(kits_folder(None).ends_with(Path::new("Music Organiser Exports").join("Kits")));
        assert!(kits_folder(Some("  ")).ends_with(Path::new("Music Organiser Exports").join("Kits")));
    }
}
