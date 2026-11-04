// No console window behind the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};

use mo_app::activity::Feed;
use mo_app::background::Pace;
use mo_app::log;
use mo_app::shortcuts::Shortcuts;
use slint::ComponentHandle;

fn main() -> Result<(), slint::PlatformError> {
    // Started by the Space page as the administrator step: do it and stop, before any window, log or setting. This also
    // tells the Space page it may ask Windows for administrator rights.
    if mo_space::admin::run_step(&std::env::args().skip(1).collect::<Vec<_>>()) {
        return Ok(());
    }
    // Before the log starts, so a crash while it starts still gets its crash file.
    log_crashes();
    let session = log::start_session();
    // Rule 28: what an earlier run or a stopped tool left behind is cleared, a little later, at low priority.
    mo_app::tidy::start();
    let feed = Feed::default();
    // This app's own settings over the C# app's, which library.db keeps and the app only reads.
    let data = mo_app::paths::data_dir();
    let library = data.join("library.db");
    let (settings, writable) = mo_ui::settings::load(&data, &library);
    let shortcuts = Shortcuts::from_setting(settings.get(mo_app::shortcuts::SETTING_KEY));
    // Settings' pace: how many low-priority threads background work gets, from now until the app closes.
    let pace = Pace::from_setting(settings.get(mo_app::app_settings::PACE_KEY));
    mo_app::background::set_pace(pace);
    log::info(
        "Settings",
        &format!("Pace {}: {} background threads", pace.name(), pace.threads(mo_app::background::cores())),
    );
    // The graphics card draws the window and the galaxy; without one, the processor does.
    mo_ui::gpu::choose();
    let look = mo_ui::settings::look_of(&settings);
    let start = mo_app::app_settings::start_place(&settings);
    let app = match mo_ui::new_app(&look, shortcuts.clone(), start, feed.clone()) {
        Ok(app) => app,
        Err(e) => {
            // Rule 17: the reason the window couldn't open reaches the log too.
            log::error("App", &format!("The window couldn't open: {e}"));
            session.close();
            return Err(e);
        }
    };
    mo_ui::gpu::connect(&app);
    // The Songs page reads the C# app's library, read-only (CLAUDE.md: until the Rust page is trusted).
    mo_ui::songs::connect(&app, library.clone(), feed.clone());
    mo_ui::sounds::connect(
        &app,
        mo_app::paths::data_dir().join("sounds.db"),
        library.clone(),
        feed.clone(),
        &shortcuts,
    );
    // MIDI: the MIDI index beside the library, read-only (the C# app's midi.db until the Rust one holds parts).
    mo_ui::midi::connect(&app, mo_app::paths::data_dir().join("library.db"), feed.clone(), &shortcuts);
    // Space, Backup: the backup settings live in a file of their own; the library is only read.
    mo_ui::backup::connect(&app, library.clone(), feed.clone());
    // Settings: changes are kept in the app's own file, never in library.db.
    mo_ui::settings::connect(
        &app,
        mo_ui::settings::Setup {
            settings,
            folder: data,
            library: library.clone(),
            running_pace: pace,
            live: true,
            writable,
            feed: feed.clone(),
        },
    );
    // Presets: the library is only read; part fixes and curves go in the app's own presets.db and curves.db.
    mo_ui::presets::connect(&app, library.clone(), feed.clone(), &shortcuts);
    // Space: the drives your folders are on and the app's own files, looked at once the page shows.
    mo_ui::space::connect(&app, library.clone(), feed.clone());
    app.window().set_maximized(true);
    // The watchdog starts once the window is up, so opening it doesn't count as a freeze.
    let watched = feed.clone();
    let sized = app.as_weak();
    if slint::invoke_from_event_loop(move || {
        // The size of everything, once the window knows the screen's own scale.
        if let Some(app) = sized.upgrade() {
            mo_ui::settings::apply_size(&app);
        }
        mo_ui::watchdog::start(watched);
    })
    .is_err()
    {
        log::warning("Freeze", "Couldn't start the freeze watchdog");
    }
    let ran = app.run();
    if let Err(e) = &ran {
        // A line of how the run ends: it shows even once the log is full.
        log::last(log::Level::Error, "App", &format!("The window stopped with an error: {e}"));
    }
    // A settings change still waiting to be written goes now.
    mo_ui::settings::finish_up();
    // A note left open keeps its draft for next time, but the launcher shouldn't wait on it any more.
    mo_ui::note::wait_for_saves();
    mo_ui::note::finish_up();
    // The sweep's and the frames' last few seconds, before the log closes.
    mo_app::sweep_timing::finish();
    mo_app::frame_timing::finish();
    session.close();
    ran
}

