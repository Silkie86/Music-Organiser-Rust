//! The Bitwig link: the app talks to the Music Organiser extension (or the script it took over from) running inside
//! Bitwig Studio, over a connection on this computer, and Send to Bitwig puts things into the project open there.
//!
//! The extension itself lives in this repo (`extensions/bitwig-java`, version 3.19; the C# app's copy is frozen):
//! Bitwig only runs Java extensions. Settings > DAWs installs it. Changing anything needs "Let the app make changes"
//! on in Bitwig (Settings > Controllers > Music Organiser); reading never does.
//!
//! For the screens:
//!
//! - [`send_sound`]`(path, name)`: one sound on a Sampler, on a new track named after it.
//! - [`kit_pads`] then [`send_kit`]: a kit on a new Drum Machine ([`drum_machine`] picks Bitwig's ID for it), with
//!   each sound on the pad General MIDI drum parts play (kick on C1, snare on D1, closed hat on F#1...).
//! - [`Bitwig::check`]: what Bitwig has open, and whether a send would go in now.
//! - [`Bitwig::play_on_selected`] then [`Bitwig::stop_playing`]: a MIDI part looping through what's on the track
//!   selected in Bitwig, for auditioning (extension 3.19).
//!
//! Each waits for Bitwig, so a screen calls them on a background thread (`mo_app::background::spawn`), never the
//! window's. Sends asked for at the same time go in one after the other, in the order they came. Every error, and
//! every result, has words to show as they are.

mod audition;
mod device;
mod error;
#[cfg(any(test, feature = "fake-bitwig"))]
pub mod fake;
mod link;
mod send;

pub use audition::{PLAY_NEEDS, Playing};
pub use device::{DRUM_MACHINE_KEY, DRUM_MACHINE_NAME, DeviceId, drum_machine};
pub use error::{Error, LOOPBACK_PORT};
pub use link::{
    CHANGE_TIMEOUT, CONNECT_TIMEOUT, Device, Hello, Inserted, LINK_VERSION, Link, NewTrack, PORT, PadContents,
    PadsFilled, SelectedTrack, TIMEOUT,
};
pub use send::{
    Bitwig, KIT_FAILED, KitSound, KitTarget, PART_FAILED, PART_IN, PART_PLACE, Pad, SOUND_FAILED, SOUND_PLACE, SentKit,
    SentPart, SentSound, kit_name, kit_pads, note_name, send_kit, send_part, send_preset, send_sound, sending_kit,
    sending_part, sending_sound,
};
