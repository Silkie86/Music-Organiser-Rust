//! The session log: a plain-text diary of each run of the app in `logs\` in the app's data folder, the same as the
//! C# app's. It says which build ran, the pages opened, the jobs, every warning and error, and any `[Freeze]`, so
//! Claude can read what happened on Solomon's PC straight from the folder. Nothing of the music is in it, only what
//! the app did.
//!
//! Writing never waits on the disk: lines go to a writer thread of their own. A message that keeps coming
//! is written three times and then only counted (at 10, 100, 1000…), and a run stops writing at 5 MB, so a runaway
//! warning can't fill the disk; only a crash is still written after that, up to 1 MB more (rule 17: crashes always
//! log), and then the crash that ends the app and the lines of how the run ended ("Closed"), up to 256 KB more (one
//! that doesn't fit is cut short, saying so). A crash is counted by where in the code it happened, whichever thread it
//! was on, and so is its backtrace, a line of its own after it; the crash that ends the app is never only counted.
//! A panic the app catches on purpose (`mo_catch::catch`: a damaged file tripped a reader) isn't a crash: it's one
//! warning line ([`caught`]), counted by its place too.
//! At each start, a low-priority thread keeps the folder in bounds
//! ([`crate::tidy::prune_logs`]): the newest 30 runs' logs, the newest 30 crash logs and each tool's newest 10 reports.
//! A run's log can't be removed while it's written: on Windows nobody can remove it while it's open, and on a Mac or
//! Linux it's locked (a lock that only says so, for another copy of the app keeping the folder in bounds to see).
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

use crate::tidy::Tidied;

/// This app's logs start with this. The C# app's are `session-…` and it prunes those by itself, so each app keeps
/// its own 30 and the launcher's many restarts never push out the C# app's logs (or a C# log still being written).
pub const PREFIX: &str = "rust-session-";
/// How many runs' logs are kept.
pub const KEPT: usize = 30;
/// The background helper's logs ([`crate::helper`]) start with this instead, so its runs (each can last days) never
/// push out the app's.
pub const HELPER_PREFIX: &str = "rust-helper-";
/// How many of the helper's runs' logs are kept.
pub const HELPER_KEPT: usize = 10;
/// A run that writes this much stops writing (and says so).
const MAX_BYTES: u64 = 5 * 1024 * 1024;
/// How much more a run writes for crashes, once it has stopped writing everything else: a crash always reaches the
/// log (rule 17), but a crash that keeps happening can't fill the disk either.
const CRASH_ROOM: u64 = 1024 * 1024;
/// How much more again the crash that ends the app and the lines of how the run ended ("Closed") may use, past
/// [`CRASH_ROOM`]. Each comes once a run, so they show; and if a crash were taken for the app's end again and again,
/// they still couldn't fill the disk. One that doesn't fit is cut short, saying so.
const LAST_ROOM: u64 = 256 * 1024;
/// When a line is cut short to fit in [`LAST_ROOM`], this much is still kept for the lines after it, so "Closed"
/// shows.
const KEPT_FOR_THE_END: u64 = 1024;
/// What the crash that ends the app says when it's cut short to fit (the crash that ends the app always gets a crash
/// log of its own, with the whole crash, unless that can't be written either).
const CUT_CRASH: &str = "(cut short: the log is full; the whole crash goes in a crash-….log of its own)";
/// What another line of how the run ended says when it's cut short to fit.
const CUT_LINE: &str = "(cut short: the log is full)";
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
/// What std says of a crash it can't recover from: it ends the app straight after, on whichever thread it was
/// (checked in Rust 1.99's own code, `core::panicking`, `core::ub_checks` and `std::sys::exit`, and in the checks the
/// compiler puts in `mem::zeroed` and `mem::uninitialized`). In any build: a second crash while one is being cleaned
/// up, a crash inside code that mustn't crash, the app's end started again while it's ending, or a value made of
/// zeroes (or left unset) that its type doesn't allow. In debug builds (the launcher's), also the checks that stop
/// unsafe code going wrong: a pointer that's null or not lined up, an enum with no such value, or a broken promise to
/// an unsafe function. Each of these also can't unwind ([`cannot_unwind`]), which the crash handler checks first.
///
/// A crash in a thread's own data as the thread ends (a thread-local's `Drop`) ends the app too, but says nothing of
/// the kind. So does a second crash while one is being cleaned up, on Windows: there the system ends the app as soon
/// as the second crash leaves its `Drop`, before std can say anything. So for every crash (not a panic caught on
/// purpose: see `mo_catch`), the crash handler keeps the words typed in an open F12 note whatever this says (the words
/// of a note being saved that aren't on disk yet are kept only for a crash it knows ends the app).
const ENDS_THE_APP: [&str; 6] = [
    "panic in a destructor during cleanup",
    "panic in a function that cannot unwind",
    "std::process::exit called re-entrantly",
    "null pointer dereference occurred",
    "null reference produced",
    "is_nonoverlapping: `size_of::<T>() * count` overflows a usize",
];
/// The same, for those that go on to say more (which check, which address): they're known by how they start.
const ENDS_THE_APP_STARTS: [&str; 6] = [
    "unsafe precondition(s) violated: ",
    "misaligned pointer dereference: ",
    "trying to construct an enum from an invalid value ",
    "attempted to zero-initialize type `",
    "attempted to leave type `",
    "attempted to instantiate uninhabited type `",
];

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

/// Whether a crash that nothing catches ends the app: one on the window's thread (`main`), or one std can't recover
/// from, on any thread (`message` is what the crash says, see [`std_ends_the_app`]). `thread` is the crashing thread's
/// name.
pub fn crash_ends_the_app(thread: Option<&str>, message: Option<&str>) -> bool {
    thread == Some("main") || std_ends_the_app(message)
}

/// Whether `message` (what a panic says) is std's own as it ends the app ([`ENDS_THE_APP`]): nothing can catch such a
/// panic, whatever was to catch it.
pub fn std_ends_the_app(message: Option<&str>) -> bool {
    message.is_some_and(|m| ENDS_THE_APP.contains(&m) || ENDS_THE_APP_STARTS.iter().any(|s| m.starts_with(s)))
}