/// Rule 17, crashes always log: a panic anywhere goes into the session log, with where it happened, and is written out
/// before the app closes. It also gets its own `logs\crash-<time>.log`, in case the session log couldn't be written
/// (one for each place in the code that crashed, a few places a run, so a crash that keeps happening can't fill the
/// folder; the crash that ends the app always gets its file; rule 28). Nothing in here may crash itself: std would end
/// the app on the spot.
fn log_crashes() {
    set_crash_hook(mo_app::paths::logs_dir, mo_ui::note::after_crash, flush_the_log, || {});
}

/// Puts in the crash handler: what the app does with a crash ([`on_crash`], into the logs folder `logs` gives, with
/// `keep_note` and `flush`), then `after` (nothing in the app; a test counts the crashes), then std's own report of it,
/// on the console when there is one. Std's report comes last: it can wait for ever on a console nobody reads.
fn set_crash_hook(
    logs: impl Fn() -> PathBuf + Send + Sync + 'static,
    keep_note: impl Fn(bool) + Send + Sync + 'static,
    flush: impl Fn() + Send + Sync + 'static,
    after: impl Fn() + Send + Sync + 'static,
) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        on_crash(info, &logs(), &keep_note, &flush);
        after();
        default(info);
    }));
}

/// The places in the code this run has written a crash log for.
static CRASH_FILES: log::CrashFiles = log::CrashFiles::new();

/// Waits until what the session log was given so far is on disk (a moment at most).
fn flush_the_log() {
    if let Some(l) = log::current() {
        l.flush();
    }
}

