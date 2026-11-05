//! What can go wrong talking to Bitwig, in words to show as they are (`to_string()`). The words are the C# app's
//! (`BitwigLink`, `BitwigSend`), which Solomon has seen there, except where the Rust app has no screen for what they
//! pointed at yet (Settings > DAW links): those point at the old app's.

use std::fmt;

/// The MIDI port the extension (3.7 and later) has its MIDI in and out set to, for its loopback (made in loopMIDI on
/// Windows). Never FL Studio's port, "Music Organiser".
pub const LOOPBACK_PORT: &str = "Music Organiser Bitwig";

/// Why the app couldn't do what it asked Bitwig to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Nothing answered on the link's port: Bitwig isn't open, or the Music Organiser controller isn't added there.
    NotRunning,
    /// Something answered on the port, but it didn't greet the app the way the link does.
    NotTheLink { port: u16 },
    /// The link in Bitwig is older than link version 2: it can be read, but takes no tracks, devices or kits.
    OldLink,
    /// "Let the app make changes" is off in the controller's settings in Bitwig.
    ChangesOff,
    /// Part of the extension didn't start in Bitwig, so it greeted the app with only what it knew; this is why, in
    /// the extension's words.
    NotStarted(String),
    /// Bitwig said no, in the link's own words (they're written to be shown as they are).
    Bitwig(String),
    /// Bitwig didn't answer in time. The app closes the connection then (see [`super::Link`]).
    TimedOut,
    /// Bitwig closed the connection (it quit, or the controller was turned off).
    Closed,
    /// The connection was closed before: Bitwig closed it, it broke, or an answer didn't come in time.
    NotConnected,
    /// The connection broke; the system's words for why.
    Lost(String),
    /// Bitwig's answer wasn't laid out the way the link's are.
    Unexpected,
    /// The app asked for something the link doesn't take (a mistake in the app, not Bitwig's answer), and sent
    /// nothing: why, in words.
    Invalid(String),
    /// The sound to send isn't where the app last saw it.
    Missing { file: String, path: String },
    /// None of a kit's sounds are where the app last saw them: how many, and the first of them.
    KitMissing { sounds: usize, file: String, path: String },
    /// A kit with no sounds on it: nothing to send.
    NoPads,
    /// Bitwig has an older extension than something needs: the version it has ("" for the script), and the one needed.
    OldExtension { has: String, needs: &'static str },
    /// No track is selected in Bitwig, for something that plays on the selected track.
    NoTrackSelected,
    /// The selected track can't play notes (an audio, group, effect or master track): its name and Bitwig's kind.
    NotAnInstrumentTrack { name: String, kind: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Bitwig starts the extension 3.7 and later, and the link answers, with its MIDI in and out unassigned, but
            // it stops a controller whose device goes away, so a missing loopback port may be why.
            Error::NotRunning => write!(
                f,
                "Bitwig isn't answering. Check that Bitwig Studio is open, with the Music Organiser controller added \
                 (Settings > Controllers > Add controller > Music Organiser > DAW link). If that controller's MIDI in \
                 and out are set to \u{201c}{LOOPBACK_PORT}\u{201d}, check that the port is there (on Windows, that \
                 loopMIDI is running): Bitwig may not start a controller whose port is missing."
            ),
            // The link greets from Bitwig's controller thread, which may be slow to come to it while Bitwig is busy.
            Error::NotTheLink { port } => write!(
                f,
                "Something answered on port {port}, but not the Music Organiser script in Bitwig. If Bitwig is busy \
                 (opening a project, say), try again in a moment."
            ),
            Error::OldLink => f.write_str(
                "Bitwig has an older copy of the Music Organiser script. Install it again from the old app's Settings \
                 > DAW links (Bitwig picks up the new one by itself), then try again.",
            ),
            Error::ChangesOff => f.write_str(
                "Bitwig isn't letting the app make changes. Turn on \"Let the app make changes\" in Bitwig (Settings \
                 > Controllers > Music Organiser), then send it again.",
            ),
            Error::NotStarted(why) => write!(
                f,
                "Part of the Music Organiser extension didn't start in Bitwig, so the app didn't send anything. \
                 Restarting Bitwig may help. Bitwig said: {why}"
            ),
            Error::Bitwig(words) => f.write_str(words),
            Error::TimedOut => f.write_str("Bitwig didn't answer in time."),
            Error::Closed => f.write_str("Bitwig closed the connection."),
            Error::NotConnected => f.write_str("The connection to Bitwig is closed."),
            Error::Lost(why) if why.is_empty() => f.write_str("Lost the connection to Bitwig."),
            Error::Lost(why) => write!(f, "Lost the connection to Bitwig ({why})."),
            Error::Unexpected => f.write_str(
                "Bitwig's answer wasn't what the app expected. Install the extension again from the old app's \
                 Settings > DAW links, in case Bitwig has an older version.",
            ),
            Error::Invalid(why) => f.write_str(why),
            Error::Missing { file, path } => write!(f, "{file} isn't at {path} any more."),
            Error::KitMissing { sounds, file, path } => write!(
                f,
                "None of the kit's {sounds} sounds are where the app last saw them ({file} was at {path}), so nothing \
                 was sent."
            ),
            Error::NoPads => f.write_str("Roll a kit first."),
            Error::OldExtension { has, needs } if has.is_empty() => write!(
                f,
                "This needs the Music Organiser extension {needs} in Bitwig, and Bitwig has the older script. Install \
                 the extension from Settings > DAWs (close Bitwig first on Windows), then try again."
            ),
            Error::OldExtension { has, needs } => write!(
                f,
                "This needs the Music Organiser extension {needs} in Bitwig, and Bitwig has {has}. Install it again \
                 from Settings > DAWs (close Bitwig first on Windows), then try again."
            ),
            Error::NoTrackSelected => {
                f.write_str("Select a track in Bitwig first: the part plays through what's on the selected track.")
            }
            Error::NotAnInstrumentTrack { name, kind } => write!(
                f,
                "\u{201c}{name}\u{201d} is {} track in Bitwig, so it can't play notes. Select an instrument track.",
                a_kind(kind)
            ),
        }
    }
}

/// "an audio", "a group"... for Bitwig's word for a track's kind.
fn a_kind(kind: &str) -> String {
    let kind = kind.to_lowercase();
    if kind.is_empty() {
        "not an instrument".into()
    } else if kind.starts_with(['a', 'e', 'i', 'o', 'u']) {
        format!("an {kind}")
    } else {
        format!("a {kind}")
    }
}

impl std::error::Error for Error {}

impl Error {
    /// Whether Bitwig refused a request, or stopped a change part way, because another project came to the front
    /// than the one the change was kept to ([`super::Link::only_in`]), in the words the extension and the script both
    /// use (the C# app's `BitwigLink.IsElsewhere`).
    pub fn is_elsewhere(&self) -> bool {
        matches!(self, Error::Bitwig(words) if words.starts_with("Bitwig has \"")
            && words.ends_with("\", so the link stopped before changing anything there."))
    }
}
