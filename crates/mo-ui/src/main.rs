// No console window behind the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Write;

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
    let session = log::start_session();
    log_crashes();
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
    mo_ui::space::connect(&app, library.clone(), feed.clone(), &shortcuts);
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
        log::error("App", &format!("The window stopped with an error: {e}"));
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
