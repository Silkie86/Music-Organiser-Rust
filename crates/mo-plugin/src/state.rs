//! What the plugin keeps in the DAW's project (CLAP's `state` extension): the DAW asks for it when the project is
//! saved or the track is copied, and hands it back when the project opens again.
//!
//! It's a few lines of plain text, a name and a value on each, under a first line that names it: the size the window
//! was dragged to, and where Volume, Loop, Match tempo and In time were left (the knobs that are settings; the list
//! knobs and Sounds like start again with the panel, by design). A line this version doesn't know is skipped, and so
//! is a value that makes no sense, so a project saved by a newer plugin (or a damaged one) still opens: a state that
//! can't be read is never a reason for the DAW to refuse the project.

use crate::knobs::Knob;

/// The first line of every state the plugin writes.
const NAMED: &str = "Music Organiser plugin state";
/// The most of a state that's read: far more than it ever writes.
pub const MOST: u64 = 64 * 1024;

/// The knobs kept, with the name each has in the state (never change one: a project keeps it).
pub const KNOBS: [(Knob, &str); 4] =
    [(Knob::Volume, "volume"), (Knob::Loop, "loop"), (Knob::Match, "match"), (Knob::InTime, "in_time")];

/// What's kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kept {
    /// The panel's size in its own units, when it was written.
    pub panel: Option<(f64, f64)>,
    /// The knobs' values (0 to 1) it has: a knob it doesn't have starts as the panel does.
    pub knobs: Vec<(Knob, f64)>,
}

/// The state as the DAW keeps it: the panel's size, and each knob of [`KNOBS`] as `knob` gives it.
pub fn write(panel: (f64, f64), knob: impl Fn(Knob) -> f64) -> String {
    let mut out = format!("{NAMED}\nwidth={}\nheight={}\n", panel.0, panel.1);
    for (k, name) in KNOBS {
        out.push_str(&format!("{name}={}\n", knob(k)));
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
    Kept { panel: width.zip(height), knobs }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(panel: (f64, f64)) -> Kept {
        Kept { panel: Some(panel), knobs: Vec::new() }
    }

    #[test]
    fn what_is_written_is_read_back() {
        let state = write((1040.0, 712.5), |k| if k == Knob::Volume { 0.4 } else { 1.0 });
        let knobs = vec![(Knob::Volume, 0.4), (Knob::Loop, 1.0), (Knob::Match, 1.0), (Knob::InTime, 1.0)];
        assert_eq!(read(&state), Kept { panel: Some((1040.0, 712.5)), knobs });
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
