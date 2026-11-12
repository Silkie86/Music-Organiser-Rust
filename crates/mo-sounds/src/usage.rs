//! How many of your projects use each sound, from the library's sample lists ([`mo_library::Library::sample_usage`]),
//! as the C# app's `SoundOrganiser.ApplyUsage` matches them.
//!
//! Projects store samples by full path, relative path or just a file name (Bitwig collects copies), so a full-path
//! match is used when there is one, otherwise the file name, but only a name no other sound in the library has:
//! thousands of packs have a "01.wav", and each one was credited with every project using any of them.

use std::collections::{HashMap, HashSet};

use mo_formats::case;
use mo_library::SampleUse;

use crate::{Sound, file_name};

/// One sound's use in your projects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// How many projects use it.
    pub projects: u32,
    /// How many of those were saved in the last year (or since the date the usage was read for).
    pub recent: u32,
    /// The years those projects were saved in, oldest first (for the "Used in" filter).
    pub years: Vec<i32>,
    /// The projects, by id, smallest first, each once (the card lists them).
    pub project_ids: Vec<i64>,
}

impl Usage {
    fn add(&mut self, u: &SampleUse) {
        merge(&mut self.years, &u.years);
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
/// one project listing it twice (a Bitwig copy and a relative path) counts once (the C# app counted it twice). Sounds
/// no project uses are left out (their use is [`Usage::default`]).
pub fn usage(sounds: &[Sound], uses: &[SampleUse]) -> HashMap<i64, Usage> {
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
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::sound;

    #[test]
    fn a_trimmed_copy_in_a_project_counts_for_its_original() {
        let sounds = vec![sound(1, "D:\\Samples\\Kick.wav"), sound(2, "D:\\Samples\\Snare.wav")];
        let copy = format!("E:\\Exports\\Trimmed sounds\\{}", crate::trimmed::copy_name("D:\\Samples\\Kick.wav", -60));
        let found = usage(&sounds, &[used(&copy, 2, 1, &[2026]), used("D:\\Samples\\Kick.wav", 1, 0, &[2025])]);
        assert_eq!(found[&1].projects, 3);
        assert!(!found.contains_key(&2));
    }

    fn used(path: &str, projects: u32, recent: u32, years: &[i32]) -> SampleUse {
        // Projects numbered from the first year they were saved in, one each.
        let project_ids = (0..i64::from(projects)).map(|n| i64::from(years[0]) * 100 + n).collect();
        SampleUse { path: path.into(), projects, recent, years: years.to_vec(), project_ids }
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
        let u = usage(&sounds, &uses);
        assert_eq!(u[&1], Usage { projects: 2, recent: 1, years: vec![2024, 2025], project_ids: vec![202400, 202401] });
        // Which projects too, from both paths.
        assert_eq!(u[&2], Usage { projects: 2, recent: 1, years: vec![2019, 2025], project_ids: vec![201900, 202500] });
        assert!(!u.contains_key(&3) && !u.contains_key(&4) && !u.contains_key(&5));
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
        };
        // Project 7 has a Bitwig copy of the pad and names it by a relative path too.
        let u = usage(&sounds, &[one("C:\\Bitwig\\Song\\samples\\Warm.wav"), one("samples/warm.wav")]);
        assert_eq!(u[&1], Usage { projects: 1, recent: 1, years: vec![2025], project_ids: vec![7] });
        // A use that doesn't say which projects (the twin tool's) adds its count, as before.
        let counted = |path: &str| SampleUse { project_ids: Vec::new(), ..one(path) };
        let u = usage(&sounds, &[counted("C:\\Bitwig\\Song\\samples\\Warm.wav"), counted("samples/warm.wav")]);
        assert_eq!((u[&1].projects, u[&1].recent), (2, 2));
    }
}
