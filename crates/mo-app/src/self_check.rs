//! The self-check (Solomon, 2026-11-15: as much of beta testing automated as can be): the app reads its own logs and
//! looks at where it keeps its files, and marks the beta checklist's Self-check steps ([`crate::beta`]) by itself,
//! once a day and from its button on Settings › Developer. A step it finds wrong is a Problem with the lines that show
//! why, so Claude reads the story from the data folder; Solomon's own list is then only what needs his eyes, ears or a
//! DAW.
//!
//! It only reads: this app's session logs (`rust-session-….log`) and crash logs (`crash-….log`) in the logs folder
//! from the last [`DAYS`] days, a line at a time, at low priority, never on the window's thread.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::beta::Mark;

/// How far back it looks.
pub const DAYS: u64 = 7;
/// A start slower than this (to the first frame) is a Problem.
pub const SLOW_START_SECS: f64 = 5.0;
/// A freeze at least this long is a Problem.
pub const FREEZE_MS: u64 = 1000;
/// The most lines kept for one step's Problem.
const KEPT: usize = 20;

/// What the logs say.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LogFacts {
    /// The app's runs (session logs) looked at.
    pub runs: usize,
    /// Crash logs' names.
    pub crash_files: Vec<String>,
    /// Each freeze: how long (ms) and its line.
    pub freezes: Vec<(u64, String)>,
    /// Each run's time to the first frame, in seconds.
    pub starts: Vec<f64>,
    /// Error lines (with the log's name in front), and how many in all.
    pub errors: Vec<String>,
    pub error_count: usize,
    /// Logs that couldn't be read.
    pub unread: Vec<String>,
}

/// Reads the logs in `dir` changed since `since`. The log of this run (`skip`, if any) is read too: it's the newest.
pub fn read_logs(dir: &Path, since: SystemTime) -> io::Result<LogFacts> {
    let mut facts = LogFacts::default();
    let mut names: Vec<(String, SystemTime)> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        // Never through a link, and only plain files (rule 28's care, though this only reads).
        if !meta.is_file() || entry.file_type().map(|t| t.is_symlink()).unwrap_or(true) {
            continue;
        }
        let changed = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if changed < since {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        names.push((name, changed));
    }
    names.sort_by_key(|(_, t)| *t);
    for (name, _) in names {
        if crate::log::is_crash_log(&name) {
            facts.crash_files.push(name);
        } else if crate::log::is_session_log(&name) {
            facts.runs += 1;
            match File::open(dir.join(&name)) {
                Ok(f) => read_one(&name, BufReader::new(f), &mut facts),
                Err(e) => facts.unread.push(format!("{name}: {e}")),
            }
        }
    }
    Ok(facts)
}

fn read_one(name: &str, lines: impl BufRead, facts: &mut LogFacts) {
    for line in lines.split(b'\n') {
        let Ok(line) = line else { break };
        let line = String::from_utf8_lossy(&line);
        let line = line.trim_end();
        let Some((level, area, message)) = split_line(line) else { continue };
        match area {
            "Start" => {
                if let Some(secs) = start_secs(message) {
                    facts.starts.push(secs);
                }
            }
            "Freeze" => {
                if let Some(ms) = freeze_ms(message) {
                    facts.freezes.push((ms, format!("{name}: {line}")));
                }
            }
            _ => {}
        }
        if level == "Error" {
            facts.error_count += 1;
            if facts.errors.len() < KEPT {
                facts.errors.push(format!("{name}: {line}"));
            }
        }
    }
}

/// A log line's level, area and message: `12:00:00.000 Warning [Area] message`.
fn split_line(line: &str) -> Option<(&str, &str, &str)> {
    let (_, rest) = line.split_once(' ')?;
    let rest = rest.trim_start();
    let (level, rest) = rest.split_once(' ')?;
    let rest = rest.trim_start().strip_prefix('[')?;
    let (area, message) = rest.split_once("] ")?;
    Some((level, area, message))
}

