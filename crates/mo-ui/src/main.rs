// No console window behind the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Write;

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
    let session = log::start_session();
    log_crashes();
    let feed = Feed::default();
    // The settings come from library.db once mo-library exists (phase 2); until then the app opens in its defaults.
    let app = match mo_ui::new_app(
        &Look::from_settings(None, None),
        Shortcuts::from_setting(None),
        Place::Songs,
        feed.clone(),
    ) {
        Ok(app) => app,
        Err(e) => {
            // Rule 17: the reason the window couldn't open reaches the log too.
            log::error("App", &format!("The window couldn't open: {e}"));
            session.close();
            return Err(e);
        }
    };
    // The Songs page reads the C# app's library, read-only (CLAUDE.md: until the Rust page is trusted).
    mo_ui::songs::connect(&app, mo_app::paths::data_dir().join("library.db"), feed.clone());
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
    session.close();
    ran
}

/// Rule 17, crashes always log: a panic anywhere goes into the session log, with where it happened, and is written out
/// before the app closes. It also gets its own `logs\crash-<time>.log`, in case the session log couldn't be written.
fn log_crashes() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current().name().unwrap_or("unnamed").to_owned();
        let trace = std::backtrace::Backtrace::force_capture();
        let text =
            format!("Music Organiser {} crashed on thread {thread}\n{info}\n\n{trace}\n", mo_app::version::version());
        log::error("Crash", &text);
        // The window runs on the main thread: only a crash there ends the app.
        mo_ui::note::after_crash(std::thread::current().name() == Some("main"));
        if let Some(l) = log::current() {
            l.flush();
        }
        let dir = mo_app::paths::logs_dir();
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        if std::fs::create_dir_all(&dir).is_ok()
            && let Ok(mut f) = std::fs::File::create(dir.join(format!("crash-{secs}.log")))
        {
            let _ = f.write_all(text.as_bytes());
        }
        default(info);
    }));
}
