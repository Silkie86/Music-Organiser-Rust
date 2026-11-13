//! The background helper's own small program (`music-organiser --helper`, see [`mo_app::helper`]): no window and no
//! pages, only the helper, its log and, on Windows, its icon by the clock.

use std::sync::Arc;
use std::time::Duration;

use mo_app::helper::{self, Helper, Start};
use mo_app::log;

#[cfg(windows)]
#[allow(unsafe_code)]
mod tray;

/// How often the helper's log notes a change in whether it has Bitwig.
const LOOK_EVERY: Duration = Duration::from_secs(5);

/// Whether the app was started to be the helper.
pub fn asked(args: &[String]) -> bool {
    args.iter().any(|a| a == helper::FLAG)
}

/// Runs the helper until it's asked to quit (from its icon, the app or the launcher). Returns at once, writing
/// nothing, when another helper is already running.
pub fn run() {
    lower_process_priority();
    let started = match Helper::start(helper::PORT, mo_dawlink::bitwig::PORT) {
        Ok(Start::Started(h)) => h,
        // Before its log starts, so a helper started twice (at login and by the app) leaves no empty logs.
        Ok(Start::AlreadyRunning) => return,
        Err(e) => {
            log::start_helper();
            log::error("Helper", &format!("Couldn't start: {e}"));
            close_log();
            return;
        }
    };
    let session = log::start_helper();
    log::info("Helper", &format!("Answering on port {} (this computer only)", started.port()));
    let watching = Arc::clone(&started);
    if mo_app::background::spawn("Helper: Bitwig state", move || note_changes(&watching)).is_err() {
        log::warning("Helper", "Couldn't start the thread that notes Bitwig's state");
    }
    #[cfg(windows)]
    tray::run(&started);
    #[cfg(not(windows))]
    started.wait(None);
    log::info("Helper", "Closed");
    session.close();
}

/// Notes in the log each change in whether the helper has Bitwig, until it quits.
fn note_changes(helper: &Helper) {
    let mut said = String::new();
    loop {
        let now = helper::bitwig_words(helper.bitwig().as_ref());
        if now != said {
            log::info("Helper", &now);
            said = now;
        }
        if helper.wait(Some(LOOK_EVERY)) {
            return;
        }
    }
}

fn close_log() {
    if let Some(session) = log::current() {
        session.close();
    }
}

/// The whole helper runs below normal priority, so nothing it does can slow Bitwig or the app (rule 21).
fn lower_process_priority() {
    #[cfg(windows)]
    #[allow(unsafe_code)]
    {
        use windows_sys::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, SetPriorityClass};
        // SAFETY: GetCurrentProcess gives a handle that needs no closing; SetPriorityClass only reads it.
        unsafe {
            SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
        }
    }
    mo_app::background::lower_priority();
}
