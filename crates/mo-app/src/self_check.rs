//! The self-check (Solomon, 2026-11-15: as much of beta testing automated as can be): the app reads its own logs and
//! looks at where it keeps its files, and marks the beta checklist's Self-check steps ([`crate::beta`]) by itself,
//! once a day and from its button on Settings › Developer. A step it finds wrong is a Problem with the lines that show
//! why, so Claude reads the story from the data folder; Solomon's own list is then only what needs his eyes, ears or a
//! DAW.
//!
//! It only reads: this app's session logs (`rust-session-….log`) and crash logs (`crash-….log`) in the logs folder
//! from the last [`DAYS`] days, a line at a time, at low priority, never on the window's thread.

use std::collections::VecDeque;
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
/// The words a freeze line carries when Windows had moved most of the app's memory to disk before it (written by
/// `mo_ui::stuck`): such a freeze still counts, but the note says how many were the computer short of memory.
pub const MOVED_TO_DISK: &str = "Windows had moved most of the app's memory to disk";
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
    /// Each run's time to the first frame, in seconds, and its line.
    pub starts: Vec<(f64, String)>,
    /// The newest error lines (with the log's name in front), and how many in all. Crash and freeze lines aren't
    /// counted: they have steps of their own.
    pub errors: VecDeque<String>,
    pub error_count: usize,
    /// Logs that couldn't be read.
    pub unread: Vec<String>,
}

/// Reads the logs in `dir` from `since` on, this run's too: by each line's own time (a log's name has the day it
/// started, its lines the time of day), so a run that started before `since` counts only its later lines.
pub fn read_logs(dir: &Path, since: SystemTime) -> io::Result<LogFacts> {
    let from = chrono::DateTime::<chrono::Local>::from(since).naive_local();
    let mut facts = LogFacts::default();
    let mut names: Vec<(String, SystemTime)> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        // Only the names the app gives its logs, and only plain files, never through a link (rule 28's care, though
        // this only reads).
        if !crate::log::is_session_log(&name) && !crate::log::is_crash_log(&name) {
            continue;
        }
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let changed = entry.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
        if changed < since {
            continue;
        }
        names.push((name, changed));
    }
    names.sort_by_key(|(_, t)| *t);
    for (name, _) in names {
        if crate::log::is_crash_log(&name) {
            facts.crash_files.push(name);
        } else {
            facts.runs += 1;
            match File::open(dir.join(&name)) {
                Ok(f) => read_one(&name, BufReader::new(f), from, &mut facts),
                Err(e) => facts.unread.push(format!("{name}: {e}")),
            }
        }
    }
    let skip = facts.crash_files.len().saturating_sub(KEPT);
    facts.crash_files.drain(..skip);
    Ok(facts)
}

/// When a session log started, from its name (`rust-session-2026-11-15_10-00-00.log`).
fn started(name: &str) -> Option<chrono::NaiveDateTime> {
    let stamp = name.strip_prefix(crate::log::PREFIX)?.get(..19)?;
    chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d_%H-%M-%S").ok()
}

/// Reads one session log's lines from `from` on (all of them when its name has no time).
fn read_one(name: &str, lines: impl BufRead, from: chrono::NaiveDateTime, facts: &mut LogFacts) {
    let mut day = started(name);
    let mut last_time: Option<chrono::NaiveTime> = None;
    // A "still stuck" freeze waits for its "was stuck" line, which says how long it really was: only when none follows
    // (the app was closed while stuck) does it count by itself.
    let mut stuck: Option<(u64, String)> = None;
    for line in lines.split(b'\n') {
        let Ok(line) = line else { break };
        let line = String::from_utf8_lossy(&line);
        let line = line.trim_end();
        let Some((time, level, area, message)) = split_line(line) else { continue };
        // The day moves on when the time of day goes back past midnight.
        if let (Some(d), Some(t)) = (day.as_mut(), chrono::NaiveTime::parse_from_str(time, "%H:%M:%S%.3f").ok()) {
            if last_time.is_some_and(|l| t < l) {
                *d += chrono::Duration::days(1);
            }
            last_time = Some(t);
            if d.date().and_time(t) < from {
                continue;
            }
        }
        let kept = || format!("{name}: {line}");
        match area {
            "Start" => {
                if let Some(secs) = start_secs(message) {
                    facts.starts.push((secs, kept()));
                }
            }
            "Freeze" => {
                if message.starts_with("The window is still stuck") {
                    if let Some(ms) = freeze_ms(message) {
                        stuck = Some((ms, kept()));
                    }
                } else if let Some(ms) = freeze_ms(message) {
                    stuck = None;
                    facts.freezes.push((ms, kept()));
                }
            }
            _ => {}
        }
        // Crashes and freezes have their own steps.
        if level == "Error" && area != "Crash" && area != "Freeze" {
            facts.error_count += 1;
            facts.errors.push_back(kept());
            if facts.errors.len() > KEPT {
                facts.errors.pop_front();
            }
        }
    }
    if let Some(s) = stuck {
        facts.freezes.push(s);
    }
}

