//! The Curves tab's curves: the ones the app's Presets › Curves keeps in `curves.db`, read without changing it (the
//! plugin never writes the app's files), searched and filtered as the app's list is (mo-app's
//! `presets::shown_curves`), and dragged out as a Bitwig curve file (Solomon, 13 Nov: "both presets and curves should
//! be draggable").
//!
//! A dragged curve is written to a drag folder of its own in Temp, as a dragged MIDI part is
//! ([`mo_app::drag_out::write_drag_files`]), and cleared a day later: the app's own Curves folder is the app's.

use mo_app::presets::{self as page, CurveFilters, CurveKey};
use mo_presets::curves::{self, Curve, CurvesDb};
use std::path::{Path, PathBuf};

/// The curves, read once each time the tab asks.
pub struct Curves {
    pub curves: Vec<Curve>,
    keys: Vec<CurveKey>,
}

/// How reading the curves went.
pub enum Read {
    Ready(Curves),
    /// Nothing to show, or it couldn't be read: why, in plain words.
    Not(String),
}

/// The curves in the app's `curves.db`. Asks the disk: call it off the panel's thread.
pub fn read() -> Read {
    let started = std::time::Instant::now();
    let read = read_from(&mo_app::paths::data_dir().join(curves::FILE_NAME));
    match &read {
        Read::Ready(c) => crate::log::took("Curves", &format!("Read {} curves", c.curves.len()), started),
        Read::Not(why) => crate::log::took("Curves", &format!("Not read: {why}"), started),
    }
    read
}

/// The curves in the `curves.db` at `path`, opened only to read.
pub fn read_from(path: &Path) -> Read {
    if !path.is_file() {
        return Read::Not(
            "No curves yet: open Presets › Curves in the app once, and it finds them in your songs".into(),
        );
    }
    match CurvesDb::open_read_only(path).and_then(|db| db.list()) {
        Ok(list) if list.is_empty() => Read::Not("The app has found no curves in your songs yet".into()),
        Ok(curves) => Read::Ready(Curves { keys: CurveKey::all(&curves), curves }),
        Err(e) => Read::Not(format!("The app's curves couldn't be read: {e}")),
    }
}

impl Curves {
    /// The curves `filters` let through, newest found first, as places in [`Curves::curves`], with the Found in
    /// filter's chips.
    pub fn view(&self, filters: &CurveFilters) -> (Vec<usize>, mo_app::filters::Filter) {
        page::shown_curves(&self.curves, &self.keys, filters)
    }

    /// What a drag of the curve at `at` carries: its Bitwig curve file, written to a new drag folder in `temp`.
    /// Writes to the disk: call it off the panel's thread.
    pub fn to_drag(&self, at: usize, temp: &Path) -> Result<PathBuf, String> {
        let c = self.curves.get(at).ok_or("That curve isn't in the list any more")?;
        let name = if c.name.trim().is_empty() { "Curve" } else { c.name.as_str() };
        let file = mo_app::drag_out::MadeFile { name: format!("{name}.bwcurve"), bytes: curves::curve_bytes(c)? };
        let mut written = mo_app::drag_out::write_drag_files(temp, &[file])
            .map_err(|e| format!("The curve couldn't be saved in Temp: {e}"))?;
        written.pop().ok_or_else(|| "The curve couldn't be saved in Temp".to_owned())
    }
}

/// The status line over the list: "120 curves", or "12 of 120 curves" when the search or a filter narrows it.
pub fn count_line(total: usize, shown: Option<usize>) -> String {
    let all = format!("{} curve{}", page::thousands(total), if total == 1 { "" } else { "s" });
    match shown {
        Some(n) => format!("{} of {all}", page::thousands(n)),
        None => all,
    }
}
