//! The plugin's own log: what it did and how long it took, for looking into a problem afterwards.
//!
//! One file for each run of a DAW, in the app's logs folder beside the app's own logs:
//! `plugin-log-2026-11-11_14-30-05.txt`. Every copy of the plugin in that DAW writes to it. It's small and it's kept
//! in bounds (rule 28): never more than [`MOST_BYTES`], and only the newest ten are kept (the app's own tidying of its
//! logs folder knows them as one of its reports, `mo_app::tidy::REPORTS`, and the plugin runs that tidying as it
//! starts).
//!
//! Lines are written by a background thread at low priority, so nothing waits on the disk to write one. The thread
//! ends once it has written what's waiting (the next line starts another): the plugin keeps no thread of its own
//! going in the DAW, which may take the plugin out of memory at any time. Never called on the DAW's audio thread.
//! Until [`start`] is called (in tests, always) a line goes nowhere.

use crate::threads::lock;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How a plugin log's name starts: one of the reports the app's tidying keeps the newest ten of.
pub const START: &str = "plugin-log-";

/// The most one log holds: about ten thousand lines.
pub const MOST_BYTES: usize = 1024 * 1024;

const FULL: &str = "(the log is full: nothing more is written for this run of the DAW)\n";

/// Work that took this long or longer is worth a line however often it's done.
pub const SLOW: Duration = Duration::from_millis(100);

/// The most lines that wait to be written: more than any burst of them, and a disk that has stopped taking them
/// doesn't fill the memory.
const MOST_WAITING: usize = 2000;

/// The log, once started.
struct Log {
    folder: PathBuf,
    /// Lines still to be written.
    waiting: Mutex<Vec<String>>,
    /// Whether the writing thread is running.
    writing: AtomicBool,
    file: Mutex<LogFile>,
}

enum LogFile {
    NotMade,
    /// It couldn't be made (the folder can't be written): lines are dropped.
    None,
    Open(Capped<std::fs::File>),
}

static LOG: OnceLock<Log> = OnceLock::new();

/// Starts the log in the app's logs folder, once for the whole DAW: a later call does nothing. `header` is its first
/// line (the plugin's version and the DAW).
pub fn start(header: &str) {
    let mut first = false;
    LOG.get_or_init(|| {
        first = true;
        Log {
            folder: mo_app::paths::logs_dir(),
            waiting: Mutex::new(Vec::new()),
            writing: AtomicBool::new(false),
            file: Mutex::new(LogFile::NotMade),
        }
    });
    if first {
        line("Plugin", header);
    }
}

/// Adds a line: the time, `area` (what part of the plugin says it) and `message`.
pub fn line(area: &str, message: &str) {
    let Some(log) = LOG.get() else { return };
    let at = chrono::Local::now().format("%H:%M:%S%.3f");
    {
        let mut waiting = lock(&log.waiting);
        if waiting.len() >= MOST_WAITING {
            return;
        }
        waiting.push(format!("{at} [{area}] {}\n", one_line(message)));
    }
    if log.writing.swap(true, Ordering::AcqRel) {
        // The writing thread is running: it takes this line too.
        return;
    }
    crate::threads::background("mo-plugin-log", move || {
        // Marked as stopped however the thread ends, so the next line starts a new one.
        struct Stopped<'a>(&'a AtomicBool);
        impl Drop for Stopped<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _stopped = Stopped(&log.writing);
        loop {
            log.write_waiting();
            log.writing.store(false, Ordering::Release);
            // A line added just as it stopped would find it running and leave the line to it: so look once more, and
            // go on if there's one and no new thread has taken over.
            if lock(&log.waiting).is_empty() || log.writing.swap(true, Ordering::AcqRel) {
                break;
            }
        }
    });
}

impl Log {
    /// Writes the lines waiting, making the file first (and keeping the folder in bounds) the first time.
    fn write_waiting(&self) {
        let mut file = lock(&self.file);
        if matches!(*file, LogFile::NotMade) {
            *file = match create(&self.folder) {
                Some(made) => {
                    // The app's own tidying of its logs folder, as the app runs it when it starts. This log, just
                    // made, is among the newest it keeps.
                    let _ = mo_app::tidy::prune_logs(&self.folder, None, std::time::SystemTime::now());
                    LogFile::Open(Capped::new(made, MOST_BYTES))
                }
                None => LogFile::None,
            };
        }
        let lines = std::mem::take(&mut *lock(&self.waiting));
        if let LogFile::Open(log) = &mut *file {
            for line in &lines {
                log.line(line);
            }
        }
    }
}

/// Adds a line saying how long `what` took since `since`.
pub fn took(area: &str, what: &str, since: Instant) {
    line(area, &format!("{what} ({})", millis(since.elapsed())));
}

/// As [`took`], only when it took [`SLOW`] or longer: for work done often.
pub fn took_if_slow(area: &str, what: &str, since: Instant) {
    if since.elapsed() >= SLOW {
        took(area, what, since);
    }
}

