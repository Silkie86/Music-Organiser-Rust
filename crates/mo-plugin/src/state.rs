//! What the plugin keeps in the DAW's project (CLAP's `state` extension): the DAW asks for it when the project is
//! saved or the track is copied, and hands it back when the project opens again.
//!
//! It's a few lines of plain text, a name and a value on each, under a first line that names it: the size the window
//! was dragged to, where Volume, Loop, Match tempo and In time were left (the knobs that are settings; the list
//! knobs and Sounds like start again with the panel, by design), and what Record records from (the input by its name,
//! and L+R, L or R). A line this version doesn't know is skipped, and so is a value that makes no sense, so a project
//! saved by a newer plugin (or a damaged one) still opens: a state that can't be read is never a reason for the DAW to
//! refuse the project.
//!
//! How it grows: the first line stays the same, and a new thing kept is a new line with a name of its own. An older
//! plugin skips the lines it doesn't know (so it reads a newer state's size and knobs as before), and a newer one
//! reading an older state just doesn't find the new lines (so what they keep starts as it always did). Nothing is ever
//! renamed or given another meaning: a project keeps its state for years.

use crate::knobs::Knob;

/// The first line of every state the plugin writes. It keeps the app's first name: songs saved with the plugin
/// have it, and a state without it isn't read.
const NAMED: &str = "Music Organiser plugin state";
/// The most of a state that's read: far more than it ever writes.
pub const MOST: u64 = 64 * 1024;

/// The knobs kept, with the name each has in the state (never change one: a project keeps it).
pub const KNOBS: [(Knob, &str); 4] =
    [(Knob::Volume, "volume"), (Knob::Loop, "loop"), (Knob::Match, "match"), (Knob::InTime, "in_time")];

/// The line with the sound card input Record records from, by the name the system gives it ("Input 1/2 (Bitwig
/// Connect)"): never its place in the list, which changes as cards come and go. Not written for the DAW's input.
const RECORD_FROM: &str = "record_from";
/// The line with the side of it Record records: both, left or right, by these names (the panel's 0, 1 and 2).
const RECORD_SIDE: &str = "record_side";
const SIDES: [&str; 3] = ["lr", "l", "r"];

/// What's kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kept {
    /// The panel's size in its own units, when it was written.
    pub panel: Option<(f64, f64)>,
    /// The knobs' values (0 to 1) it has: a knob it doesn't have starts as the panel does.
    pub knobs: Vec<(Knob, f64)>,
    /// What Record records from: a sound card input by its name, or "" for the DAW's input. None when the state doesn't
    /// say (written before it was kept): it starts as the plugin does, on the DAW's input.
    pub record_from: Option<String>,
    /// Which side: 0 both (L+R), 1 left, 2 right. None when the state doesn't say.
    pub record_side: Option<i32>,
}

/// What Record was left on, for [`write`]: the input's name ("" for the DAW's input), and the side (0 both, 1 left, 2
/// right).
#[derive(Clone, Copy, Debug, Default)]
pub struct Record<'a> {
    pub from: &'a str,
    pub side: i32,
}

/// The state as the DAW keeps it: the panel's size, each knob of [`KNOBS`] as `knob` gives it, and what Record records
/// from.
pub fn write(panel: (f64, f64), knob: impl Fn(Knob) -> f64, record: Record) -> String {
    let mut out = format!("{NAMED}\nwidth={}\nheight={}\n", panel.0, panel.1);
    for (k, name) in KNOBS {
        out.push_str(&format!("{name}={}\n", knob(k)));
    }
    // A name on one line, as every value is; the DAW's input is the plugin's own start, so it needs no line. A name
    // with a line break in it (no system gives one) would spoil the lines after it: it's left out, and the take starts
    // on the DAW's input.
    if !record.from.is_empty() && !record.from.contains(['\n', '\r']) {
        out.push_str(&format!("{RECORD_FROM}={}\n", record.from));
    }
    if let Some(side) = usize::try_from(record.side).ok().and_then(|s| SIDES.get(s)) {
        out.push_str(&format!("{RECORD_SIDE}={side}\n"));
    }
    out
}

