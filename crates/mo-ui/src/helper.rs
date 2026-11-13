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
    leave_console();
    lower_process_priority();
    let started = match Helper::start(helper::PORT, mo_dawlink::bitwig::PORT) {
        Ok(Start::Started(h)) => h,
        // Before its log starts, so a helper started twice (at login and by the app) leaves no empty logs.
        Ok(Start::AlreadyRunning) if helper::hello(helper::PORT).is_some() => return,
        Ok(Start::AlreadyRunning) => {
            log::start_helper();
            log::error(
                "Helper",
                &format!("Port {} is taken by something that doesn't answer as the helper: not starting", helper::PORT),
            );
            close_log();
            return;
        }
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

/// What [`apply`] does when it's done: told how it went, in words.
type Then = Box<dyn FnOnce(String) + Send>;

/// The one thread that starts and stops the helper, a choice at a time, the newest winning: two quick clicks can't
/// finish the wrong way round.
static APPLY: std::sync::OnceLock<Option<mo_app::background::Newest<(bool, Then)>>> = std::sync::OnceLock::new();

/// Does what the Settings choice says, on a low-priority thread (it may wait for an older helper to quit), then
/// hands `then` how it went, in words, also written to the log. `on`: make sure the helper starts at login and runs
/// now, from this build. Off: take it out of the startup list and ask it to quit. A choice made while another is
/// still being done waits its turn, and only the newest waiting one is done.
pub fn apply(on: bool, then: impl FnOnce(String) + Send + 'static) {
    let worker = APPLY.get_or_init(|| {
        mo_app::background::Newest::start("Helper: start or stop", |(on, then): (bool, Then)| then(apply_now(on)))
            .map_err(|e| log::warning("Helper", &format!("Couldn't start the thread that starts the helper: {e}")))
            .ok()
    });
    match worker {
        Some(worker) => worker.ask((on, Box::new(then))),
        None => then("The helper couldn't be started or stopped: the app couldn't start a thread for it".into()),
    }
}

fn apply_now(on: bool) -> String {
    use mo_app::helper_start::{self, SystemList};
    let done = if on {
        match std::env::current_exe() {
            Ok(exe) => helper_start::turn_on(
                &SystemList,
                helper::PORT,
                &exe,
                &mo_app::version::version(),
                helper_start::start_detached,
            ),
            Err(e) => {
                let line = format!("The helper couldn't be started: the app can't tell where it runs from ({e})");
                log::warning("Helper", &line);
                return line;
            }
        }
    } else {
        helper_start::turn_off(&SystemList, helper::PORT)
    };
    let line = done.line();
    if done.login_problem.is_some() || matches!(done.running, helper_start::Running::Failed(_)) {
        log::warning("Helper", &line);
    } else {
        log::info("Helper", &line);
    }
    line
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

/// A test build has a console window: one Windows opened for the helper alone (at login) is let go, so no window
/// stays open for it. A release build has none.
fn leave_console() {
    #[cfg(windows)]
    #[allow(unsafe_code)]
    // SAFETY: takes no arguments; the helper writes nothing to a console.
    unsafe {
        windows_sys::Win32::System::Console::FreeConsole();
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
