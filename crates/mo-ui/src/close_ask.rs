//! A close asked of the app from outside always closes it properly (Windows).
//!
//! winit keeps a second window on the window's thread that is never seen: every message from another thread (Slint's
//! `invoke_from_event_loop`, so the watchdog's knocks, every job's result, and the ask to end the loop) is posted to
//! it. To Windows it is an ordinary top-level window, shown and with no owner. A program that asks "the app's main
//! window" to close picks the first such window from the front, and once the real window is minimised (so at the
//! back) that is the unseen one. Left alone it takes the close for itself and is destroyed: the real window never
//! hears of it, and from then on nothing posted reaches the window's thread, which sits waiting for ever. The launcher
//! (`CloseMainWindow` in `tools/dev/run-latest.ps1`) did just that to a minimised app on 12 to 14 Nov 2026: five
//! sessions ended "still stuck after 5.0 s" and were forced shut, with nothing of what is written at the end.
//!
//! [`connect`] has the unseen window end the event loop instead, the way closing the real window does, so everything
//! waiting to be written goes out.

/// From now on a close asked of winit's unseen window closes the app. Call it on the window's thread, once the window
/// is made. Nothing to do on Mac and Linux.
pub fn connect() {
    #[cfg(windows)]
    if !on_windows::guard_class(on_windows::WINIT_UNSEEN, close) {
        mo_app::log::warning(
            "App",
            "Couldn't find winit's unseen window: a close asked of a minimised app may not close it",
        );
    }
}

#[cfg(windows)]
fn close() {
    mo_app::log::last(mo_app::log::Level::Info, "App", "Asked to close from outside while out of sight: closing");
    crate::watchdog::stop();
    let _ = slint::quit_event_loop();
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod on_windows {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId, WM_CLOSE, WM_NCDESTROY};
    use windows::core::{PCWSTR, w};

    /// The class of winit's unseen window (winit 0.30, `THREAD_EVENT_TARGET_WINDOW_CLASS`).
    pub(super) const WINIT_UNSEEN: PCWSTR = w!("Winit Thread Event Target");
    /// Our mark on the windows guarded here ("MOCA").
    const OURS: usize = 0x4d4f_4341;

    /// Guards every top-level window of class `class` on this thread; whether there was one.
    pub(super) fn guard_class(class: PCWSTR, on_close: fn()) -> bool {
        // SAFETY: a plain call.
        let this_thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        let mut found = false;
        let mut after: Option<HWND> = None;
        // SAFETY: `class` is a string that ends with a zero; a window gone in between is an error, not a fault.
        while let Ok(window) = unsafe { FindWindowExW(None, after, class, PCWSTR::null()) } {
            if window.is_invalid() {
                break;
            }
            // SAFETY: as above.
            if unsafe { GetWindowThreadProcessId(window, None) } == this_thread {
                found |= guard(window, on_close);
            }
            after = Some(window);
        }
        found
    }

    /// From now on `window`, which belongs to this thread, calls `on_close` when asked to close, and stays.
    pub(super) fn guard(window: HWND, on_close: fn()) -> bool {
        // SAFETY: `window` is this thread's (Windows refuses another's); `asked` has the shape Windows calls, and
        // the number kept with it is the plain function `on_close`, which lives as long as the program.
        unsafe { SetWindowSubclass(window, Some(asked), OURS, on_close as usize) }.as_bool()
    }

    /// What a guarded window does with each message: a close calls the function kept with it; the rest goes on to the
    /// window as before.
    unsafe extern "system" fn asked(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _mark: usize,
        on_close: usize,
    ) -> LRESULT {
        if message == WM_CLOSE {
            // SAFETY: `on_close` is the `fn()` [`guard`] put there, and nothing else sets it.
            let on_close = unsafe { std::mem::transmute::<usize, fn()>(on_close) };
            on_close();
            return LRESULT(0);
        }
        if message == WM_NCDESTROY {
            // SAFETY: the window is going: our hold on it is let go first, as Windows asks.
            let _ = unsafe { RemoveWindowSubclass(window, Some(asked), OURS) };
        }
        // SAFETY: the message is passed on untouched to the window's own handling.
        unsafe { DefSubclassProc(window, message, wparam, lparam) }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, IsWindow, SendMessageW, WINDOW_EX_STYLE, WINDOW_STYLE,
        };

        use super::*;

        /// A small top-level window on this thread, of Windows' own "Static" class.
        fn window() -> HWND {
            // SAFETY: a plain window of a class Windows has, with no parent, menu or data.
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("Static"),
                    PCWSTR::null(),
                    WINDOW_STYLE(0),
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .expect("a window")
        }

        fn close(window: HWND) {
            // SAFETY: a message to this thread's own window.
            unsafe { SendMessageW(window, WM_CLOSE, None, None) };
        }

        fn there(window: HWND) -> bool {
            // SAFETY: a plain question; a window that's gone is "no".
            unsafe { IsWindow(Some(window)) }.as_bool()
        }

        #[test]
        fn a_close_asked_of_a_guarded_window_is_passed_on_and_the_window_stays() {
            static ASKED: AtomicUsize = AtomicUsize::new(0);
            fn note() {
                ASKED.fetch_add(1, Ordering::SeqCst);
            }
            // What went wrong: a window left alone takes the close for itself and is gone.
            let alone = window();
            close(alone);
            assert!(!there(alone));

            // Found by its class on this thread, as winit's is, and guarded.
            let guarded = window();
            assert!(guard_class(w!("Static"), note));
            close(guarded);
            close(guarded);
            assert_eq!(ASKED.load(Ordering::SeqCst), 2);
            assert!(there(guarded));
            // SAFETY: this thread's own window.
            unsafe { DestroyWindow(guarded) }.expect("destroyed");

            // No such window on this thread: said, not guessed.
            assert!(!guard_class(w!("Static"), note));
        }
    }
}
