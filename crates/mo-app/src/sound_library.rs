//! The Sounds library as the sample locator sees it (rule 13's last place to look): read from `sounds.db` once and
//! kept for a minute, so clicking through songs or saving every minute doesn't read the database each time.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mo_library::samples::{SampleLocator, SoundLibrary};

/// How long a library read stays fresh: sounds added on the Sounds page are seen after this.
const FRESH: Duration = Duration::from_secs(60);

static SHARED: Mutex<Option<(PathBuf, Instant, Arc<SoundLibrary>)>> = Mutex::new(None);

/// The sounds in `sounds_db`, read again when the last read is over a minute old. Reads the disk, so never on the
/// window's thread.
pub fn shared(sounds_db: &Path) -> Arc<SoundLibrary> {
    if let Ok(slot) = SHARED.lock()
        && let Some((path, at, library)) = slot.as_ref()
        && path == sounds_db
        && at.elapsed() < FRESH
    {
        return library.clone();
    }
    // Read outside the lock: two threads may both read once, which is cheaper than one waiting on the other.
    let library = Arc::new(SoundLibrary::read(sounds_db));
    if let Ok(mut slot) = SHARED.lock() {
        *slot = Some((sounds_db.to_path_buf(), Instant::now(), library.clone()));
    }
    library
}

/// A sample locator that also looks in the Sounds library at `sounds_db`, when one is given.
pub fn locator(sounds_db: Option<&Path>) -> SampleLocator {
    let locator = SampleLocator::default();
    match sounds_db {
        Some(db) => locator.with_library(shared(db)),
        None => locator,
    }
}
