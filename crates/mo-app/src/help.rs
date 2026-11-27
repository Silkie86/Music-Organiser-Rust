//! The help on the key sheet (F1, ?, or the ? beside Settings on the rail): for each place, one line on what it's for and a few tips,
//! above its keys. Short on purpose (Solomon, 24 Nov 2026: nothing clunky or over-explained): the pages explain
//! themselves through icons and tooltips, and this is only the "where do I start" for someone new, such as a beta
//! tester. Keys are never written here: the key sheet lists them under the tips, from the shortcut list (rule 15), so
//! a changed key can never make the help wrong.

use crate::places::Place;

/// The most tips a place gets, so the sheet's keys stay in sight without scrolling.
pub const MAX_TIPS: usize = 4;

/// The longest a line may be, so each tip fits on one or two lines of the sheet.
pub const MAX_LINE: usize = 110;

/// One place's help.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageHelp {
    /// What the place is for, in one line.
    pub what: &'static str,
    /// Where to start, one line each.
    pub tips: &'static [&'static str],
}

/// The help for `place`.
pub fn for_place(place: Place) -> PageHelp {
    match place {
        Place::Songs => PageHelp {
            what: "Every song in your project folders: FL Studio, Bitwig, Studio One and Ableton Live.",
            tips: &[
                "Click a song for its card: open it, hear its bounce, keep its to-dos.",
                "Next up lists every to-do left; Board sorts songs from Idea to Released.",
                "Dust off brings back songs you spent hours on, then left.",
                "Open in another DAW makes a new copy. The song itself is never changed.",
            ],
        },
        Place::Sounds => PageHelp {
            what: "Every sample in your sound folders, to hear and drag into a DAW.",
            tips: &[
                "Click a sound to hear it; drag it into your DAW. Pick a part, or Edit a copy, on its card.",
                "Narrow the list by type, key and year. Tick Favourites for Sounds like them.",
                "Galaxy puts alike sounds together: sweep the mouse over the stars to hear them.",
                "Hide duplicates only hides copies in the app. No sound is ever moved or deleted.",
            ],
        },
        Place::Presets => PageHelp {
            what: "Synth and effect presets, from your songs and your preset folders.",
            tips: &[
                "Drag a preset into Bitwig, or Send to Bitwig, for a track with that sound.",
                "Audition in Bitwig plays it on a new track: Keep it, or Stop to take it out.",
            ],
        },
        Place::Curves => PageHelp {
            what: "LFO and volume shapes from Serum, Vital, LFOTool and Duck, to edit and reuse.",
            tips: &[
                "Pick a curve to paint, randomise, join or quantise it. Undo takes back a step.",
                "Drag a curve into Bitwig, or Hear it on a tone first.",
            ],
        },
        Place::Midi => PageHelp {
            what: "Every MIDI file in your MIDI folders, with its key and chords.",
            tips: &[
                "Pick a part and play it; change the BPM or add the metronome as it plays.",
                "Fits my song finds parts that suit one of your songs, and plays them with it.",
                "Drag a part into your DAW.",
            ],
        },
        Place::Plugins => PageHelp {
            what: "Every plugin you have, and how much room its sounds and presets take.",
            tips: &[
                "Move to another drive moves a big library and leaves a link, so the plugin still finds it.",
                "Clean up and Uninstall send files to the Recycle Bin, with Put back.",
            ],
        },
        Place::Space => PageHelp {
            what: "Where your drives' room goes, and spare copies you could let go.",
            tips: &[
                "Only what you tick goes, to the Recycle Bin. Put back returns it.",
                "Fix writes a \"(fixed)\" project beside a Bitwig song. The original is never changed.",
            ],
        },
        Place::Backup => PageHelp {
            what: "Copies your songs and the sounds they use to a spare drive.",
            tips: &[
                "Pick a drive, add your folders, then Back up now.",
                "Older saves, and files you deleted, are kept on the backup.",
            ],
        },
        Place::Settings => PageHelp {
            what: "Where the app looks, its links to your DAWs, your keys and its look.",
            tips: &[
                "Folders: add where your songs, samples and MIDI are; each page fills from them.",
                "DAWs: put in or take out the Bitwig extension and the FL Studio script.",
                "Changes are kept as you make them.",
            ],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_place_has_short_help() {
        for place in Place::ALL {
            let help = for_place(place);
            assert!(!help.what.is_empty(), "{place:?} says what it's for");
            assert!(!help.tips.is_empty() && help.tips.len() <= MAX_TIPS, "{place:?} has 1 to {MAX_TIPS} tips");
            for line in std::iter::once(help.what).chain(help.tips.iter().copied()) {
                assert!(line.chars().count() <= MAX_LINE, "{place:?}: too long: {line}");
                assert!(line.ends_with('.'), "{place:?}: a whole sentence: {line}");
                // Plain ASCII only, so it reads the same in every font the app may fall back to.
                assert!(line.is_ascii(), "{place:?}: plain characters only: {line}");
            }
        }
    }

    #[test]
    fn the_help_never_names_a_key() {
        // Keys can be changed in Settings, so the help leaves them to the sheet's key list (rule 15).
        let keys = ["Ctrl+", "Alt+", "Shift+", "F1", "F5", "F11", "F12", "Esc", "Space bar"];
        for place in Place::ALL {
            let help = for_place(place);
            for line in std::iter::once(help.what).chain(help.tips.iter().copied()) {
                for key in keys {
                    assert!(!line.contains(key), "{place:?} names {key}: {line}");
                }
            }
        }
    }
}
