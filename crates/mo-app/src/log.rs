//! The session log: a plain-text diary of each run of the app in `logs\` in the app's data folder, the same as the
//! C# app's. It says which build ran, the pages opened, the jobs, every warning and error, and any `[Freeze]`, so
//! Claude can read what happened on Solomon's PC straight from the folder. Nothing of the music is in it, only what
//! the app did.
//!
//! Writing never waits on the disk: lines go to a writer thread of their own. A message that keeps coming
//! is written three times and then only counted (at 10, 100, 1000…), and a run stops writing at 5 MB, so a runaway
//! warning can't fill the disk. The newest 30 runs' logs are kept.
//!
//! A crash also gets a log of its own ([`write_crash_file`]): one for each place in the code that crashed, up to
//! [`CRASH_FILES_PER_RUN`] places a run, and always one for the crash that ends the app; the start-up sweep
//! ([`crate::tidy`]) keeps the newest 30 of those.

use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;

use chrono::{DateTime, Local};

/// This app's logs start with this. The C# app's are `session-…` and it prunes those by itself, so each app keeps
/// its own 30 and the launcher's many restarts never push out the C# app's logs (or a C# log still being written).
pub const PREFIX: &str = "rust-session-";
/// How many runs' logs are kept.
pub const KEPT: usize = 30;
/// A run that writes this much stops writing (and says so).
const MAX_BYTES: u64 = 5 * 1024 * 1024;
/// The same message more often than this is counted, not written again.
const REPEATS_WRITTEN: u32 = 3;
/// How many different messages are counted for repeats before the count starts again, so it can't grow all day.
const SEEN_MAX: usize = 2000;
/// How many recent lines are kept for an F12 note.
const RECENT: usize = 60;
/// How a crash's own log is named: `crash-<seconds since 1970>.log`, or `crash-<seconds>-2.log` for the next crash in
/// the same second. The C# app writes none of these, so the start-up sweep knows them as this app's.
pub const CRASH_PREFIX: &str = "crash-";
/// The most places in the code a run writes crash logs for, besides the crash that ends the app. A crash that keeps
/// happening (one bad file hit over and over on every background thread) gets one file, and still goes in the session
/// log each time, counted once it repeats, but can't fill the folder with files of its own.
pub const CRASH_FILES_PER_RUN: usize = 10;

/// The places in the code this run has written a crash log for ([`write_crash_file`]).
pub struct CrashFiles {
    places: Mutex<Vec<String>>,
}

impl Default for CrashFiles {
    fn default() -> Self {
        CrashFiles::new()
    }
}

impl CrashFiles {
    pub const fn new() -> CrashFiles {
        CrashFiles { places: Mutex::new(Vec::new()) }
    }

    /// Whether a crash at `place` gets a file: always when it ends the app (`fatal`); otherwise only the first crash at
    /// each place, and only while fewer than [`CRASH_FILES_PER_RUN`] places have one.
    fn take(&self, place: &str, fatal: bool) -> bool {
        if fatal {
            return true;
        }
        let mut places = self.places.lock().unwrap_or_else(|e| e.into_inner());
        if places.len() >= CRASH_FILES_PER_RUN || places.iter().any(|p| p == place) {
            return false;
        }
        places.push(place.to_owned());
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
    Error,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Level::Info => "Info",
            Level::Warning => "Warning",
            Level::Error => "Error",
        }
    }
}

enum Msg {
    Line(String),
    /// Write out everything so far, then say so on the channel.
    Flush(Sender<()>),
    Close,
}

struct State {
    seen: HashMap<String, u32>,
    recent: VecDeque<String>,
}

/// One run's log.
pub struct SessionLog {
    path: Option<PathBuf>,
    tx: Mutex<Option<Sender<Msg>>>,
    state: Mutex<State>,
    writer: Mutex<Option<JoinHandle<()>>>,
}

static CURRENT: OnceLock<SessionLog> = OnceLock::new();

/// Starts this run's log in `folder` (normally `paths::logs_dir()`). Call once, early in `main`; a second call returns
/// the log already started.
pub fn start(folder: &Path, header: &str) -> &'static SessionLog {
    CURRENT.get_or_init(|| SessionLog::new(folder, Local::now(), header))
}

