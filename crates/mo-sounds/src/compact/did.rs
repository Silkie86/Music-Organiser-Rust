//! What you do with sounds that says you like them is kept in the app's own sound library (stage 3 of "the sound
//! library grows again", review finding X1-own1): deliberate plays, drags out of the app and sends into a DAW, as the
//! C# app's `SoundLibrary.MarkPlayed` and `Record` kept them. History lists the plays, and Most liked counts all
//! three ([`crate::SoundsDb::activity`]). Since the C# app retired, none were kept.
//!
//! Each call is one transaction on the app's own library, one at a time with every other change to it, as the scan's
//! writes are ([`super::grow`]). They aren't counted among its changes ([`super::grow::CHANGES`]): the backup copy
//! follows them once a day.

use std::path::Path;

use rusqlite::params;

use crate::{SoundsError, event};

/// Something done with a sound: which one (its number), what ([`event::PLAY`], [`event::DRAG`] or [`event::SEND`]),
/// and when (Unix seconds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Did {
    pub id: i64,
    pub kind: &'static str,
    pub at: i64,
}

/// The C# app's table of what was done with sounds, with its indexes, for a library made from a `sounds.db` without
/// it: the same, so either app reads it.
const EVENTS: &str = "CREATE TABLE IF NOT EXISTS sound_events (
         sound_id INTEGER REFERENCES sounds(id) ON DELETE CASCADE,
         kind TEXT NOT NULL, at INTEGER NOT NULL);
     CREATE INDEX IF NOT EXISTS ix_sound_events ON sound_events(kind, at);
     CREATE INDEX IF NOT EXISTS ix_sound_events_sound ON sound_events(sound_id);";

/// Keeps `did` in the app's own sound library beside `sounds_db`. A play also counts on its sound: one more play, and
/// when it was last played. A sound forgotten since (its file gone at a scan) is skipped, as in the C# app. All of it
/// or none; how many were kept.
pub fn keep(sounds_db: &Path, did: &[Did]) -> Result<usize, SoundsError> {
    if did.is_empty() {
        return Ok(0);
    }
    super::grow::write_small(sounds_db, |tx| {
        tx.execute_batch(EVENTS)?;
        let mut kept = 0;
        for d in did {
            let added = tx
                .prepare_cached(
                    "INSERT INTO sound_events (sound_id, kind, at) SELECT ?1, ?2, ?3
                     WHERE EXISTS (SELECT 1 FROM sounds WHERE id = ?1)",
                )?
                .execute(params![d.id, d.kind, d.at])?;
            if added == 1 && d.kind == event::PLAY {
                let counted = tx
                    .prepare_cached("UPDATE sounds SET plays = COALESCE(plays, 0) + 1, last_played = ?2 WHERE id = ?1")?
                    .execute(params![d.id, d.at])?;
                // Checked before it's kept: the play that was noted is counted on its sound.
                if counted != 1 {
                    return Ok(Err(format!("the play of sound {} wasn't counted", d.id)));
                }
            }
            kept += added;
        }
        Ok(Ok(kept))
    })
}
