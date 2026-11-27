//! The Songs board's stage guess: how far along a song looks from its newest save's shape
//! ([`mo_formats::shape`]), with the reasons in plain words for the card's tooltip. Your status always wins: the guess
//! only suggests a move ("Looks like Mixing now"), and only to a later stage than yours ([`suggest`]).
//!
//! Each save's shape is kept in `songs.db` (`shapes`, one row a save: [`crate::song_writes::keep_shapes`]), so it's
//! read from the file once. The row notes the file it's of (its time and size, as `projects` has them) and the shape
//! reader's version: once the save's file changes, or the reader does (rule 7), the row no longer counts and the shape
//! is read afresh. That keeps the time and size twice (rule 23), on purpose: a row that only went away when the save was
//! read again would be wrong after a read that didn't take it away (an older build of the app), and a save read again
//! with its file unchanged (one with no key found is, at every start) would be read for its shape again each time. A
//! row with no counts is a save that has no shape (a Studio One song, or one too damaged), so it isn't read again
//! either.
//!
//! The lines between stages are first guesses from the 1.1 plan (24 Nov 2026); they're to be tuned on counts from
//! Solomon's songs, then checked with him on 20 songs.

use std::collections::HashMap;

use mo_formats::shape::Shape;
use rusqlite::params_from_iter;

use crate::{Library, LibraryError, STATUSES};

/// The table of shapes. Made with a library's first write after it ([`crate::song_writes`]); tables are only ever
/// added (rule 6), so the layout number stays as it is. `mtime`, `size` and `version` say what it was read from (the
/// file, and [`mo_formats::shape::VERSION`]); `master` is the master's effects, one a line; the counts are all null
/// for a save that has no shape.
pub(crate) const SHAPES: &str = "
    CREATE TABLE IF NOT EXISTS shapes (
        project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
        mtime INTEGER NOT NULL, size INTEGER NOT NULL, version INTEGER NOT NULL,
        seconds REAL, bars REAL, markers INTEGER, automation INTEGER,
        tracks INTEGER, eq INTEGER, compressed INTEGER, sends INTEGER, master TEXT
    ) WITHOUT ROWID;
";

/// A song needs this many bars arranged to be more than an idea.
pub const IDEA_BARS: f64 = 16.0;
/// And this long (seconds) to be arranging: two minutes.
pub const ARRANGED_SECONDS: f64 = 120.0;
/// Mixing: at least this many mixer tracks, this share of them with an EQ, and this share with a compressor.
pub const MIXED_TRACKS: u32 = 3;
pub const MIXED_EQ: f64 = 0.5;
pub const MIXED_COMPRESSED: f64 = 0.3;
/// And at least this many moving settings, or this many send tracks (Bitwig starts every project with one).
pub const MIXED_AUTOMATION: u32 = 2;
pub const MIXED_SENDS: u32 = 2;

/// The stages a save's shape can show; Released and Archived are only ever yours.
const IDEA: usize = 0;
const IN_PROGRESS: usize = 1;
const ARRANGING: usize = 2;
const MIXING: usize = 3;
const FINISHED: usize = 4;

/// How far along a save looks, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Guess {
    /// One of [`STATUSES`], as spelt there: Idea to Finished.
    pub stage: &'static str,
    /// What it's guessed from, a short line each, for the tooltip.
    pub why: Vec<String>,
}