/// Whether the panic the crash handler was given can't unwind: nothing can catch it, and std ends the app as soon as
/// the handler is done, whatever it says ([`ENDS_THE_APP`] are the words of those known). Std doesn't let a program ask
/// yet (`PanicHookInfo::can_unwind` isn't stable in Rust 1.99), but shows it when the panic is printed for debugging,
/// so it's read from there; a test says so if that ever changes. Called from the crash handler, so it never panics.
pub fn cannot_unwind(info: &std::panic::PanicHookInfo<'_>) -> bool {
    format!("{info:?}").contains("can_unwind: false")
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
    /// The crash that ends the app, or a line of how the run ended: written past the room for crashes too, up to
    /// [`LAST_ROOM`] more, and cut short to fit, with what it then says ([`CUT_CRASH`], [`CUT_LINE`]).
    Last(String, &'static str),
    /// Write out everything so far, then say so on the channel.
    Flush(Sender<()>),
    Close,
}

/// What a line is: how it's counted, and how far past the log's limit it's still written.
#[derive(Clone, Copy)]
enum Kind<'a> {
    /// Counted by its words, and not written once the log is full.
    Plain,
    /// A crash's line, or its backtrace (`backtrace`): counted by `place`, where in the code it crashed, the two with
    /// a count each, and written into the extra room for crashes. The crash that ends the app (`ends_the_app`) is
    /// never only counted, however many came before it, and goes into the room for how the run ended.
    Crash { place: &'a str, backtrace: bool, ends_the_app: bool },
    /// A panic the app caught on purpose ([`caught`]): counted by `place`, as a crash is, but not written once the log
    /// is full, as it isn't a crash.
    Caught { place: &'a str },
    /// A line of how the run ended ("Closed"): written into the room for that too ([`LAST_ROOM`]), so it shows.
    Last,
}

struct State {
    seen: HashMap<String, u32>,
    recent: VecDeque<String>,
}

/// The start's look at the logs folder ([`crate::tidy::prune_logs`]).
enum Pruning {
    Going(JoinHandle<Tidied>),
    /// Finished, and what it did, until [`SessionLog::take_pruned`] takes it.
    Done(Tidied),
    /// Taken already, or there's nothing to take: the thread couldn't start, or it crashed (the crash is in the log).
    Over,
}

/// One run's log.
pub struct SessionLog {
    path: Option<PathBuf>,
    tx: Mutex<Option<Sender<Msg>>>,
    state: Mutex<State>,
    writer: Mutex<Option<JoinHandle<()>>>,
    /// Keeping the logs folder in bounds, off the window's thread, and then what it did; [`SessionLog::close`] waits a
    /// little for it.
    pruning: Mutex<Pruning>,
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

/// Starts the background helper's log in the usual folder: named [`HELPER_PREFIX`]…, so it's kept apart from the
/// app's.
pub fn start_helper() -> &'static SessionLog {
    let header = crate::version::log_header(Local::now()).replacen(
        "Music Organiser started",
        "Music Organiser helper started",
        1,
    );
    CURRENT.get_or_init(|| SessionLog::named(&crate::paths::logs_dir(), HELPER_PREFIX, Local::now(), &header))
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

/// A line of how the run ended (the window stopping with an error, a note still being saved as the app closed): written
/// past the log's limit as "Closed" is, so it shows even once the log is full (see [`LAST_ROOM`]).
pub fn last(level: Level, area: &str, message: &str) {
    if let Some(log) = current() {
        log.put(level, area, message, Kind::Last);
    }
}

/// A crash at `place` in the code (`file:line`), as an error in the "Crash" area: written even once this run's log has
/// stopped writing other lines (rule 17), up to [`CRASH_ROOM`] more. Crashes are counted by their place: the same
/// bug hit on every background thread has another thread's name each time, but one place. The crash that ends the app
/// (`ends_the_app`) is always written, in full, however many came before it. Called from the crash handler, so it
/// never waits long and never crashes itself.
pub fn crash(place: &str, text: &str, ends_the_app: bool) {
    if let Some(log) = current() {
        log.put(Level::Error, "Crash", text, Kind::Crash { place, backtrace: false, ends_the_app });
    }
}

/// A panic the app caught on purpose at `place` in the code (`file:line`), with `mo_catch::catch`: a damaged file
/// tripped a reader, and the work it was part of gives an error instead. A warning in the "Caught" area, counted by its
/// place as crashes are, so a folder of damaged files, or one read again and again, is written three times and then
/// only counted. Not written once this run's log is full: it isn't a crash. Called from the crash handler, so it never
/// waits long and never crashes itself.
pub fn caught(place: &str, text: &str) {
    if let Some(log) = current() {
        log.put(Level::Warning, "Caught", text, Kind::Caught { place });
    }
}

/// The backtrace of a crash at `place` (where in the code it came from), as a line of its own after the crash's own
/// line ([`crash`]): the crash handler writes the crash first, as taking its backtrace can wait. Counted by its place
/// as the crash's line is, with a count of its own, so each crash counts once; past the third at a place, the crash's
/// line says how many there were. Called from the crash handler, so it never waits long and never crashes itself.
pub fn crash_backtrace(place: &str, text: &str, ends_the_app: bool) {
    if let Some(log) = current() {
        log.put(Level::Error, "Crash", text, Kind::Crash { place, backtrace: true, ends_the_app });
    }
}

impl SessionLog {
    /// Opens a new log file for a run that started at `start`. If the folder can't be written, the log only keeps its
    /// recent lines in memory: no log is better than no app.
    pub fn new(folder: &Path, start: DateTime<Local>, header: &str) -> SessionLog {
        SessionLog::named(folder, PREFIX, start, header)
    }

    /// [`SessionLog::new`] with the file named `prefix`… ([`PREFIX`] or [`HELPER_PREFIX`]).
    pub fn named(folder: &Path, prefix: &str, start: DateTime<Local>, header: &str) -> SessionLog {
        let file = create_file(folder, prefix, start);
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
                crate::tidy::prune_logs(&folder, just_made.as_deref(), SystemTime::now())
            })
            .map_or(Pruning::Over, Pruning::Going)
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
        self.put(level, area, message, Kind::Plain);
    }

    /// Writes a line, counted and kept within the log's limits as its `kind` says (see [`Kind`]). A crash's line, and
    /// any line written while a thread is crashing, never waits long for the log's lock.
    fn put(&self, level: Level, area: &str, message: &str, kind: Kind<'_>) {
        // Trailing spaces only make the log wide.
        let message: String = message.lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
        // A crash's line, and any line written while a thread is crashing (from the crash handler, or by a drop as the
        // thread unwinds), mustn't crash or wait long: a crash there ends the app at once.
        let crash = matches!(kind, Kind::Crash { .. } | Kind::Caught { .. }) || std::thread::panicking();
        // The time and the key the line is counted by are made before the lock is taken: nothing that could crash
        // runs while it's held (see the top of this file).
        let time = if crash {
            // Asking the computer for its time zone can fail, and a failure in the crash handler would end the app
            // at once: such a line uses the zone the last line found.
            time_of_day(SystemTime::now(), self.offset.load(Ordering::Relaxed))
        } else {
            let now = Local::now();
            self.offset.store(now.offset().local_minus_utc(), Ordering::Relaxed);
            now.format("%H:%M:%S%.3f").to_string()
        };
        let key = match kind {
            Kind::Crash { place, backtrace: false, .. } => format!("Crash\u{1}{place}"),
            Kind::Crash { place, backtrace: true, .. } => format!("Backtrace\u{1}{place}"),
            Kind::Caught { place } => format!("Caught\u{1}{place}"),
            Kind::Plain | Kind::Last => format!("{}\u{1}{area}\u{1}{message}", level.name()),
        };
        let whole = |message: &str| format!("{time} {:<7} [{area}] {message}", level.name());
        let line = match self.state_for(crash) {
            Some(mut st) => {
                if st.seen.len() >= SEEN_MAX && !st.seen.contains_key(&key) {
                    st.seen.clear();
                }
                let times = st.seen.entry(key).or_insert(0);
                *times = times.saturating_add(1);
                let times = *times;
                let first = message.lines().next().unwrap_or_default();
                let line = match kind {
                    _ if times <= REPEATS_WRITTEN => whole(&message),
                    // Rule 17: the crash that ends the app is always written, never only counted.
                    Kind::Crash { ends_the_app: true, .. } => whole(&message),
                    _ if !is_power_of_ten(times) => return,
                    // The crash's own line says how many times by now.
                    Kind::Crash { backtrace: true, .. } => return,
                    Kind::Crash { place, .. } => whole(&format!("(crashed at {place} {times} times now) {first}")),
                    Kind::Caught { place } => whole(&format!("(caught at {place} {times} times now) {first}")),
                    Kind::Plain | Kind::Last => whole(&format!("(said {times} times now) {first}")),
                };
                st.recent.push_back(line.clone());
                while st.recent.len() > RECENT {
                    st.recent.pop_front();
                }
                line
            }
            // Only a crash's line (or one written while a thread crashes) gets here, when the lock stays busy: it's
            // written, just not counted.
            None => whole(&message),
        };
        let msg = match kind {
            Kind::Plain | Kind::Caught { .. } => Msg::Line(line),
            Kind::Crash { ends_the_app: false, .. } => Msg::Crash(line),
            Kind::Crash { ends_the_app: true, .. } => Msg::Last(line, CUT_CRASH),
            Kind::Last => Msg::Last(line, CUT_LINE),
        };
        // This lock is only ever held to hand a line over (or take the writer away as the log closes), which can't
        // crash or wait, so even a crash's line waits for it rather than be lost.
        if let Some(tx) = self.tx.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            let _ = tx.send(msg);
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

    /// Waits until everything written so far is on disk (2 seconds at most). Used by the crash handler, and by the
    /// freeze watchdog when the window stays stuck.
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

    /// Writes "Closed" (into the room for how the run ended, so a clean end shows even once the log is full) and stops
    /// the writer. Later lines are kept in memory only. It waits a little for the logs
    /// folder to be kept in bounds first, so a crash doing that still reaches this log, and then for the last lines
    /// to be written; never long, as a folder on a drive that's asleep mustn't hold the app open.
    pub fn close(&self) {
        self.wait_for_pruning(PRUNE_WAIT);
        // Into the extra room, as a crash's line goes, so a clean end shows even once the log is full.
        self.put(Level::Info, "App", "Closed", Kind::Last);
        if let Some(tx) = self.tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = tx.send(Msg::Close);
        }
        if let Some(h) = self.writer.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = wait_for(h, WRITER_WAIT);
        }
    }

    /// Waits up to `limit` for the start's look at the logs folder to finish; says whether it has. What it did is kept
    /// for [`SessionLog::take_pruned`].
    pub(crate) fn wait_for_pruning(&self, limit: Duration) -> bool {
        let mut pruning = self.pruning.lock().unwrap_or_else(|e| e.into_inner());
        match std::mem::replace(&mut *pruning, Pruning::Over) {
            Pruning::Going(h) => match wait_for(h, limit) {
                Ok(done) => {
                    if let Some(done) = done {
                        *pruning = Pruning::Done(done);
                    }
                    true
                }
                // Still going: it's put back, so a later wait (or the app's end) still finds it.
                Err(h) => {
                    *pruning = Pruning::Going(h);
                    false
                }
            },
            finished => {
                *pruning = finished;
                true
            }
        }
    }

    /// What the start's look at the logs folder did (the logs it removed, and those it couldn't), once it has
    /// finished: given once, and then `None`. It never waits: while the look is still going, or someone is waiting for
    /// it, it's `None`, and what it did is kept for a later call.
    pub fn take_pruned(&self) -> Option<Tidied> {
        let mut pruning = match self.pruning.try_lock() {
            Ok(pruning) => pruning,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        match std::mem::replace(&mut *pruning, Pruning::Over) {
            Pruning::Going(h) if !h.is_finished() => {
                *pruning = Pruning::Going(h);
                None
            }
            // Finished, so this doesn't wait.
            Pruning::Going(h) => h.join().ok(),
            Pruning::Done(done) => Some(done),
            Pruning::Over => None,
        }
    }
}

/// Waits up to `limit` for a thread to finish, and gives what it returned (`None` if it crashed). Gives the thread
/// back if it hasn't finished.
fn wait_for<T>(thread: JoinHandle<T>, limit: Duration) -> Result<Option<T>, JoinHandle<T>> {
    let until = Instant::now() + limit;
    while !thread.is_finished() {
        if Instant::now() >= until {
            return Err(thread);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(thread.join().ok())
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

fn create_file(folder: &Path, prefix: &str, start: DateTime<Local>) -> Option<(PathBuf, File)> {
    std::fs::create_dir_all(folder).ok()?;
    let stamp = start.format("%Y-%m-%d_%H-%M-%S");
    // Two runs started in the same second (tests, or a quick restart) each get their own file.
    for n in 1..100 {
        let name = if n == 1 { format!("{prefix}{stamp}.log") } else { format!("{prefix}{stamp}-{n}.log") };
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
            Ok(f) => {
                // On a Mac or Linux an open file can be removed all the same, so the log is locked while this run
                // writes it (until the file closes): another copy of the app keeping the folder in bounds as it starts
                // passes over a locked log. The lock only says so; anyone can still read the log. It never waits:
                // nobody else can have locked a file just made. Not on Windows, where the share mode above already
                // stops the removal, and a lock would stop others reading the log.
                #[cfg(unix)]
                let _ = f.try_lock();
                return Some((path, f));
            }
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
    let mut no_room_at_the_end = false;
    while let Ok(msg) = rx.recv() {
        let mut next = Some(msg);
        while let Some(msg) = next.take() {
            match msg {
                Msg::Line(line) if !full => {
                    let size = line.len() as u64 + 1;
                    if written + size > MAX_BYTES {
                        full = true;
                        let _ = writeln!(
                            out,
                            "(this log is full: a run writes up to 5 MB, and the next line didn't fit; from here it \
                             writes only crashes and the lines of how the run ends)"
                        );
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
                            "(a crash didn't fit in the room this log keeps for crashes, so it isn't here, nor is a \
                             later one that doesn't fit; the first crash at each of the first {CRASH_FILES_PER_RUN} \
                             places in the code to crash has a crash-….log file of its own, and the crash that ends \
                             the app always gets one)"
                        );
                    }
                }
                // The crash that ends the app, and the lines of how the run ended: each comes once a run, so they're
                // written past the room for crashes too (up to LAST_ROOM more). One that doesn't fit is cut short,
                // saying so (`cut`): how a line starts says the most. Some room is kept for the lines after it.
                Msg::Last(line, cut) => {
                    let size = line.len() as u64 + 1;
                    let room = (MAX_BYTES + CRASH_ROOM + LAST_ROOM).saturating_sub(written);
                    if size <= room {
                        written += size;
                        let _ = writeln!(out, "{line}");
                    } else if !no_room_at_the_end {
                        let fits = room.saturating_sub(KEPT_FOR_THE_END + cut.len() as u64 + 2);
                        let start = &line[..line.floor_char_boundary(usize::try_from(fits).unwrap_or(usize::MAX))];
                        // Once not even a line's start fits, that's said once; after that a line that doesn't fit
                        // whole is dropped, and one that fits ("Closed") is still written.
                        no_room_at_the_end = start.is_empty();
                        let short = if start.is_empty() { cut.to_owned() } else { format!("{start} {cut}") };
                        written += short.len() as u64 + 1;
                        let _ = writeln!(out, "{short}");
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

/// Whether `name` is one of the background helper's logs: as [`is_session_log`], with [`HELPER_PREFIX`].
pub fn is_helper_log(name: &str) -> bool {
    name.strip_prefix(HELPER_PREFIX).and_then(|r| r.strip_suffix(".log")).is_some_and(is_name_time)
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
/// where. `place` is where in the code it crashed (`file:line`) and `fatal` whether it ends the app (see
/// [`crash_ends_the_app`]). The crash that ends the app always gets a file; others get one for the first crash at each
/// place, for up to [`CRASH_FILES_PER_RUN`] places a run (`files` keeps count; the session log still has every crash).
/// A second crash in the same second gets a name of its own rather than writing over the first. The crash's backtrace
/// is added to it afterwards ([`add_to_crash_file`]). Called from the crash handler, so it never panics.
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

/// Adds `text` (the crash's backtrace, taken once the crash was safely written) to the end of the crash log that
/// [`write_crash_file`] just wrote at `path`. A file that's gone meanwhile stays gone. Called from the crash handler,
/// so it never panics.
pub fn add_to_crash_file(path: &Path, text: &str) {
    if let Ok(mut f) = OpenOptions::new().append(true).open(path) {
        let _ = f.write_all(text.as_bytes());
    }
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

    /// A crash at `place` that the app goes on after, as the crash handler writes it.
    fn crash_at(log: &SessionLog, place: &str, text: &str) {
        log.put(Level::Error, "Crash", text, Kind::Crash { place, backtrace: false, ends_the_app: false });
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
        // Each log was made a moment ago: Windows knows when a file was made, and a Mac or Linux knows when its entry
        // last changed (setting its time just now changed it).
        let made_is_known = cfg!(unix) || std::fs::metadata(dir.join(&copies[0])).and_then(|m| m.created()).is_ok();
        let now = Local::now();
        let a = SessionLog::new(&dir, now, "a");
        let b = SessionLog::new(&dir, now, "b");
        assert_ne!(a.path(), b.path());
        // Both starts' looks at the folder finish before the folder is counted (closing waits only 2 seconds).
        assert!(a.wait_for_pruning(Duration::from_secs(60)), "the first start's look at the folder finishes");
        assert!(b.wait_for_pruning(Duration::from_secs(60)), "the second start's look at the folder finishes");
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
        crash_at(&log, "src/scan.rs:1", "Music Organiser crashed on thread Scan 1");
        // Crashes at many places still can't fill the disk: at most 1 MB more of them.
        let big = "y".repeat(100 * 1024);
        for i in 0..30 {
            crash_at(&log, &format!("src/read.rs:{i}"), &format!("crash {i} {big}"));
        }
        log.close();
        let path = log.path().expect("a file").to_owned();
        let text = std::fs::read_to_string(&path).expect("readable");
        let full = "(this log is full: a run writes up to 5 MB, and the next line didn't fit; from here it writes only \
                    crashes and the lines of how the run ends)";
        assert!(text.contains(full));
        assert!(!text.contains("not written: the log is full"));
        assert!(text.contains("[Crash] Music Organiser crashed on thread Scan 1"));
        assert!(text.contains("[Crash] crash 0 "));
        let size = std::fs::metadata(&path).expect("there").len();
        assert!(size <= MAX_BYTES + CRASH_ROOM + 1024, "{size}");
        assert!(!text.contains("[Crash] crash 29 "), "the crashes past the extra room aren't written");
        assert_eq!(text.matches(NO_ROOM_FOR_A_CRASH).count(), 1, "and it says so, once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// How the notice that a crash didn't fit starts.
    const NO_ROOM_FOR_A_CRASH: &str = "(a crash didn't fit in the room this log keeps for crashes,";

    #[test]
    fn the_crash_that_ends_the_app_and_the_last_line_show_even_once_the_log_is_full() {
        let dir = temp_folder("full-end");
        let log = SessionLog::new(&dir, Local::now(), "start");
        let filler = "x".repeat(200);
        for i in 0..(MAX_BYTES / 200 + 100) {
            log.write(Level::Warning, "Scan", &format!("{i} {filler}"));
        }
        // Crashes at many places use up the extra room for crashes, to its last few bytes…
        let big = "y".repeat(100 * 1024);
        for i in 0..30 {
            crash_at(&log, &format!("src/read.rs:{i}"), &format!("crash {i} {big}"));
        }
        for i in 0..4000 {
            crash_at(&log, &format!("src/tiny.rs:{i}"), &format!("c{i}"));
        }
        // …and still the crash that ends the app is written, and so is how the run ended.
        let fatal = Kind::Crash { place: "src/main.rs:5", backtrace: false, ends_the_app: true };
        log.put(Level::Error, "Crash", "Music Organiser crashed on thread main", fatal);
        log.close();
        let path = log.path().expect("a file").to_owned();
        let text = std::fs::read_to_string(&path).expect("readable");
        assert!(text.contains(NO_ROOM_FOR_A_CRASH), "the room for crashes was used up");
        assert!(text.contains("[Crash] Music Organiser crashed on thread main"), "the crash that ends the app");
        assert!(text.trim_end().ends_with("[App] Closed"), "a clean end shows");
        let size = std::fs::metadata(&path).expect("there").len();
        assert!(size <= MAX_BYTES + CRASH_ROOM + LAST_ROOM, "{size}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_line_of_how_the_run_ended_too_long_for_the_room_left_is_cut_short_saying_so() {
        // Shifted by 0, 1 and 2 bytes, so in one of them the cut falls inside a character three bytes long.
        for shift in 0..3 {
            let dir = temp_folder(&format!("cut-short-{shift}"));
            let log = SessionLog::new(&dir, Local::now(), "start");
            let filler = "x".repeat(200);
            for i in 0..(MAX_BYTES / 200 + 100) {
                log.write(Level::Warning, "Scan", &format!("{i} {filler}"));
            }
            // How the run ended shows even once the log is full.
            log.put(Level::Error, "App", "The window stopped with an error: no graphics", Kind::Last);
            // The crash that ends the app, far longer than all the room left: how it starts is written, cut at a whole
            // character, and that it was cut short.
            let fatal = Kind::Crash { place: "src/main.rs:5", backtrace: false, ends_the_app: true };
            let euros = "€".repeat(usize::try_from((CRASH_ROOM + LAST_ROOM) / 3 + 64 * 1024).expect("small"));
            let long = format!("Music Organiser crashed on thread main {}{euros}", "a".repeat(shift));
            log.put(Level::Error, "Crash", &long, fatal);
            // Then two more lines of how it ended, each longer than the room kept for the end: the first says only
            // that it was cut short, the second isn't written at all.
            let more = "y".repeat(usize::try_from(2 * KEPT_FOR_THE_END).expect("small"));
            log.put(Level::Warning, "Note", &format!("first {more}"), Kind::Last);
            log.put(Level::Warning, "Note", &format!("second {more}"), Kind::Last);
            log.close();
            let path = log.path().expect("a file").to_owned();
            let text = std::fs::read_to_string(&path).expect("readable: only whole characters");
            assert!(text.contains("[App] The window stopped with an error: no graphics"));
            assert!(text.contains("[Crash] Music Organiser crashed on thread main "), "how it starts");
            assert!(text.contains(&format!("€ {CUT_CRASH}\n")), "that it was cut short, after a whole character");
            assert!(text.contains(&format!("\n{CUT_LINE}\n")), "a line whose start doesn't fit says only that");
            assert_eq!(text.matches(CUT_LINE).count(), 1, "once");
            assert!(!text.contains("first y") && !text.contains("second y"), "nothing of the lines that didn't fit");
            assert!(text.trim_end().ends_with("[App] Closed"), "a clean end still shows");
            let size = std::fs::metadata(&path).expect("there").len();
            assert!(size <= MAX_BYTES + CRASH_ROOM + LAST_ROOM, "{size}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn a_crash_that_ends_the_app_is_always_written_never_only_counted() {
        let dir = temp_folder("fatal-counted");
        let log = SessionLog::new(&dir, Local::now(), "start");
        let place = "src/x.rs:42";
        // A bad file crashes every background thread at one place (only counted after the third)…
        for i in 0..5 {
            crash_at(&log, place, &format!("Music Organiser crashed on thread Background {i}"));
            let backtrace = Kind::Crash { place, backtrace: true, ends_the_app: false };
            log.put(Level::Error, "Crash", &format!("Backtrace of the crash on thread Background {i}:"), backtrace);
        }
        // …and then the window's thread, at the same place, which ends the app.
        let fatal = |backtrace| Kind::Crash { place, backtrace, ends_the_app: true };
        log.put(Level::Error, "Crash", "Music Organiser crashed on thread main", fatal(false));
        log.put(Level::Error, "Crash", "Backtrace of the crash on thread main:\n   0: main", fatal(true));
        log.close();
        let text = std::fs::read_to_string(log.path().expect("a file")).expect("readable");
        assert!(!text.contains("Background 4"), "the crashes the app went on after are only counted: {text}");
        assert!(text.contains("[Crash] Music Organiser crashed on thread main\n"), "{text}");
        assert!(text.contains("[Crash] Backtrace of the crash on thread main:\n   0: main"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crash_while_this_thread_holds_the_logs_lock_is_still_written() {
        let dir = temp_folder("busy");
        let log = SessionLog::new(&dir, Local::now(), "start");
        {
            // As if this thread had crashed in the middle of writing a line: the lock is still taken.
            let _held = log.state.lock().expect("not poisoned");
            crash_at(&log, "src/a.rs:1", "Music Organiser crashed on thread Scan 1");
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
            crash_at(&log, "src/x.rs:42", &format!("Music Organiser crashed on thread Background {i}\nat src/x.rs:42"));
            let backtrace = Kind::Crash { place: "src/x.rs:42", backtrace: true, ends_the_app: false };
            let trace = format!("Backtrace of the crash on thread Background {i}:\n   {i}: a frame");
            log.put(Level::Error, "Crash", &trace, backtrace);
        }
        crash_at(&log, "src/y.rs:7", "Music Organiser crashed on thread Scan 1");
        log.close();
        let text = std::fs::read_to_string(log.path().expect("a file")).expect("readable");
        // Each crash counts once: its backtrace's line has a count of its own.
        assert_eq!(text.matches("[Crash] Music Organiser crashed on thread Background").count(), 3, "{text}");
        assert_eq!(text.matches("[Crash] Backtrace of the crash on thread Background").count(), 3, "{text}");
        assert!(text.contains("[Crash] (crashed at src/x.rs:42 10 times now) Music Organiser crashed on thread"));
        assert!(!text.contains("times now) Backtrace"), "the crash's own line says how many");
        assert!(!text.contains("Background 11"), "the later ones are only counted");
        assert!(text.contains("[Crash] Music Organiser crashed on thread Scan 1"), "another place is written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn panics_caught_on_purpose_are_warnings_counted_by_place_and_not_written_once_the_log_is_full() {
        let dir = temp_folder("caught");
        let log = SessionLog::new(&dir, Local::now(), "start");
        let caught = |place: &str, said: &str| log.put(Level::Warning, "Caught", said, Kind::Caught { place });
        // A folder of damaged files: each says which, but all trip the reader at one place.
        for i in 0..12 {
            caught("src/read.rs:9", &format!("damaged file {i} (at src/read.rs:9, on thread Scan {i})"));
        }
        caught("src/other.rs:3", "another place");
        // Counted apart from a crash at the same place.
        crash_at(&log, "src/read.rs:9", "Music Organiser crashed on thread Scan 1");
        // Once the log is full, only crashes go on being written.
        let filler = "x".repeat(200);
        for i in 0..(MAX_BYTES / 200 + 100) {
            log.write(Level::Info, "Scan", &format!("{i} {filler}"));
        }
        caught("src/late.rs:1", "not written: the log is full");
        log.close();
        let text = std::fs::read_to_string(log.path().expect("a file")).expect("readable");
        assert_eq!(text.matches("Warning [Caught] damaged file ").count(), 3, "{text}");
        assert!(text.contains("Warning [Caught] (caught at src/read.rs:9 10 times now) damaged file 9 (at "), "{text}");
        assert!(!text.contains("damaged file 10"), "the later ones are only counted");
        assert!(text.contains("Warning [Caught] another place"));
        assert!(text.contains("[Crash] Music Organiser crashed on thread Scan 1"));
        assert!(!text.contains("not written: the log is full"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crash_files_backtrace_is_added_after_it() {
        let dir = temp_folder("crash-added");
        let files = CrashFiles::new();
        let file = write_crash_file(&dir, "crashed\n", 1_700_000_000, "src/a.rs:1", false, &files).expect("written");
        add_to_crash_file(&file, "\n   0: a frame\n");
        assert_eq!(std::fs::read_to_string(&file).expect("readable"), "crashed\n\n   0: a frame\n");
        // One that's gone meanwhile stays gone.
        std::fs::remove_file(&file).expect("removable");
        add_to_crash_file(&file, "\n   0: a frame\n");
        assert!(!file.exists());
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
        // The 5 over the 30 went, and one more to make room for this run's own: what it did is kept, for the report of
        // what was tidied.
        assert_eq!(log.take_pruned().map(|done| done.removed), Some(5 + 1));
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
    fn what_the_start_pruned_is_given_once_and_never_waited_for() {
        let dir = temp_folder("pruned");
        let log = SessionLog::new(&dir, Local::now(), "start");
        assert!(log.wait_for_pruning(Duration::from_secs(60)));
        assert_eq!(log.take_pruned(), Some(Tidied::default()), "a new folder has nothing to remove");
        assert_eq!(log.take_pruned(), None, "it's given once");
        // A look at the folder that's still going (on a drive that's asleep): nothing yet, and no waiting for it.
        let (go, wait) = channel::<()>();
        let going = std::thread::spawn(move || {
            let _ = wait.recv_timeout(Duration::from_secs(10));
            Tidied { removed: 3, ..Tidied::default() }
        });
        *log.pruning.lock().expect("not poisoned") = Pruning::Going(going);
        let asked = Instant::now();
        assert_eq!(log.take_pruned(), None);
        assert!(asked.elapsed() < Duration::from_secs(5), "it didn't wait for the look to finish");
        go.send(()).expect("the look is still going");
        assert!(log.wait_for_pruning(Duration::from_secs(60)));
        assert_eq!(log.take_pruned().map(|done| done.removed), Some(3), "once it's done, what it did");
        assert_eq!(log.take_pruned(), None);
        log.close();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crash_that_ends_the_app_is_known_on_any_thread() {
        assert!(crash_ends_the_app(Some("main"), Some("index out of bounds")));
        assert!(crash_ends_the_app(Some("Scan 1"), Some("panic in a destructor during cleanup")));
        assert!(crash_ends_the_app(None, Some("panic in a function that cannot unwind")));
        // A debug build's checks on unsafe code, as Rust 1.99 words them: the details after the start vary.
        for said in [
            "unsafe precondition(s) violated: slice::from_raw_parts requires the pointer to be aligned and non-null, \
             and the total size of the slice not to exceed `isize::MAX`\n\nThis indicates a bug in the program. This \
             Undefined Behavior check is optional, and cannot be relied on for safety.",
            "misaligned pointer dereference: address must be a multiple of 0x4 but is 0x1001",
            "null pointer dereference occurred",
            "null reference produced",
            "trying to construct an enum from an invalid value 0x7",
            "is_nonoverlapping: `size_of::<T>() * count` overflows a usize",
            "std::process::exit called re-entrantly",
            // The compiler's own checks in `mem::zeroed` and `mem::uninitialized`, in every build.
            "attempted to zero-initialize type `&u8`, which is invalid",
            "attempted to leave type `&u8` uninitialized, which is invalid",
            "attempted to instantiate uninhabited type `core::convert::Infallible`",
        ] {
            assert!(crash_ends_the_app(Some("Background 2"), Some(said)), "{said}");
        }
        assert!(!crash_ends_the_app(Some("Scan 1"), Some("index out of bounds")));
        // Known by how they start, or by the whole of what they say.
        assert!(!crash_ends_the_app(Some("Scan 1"), Some("bad header: unsafe precondition(s) violated: ")));
        assert!(!crash_ends_the_app(Some("Scan 1"), Some("null pointer dereference occurred twice")));
        assert!(!crash_ends_the_app(Some("Background 3"), None));
    }

    /// Set for the copy of this test program that crashes in a `Drop` (see the test below).
    const IN_THE_COPY: &str = "MO_TEST_CRASH_IN_A_DROP";
    /// What the copy prints before each crash's message, so they're known among the test program's own lines.
    const SAID: &str = "The crash said: ";

    /// Only in the copy that the test below runs; in an ordinary run it does nothing. A crash in a `Drop` while an
    /// earlier crash is being cleaned up, which std can't recover from: each crash's message is printed, and then std
    /// ends the copy.
    #[test]
    fn crash_in_a_drop_while_unwinding_in_a_copy() {
        if std::env::var_os(IN_THE_COPY).is_none() {
            return;
        }
        std::panic::set_hook(Box::new(|info| {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{SAID}{}", info.payload_as_str().unwrap_or("(nothing)"));
            let _ = out.flush();
        }));
        struct CrashesWhileUnwinding;
        impl Drop for CrashesWhileUnwinding {
            fn drop(&mut self) {
                if std::thread::panicking() {
                    panic!("the second crash, in a drop");
                }
            }
        }
        let _crashes = CrashesWhileUnwinding;
        panic!("the first crash");
    }

    /// What std says when it ends the app, checked on the Rust that builds it (the list above is only words): a copy of
    /// this test program crashes in a `Drop` while unwinding, and prints what each crash said. On Windows the system
    /// ends the copy as the second crash leaves its `Drop`, so std says nothing more there (see [`ENDS_THE_APP`]).
    #[test]
    fn what_std_says_as_a_crash_in_a_drop_ends_the_app_is_known() {
        let copy = std::process::Command::new(std::env::current_exe().expect("this test program"))
            .args(["--exact", "log::tests::crash_in_a_drop_while_unwinding_in_a_copy", "--nocapture"])
            .arg("--test-threads=1")
            .env(IN_THE_COPY, "1")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("the copy ran");
        let out = String::from_utf8_lossy(&copy.stdout);
        assert!(!copy.status.success(), "std ended the copy: {out}");
        // The first may follow the test program's own "test … " on its line.
        let said: Vec<&str> = out.lines().filter_map(|l| l.split_once(SAID).map(|(_, said)| said)).collect();
        assert!(said.contains(&"the first crash") && said.contains(&"the second crash, in a drop"), "{out}");
        let ending = said.iter().filter(|m| crash_ends_the_app(Some("Scan 1"), Some(m))).count();
        let std_says_so = usize::from(!cfg!(target_env = "msvc"));
        assert_eq!(
            ending, std_says_so,
            "std's own words as it ends the app are known, and only those: {said:?}, {}",
            copy.status
        );
    }

    /// Set for the copy of this test program that panics where nothing can catch it (see the test below): how.
    const CANT_UNWIND_IN_THE_COPY: &str = "MO_TEST_PANIC_THAT_CANT_UNWIND";

    /// Only in the copy that the test below runs; in an ordinary run it does nothing. A panic std can't go on from: out
    /// of an `extern "C"` function, or a value of zeroes its type doesn't allow. What each panic said, and whether it
    /// could unwind, is printed, and then std ends the copy.
    #[test]
    #[allow(unsafe_code, invalid_value)]
    fn a_panic_that_cant_unwind_in_a_copy() {
        let Some(how) = std::env::var_os(CANT_UNWIND_IN_THE_COPY) else { return };
        std::panic::set_hook(Box::new(|info| {
            let mut out = std::io::stdout().lock();
            let said = info.payload_as_str().unwrap_or("(nothing)");
            let _ = writeln!(out, "{SAID}{}: {said}", if cannot_unwind(info) { "can't unwind" } else { "unwinds" });
            let _ = out.flush();
        }));
        extern "C" fn crashes() {
            panic!("a crash in an extern \"C\" function");
        }
        if how == "extern" {
            crashes();
        } else {
            // A reference is never all zeroes: the compiler checks for that.
            let _zeroes: &u8 = unsafe { std::mem::zeroed() };
        }
    }

    /// A panic that can't unwind is known as one, as Rust prints it for debugging (see [`cannot_unwind`]), and by its
    /// words too: checked on the Rust that builds the app, in a copy of this test program that std then ends.
    #[test]
    fn a_panic_that_cant_unwind_is_known() {
        for how in ["extern", "zeroed"] {
            let copy = std::process::Command::new(std::env::current_exe().expect("this test program"))
                .args(["--exact", "log::tests::a_panic_that_cant_unwind_in_a_copy", "--nocapture"])
                .arg("--test-threads=1")
                .env(CANT_UNWIND_IN_THE_COPY, how)
                .stdin(std::process::Stdio::null())
                .output()
                .expect("the copy ran");
            let out = String::from_utf8_lossy(&copy.stdout);
            assert!(!copy.status.success(), "std ended the copy: {out}");
            let said: Vec<&str> = out.lines().filter_map(|l| l.split_once(SAID).map(|(_, said)| said)).collect();
            if how == "extern" {
                // The crash itself can unwind; it's std's panic as it reaches the function that can't.
                let ends = [
                    "unwinds: a crash in an extern \"C\" function",
                    "can't unwind: panic in a function that cannot unwind",
                ];
                assert_eq!(said, ends, "{out}");
            } else {
                let [last] = said[..] else { panic!("one panic: {out}") };
                let words = last.strip_prefix("can't unwind: ");
                assert!(words.is_some_and(|w| std_ends_the_app(Some(w))), "known both ways: {out}");
                assert!(last.contains("zero-initialize type `&u8`"), "{out}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_open_session_log_is_locked_on_a_mac_or_linux() {
        let dir = temp_folder("locked");
        let log = SessionLog::new(&dir, Local::now(), "start");
        let path = log.path().expect("a file").to_owned();
        let other = File::open(&path).expect("readable while it's written");
        assert!(matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock)), "locked while it's written");
        log.close();
        assert!(other.try_lock().is_ok(), "free once it's closed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn an_open_session_log_cant_be_removed_on_windows() {
        let dir = temp_folder("in-use");
        let log = SessionLog::new(&dir, Local::now(), "start");
        let path = log.path().expect("a file").to_owned();
        assert!(std::fs::remove_file(&path).is_err(), "nobody can remove it while it's written");
        assert!(std::fs::read_to_string(&path).is_ok(), "anyone can read it");
        log.close();
        // A virus scanner may still be looking at the file for a moment.
        let until = Instant::now() + Duration::from_secs(5);
        let removed = loop {
            match std::fs::remove_file(&path) {
                Ok(()) => break Ok(()),
                Err(_) if Instant::now() < until => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => break Err(e),
            }
        };
        assert!(removed.is_ok(), "removable once it's closed: {removed:?}");
        let _ = std::fs::remove_dir_all(&dir);
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
    fn a_flush_waits_until_the_lines_written_so_far_are_on_disk() {
        let dir = temp_folder("flush");
        let log = SessionLog::new(&dir, Local::now(), "start");
        // A long line first, so the log's writer takes a while to reach the crash after it.
        log.write(Level::Info, "Test", &"x".repeat(3 * 1024 * 1024));
        crash_at(&log, "src/x.rs:1", "the crash");
        log.flush();
        // Looked at straight away: a flush that didn't wait would usually return before the writer got there (the next
        // test shows the flush waits for the writer's answer, whatever the timing).
        let path = log.path().expect("a file").to_path_buf();
        let size = usize::try_from(std::fs::metadata(&path).expect("there").len()).expect("small");
        log.close();
        let bytes = std::fs::read(&path).expect("readable");
        let on_disk = String::from_utf8_lossy(&bytes[..size.min(bytes.len())]);
        assert!(on_disk.contains("[Crash] the crash"), "on disk by the time the flush returned");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_flush_waits_until_the_writer_says_the_lines_are_out() {
        // A log whose writer is this test: the flush is answered only when the test answers it, whatever the timing.
        let (tx, rx) = channel();
        let log = SessionLog {
            path: None,
            tx: Mutex::new(Some(tx)),
            state: Mutex::new(State { seen: HashMap::new(), recent: VecDeque::new() }),
            writer: Mutex::new(None),
            pruning: Mutex::new(Pruning::Over),
            offset: AtomicI32::new(0),
        };
        std::thread::scope(|s| {
            let flushing = s.spawn(|| log.flush());
            let Ok(Msg::Flush(done)) = rx.recv_timeout(Duration::from_secs(20)) else {
                panic!("no flush was asked for")
            };
            std::thread::sleep(Duration::from_millis(300));
            assert!(!flushing.is_finished(), "the flush waits for the writer's answer");
            let _ = done.send(());
            flushing.join().expect("the flush didn't crash");
        });
    }

    #[test]
    fn a_line_written_while_a_thread_crashes_never_waits_long_for_the_log() {
        let dir = temp_folder("unwinding");
        let log = std::sync::Arc::new(SessionLog::new(&dir, Local::now(), "start"));
        // Writes a line as the thread it's on unwinds after a crash (as a save's end does).
        struct WritesAsItGoes(std::sync::Arc<SessionLog>);
        impl Drop for WritesAsItGoes {
            fn drop(&mut self) {
                self.0.write(Level::Error, "Note", "written as the thread unwinds");
            }
        }
        // Something else holds the log's counts the whole time.
        let held = log.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (done, finished) = channel();
        let writer = std::sync::Arc::clone(&log);
        std::thread::spawn(move || {
            let crashed = mo_catch::catch_crash(std::panic::AssertUnwindSafe(|| {
                let _writes = WritesAsItGoes(writer);
                panic!("a crash in a test, on purpose");
            }));
            let _ = done.send(crashed.is_err());
        });
        let returned = finished.recv_timeout(Duration::from_secs(20));
        drop(held);
        log.close();
        assert_eq!(returned, Ok(true), "it didn't wait for the lock");
        let text = std::fs::read_to_string(log.path().expect("a file")).expect("readable");
        assert!(text.contains("[Note] written as the thread unwinds"), "and it was written: {text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn powers_of_ten() {
        assert!(is_power_of_ten(10) && is_power_of_ten(100) && is_power_of_ten(1));
        assert!(!is_power_of_ten(4) && !is_power_of_ten(20) && !is_power_of_ten(101));
    }
}
