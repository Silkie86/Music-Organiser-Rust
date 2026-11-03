// No console window behind the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::Path;

use mo_app::activity::Feed;
use mo_app::log;
use mo_app::places::Place;
use mo_app::shortcuts::Shortcuts;
use mo_app::themes::Look;
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
    // The settings come from library.db once mo-library exists (phase 2); until then the app opens in its defaults.
    let shortcuts = Shortcuts::from_setting(None);
    // The graphics card draws the window and the galaxy; without one, the processor does.
    mo_ui::gpu::choose();
    let app = match mo_ui::new_app(&Look::from_settings(None, None), shortcuts.clone(), Place::Songs, feed.clone()) {
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
    mo_ui::songs::connect(&app, mo_app::paths::data_dir().join("library.db"), feed.clone());
    mo_ui::sounds::connect(
        &app,
        mo_app::paths::data_dir().join("sounds.db"),
        mo_app::paths::data_dir().join("library.db"),
        feed.clone(),
        &shortcuts,
    );
    // Space, Backup: the backup settings live in a file of their own; the library is only read.
    mo_ui::backup::connect(&app, mo_app::paths::data_dir().join("library.db"));
    app.window().set_maximized(true);
    // The watchdog starts once the window is up, so opening it doesn't count as a freeze.
    let watched = feed.clone();
    if slint::invoke_from_event_loop(move || mo_ui::watchdog::start(watched)).is_err() {
        log::warning("Freeze", "Couldn't start the freeze watchdog");
    }
    let ran = app.run();
    if let Err(e) = &ran {
        log::error("App", &format!("The window stopped with an error: {e}"));
    }
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
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        on_crash(info, &mo_app::paths::logs_dir(), &mo_ui::note::after_crash);
        // Then std's own report of it, on the console when there is one.
        default(info);
    }));
}

/// The places in the code this run has written a crash log for.
static CRASH_FILES: log::CrashFiles = log::CrashFiles::new();

/// What the app does with a crash, before std's own report of it: the crash goes into the session log and into a crash
/// log of its own in `logs`, and the words of an open F12 note are kept (`keep_note`, told whether the crash ends the
/// app). What the crash says goes to disk first, everywhere, and its backtrace only after: taking a backtrace waits for
/// a lock of std's that std holds while it reports another thread's crash on the console, and that can wait for ever
/// (a console that's paused, or a pipe nobody reads). Nothing else in here waits long.
fn on_crash(info: &std::panic::PanicHookInfo<'_>, logs: &Path, keep_note: &dyn Fn(bool)) {
    let current = std::thread::current();
    let thread = current.name().unwrap_or("unnamed");
    let place = info.location().map_or_else(|| "unknown".to_owned(), |l| format!("{}:{}", l.file(), l.line()));
    // A crash on the window's thread ends the app, and so does one std can't recover from, on any thread.
    let fatal = log::crash_ends_the_app(current.name(), info.payload_as_str());
    let text = format!("Music Organiser {} crashed on thread {thread}\n{info}\n", mo_app::version::version());
    log::crash(&place, &text, fatal);
    // Out to disk now. The log's own writer can't wait for itself to write the crash out.
    let flush = || {
        if thread != log::WRITER_THREAD
            && let Some(l) = log::current()
        {
            l.flush();
        }
    };
    flush();
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
    flush();
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::*;

    /// Set for the copy of this test program that crashes while the console is blocked: the folder it logs into.
    const IN_THE_COPY: &str = "MO_TEST_CRASH_WHILE_THE_CONSOLE_WAITS";
    /// The file the copy's stand-in for keeping an F12 note writes a line in, for each crash.
    const NOTES_KEPT: &str = "notes kept.txt";

    /// Only in the copy that the test below runs; in an ordinary run it does nothing. The console (a pipe the test
    /// never reads) is filled, so std's report of a first crash waits there for ever, holding its backtrace lock; then
    /// a second thread crashes.
    #[test]
    fn crash_while_the_console_waits_in_a_copy() {
        use std::io::Write;
        let Some(dir) = std::env::var_os(IN_THE_COPY).map(PathBuf::from) else { return };
        let _log = log::start(&dir, "A copy of the test program, to crash in");
        static HANDLED: AtomicUsize = AtomicUsize::new(0);
        let default = std::panic::take_hook();
        let logs = dir.clone();
        std::panic::set_hook(Box::new(move |info| {
            on_crash(info, &logs, &|_: bool| {
                let kept = std::fs::OpenOptions::new().create(true).append(true).open(logs.join(NOTES_KEPT));
                if let Ok(mut f) = kept {
                    let _ = writeln!(f, "kept");
                }
            });
            HANDLED.fetch_add(1, Ordering::SeqCst);
            default(info);
        }));
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
        // The session log's text, the crash logs' text, and how many times an F12 note was kept.
        let read = |dir: &Path| -> (String, String, usize) {
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
            (session, crashes, std::fs::read_to_string(dir.join(NOTES_KEPT)).unwrap_or_default().lines().count())
        };
        let until = Instant::now() + Duration::from_secs(120);
        let (seen, (session, crashes, kept)) = loop {
            let found = read(&dir);
            let (session, crashes, kept) = &found;
            // The first crash was handled in full, and std's report of it waits on the console: the second crash is
            // written, and its note kept, though its backtrace waits.
            let seen = session.contains("[Crash] Backtrace of the crash on thread Crash A:")
                && session.contains("crashed on thread Crash B")
                && crashes.contains("crashed on thread Crash B")
                && *kept == 2;
            let ended = copy.0.try_wait().ok().flatten().is_some();
            if seen || ended || Instant::now() >= until {
                break (seen, found);
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        drop(copy);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(seen, "the second crash is written before its backtrace is taken:\n{session}\n---\n{crashes}\n{kept}");
    }
}
