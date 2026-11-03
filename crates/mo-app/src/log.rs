//! The session log: a plain-text diary of each run of the app in `logs\` in the app's data folder, the same as the
//! C# app's. It says which build ran, the pages opened, the jobs, every warning and error, and any `[Freeze]`, so
//! Claude can read what happened on Solomon's PC straight from the folder. Nothing of the music is in it, only what
//! the app did.
//!
//! Writing never waits on the disk: lines go to a writer thread of their own. A message that keeps coming
//! is written three times and then only counted (at 10, 100, 1000…), and a run stops writing at 5 MB, so a runaway
//! warning can't fill the disk; only a crash is still written after that, up to 1 MB more (rule 17: crashes always
//! log). A crash is counted by where in the code it happened, whichever thread it was on. At each start, a
//! low-priority thread keeps the folder in bounds ([`crate::tidy::prune_logs`]): the newest 30 runs' logs, the newest
//! 30 crash logs and each tool's newest 10 reports.
//!
//! A crash also gets a log of its own ([`write_crash_file`]): one for each place in the code that crashed, up to
//! [`CRASH_FILES_PER_RUN`] places a run, and always one for the crash that ends the app.
//!
//! The crash handler uses this log too, so nothing that can crash runs while the log's lock is held: a crash in
//! there would leave the lock taken, and every later line, the crash's own too, would wait for it for ever.

use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError, TryLockError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local};

/// This app's logs start with this. The C# app's are `session-…` and it prunes those by itself, so each app keeps
/// its own 30 and the launcher's many restarts never push out the C# app's logs (or a C# log still being written).
pub const PREFIX: &str = "rust-session-";
/// How many runs' logs are kept.
pub const KEPT: usize = 30;
/// A run that writes this much stops writing (and says so).
const MAX_BYTES: u64 = 5 * 1024 * 1024;
/// How much more a run writes for crashes, once it has stopped writing everything else: a crash always reaches the
/// log (rule 17), but a crash that keeps happening can't fill the disk either.
const CRASH_ROOM: u64 = 1024 * 1024;
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
/// happening (one bad file hit over and over on every background thread) gets one file, and goes in the session log
/// three times and is then counted by its place, so it can't fill the folder with files of its own, nor the log.
pub const CRASH_FILES_PER_RUN: usize = 10;
/// The name of the thread that writes the log. A crash there can't wait for the log to be written: it's the one
/// writing it.
pub const WRITER_THREAD: &str = "Session log";
/// How long the app waits as it closes for the logs folder to be kept in bounds, and then for the last lines to be
/// written: a folder on a drive that's asleep mustn't hold it open (the launcher stops it after 10 seconds).
const PRUNE_WAIT: Duration = Duration::from_secs(2);
const WRITER_WAIT: Duration = Duration::from_secs(5);
/// What std says of a crash it can't recover from (a second crash while one is being cleaned up, or one inside code
/// that mustn't crash): it ends the app straight after, on whichever thread it was (checked on Rust 1.99).
const ENDS_THE_APP: [&str; 2] = ["panic in a destructor during cleanup", "panic in a function that cannot unwind"];

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
    /// each place, and only while fewer than [`CRASH_FILES_PER_RUN`] places have one. A place is taken here, in one
    /// go, so threads crashing at one place at the same moment get one file between them, not one each.
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

    /// The file for a crash at `place` couldn't be written (a full disk): the place is free again for the next crash.
    fn give_back(&self, place: &str) {
        self.places.lock().unwrap_or_else(|e| e.into_inner()).retain(|p| p != place);
    }
}