/// Starts this run's log in the usual folder, with the usual header (version, build, system).
pub fn start_session() -> &'static SessionLog {
    start(&crate::paths::logs_dir(), &crate::version::log_header(Local::now()))
}

/// This run's log, if it's been started. Logging before then (or in tests) goes nowhere.
pub fn current() -> Option<&'static SessionLog> {
    CURRENT.get()
}

pub fn info(area: &str, message: &str) {
    if let Some(log) = current() {
        log.write(Level::Info, area, message);
    }
}

pub fn warning(area: &str, message: &str) {
    if let Some(log) = current() {
        log.write(Level::Warning, area, message);
    }
}

pub fn error(area: &str, message: &str) {
    if let Some(log) = current() {
        log.write(Level::Error, area, message);
    }
}

impl SessionLog {
    /// Opens a new log file for a run that started at `start`. If the folder can't be written, the log only keeps its
    /// recent lines in memory: no log is better than no app.
    pub fn new(folder: &Path, start: DateTime<Local>, header: &str) -> SessionLog {
        let file = create_file(folder, start);
        let path = file.as_ref().map(|(p, _)| p.clone());
        let (tx, writer) = match file {
            Some((_, f)) => {
                let (tx, rx) = channel();
                // At normal priority, unlike other background work: a crash waits for it to write the crash out.
                let handle = std::thread::Builder::new().name("Session log".into()).spawn(move || write_queued(f, rx));
                match handle {
                    Ok(h) => (Some(tx), Some(h)),
                    Err(_) => (None, None),
                }
            }
            None => (None, None),
        };
        prune(folder, KEPT, path.as_deref());
        let log = SessionLog {
            path,
            tx: Mutex::new(tx),
            state: Mutex::new(State { seen: HashMap::new(), recent: VecDeque::new() }),
            writer: Mutex::new(writer),
        };
        log.write(Level::Info, "App", header);
        log
    }

    /// This run's file, if it could be made.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn write(&self, level: Level, area: &str, message: &str) {
        // Trailing spaces only make the log wide.
        let message: String = message.lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
        let key = format!("{}\u{1}{area}\u{1}{message}", level.name());
        let line = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.seen.len() >= SEEN_MAX && !st.seen.contains_key(&key) {
                st.seen.clear();
            }
            let times = st.seen.entry(key).or_insert(0);
            *times = times.saturating_add(1);
            let times = *times;
            let message = if times > REPEATS_WRITTEN {
                if !is_power_of_ten(times) {
                    return;
                }
                format!("(said {times} times now) {}", message.lines().next().unwrap_or_default())
            } else {
                message
            };
            let line = format!("{} {:<7} [{area}] {message}", Local::now().format("%H:%M:%S%.3f"), level.name());
            st.recent.push_back(line.clone());
            while st.recent.len() > RECENT {
                st.recent.pop_front();
            }
            line
        };
        if let Some(tx) = self.tx.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = tx.send(Msg::Line(line));
        }
    }

    /// The last lines written, newest last, for a note to Claude.
    pub fn recent(&self) -> Vec<String> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).recent.iter().cloned().collect()
    }

    /// Waits until everything written so far is on disk. Used before the app closes, and by a crash.
    pub fn flush(&self) {
        let (done, wait) = channel();
        let sent = self
            .tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|tx| tx.send(Msg::Flush(done)).is_ok());
        if sent {
            let _ = wait.recv_timeout(std::time::Duration::from_secs(2));
        }
    }

    /// Writes "Closed" and stops the writer. Later lines are kept in memory only.
    pub fn close(&self) {
        self.write(Level::Info, "App", "Closed");
        if let Some(tx) = self.tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = tx.send(Msg::Close);
        }
        if let Some(h) = self.writer.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = h.join();
        }
    }
}