/// What the app does with a crash, before std's own report of it: the crash goes into the session log (`flush` waits
/// for it to be on disk) and into a crash log of its own in `logs`, and the words of an open F12 note are kept
/// (`keep_note`, told whether the crash ends the app). What the crash says goes to disk first, everywhere, and its
/// backtrace only after: taking a backtrace waits for a lock of std's that std holds while it reports another thread's
/// crash on the console, and that can wait for ever (a console that's paused, or a pipe nobody reads). Nothing else in
/// here waits long.
fn on_crash(info: &std::panic::PanicHookInfo<'_>, logs: &Path, keep_note: &dyn Fn(bool), flush: &dyn Fn()) {
    let current = std::thread::current();
    let thread = current.name().unwrap_or("unnamed");
    let place = info.location().map_or_else(|| "unknown".to_owned(), |l| format!("{}:{}", l.file(), l.line()));
    // A crash on the window's thread ends the app, and so does one std can't recover from, on any thread.
    let fatal = log::crash_ends_the_app(current.name(), info.payload_as_str());
    let text = format!("Music Organiser {} crashed on thread {thread}\n{info}\n", mo_app::version::version());
    log::crash(&place, &text, fatal);
    // Out to disk now. The log's own writer can't wait for itself to write the crash out.
    let on_disk = || {
        if thread != log::WRITER_THREAD {
            flush();
        }
    };
    on_disk();
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let file = log::write_crash_file(logs, &text, secs, &place, fatal, &CRASH_FILES);
    keep_note(fatal);
    // Then where in the code it came from. Written out rather than formatted: a backtrace that can't be written in
    // full mustn't crash the crash handler.
    let mut trace = String::new();
    let _ = std::fmt::Write::write_fmt(&mut trace, format_args!("{}", std::backtrace::Backtrace::force_capture()));
    if let Some(file) = &file {
        log::add_to_crash_file(file, &format!("\n{trace}\n"));
    }
    log::crash_backtrace(&place, &format!("Backtrace of the crash on thread {thread}:\n{trace}"), fatal);
    on_disk();
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::*;

    /// Set for the copy of this test program that crashes while the console is blocked: the folder it logs into.
    const IN_THE_COPY: &str = "MO_TEST_CRASH_WHILE_THE_CONSOLE_WAITS";
    /// The file the copy writes what its crash handler did in, a line each time, in order: "<thread> flushed, on disk:
    /// <true or false>" once the session log was flushed (whether it then held that thread's crash), and "<thread> kept
    /// the note, ends the app: <true or false>, crash log: <true or false>" from its stand-in for keeping an F12 note
    /// (whether that thread's crash log was there by then). The copy itself adds "a backtrace could be taken: <true or
    /// false>" once it has tried to take one while std's report of the first crash waits.
    const DONE: &str = "what the crash handler did.txt";

    /// Adds `line` to [`DONE`] in `folder`, in one go, so two threads crashing at once can't mix their lines.
    fn did(folder: &Path, line: &str) {
        use std::io::Write;
        let file = std::fs::OpenOptions::new().create(true).append(true).open(folder.join(DONE));
        if let Ok(mut f) = file {
            let _ = f.write_all(format!("{line}\n").as_bytes());
        }
    }

    /// The name of the thread this runs on.
    fn this_thread() -> String {
        std::thread::current().name().unwrap_or("unnamed").to_owned()
    }

    /// A file in `folder` that `is_it` names, and that says the crash on the thread this runs on is in it.
    fn holds_this_crash(folder: &Path, is_it: fn(&str) -> bool) -> bool {
        let said = format!("crashed on thread {}\n", this_thread());
        std::fs::read_dir(folder).into_iter().flatten().flatten().any(|entry| {
            is_it(&entry.file_name().to_string_lossy())
                && std::fs::read_to_string(entry.path()).is_ok_and(|text| text.contains(&said))
        })
    }

    /// Only in the copy that the test below runs; in an ordinary run it does nothing. The console (a pipe the test
    /// never reads) is filled, so std's report of a first crash waits there for ever, holding its backtrace lock; then
    /// two more threads crash, one of them called "main" as the window's thread is, so it's taken for a crash that ends
    /// the app. Last, it writes down whether a backtrace can be taken then (it can't, while that lock is held).
    #[test]
    fn crash_while_the_console_waits_in_a_copy() {
        use std::io::Write;
        let Some(dir) = std::env::var_os(IN_THE_COPY).map(PathBuf::from) else { return };
        let _log = log::start(&dir, "A copy of the test program, to crash in");
        static HANDLED: AtomicUsize = AtomicUsize::new(0);
        // The app's own crash handler, with stand-ins that write down what it did, and when.
        let (logs, kept, flushed) = (dir.clone(), dir.clone(), dir.clone());
        let keep_note = move |fatal: bool| {
            let crash_log = holds_this_crash(&kept, log::is_crash_log);
            did(&kept, &format!("{} kept the note, ends the app: {fatal}, crash log: {crash_log}", this_thread()));
        };
        let flush = move || {
            flush_the_log();
            let on_disk = holds_this_crash(&flushed, log::is_session_log);
            did(&flushed, &format!("{} flushed, on disk: {on_disk}", this_thread()));
        };
        set_crash_hook(
            move || logs.clone(),
            keep_note,
            flush,
            || {
                HANDLED.fetch_add(1, Ordering::SeqCst);
            },
        );
        // Fills the console, and waits there for ever.
        std::thread::spawn(|| std::io::stderr().write_all("x".repeat(4 * 1024 * 1024).as_bytes()));
        std::thread::sleep(Duration::from_millis(300));
        let _ = std::thread::Builder::new().name("Crash A".into()).spawn(|| panic!("the first crash"));
        // Once the first crash is handled, std's report of it waits on the console, holding the backtrace lock.
        let until = Instant::now() + Duration::from_secs(100);
        while HANDLED.load(Ordering::SeqCst) == 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(300));
        let _ = std::thread::Builder::new().name("Crash B".into()).spawn(|| panic!("the second crash"));
        let _ = std::thread::Builder::new().name("main".into()).spawn(|| panic!("the crash that ends the app"));
        // What the test stands on: a backtrace can't be taken while std's report of the first crash waits.
        std::thread::sleep(Duration::from_secs(1));
        static TAKEN: AtomicBool = AtomicBool::new(false);
        std::thread::spawn(|| {
            let _ = std::backtrace::Backtrace::force_capture();
            TAKEN.store(true, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_secs(2));
        did(&dir, &format!("a backtrace could be taken: {}", TAKEN.load(Ordering::SeqCst)));
        // The test ends this copy once it has seen what it needs to; if it doesn't, the copy ends by itself.
        std::thread::sleep(Duration::from_secs(180));
    }

    #[test]
    fn a_crash_is_written_before_its_backtrace_is_taken() {
        let dir = std::env::temp_dir().join(format!("mo-ui-crash-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("made");
        let copy = std::process::Command::new(std::env::current_exe().expect("this test program"))
            .args(["--exact", "tests::crash_while_the_console_waits_in_a_copy", "--nocapture", "--test-threads=1"])
            .env(IN_THE_COPY, &dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // Never read, so it fills up.
            .stderr(Stdio::piped())
            .spawn()
            .expect("the copy started");
        // The copy is ended however this test ends, so it never runs on.
        struct Ends(std::process::Child);
        impl Drop for Ends {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut copy = Ends(copy);
        // The session log's text, the crash logs' text, and what the crash handler did, in order.
        let read = |dir: &Path| -> (String, String, Vec<String>) {
            let (mut session, mut crashes) = (String::new(), String::new());
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
                if log::is_session_log(&name) {
                    session += &text;
                } else if log::is_crash_log(&name) {
                    crashes += &text;
                }
            }
            let done = std::fs::read_to_string(dir.join(DONE)).unwrap_or_default();
            // Only whole lines: one may be read as it's being added.
            let whole = done.split_inclusive('\n').filter(|l| l.ends_with('\n'));
            (session, crashes, whole.map(|l| l.trim_end_matches('\n').to_owned()).collect())
        };
        // The crashes, and whether each ends the app.
        let threads = [("Crash A", false), ("Crash B", false), ("main", true)];
        let until = Instant::now() + Duration::from_secs(120);
        let (seen, (session, crashes, done)) = loop {
            let found = read(&dir);
            let (session, crashes, done) = &found;
            // The first crash was handled in full, and std's report of it waits on the console: the other two are
            // written, and their notes kept, though their backtraces wait.
            let written = |t: &str| {
                let said = format!("crashed on thread {t}\n");
                session.contains(&said) && crashes.contains(&said)
            };
            let seen = session.contains("[Crash] Backtrace of the crash on thread Crash A:")
                && written("Crash B")
                && written("main")
                && threads.iter().all(|(t, _)| done.iter().any(|l| l.starts_with(&format!("{t} kept the note"))))
                && done.iter().any(|l| l.starts_with("a backtrace could be taken"));
            let ended = copy.0.try_wait().ok().flatten().is_some();
            if seen || ended || Instant::now() >= until {
                break (seen, found);
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        drop(copy);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(seen, "the later crashes are written before their backtraces are taken:\n{session}\n---\n{crashes}");
        // Otherwise nothing waited, and the test shows nothing.
        assert!(done.iter().any(|l| l == "a backtrace could be taken: false"), "the console held std's lock: {done:?}");
        // Each crash was on disk in the session log, and in its crash log, before its note was kept, and the note was
        // told whether the crash ends the app.
        for (thread, ends) in threads {
            let at = |line: String| done.iter().position(|l| *l == line);
            let flushed = at(format!("{thread} flushed, on disk: true"));
            let kept = at(format!("{thread} kept the note, ends the app: {ends}, crash log: true"));
            assert!(matches!((flushed, kept), (Some(f), Some(k)) if f < k), "{thread}: {done:?}");
        }
    }
}