/// What a kept state says: nothing at all for one that isn't the plugin's.
pub fn read(state: &str) -> Kept {
    let mut lines = state.lines();
    if lines.next().map(str::trim) != Some(NAMED) {
        return Kept::default();
    }
    let (mut width, mut height, mut knobs) = (None, None, Vec::new());
    let (mut record_from, mut record_side) = (None, None);
    for line in lines {
        let Some((name, words)) = line.split_once('=') else { continue };
        let value = words.trim().parse::<f64>().ok().filter(|n| n.is_finite());
        let size = value.filter(|n| *n > 0.0);
        match name.trim() {
            "width" => width = size,
            "height" => height = size,
            // The name as written: only the line's end is the state's (a "\r" if it went through Windows' notepad).
            RECORD_FROM => record_from = Some(words.trim_end_matches('\r').to_owned()),
            RECORD_SIDE => {
                let side = SIDES.iter().position(|s| *s == words.trim());
                record_side = side.and_then(|s| i32::try_from(s).ok());
            }
            name => {
                let knob = KNOBS.iter().find(|(_, n)| *n == name).map(|(k, _)| *k);
                if let (Some(knob), Some(value)) = (knob, value.filter(|v| (0.0..=1.0).contains(v))) {
                    knobs.retain(|(k, _)| *k != knob);
                    knobs.push((knob, value));
                }
            }
        }
    }
    // A state that keeps a side keeps the input it's of: none said there is the DAW's input (not written, as it's the
    // start). One that keeps neither was written before they were kept.
    if record_side.is_some() && record_from.is_none() {
        record_from = Some(String::new());
    }
    Kept { panel: width.zip(height), knobs, record_from, record_side }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(panel: (f64, f64)) -> Kept {
        Kept { panel: Some(panel), ..Kept::default() }
    }

    fn all_knobs() -> Vec<(Knob, f64)> {
        vec![(Knob::Volume, 0.4), (Knob::Loop, 1.0), (Knob::Match, 1.0), (Knob::InTime, 1.0)]
    }

    fn knob(k: Knob) -> f64 {
        if k == Knob::Volume { 0.4 } else { 1.0 }
    }

    #[test]
    fn what_is_written_is_read_back() {
        let state = write((1040.0, 712.5), knob, Record { from: "Input 1/2 (Bitwig Connect)", side: 2 });
        let kept = Kept {
            panel: Some((1040.0, 712.5)),
            knobs: all_knobs(),
            record_from: Some("Input 1/2 (Bitwig Connect)".into()),
            record_side: Some(2),
        };
        assert_eq!(read(&state), kept);
        // The DAW's input, both sides: the plugin's own start, said as such.
        let state = write((1040.0, 712.5), knob, Record::default());
        assert!(!state.contains("record_from"), "{state}");
        let kept = read(&state);
        assert_eq!((kept.record_from.as_deref(), kept.record_side), (Some(""), Some(0)));
    }

    /// The state a plugin from before Record was kept (0.9.0-beta.2, PL-4) wrote, word for word.
    const OLDER: &str =
        "Music Organiser plugin state\nwidth=1280\nheight=800\nvolume=0.4\nloop=1\nmatch=0\nin_time=1\n";

    #[test]
    fn a_state_from_an_older_plugin_reads_as_it_did() {
        let knobs = vec![(Knob::Volume, 0.4), (Knob::Loop, 1.0), (Knob::Match, 0.0), (Knob::InTime, 1.0)];
        // Record isn't said, so it starts as the plugin does (the DAW's input, both sides).
        let kept = Kept { panel: Some((1280.0, 800.0)), knobs, record_from: None, record_side: None };
        assert_eq!(read(OLDER), kept);
    }

    /// What the older plugin read: the panel's size and the knobs.
    type OlderKept = (Option<(f64, f64)>, Vec<(Knob, f64)>);

    /// The older plugin's reader (state.rs as it was before Record was kept), word for word, to check what it makes of
    /// a state this one writes.
    fn read_as_older(state: &str) -> OlderKept {
        let mut lines = state.lines();
        if lines.next().map(str::trim) != Some(NAMED) {
            return (None, Vec::new());
        }
        let (mut width, mut height, mut knobs) = (None, None, Vec::new());
        for line in lines {
            let Some((name, value)) = line.split_once('=') else { continue };
            let value = value.trim().parse::<f64>().ok().filter(|n| n.is_finite());
            let size = value.filter(|n| *n > 0.0);
            match name.trim() {
                "width" => width = size,
                "height" => height = size,
                name => {
                    let knob = KNOBS.iter().find(|(_, n)| *n == name).map(|(k, _)| *k);
                    if let (Some(knob), Some(value)) = (knob, value.filter(|v| (0.0..=1.0).contains(v))) {
                        knobs.retain(|(k, _)| *k != knob);
                        knobs.push((knob, value));
                    }
                }
            }
        }
        (width.zip(height), knobs)
    }

    #[test]
    fn an_older_plugin_reads_a_newer_state_as_before() {
        // Names that look like numbers, or hold "=", are still only Record's.
        for from in ["Input 1/2 (Bitwig Connect)", "0.5", "Line = In (USB)", ""] {
            for side in 0..3 {
                let state = write((960.0, 640.0), knob, Record { from, side });
                assert_eq!(read_as_older(&state), (Some((960.0, 640.0)), all_knobs()), "{state}");
            }
        }
        // And the older reader reads its own state as this one does.
        let kept = read(OLDER);
        assert_eq!(read_as_older(OLDER), (kept.panel, kept.knobs));
    }

    #[test]
    fn record_lines_that_make_no_sense_are_skipped() {
        let state = format!("{NAMED}\nwidth=900\nheight=600\nrecord_side=middle\nrecord_from=Mic (USB)\r\n");
        let kept = read(&state);
        assert_eq!(kept.record_side, None, "an unknown side is the plugin's start");
        assert_eq!(kept.record_from.as_deref(), Some("Mic (USB)"), "a Windows line end isn't part of the name");
        // A side out of range isn't written, nor a name that would break the lines.
        let state = write((900.0, 600.0), knob, Record { from: "two\nlines", side: 7 });
        assert!(!state.contains("record_"), "{state}");
        let state = write((900.0, 600.0), knob, Record { from: "Mic", side: -1 });
        assert_eq!(read(&state).record_side, None);
        assert_eq!(read(&state).record_from.as_deref(), Some("Mic"));
    }

    #[test]
    fn a_damaged_state_never_panics() {
        // Every cut of a whole state, and a few bytes spoiled, read as something (or nothing) without a panic.
        let state = write((1040.0, 712.5), knob, Record { from: "Input 3 (Scarlett 2i2 USB)", side: 1 });
        for end in 0..=state.len() {
            if state.is_char_boundary(end) {
                let _ = read(&state[..end]);
            }
        }
        let mut bytes = state.into_bytes();
        for at in (0..bytes.len()).step_by(7) {
            bytes[at] = bytes[at].wrapping_mul(31).wrapping_add(at as u8);
            let _ = read(&String::from_utf8_lossy(&bytes));
        }
    }

    #[test]
    fn a_state_that_makes_no_sense_reads_as_nothing_kept() {
        assert_eq!(read(""), Kept::default());
        assert_eq!(read("something else\nwidth=900\nheight=600\n"), Kept::default());
        assert_eq!(read(&format!("{NAMED}\nwidth=900\n")), Kept::default());
        assert_eq!(read(&format!("{NAMED}\nwidth=NaN\nheight=-4\n")), Kept::default());
        assert_eq!(read(&format!("{NAMED}\nvolume=7\nloop=NaN\n")), Kept::default());
    }

    #[test]
    fn lines_a_newer_plugin_wrote_are_skipped() {
        let state = format!("{NAMED}\nview=galaxy\nwidth=900\nno equals here\nheight=600\n");
        assert_eq!(read(&state), size((900.0, 600.0)));
    }
}
