//! What the Space page's last look for Bitwig copies found, kept between runs: the page shows it at once after a
//! start, marked with when it's from, while a new look checks it (Solomon, 25 Nov 2026: the look took 68 seconds after
//! every start, and the card stood empty till then). Only what was found is kept ([`Copies`]): never the ticks, what
//! Fix did, or which fixed songs you checked play, which belong to one run. Nothing acts on what's read back: Fix,
//! Free the space and the fixed projects' button wait for the new look.
//!
//! One file in the data folder, written whole and replaced by each look that ends (rule 28), on
//! [`crate::backup::WRITTEN_WHOLE`] and counted with the caches on the App files card. One too big to be worth
//! keeping ([`KEEP_AT_MOST`]) isn't kept, and the last one goes.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mo_space::bitwig::{LibraryCopy, Save};
use serde::{Deserialize, Serialize};

use crate::space_bitwig::{Copies, Project};

/// The file, in the data folder.
pub const FILE_NAME: &str = "bitwig-copies-found.json";

/// The most the file may take (rule 28). Solomon's list of 25 Nov 2026 (234 GB of copies in 400 projects) would
/// have taken 52.8 MB with every path written out in full, which the first make of the file did; this make writes
/// each path below its project's folder and each original once, and his takes a fraction of that.
pub const KEEP_AT_MOST: usize = 64 * 1024 * 1024;

/// The file's make: one of another make isn't read. 2 since paths are written below their project's folder.
const VERSION: u32 = 2;

/// A file's make, read first: one of another make may hold anything else.
#[derive(Deserialize)]
struct Make {
    version: u32,
}

/// The file. Written small, as the list is long: an original many copies are the same as is written once
/// (`originals`, which copies name by place), and a path in a project's folder is written from that folder on.
#[derive(Serialize, Deserialize)]
struct File<'a> {
    version: u32,
    /// When the look ended, in seconds since 1970.
    found_at: u64,
    library: Cow<'a, [PathBuf]>,
    folders: usize,
    originals: Vec<Cow<'a, str>>,
    projects: Vec<KeptProject<'a>>,
}

#[derive(Serialize, Deserialize)]
struct KeptProject<'a> {
    folder: Cow<'a, str>,
    copies: Vec<KeptCopy<'a>>,
    saves: Vec<KeptSave<'a>>,
}

/// A copy: its path, its original's place in `originals`, its size.
#[derive(Serialize, Deserialize)]
struct KeptCopy<'a>(Below<'a>, u32, u64);

#[derive(Serialize, Deserialize)]
struct KeptSave<'a> {
    path: Below<'a>,
    #[serde(with = "kept_time")]
    modified: Option<SystemTime>,
    below: bool,
    fixed: Option<(Below<'a>, bool)>,
}

/// How a save's time is kept in a file: seconds and nanoseconds since 1970. A time before then (a zeroed or broken
/// date) or past what a time can hold is kept and read as none, so one odd file never stops a whole list being kept
/// or read.
mod kept_time {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(time: &Option<SystemTime>, to: S) -> Result<S::Ok, S::Error> {
        let since = time.and_then(|t| t.duration_since(UNIX_EPOCH).ok());
        since.map(|d| (d.as_secs(), d.subsec_nanos())).serialize(to)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<Option<SystemTime>, D::Error> {
        let kept = Option::<(u64, u32)>::deserialize(from)?;
        Ok(kept.and_then(|(secs, nanos)| {
            // Past the year 9999 no clock or calendar here reads it.
            const LATEST: u64 = 253_402_300_799;
            (secs <= LATEST && nanos < 1_000_000_000).then(|| UNIX_EPOCH.checked_add(Duration::new(secs, nanos)))?
        }))
    }
}

/// A path as the file holds it: what follows the project's folder when it begins with it (true), or all of it.
#[derive(Serialize, Deserialize)]
struct Below<'a>(Cow<'a, str>, bool);

impl<'a> Below<'a> {
    fn of(folder: &str, path: &'a str) -> Self {
        match path.strip_prefix(folder) {
            Some(rest) => Self(Cow::Borrowed(rest), true),
            None => Self(Cow::Borrowed(path), false),
        }
    }

    fn whole(self, folder: &str) -> String {
        if self.1 { format!("{folder}{}", self.0) } else { self.0.into_owned() }
    }
}

/// A path as text, or why the list can't be kept.
fn text(path: &Path) -> Result<&str, String> {
    path.to_str().ok_or_else(|| format!("{} isn't text", path.display()))
}

