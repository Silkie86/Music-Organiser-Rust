//! The real files check: the Rust readers read every FL Studio, Bitwig and Studio One project and MIDI file on the PC,
//! and the check says which files they trip over, which they can't read, which are slow, and which now read
//! differently from the last check though the file itself hasn't changed. That last one is what a change to a reader
//! does to the real library, before it reaches the app. The C# app isn't needed: it's the twin test's other half,
//! without the twin.
//!
//! ```text
//! twin check <folder or file>... --state <file> [--report <file>] [--library <library.db>]
//! ```
//!
//! Three files, and only the summary (printed) leaves the PC:
//! - the summary: counts, file kinds and the parts of a reading that changed (as `song.channels`), nothing from the
//!   songs, so it's safe to paste into a conversation;
//! - `--state`: one line a file (its path, size, date and a short fingerprint of each part of its reading), kept for
//!   the next check to compare with; it names the files, so it stays on the PC;
//! - `--report`: the files behind each count, by path; it stays on the PC too.
//!
//! Each file is read whole once (rule 12), on all cores but one at low priority (rule 21). A panic in a reader is
//! caught (`mo_catch::catch`), counted and named in the report, and the check goes on.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

use mo_formats::midi::smf;
use mo_library::folders::FolderList;
use mo_library::songs::thousands;
use rayon::prelude::*;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::rust_side;

/// A reading over this long counts as slow.
const SLOW_MS: u64 = 1_000;
/// How many of the slowest files the report names.
const SLOWEST: usize = 50;
/// The top of a state file, so a later check can tell one it can't read.
const STATE_HEADER: &str = "# twin check state 1: path, size, modified (ms), reading ms, outcome, part=fingerprint...";

/// What the check found in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub path: String,
    pub size: u64,
    pub modified: u64,
    pub ms: u64,
    pub outcome: Outcome,
    /// A short fingerprint of each part of the reading ("info.notes", "song.channels", "midi"...).
    pub parts: BTreeMap<String, String>,
}

/// How a reading ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Read, with nothing to say.
    Read,
    /// Read, with warnings (a damaged save read on, rule 9).
    Warned,
    /// The readers said it can't be read.
    Unreadable,
    /// A reader panicked on it: a bug to fix.
    Panicked,
}

impl Outcome {
    fn word(self) -> &'static str {
        match self {
            Outcome::Read => "read",
            Outcome::Warned => "warned",
            Outcome::Unreadable => "unreadable",
            Outcome::Panicked => "panicked",
        }
    }

    fn from_word(w: &str) -> Option<Outcome> {
        Some(match w {
            "read" => Outcome::Read,
            "warned" => Outcome::Warned,
            "unreadable" => Outcome::Unreadable,
            "panicked" => Outcome::Panicked,
            _ => return None,
        })
    }
}

/// The parts of a reading and a fingerprint of each: every key of the reading, and each key one level in for the
/// keys that hold an object (`info.notes`, `song.channels`), so the summary can say which part changed. The file's
/// own path is left out: a reading doesn't change because the folder was renamed.
pub fn parts(reading: &Value) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(top) = reading.as_object() else {
        out.insert("reading".into(), fingerprint(reading));
        return out;
    };
    for (k, v) in top {
        if k == "file" {
            continue;
        }
        match v.as_object() {
            Some(inner) if !inner.is_empty() => {
                for (k2, v2) in inner {
                    out.insert(format!("{k}.{k2}"), fingerprint(v2));
                }
            }
            _ => {
                out.insert(k.clone(), fingerprint(v));
            }
        }
    }
    out
}

