//! What the plugin keeps in the DAW's project (CLAP's `state` extension): the DAW asks for it when the project is
//! saved or the track is copied, and hands it back when the project opens again.
//!
//! It's a few lines of plain text, a name and a value on each, under a first line that names it. For now only the
//! size the window was dragged to. A line this version doesn't know is skipped, and so is a value that makes no
//! sense, so a project saved by a newer plugin (or a damaged one) still opens: a state that can't be read is never
//! a reason for the DAW to refuse the project.

/// The first line of every state the plugin writes.
const NAMED: &str = "Music Organiser plugin state";
/// The most of a state that's read: far more than it ever writes.
pub const MOST: u64 = 64 * 1024;

/// What's kept.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Kept {
    /// The panel's size in its own units, when it was written.
    pub panel: Option<(f64, f64)>,
}

/// The state as the DAW keeps it.
pub fn write(panel: (f64, f64)) -> String {
    format!("{NAMED}\nwidth={}\nheight={}\n", panel.0, panel.1)
}

/// What a kept state says: nothing at all for one that isn't the plugin's.
pub fn read(state: &str) -> Kept {
    let mut lines = state.lines();
    if lines.next().map(str::trim) != Some(NAMED) {
        return Kept::default();
    }
    let (mut width, mut height) = (None, None);
    for line in lines {
        let Some((name, value)) = line.split_once('=') else { continue };
        let number = value.trim().parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0);
        match name.trim() {
            "width" => width = number,
            "height" => height = number,
            _ => {}
        }
    }
    Kept { panel: width.zip(height) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_written_is_read_back() {
        assert_eq!(read(&write((1040.0, 712.5))), Kept { panel: Some((1040.0, 712.5)) });
    }

    #[test]
    fn a_state_that_makes_no_sense_reads_as_nothing_kept() {
        assert_eq!(read(""), Kept::default());
        assert_eq!(read("something else\nwidth=900\nheight=600\n"), Kept::default());
        assert_eq!(read(&format!("{NAMED}\nwidth=900\n")), Kept::default());
        assert_eq!(read(&format!("{NAMED}\nwidth=NaN\nheight=-4\n")), Kept::default());
    }

    #[test]
    fn lines_a_newer_plugin_wrote_are_skipped() {
        let state = format!("{NAMED}\nview=galaxy\nwidth=900\nno equals here\nheight=600\n");
        assert_eq!(read(&state), Kept { panel: Some((900.0, 600.0)) });
    }
}