/// Whether a crash ends the app: one on the window's thread (`main`), or one std can't recover from, on any thread
/// (`message` is what the crash says). `thread` is the crashing thread's name.
pub fn crash_ends_the_app(thread: Option<&str>, message: Option<&str>) -> bool {
    thread == Some("main") || message.is_some_and(|m| ENDS_THE_APP.contains(&m))
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
    /// A crash: written even once the log has stopped writing other lines, up to [`CRASH_ROOM`] more.
    Crash(String),
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
    /// Keeping the logs folder in bounds, off the window's thread; [`SessionLog::close`] waits a little for it.
    pruning: Mutex<Option<JoinHandle<()>>>,
    /// How far the computer's clock is from UTC, in seconds, as the last line found it: a crash's line tells its time
    /// with it, without asking the computer for its time zone (see [`time_of_day`]).
    offset: AtomicI32,
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

/// A crash at `place` in the code (`file:line`), as an error in the "Crash" area: written even once this run's log has
/// stopped writing other lines (rule 17), up to [`CRASH_ROOM`] more. Crashes are counted by their place: the same
/// bug hit on every background thread has another thread's name and backtrace each time, but one place. Called from
/// the crash handler, so it never waits long and never crashes itself.
pub fn crash(place: &str, text: &str) {
    if let Some(log) = current() {
        log.put(Level::Error, "Crash", text, Some(place));
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
                let handle = std::thread::Builder::new().name(WRITER_THREAD.into()).spawn(move || write_queued(f, rx));
                match handle {
                    Ok(h) => (Some(tx), Some(h)),
                    Err(_) => (None, None),
                }
            }
            None => (None, None),
        };
        // The folder is kept in bounds at every start, so an app that crashes soon after it opens (a crash loop)
        // still keeps it so, on a low-priority thread of its own: the window never waits on the disk for it (rules
        // 12 and 21).
        let pruning = {
            let (folder, just_made) = (folder.to_path_buf(), path.clone());
            crate::background::spawn("Prune logs", move || {
                crate::tidy::prune_logs(&folder, just_made.as_deref(), std::time::SystemTime::now());
            })
            .ok()
        };
        let log = SessionLog {
            path,
            tx: Mutex::new(tx),
            state: Mutex::new(State { seen: HashMap::new(), recent: VecDeque::new() }),
            writer: Mutex::new(writer),
            pruning: Mutex::new(pruning),
            offset: AtomicI32::new(start.offset().local_minus_utc()),
        };
        log.write(Level::Info, "App", header);
        log
    }

    /// This run's file, if it could be made.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn write(&self, level: Level, area: &str, message: &str) {
        self.put(level, area, message, None);
    }

    /// Writes a line. `crash` is where in the code a crash happened (see [`crash`]): its line is written even once the
    /// log has stopped writing other lines, it's counted by its place, and it never waits long for the log's lock.
    fn put(&self, level: Level, area: &str, message: &str, crash: Option<&str>) {
        // Trailing spaces only make the log wide.
        let message: String = message.lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
        // The time and the key the line is counted by are made before the lock is taken: nothing that could crash
        // runs while it's held (see the top of this file).
        let time = match crash {
            // Asking the computer for its time zone can fail, and a failure in the crash handler would end the app
            // at once: a crash's line uses the zone the last line found.
            Some(_) => time_of_day(SystemTime::now(), self.offset.load(Ordering::Relaxed)),
            None => {
                let now = Local::now();
                self.offset.store(now.offset().local_minus_utc(), Ordering::Relaxed);
                now.format("%H:%M:%S%.3f").to_string()
            }
        };
        let key = match crash {
            Some(place) => format!("Crash\u{1}{place}"),
            None => format!("{}\u{1}{area}\u{1}{message}", level.name()),
        };
        let whole = |message: &str| format!("{time} {:<7} [{area}] {message}", level.name());
        let line = match self.state_for(crash.is_some()) {
            Some(mut st) => {
                if st.seen.len() >= SEEN_MAX && !st.seen.contains_key(&key) {
                    st.seen.clear();
                }
                let times = st.seen.entry(key).or_insert(0);
                *times = times.saturating_add(1);
                let times = *times;
                let line = if times > REPEATS_WRITTEN {
                    if !is_power_of_ten(times) {
                        return;
                    }
                    let first = message.lines().next().unwrap_or_default();
                    match crash {
                        Some(place) => whole(&format!("(crashed at {place} {times} times now) {first}")),
                        None => whole(&format!("(said {times} times now) {first}")),
                    }
                } else {
                    whole(&message)
                };
                st.recent.push_back(line.clone());
                while st.recent.len() > RECENT {
                    st.recent.pop_front();
                }
                line
            }
            // Only a crash gets here, when the lock stays busy: its line is written, just not counted.
            None => whole(&message),
        };
        // This lock is only ever held to hand a line over (or take the writer away as the log closes), which can't
        // crash or wait, so even a crash's line waits for it rather than be lost.
        if let Some(tx) = self.tx.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            let _ = tx.send(if crash.is_some() { Msg::Crash(line) } else { Msg::Line(line) });
        }
    }

    /// The counts and recent lines. A crash waits only a moment for them (see [`lock_briefly`]).
    fn state_for(&self, crash: bool) -> Option<MutexGuard<'_, State>> {
        if crash { lock_briefly(&self.state) } else { Some(self.state.lock().unwrap_or_else(PoisonError::into_inner)) }
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

    /// Writes "Closed" and stops the writer. Later lines are kept in memory only. It waits a little for the logs
    /// folder to be kept in bounds first, so a crash doing that still reaches this log, and then for the last lines
    /// to be written; never long, as a folder on a drive that's asleep mustn't hold the app open.
    pub fn close(&self) {
        self.wait_for_pruning(PRUNE_WAIT);
        self.write(Level::Info, "App", "Closed");
        if let Some(tx) = self.tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = tx.send(Msg::Close);
        }
        if let Some(h) = self.writer.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = wait_for(h, WRITER_WAIT);
        }
    }

    /// Waits up to `limit` for the start's look at the logs folder to finish; says whether it has.
    pub(crate) fn wait_for_pruning(&self, limit: Duration) -> bool {
        let mut pruning = self.pruning.lock().unwrap_or_else(|e| e.into_inner());
        match pruning.take() {
            None => true,
            Some(h) => match wait_for(h, limit) {
                Ok(()) => true,
                // Still going: it's put back, so a later wait (or the app's end) still finds it.
                Err(h) => {
                    *pruning = Some(h);
                    false
                }
            },
        }
    }
}

