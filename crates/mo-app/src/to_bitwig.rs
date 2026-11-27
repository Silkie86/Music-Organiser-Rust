//! Send to Bitwig from the Sounds page (the picked sound, the kit on show, or a sound's slices from the Edit strip)
//! and the MIDI page (the picked part), into
//! the project open in Bitwig, through the DAWmate extension there (`mo_dawlink::bitwig` does the sending, and
//! has the words, the C# app's). Each waits for Bitwig, so the page calls them on a thread of their own, never the
//! window's (rule 12). Each has an activity card from the moment it's asked for, which says how it went once Bitwig
//! answers.
//!
//! What the C# app also did and this doesn't yet: count each send for Most liked (that writes to `sounds.db`, which
//! the Rust app only reads for now), and write the Drum Machine's ID into `library.db` (the Rust app knows Bitwig's
//! own, so it has no need to).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use mo_dawlink::bitwig::{self, KitTarget};
pub use mo_dawlink::bitwig::{Bitwig, DRUM_MACHINE_KEY, KitSound, Pad, kit_name, kit_pads};
use mo_sounds::Sound;
use mo_sounds::kits::KitSlot;

use crate::activity::{Feed, Job};

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

/// The card for a MIDI part on its way, shown at once: "Putting Keys · Pattern 3 into Bitwig", "As a clip on a new
/// track".
pub fn part_card(feed: &Feed, name: &str) -> Job {
    feed.start(&bitwig::sending_part(name), bitwig::PART_PLACE)
}

/// Puts the MIDI file at `path` as a clip on a new track named `name` in the project open in Bitwig, and ends `job`'s
/// card ([`part_card`]) with how it went. Returns whether it went in as asked. Waits for Bitwig, so never call it on
/// the window's thread.
pub fn send_part(job: Job, to: &Bitwig, path: &str, name: &str) -> bool {
    part_sent(job, to.send_part(path, name), &|_| String::new())
}

/// [`send_part`] for a part found for a song (`mo_dawlink::bitwig::Bitwig::send_part_for_song`): Bitwig's play start
/// then marks the first of the song's `bars` it was found for, and the card's line says so, or why not, in the words
/// `says` gives for the mark ([`crate::midi_fits::mark_words`]).
pub fn send_part_for_song(
    job: Job,
    to: &Bitwig,
    path: &str,
    name: &str,
    bars: &bitwig::SongLoop,
    says: &dyn Fn(&bitwig::Mark) -> String,
) -> bool {
    part_sent(job, to.send_part_for_song(path, name, bars), says)
}

/// Ends a sent part's card with how it went (`sent`); one that went in as asked also says what `says` gives for its
/// mark. Returns whether it went in as asked.
fn part_sent(job: Job, sent: Result<bitwig::SentPart, bitwig::Error>, says: &dyn Fn(&bitwig::Mark) -> String) -> bool {
    match sent {
        Ok(sent) if sent.went_in() => {
            // The mark's words first: the rail shows a done card as one cut-off line (the review's BW-8).
            let mark = says(&sent.mark);
            let detail = format!("{} {}", mark.trim(), sent.detail());
            match sent.mark {
                bitwig::Mark::NotAsked | bitwig::Mark::Moved => job.done_as(&sent.title(), detail.trim()),
                // The part is in, but its bar isn't marked, and the words say what to do about it: a card that
                // stays until it's closed, not a line gone in a few seconds. Done all the same: the part went in.
                _ => job.done_to_read(&format!("{}, but its bar isn't marked", sent.title()), detail.trim()),
            }
            true
        }
        Ok(sent) => {
            job.failed_as(&sent.title(), &sent.detail());
            false
        }
        Err(e) => {
            job.failed_as(bitwig::PART_FAILED, &e.to_string());
            false
        }
    }
}

