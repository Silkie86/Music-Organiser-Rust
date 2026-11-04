//! The half-done files a run leaves on the backup drive if it's cut off (a crash, a power cut, the drive pulled out),
//! cleared away by a later run (rule 28: temporary files are removed and leftovers swept).
//!
//! Before a run makes a temporary file, it writes its place in this run's list of parts
//! (`<backup>\.mo-backup\parts-<process>-<time>.txt`, one plain line each), and the list is removed when the run
//! ends, unless something it names couldn't be cleared away (a program holding it). A list left over is a run that
//! was cut off: a later run removes what it names that is still a leftover (a `.mo-part` copy or zip, a drive test's
//! file, or an empty placeholder in Older versions whose old copy never took its place), then the list. A list
//! written to in the last six hours is left alone: another copy of the app may be using it right now (a part's own
//! time can't tell: a copy takes its file's old time). Only what a list names is ever removed, so nothing else on the
//! drive is touched.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::copy::{OLDER_VERSIONS, PART};
use crate::record::{self, FOLDER};

/// What a list must be older than to be cleared: another run may be using it otherwise.
const QUIET_FOR: Duration = Duration::from_secs(6 * 60 * 60);

/// A name no other run uses: what it's for, this process and the time now.
pub(crate) fn run_name(what: &str) -> String {
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos();
    format!("{what}-{}-{stamp}", std::process::id())
}

/// This run's list of parts. Removed when dropped (the run ended, or stopped).
pub(crate) struct Parts {
    file: Option<Mutex<File>>,
    path: PathBuf,
    backup: PathBuf,
}

impl Parts {
    /// Clears what earlier runs left, then starts this run's list. A drive the list can't be written on (read-only)
    /// gives a list that notes nothing.
    pub(crate) fn start(backup: &Path) -> Parts {
        let folder = backup.join(FOLDER);
        sweep(backup, &folder);
        let path = folder.join(format!("{}.txt", run_name("parts")));
        let file = fs::create_dir_all(&folder).and_then(|()| File::create_new(&path)).ok().map(Mutex::new);
        Parts { file, path, backup: backup.to_path_buf() }
    }

    /// Notes a temporary copy, zip or drive test (`<name>.mo-part`) before it's made.
    pub(crate) fn part(&self, part: &Path) {
        self.note("part", part);
    }

    /// Notes an empty placeholder claimed in Older versions for an old copy of `old_len` bytes, before it's made.
    pub(crate) fn placeholder(&self, place: &Path, old_len: u64) {
        // An empty old copy leaves an empty file either way: nothing to tell apart, nothing to clear.
        if old_len > 0 {
            self.note("empty", place);
        }
    }

    fn note(&self, kind: &str, path: &Path) {
        let (Some(file), Some(rel)) = (&self.file, record::rel_path(&self.backup, path)) else { return };
        if rel.contains(['\n', '\r']) {
            return;
        }
        // One write a line, not buffered: what's written is there even if the app is cut off a moment later.
        let mut file = file.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = file.write_all(format!("{kind}\t{rel}\n").as_bytes());
    }
}

impl Drop for Parts {
    /// The run ended: its list goes, unless something it names couldn't be cleared (a later run tries again).
    fn drop(&mut self) {
        if self.file.take().is_some() {
            let none_left = fs::read_to_string(&self.path).is_ok_and(|text| clear(&self.backup, &text, false));
            if none_left {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

/// Whether a file was last written longer ago than [`QUIET_FOR`].
fn quiet(meta: &fs::Metadata) -> bool {
    meta.modified().ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age >= QUIET_FOR)
}

/// Clears what the lists earlier runs left name, then the lists.
fn sweep(backup: &Path, folder: &Path) {
    let Ok(dir) = fs::read_dir(folder) else { return };
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("parts-") && name.ends_with(".txt")) {
            continue;
        }
        let list = entry.path();
        if !fs::symlink_metadata(&list).is_ok_and(|m| m.is_file() && quiet(&m)) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&list) else { continue };
        if clear(backup, &text, true) {
            let _ = fs::remove_file(&list);
        }
    }
}

/// Removes what a list names that is still a leftover (only looks, unless `remove`). Whether none is left.
fn clear(backup: &Path, text: &str, remove: bool) -> bool {
    let mut none_left = true;
    for line in text.lines() {
        let Some((kind, rel)) = line.split_once('\t') else { continue };
        let Some(path) = record::place(backup, rel) else { continue };
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        let leftover = meta.is_file()
            && match kind {
                "part" => rel.ends_with(PART),
                "empty" => meta.len() == 0 && rel.starts_with(&format!("{OLDER_VERSIONS}/")),
                _ => false,
            };
        if leftover && remove {
            crate::copy::remove_part(&path);
        }
        if leftover && fs::symlink_metadata(&path).is_ok() {
            none_left = false;
        }
    }
    none_left
}
