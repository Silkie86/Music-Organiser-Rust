//! The songs twin: groups the library into songs the C# app's way (through the C# helper, on a copy of the library)
//! and the Rust way (on the library itself, opened read-only), and sorts every difference by its cause. One cause is
//! meant: the same file found in two places (a copied project folder) is one save in Rust, so the songs holding it are
//! one song, and the extra copy is hidden. Any other difference is one to look at.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

use mo_formats::case;
use mo_library::songs::{Song, file_id, group_songs};
use mo_library::{Library, Save};
use serde_json::Value;

/// How the C# app grouped one save.
struct CsSave {
    key: String,
    latest: bool,
}

/// What the songs twin found: the summary (counts only) and the report (names and paths, to keep on the PC).
pub struct Outcome {
    pub summary: String,
    pub report: String,
    /// Whether every difference is one the copied folders explain.
    pub same: bool,
}

/// Groups the library at `library` both ways and compares them. Both sides read one copy, made at `copy` and deleted
/// afterwards, so a save made meanwhile can't make the two differ.
pub fn run(helper: &Path, library: &Path, copy: &Path) -> Result<Outcome, String> {
    let lib = Library::open_read_only(library).map_err(|e| format!("Couldn't open the library: {e}"))?;
    lib.copy_to(copy).map_err(|e| format!("Couldn't copy the library: {e}"))?;
    drop(lib);
    let _gone = Delete(copy);
    let lib = Library::open_read_only(copy).map_err(|e| format!("Couldn't open the library's copy: {e}"))?;
    let pending = lib.pending_fixes().map_err(|e| e.to_string())?;
    let mut saves = lib.saves().map_err(|e| e.to_string())?;
    saves.retain(|s| !pending.contains(&s.path));
    let choices = lib.song_choices().map_err(|e| e.to_string())?;
    drop(lib);
    let songs = group_songs(&saves, &choices);
    let cs = cs_songs(helper, copy)?;
    Ok(compare(&cs, &songs, saves.len()))
}

/// Deletes the library's copy (and SQLite's files beside it) when the songs twin ends, however it ends: it holds the
/// library's song names and paths.
struct Delete<'a>(&'a Path);

impl Drop for Delete<'_> {
    fn drop(&mut self) {
        for end in ["", "-wal", "-shm", "-journal"] {
            let mut name = self.0.as_os_str().to_owned();
            name.push(end);
            let _ = std::fs::remove_file(name);
        }
    }
}

/// Runs the C# helper's `--songs` on the copy, one save a line.
fn cs_songs(helper: &Path, copy: &Path) -> Result<HashMap<String, CsSave>, String> {
    let mut command = if helper.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll")) {
        let mut c = Command::new("dotnet");
        c.arg(helper);
        c
    } else {
        Command::new(helper)
    };
    let mut child = command
        .arg("--songs")
        .arg(copy)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't start the C# helper ({}): {e}", helper.display()))?;
    let stdout = child.stdout.take().ok_or("The C# helper has no output.")?;
    let read = read_cs(stdout);
    if read.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let out = read?;
    if !status.success() {
        return Err(format!("The C# helper stopped with {status}."));
    }
    Ok(out)
}

fn read_cs(stdout: impl std::io::Read) -> Result<HashMap<String, CsSave>, String> {
    let mut out = HashMap::new();
    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|e| format!("Couldn't read the C# helper's answer: {e}"))?;
        // dotnet may say something of its own first (its first-run note): only the helper's lines are JSON.
        if !line.starts_with('{') {
            continue;
        }
        let v: Value =
            serde_json::from_str(&line).map_err(|e| format!("The C# helper wrote something odd ({e}): {line}"))?;
        let (Some(path), Some(key)) = (v.get("path").and_then(Value::as_str), v.get("key").and_then(Value::as_str))
        else {
            continue;
        };
        let latest = v.get("latest").and_then(Value::as_bool).unwrap_or(false);
        out.insert(case::key(path), CsSave { key: key.to_owned(), latest });
    }
    Ok(out)
}

