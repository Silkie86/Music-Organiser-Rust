//! The galaxy's maps kept in the app's own sound library (stage 4 of "the sound library grows again", review finding
//! X2-2), as the C# app's `SoundLibrary.SaveMap` kept them: each way of placing the stars ("features", "clap",
//! "clap+features") has its own map in `sound_maps`, so the next start brings it up to date rather than laying it out
//! again. Since the C# app retired, none were kept.
//!
//! Each call is one transaction on the app's own library, one at a time with every other change to it
//! ([`super::grow`]). Like plays, a map isn't counted among the library's changes ([`super::grow::CHANGES`]): the
//! backup copy follows it once a day, and a map lost with it is brought up to date again from the one before.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::params;

use crate::{SoundsError, StarPoint};

/// The C# app's table of the galaxy's maps, with its index, for a library made from a `sounds.db` without it: the
/// same, so either app reads it.
const MAPS: &str = "CREATE TABLE IF NOT EXISTS sound_maps (
         space TEXT NOT NULL,
         sound_id INTEGER NOT NULL REFERENCES sounds(id) ON DELETE CASCADE,
         x REAL NOT NULL, y REAL NOT NULL,
         PRIMARY KEY (space, sound_id));
     CREATE INDEX IF NOT EXISTS ix_sound_maps_sound ON sound_maps(sound_id);";

/// Keeps `points` (where each sound sits, by its id) on the map placed by `space` in the app's own sound library beside
/// `sounds_db`: over the places they had; with `replace` (a map laid out afresh), in place of the whole map. A sound
/// forgotten since is skipped, as in the C# app. All of it or none; how many places were kept.
pub fn keep(
    sounds_db: &Path,
    space: &str,
    points: &HashMap<i64, StarPoint>,
    replace: bool,
) -> Result<usize, SoundsError> {
    if points.is_empty() && !replace {
        return Ok(0);
    }
    super::grow::write_small(sounds_db, |tx| {
        tx.execute_batch(MAPS)?;
        if replace {
            tx.execute("DELETE FROM sound_maps WHERE space = ?1", [space])?;
        }
        let mut kept = 0;
        let mut put = tx.prepare_cached(
            "INSERT OR REPLACE INTO sound_maps (space, sound_id, x, y) SELECT ?1, ?2, ?3, ?4
             WHERE EXISTS (SELECT 1 FROM sounds WHERE id = ?2)",
        )?;
        for (id, p) in points {
            kept += put.execute(params![space, id, f64::from(p.x), f64::from(p.y)])?;
        }
        // Checked before it's kept: a map laid out afresh is all there is of it.
        if replace {
            let there: i64 = tx.query_row("SELECT COUNT(*) FROM sound_maps WHERE space = ?1", [space], |r| r.get(0))?;
            if usize::try_from(there).ok() != Some(kept) {
                return Ok(Err(format!("{there} places read back for the {kept} kept")));
            }
        }
        Ok(Ok(kept))
    })
}