fn create_file(folder: &Path, start: DateTime<Local>) -> Option<(PathBuf, File)> {
    std::fs::create_dir_all(folder).ok()?;
    let stamp = start.format("%Y-%m-%d_%H-%M-%S");
    // Two runs started in the same second (tests, or a quick restart) each get their own file.
    for n in 1..100 {
        let name = if n == 1 { format!("{PREFIX}{stamp}.log") } else { format!("{PREFIX}{stamp}-{n}.log") };
        let path = folder.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(f) => return Some((path, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// The writer thread: writes lines as they come, flushing whenever it catches up.
fn write_queued(file: File, rx: Receiver<Msg>) {
    let mut out = BufWriter::new(file);
    let mut written: u64 = 0;
    let mut full = false;
    while let Ok(msg) = rx.recv() {
        let mut next = Some(msg);
        while let Some(msg) = next.take() {
            match msg {
                Msg::Line(line) if !full => {
                    written += line.len() as u64 + 1;
                    if written > MAX_BYTES {
                        full = true;
                        let _ = writeln!(out, "(the log is full at 5 MB; this run writes no more)");
                    } else {
                        let _ = writeln!(out, "{line}");
                    }
                }
                Msg::Line(_) => {}
                Msg::Flush(done) => {
                    let _ = out.flush();
                    let _ = done.send(());
                }
                Msg::Close => {
                    let _ = out.flush();
                    return;
                }
            }
            next = rx.try_recv().ok();
        }
        let _ = out.flush();
    }
    let _ = out.flush();
}

/// Deletes all but the newest `keep` of this app's session logs, newest by when they were last written, never
/// `just_made` (this run's), and only ones named exactly as [`create_file`] names them.
fn prune(folder: &Path, keep: usize, just_made: Option<&Path>) {
    crate::tidy::keep_newest(folder, is_session_log, keep, std::time::SystemTime::now(), just_made);
}

/// Whether `name` is a session log as [`create_file`] names them, and nothing else: `rust-session-`, the time
/// (`2026-11-02_14-30-05`), perhaps `-` and a number, then `.log`. A copy ("… - Copy.log") isn't one.
pub fn is_session_log(name: &str) -> bool {
    const STAMP: &[u8; 19] = b"0000-00-00_00-00-00";
    let Some(rest) = name.strip_prefix(PREFIX).and_then(|r| r.strip_suffix(".log")) else { return false };
    let (Some(stamp), Some(tail)) = (rest.get(..STAMP.len()), rest.get(STAMP.len()..)) else { return false };
    let stamp_fits = stamp.bytes().zip(STAMP).all(|(b, t)| if *t == b'0' { b.is_ascii_digit() } else { b == *t });
    let number = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    stamp_fits && (tail.is_empty() || tail.strip_prefix('-').is_some_and(number))
}

/// Writes a crash's own log into `folder` (`logs\`), in case the session log couldn't be written (rule 17), and says
/// where. `place` is where in the code it crashed (`file:line`) and `fatal` whether it ends the app (the window's
/// thread crashed). The crash that ends the app always gets a file; others get one for the first crash at each place,
/// for up to [`CRASH_FILES_PER_RUN`] places a run (`files` keeps count; the session log still has every crash). A
/// second crash in the same second gets a name of its own rather than writing over the first. Called from the crash
/// handler, so it never panics.
pub fn write_crash_file(
    folder: &Path,
    text: &str,
    secs: u64,
    place: &str,
    fatal: bool,
    files: &CrashFiles,
) -> Option<PathBuf> {
    if !files.take(place, fatal) {
        return None;
    }
    std::fs::create_dir_all(folder).ok()?;
    for n in 1..100 {
        let name = if n == 1 { format!("{CRASH_PREFIX}{secs}.log") } else { format!("{CRASH_PREFIX}{secs}-{n}.log") };
        let path = folder.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                let _ = f.write_all(text.as_bytes());
                return Some(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Whether `name` is a crash's own log as [`write_crash_file`] names them: `crash-`, a number, perhaps `-` and a
/// second number, then `.log`, and nothing else.
pub fn is_crash_log(name: &str) -> bool {
    let Some(stamp) = name.strip_prefix(CRASH_PREFIX).and_then(|r| r.strip_suffix(".log")) else { return false };
    let number = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match stamp.split_once('-') {
        Some((secs, n)) => number(secs) && number(n),
        None => number(stamp),
    }
}

fn is_power_of_ten(mut n: u32) -> bool {
    while n >= 10 && n.is_multiple_of(10) {
        n /= 10;
    }
    n == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-log-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn writes_header_lines_and_closed() {
        let dir = temp_folder("basic");
        let log = SessionLog::new(&dir, Local::now(), "Music Organiser started\n  Version: test");
        log.write(Level::Warning, "Songs", "Something odd   ");
        log.close();
        let text = std::fs::read_to_string(log.path().expect("a file")).expect("readable");
        assert!(text.contains("Info    [App] Music Organiser started\n  Version: test"));
        assert!(text.contains("Warning [Songs] Something odd\n"));
        assert!(text.trim_end().ends_with("[App] Closed"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeats_are_counted_not_written() {
        let dir = temp_folder("repeats");
        let log = SessionLog::new(&dir, Local::now(), "start");
        for _ in 0..100 {
            log.write(Level::Warning, "Grid", "A row drew late");
        }
        log.close();
        let text = std::fs::read_to_string(log.path().expect("a file")).expect("readable");
        assert_eq!(text.matches("] A row drew late").count(), 3);
        assert!(text.contains("(said 10 times now) A row drew late"));
        assert!(text.contains("(said 100 times now) A row drew late"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_second_runs_get_their_own_files_and_old_ones_go() {
        let dir = temp_folder("prune");
        let now = Local::now();
        let a = SessionLog::new(&dir, now, "a");
        let b = SessionLog::new(&dir, now, "b");
        assert_ne!(a.path(), b.path());
        a.close();
        b.close();
        let long_ago = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(946_684_800);
        for i in 0..40 {
            let old = dir.join(format!("{PREFIX}2000-01-01_00-00-{i:02}.log"));
            std::fs::write(&old, "old").expect("writable");
            let f = File::options().write(true).open(&old).expect("openable");
            f.set_modified(long_ago + std::time::Duration::from_secs(i)).expect("settable");
        }
        // The C# app's logs in the same folder are left alone.
        for i in 0..5 {
            std::fs::write(dir.join(format!("session-2000-01-01_00-00-{i:02}.log")), "c#").expect("writable");
        }
        // Nor are files that are only named like this app's logs: a copy, or one renamed by hand.
        let copies = [
            format!("{PREFIX}2000-01-01_00-00-00 - Copy.log"),
            format!("{PREFIX}2000-01-01_00-00-00 (2).log"),
            format!("{PREFIX}notes.log"),
            format!("{PREFIX}2000-01-01_00-00-00-2.log.bak"),
        ];
        for name in &copies {
            std::fs::write(dir.join(name), "yours").expect("writable");
            let f = File::options().write(true).open(dir.join(name)).expect("openable");
            f.set_modified(long_ago).expect("settable");
        }
        prune(&dir, KEPT, None);
        let left = std::fs::read_dir(&dir).expect("listable").count();
        assert_eq!(left, KEPT + 5 + copies.len());
        assert!(a.path().is_some_and(|p| p.exists()), "the newest logs stay");
        for name in &copies {
            assert!(dir.join(name).exists(), "{name} stays");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_log_just_made_always_stays_and_one_dated_after_now_goes_first() {
        let dir = temp_folder("future");
        let _ = std::fs::create_dir_all(&dir);
        let set = |name: &str, when: std::time::SystemTime| {
            std::fs::write(dir.join(name), "old").expect("writable");
            File::options().write(true).open(dir.join(name)).expect("openable").set_modified(when).expect("settable");
        };
        let now = std::time::SystemTime::now();
        // Others of this app's logs dated years after now (a clock that was wrong once): without care they'd hold
        // every place, and the log just made would be the oldest.
        for i in 0..KEPT + 3 {
            set(&format!("{PREFIX}2099-01-01_00-00-{i:02}.log"), now + std::time::Duration::from_secs(1_000_000_000));
        }
        set(&format!("{PREFIX}2001-01-01_00-00-00.log"), now - std::time::Duration::from_secs(1_000_000));
        // Started with a clock set years back, so its own time is the oldest of all.
        let start = DateTime::parse_from_rfc3339("1999-01-01T00:00:00+00:00").expect("a time").with_timezone(&Local);
        let log = SessionLog::new(&dir, start, "start");
        log.close();
        let mine = log.path().expect("a file").to_owned();
        assert!(mine.exists(), "the log just made stays");
        let left: Vec<String> = std::fs::read_dir(&dir)
            .expect("listable")
            .map(|e| e.expect("listed").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left.len(), KEPT);
        assert!(left.contains(&format!("{PREFIX}2001-01-01_00-00-00.log")), "the rightly dated one isn't pushed out");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_logs_are_known_by_their_whole_name() {
        assert!(is_session_log("rust-session-2026-11-02_14-30-05.log"));
        assert!(is_session_log("rust-session-2026-11-02_14-30-05-12.log"));
        for name in [
            "rust-session-2026-11-02_14-30-05 - Copy.log",
            "rust-session-2026-11-02_14-30-05-.log",
            "rust-session-2026-11-02_14-30-05-2-3.log",
            "rust-session-2026-11-02 14-30-05.log",
            "rust-session-2026-11-02_14-30.log",
            "rust-session-2026-11-02_14-30-05.log.txt",
            "session-2026-11-02_14-30-05.log",
            "rust-session-.log",
            "rust-session-２０２６-11-02_14-30-05.log",
        ] {
            assert!(!is_session_log(name), "{name}");
        }
    }

    #[test]
    fn recent_keeps_the_last_lines() {
        let dir = temp_folder("recent");
        let log = SessionLog::new(&dir, Local::now(), "start");
        for i in 0..100 {
            log.write(Level::Info, "Test", &format!("line {i}"));
        }
        let recent = log.recent();
        assert_eq!(recent.len(), RECENT);
        assert!(recent.last().is_some_and(|l| l.ends_with("line 99")));
        log.close();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crash_files_get_their_own_names_one_for_each_place() {
        let dir = temp_folder("crash");
        let files = CrashFiles::new();
        let first = write_crash_file(&dir, "first", 1_700_000_000, "src/a.rs:1", false, &files).expect("written");
        let second = write_crash_file(&dir, "second", 1_700_000_000, "src/b.rs:2", false, &files).expect("written");
        assert_eq!(first, dir.join("crash-1700000000.log"));
        assert_eq!(second, dir.join("crash-1700000000-2.log"));
        assert_eq!(std::fs::read_to_string(&first).expect("readable"), "first", "a second crash doesn't write over it");
        assert!(is_crash_log("crash-1700000000.log") && is_crash_log("crash-1700000000-2.log"));
        // The same place crashing again and again (a bad file on every thread) gets no more files…
        for n in 0..50 {
            assert_eq!(write_crash_file(&dir, "again", 1_700_000_010 + n, "src/a.rs:1", false, &files), None);
        }
        // …so other places still get theirs.
        assert!(write_crash_file(&dir, "third", 1_700_000_100, "src/c.rs:3", false, &files).is_some());
        assert_eq!(std::fs::read_dir(&dir).expect("listable").count(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_crash_that_ends_the_app_always_gets_its_file() {
        let dir = temp_folder("fatal");
        let files = CrashFiles::new();
        // Twelve crashes caught at twelve places (a decoder, a reader…) use up the run's files.
        let caught: Vec<Option<PathBuf>> = (0..12u64)
            .map(|n| write_crash_file(&dir, "caught", 1_700_000_000 + n, &format!("src/r{n}.rs:9"), false, &files))
            .collect();
        assert_eq!(caught.iter().filter(|f| f.is_some()).count(), CRASH_FILES_PER_RUN);
        let fatal = write_crash_file(&dir, "the end", 1_700_000_100, "src/main.rs:5", true, &files);
        let fatal = fatal.expect("the crash that ends the app is written");
        assert_eq!(std::fs::read_to_string(fatal).expect("readable"), "the end");
        assert_eq!(std::fs::read_dir(&dir).expect("listable").count(), CRASH_FILES_PER_RUN + 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_crash_logs_named_as_this_app_names_them_are_crash_logs() {
        for name in [
            "crash-.log",
            "crash-12.txt",
            "crash-12-.log",
            "crash-12-2-3.log",
            "crash-x.log",
            "my crash-12.log",
            "crash-12.log.old",
        ] {
            assert!(!is_crash_log(name), "{name}");
        }
    }

    #[test]
    fn powers_of_ten() {
        assert!(is_power_of_ten(10) && is_power_of_ten(100) && is_power_of_ten(1));
        assert!(!is_power_of_ten(4) && !is_power_of_ten(20) && !is_power_of_ten(101));
    }
}