fn to_file(found: &Copies, found_at: u64) -> Result<File<'_>, String> {
    let mut originals: Vec<Cow<'_, str>> = Vec::new();
    let mut places: HashMap<&str, u32> = HashMap::new();
    let mut projects = Vec::with_capacity(found.projects.len());
    for p in &found.projects {
        let folder = text(&p.folder)?;
        let mut copies = Vec::with_capacity(p.copies.len());
        for c in &p.copies {
            let place = match places.get(c.library_file.as_str()) {
                Some(&place) => place,
                None => {
                    let place = u32::try_from(originals.len()).map_err(|_| "too many originals".to_owned())?;
                    originals.push(Cow::Borrowed(c.library_file.as_str()));
                    places.insert(&c.library_file, place);
                    place
                }
            };
            copies.push(KeptCopy(Below::of(folder, &c.in_project), place, c.bytes));
        }
        let mut saves = Vec::with_capacity(p.saves.len());
        for s in &p.saves {
            let fixed = match &s.fixed {
                Some((path, new)) => Some((Below::of(folder, text(path)?), *new)),
                None => None,
            };
            saves.push(KeptSave {
                path: Below::of(folder, text(&s.path)?),
                modified: s.modified,
                below: s.below,
                fixed,
            });
        }
        projects.push(KeptProject { folder: Cow::Borrowed(folder), copies, saves });
    }
    Ok(File {
        version: VERSION,
        found_at,
        library: Cow::Borrowed(&found.library),
        folders: found.folders,
        originals,
        projects,
    })
}

fn from_file(file: File<'_>) -> Result<Copies, String> {
    let originals = file.originals;
    let mut projects = Vec::with_capacity(file.projects.len());
    for p in file.projects {
        let folder = p.folder;
        let mut copies = Vec::with_capacity(p.copies.len());
        for KeptCopy(path, place, bytes) in p.copies {
            let library_file = originals.get(place as usize).ok_or("a copy names an original that isn't there")?;
            copies.push(LibraryCopy { in_project: path.whole(&folder), library_file: library_file.to_string(), bytes });
        }
        let saves = p
            .saves
            .into_iter()
            .map(|s| Save {
                path: PathBuf::from(s.path.whole(&folder)),
                modified: s.modified,
                below: s.below,
                fixed: s.fixed.map(|(path, new)| (PathBuf::from(path.whole(&folder)), new)),
            })
            .collect();
        projects.push(Project { folder: PathBuf::from(folder.into_owned()), copies, saves });
    }
    Ok(Copies { library: file.library.into_owned(), folders: file.folders, projects })
}

/// Keeps what a look that ended at `now` found, in place of the last one. Not kept (and the last one removed, as
/// it's older than this look): one over [`KEEP_AT_MOST`], or with a path that isn't text.
pub fn keep(data: &Path, found: &Copies, now: SystemTime) -> io::Result<()> {
    let path = data.join(FILE_NAME);
    let found_at = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let written = to_file(found, found_at).and_then(|file| serde_json::to_string(&file).map_err(|e| e.to_string()));
    let text = match written {
        Ok(text) if text.len() <= KEEP_AT_MOST => text,
        Ok(text) => {
            forget(data);
            return Err(io::Error::other(format!("it would take {} bytes, more than is kept", text.len())));
        }
        Err(e) => {
            forget(data);
            return Err(io::Error::other(e));
        }
    };
    crate::backup::write_whole(&path, &text)
}

/// What the last look found and when it ended. None when nothing's kept, or it can't be read (said in the log; the
/// new look's own takes its place).
pub fn kept(data: &Path) -> Option<(Copies, SystemTime)> {
    let path = data.join(FILE_NAME);
    match fs::metadata(&path) {
        Ok(m) if m.len() <= KEEP_AT_MOST as u64 => {}
        Ok(_) => return None,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            crate::log::warning(
                "Space",
                &format!("Couldn't read the last look's Bitwig copies ({}): {e}", path.display()),
            );
            return None;
        }
    }
    let read = fs::read(&path).map_err(|e| e.to_string()).and_then(|bytes| {
        let make = serde_json::from_slice::<Make>(&bytes).map_err(|e| e.to_string())?;
        if make.version != VERSION {
            return Ok(None);
        }
        let file = serde_json::from_slice::<File>(&bytes).map_err(|e| e.to_string())?;
        // A time no clock holds is a damaged file's.
        let at = UNIX_EPOCH.checked_add(Duration::from_secs(file.found_at)).ok_or("its time can't be read")?;
        Ok(Some((from_file(file)?, at)))
    });
    match read {
        Ok(kept) => kept,
        Err(e) => {
            crate::log::warning(
                "Space",
                &format!("Couldn't read the last look's Bitwig copies ({}): {e}", path.display()),
            );
            None
        }
    }
}

