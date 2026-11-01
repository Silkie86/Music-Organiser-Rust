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
}

impl Usage {
    fn add(&mut self, u: &SampleUse) {
        self.projects += u.projects;
        self.recent += u.recent;
        if self.years.is_empty() {
            self.years.clone_from(&u.years);
        } else if !u.years.is_empty() {
            self.years.extend(&u.years);
            self.years.sort_unstable();
            self.years.dedup();
        }
    }
}

/// Each sound's use, by sound id. As in the C# app, a sound matched by its name adds up every path with that name, so
/// one project listing it twice (a Bitwig copy and a relative path) counts twice. Sounds no project uses are left out (their use is [`Usage::default`]).
pub fn usage(sounds: &[Sound], uses: &[SampleUse]) -> HashMap<i64, Usage> {
    let mut by_path: HashMap<String, Usage> = HashMap::new();
    let mut by_name: HashMap<String, Usage> = HashMap::new();
    for u in uses {
        by_path.entry(case::key(&u.path)).or_default().add(u);
        by_name.entry(case::key(file_name(&u.path))).or_default().add(u);
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

    fn used(path: &str, projects: u32, recent: u32, years: &[i32]) -> SampleUse {
        SampleUse { path: path.into(), projects, recent, years: years.to_vec() }
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
        assert_eq!(u[&1], Usage { projects: 2, recent: 1, years: vec![2024, 2025] });
        assert_eq!(u[&2], Usage { projects: 2, recent: 1, years: vec![2019, 2025] });
        assert!(!u.contains_key(&3) && !u.contains_key(&4) && !u.contains_key(&5));
    }
}
