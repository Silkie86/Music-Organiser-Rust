// No console window behind the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Write;

use mo_app::places::Place;
use mo_app::shortcuts::Shortcuts;
use mo_app::themes::Look;
use slint::ComponentHandle;

fn main() -> Result<(), slint::PlatformError> {
    log_crashes();
    // The settings come from library.db once mo-library exists (phase 2); until then the app opens in its defaults.
    let app = mo_ui::new_app(&Look::from_settings(None, None), Shortcuts::from_setting(None), Place::Songs)?;
    app.window().set_maximized(true);
    app.run()
}

/// Rule 17, crashes always log: a panic anywhere writes `logs\crash-<time>.log` with where it happened, before the
/// app closes. (The session log arrives in the next step of phase 0 and takes this over.)
fn log_crashes() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let dir = mo_app::paths::logs_dir();
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let thread = std::thread::current().name().unwrap_or("unnamed").to_owned();
        let trace = std::backtrace::Backtrace::force_capture();
        let text =
            format!("Music Organiser {} crashed on thread {thread}\n{info}\n\n{trace}\n", env!("CARGO_PKG_VERSION"));
        if std::fs::create_dir_all(&dir).is_ok()
            && let Ok(mut f) = std::fs::File::create(dir.join(format!("crash-{secs}.log")))
        {
            let _ = f.write_all(text.as_bytes());
        }
        default(info);
    }));
}
