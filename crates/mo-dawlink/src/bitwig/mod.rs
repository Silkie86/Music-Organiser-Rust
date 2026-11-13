//! The Bitwig link: the app talks to the Music Organiser extension (or the script it took over from) running inside
//! Bitwig Studio, over a connection on this computer, and Send to Bitwig puts things into the project open there.
//!
//! The extension itself lives in this repo (`extensions/bitwig-java`, version 3.24; the C# app's copy is frozen):
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
//!   selected in Bitwig, for auditioning (extension 3.19). [`Bitwig::play_with_song`]: the same, with the bars of the
//!   song it was found for going round in Bitwig's arranger (extension 3.22).
//! - [`Bitwig::audition_preset`] then [`Bitwig::stop_audition`] or [`Bitwig::keep_audition`]: a preset on a track of
//!   its own playing a part (extension 3.20).
//! - [`Bitwig::put_in_song`]: a MIDI part into the arranger at a song's bars, on the selected track (Bitwig records
//!   it in from the clip launcher); [`Bitwig::stop_putting`] stops a recording under way.
//! - [`Bitwig::send_part_for_song`]: [`send_part`] for a part found for a song, with Bitwig's play start moved to
//!   the first of the song's bars, so it marks where the clip goes (extension 3.23).
//! - [`Watch::start`] then [`Watch::now`]: the song as Bitwig shows it now (tracks, selection, transport, cue markers,
//!   the notes each track plays, each track's level), kept up to date on a thread of its own (extension 3.24). It only
//!   reads, and [`Watch::now`] never waits.
//!
//! Each waits for Bitwig, so a screen calls them on a background thread (`mo_app::background::spawn`), never the
//! window's. Everything that changes the project (sends, plays, auditions and their stops) goes in one after the
//! other, in the order it came: a play, an audition or a stop goes in a [`Turn`] from a [`ticket`] the screen takes
//! as the button is pressed. A play or audition asked for again meanwhile stops as its turn comes. Every error, and
//! every result, has words to show as they are.

mod audition;
mod device;
mod error;
#[cfg(any(test, feature = "fake-bitwig"))]
pub mod fake;
mod link;
mod put;
mod send;
pub mod watch;

pub use audition::{AUDITION_NEEDS, Auditioning, PLAY_NEEDS, Playing, SongLoop, WITH_SONG_NEEDS, WithSong};
pub use device::{DRUM_MACHINE_KEY, DRUM_MACHINE_NAME, DeviceId, drum_machine};
pub use error::{Error, LOOPBACK_PORT};
pub use link::{
    CHANGE_TIMEOUT, CONNECT_TIMEOUT, Device, Hello, Inserted, LINK_VERSION, Link, Metre, NewTrack, PLAY_START_NEEDS,
    PORT, PadContents, PadsFilled, Recorded, SelectedTrack, TIMEOUT, TakenOut,
};
pub use put::{PUT_NEEDS, PutIn, Seen};
pub use send::{
    Bitwig, KIT_FAILED, KitSound, KitTarget, Mark, PART_FAILED, PART_IN, PART_PLACE, Pad, SOUND_FAILED, SOUND_PLACE,
    SentKit, SentPart, SentSound, Ticket, Turn, kit_name, kit_pads, note_name, send_kit, send_part, send_preset,
    send_sound, sending_kit, sending_part, sending_sound, ticket,
};
pub use watch::{Live, WATCH_NEEDS, Watch};
