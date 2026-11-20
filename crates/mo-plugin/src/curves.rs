//! The Curves tab's curves: the ones the app's Presets › Curves keeps in `curves.db`, read without changing it, searched and filtered as the app's list is (mo-app's
//! `presets::shown_curves`), and dragged out as a Bitwig curve file (Solomon, 13 Nov: "both presets and curves should
//! be draggable").
//!
//! A dragged curve is written to a drag folder of its own in Temp, as a dragged MIDI part is
//! ([`mo_app::drag_out::write_drag_files`]), and cleared a day later: the app's own Curves folder is the app's.
//!
//! The one thing written in the app's files: a curve edited here, kept as a new curve of its own ([`save_as_new`];
//! Solomon, 20 Nov 2026: "Yes plugin too"). It's added to `curves.db` as the app's own Save as new curve adds it, and
//! the app lists it when its window next comes to the front.

use mo_app::curve_keys::{self, KeyMap};
use mo_app::presets::{self as page, CurveFilters, CurveKey};
use mo_presets::curves::{self, Curve, CurvesDb};
use std::path::{Path, PathBuf};

/// The curves, read once each time the tab asks.
pub struct Curves {
    pub curves: Vec<Curve>,
    keys: Vec<CurveKey>,
    /// Which key does what in the curve editor, as the app's Settings have it.
    pub editor_keys: KeyMap,
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
    let data = mo_app::paths::data_dir();
    let mut read = read_from(&data.join(curves::FILE_NAME));
    if let Read::Ready(c) = &mut read {
        c.editor_keys = KeyMap::from_setting(crate::midi::setting(&data, curve_keys::SETTING).as_deref());
    }
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
        // Sorting them into groups draws every curve: on the app's low-priority threads, not the host's.
        Ok(curves) => Read::Ready(Curves {
            keys: mo_app::background::on_pool(|| CurveKey::all(&curves)),
            curves,
            editor_keys: KeyMap::default(),
        }),
        Err(e) => Read::Not(format!("The app's curves couldn't be read: {e}")),
    }
}

/// A curve kept by [`save_as_new`], as the list takes it in.
pub struct Saved {
    pub curve: Curve,
    key: CurveKey,
    /// The id of the curve the edits were made on, and the points and length that were saved.
    pub from: String,
    pub shape: (Vec<mo_formats::bitwig::curve::CurvePoint>, f64),
}

/// Keeps the editor's curve (`shape`: its points and length) as a new curve of its own in the `curves.db` at `path`,
/// named as an edit of `from_name`. The same shape saved before is that curve, under the name it has. Writes to the
/// disk: call it off the panel's thread.
pub fn save_as_new(
    path: &Path,
    from: String,
    from_name: &str,
    shape: (Vec<mo_formats::bitwig::curve::CurvePoint>, f64),
) -> Result<Saved, String> {
    if !path.is_file() {
        return Err("Open Presets › Curves in the app once first: it makes the list this curve is kept in".into());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let saved = CurvesDb::open_to_save_made(path)
        .and_then(|db| db.save_made(&page::edited_name(from_name), &shape.0, shape.1, now));
    match saved {
        Ok(curve) => {
            crate::log::line("Curves", &format!("Saved \"{}\" as a new curve in the app's list", curve.name));
            Ok(Saved { key: CurveKey::of(&curve), curve, from, shape })
        }
        Err(e) => {
            crate::log::line("Curves", &format!("Couldn't save a new curve: {e}"));
            Err(format!("Couldn't save the curve: {e}"))
        }
    }
}

impl Curves {
    /// The list with `saved` in it (in the place of the curve with its id, when that shape was saved before), and
    /// its place.
    pub fn with(&self, saved: Saved) -> (Curves, usize) {
        let (mut curves, mut keys) = (self.curves.clone(), self.keys.clone());
        let at = match curves.iter().position(|c| c.id == saved.curve.id) {
            Some(at) => {
                curves[at] = saved.curve;
                keys[at] = saved.key;
                at
            }
            None => {
                curves.push(saved.curve);
                keys.push(saved.key);
                curves.len() - 1
            }
        };
        (Curves { curves, keys, editor_keys: self.editor_keys }, at)
    }

    /// The curves `filters` let through, newest found first, as places in [`Curves::curves`], with the Found in
    /// filter's chips.
    pub fn view(&self, filters: &CurveFilters) -> (Vec<usize>, mo_app::filters::Filter) {
        page::shown_curves(&self.curves, &self.keys, filters)
    }

    /// What a drag of the curve at `at` carries: its Bitwig curve file, written to a new drag folder in `temp`.
    /// Writes to the disk: call it off the panel's thread.
    pub fn to_drag(&self, at: usize, temp: &Path) -> Result<PathBuf, String> {
        drag_file(self.curves.get(at).ok_or("That curve isn't in the list any more")?, temp)
    }
}

/// What a drag of `c` carries: its Bitwig curve file, written to a new drag folder in `temp`. Writes to the disk: call
/// it off the panel's thread.
pub fn drag_file(c: &Curve, temp: &Path) -> Result<PathBuf, String> {
    let name = if c.name.trim().is_empty() { "Curve" } else { c.name.as_str() };
    let file = mo_app::drag_out::MadeFile { name: format!("{name}.bwcurve"), bytes: curves::curve_bytes(c)? };
    let mut written = mo_app::drag_out::write_drag_files(temp, &[file])
        .map_err(|e| format!("The curve couldn't be saved in Temp: {e}"))?;
    written.pop().ok_or_else(|| "The curve couldn't be saved in Temp".to_owned())
}

/// `c` as the editor has left it: these points, this long, named as an edit of it (so its file doesn't pass for the
/// curve as found).
pub fn edited(c: &Curve, points: Vec<mo_formats::bitwig::curve::CurvePoint>, length_beats: f64) -> Curve {
    let name = if c.name.trim().is_empty() { "Curve (edited)".to_owned() } else { format!("{} (edited)", c.name) };
    Curve { name, points, length_beats, ..c.clone() }
}

/// The status line over the list: "120 curves", or "12 of 120 curves" when the search or a filter narrows it.
pub fn count_line(total: usize, shown: Option<usize>) -> String {
    let all = format!("{} curve{}", page::thousands(total), if total == 1 { "" } else { "s" });
    match shown {
        Some(n) => format!("{} of {all}", page::thousands(n)),
        None => all,
    }
}