/// "Window drawn 2.31 s after the app started: …" → 2.31.
fn start_secs(message: &str) -> Option<f64> {
    let rest = message.strip_prefix("Window drawn ")?;
    rest.split_once(" s after")?.0.trim().parse().ok()
}

/// "The window was stuck for 1520 ms (…)" → 1520; "The window is still stuck after 3.0 s (…)" → 3000.
fn freeze_ms(message: &str) -> Option<u64> {
    if let Some(rest) = message.strip_prefix("The window was stuck for ") {
        return rest.split_once(" ms")?.0.trim().parse().ok();
    }
    let rest = message.strip_prefix("The window is still stuck after ")?;
    let secs: f64 = rest.split_once(" s")?.0.trim().parse().ok()?;
    Some((secs * 1000.0).round() as u64)
}

/// One Self-check step's result: its name in the checklist, the mark, what it found, and the lines that show it.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub id: &'static str,
    pub mark: Mark,
    pub note: String,
    pub lines: Vec<String>,
}

/// What the facts mean for the checklist's Self-check steps. A step with nothing to go on (no runs yet) isn't marked.
/// `data` is the data folder; `windows` whether on Windows (where C: is the full drive, Solomon's PC 2026-11).
pub fn findings(facts: &LogFacts, data: &Path, windows: bool) -> Vec<Found> {
    let mut out = Vec::new();
    let days = format!("the last {DAYS} days");
    let runs = facts.runs;
    let found = |id, mark, note: String, lines: Vec<String>| Found { id, mark, note, lines };

    if runs > 0 || !facts.crash_files.is_empty() {
        out.push(if facts.crash_files.is_empty() {
            found("auto.crashes", Mark::Works, format!("No crashes in {runs} runs in {days}"), Vec::new())
        } else {
            found(
                "auto.crashes",
                Mark::Problem,
                format!("{} crash logs in {days} ({runs} runs)", facts.crash_files.len()),
                facts.crash_files.iter().take(KEPT).cloned().collect(),
            )
        });
    }
    if runs > 0 {
        let long: Vec<&(u64, String)> = facts.freezes.iter().filter(|(ms, _)| *ms >= FREEZE_MS).collect();
        let longest = facts.freezes.iter().map(|(ms, _)| *ms).max();
        out.push(if long.is_empty() {
            let note = match longest {
                Some(ms) => format!("No freeze of a second or more in {runs} runs; the longest was {ms} ms"),
                None => format!("No freezes in {runs} runs"),
            };
            found("auto.freezes", Mark::Works, note, Vec::new())
        } else {
            found(
                "auto.freezes",
                Mark::Problem,
                format!(
                    "{} freezes of a second or more in {runs} runs; the longest was {} ms",
                    long.len(),
                    longest.unwrap_or(0)
                ),
                long.iter().rev().take(KEPT).map(|(_, l)| l.clone()).collect(),
            )
        });
        out.push(if facts.error_count == 0 {
            found("auto.errors", Mark::Works, format!("No errors in {runs} runs in {days}"), Vec::new())
        } else {
            found(
                "auto.errors",
                Mark::Problem,
                format!("{} error lines in {runs} runs in {days}", facts.error_count),
                facts.errors.clone(),
            )
        });
    }
    if !facts.starts.is_empty() {
        let mut s = facts.starts.clone();
        s.sort_by(f64::total_cmp);
        let middle = s[s.len() / 2];
        let slowest = s[s.len() - 1];
        let note = format!(
            "{} starts: usually {middle:.1} s to the window, the slowest {slowest:.1} s (a Problem from {SLOW_START_SECS:.0} s)",
            s.len()
        );
        out.push(found(
            "auto.start",
            if middle <= SLOW_START_SECS { Mark::Works } else { Mark::Problem },
            note,
            Vec::new(),
        ));
    }
    let on_c = windows && data.to_string_lossy().get(..2).is_some_and(|d| d.eq_ignore_ascii_case("c:"));
    out.push(if on_c {
        found(
            "auto.data-folder",
            Mark::Problem,
            format!("The data folder is on C: ({}), the full drive", data.display()),
            Vec::new(),
        )
    } else {
        found("auto.data-folder", Mark::Works, format!("The data folder is {}", data.display()), Vec::new())
    });
    out
}