/// The kinds of difference, as the summary counts them.
const COPIES_JOIN: &str = "songs joined because they share a copied file (meant)";
const COPY_OPENS: &str = "songs whose C# newest save is a copy Rust hides (meant)";
const OTHER_JOIN: &str = "songs joined for another reason";
const SPLIT: &str = "C# songs split into more than one Rust song";
const LATEST: &str = "songs that open a different save";
const ONLY_CS: &str = "saves only the C# app lists";
const ONLY_RUST: &str = "saves only Rust lists";

fn compare(cs: &HashMap<String, CsSave>, songs: &[Song], save_count: usize) -> Outcome {
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    let mut report = String::new();
    let note = |kinds: &mut BTreeMap<&str, usize>, kind: &'static str, song: &Song, why: &str, report: &mut String| {
        *kinds.entry(kind).or_default() += 1;
        let _ = writeln!(report, "{kind}: {} ({})\n  {why}\n", song.name, song.latest.path);
    };
    // Which Rust songs each C# song's shown saves went to. A hidden copy can sit in another song than the C# app
    // puts it in (a copy of a save split off on its own goes with that save): that's the copies, not a split.
    let mut rust_of: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, song) in songs.iter().enumerate() {
        for s in song.saves() {
            if let Some(c) = cs.get(&case::key(&s.path)) {
                let to = rust_of.entry(&c.key).or_default();
                if !to.contains(&i) {
                    to.push(i);
                }
            }
        }
    }
    let mut seen = 0;
    for song in songs {
        let all: Vec<&Save> = song.saves().chain(&song.copies).collect();
        let mut keys: Vec<&str> = Vec::new();
        for s in &all {
            seen += 1;
            match cs.get(&case::key(&s.path)) {
                Some(c) => {
                    if !keys.contains(&c.key.as_str()) {
                        keys.push(&c.key);
                    }
                }
                None => {
                    *kinds.entry(ONLY_RUST).or_default() += 1;
                    let _ = writeln!(report, "{ONLY_RUST}: {}\n", s.path);
                }
            }
        }
        if keys.len() > 1 {
            if linked_by_copies(&all, cs, &keys) {
                note(&mut kinds, COPIES_JOIN, song, &format!("{} C# songs", keys.len()), &mut report);
            } else {
                note(&mut kinds, OTHER_JOIN, song, &format!("C# songs: {}", keys.join(" | ")), &mut report);
            }
            continue;
        }
        // A C# song split across Rust songs is counted once, as a split, not again for the save each piece opens.
        if keys.first().is_some_and(|k| rust_of.get(k).is_some_and(|to| to.len() > 1)) {
            continue;
        }
        // One C# song: it should open the same save, unless C#'s newest is a copy Rust keeps aside.
        let cs_latest = all.iter().find(|s| cs.get(&case::key(&s.path)).is_some_and(|c| c.latest));
        match cs_latest {
            Some(l) if case::key(&l.path) == case::key(&song.latest.path) => {}
            Some(l) if song.copies.iter().any(|c| case::key(&c.path) == case::key(&l.path)) => {
                note(&mut kinds, COPY_OPENS, song, &format!("C# opens {}", l.path), &mut report);
            }
            Some(l) => note(&mut kinds, LATEST, song, &format!("C# opens {}", l.path), &mut report),
            None => {}
        }
    }
    for (key, to) in &rust_of {
        if to.len() > 1 {
            *kinds.entry(SPLIT).or_default() += 1;
            let names: Vec<&str> = to.iter().filter_map(|i| songs.get(*i)).map(|s| s.name.as_str()).collect();
            let _ = writeln!(report, "{SPLIT}: {key}\n  Rust songs: {}\n", names.join(" | "));
        }
    }
    let only_cs = cs.len().saturating_sub(seen - kinds.get(ONLY_RUST).copied().unwrap_or(0));
    if only_cs > 0 {
        kinds.insert(ONLY_CS, only_cs);
    }

    let cs_songs = {
        let mut k: Vec<&str> = cs.values().map(|c| c.key.as_str()).collect();
        k.sort_unstable();
        k.dedup();
        k.len()
    };
    let shown: usize = songs.iter().map(Song::save_count).sum();
    let hidden: usize = songs.iter().map(|s| s.copies.len()).sum();
    let mut summary = format!(
        "Songs twin test\n  C# app: {cs_songs} songs, {} saves\n  Rust:   {} songs, {shown} saves shown, {hidden} copies hidden ({save_count} in all)\n",
        cs.len(),
        songs.len()
    );
    let meant = [COPIES_JOIN, COPY_OPENS];
    let mut same = true;
    if kinds.is_empty() {
        summary += "  Every song is grouped the same way.\n";
    } else {
        summary += "  Differences:\n";
        for (kind, n) in &kinds {
            summary += &format!("    {n:>6}  {kind}\n");
            same &= meant.contains(kind);
        }
        if same {
            summary += "  Every difference comes from copied folders, as meant.\n";
        }
    }
    Outcome { summary, report, same }
}

