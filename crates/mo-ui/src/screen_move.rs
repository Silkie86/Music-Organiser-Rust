//! Moving the window between two screens with different scales (Windows).
//!
//! Solomon's screens (25 Nov 2026): a 3440-wide one at 100% and a 4K one at 150%. Dragging the window from one to the
//! other made it flicker: his log has the screen's scale changing 1, 1.5, 1, 1.5... about 40 times in one second,
//! once each mouse move, before it settled. When a window crosses to a screen with another scale, Windows sends
//! `WM_DPICHANGED` with the place and size it suggests for the new scale, worked out so the window stays under the
//! mouse the way Windows' own drag keeps it. winit 0.30 doesn't use that place: it works out its own size and moves
//! the window to keep the mouse at the same share of its width. Windows' drag then puts the window back by its own
//! reckoning at the next mouse move, which can put most of it on the first screen again: the scale changes back, and
//! so on while the mouse moves.
//!
//! [`connect`] lets winit do all it does for the new scale, then, only while the window is being dragged, puts it
//! where Windows suggested, as Microsoft asks of every program (the `WM_DPICHANGED` page). A scale change while the
//! window isn't being dragged (display settings changed, a screen unplugged) is left to winit as before.

/// From now on a drag across screens with different scales uses Windows' own place for the window. Call it on the
/// window's thread, once the window is up. Nothing to do on Mac and Linux.
pub fn connect(app: &crate::AppWindow) {
    #[cfg(windows)]
    {
        use slint::ComponentHandle;
        if !on_windows::connect(app.window()) {
            mo_app::log::warning(
                "Window",
                "Couldn't watch the window's moves between screens: dragging it to a screen with another scale may \
                 flicker",
            );
        }
    }
    #[cfg(not(windows))]
    let _ = app;
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod on_windows {
    use std::cell::Cell;
    use std::ffi::c_void;

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, WM_DPICHANGED, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_NCDESTROY,
    };

    /// Our mark on the window ("MOSM").
    const OURS: usize = 0x4d4f_534d;

    thread_local! {
        /// Whether the window is being dragged or sized by the user now (Windows' move-and-size loop).
        static DRAGGED: Cell<bool> = const { Cell::new(false) };
    }

    pub(super) fn connect(window: &slint::Window) -> bool {
        let handle = window.window_handle();
        let Some(own) = (match HasWindowHandle::window_handle(&handle).ok().map(|h| h.as_raw()) {
            Some(RawWindowHandle::Win32(h)) => Some(HWND(h.hwnd.get() as *mut c_void)),
            _ => None,
        }) else {
            return false;
        };
        // SAFETY: `own` is the app's window, made on this thread; `moved` has the shape Windows calls.
        unsafe { SetWindowSubclass(own, Some(moved), OURS, 0) }.as_bool()
    }

    /// What the window does with each message: a drag's start and end are noted, and a new scale met during a drag
    /// puts the window where Windows suggested, once winit has handled it; every message goes on to winit as before.
    unsafe extern "system" fn moved(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _mark: usize,
        _kept: usize,
    ) -> LRESULT {
        match message {
            WM_ENTERSIZEMOVE => DRAGGED.with(|d| d.set(true)),
            WM_EXITSIZEMOVE => DRAGGED.with(|d| d.set(false)),
            WM_DPICHANGED => {
                // SAFETY: for this message Windows' lparam points at the suggested place, valid during the call.
                let suggested = unsafe { *(lparam.0 as *const RECT) };
                // SAFETY: the message is passed on untouched, so winit takes the new scale as before.
                let answer = unsafe { DefSubclassProc(window, message, wparam, lparam) };
                if DRAGGED.with(Cell::get) {
                    let (width, height) = (suggested.right - suggested.left, suggested.bottom - suggested.top);
                    // SAFETY: this thread's own window and a plain place.
                    let _ = unsafe {
                        SetWindowPos(
                            window,
                            None,
                            suggested.left,
                            suggested.top,
                            width,
                            height,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        )
                    };
                }
                return answer;
            }
            WM_NCDESTROY => {
                // SAFETY: the window is going: our hold on it is let go first, as Windows asks.
                let _ = unsafe { RemoveWindowSubclass(window, Some(moved), OURS) };
            }
            _ => {}
        }
        // SAFETY: the message is passed on untouched to the window's own handling.
        unsafe { DefSubclassProc(window, message, wparam, lparam) }
    }
}