/// A log line's time, level, area and message: `12:00:00.000 Warning [Area] message`.
fn split_line(line: &str) -> Option<(&str, &str, &str, &str)> {
    let (time, rest) = line.split_once(' ')?;
    let rest = rest.trim_start();
    let (level, rest) = rest.split_once(' ')?;
    let rest = rest.trim_start().strip_prefix('[')?;
    let (area, message) = rest.split_once("] ")?;
    Some((time, level, area, message))
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
                facts.crash_files.clone(),
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
            let paged = long.iter().filter(|(_, l)| l.contains(MOVED_TO_DISK)).count();
            let paged = match paged {
                0 => String::new(),
                n if n == long.len() => format!(" (all {n} while Windows had moved the app's memory to disk)"),
                n => format!(" ({n} of them while Windows had moved the app's memory to disk)"),
            };
            found(
                "auto.freezes",
                Mark::Problem,
                format!(
                    "{} freezes of a second or more in {runs} runs; the longest was {} ms{paged}",
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
                facts.errors.iter().cloned().collect(),
            )
        });
    }
    if !facts.starts.is_empty() {
        let mut s: Vec<f64> = facts.starts.iter().map(|(secs, _)| *secs).collect();
        s.sort_by(f64::total_cmp);
        let middle = s[s.len() / 2];
        let slowest = s[s.len() - 1];
        let note = format!(
            "{} starts: usually {middle:.1} s to the window, the slowest {slowest:.1} s (a Problem from {SLOW_START_SECS:.0} s)",
            s.len()
        );
        let slow: Vec<String> = facts
            .starts
            .iter()
            .rev()
            .filter(|(secs, _)| *secs > SLOW_START_SECS)
            .take(KEPT)
            .map(|(_, l)| l.clone())
            .collect();
        out.push(if middle <= SLOW_START_SECS {
            found("auto.start", Mark::Works, note, Vec::new())
        } else {
            found("auto.start", Mark::Problem, note, slow)
        });
    }
    let on_c = windows && data.to_string_lossy().get(..2).is_some_and(|d| d.eq_ignore_ascii_case("c:"));
    out.push(if on_c {
        found(
            "auto.data-folder",
            Mark::Problem,
            format!("The data folder is on C: ({}), the full drive", data.display()),
            vec![format!("Data folder: {}", data.display())],
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
10:04:02.000 Warning [Freeze] The window was stuck for 4100 ms (page MIDI; no jobs running; busy)
10:05:00.000 Error   [Crash] something broke
10:06:00.000 Error   [Freeze] The window is still stuck after 3.0 s (page Songs; no jobs running; busy)
";

    fn long_ago() -> chrono::NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2000, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0)).expect("a date")
    }

    #[test]
    fn reads_a_log() {
        let mut facts = LogFacts::default();
        read_one("rust-session-1.log", LOG.as_bytes(), long_ago(), &mut facts);
        assert_eq!(facts.starts.iter().map(|s| s.0).collect::<Vec<_>>(), [2.31]);
        // The 3 s "still stuck" counts as the 4100 ms it was; the last one, with no end, by itself.
        assert_eq!(facts.freezes.iter().map(|f| f.0).collect::<Vec<_>>(), [1520, 300, 4100, 3000]);
        assert_eq!(facts.error_count, 1, "crash and freeze lines have their own steps");
        assert!(facts.errors[0].starts_with("rust-session-1.log: 10:03"));
    }

    #[test]
    fn findings_mark_each_step() {
        let mut facts = LogFacts { runs: 2, ..LogFacts::default() };
        read_one("a.log", LOG.as_bytes(), long_ago(), &mut facts);
        let f = findings(&facts, Path::new("I:\\MusicOrganiser"), true);
        let by = |id: &str| f.iter().find(|x| x.id == id).map(|x| x.mark);
        assert_eq!(by("auto.crashes"), Some(Mark::Works));
        assert_eq!(by("auto.freezes"), Some(Mark::Problem));
        assert_eq!(by("auto.errors"), Some(Mark::Problem));
        assert_eq!(by("auto.start"), Some(Mark::Works));
        assert_eq!(by("auto.data-folder"), Some(Mark::Works));
        let freezes = f.iter().find(|x| x.id == "auto.freezes").expect("there");
        assert_eq!(freezes.lines.len(), 3);
        assert!(freezes.note.contains("4100 ms"), "{}", freezes.note);
        assert!(!freezes.note.contains("disk"), "{}", freezes.note);
        // A freeze while the app's memory was on disk still counts, and the note says so.
        let paged = format!(
            "10:07:00.000 Warning [Freeze] The window was stuck for 2000 ms (page Space; no jobs running; {MOVED_TO_DISK} before this)\n"
        );
        let mut more = LogFacts { runs: 2, ..LogFacts::default() };
        read_one("b.log", format!("{LOG}{paged}").as_bytes(), long_ago(), &mut more);
        let f = findings(&more, Path::new("I:\\MusicOrganiser"), true);
        let freezes = f.iter().find(|x| x.id == "auto.freezes").expect("there");
        assert_eq!(freezes.mark, Mark::Problem);
        // (The 10:06 "still stuck" ends with this line, so it is one freeze of 2000 ms, not two.)
        assert!(freezes.note.starts_with("3 freezes"), "{}", freezes.note);
        assert!(freezes.note.contains("(1 of them while Windows had moved"), "{}", freezes.note);
        let c = findings(&LogFacts::default(), Path::new("C:\\Users\\x\\AppData\\Local\\MusicOrganiser"), true);
        assert_eq!(c.len(), 1, "nothing to go on but the folder");
        assert_eq!(c[0].mark, Mark::Problem);
        let crashed = LogFacts { runs: 1, crash_files: vec!["crash-1.log".into()], ..LogFacts::default() };
        assert_eq!(findings(&crashed, Path::new("/d"), false)[0].mark, Mark::Problem);
    }

    #[test]
    fn every_finding_is_a_step_of_the_list() {
        let facts = LogFacts { runs: 1, starts: vec![(1.0, String::new())], ..LogFacts::default() };
        let steps = crate::beta::steps();
        for f in findings(&facts, Path::new("/d"), false) {
            assert!(steps.iter().any(|s| s.id == f.id), "{} isn't on the checklist", f.id);
        }
    }

    #[test]
    fn lines_before_the_window_are_left_out() {
        let name = "rust-session-2026-11-01_23-59-00.log";
        let text = "23:59:30.000 Error   [A] old\n00:00:30.000 Error   [A] new, the next day\n";
        let from = chrono::NaiveDate::from_ymd_opt(2026, 11, 2).and_then(|d| d.and_hms_opt(0, 0, 0)).expect("a date");
        let mut facts = LogFacts::default();
        read_one(name, text.as_bytes(), from, &mut facts);
        assert_eq!(facts.error_count, 1);
        assert!(facts.errors[0].contains("new"));
    }

    #[test]
    fn a_slow_start_keeps_its_lines() {
        let facts = LogFacts { runs: 1, starts: vec![(7.0, "slow".into())], ..LogFacts::default() };
        let f = findings(&facts, Path::new("/d"), false);
        let start = f.iter().find(|x| x.id == "auto.start").expect("there");
        assert_eq!((start.mark, start.lines.clone()), (Mark::Problem, vec!["slow".to_owned()]));
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