/// Whether the C# songs `keys` are all joined to each other by saves that are the same file.
fn linked_by_copies(all: &[&Save], cs: &HashMap<String, CsSave>, keys: &[&str]) -> bool {
    fn root(parent: &[usize], mut i: usize) -> usize {
        while let Some(&p) = parent.get(i) {
            if p == i {
                break;
            }
            i = p;
        }
        i
    }
    // Each save's C# song and file, worked out once.
    let marks: Vec<_> = all
        .iter()
        .filter_map(|s| {
            let key = cs.get(&case::key(&s.path)).and_then(|c| keys.iter().position(|x| *x == c.key))?;
            Some((key, file_id(s)?))
        })
        .collect();
    let mut parent: Vec<usize> = (0..keys.len()).collect();
    let mut first_with: HashMap<_, usize> = HashMap::new();
    for (key, id) in &marks {
        match first_with.get(id) {
            Some(&other) => {
                let (rx, ry) = (root(&parent, *key), root(&parent, other));
                if let Some(p) = parent.get_mut(rx) {
                    *p = ry;
                }
            }
            None => {
                first_with.insert(id, *key);
            }
        }
    }
    let first = root(&parent, 0);
    (1..keys.len()).all(|i| root(&parent, i) == first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_library::SongChoices;

    fn save(id: i64, folder: &str, name: &str) -> Save {
        Save {
            id,
            path: format!("{folder}\\{name}.flp"),
            name: name.into(),
            daw: "FL Studio".into(),
            folder: folder.into(),
            size: 1000 + id,
            note_count: 40,
            track_count: 4,
            ..Save::default()
        }
    }

    fn cs_of(rows: &[(&Save, &str, bool)]) -> HashMap<String, CsSave> {
        rows.iter().map(|(s, k, l)| (case::key(&s.path), CsSave { key: (*k).into(), latest: *l })).collect()
    }

    #[test]
    fn a_copy_of_a_split_off_save_is_not_a_split() {
        let a = save(1, "C:\\Music\\Night", "Night_1");
        let x = save(2, "C:\\Music\\Night", "Night_2");
        // The same file copied to another drive.
        let copy =
            Save { id: 3, path: "D:\\Copy\\Night\\Night_2.flp".into(), folder: "D:\\Copy\\Night".into(), ..x.clone() };
        let choices = SongChoices::from_rows([("split", x.path.as_str(), "")]);
        let saves = vec![a.clone(), x.clone(), copy.clone()];
        let songs = group_songs(&saves, &choices);
        assert_eq!(songs.len(), 2);
        // The C# app keeps the copy in the first song.
        let cs = cs_of(&[(&a, "A", true), (&copy, "A", false), (&x, "S", true)]);
        let out = compare(&cs, &songs, saves.len());
        assert!(out.same, "{}", out.summary);
        assert!(!out.summary.contains(SPLIT), "{}", out.summary);
    }

    #[test]
    fn a_split_song_is_counted_once() {
        let a = save(1, "C:\\Music\\Night", "Night_1");
        let b = save(2, "C:\\Music\\Day", "Day_1");
        let songs = group_songs(&[a.clone(), b.clone()], &SongChoices::default());
        assert_eq!(songs.len(), 2);
        // The C# app has them as one song, opening b.
        let out = compare(&cs_of(&[(&a, "A", false), (&b, "A", true)]), &songs, 2);
        assert!(!out.same);
        assert!(out.summary.contains(SPLIT) && !out.summary.contains(LATEST), "{}", out.summary);
    }
}