/// Waits up to `limit` for a thread to finish. Gives it back if it hasn't.
fn wait_for(thread: JoinHandle<()>, limit: Duration) -> Result<(), JoinHandle<()>> {
    let until = Instant::now() + limit;
    while !thread.is_finished() {
        if Instant::now() >= until {
            return Err(thread);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = thread.join();
    Ok(())
}

/// Takes a lock for the crash handler: it tries for a moment (another thread may have it for a line of its own), and
/// then goes on without it rather than wait, as the thread that crashed may be the one holding it.
fn lock_briefly<T>(lock: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    for _ in 0..200 {
        match lock.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(e)) => return Some(e.into_inner()),
            Err(TryLockError::WouldBlock) => std::thread::yield_now(),
        }
    }
    None
}

/// The time of day as the log writes it (`14:30:05.123`), from the clock and how far it is from UTC (`offset`, in
/// seconds), without asking the computer for its time zone: for a crash's line.
fn time_of_day(now: SystemTime, offset: i32) -> String {
    let since = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = i64::try_from(since.as_secs()).unwrap_or(0).saturating_add(i64::from(offset)).rem_euclid(24 * 60 * 60);
    format!("{:02}:{:02}:{:02}.{:03}", secs / 3600, secs / 60 % 60, secs % 60, since.subsec_millis())
}