/// Removes what's kept, when there is any.
pub fn forget(data: &Path) {
    let path = data.join(FILE_NAME);
    if let Err(e) = fs::remove_file(&path)
        && e.kind() != io::ErrorKind::NotFound
    {
        crate::log::warning(
            "Space",
            &format!("Couldn't remove the last look's Bitwig copies ({}): {e}", path.display()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn found() -> Copies {
        let at = UNIX_EPOCH + Duration::new(1_795_000_000, 123_456_700);
        Copies {
            library: vec![PathBuf::from(r"K:\Bitwig Studio\Library")],
            folders: 428,
            projects: vec![Project {
                folder: PathBuf::from(r"K:\Projects\GARAGE é"),
                copies: vec![LibraryCopy {
                    in_project: r"K:\Projects\GARAGE é\multi-samples\Keys 2.multisample".to_owned(),
                    library_file: r"K:\Bitwig Studio\Library\Multisamples\Keys.multisample".to_owned(),
                    bytes: 5_000_000_000,
                }],
                saves: vec![
                    Save {
                        path: PathBuf::from(r"K:\Projects\GARAGE é\GARAGE.bwproject"),
                        modified: Some(at),
                        below: false,
                        fixed: Some((PathBuf::from(r"K:\Projects\GARAGE é\GARAGE (fixed).bwproject"), true)),
                    },
                    Save {
                        path: PathBuf::from(r"K:\Projects\GARAGE é\auto-backups\GARAGE-3.bwproject"),
                        modified: None,
                        below: true,
                        fixed: None,
                    },
                ],
            }],
        }
    }

    #[test]
    fn what_a_look_found_comes_back_as_it_was_with_when_it_ended() {
        let data = dir("mo-app-bitwig-kept");
        assert_eq!(kept(&data), None, "nothing kept yet");
        let ended = UNIX_EPOCH + Duration::from_secs(1_795_600_920);
        keep(&data, &found(), ended).unwrap();
        assert_eq!(kept(&data), Some((found(), ended)));
        // The next look's takes its place: one file, and nothing left beside it.
        let none = Copies { folders: 3, ..Copies::default() };
        keep(&data, &none, ended + Duration::from_secs(60)).unwrap();
        assert_eq!(kept(&data), Some((none, ended + Duration::from_secs(60))));
        let names: Vec<_> = fs::read_dir(&data).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, [FILE_NAME]);
        forget(&data);
        assert_eq!(kept(&data), None);
        forget(&data);
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn a_file_that_cant_be_read_or_is_another_make_gives_nothing() {
        let data = dir("mo-app-bitwig-kept-damaged");
        let file = data.join(FILE_NAME);
        let far = format!(
            "{{\"version\":2,\"found_at\":{},\"library\":[],\"folders\":0,\"originals\":[],\"projects\":[]}}",
            u64::MAX
        );
        let partly = "{\"version\":2,\"found_at\":5,\"library\":[]}";
        // A copy that names an original the file doesn't hold.
        let no_original = "{\"version\":2,\"found_at\":5,\"library\":[],\"folders\":0,\"originals\":[],\"projects\":[\
            {\"folder\":\"a\",\"copies\":[[[\"/b\",true],0,5]],\"saves\":[]}]}";
        for text in ["", "not json", "{\"version\":2}", partly, &far, no_original] {
            fs::write(&file, text).unwrap();
            assert_eq!(kept(&data), None, "{text}");
        }
        let found_before = found();
        let mut other = to_file(&found_before, 5).unwrap();
        other.version = VERSION + 1;
        fs::write(&file, serde_json::to_string(&other).unwrap()).unwrap();
        assert_eq!(kept(&data), None, "a later make");
        fs::write(&file, "{\"version\":3,\"something\":\"else\"}").unwrap();
        assert_eq!(kept(&data), None, "a later make of another shape");
        // The first make, which wrote every path in full.
        fs::write(&file, "{\"version\":1,\"found_at\":5,\"found\":{\"library\":[],\"folders\":0,\"projects\":[]}}")
            .unwrap();
        assert_eq!(kept(&data), None, "the first make");
        // A new look's own takes the place of any of them.
        keep(&data, &found(), UNIX_EPOCH).unwrap();
        assert_eq!(kept(&data), Some((found(), UNIX_EPOCH)));
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn a_save_dated_before_1970_or_past_any_calendar_is_kept_without_its_date() {
        let data = dir("mo-app-bitwig-kept-odd-dates");
        let mut odd = found();
        odd.projects[0].saves[0].modified = Some(UNIX_EPOCH - Duration::from_secs(86_400));
        keep(&data, &odd, UNIX_EPOCH).unwrap();
        odd.projects[0].saves[0].modified = None;
        assert_eq!(kept(&data), Some((odd, UNIX_EPOCH)));
        keep(&data, &found(), UNIX_EPOCH).unwrap();
        let text = fs::read_to_string(data.join(FILE_NAME)).unwrap();
        assert!(text.contains("[1795000000,123456700]"), "{text}");
        fs::write(data.join(FILE_NAME), text.replace("[1795000000,123456700]", "[99999999999999,0]")).unwrap();
        let (read, _) = kept(&data).unwrap();
        assert_eq!(read.projects[0].saves[0].modified, None);
        let _ = fs::remove_dir_all(&data);
    }

    /// Solomon's list of 25 Nov 2026 took 52.8 MB as the first make wrote it, over the limit, so nothing was kept
    /// and Space looked afresh at every start all the same.
    #[test]
    fn a_list_as_long_as_solomons_is_small_enough_to_keep() {
        let data = dir("mo-app-bitwig-kept-long");
        let library = r"K:\Bitwig Studio\Library\Multisamples\Solomon's own\Keys and pads";
        let projects = (0..400)
            .map(|p| {
                let folder = format!(r"K:\Music\Bitwig Projects\2026\A song with a long name {p}");
                Project {
                    copies: (0..400)
                        .map(|c| LibraryCopy {
                            in_project: format!(r"{folder}\multi-samples\A multisample's name {c} 2.multisample"),
                            library_file: format!(r"{library}\A multisample's name {c}.multisample"),
                            bytes: 1_500_000,
                        })
                        .collect(),
                    saves: (0..30)
                        .map(|s| Save {
                            path: PathBuf::from(format!(r"{folder}\auto-backups\A song {s}.bwproject")),
                            modified: Some(UNIX_EPOCH + Duration::new(1_795_000_000 + s, 5)),
                            below: true,
                            fixed: None,
                        })
                        .collect(),
                    folder: PathBuf::from(folder),
                }
            })
            .collect();
        let long = Copies { library: vec![PathBuf::from(library)], folders: 428, projects };
        assert_eq!(long.projects.iter().map(|p| p.copies.len()).sum::<usize>(), 160_000);
        keep(&data, &long, UNIX_EPOCH).unwrap();
        let size = fs::metadata(data.join(FILE_NAME)).unwrap().len();
        let back = kept(&data);
        // Before the checks: a failure mustn't leave the file behind. (And not `assert_eq`, which would print both.)
        let _ = fs::remove_dir_all(&data);
        assert!(size < KEEP_AT_MOST as u64 / 4, "{size} bytes");
        assert!(back == Some((long, UNIX_EPOCH)), "the long list came back changed");
    }

    /// A path outside its project's folder (none is today) is kept whole.
    #[test]
    fn a_path_outside_its_projects_folder_comes_back_as_it_was() {
        let data = dir("mo-app-bitwig-kept-outside");
        let mut odd = found();
        odd.projects[0].copies[0].in_project = r"Z:\elsewhere\Keys 2.multisample".to_owned();
        odd.projects[0].saves[0].fixed = Some((PathBuf::from(r"Z:\elsewhere\GARAGE (fixed).bwproject"), false));
        keep(&data, &odd, UNIX_EPOCH).unwrap();
        assert_eq!(kept(&data), Some((odd, UNIX_EPOCH)));
        let _ = fs::remove_dir_all(&data);
    }

    #[test]
    fn its_name_is_on_the_list_that_clears_what_a_crash_leaves_of_it() {
        assert!(crate::backup::WRITTEN_WHOLE.contains(&FILE_NAME));
    }

    #[test]
    fn an_earlier_runs_finds_say_when_theyre_from() {
        use crate::space_bitwig::{card, found_when, from_words, line, mark_card, mark_line};
        let from = SystemTime::now();
        let when = chrono::DateTime::<chrono::Local>::from(from).format("%-d %b, %H:%M").to_string();
        assert_eq!(found_when(from, from), when);
        // Another year's says its year; a time no calendar reads still says something.
        let last_year = from - Duration::from_secs(400 * 86_400);
        let with_year = chrono::DateTime::<chrono::Local>::from(last_year).format("%-d %b %Y, %H:%M").to_string();
        assert_eq!(found_when(last_year, from), with_year);
        // (Where the clock itself can hold such a time: Windows' can't, so there it never comes up.)
        if let Some(far) = UNIX_EPOCH.checked_add(Duration::from_secs(u64::MAX / 4)) {
            assert_eq!(found_when(far, from), "an earlier day");
        }
        assert_eq!(from_words(from, true), format!("As found {when}. Checking again…"));
        assert_eq!(from_words(from, false), format!("As found {when}. Measure again checks it."));
        let found = found();
        let (_, mut l) = line(Some(&found), Some((0, 0))).unwrap();
        mark_line(&mut l, from);
        assert_eq!(l.why, format!("Copies of your multisamples and samples in 1 project, as found {when}"));
        let mut c = card(Some(&found), Some((0, 0)), &Default::default());
        mark_card(&mut c, from, true);
        assert_eq!(c.big_note, format!("1 copy in 1 project. As found {when}. Checking again…"));
    }
}
