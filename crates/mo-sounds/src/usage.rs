//! How many of your saves use each sound, from the library's sample lists ([`mo_library::Library::sample_usage`]),
//! as the C# app's `SoundOrganiser.ApplyUsage` matches them, and how many songs those are: the saves of one song, as
//! the Songs page groups them, count as one (Solomon's F12 note of 23 Nov 2026: a sound "used in 44 projects" was in
//! 44 saves of one song, "basically the same project").
//!
//! Projects store samples by full path, relative path or just a file name (Bitwig collects copies), so a full-path
//! match is used when there is one, otherwise the file name, but only a name no other sound in the library has:
//! thousands of packs have a "01.wav", and each one was credited with every project using any of them.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDateTime;
use mo_formats::case;
use mo_library::{ProjectFile, SampleUse};

use crate::{Sound, file_name};

/// One sound's use in your songs and their saves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// How many projects use it: each save of a song is one.
    pub projects: u32,
    /// How many of those were saved in the last year (or since the date the usage was read for).
    pub recent: u32,
    /// How many songs use it: a song's saves that use it count once. What the page says a sound is used in.
    pub songs: u32,
    /// How many of those have a save that uses it saved in the last year (or since the date the usage was read for).
    pub recent_songs: u32,
    /// The years those projects were saved in, oldest first (for the "Used in" filter).
    pub years: Vec<i32>,
    /// The projects, by id, smallest first, each once (the card lists them).
    pub project_ids: Vec<i64>,
    /// When you first used it in a song: the earliest of its paths' ([`SampleUse::first_used`]).
    pub first_used: Option<NaiveDateTime>,
}

impl Usage {
    fn add(&mut self, u: &SampleUse) {
        merge(&mut self.years, &u.years);
        self.first_used = self.first_used.into_iter().chain(u.first_used).min();
        if u.project_ids.is_empty() {
            // A use that doesn't say which projects (the twin tool's, made from the C# app's counts) adds its count.
            self.projects += u.projects;
        } else {
            // Each project once, whichever paths it names the sound by.
            merge(&mut self.project_ids, &u.project_ids);
            self.projects = u32::try_from(self.project_ids.len()).unwrap_or(u32::MAX);
        }
        // Never more saved lately than in all (a project naming it by two paths can still count twice here).
        self.recent = (self.recent + u.recent).min(self.projects);
    }
}

/// Puts `more` into `sorted`, keeping it sorted with each once.
fn merge<T: Ord + Copy>(sorted: &mut Vec<T>, more: &[T]) {
    if sorted.is_empty() {
        sorted.extend_from_slice(more);
    } else if !more.is_empty() {
        sorted.extend_from_slice(more);
        sorted.sort_unstable();
        sorted.dedup();
    }
}

/// Each sound's use, by sound id. A sound matched by its name adds up every path with that name, each project once:
/// one project listing it twice (a Bitwig copy and a relative path) counts once (the C# app counted it twice). Its
/// songs are counted from `projects`, the files of the projects counted, by id, as
/// [`mo_library::Library::sample_usage_with_projects`] reads them with `uses`. Sounds no project uses are left out
/// (their use is [`Usage::default`]).
pub fn usage(sounds: &[Sound], uses: &[SampleUse], projects: &HashMap<i64, ProjectFile>) -> HashMap<i64, Usage> {
    // A trimmed copy Trim silence dragged into a project counts as a use of its original, found by the mark in the
    // copy's name ([`crate::trimmed`]). Worked out only when some project uses a copy.
    let copies = uses.iter().any(|u| crate::trimmed::mark_in(file_name(&u.path)).is_some());
    let originals: HashMap<String, &str> = if copies {
        sounds.iter().map(|s| (crate::trimmed::mark(&s.path), s.path.as_str())).collect()
    } else {
        HashMap::new()
    };
    let mut by_path: HashMap<String, Usage> = HashMap::new();
    let mut by_name: HashMap<String, Usage> = HashMap::new();
    for u in uses {
        let original = crate::trimmed::mark_in(file_name(&u.path)).and_then(|m| originals.get(m)).copied();
        let path = original.unwrap_or(&u.path);
        by_path.entry(case::key(path)).or_default().add(u);
        by_name.entry(case::key(file_name(path))).or_default().add(u);
    }
    // File names more than one sound in the library has: a project naming only that can't say which it meant.
    let mut seen = HashSet::new();
    let shared: HashSet<String> =
        sounds.iter().map(|s| case::key(file_name(&s.path))).filter(|name| !seen.insert(name.clone())).collect();
    let mut out = HashMap::new();
    for s in sounds {
        let name = case::key(file_name(&s.path));
        let found = by_path
            .get(&case::key(&s.path))
            .or_else(|| (!shared.contains(&name)).then(|| by_name.get(&name)).flatten());
        if let Some(u) = found.filter(|u| u.projects > 0) {
            out.insert(s.id, u.clone());
        }
    }
    for u in out.values_mut() {
        u.count_songs(projects);
    }
    out
}