fn create_file(folder: &Path, start: DateTime<Local>) -> Option<(PathBuf, File)> {
    std::fs::create_dir_all(folder).ok()?;
    let stamp = start.format("%Y-%m-%d_%H-%M-%S");
    // Two runs started in the same second (tests, or a quick restart) each get their own file.
    for n in 1..100 {
        let name = if n == 1 { format!("{PREFIX}{stamp}.log") } else { format!("{PREFIX}{stamp}-{n}.log") };
        let path = folder.join(name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        // On Windows nobody can remove the log while this run is writing it (another copy of the app keeping the
        // folder in bounds as it starts): the removal fails, and is tried again at a later start, once this run has
        // closed it. Anyone can still read it.
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_SHARE_READ | FILE_SHARE_WRITE, without FILE_SHARE_DELETE.
            options.share_mode(0x1 | 0x2);
        }
        match options.open(&path) {
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
    let mut no_room_for_crashes = false;
    while let Ok(msg) = rx.recv() {
        let mut next = Some(msg);
        while let Some(msg) = next.take() {
            match msg {
                Msg::Line(line) if !full => {
                    let size = line.len() as u64 + 1;
                    if written + size > MAX_BYTES {
                        full = true;
                        let _ = writeln!(out, "(the log is full at 5 MB; this run writes no more, except crashes)");
                    } else {
                        written += size;
                        let _ = writeln!(out, "{line}");
                    }
                }
                Msg::Line(_) => {}
                // Rule 17: a crash is written even once the log has stopped writing other lines, up to CRASH_ROOM more.
                Msg::Crash(line) => {
                    let size = line.len() as u64 + 1;
                    if written + size <= MAX_BYTES + CRASH_ROOM {
                        written += size;
                        let _ = writeln!(out, "{line}");
                    } else if !no_room_for_crashes {
                        // Said once, so whoever reads the log knows the crashes didn't stop, only their lines here.
                        no_room_for_crashes = true;
                        let _ = writeln!(
                            out,
                            "(no room for more crashes in this log; the first crash at each place has a crash-….log \
                             file of its own)"
                        );
                    }
                }
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

/// Whether `name` is a session log as [`create_file`] names them, and nothing else: `rust-session-`, the time
/// (`2026-11-02_14-30-05`), perhaps `-` and a number, then `.log`. A copy ("… - Copy.log") isn't one.
pub fn is_session_log(name: &str) -> bool {
    name.strip_prefix(PREFIX).and_then(|r| r.strip_suffix(".log")).is_some_and(is_name_time)
}

/// Whether `s` is a time as the app puts it in a file's name (`2026-11-02_14-30-05`, from `%Y-%m-%d_%H-%M-%S`),
/// perhaps followed by `-` and a number (a second file in the same second), and nothing else. Session logs and F12
/// notes are named so (the C# app names its notes the same way).
pub(crate) fn is_name_time(s: &str) -> bool {
    const STAMP: &[u8; 19] = b"0000-00-00_00-00-00";
    let (Some(stamp), Some(tail)) = (s.get(..STAMP.len()), s.get(STAMP.len()..)) else { return false };
    let stamp_fits = stamp.bytes().zip(STAMP).all(|(b, t)| if *t == b'0' { b.is_ascii_digit() } else { b == *t });
    stamp_fits && (tail.is_empty() || tail.strip_prefix('-').is_some_and(crate::tidy::is_number))
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
    let written = new_crash_file(folder, text, secs);
    // A crash that ends the app took no place (it always gets a file), so it has none to give back.
    if written.is_none() && !fatal {
        files.give_back(place);
    }
    written
}

/// Writes `text` into a crash log of its own in `folder`, under the first free name for `secs`. `None` when no file
/// could be made; once one is made it's the crash's, even if the disk filled part way (part of a crash is more than
/// none, and a full disk then can't be handed a new empty file for every crash).
fn new_crash_file(folder: &Path, text: &str, secs: u64) -> Option<PathBuf> {
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
    let number = crate::tidy::is_number;
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
        let _ = std::fs::create_dir_all(&dir);
        // A log's age runs from the later of when it was made and changed, so the old ones are made first.
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
        std::thread::sleep(std::time::Duration::from_millis(20));
        let made_is_known = std::fs::metadata(dir.join(&copies[0])).and_then(|m| m.created()).is_ok();
        let now = Local::now();
        let a = SessionLog::new(&dir, now, "a");
        let b = SessionLog::new(&dir, now, "b");
        assert_ne!(a.path(), b.path());
        a.close();
        b.close();
        if made_is_known {
            // Every log was made a moment ago, so each may be another copy's, still being written: nothing went.
            assert_eq!(std::fs::read_dir(&dir).expect("listable").count(), 42 + 5 + copies.len());
        }
        // Three days on, the oldest go.
        crate::tidy::prune_logs(&dir, None, std::time::SystemTime::now() + 3 * crate::tidy::DAY);
        let left = std::fs::read_dir(&dir).expect("listable").count();
        assert_eq!(left, KEPT + 5 + copies.len());
        assert!(a.path().is_some_and(|p| p.exists()) && b.path().is_some_and(|p| p.exists()), "the newest stay");
        for name in &copies {
            assert!(dir.join(name).exists(), "{name} stays");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_log_just_made_always_stays_and_one_dated_far_after_now_goes_first() {
        let dir = temp_folder("future");
        let _ = std::fs::create_dir_all(&dir);
        let set = |name: &str, when: Option<std::time::SystemTime>| {
            std::fs::write(dir.join(name), "log").expect("writable");
            if let Some(when) = when {
                let f = File::options().write(true).open(dir.join(name)).expect("openable");
                f.set_modified(when).expect("settable");
            }
        };
        let names = |dir: &Path| -> Vec<String> {
            std::fs::read_dir(dir)
                .expect("listable")
                .map(|e| e.expect("listed").file_name().to_string_lossy().into_owned())
                .collect()
        };
        let later = std::time::SystemTime::now() + 3 * crate::tidy::DAY;
        // The log just made, made first so it's the oldest of all; then others, newer.
        let mine = format!("{PREFIX}1999-01-01_00-00-00.log");
        set(&mine, None);
        std::thread::sleep(std::time::Duration::from_millis(20));
        for i in 0..KEPT + 2 {
            set(&format!("{PREFIX}2001-01-01_00-00-{i:02}.log"), None);
        }
        crate::tidy::prune_logs(&dir, Some(&dir.join(&mine)), later);
        let left = names(&dir);
        assert_eq!(left.len(), KEPT);
        assert!(left.contains(&mine), "the log just made stays");
        let _ = std::fs::remove_dir_all(&dir);

        // Logs dated decades on (a clock that was plainly wrong once) would hold every place: they go first.
        let _ = std::fs::create_dir_all(&dir);
        let rightly = format!("{PREFIX}2001-01-01_00-00-00.log");
        set(&rightly, None);
        for i in 0..KEPT + 3 {
            set(&format!("{PREFIX}2099-01-01_00-00-{i:02}.log"), Some(later + 10_000 * crate::tidy::DAY));
        }
        crate::tidy::prune_logs(&dir, None, later);
        let left = names(&dir);
        assert_eq!(left.len(), KEPT);
        assert!(left.contains(&rightly), "the rightly dated one isn't pushed out");
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
    fn threads_crashing_at_one_place_at_once_get_one_file() {
        let dir = temp_folder("crash-race");
        let files = CrashFiles::new();
        let start = std::sync::Barrier::new(16);
        let written: Vec<Option<PathBuf>> = std::thread::scope(|s| {
            let threads: Vec<_> = (0..16u64)
                .map(|n| {
                    let (dir, files, start) = (&dir, &files, &start);
                    s.spawn(move || {
                        start.wait();
                        write_crash_file(dir, "the same bad file", 1_700_000_000 + n, "src/x.rs:42", false, files)
                    })
                })
                .collect();
            threads.into_iter().map(|t| t.join().expect("no panic")).collect()
        });
        assert_eq!(written.iter().filter(|w| w.is_some()).count(), 1);
        assert_eq!(std::fs::read_dir(&dir).expect("listable").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_place_whose_file_couldnt_be_made_gets_the_next_crash_its_file() {
        let dir = temp_folder("crash-blocked");
        let _ = std::fs::create_dir_all(&dir);
        // A file where the logs folder should be: no crash file can be made in it.
        let logs = dir.join("logs");
        std::fs::write(&logs, "in the way").expect("writable");
        let files = CrashFiles::new();
        assert_eq!(write_crash_file(&logs, "first", 1_700_000_000, "src/a.rs:1", false, &files), None);
        std::fs::remove_file(&logs).expect("removable");
        let second = write_crash_file(&logs, "second", 1_700_000_001, "src/a.rs:1", false, &files);
        assert_eq!(std::fs::read_to_string(second.expect("written")).expect("readable"), "second");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crash_is_written_even_once_the_log_is_full() {
        let dir = temp_folder("full");
        let log = SessionLog::new(&dir, Local::now(), "start");
        // Different lines (repeats are only counted), more than the 5 MB a run may write.
        let filler = "x".repeat(200);
        for i in 0..(MAX_BYTES / 200 + 100) {
            log.write(Level::Warning, "Scan", &format!("{i} {filler}"));
        }
        log.write(Level::Warning, "Scan", "not written: the log is full");
        log.put(Level::Error, "Crash", "Music Organiser crashed on thread Scan 1", Some("src/scan.rs:1"));
        // Crashes at many places still can't fill the disk: at most 1 MB more of them.
        let big = "y".repeat(100 * 1024);
        for i in 0..30 {
            log.put(Level::Error, "Crash", &format!("crash {i} {big}"), Some(&format!("src/read.rs:{i}")));
        }
        log.close();
        let path = log.path().expect("a file").to_owned();
        let text = std::fs::read_to_string(&path).expect("readable");
        assert!(text.contains("(the log is full at 5 MB; this run writes no more, except crashes)"));
        assert!(!text.contains("not written: the log is full"));
        assert!(text.contains("[Crash] Music Organiser crashed on thread Scan 1"));
        assert!(text.contains("[Crash] crash 0 "));
        let size = std::fs::metadata(&path).expect("there").len();
        assert!(size <= MAX_BYTES + CRASH_ROOM + 1024, "{size}");
        assert!(!text.contains("[Crash] crash 29 "), "the crashes past the extra room aren't written");
        assert_eq!(text.matches("(no room for more crashes in this log;").count(), 1, "and it says so, once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crash_while_this_thread_holds_the_logs_lock_is_still_written() {
        let dir = temp_folder("busy");
        let log = SessionLog::new(&dir, Local::now(), "start");
        {
            // As if this thread had crashed in the middle of writing a line: the lock is still taken.
            let _held = log.state.lock().expect("not poisoned");
            log.put(Level::Error, "Crash", "Music Organiser crashed on thread Scan 1", Some("src/a.rs:1"));
        }
        log.close();
        let text = std::fs::read_to_string(log.path().expect("a file")).expect("readable");
        assert!(text.contains("Error   [Crash] Music Organiser crashed on thread Scan 1"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crashes_at_one_place_are_counted_whichever_thread_they_were_on() {
        let dir = temp_folder("crash-repeats");
        let log = SessionLog::new(&dir, Local::now(), "start");
        // The same bad file on every background thread: another thread's name and backtrace each time.
        for i in 0..12 {
            let text = format!("Music Organiser crashed on thread Background {i}\nat src/x.rs:42\n\n{i}: backtrace");
            log.put(Level::Error, "Crash", &text, Some("src/x.rs:42"));
        }
        log.put(Level::Error, "Crash", "Music Organiser crashed on thread Scan 1", Some("src/y.rs:7"));
        log.close();
        let text = std::fs::read_to_string(log.path().expect("a file")).expect("readable");
        assert_eq!(text.matches("[Crash] Music Organiser crashed on thread Background").count(), 3, "{text}");
        assert!(text.contains("[Crash] (crashed at src/x.rs:42 10 times now) Music Organiser crashed on thread"));
        assert!(!text.contains("Background 11"), "the later ones are only counted");
        assert!(text.contains("[Crash] Music Organiser crashed on thread Scan 1"), "another place is written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crash_tells_its_time_without_the_time_zone() {
        let at = UNIX_EPOCH + Duration::from_millis((25 * 3600 + 61) * 1000 + 500);
        assert_eq!(time_of_day(at, 3600), "02:01:01.500");
        assert_eq!(time_of_day(UNIX_EPOCH + Duration::from_secs(30 * 60), -3600), "23:30:00.000", "the day before");
    }

    #[test]
    fn the_folder_is_kept_in_bounds_at_each_start() {
        let dir = temp_folder("start-prune");
        let _ = std::fs::create_dir_all(&dir);
        // Dated far after now (a clock that was wrong): they count as the oldest, and none is still being written.
        let far = std::time::SystemTime::now() + 10_000 * crate::tidy::DAY;
        for i in 0..KEPT + 5 {
            let old = dir.join(format!("{PREFIX}2099-01-01_00-00-{i:02}.log"));
            std::fs::write(&old, "old").expect("writable");
            File::options().write(true).open(&old).and_then(|f| f.set_modified(far)).expect("settable");
        }
        let log = SessionLog::new(&dir, Local::now(), "start");
        assert!(log.wait_for_pruning(Duration::from_secs(60)), "the start's look at the folder finishes");
        log.close();
        let logs: Vec<String> = std::fs::read_dir(&dir)
            .expect("listable")
            .map(|e| e.expect("listed").file_name().to_string_lossy().into_owned())
            .filter(|n| is_session_log(n))
            .collect();
        assert_eq!(logs.len(), KEPT, "{logs:?}");
        assert!(log.path().is_some_and(|p| p.exists()), "this run's own stays");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crash_that_ends_the_app_is_known_on_any_thread() {
        assert!(crash_ends_the_app(Some("main"), Some("index out of bounds")));
        assert!(crash_ends_the_app(Some("Scan 1"), Some("panic in a destructor during cleanup")));
        assert!(crash_ends_the_app(None, Some("panic in a function that cannot unwind")));
        assert!(!crash_ends_the_app(Some("Scan 1"), Some("index out of bounds")));
        assert!(!crash_ends_the_app(Some("Background 3"), None));
    }

    #[test]
    fn a_crash_that_ends_the_app_frees_no_other_crash_its_place() {
        let dir = temp_folder("fatal-blocked");
        let _ = std::fs::create_dir_all(&dir);
        let logs = dir.join("logs");
        let files = CrashFiles::new();
        assert!(write_crash_file(&logs, "first", 1_700_000_000, "src/a.rs:1", false, &files).is_some());
        // The crash that ends the app, at the same place, when no file can be made (a file in the folder's way).
        let blocked = dir.join("blocked");
        std::fs::write(&blocked, "in the way").expect("writable");
        assert_eq!(write_crash_file(&blocked, "the end", 1_700_000_001, "src/a.rs:1", true, &files), None);
        // That place had its file: it gets no second one.
        assert_eq!(write_crash_file(&logs, "again", 1_700_000_002, "src/a.rs:1", false, &files), None);
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