/// How far along a save with this shape looks. `bounced_since` is whether the song has a bounce newer than the save.
pub fn guess(shape: &Shape, bounced_since: bool) -> Guess {
    let mut why = Vec::new();
    let minutes = shape.seconds / 60.0;
    // Whole bars, as they're compared: 15.75 bars is under 16.
    let length = format!(
        "{}:{:02} long, {} bars",
        minutes.floor() as u64,
        (shape.seconds % 60.0).floor() as u64,
        shape.bars.floor()
    );
    let sections = match shape.markers {
        0 => String::new(),
        1 => ", 1 marker".to_owned(),
        n => format!(", {n} markers"),
    };
    let share = |n: u32| if shape.tracks == 0 { 0.0 } else { n as f64 / shape.tracks as f64 };
    let mixed = shape.tracks >= MIXED_TRACKS
        && share(shape.eq) >= MIXED_EQ
        && share(shape.compressed) >= MIXED_COMPRESSED
        && (shape.automation >= MIXED_AUTOMATION || shape.sends >= MIXED_SENDS);
    let arranged = shape.seconds >= ARRANGED_SECONDS && shape.markers != 1;
    let started = shape.bars.floor() >= IDEA_BARS;

    let stage = if started && arranged && shape.master_limited() && bounced_since {
        why.push(format!("{} on the master", shape.master_limiter().unwrap_or("A limiter")));
        why.push("a bounce newer than the save".to_owned());
        why.push(format!("{length}{sections}"));
        FINISHED
    } else if started && mixed {
        why.push(format!("EQ on {} of {} tracks, a compressor on {}", shape.eq, shape.tracks, shape.compressed));
        why.push(format!("{}, {}", count(shape.sends, "send track"), count(shape.automation, "automation lane")));
        why.push(format!("{length}{sections}"));
        MIXING
    } else if started && arranged {
        why.push(format!("{length}{sections}"));
        why.push(count(shape.tracks, "mixer track"));
        ARRANGING
    } else if started {
        why.push(format!("{length}{sections}"));
        IN_PROGRESS
    } else {
        why.push(if shape.bars <= 0.0 {
            "nothing arranged yet".to_owned()
        } else {
            format!("{length}: under {IDEA_BARS} bars")
        });
        IDEA
    };
    Guess { stage: STATUSES.get(stage).copied().unwrap_or(STATUSES[0]), why }
}

fn count(n: u32, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") }
}

/// The stage to suggest moving a song to: the guess, when it's later than `status` (yours). Never for a song you've
/// marked Released or Archived, or a status the app doesn't know.
pub fn suggest(status: &str, guess: &Guess) -> Option<&'static str> {
    let at = |s: &str| STATUSES.iter().position(|t| t.eq_ignore_ascii_case(s));
    let (mine, guessed) = (at(status)?, at(guess.stage)?);
    (mine <= FINISHED && guessed > mine).then_some(guess.stage)
}

