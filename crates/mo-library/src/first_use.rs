//! When you first used a sample in a song, for the Sounds page's year strip ("First used"). A newer save of an old
//! song mustn't date a sound by that save (Solomon, 13 Nov 2026), so the saves of one song are compared:
//!
//! - In date order, oldest first, the first save of the song that has the sound is found.
//! - If an older save of that song doesn't have it, you put it in later: it's dated by that first save that has it.
//! - If it's in the song's oldest save, it was there from the start: it's dated by when the song was first made
//!   ([`crate::songs::first_made`]: the oldest save, or the date FL Studio keeps in the file through re-saves, whichever
//!   is earlier).
//!
//! A sound's first use is the earliest of those over every song that uses it. Only saves that can say a sound isn't
//! in them count as not having it: one whose file has gone, or that couldn't be read, says nothing.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDateTime;
use mo_formats::dates::RealDates;

use crate::songs::{Song, first_made};

/// Every song's saves in date order, to date a sample's first use by ([`FirstUses::of`]).
#[derive(Clone, Debug, Default)]
pub struct FirstUses {
    /// Each save's song, as a place in `songs`.
    song_of: HashMap<i64, usize>,
    songs: Vec<Timeline>,
}

/// One song's saves with a real date, oldest first, and when the song was first made.
#[derive(Clone, Debug)]
struct Timeline {
    saves: Vec<Dated>,
    start: Option<NaiveDateTime>,
}

#[derive(Clone, Copy, Debug)]
struct Dated {
    id: i64,
    when: NaiveDateTime,
    /// The save can say a sound isn't in it: its file is there and it was read.
    tells: bool,
}

impl FirstUses {
    /// The saves of `songs` (copies found elsewhere too), with only real dates counting (`real`): a save dated still to
    /// come, or not at all, says nothing of when a sound was put in.
    pub fn new(songs: &[Song], real: &RealDates) -> FirstUses {
        let mut song_of = HashMap::new();
        let mut timelines = Vec::with_capacity(songs.len());
        for song in songs {
            let all = || song.saves().chain(&song.copies);
            let mut saves: Vec<Dated> = all()
                .filter(|s| real.local(s.modified))
                .map(|s| Dated { id: s.id, when: s.modified, tells: !s.file_missing && s.error.is_none() })
                .collect();
            saves.sort_by_key(|d| (d.when, d.id));
            for d in &saves {
                song_of.insert(d.id, timelines.len());
            }
            timelines.push(Timeline { saves, start: first_made(all(), real) });
        }
        FirstUses { song_of, songs: timelines }
    }

    /// When the sample the saves `using` (by id) have in them was first used, by the rules at the top of this file;
    /// None when none of them has a real date.
    pub fn of(&self, using: &HashSet<i64>) -> Option<NaiveDateTime> {
        let mut songs: Vec<usize> = using.iter().filter_map(|id| self.song_of.get(id).copied()).collect();
        songs.sort_unstable();
        songs.dedup();
        songs.into_iter().filter_map(|i| self.songs[i].first_use(using)).min()
    }
}

impl Timeline {
    fn first_use(&self, using: &HashSet<i64>) -> Option<NaiveDateTime> {
        let mut lacked = false;
        for d in &self.saves {
            if using.contains(&d.id) {
                return Some(if lacked { d.when } else { self.start.map_or(d.when, |s| s.min(d.when)) });
            }
            lacked |= d.tells;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Save;
    use chrono::NaiveDate;

    fn day(y: i32, m: u32, d: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d).and_then(|d| d.and_hms_opt(12, 0, 0)).unwrap()
    }

    fn save(id: i64, when: NaiveDateTime) -> Save {
        Save { id, modified: when, ..Save::default() }
    }

    fn song(mut saves: Vec<Save>) -> Song {
        saves.sort_by_key(|s| std::cmp::Reverse(s.modified));
        let latest = saves.remove(0);
        Song { name: "Song".into(), latest, older: saves, key: "song".into(), copies: Vec::new() }
    }

    fn ids(ids: &[i64]) -> HashSet<i64> {
        ids.iter().copied().collect()
    }

    #[test]
    fn a_sound_in_the_oldest_save_dates_from_the_start_even_when_only_newer_saves_are_left_to_say() {
        let real = RealDates::now();
        let songs = [song(vec![save(1, day(2019, 3, 1)), save(2, day(2024, 6, 1))])];
        let uses = FirstUses::new(&songs, &real);
        // In both saves: 2019, not 2024.
        assert_eq!(uses.of(&ids(&[1, 2])), Some(day(2019, 3, 1)));
    }

    #[test]
    fn a_sound_put_in_later_dates_from_the_first_save_that_has_it() {
        let real = RealDates::now();
        let songs = [song(vec![save(1, day(2019, 3, 1)), save(2, day(2021, 1, 5)), save(3, day(2024, 6, 1))])];
        let uses = FirstUses::new(&songs, &real);
        assert_eq!(uses.of(&ids(&[2, 3])), Some(day(2021, 1, 5)));
        assert_eq!(uses.of(&ids(&[3])), Some(day(2024, 6, 1)));
    }

    #[test]
    fn the_date_fl_keeps_in_the_file_dates_a_song_started_before_its_oldest_save_left() {
        let real = RealDates::now();
        let mut only = save(1, day(2024, 6, 1));
        only.created = Some(day(2018, 9, 9));
        let uses = FirstUses::new(&[song(vec![only])], &real);
        assert_eq!(uses.of(&ids(&[1])), Some(day(2018, 9, 9)));
    }

    #[test]
    fn a_save_that_is_gone_or_unread_says_nothing_of_what_is_not_in_it() {
        let real = RealDates::now();
        let mut gone = save(1, day(2019, 3, 1));
        gone.file_missing = true;
        let mut unread = save(2, day(2020, 3, 1));
        unread.error = Some("damaged".into());
        let songs = [song(vec![gone, unread, save(3, day(2024, 6, 1))])];
        let uses = FirstUses::new(&songs, &real);
        // Neither older save can say it lacked the sound, so it counts from the song's start.
        assert_eq!(uses.of(&ids(&[3])), Some(day(2019, 3, 1)));
    }

    #[test]
    fn the_earliest_song_wins_and_saves_with_no_real_date_are_left_out() {
        let real = RealDates::now();
        let a = song(vec![save(1, day(2022, 1, 1))]);
        let b = song(vec![save(2, day(2020, 1, 1)), save(3, NaiveDateTime::default())]);
        let uses = FirstUses::new(&[a, b], &real);
        assert_eq!(uses.of(&ids(&[1, 2])), Some(day(2020, 1, 1)));
        assert_eq!(uses.of(&ids(&[3])), None);
        assert_eq!(uses.of(&ids(&[99])), None);
    }
}
