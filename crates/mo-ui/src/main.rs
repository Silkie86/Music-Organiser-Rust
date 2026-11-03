// No console window behind the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

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
    static CRASH_FILES: log::CrashFiles = log::CrashFiles::new();
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let current = std::thread::current();
        let thread = current.name().unwrap_or("unnamed");
        let trace = std::backtrace::Backtrace::force_capture();
        let text =
            format!("Music Organiser {} crashed on thread {thread}\n{info}\n\n{trace}\n", mo_app::version::version());
        let place = info.location().map_or_else(|| "unknown".to_owned(), |l| format!("{}:{}", l.file(), l.line()));
        log::crash(&place, &text);
        // A crash on the window's thread ends the app, and so does one std can't recover from, on any thread.
        let fatal = log::crash_ends_the_app(current.name(), info.payload_as_str());
        // The log's own writer can't wait for itself to write the crash out.
        if thread != log::WRITER_THREAD
            && let Some(l) = log::current()
        {
            l.flush();
        }
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = log::write_crash_file(&mo_app::paths::logs_dir(), &text, secs, &place, fatal, &CRASH_FILES);
        // The words of an open F12 note are kept for next time; after the crash itself is safely written.
        mo_ui::note::after_crash(fatal);
        default(info);
    }));
}