impl Usage {
    /// Works out [`Usage::songs`] and [`Usage::recent_songs`] from the songs its projects are saves of (`projects`, by
    /// id, as read with the counts).
    fn count_songs(&mut self, projects: &HashMap<i64, ProjectFile>) {
        if self.project_ids.is_empty() {
            // A count that doesn't say which projects (the twin tool's) can't say which songs: each is one.
            (self.songs, self.recent_songs) = (self.projects, self.recent);
            return;
        }
        let mut songs = Vec::with_capacity(self.project_ids.len());
        let mut recent = Vec::new();
        // A project not read with the counts is a song of its own, not known to be recent. That can't happen when the
        // counts and `projects` come from one read (`sample_usage_with_projects`), as the page reads them; the card,
        // which can only list what `projects` has, would leave such a project out.
        let mut unknown = 0u32;
        for id in &self.project_ids {
            match projects.get(id) {
                Some(p) => {
                    songs.push(p.song);
                    if p.recent {
                        recent.push(p.song);
                    }
                }
                None => unknown = unknown.saturating_add(1),
            }
        }
        for found in [&mut songs, &mut recent] {
            found.sort_unstable();
            found.dedup();
        }
        let count = |found: &[u32]| u32::try_from(found.len()).unwrap_or(u32::MAX);
        self.songs = count(&songs).saturating_add(unknown);
        self.recent_songs = count(&recent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::sound;

    #[test]
    fn a_trimmed_copy_in_a_project_counts_for_its_original() {
        let sounds = vec![sound(1, "D:\\Samples\\Kick.wav"), sound(2, "D:\\Samples\\Snare.wav")];
        let copy = format!("E:\\Exports\\Trimmed sounds\\{}", crate::trimmed::copy_name("D:\\Samples\\Kick.wav", -60));
        let found = usage(
            &sounds,
            &[used(&copy, 2, 1, &[2026]), used("D:\\Samples\\Kick.wav", 1, 0, &[2025])],
            &HashMap::new(),
        );
        assert_eq!(found[&1].projects, 3);
        assert!(!found.contains_key(&2));
    }

    fn used(path: &str, projects: u32, recent: u32, years: &[i32]) -> SampleUse {
        // Projects numbered from the first year they were saved in, one each.
        let project_ids = (0..i64::from(projects)).map(|n| i64::from(years[0]) * 100 + n).collect();
        SampleUse { path: path.into(), projects, recent, years: years.to_vec(), project_ids, first_used: None }
    }

    #[test]
    fn full_paths_first_then_names_no_other_sound_has() {
        let sounds = [
            sound(1, "D:\\Samples\\Kicks\\Kick 1.wav"),
            sound(2, "D:\\Samples\\Pads\\Warm.wav"),
            // Two packs with a 01.wav: a project naming just "01.wav" credits neither.
            sound(3, "D:\\Samples\\A\\01.wav"),
            sound(4, "D:\\Samples\\B\\01.wav"),
            sound(5, "D:\\Samples\\Unused.wav"),
        ];
        let uses = [
            used("d:\\samples\\kicks\\KICK 1.wav", 2, 1, &[2024, 2025]),
            // A Bitwig copy of the pad, and another project with it by a relative path.
            used("C:\\Bitwig\\Song\\samples\\Warm.wav", 1, 0, &[2019]),
            used("samples/warm.wav", 1, 1, &[2025]),
            used("01.wav", 4, 4, &[2025]),
            // The kick by name too: the full path wins.
            used("Kick 1.wav", 9, 9, &[2010]),
        ];
        let u = usage(&sounds, &uses, &HashMap::new());
        // No project files given: each project is a song of its own, not known to be recent.
        assert_eq!(
            u[&1],
            Usage {
                projects: 2,
                recent: 1,
                songs: 2,
                recent_songs: 0,
                years: vec![2024, 2025],
                project_ids: vec![202400, 202401],
                first_used: None
            }
        );
        // Which projects too, from both paths.
        assert_eq!(
            u[&2],
            Usage {
                projects: 2,
                recent: 1,
                songs: 2,
                recent_songs: 0,
                years: vec![2019, 2025],
                project_ids: vec![201900, 202500],
                first_used: None
            }
        );
        assert!(!u.contains_key(&3) && !u.contains_key(&4) && !u.contains_key(&5));
    }

    #[test]
    fn a_sound_named_by_two_paths_was_first_used_by_the_earlier() {
        let sounds = [sound(1, "D:\\Samples\\Pads\\Warm.wav")];
        let day = |y: i32| chrono::NaiveDate::from_ymd_opt(y, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0));
        let with =
            |path: &str, first: Option<NaiveDateTime>| SampleUse { first_used: first, ..used(path, 1, 0, &[2025]) };
        let u = usage(
            &sounds,
            &[
                with("samples/warm.wav", day(2024)),
                with("D:\\Samples\\Pads\\Warm.wav", day(2019)),
                with("Warm.wav", None),
            ],
            &HashMap::new(),
        );
        assert_eq!(u[&1].first_used, day(2019));
    }

    #[test]
    fn a_project_naming_a_sound_by_two_paths_counts_once() {
        let sounds = [sound(1, "D:\\Samples\\Pads\\Warm.wav")];
        let one = |path: &str| SampleUse {
            path: path.into(),
            projects: 1,
            recent: 1,
            years: vec![2025],
            project_ids: vec![7],
            first_used: None,
        };
        // Project 7 has a Bitwig copy of the pad and names it by a relative path too.
        let files = HashMap::from([(7, file(7, 0, true))]);
        let u = usage(&sounds, &[one("C:\\Bitwig\\Song\\samples\\Warm.wav"), one("samples/warm.wav")], &files);
        assert_eq!(
            u[&1],
            Usage {
                projects: 1,
                recent: 1,
                songs: 1,
                recent_songs: 1,
                years: vec![2025],
                project_ids: vec![7],
                first_used: None
            }
        );
        // A use that doesn't say which projects (the twin tool's) adds its count, as before, each a song.
        let counted = |path: &str| SampleUse { project_ids: Vec::new(), ..one(path) };
        let u = usage(&sounds, &[counted("C:\\Bitwig\\Song\\samples\\Warm.wav"), counted("samples/warm.wav")], &files);
        assert_eq!((u[&1].projects, u[&1].recent, u[&1].songs, u[&1].recent_songs), (2, 2, 2, 2));
    }

    /// A project's file: `song` is which song it's a save of.
    fn file(id: i64, song: u32, recent: bool) -> ProjectFile {
        ProjectFile {
            id,
            path: format!("C:\\Songs\\{id}.flp"),
            mtime: 0,
            song,
            song_name: "Song".into(),
            recent,
            copy: false,
        }
    }

    #[test]
    fn the_saves_of_one_song_count_as_one_song() {
        // Solomon's note of 23 Nov 2026: a cymbal in 44 saves of "bass junkie_50k", a song of their own, read "used in
        // 44 projects". Here: projects 1 to 4 are saves of song 0 (only 4 saved lately), 5 is song 1, 6 is song 2.
        let sounds = [sound(1, "D:\\Samples\\Cymbal.wav"), sound(2, "D:\\Samples\\Kick.wav")];
        let files: HashMap<i64, ProjectFile> = [(1, 0, false), (2, 0, false), (3, 0, false), (4, 0, true)]
            .into_iter()
            .chain([(5, 1, true), (6, 2, false)])
            .map(|(id, song, recent)| (id, file(id, song, recent)))
            .collect();
        let at = |path: &str, ids: &[i64]| SampleUse {
            path: path.into(),
            projects: ids.len() as u32,
            recent: 0,
            years: vec![2010],
            project_ids: ids.to_vec(),
            first_used: None,
        };
        // The cymbal by two paths: the saves of song 0 by its full path, song 0's newest and song 1 by its name.
        let uses = [at("D:\\Samples\\Cymbal.wav", &[1, 2, 3]), at("Cymbal.wav", &[4, 5]), at("Kick.wav", &[6, 9])];
        let u = usage(&sounds, &uses, &files);
        // The full path wins over the name, as before: its three saves are one song.
        assert_eq!((u[&1].projects, u[&1].songs, u[&1].recent_songs), (3, 1, 0));
        // Project 9 wasn't read with the counts: a song of its own.
        assert_eq!((u[&2].projects, u[&2].songs, u[&2].recent_songs), (2, 2, 0));
        // Another cymbal, found by its name alone: every path of that name, saves 1 to 5, in songs 0 and 1, each with a
        // save made lately (4 and 5).
        let by_name = usage(&[sound(3, "E:\\Other\\Cymbal.wav")], &uses, &files);
        assert_eq!((by_name[&3].projects, by_name[&3].songs, by_name[&3].recent_songs), (5, 2, 2));
    }
}