impl Library {
    /// The shapes kept for these saves, by id: `Some(None)` for a save read with no shape, nothing for a save whose
    /// shape hasn't been read, or was read from another file (its time or size differ now) or by another version of
    /// the shape reader. A library without the table has none.
    pub fn shapes(&self, ids: &[i64]) -> Result<HashMap<i64, Option<Shape>>, LibraryError> {
        let mut found = HashMap::new();
        if !self.has("shapes") || ids.is_empty() {
            return Ok(found);
        }
        // In pieces, under SQLite's limit on how many values one query takes.
        for piece in ids.chunks(500) {
            let marks = vec!["?"; piece.len()].join(",");
            let mut st = self.conn.prepare(&format!(
                "SELECT s.project_id, s.seconds, s.bars, s.markers, s.automation, s.tracks, s.eq, s.compressed, \
                 s.sends, s.master FROM shapes s JOIN projects p ON p.id = s.project_id \
                 AND p.mtime = s.mtime AND p.size = s.size WHERE s.version = ? AND s.project_id IN ({marks})"
            ))?;
            let version = mo_formats::shape::VERSION as i64;
            let mut rows = st.query(params_from_iter(std::iter::once(&version).chain(piece)))?;
            while let Some(r) = rows.next()? {
                let id: i64 = r.get(0)?;
                let tracks: Option<i64> = r.get(5)?;
                let n = |i: usize| -> rusqlite::Result<u32> {
                    Ok(r.get::<_, Option<i64>>(i)?.unwrap_or(0).clamp(0, u32::MAX as i64) as u32)
                };
                let shape = match tracks {
                    None => None,
                    Some(_) => Some(Shape {
                        seconds: r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                        bars: r.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                        markers: n(3)?,
                        automation: n(4)?,
                        tracks: n(5)?,
                        eq: n(6)?,
                        compressed: n(7)?,
                        sends: n(8)?,
                        master: r
                            .get::<_, Option<String>>(9)?
                            .unwrap_or_default()
                            .lines()
                            .filter(|l| !l.is_empty())
                            .map(str::to_owned)
                            .collect(),
                    }),
                };
                found.insert(id, shape);
            }
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(bars: f64, seconds: f64) -> Shape {
        Shape { bars, seconds, ..Shape::default() }
    }

    #[test]
    fn length_alone_takes_a_song_from_idea_to_arranging() {
        assert_eq!(guess(&Shape::default(), false).stage, "Idea");
        assert_eq!(guess(&Shape::default(), false).why, ["nothing arranged yet"]);
        assert_eq!(guess(&shape(8.0, 15.0), false).stage, "Idea");
        assert_eq!(guess(&shape(15.75, 31.5), false).why, ["0:31 long, 15 bars: under 16 bars"]);
        assert_eq!(guess(&shape(32.0, 60.0), false).stage, "In progress");
        let g = guess(&shape(64.0, 128.0), false);
        assert_eq!(g.stage, "Arranging");
        assert_eq!(g.why[0], "2:08 long, 64 bars");
    }

    #[test]
    fn one_marker_is_one_section_so_not_arranged_yet() {
        let one = Shape { markers: 1, ..shape(64.0, 128.0) };
        assert_eq!(guess(&one, false).stage, "In progress");
        let five = Shape { markers: 5, ..shape(64.0, 128.0) };
        assert_eq!(guess(&five, false).stage, "Arranging");
        assert_eq!(guess(&five, false).why[0], "2:08 long, 64 bars, 5 markers");
    }

    #[test]
    fn eq_and_compression_on_most_tracks_with_automation_is_mixing() {
        let mix = Shape { tracks: 12, eq: 9, compressed: 7, sends: 2, automation: 14, ..shape(120.0, 232.0) };
        let g = guess(&mix, false);
        assert_eq!(g.stage, "Mixing");
        assert_eq!(g.why[..2], ["EQ on 9 of 12 tracks, a compressor on 7", "2 send tracks, 14 automation lanes"]);
        // EQ on too few tracks: arranging still.
        assert_eq!(guess(&Shape { eq: 4, ..mix.clone() }, false).stage, "Arranging");
        // Bitwig's one effect track and no automation: not yet.
        assert_eq!(guess(&Shape { sends: 1, automation: 0, ..mix.clone() }, false).stage, "Arranging");
        // A short loop being mixed is mixing all the same.
        assert_eq!(guess(&Shape { bars: 16.0, seconds: 30.0, ..mix.clone() }, false).stage, "Mixing");
    }

    #[test]
    fn finished_needs_a_master_limiter_full_length_and_a_newer_bounce() {
        let done = Shape { master: vec!["Pro-Q 3".into(), "Pro-L 2".into()], ..shape(120.0, 232.0) };
        let g = guess(&done, true);
        assert_eq!(g.stage, "Finished");
        assert_eq!(g.why[..2], ["Pro-L 2 on the master", "a bounce newer than the save"]);
        assert_eq!(guess(&done, false).stage, "Arranging");
        assert_eq!(guess(&Shape { master: vec!["Pro-Q 3".into()], ..done.clone() }, true).stage, "Arranging");
        assert_eq!(guess(&Shape { seconds: 60.0, ..done }, true).stage, "In progress");
    }

    #[test]
    fn only_a_later_stage_than_yours_is_suggested() {
        let mixing = Guess { stage: "Mixing", why: Vec::new() };
        assert_eq!(suggest("Arranging", &mixing), Some("Mixing"));
        assert_eq!(suggest("idea", &mixing), Some("Mixing"));
        assert_eq!(suggest("Mixing", &mixing), None);
        assert_eq!(suggest("Finished", &mixing), None);
        assert_eq!(suggest("Released", &Guess { stage: "Finished", why: Vec::new() }), None);
        assert_eq!(suggest("Archived", &mixing), None);
        assert_eq!(suggest("Something else", &mixing), None);
    }
}
