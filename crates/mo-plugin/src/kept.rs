//! What the plugin read from the app's files, kept while the plugin is loaded in the DAW, so its window shows it at
//! once when it's opened again (Solomon, 13 Nov: "when you close the gui and reopen it it has to load everything
//! again it should persist").
//!
//! Each list read (the sound library, the MIDI parts, the presets) is kept with a stamp of the app's files it came
//! from: each file's size and when it was last changed. A window opened later takes the kept list when the stamp is
//! the same, and reads again when the app changed one of those files since. Every copy of the plugin shares them; the
//! last copy taken out of the DAW lets them go ([`Alive`]).

use crate::threads::lock;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// The size and last change of each file a list was read from (none for one that isn't there).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp(Vec<Option<(u64, SystemTime)>>);

impl Stamp {
    /// The stamp of `files` now; a database's `-wal` file beside it counts too, as what's written to it is part of
    /// the database. Only asks for each file's details: quick.
    pub fn of(files: &[PathBuf]) -> Stamp {
        let look = |p: &Path| std::fs::metadata(p).ok().and_then(|m| Some((m.len(), m.modified().ok()?)));
        let mut out = Vec::with_capacity(files.len() * 2);
        for file in files {
            out.push(look(file));
            let mut wal = file.clone().into_os_string();
            wal.push("-wal");
            out.push(look(Path::new(&wal)));
        }
        Stamp(out)
    }
}

/// One list kept, with the stamp of the files it was read from.
pub struct Kept<T> {
    slot: Mutex<Option<(Stamp, Arc<T>)>>,
}

impl<T> Kept<T> {
    pub const fn new() -> Kept<T> {
        Kept { slot: Mutex::new(None) }
    }

    /// The list kept, when `files` are as they were when it was read.
    pub fn get(&self, files: &[PathBuf]) -> Option<Arc<T>> {
        let slot = lock(&self.slot);
        let (stamp, kept) = slot.as_ref()?;
        (*stamp == Stamp::of(files)).then(|| Arc::clone(kept))
    }

    /// Keeps `list`, read from `files` as `stamp` found them (taken before the read, so a change made while it was
    /// read makes the next window read again). Only while a copy of the plugin is loaded.
    pub fn keep(&self, stamp: Stamp, list: &Arc<T>) {
        if ALIVE.load(Ordering::Acquire) > 0 {
            *lock(&self.slot) = Some((stamp, Arc::clone(list)));
        }
    }

    fn clear(&self) {
        *lock(&self.slot) = None;
    }
}

/// How many copies of the plugin are loaded.
static ALIVE: AtomicUsize = AtomicUsize::new(0);

/// Held by each copy of the plugin while the DAW has it loaded: when the last one goes, the kept lists go too.
pub struct Alive(());

impl Alive {
    pub fn new() -> Alive {
        ALIVE.fetch_add(1, Ordering::AcqRel);
        Alive(())
    }
}

impl Drop for Alive {
    fn drop(&mut self) {
        if ALIVE.fetch_sub(1, Ordering::AcqRel) == 1 {
            crate::library::KEPT.clear();
            crate::midi::KEPT.clear();
            crate::presets::KEPT.clear();
            crate::galaxy::KEPT.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_is_kept_until_its_files_change_or_the_last_plugin_goes() {
        let dir = std::env::temp_dir().join(format!("mo-plugin-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("list.db");
        std::fs::write(&file, b"one").unwrap();
        let files = vec![file.clone()];
        let kept: Kept<u32> = Kept::new();
        let alive = Alive::new();
        kept.keep(Stamp::of(&files), &Arc::new(2));
        assert_eq!(kept.get(&files).as_deref(), Some(&2));
        // The app wrote to its database: read again.
        std::fs::write(dir.join("list.db-wal"), b"changed").unwrap();
        assert!(kept.get(&files).is_none());
        kept.keep(Stamp::of(&files), &Arc::new(3));
        assert_eq!(kept.get(&files).as_deref(), Some(&3));
        drop(alive);
        kept.clear();
        assert!(kept.get(&files).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
