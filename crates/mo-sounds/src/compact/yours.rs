//! Your own say about sounds kept in the app's own sound library (stage 5 of "the sound library grows again", review
//! finding X1-own1): a favourite (the heart), stars, your own type and your tags, as the C# app's `SoundLibrary`
//! kept them (`SetFavourite`, `SetRating`, `SetUserType`, `AddTag` and `RemoveTag`). Since the C# app retired, the
//! card could only show them.
//!
//! Each call is one transaction on the app's own library, one at a time with every other change to it
//! ([`super::grow`]). Like plays, they aren't counted among the library's changes ([`super::grow::CHANGES`]): you may
//! change a few at nearly every start, and the backup copy (the library's size) follows them once a day.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{OptionalExtension, Transaction, params};

use crate::{SoundsError, learn};

/// The most stars a sound can have.
pub const MOST_STARS: i32 = 5;

/// A change you made to a sound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// A favourite, or not.
    Favourite(bool),
    /// Its stars, 0 (none) to [`MOST_STARS`].
    Stars(i32),
    /// Your own type for it ("Kick"); `None` takes yours away, so its name's type (or Unknown) shows again.
    Type(Option<String>),
    /// A tag added.
    Tag(String),
    /// A tag taken away.
    Untag(String),
}

/// A change to the sound numbered `id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Yours {
    pub id: i64,
    pub change: Change,
}

/// The C# app's table of tags, with its index, for a library made from a `sounds.db` without it: the same, so either
/// app reads it.
const TAGS: &str = "CREATE TABLE IF NOT EXISTS sound_tags (
         sound_id INTEGER REFERENCES sounds(id) ON DELETE CASCADE,
         tag TEXT NOT NULL, source TEXT NOT NULL DEFAULT 'you',
         UNIQUE(sound_id, tag));
     CREATE INDEX IF NOT EXISTS ix_sound_tags_tag ON sound_tags(tag);";

/// What one of a sound's things should read once every change is in: checked before they're kept.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Thing {
    Favourite,
    Stars,
    Type,
    /// A tag, as SQLite matches it (`COLLATE NOCASE`: capitals only in A to Z).
    Tag(String),
}

/// Keeps `yours` in the app's own sound library beside `sounds_db`, in order (a later change to the same thing wins).
/// A sound forgotten since is skipped, as in the C# app. Text is trimmed; a tag with no words is skipped. A tag the
/// sound has already in other capitals isn't added twice (the Tag filter matches either), and taking one away takes
/// it in any capitals. All of it or none; how many changes found their sound.
pub fn keep(sounds_db: &Path, yours: &[Yours]) -> Result<usize, SoundsError> {
    if yours.is_empty() {
        return Ok(0);
    }
    super::grow::write_small(sounds_db, |tx| {
        tx.execute_batch(TAGS)?;
        let mut kept = 0;
        let mut should: HashMap<(i64, Thing), Option<String>> = HashMap::new();
        for y in yours {
            if !there(tx, y.id)? {
                continue;
            }
            let id = y.id;
            match &y.change {
                Change::Favourite(on) => {
                    tx.prepare_cached("UPDATE sounds SET favourite = ?2 WHERE id = ?1")?.execute(params![id, on])?;
                    should.insert((id, Thing::Favourite), Some(i32::from(*on).to_string()));
                }
                Change::Stars(stars) => {
                    let stars = (*stars).clamp(0, MOST_STARS);
                    tx.prepare_cached("UPDATE sounds SET rating = ?2 WHERE id = ?1")?.execute(params![id, stars])?;
                    should.insert((id, Thing::Stars), Some(stars.to_string()));
                }
                Change::Type(own) => {
                    let own = own.as_deref().map(str::trim).filter(|t| !t.is_empty());
                    let named: Option<String> = tx
                        .prepare_cached("SELECT name_type FROM sounds WHERE id = ?1")?
                        .query_row([id], |r| r.get(0))?;
                    let (sound_type, source, sure) = match own {
                        Some(own) => (own, "you", 1.0),
                        None => learn::named_or_unknown(named.as_deref()),
                    };
                    tx.prepare_cached(
                        "UPDATE sounds SET user_type = ?2, type = ?3, type_source = ?4, type_conf = ?5 WHERE id = ?1",
                    )?
                    .execute(params![id, own, sound_type, source, sure])?;
                    should.insert((id, Thing::Type), own.map(str::to_owned));
                }
                Change::Tag(tag) => {
                    let tag = tag.trim();
                    if tag.is_empty() {
                        continue;
                    }
                    let yours_now = tx
                        .prepare_cached(
                            "UPDATE sound_tags SET source = 'you' WHERE sound_id = ?1 AND tag = ?2 COLLATE NOCASE",
                        )?
                        .execute(params![id, tag])?;
                    if yours_now == 0 {
                        tx.prepare_cached("INSERT INTO sound_tags (sound_id, tag, source) VALUES (?1, ?2, 'you')")?
                            .execute(params![id, tag])?;
                    }
                    should.insert((id, Thing::Tag(tag.to_ascii_lowercase())), Some(tag.to_owned()));
                }
                Change::Untag(tag) => {
                    let tag = tag.trim();
                    tx.prepare_cached("DELETE FROM sound_tags WHERE sound_id = ?1 AND tag = ?2 COLLATE NOCASE")?
                        .execute(params![id, tag])?;
                    should.insert((id, Thing::Tag(tag.to_ascii_lowercase())), None);
                }
            }
            kept += 1;
        }
        // Checked before it's kept: each thing changed reads back as its last change left it.
        for ((id, thing), should) in &should {
            let reads = read_back(tx, *id, thing)?;
            let same = match thing {
                // A tag added that was there in other capitals stays as it was.
                Thing::Tag(_) => reads.is_some() == should.is_some(),
                _ => reads == *should,
            };
            if !same {
                return Ok(Err(format!("sound {id}'s {thing:?} reads {reads:?}, not {should:?}")));
            }
        }
        Ok(Ok(kept))
    })
}

/// Whether the sound numbered `id` is in the library.
fn there(tx: &Transaction<'_>, id: i64) -> rusqlite::Result<bool> {
    tx.prepare_cached("SELECT 1 FROM sounds WHERE id = ?1")?.query_row([id], |_| Ok(())).optional().map(|r| r.is_some())
}

/// What `thing` of the sound numbered `id` reads now: a favourite and stars as numbers, your own type (`None` when
/// there's none), a tag (`None` when the sound hasn't it).
fn read_back(tx: &Transaction<'_>, id: i64, thing: &Thing) -> rusqlite::Result<Option<String>> {
    let number = |sql: &str| -> rusqlite::Result<Option<String>> {
        let n: Option<i64> = tx.prepare_cached(sql)?.query_row([id], |r| r.get(0))?;
        Ok(Some(n.unwrap_or(0).to_string()))
    };
    match thing {
        Thing::Favourite => number("SELECT favourite FROM sounds WHERE id = ?1"),
        Thing::Stars => number("SELECT rating FROM sounds WHERE id = ?1"),
        Thing::Type => tx.prepare_cached("SELECT user_type FROM sounds WHERE id = ?1")?.query_row([id], |r| r.get(0)),
        Thing::Tag(tag) => tx
            .prepare_cached("SELECT tag FROM sound_tags WHERE sound_id = ?1 AND tag = ?2 COLLATE NOCASE LIMIT 1")?
            .query_row(params![id, tag], |r| r.get(0))
            .optional(),
    }
}
