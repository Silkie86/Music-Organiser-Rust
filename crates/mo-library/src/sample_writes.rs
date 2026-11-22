//! Looking again at the samples the app's own song library, `songs.db`, has marked missing, once the Sounds library
//! has new sounds in it (a sample folder added in Settings and scanned, say). A save is only read when its file
//! changes, so without this an old project whose samples are in a folder added since stayed "missing samples" for good,
//! though it opened fine in its DAW.
//!
//! Only a `songs.db` that's the app's own is written ([`crate::songs_db::own_since`]), in one short IMMEDIATE
//! transaction, as [`crate::plugin_writes`]: the looking is done first, away from it. Only ever from missing to found:
//! a sample found once and gone since is the save's own read's to judge, the next time its file changes.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use rayon::prelude::*;
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::LibraryError;
use crate::plugin_writes::{open_own, own_under_lock};

/// The note in `songs_info` of what the missing samples were last looked at against: the Sounds library's stamp, as
/// the caller gives it ([`recheck_found`]).
const RECHECKED_KEY: &str = "samples_rechecked";

/// What [`recheck_found`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SamplesRechecked {
    /// Whether the missing samples were looked at (not when nothing changed since the last look).
    pub looked: bool,
    /// How many samples marked missing were looked for.
    pub looked_for: usize,
    /// How many of them are found now, and marked so.
    pub found: usize,
    /// The saves that miss fewer samples now, with how many they still miss, by id.
    pub missing: Vec<(i64, u32)>,
    /// It stopped part way (the app closing): what was found is marked, and the rest is looked for next time.
    pub stopped: bool,
}

/// Looks again for every sample the app's own `songs_db` has marked missing, by `found(sample, project)` (the sample
/// locator's rule: the project's and its DAW's folders, then the Sounds library), and marks the ones found now. Only
/// when `stamp` (what the Sounds library holds, in short) differs from the last look's, noted in the library, so a
/// start with nothing new looks at nothing. `found` is asked once for each sample and project, on the rayon pool the
/// caller runs this on, so it may read the disk: never call this on the window's thread. Once `stop` says so (the app
/// closing), nothing more is looked for, and the look isn't noted, so the next one looks again.
pub fn recheck_found(
    songs_db: &Path,
    stamp: &str,
    found: impl Fn(&str, &str) -> bool + Sync,
    stop: impl Fn() -> bool + Sync,
) -> Result<SamplesRechecked, LibraryError> {
    let (mut conn, sql) = open_own(songs_db)?;
    let noted: Option<String> = conn
        .query_row("SELECT value FROM songs_info WHERE key = ?1", [RECHECKED_KEY], |r| r.get(0))
        .optional()
        .map_err(&sql)?;
    if noted.as_deref() == Some(stamp) {
        return Ok(SamplesRechecked::default());
    }
    // Every sample marked missing, with its save's path: only what's missing is read (the list's index has `found`).
    let rows: Vec<(i64, i64, String, String)> = {
        let mut st = conn
            .prepare(
                "SELECT s.rowid, s.project_id, s.path, p.path FROM samples s JOIN projects p ON p.id = s.project_id \
                 WHERE s.found = 0 AND s.path IS NOT NULL AND p.path IS NOT NULL",
            )
            .map_err(&sql)?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(&sql)?;
        rows.collect::<rusqlite::Result<_>>().map_err(&sql)?
    };
    // Each sample of each save judged once (a save may name one twice), spread over the pool.
    let mut asks: Vec<(&str, &str)> = rows.iter().map(|(_, _, s, p)| (s.as_str(), p.as_str())).collect();
    asks.sort_unstable();
    asks.dedup();
    let judged: HashMap<(&str, &str), bool> =
        asks.into_par_iter().map(|(s, p)| ((s, p), !stop() && found(s, p))).collect();
    let stopped = stop();
    let now_found: Vec<&(i64, i64, String, String)> =
        rows.iter().filter(|(_, _, s, p)| judged.get(&(s.as_str(), p.as_str())).copied().unwrap_or(false)).collect();

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(&sql)?;
    own_under_lock(songs_db, &tx)?;
    let mut changed: BTreeMap<i64, ()> = BTreeMap::new();
    let mut marked = 0;
    {
        // By its row, but only while it's still the same sample of the same save, still missing: the Songs watch may
        // have kept that save again meanwhile, and its new rows can take the old row numbers.
        let mut set = tx
            .prepare("UPDATE samples SET found = 1 WHERE rowid = ?1 AND project_id = ?2 AND path = ?3 AND found = 0")
            .map_err(&sql)?;
        for (row, project, sample, _) in now_found {
            if set.execute(params![row, project, sample]).map_err(&sql)? > 0 {
                marked += 1;
                changed.insert(*project, ());
            }
        }
    }
    let mut missing = Vec::with_capacity(changed.len());
    {
        let mut count = tx.prepare("SELECT COUNT(*) FROM samples WHERE project_id = ?1 AND found = 0").map_err(&sql)?;
        for id in changed.into_keys() {
            let n: i64 = count.query_row([id], |r| r.get(0)).map_err(&sql)?;
            missing.push((id, u32::try_from(n).unwrap_or(u32::MAX)));
        }
    }
    if !stopped {
        tx.execute("INSERT OR REPLACE INTO songs_info (key, value) VALUES (?1, ?2)", params![RECHECKED_KEY, stamp])
            .map_err(&sql)?;
    }
    tx.commit().map_err(&sql)?;
    Ok(SamplesRechecked { looked: true, looked_for: rows.len(), found: marked, missing, stopped })
}
