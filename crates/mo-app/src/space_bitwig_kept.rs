//! What the Space page's last look for Bitwig copies found, kept between runs: the page shows it at once after a
//! start, marked with when it's from, while a new look checks it (Solomon, 25 Nov 2026: the look took 68 seconds after
//! every start, and the card stood empty till then). Only what was found is kept ([`Copies`]): never the ticks, what
//! Fix did, or which fixed songs you checked play, which belong to one run. Nothing acts on what's read back: Fix,
//! Free the space and the fixed projects' button wait for the new look.
//!
//! One file in the data folder, written whole and replaced by each look that ends (rule 28), on
//! [`crate::backup::WRITTEN_WHOLE`] and counted with the caches on the App files card. One too big to be worth
//! keeping ([`KEEP_AT_MOST`]) isn't kept, and the last one goes.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::space_bitwig::Copies;

/// The file, in the data folder.
pub const FILE_NAME: &str = "bitwig-copies-found.json";

/// The most the file may take (rule 28): a few thousand copies take about a megabyte.
pub const KEEP_AT_MOST: usize = 32 * 1024 * 1024;

/// The file's make: one of another make isn't read.
const VERSION: u32 = 1;

#[derive(Serialize)]
struct Writes<'a> {
    version: u32,
    /// When the look ended, in seconds since 1970.
    found_at: u64,
    found: &'a Copies,
}

/// A file's make, read first: one of another make may hold anything else.
#[derive(Deserialize)]
struct Make {
    version: u32,
}

#[derive(Deserialize)]
struct Reads {
    found_at: u64,
    found: Copies,
}

/// Keeps what a look that ended at `now` found, in place of the last one. Not kept (and the last one removed, as
/// it's older than this look): one over [`KEEP_AT_MOST`], or with a path that isn't text.
pub fn keep(data: &Path, found: &Copies, now: SystemTime) -> io::Result<()> {
    let path = data.join(FILE_NAME);
    let found_at = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let text = match serde_json::to_string(&Writes { version: VERSION, found_at, found }) {
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
        let read = serde_json::from_slice::<Reads>(&bytes).map_err(|e| e.to_string())?;
        // A time no clock holds is a damaged file's.
        let at = UNIX_EPOCH.checked_add(Duration::from_secs(read.found_at)).ok_or("its time can't be read")?;
        Ok(Some((read.found, at)))
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
    use std::path::PathBuf;

    use mo_space::bitwig::{LibraryCopy, Save};

    use super::*;
    use crate::space_bitwig::Project;

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
            "{{\"version\":1,\"found_at\":{},\"found\":{{\"library\":[],\"folders\":0,\"projects\":[]}}}}",
            u64::MAX
        );
        let partly = "{\"version\":1,\"found_at\":5,\"found\":{\"library\":[]}}";
        for text in ["", "not json", "{\"version\":1}", partly, &far] {
            fs::write(&file, text).unwrap();
            assert_eq!(kept(&data), None, "{text}");
        }
        let other = serde_json::to_string(&Writes { version: VERSION + 1, found_at: 5, found: &found() }).unwrap();
        fs::write(&file, other).unwrap();
        assert_eq!(kept(&data), None, "a later make");
        fs::write(&file, "{\"version\":2,\"something\":\"else\"}").unwrap();
        assert_eq!(kept(&data), None, "a later make of another shape");
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
