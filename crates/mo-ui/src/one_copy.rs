//! One copy of the app at a time, the window's side ([`mo_app::one_copy`], the 14 Nov review's DF-1): the claim made
//! at start, and what the window does when another copy or the helper's icon asks it to show or to close.

use std::sync::Arc;

use mo_app::log;
use mo_app::one_copy::{self, Asked, CLOSE_WAIT, Claim, OneCopy, PORT};
use slint::ComponentHandle;

/// Takes the app's place as the open copy: see [`mo_app::one_copy::claim`]. Before the window and the settings.
pub fn claim() -> Claim {
    let exe = std::env::current_exe().map(|e| e.display().to_string()).unwrap_or_default();
    one_copy::claim(PORT, &mo_app::version::version(), &exe, CLOSE_WAIT, let_come_to_front)
}

/// Lets the process `pid` put its window in front, on Windows: only the program in front (this one, just started by
/// a click) can pass that on.
#[allow(unsafe_code)]
pub fn let_come_to_front(pid: u32) {
    #[cfg(windows)]
    {
        // SAFETY: a plain call with a process id; one that's gone is refused, not a fault.
        let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid) };
    }
    #[cfg(not(windows))]
    let _ = pid;
}

/// A line for the session log on how the claim went, once the log has started.
pub fn log_claim(claim: &Claim) {
    match claim {
        Claim::Ours { closed: None, .. } => log::info("One copy", "This is the open copy"),
        Claim::Ours { closed: Some(other), .. } => {
            log::info("One copy", &format!("A copy from another build ({other}) was open: it closed for this one"));
        }
        Claim::Shown => {}
        Claim::Unguarded(why) => {
            log::warning("One copy", &format!("Can't tell whether another copy is open: {why}. Opening anyway"));
        }
    }
}

/// From now on the window comes to the front when asked to show, and closes when asked to close (the event loop
/// ends, so everything waiting to be written goes out as when the window is closed by hand).
pub fn connect(app: &crate::AppWindow, held: &Arc<OneCopy>) {
    let weak = app.as_weak();
    held.on_asked(move |asked| {
        let weak = weak.clone();
        let sent = slint::invoke_from_event_loop(move || match asked {
            Asked::Show => {
                if let Some(app) = weak.upgrade() {
                    show(&app);
                }
            }
            Asked::Close => {
                log::last(log::Level::Info, "One copy", "Closing for a copy from another build");
                crate::watchdog::stop();
                let _ = slint::quit_event_loop();
            }
        });
        if sent.is_err() {
            log::warning("One copy", "The window couldn't be told what another copy asked");
        }
    });
}

/// Brings the window back from the taskbar and to the front.
fn show(app: &crate::AppWindow) {
    use slint::winit_030::WinitWindowAccessor;
    log::info("One copy", "Asked to show the window");
    app.window().set_minimized(false);
    app.window().with_winit_window(|window| window.focus_window());
}