/// 8 bytes of SHA-256 of a value's JSON: enough to tell two readings apart, short enough for a state line.
fn fingerprint(v: &Value) -> String {
    Sha256::digest(v.to_string().as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// How a reading ended, from what the readers wrote.
pub fn outcome(reading: &Value) -> Outcome {
    let has = |v: Option<&Value>| v.is_some_and(|v| !v.is_null());
    if has(reading.get("error")) || has(reading.get("error_more")) || has(reading.pointer("/info/error")) {
        return Outcome::Unreadable;
    }
    match reading.pointer("/info/warnings") {
        Some(Value::Array(w)) if !w.is_empty() => Outcome::Warned,
        _ => Outcome::Read,
    }
}

/// Reads one file as the twin test does, catching a reader's panic.
fn read_one(path: &Path) -> (Reading, Option<String>) {
    let meta = std::fs::metadata(path).ok();
    let size = meta.as_ref().map_or(0, |m| m.len());
    let modified = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    let start = Instant::now();
    let result = mo_catch::catch(AssertUnwindSafe(|| rust_side::dump_with(path, false)));
    let ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    let path = path.to_string_lossy().into_owned();
    match result {
        Ok(reading) => {
            let reading = Reading { path, size, modified, ms, outcome: outcome(&reading), parts: parts(&reading) };
            (reading, None)
        }
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic with no message".into());
            let reading = Reading { path, size, modified, ms, outcome: Outcome::Panicked, parts: BTreeMap::new() };
            (reading, Some(message))
        }
    }
}

/// One state line: tabs between the fields, so a path with spaces stays whole.
pub fn state_line(r: &Reading) -> String {
    let parts: Vec<String> = r.parts.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("{}\t{}\t{}\t{}\t{}\t{}", r.path, r.size, r.modified, r.ms, r.outcome.word(), parts.join(" "))
}

/// The readings in a state file, by path. Lines it can't read are passed over.
pub fn read_state(text: &str) -> HashMap<String, Reading> {
    let mut out = HashMap::new();
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        let [path, size, modified, ms, outcome, parts] = f[..] else { continue };
        let (Ok(size), Ok(modified), Ok(ms), Some(outcome)) =
            (size.parse(), modified.parse(), ms.parse(), Outcome::from_word(outcome))
        else {
            continue;
        };
        let parts =
            parts.split(' ').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
        out.insert(path.to_owned(), Reading { path: path.to_owned(), size, modified, ms, outcome, parts });
    }
    out
}

/// The parts that read differently from the last check, for a file that hasn't changed on disk (same size and
/// date); `None` for a file that changed, whose reading may well differ. When a reader starts or stops panicking on a
/// file, only that is said: a panic has no parts to compare.
pub fn changed_parts(old: &Reading, new: &Reading) -> Option<BTreeSet<String>> {
    if old.size != new.size || old.modified != new.modified {
        return None;
    }
    let mut out = BTreeSet::new();
    if old.outcome != new.outcome {
        out.insert(format!("outcome {} to {}", old.outcome.word(), new.outcome.word()));
        if old.outcome == Outcome::Panicked || new.outcome == Outcome::Panicked {
            return Some(out);
        }
    }
    for k in old.parts.keys().chain(new.parts.keys()) {
        if old.parts.get(k) != new.parts.get(k) {
            out.insert(k.clone());
        }
    }
    Some(out)
}

/// A file's kind, for the summary.
fn kind(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    if lower.ends_with(".flp") {
        "FL Studio"
    } else if lower.ends_with(".bwproject") {
        "Bitwig"
    } else if lower.ends_with(".song") {
        "Studio One"
    } else if smf::is_midi_path(path) {
        "MIDI"
    } else {
        "other"
    }
}

/// A panic's words with its numbers and anything it quotes taken out, so the same bug in many files counts once and
/// nothing from a song (a length, an offset, a name it quotes) reaches the summary. Quoted text (in `'`, `"` or a
/// backtick) becomes `...`, a number `#`, and any letter outside plain ASCII is left out.
pub fn panic_kind(message: &str) -> String {
    let first = message.lines().next().unwrap_or("");
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut in_number = false;
    for c in first.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        if matches!(c, '\'' | '"' | '`') {
            quote = Some(c);
            out.push_str("...");
            in_number = false;
            continue;
        }
        if c.is_ascii_digit() {
            if !in_number {
                out.push('#');
            }
            in_number = true;
            continue;
        }
        in_number = false;
        if c.is_ascii() && !c.is_ascii_control() {
            out.push(c);
        }
    }
    out.chars().take(160).collect()
}

/// The options of `twin check`.
#[derive(Default)]
struct Options {
    state: Option<PathBuf>,
    report: Option<PathBuf>,
    library: Option<PathBuf>,
    inputs: Vec<PathBuf>,
}

pub const USAGE: &str = "twin check <folder or file>... --state <file> [--report <file>] [--library <library.db>]";

fn options(args: &[String]) -> Result<Options, String> {
    let mut o = Options::default();
    let mut args = args.iter();
    while let Some(a) = args.next() {
        let mut value = || args.next().map(PathBuf::from).ok_or_else(|| format!("{a} needs a value\n{USAGE}"));
        match a.as_str() {
            "--state" => o.state = Some(value()?),
            "--report" => o.report = Some(value()?),
            "--library" => o.library = Some(value()?),
            _ => o.inputs.push(PathBuf::from(a)),
        }
    }
    if o.state.is_none() || (o.inputs.is_empty() && o.library.is_none()) {
        return Err(USAGE.to_owned());
    }
    Ok(o)
}

