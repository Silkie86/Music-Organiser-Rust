//! Send to Bitwig from the Sounds page: the picked sound, or the kit on show, into the project open in Bitwig, through
//! the Music Organiser extension there (`mo_dawlink::bitwig` does the sending, and has the words, the C# app's). Each
//! waits for Bitwig, so the page calls them on a thread of their own, never the window's (rule 12). Each has an
//! activity card from the moment it's asked for, which says how it went once Bitwig answers.
//!
//! What the C# app also did and this doesn't yet: count each send for Most liked (that writes to `sounds.db`, which
//! the Rust app only reads for now), and write the Drum Machine's ID into `library.db` (the Rust app knows Bitwig's
//! own, so it has no need to).

use std::sync::atomic::{AtomicBool, Ordering};

use mo_dawlink::bitwig::{self, KitTarget};
pub use mo_dawlink::bitwig::{Bitwig, DRUM_MACHINE_KEY, KitSound, Pad, kit_name, kit_pads};
use mo_sounds::kits::KitSlot;

use crate::activity::{Feed, Job};
use crate::sounds::SoundList;

/// The card for a sound on its way, shown at once: "Putting Kick 909 into Bitwig", "On a Sampler, on a new track".
pub fn sound_card(feed: &Feed, name: &str) -> Job {
    feed.start(&bitwig::sending_sound(name), bitwig::SOUND_PLACE)
}

/// Puts the sound at `path` on a Sampler on a new track named `name` in the project open in Bitwig, and ends `job`'s
/// card ([`sound_card`]) with how it went: "Kick 909 is in Bitwig", or what went wrong and what to do about it.
/// Returns whether it went in as asked. Waits for Bitwig, so never call it on the window's thread.
pub fn send_sound(job: Job, to: &Bitwig, path: &str, name: &str) -> bool {
    match to.send_sound(path, name) {
        Ok(sent) if sent.went_in() => {
            job.done_as(&sent.title(), &sent.detail());
            true
        }
        // Something went in, or may have: the card says what.
        Ok(sent) => {
            job.failed_as(&sent.title(), &sent.detail());
            false
        }
        // Nothing changed in Bitwig.
        Err(e) => {
            job.failed_as(bitwig::SOUND_FAILED, &e.to_string());
            false
        }
    }
}

/// The kit on show as the link takes it: each pad that has a sound, by the pad's name, with the kind of sound the pad
/// is for (its first type, which places it the General MIDI way) and the sound's file, in the kit's order. Pads with no
/// sound, and sounds no longer in the list, are left out. Give it to [`kit_pads`] for the pads.
pub fn kit_sounds(slots: &[KitSlot], list: &SoundList) -> Vec<KitSound> {
    slots
        .iter()
        .filter_map(|slot| {
            let sound = list.sound(list.find(slot.sound?)?)?;
            Some(KitSound {
                name: slot.name.clone(),
                kind: slot.types.first().cloned().unwrap_or_default(),
                path: sound.path.clone(),
            })
        })
        .collect()
}

/// Where kits go in Bitwig while the app runs: on a new Drum Machine on a new track, until Bitwig once doesn't add a
/// Drum Machine for the ID it's given; from then on, on the Drum Machine selected in Bitwig, until the app restarts
/// (as the C# app did).
#[derive(Debug, Default)]
pub struct Kits {
    selected: AtomicBool,
}

impl Kits {
    pub const fn new() -> Kits {
        Kits { selected: AtomicBool::new(false) }
    }

    /// Where the next kit goes. `setting` is the Drum Machine ID the C# app kept in `library.db`
    /// (`mo_dawlink::bitwig::DRUM_MACHINE_KEY`), if it kept one; without it, Bitwig's own.
    pub fn target(&self, setting: Option<&str>) -> KitTarget {
        if self.on_the_selected() {
            KitTarget::Selected
        } else {
            KitTarget::NewDrumMachine(bitwig::drum_machine(setting))
        }
    }

    /// Whether kits now go on the Drum Machine selected in Bitwig.
    pub fn on_the_selected(&self) -> bool {
        self.selected.load(Ordering::Relaxed)
    }

    /// From now on, kits go on the Drum Machine selected in Bitwig.
    fn use_the_selected(&self) {
        self.selected.store(true, Ordering::Relaxed);
    }
}

/// The card for a kit on its way, shown at once: "Sending the kit to Bitwig", "10 pads".
pub fn kit_card(feed: &Feed, pads: usize) -> Job {
    let (title, line) = bitwig::sending_kit(pads);
    feed.start(&title, &line)
}

/// Puts a kit's `pads` ([`kit_pads`]) on a Drum Machine in the project open in Bitwig, where `kits` says, and ends
/// `job`'s card ([`kit_card`]) with how it went. Returns the line for under the Kits buttons: which sound went on
/// which pad, or what went wrong and what to do about it. Waits for Bitwig, so never call it on the window's thread.
pub fn send_kit(job: Job, to: &Bitwig, kits: &Kits, name: &str, pads: &[Pad], setting: Option<&str>) -> String {
    match to.send_kit(name, pads, kits.target(setting)) {
        Ok(sent) => {
            let (title, detail) = (sent.title(), sent.detail());
            let line = if sent.drum_machine {
                detail.clone()
            } else {
                // The ID didn't give a Drum Machine: the selected one from now on.
                kits.use_the_selected();
                format!("{title}. {detail}")
            };
            if sent.went_in() {
                job.done_as(&title, &detail);
            } else {
                job.failed_as(&title, &detail);
            }
            line
        }
        // Nothing changed in Bitwig.
        Err(e) => {
            let why = e.to_string();
            job.failed_as(bitwig::KIT_FAILED, &why);
            why
        }
    }
}
