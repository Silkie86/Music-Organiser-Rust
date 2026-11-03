//! The Sounds library as the sample locator sees it (rule 13's last place to look): read from `sounds.db` the first
//! time a sample isn't in its project's or its DAW's folders, then kept while `sounds.db` stays as it is, so clicking
//! through songs or saving every minute doesn't read the database each time.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use mo_library::samples::{SampleLocator, SoundLibrary};

/// How long a read is kept at least, even while the C# app keeps saving `sounds.db` (a scan for new sounds saves every
/// few seconds): sounds added on the Sounds page are seen after this.
const FRESH: Duration = Duration::from_secs(60);

/// The last read: of which database, what it looked like just before, and when the read was done.
struct Kept {
    db: PathBuf,
    stamp: Stamp,
    at: Instant,
    library: Arc<SoundLibrary>,
}

impl Kept {
    /// Whether this read still stands for `db`, which looks like `stamp` now: it hasn't changed since, or this read is
    /// `age` old, under a minute.
    fn still_good(&self, db: &Path, stamp: &Stamp, age: Duration) -> bool {
        self.db == db && (self.stamp == *stamp || age < FRESH)
    }
}

static SHARED: Mutex<Option<Kept>> = Mutex::new(None);

/// The sounds in `sounds_db`: the last read while `sounds_db` hasn't changed since or that read is under a minute old,
/// otherwise read again. Reads the disk, so never on the window's thread.
pub fn shared(sounds_db: &Path) -> Arc<SoundLibrary> {
    // Looked at before reading, so a save made while it reads counts as a change next time.
    let stamp = stamp(sounds_db);
    if let Ok(slot) = SHARED.lock()
        && let Some(kept) = slot.as_ref()
        && kept.still_good(sounds_db, &stamp, kept.at.elapsed())
    {
        return kept.library.clone();
    }
    // Read outside the lock: two threads may both read once, which is cheaper than one waiting on the other.
    let library = Arc::new(SoundLibrary::read(sounds_db));
    if let Ok(mut slot) = SHARED.lock() {
        *slot = Some(Kept { db: sounds_db.to_path_buf(), stamp, at: Instant::now(), library: library.clone() });
    }
    library
}

/// A sample locator that also looks in the Sounds library at `sounds_db`, when one is given. The library is read only
/// when a sample isn't in its project's or its DAW's folders.
pub fn locator(sounds_db: Option<&Path>) -> SampleLocator {
    let locator = SampleLocator::default();
    match sounds_db {
        Some(db) => {
            let db = db.to_path_buf();
            locator.with_library_from(move || shared(&db))
        }
        None => locator,
    }
}

/// What a database and the log SQLite writes its saves into first (`sounds.db-wal`) look like: their sizes and when
/// each last changed. Every save the C# app makes changes one of them.
type Stamp = [Option<(u64, Option<SystemTime>)>; 2];

fn stamp(db: &Path) -> Stamp {
    let look = |p: &Path| fs::metadata(p).ok().map(|m| (m.len(), m.modified().ok()));
    let mut wal = db.as_os_str().to_owned();
    wal.push("-wal");
    [look(db), look(Path::new(&wal))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_is_kept_until_the_database_changes() {
        let dir = std::env::temp_dir().join(format!("mo-app-sound-library-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let db = dir.join("sounds.db");
        fs::write(&db, b"one").unwrap();
        let before = stamp(&db);
        assert_eq!(stamp(&db), before);
        let kept = Kept { db: db.clone(), stamp: before, at: Instant::now(), library: Arc::default() };
        let old = FRESH * 10;
        // Kept for as long as the database stays as it is, however long that is.
        assert!(kept.still_good(&db, &stamp(&db), old));
        // A save written into the log beside it, or into the database itself: read again, once a minute at most.
        let mut wal = db.as_os_str().to_owned();
        wal.push("-wal");
        fs::write(&wal, b"a save").unwrap();
        let saved = stamp(&db);
        assert_ne!(saved, before);
        assert!(!kept.still_good(&db, &saved, old));
        assert!(kept.still_good(&db, &saved, Duration::from_secs(5)));
        fs::remove_file(&wal).unwrap();
        fs::write(&db, b"one more").unwrap();
        assert!(!kept.still_good(&db, &stamp(&db), old));
        // Another database is another library.
        assert!(!kept.still_good(&dir.join("other.db"), &before, Duration::ZERO));
        let _ = fs::remove_dir_all(&dir);
    }
}