/// Runs the check. `Ok(false)` when a reader panicked on a file: that's a bug to fix.
pub fn run(args: &[String]) -> Result<bool, String> {
    let mut o = options(args)?;
    if let Some(db) = &o.library {
        let lib = mo_library::Library::open_read_only(db)
            .map_err(|e| format!("Couldn't open {} to find the project folders: {e}", db.display()))?;
        let mut roots = lib.roots().map_err(|e| format!("Couldn't read the project folders: {e}"))?;
        // And the MIDI page's folders, so the MIDI packs are read too.
        roots.extend(
            lib.strict_list_setting(FolderList::Midi.setting_key().unwrap_or("midi_folders"))
                .map_err(|e| format!("Couldn't read the MIDI folders: {e}"))?,
        );
        o.inputs.extend(roots.into_iter().map(PathBuf::from).filter(|p| p.exists()));
    }
    let files = crate::collect(&o.inputs);
    if files.is_empty() {
        return Err(
            "No FL Studio, Bitwig or Studio One projects or MIDI files (.flp, .bwproject, .song, .mid) there.".into()
        );
    }
    let state_path = o.state.clone().ok_or(USAGE)?;
    let old = match std::fs::read_to_string(&state_path) {
        Ok(text) => read_state(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
        Err(e) => return Err(format!("Couldn't read the last check's state: {e}")),
    };

    // A panic is caught and counted: the default hook would print each one (with its file's path in what's around
    // it) over the summary.
    std::panic::set_hook(Box::new(|_| {}));
    let pool = crate::low_priority_pool()?;

    let mut readings: Vec<Reading> = Vec::with_capacity(files.len());
    let mut panics: Vec<(String, String)> = Vec::new();
    let mut done = 0;
    for chunk in files.chunks(crate::CHUNK) {
        eprint!("\r  {} of {} files read...", thousands(done), thousands(files.len()));
        done += chunk.len();
        let got: Vec<(Reading, Option<String>)> = pool.install(|| chunk.par_iter().map(|f| read_one(f)).collect());
        for (r, panic) in got {
            if let Some(message) = panic {
                panics.push((r.path.clone(), message));
            }
            readings.push(r);
        }
    }
    eprintln!("\r  {} of {} files read.   ", thousands(done), thousands(files.len()));
    let _ = std::panic::take_hook();

    let roots: Vec<&Path> = o.inputs.iter().map(PathBuf::as_path).collect();
    let summary = summarise(&readings, &old, &panics, &roots);
    print!("{}", summary.text);

    // The state is written by copy then rename, so a check stopped part way leaves the last one whole.
    let mut state = String::from(STATE_HEADER);
    state.push('\n');
    for r in &readings {
        state += &state_line(r);
        state.push('\n');
    }
    // Files this check didn't reach (one folder checked, a drive unplugged) keep their last reading, so the next
    // full check still compares them with it. Gone files go when their folder is checked and they aren't found.
    let reached: Vec<&Path> = o.inputs.iter().map(PathBuf::as_path).collect();
    let read_now: std::collections::HashSet<&str> = readings.iter().map(|r| r.path.as_str()).collect();
    for (path, r) in &old {
        if !read_now.contains(path.as_str()) && !reached.iter().any(|root| Path::new(path).starts_with(root)) {
            state += &state_line(r);
            state.push('\n');
        }
    }
    let part = state_path.with_extension("part");
    let written = std::fs::write(&part, state).and_then(|()| std::fs::rename(&part, &state_path));
    if let Err(e) = written {
        // Nothing of the check's own is left behind (rule 28).
        let _ = std::fs::remove_file(&part);
        return Err(format!("Couldn't write the state: {e}"));
    }

    if let Some(path) = &o.report {
        let mut f = std::io::BufWriter::new(
            std::fs::File::create(path).map_err(|e| format!("Couldn't write the report: {e}"))?,
        );
        f.write_all(summary.report.as_bytes()).map_err(|e| format!("Couldn't write the report: {e}"))?;
        f.flush().map_err(|e| format!("Couldn't write the report: {e}"))?;
        println!("The files behind each count: {}", path.display());
    }
    Ok(panics.is_empty())
}

/// The summary (safe to share) and the report (names files: stays on the PC).
pub struct Summary {
    pub text: String,
    pub report: String,
}

/// Counts the readings, and compares them with the last check's.
/// `roots` are the folders and files this check read: a file the last check read elsewhere isn't counted as gone.
pub fn summarise(
    readings: &[Reading],
    old: &HashMap<String, Reading>,
    panics: &[(String, String)],
    roots: &[&Path],
) -> Summary {
    let mut text = String::new();
    let mut report = String::new();
    let mut by_kind: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::new();
    for r in readings {
        *by_kind.entry(kind(&r.path)).or_default().entry(r.outcome.word()).or_default() += 1;
    }
    text += &format!("Real files check: {} file(s).\n", thousands(readings.len()));
    for (k, outcomes) in &by_kind {
        let total: usize = outcomes.values().sum();
        let rest: Vec<String> = ["warned", "unreadable", "panicked"]
            .iter()
            .filter_map(|w| outcomes.get(w).map(|n| format!("{n} {w}")))
            .collect();
        let rest = if rest.is_empty() { String::new() } else { format!(" ({})", rest.join(", ")) };
        text += &format!("  {k}: {}{rest}\n", thousands(total));
    }

    // Panics: each kind once, with how many files.
    if !panics.is_empty() {
        let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
        for (_, m) in panics {
            *kinds.entry(panic_kind(m)).or_default() += 1;
        }
        text += &format!("  A reader panicked on {} file(s), a bug each kind:\n", panics.len());
        for (k, n) in &kinds {
            text += &format!("  {n:>7}  {k}\n");
        }
        report += "Panicked\n";
        for (p, m) in panics {
            report += &format!("  {p}\n    {m}\n");
        }
        report += "\n";
    }
    for (word, outcome) in [("Unreadable", Outcome::Unreadable), ("Read with warnings", Outcome::Warned)] {
        let files: Vec<&Reading> = readings.iter().filter(|r| r.outcome == outcome).collect();
        if !files.is_empty() {
            report += &format!("{word}\n");
            for r in files {
                report += &format!("  {}\n", r.path);
            }
            report += "\n";
        }
    }

    // Slow files: the readers read each file whole once, so these are the files the app will be slow on too.
    let mut slow: Vec<&Reading> = readings.iter().collect();
    slow.sort_by_key(|r| std::cmp::Reverse(r.ms));
    let over = slow.iter().filter(|r| r.ms >= SLOW_MS).count();
    let slowest = slow.first().map_or(0, |r| r.ms);
    text += &format!("  {over} file(s) took over {} s to read; the slowest took {} ms.\n", SLOW_MS / 1000, slowest);
    report += "Slowest\n";
    for r in slow.iter().take(SLOWEST) {
        report += &format!("  {:>7} ms  {:>12} bytes  {}\n", r.ms, r.size, r.path);
    }
    report += "\n";

    // Against the last check.
    if old.is_empty() {
        text += "  No earlier check to compare with: the next check will say what reads differently.\n";
    } else {
        let mut same_file = 0;
        let mut changed: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        let mut files_changed = 0;
        for r in readings {
            let Some(parts) = old.get(&r.path).and_then(|o| changed_parts(o, r)) else { continue };
            same_file += 1;
            if !parts.is_empty() {
                files_changed += 1;
            }
            for p in parts {
                changed.entry(format!("{}: {p}", kind(&r.path))).or_default().push(&r.path);
            }
        }
        let new_files = readings.iter().filter(|r| !old.contains_key(&r.path)).count();
        let read_now: std::collections::HashSet<&str> = readings.iter().map(|r| r.path.as_str()).collect();
        let gone = old
            .keys()
            .filter(|p| {
                !read_now.contains(p.as_str()) && roots.iter().any(|root| Path::new(p.as_str()).starts_with(root))
            })
            .count();
        text += &format!(
            "  Since the last check: {} file(s) new, {} gone, {} changed on disk; of the {} unchanged, {} read \
             differently now.\n",
            thousands(new_files),
            thousands(gone),
            thousands(readings.len() - new_files - same_file),
            thousands(same_file),
            thousands(files_changed),
        );
        let mut by_count: Vec<(&String, &Vec<&str>)> = changed.iter().collect();
        by_count.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
        for (part, files) in &by_count {
            text += &format!("  {:>7}  {part}\n", files.len());
        }
        if !by_count.is_empty() {
            report += "Read differently from the last check (the files themselves unchanged)\n";
            for (part, files) in by_count {
                report += &format!("  {part}\n");
                for f in files {
                    report += &format!("    {f}\n");
                }
            }
        }
    }
    Summary { text, report }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reading(path: &str, outcome: Outcome, parts: &[(&str, &str)]) -> Reading {
        Reading {
            path: path.into(),
            size: 10,
            modified: 20,
            ms: 3,
            outcome,
            parts: parts.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
        }
    }

    #[test]
    fn parts_go_one_level_in_and_leave_the_path_out() {
        let r = json!({"file": "C:\\a.flp", "info": {"notes": 3, "bpm": 120}, "midi": [1, 2], "song": {}});
        let p = parts(&r);
        assert_eq!(p.keys().collect::<Vec<_>>(), ["info.bpm", "info.notes", "midi", "song"]);
        let moved = json!({"file": "D:\\b.flp", "info": {"notes": 3, "bpm": 120}, "midi": [1, 2], "song": {}});
        assert_eq!(parts(&moved), p);
        let other = json!({"file": "C:\\a.flp", "info": {"notes": 4, "bpm": 120}, "midi": [1, 2], "song": {}});
        let changed: Vec<_> = parts(&other).into_iter().filter(|(k, v)| p[k] != *v).map(|(k, _)| k).collect();
        assert_eq!(changed, ["info.notes"]);
    }

    #[test]
    fn outcomes_from_the_reading() {
        assert_eq!(outcome(&json!({"info": {"warnings": []}})), Outcome::Read);
        assert_eq!(outcome(&json!({"info": {"warnings": ["cut short"]}})), Outcome::Warned);
        assert_eq!(outcome(&json!({"info": {}, "error_more": "damaged"})), Outcome::Unreadable);
        assert_eq!(outcome(&json!({"error": "no such file"})), Outcome::Unreadable);
        assert_eq!(outcome(&json!({"info": {"error": "unreadable"}})), Outcome::Unreadable);
    }

    #[test]
    fn state_lines_read_back() {
        let r = reading("C:\\My Songs\\a b.flp", Outcome::Warned, &[("info.notes", "00ff"), ("midi", "1234")]);
        let text = format!("{STATE_HEADER}\n{}\nnot a line\n", state_line(&r));
        let back = read_state(&text);
        assert_eq!(back.len(), 1);
        assert_eq!(back[&r.path], r);
        let empty = reading("C:\\x.flp", Outcome::Panicked, &[]);
        assert_eq!(read_state(&state_line(&empty))[&empty.path], empty);
    }

    #[test]
    fn only_unchanged_files_count_as_reading_differently() {
        let old = reading("a.flp", Outcome::Read, &[("info.notes", "1"), ("midi", "2")]);
        let new = reading("a.flp", Outcome::Warned, &[("info.notes", "1"), ("midi", "3"), ("song", "4")]);
        assert_eq!(
            changed_parts(&old, &new).unwrap().into_iter().collect::<Vec<_>>(),
            ["midi", "outcome read to warned", "song"]
        );
        let panicked = reading("a.flp", Outcome::Panicked, &[]);
        assert_eq!(
            changed_parts(&old, &panicked).unwrap().into_iter().collect::<Vec<_>>(),
            ["outcome read to panicked"]
        );
        let saved_again = Reading { modified: 21, ..new };
        assert_eq!(changed_parts(&old, &saved_again), None);
    }

    #[test]
    fn panic_kinds_drop_what_they_quote() {
        assert_eq!(
            panic_kind("byte index 5 is not a char boundary; it is inside 'é' (bytes 4..6) of `Café Demo`"),
            "byte index # is not a char boundary; it is inside ... (bytes #..#) of ..."
        );
        assert_eq!(
            panic_kind("called `Result::unwrap()` on an `Err` value: Zip(\"Drums/Kick é.wav\")"),
            "called ... on an ... value: Zip(...)"
        );
        assert_eq!(panic_kind("Café"), "Caf");
    }

    #[test]
    fn the_usage_is_in_the_twins_own() {
        assert!(crate::USAGE.contains(USAGE));
    }

    #[test]
    fn panic_kinds_drop_numbers() {
        assert_eq!(
            panic_kind("index out of bounds: the len is 12 but the index is 400\nmore"),
            "index out of bounds: the len is # but the index is #"
        );
    }

    #[test]
    fn the_summary_names_no_file() {
        let old: HashMap<String, Reading> = [reading("C:\\Secret Song.flp", Outcome::Read, &[("info.notes", "1")])]
            .into_iter()
            .map(|r| (r.path.clone(), r))
            .collect();
        let now = [
            reading("C:\\Secret Song.flp", Outcome::Read, &[("info.notes", "2")]),
            reading("C:\\Other Secret.bwproject", Outcome::Panicked, &[]),
        ];
        let panics = [("C:\\Other Secret.bwproject".to_owned(), "slice index 99 out of range".to_owned())];
        let s = summarise(&now, &old, &panics, &[Path::new("C:\\")]);
        assert!(!s.text.contains("Secret"), "{}", s.text);
        assert!(s.text.contains("FL Studio: info.notes"), "{}", s.text);
        assert!(s.text.contains("slice index # out of range"), "{}", s.text);
        assert!(s.report.contains("Secret Song.flp"));
        assert!(s.report.contains("Other Secret.bwproject"));
    }
}