fn millis(time: Duration) -> String {
    format!("{} ms", time.as_millis())
}

/// A message on one line: a line break in it (an error from Windows can have one) becomes a space.
fn one_line(message: &str) -> String {
    message.replace(['\r', '\n'], " ")
}

/// The name of the log started at `at`: [`START`], the time as the app writes it in a file's name, `.txt`.
fn name(at: chrono::NaiveDateTime) -> String {
    format!("{START}{}.txt", at.format("%Y-%m-%d_%H-%M-%S"))
}

/// A new log file in `folder`, named for now.
fn create(folder: &Path) -> Option<std::fs::File> {
    std::fs::create_dir_all(folder).ok()?;
    let path = folder.join(name(chrono::Local::now().naive_local()));
    let mut options = std::fs::OpenOptions::new();
    // Added to, should two DAWs start in the same second.
    options.append(true).create(true);
    // On Windows nobody can remove the log while this DAW is writing it (another DAW's plugin, or the app, keeping
    // the folder in bounds), as the app's own log does; anyone can still read it.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE, without FILE_SHARE_DELETE.
        options.share_mode(0x1 | 0x2);
    }
    options.open(&path).ok()
}

/// Writes lines until `most` bytes are written, then says the log is full, once, and writes no more.
struct Capped<W: Write> {
    out: W,
    left: usize,
    full: bool,
}

impl<W: Write> Capped<W> {
    fn new(out: W, most: usize) -> Capped<W> {
        Capped { out, left: most.saturating_sub(FULL.len()), full: false }
    }

    /// Writes `line` (ending in a line break) straight to the disk, so a DAW that crashes next still has it.
    fn line(&mut self, line: &str) {
        if self.full {
            return;
        }
        let line = if line.len() > self.left {
            self.full = true;
            FULL
        } else {
            self.left -= line.len();
            line
        };
        let _ = self.out.write_all(line.as_bytes());
        let _ = self.out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_stops_at_its_limit_and_says_so_once() {
        let mut log = Capped::new(Vec::new(), FULL.len() + 10);
        log.line("12345\n");
        log.line("123456\n");
        log.line("1\n");
        log.line("2\n");
        let text = String::from_utf8(log.out).expect("text");
        assert_eq!(text, format!("12345\n{FULL}"));
        assert!(text.len() <= FULL.len() + 10);
    }

    #[test]
    fn the_app_s_tidying_knows_a_plugin_log_by_its_name() {
        // The app keeps the newest ten of each report it knows: this log is one of them, named as they are (the
        // start, a time in digits, `-` and `_`, then `.txt`).
        assert!(mo_app::tidy::REPORTS.contains(&START));
        let at = chrono::NaiveDate::from_ymd_opt(2026, 11, 11).and_then(|d| d.and_hms_opt(14, 30, 5)).expect("a time");
        assert_eq!(name(at), "plugin-log-2026-11-11_14-30-05.txt");
    }

    #[test]
    fn lines_are_written_to_a_file_of_the_run_and_only_the_newest_logs_stay() {
        let folder = std::env::temp_dir().join(format!("mo-plugin-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("a folder");
        // Twelve logs of earlier runs, a note of the user's own and a copy of a log: only logs go, down to ten.
        for day in 10..22 {
            std::fs::write(folder.join(format!("plugin-log-2020-01-{day}_10-00-00.txt")), "old").expect("written");
        }
        std::fs::write(folder.join("my notes.txt"), "mine").expect("written");
        std::fs::write(folder.join("plugin-log-2020-01-10_10-00-00 - Copy.txt"), "mine").expect("written");
        let log = Log {
            folder: folder.clone(),
            waiting: Mutex::new(vec!["one\n".into(), "two\n".into()]),
            writing: AtomicBool::new(false),
            file: Mutex::new(LogFile::NotMade),
        };
        log.write_waiting();
        lock(&log.waiting).push("three\n".into());
        log.write_waiting();
        *lock(&log.file) = LogFile::None;
        let names: Vec<String> = std::fs::read_dir(&folder)
            .expect("the folder")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let logs: Vec<&String> = names.iter().filter(|n| n.starts_with(START) && !n.contains("Copy")).collect();
        assert_eq!(logs.len(), 10, "{names:?}");
        assert!(names.iter().any(|n| n == "my notes.txt") && names.iter().any(|n| n.contains("Copy")));
        let new = logs.iter().find(|n| !n.contains("2020")).expect("this run's log");
        assert_eq!(std::fs::read_to_string(folder.join(new)).expect("read"), "one\ntwo\nthree\n");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_message_stays_on_one_line_and_goes_nowhere_before_the_log_starts() {
        assert_eq!(one_line("Access is denied.\r\n(5)"), "Access is denied.  (5)");
        // Not started (tests never start it): nothing is written anywhere.
        line("Test", "nothing");
        assert!(LOG.get().is_none());
    }
}