/// When the self-check should look back to: [`DAYS`] before `now`.
pub fn since(now: SystemTime) -> SystemTime {
    now.checked_sub(Duration::from_secs(DAYS * 24 * 3600)).unwrap_or(SystemTime::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\
10:00:00.000 Info    [App] Music Organiser 0.1.0 started
10:00:02.310 Info    [Start] Window drawn 2.31 s after the app started: settings 10 ms, first frame 40 ms
10:01:00.000 Warning [Freeze] The window was stuck for 1520 ms (page Sounds; no jobs running; busy)
10:02:00.000 Warning [Freeze] The window was stuck for 300 ms (page Songs; no jobs running; busy)
10:03:00.000 Error   [Settings] Couldn't save rust-settings.json: denied
10:04:00.000 Error   [Freeze] The window is still stuck after 3.0 s (page MIDI; no jobs running; busy)
";

    #[test]
    fn reads_a_log() {
        let mut facts = LogFacts::default();
        read_one("rust-session-1.log", LOG.as_bytes(), &mut facts);
        assert_eq!(facts.starts, [2.31]);
        assert_eq!(facts.freezes.iter().map(|f| f.0).collect::<Vec<_>>(), [1520, 300, 3000]);
        assert_eq!(facts.error_count, 2);
        assert!(facts.errors[0].starts_with("rust-session-1.log: 10:03"));
    }

    #[test]
    fn findings_mark_each_step() {
        let mut facts = LogFacts { runs: 2, ..LogFacts::default() };
        read_one("a.log", LOG.as_bytes(), &mut facts);
        let f = findings(&facts, Path::new("I:\\MusicOrganiser"), true);
        let by = |id: &str| f.iter().find(|x| x.id == id).map(|x| x.mark);
        assert_eq!(by("auto.crashes"), Some(Mark::Works));
        assert_eq!(by("auto.freezes"), Some(Mark::Problem));
        assert_eq!(by("auto.errors"), Some(Mark::Problem));
        assert_eq!(by("auto.start"), Some(Mark::Works));
        assert_eq!(by("auto.data-folder"), Some(Mark::Works));
        let freezes = f.iter().find(|x| x.id == "auto.freezes").expect("there");
        assert_eq!(freezes.lines.len(), 2);
        assert!(freezes.note.contains("3000 ms"), "{}", freezes.note);
        let c = findings(&LogFacts::default(), Path::new("C:\\Users\\x\\AppData\\Local\\MusicOrganiser"), true);
        assert_eq!(c.len(), 1, "nothing to go on but the folder");
        assert_eq!(c[0].mark, Mark::Problem);
        let crashed = LogFacts { runs: 1, crash_files: vec!["crash-1.log".into()], ..LogFacts::default() };
        assert_eq!(findings(&crashed, Path::new("/d"), false)[0].mark, Mark::Problem);
    }

    #[test]
    fn every_finding_is_a_step_of_the_list() {
        let facts = LogFacts { runs: 1, starts: vec![1.0], ..LogFacts::default() };
        let steps = crate::beta::steps();
        for f in findings(&facts, Path::new("/d"), false) {
            assert!(steps.iter().any(|s| s.id == f.id), "{} isn't on the checklist", f.id);
        }
    }

    #[test]
    fn reads_the_folder() {
        let dir = std::env::temp_dir().join(format!("mo-self-check-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("made");
        fs::write(dir.join("rust-session-2026-11-15_10-00-00.log"), LOG).expect("log");
        fs::write(dir.join("crash-1763200000.log"), "boom").expect("crash");
        fs::write(dir.join("notes.txt"), "not a log").expect("other");
        let facts = read_logs(&dir, since(SystemTime::now())).expect("read");
        assert_eq!((facts.runs, facts.crash_files.len()), (1, 1), "{facts:?}");
        let none = read_logs(&dir, SystemTime::now() + Duration::from_secs(3600)).expect("read");
        assert_eq!(none.runs, 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