/// The kit on show as the link takes it: each pad that has a sound, by the pad's name, with the kind of sound the pad
/// is for (its first type, which places it the General MIDI way) and the sound's file, in the kit's order. Pads with no
/// sound, and sounds no longer in the list, are left out. `sound` finds a sound by its id: the page's own map of them,
/// not a walk through the whole list for each pad (review G-6). Give it to [`kit_pads`] for the pads.
pub fn kit_sounds<'a>(slots: &[KitSlot], sound: impl Fn(i64) -> Option<&'a Sound>) -> Vec<KitSound> {
    slots
        .iter()
        .filter_map(|slot| {
            let sound = sound(slot.sound?)?;
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
/// `job`'s card ([`kit_card`]) with how it went. Returns the line for under the Kits buttons (which sound went on
/// which pad, or what went wrong and what to do about it), and whether the kit went in as asked. Waits for Bitwig, so
/// never call it on the window's thread.
pub fn send_kit(job: Job, to: &Bitwig, kits: &Kits, name: &str, pads: &[Pad], setting: Option<&str>) -> (String, bool) {
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
            let went_in = sent.went_in();
            if went_in {
                job.done_as(&title, &detail);
            } else {
                job.failed_as(&title, &detail);
            }
            (line, went_in)
        }
        // Nothing changed in Bitwig.
        Err(e) => {
            let why = e.to_string();
            job.failed_as(bitwig::KIT_FAILED, &why);
            (why, false)
        }
    }
}

/// The pad a sound's first slice goes on: C1 as Bitwig names it, the first pad of a Drum Machine's first page. The
/// rest follow, a note up each.
pub const FIRST_SLICE_PAD: u8 = 36;

/// The note of the pad the slice `i` (from 0) goes on.
fn slice_note(i: usize) -> u8 {
    u8::try_from(usize::from(FIRST_SLICE_PAD) + i).map_or(127, |note| note.min(127))
}

/// The pad the slice `i` (from 0) goes on, by its note as Bitwig names it: "C1", "C#1", "D1"…
pub fn slice_pad_name(i: usize) -> String {
    bitwig::note_name(slice_note(i))
}

/// A sound's slices (their files, in order: `edit_copy::slices_to_keep`) as the link takes them: one to a pad, from
/// [`FIRST_SLICE_PAD`] up, named "Slice 1", "Slice 2"… (a slice's file is named for its sound and its slicing, too
/// long for a pad).
pub fn slice_pads(files: &[PathBuf]) -> Vec<Pad> {
    files
        .iter()
        .enumerate()
        .map(|(i, file)| Pad {
            note: slice_note(i),
            path: file.to_string_lossy().into_owned(),
            name: format!("Slice {}", i + 1),
        })
        .collect()
}

/// "1 slice", "20 slices".
pub fn slices_text(slices: usize) -> String {
    if slices == 1 { "1 slice".to_owned() } else { format!("{slices} slices") }
}

/// The card for a sound's slices on their way, shown at once: "Sending 20 slices of Loop 128 to Bitwig", "Making the
/// slices" (they're saved first).
pub fn slices_card(feed: &Feed, name: &str, slices: usize) -> Job {
    feed.start(&format!("Sending {} of {name} to Bitwig", slices_text(slices)), "Making the slices")
}

/// The card's headline when slices couldn't be sent at all; the line under it is the error.
pub const SLICES_FAILED: &str = "Couldn't send the slices";

/// Puts a sound's slices ([`slice_pads`]) on a Drum Machine in the project open in Bitwig, where `kits` says (as a kit
/// goes: [`send_kit`]), on a track named for the sound ("Loop 128 (sliced)"), and ends `job`'s card ([`slices_card`])
/// with how it went: "Loop 128 (sliced) is in Bitwig", "20 slices on a Drum Machine on a new track, C1 to G2"; or what
/// went wrong and what to do about it. Returns whether they went in as asked. Waits for Bitwig, so never call it on
/// the window's thread.
pub fn send_slices(job: Job, to: &Bitwig, kits: &Kits, name: &str, pads: &[Pad], setting: Option<&str>) -> bool {
    match to.send_kit(&format!("{name} (sliced)"), pads, kits.target(setting)) {
        Ok(sent) => {
            if !sent.drum_machine {
                // The ID didn't give a Drum Machine: the selected one from now on.
                kits.use_the_selected();
            }
            let went_in = sent.went_in();
            match (sent.filled.first(), sent.filled.last()) {
                // All of them, where they were sent: said in a line, not pad by pad.
                (Some(first), Some(last)) if went_in && sent.skipped.is_empty() => {
                    let place = match sent.target {
                        KitTarget::Selected => "the Drum Machine selected in Bitwig",
                        KitTarget::NewDrumMachine(_) => "a Drum Machine on a new track",
                    };
                    let pads = if sent.filled.len() == 1 {
                        bitwig::note_name(first.note)
                    } else {
                        format!("{} to {}", bitwig::note_name(first.note), bitwig::note_name(last.note))
                    };
                    job.done_as(&sent.title(), &format!("{} on {place}, {pads}", slices_text(sent.filled.len())));
                }
                _ if went_in => job.done_as(&sent.title(), &of_slices(&sent.detail())),
                _ => job.failed_as(&sent.title(), &of_slices(&sent.detail())),
            }
            went_in
        }
        // Nothing changed in Bitwig.
        Err(e) => {
            job.failed_as(SLICES_FAILED, &of_slices(&e.to_string()));
            false
        }
    }
}

/// The link's words about a kit, said of a sound's slices: they go the kits' way in, so the link (and the extension
/// in Bitwig) calls them a kit, and "send the kit again" would send him to the Kits for what the Edit strip sent.
fn of_slices(words: &str) -> String {
    words
        .replace("send the kit again", "send the slices again")
        .replace("track for the kit", "track for the slices")
        .replace("The kit's track", "The slices' track")
}
