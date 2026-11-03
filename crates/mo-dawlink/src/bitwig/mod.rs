//! The Bitwig link: the app talks to the Music Organiser extension (or the script it took over from) running inside
//! Bitwig Studio, over a connection on this computer, and Send to Bitwig puts things into the project open there.
//!
//! The extension itself is the C# app's (`extensions/bitwig-java` in `Silkie86/Music-Organiser`, version 3.18), which
//! stays as it is (spec section 2): Bitwig only runs Java extensions. Installing it is still done from the old app's
//! Settings > DAW links. Changing anything needs "Let the app make changes" on in Bitwig (Settings > Controllers >
//! Music Organiser); reading never does.
//!
//! For the screens:
//!
//! - [`send_sound`]`(path, name)`: one sound on a Sampler, on a new track named after it.
//! - [`kit_pads`] then [`send_kit`]: a kit on a new Drum Machine ([`drum_machine`] picks Bitwig's ID for it), with
//!   each sound on the pad General MIDI drum parts play (kick on C1, snare on D1, closed hat on F#1...).
//! - [`Bitwig::check`]: what Bitwig has open, and whether a send would go in now.
//!
//! Each waits for Bitwig, so a screen calls them on a background thread (`mo_app::background::spawn`), never the
//! window's. Sends made at the same time each go in, one after the other: the link in Bitwig makes changes one at a
//! time, in the order they came. Every error, and every result, has words to show as they are.

mod device;
mod error;
#[cfg(any(test, feature = "fake-bitwig"))]
pub mod fake;
mod link;
mod send;

pub use device::{DRUM_MACHINE_KEY, DRUM_MACHINE_NAME, DeviceId, drum_machine};
pub use error::{Error, LOOPBACK_PORT};
pub use link::{
    CHANGE_TIMEOUT, CONNECT_TIMEOUT, Device, Hello, Inserted, LINK_VERSION, Link, NewTrack, PORT, PadContents,
    PadsFilled, TIMEOUT,
};
pub use send::{
    Bitwig, KIT_FAILED, KitSound, KitTarget, Pad, SOUND_FAILED, SOUND_PLACE, SentKit, SentSound, kit, kit_name,
    kit_pads, note_name, send_kit, send_sound, sending_kit, sending_sound, sound,
};
