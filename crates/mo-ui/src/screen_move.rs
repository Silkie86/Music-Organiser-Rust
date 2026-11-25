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
//!
//! The same watch keeps the last few size, place and window-menu messages Windows sent ([`heard`]), so the log line
//! for a window that left maximised by itself (`settings::resized`) can say what Windows told it first: on 25 Nov at
//! 18:45 it went back to 1440 x 900 with nothing asked by Slint or the app.

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

/// The messages Windows sent the window in the last `age` that touch its size, place or state, oldest first, each
/// with how long ago it came. Empty on Mac and Linux, and before [`connect`].
pub fn heard(age: std::time::Duration) -> Vec<String> {
    #[cfg(windows)]
    return on_windows::heard(age);
    #[cfg(not(windows))]
    {
        let _ = age;
        Vec::new()
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod on_windows {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::ffi::c_void;
    use std::time::{Duration, Instant};

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        InSendMessageEx, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, WINDOWPOS,
        WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_NCDESTROY, WM_NCLBUTTONDBLCLK,
        WM_NCLBUTTONDOWN, WM_SETTINGCHANGE, WM_SHOWWINDOW, WM_SIZE, WM_SYSCOMMAND, WM_WINDOWPOSCHANGED,
    };

    /// Our mark on the window ("MOSM").
    const OURS: usize = 0x4d4f_534d;

    thread_local! {
        /// Whether the window is being dragged or sized by the user now (Windows' move-and-size loop).
        static DRAGGED: Cell<bool> = const { Cell::new(false) };
        /// The last messages that touch the window's size, place or state, newest last (at most [`KEPT`]).
        static HEARD: RefCell<VecDeque<(Instant, String)>> = const { RefCell::new(VecDeque::new()) };
    }

    /// How many messages [`HEARD`] keeps.
    const KEPT: usize = 48;

    pub(super) fn heard(age: Duration) -> Vec<String> {
        let now = Instant::now();
        HEARD.with_borrow(|heard| {
            heard
                .iter()
                .filter(|(at, _)| now.duration_since(*at) <= age)
                .map(|(at, what)| format!("{} ms ago: Windows sent {what}", now.duration_since(*at).as_millis()))
                .collect()
        })
    }

    fn note(what: String) {
        // SAFETY: asks only about the message being handled on this thread.
        let sender = if unsafe { InSendMessageEx(None) } == 0 { "" } else { " (sent from another thread or program)" };
        HEARD.with_borrow_mut(|heard| {
            if heard.len() >= KEPT {
                heard.pop_front();
            }
            heard.push_back((Instant::now(), format!("{what}{sender}")));
        });
    }

    /// The name of a window-menu command (the low four bits are Windows' own).
    fn command(wparam: usize) -> String {
        match wparam & 0xfff0 {
            0xf000 => "size".to_owned(),
            0xf010 => "move".to_owned(),
            0xf020 => "minimise".to_owned(),
            0xf030 => "maximise".to_owned(),
            0xf060 => "close".to_owned(),
            0xf100 => "window menu by key".to_owned(),
            0xf120 => "restore".to_owned(),
            other => format!("{other:#06x}"),
        }
    }

    /// The name of the part of the window frame the pointer is on (`WM_NCHITTEST`'s answers).
    fn frame_part(wparam: usize) -> String {
        match wparam {
            1 => "inside".to_owned(),
            2 => "title bar".to_owned(),
            3 => "window menu".to_owned(),
            8 => "minimise button".to_owned(),
            9 => "maximise button".to_owned(),
            10..=17 => "edge or corner".to_owned(),
            20 => "close button".to_owned(),
            other => format!("part {other}"),
        }
    }

    /// Writes down a message that touches the window's size, place or state.
    fn listen(message: u32, wparam: WPARAM, lparam: LPARAM) {
        let (low, high) = (lparam.0 & 0xffff, (lparam.0 >> 16) & 0xffff);
        match message {
            WM_SIZE => {
                let kind = match wparam.0 {
                    0 => "restored",
                    1 => "minimised",
                    2 => "maximised",
                    _ => "other",
                };
                note(format!("WM_SIZE {kind} {low} x {high}"));
            }
            WM_WINDOWPOSCHANGED => {
                // SAFETY: for this message Windows' lparam points at the new place, valid during the call.
                let at = unsafe { *(lparam.0 as *const WINDOWPOS) };
                if !at.flags.contains(SWP_NOSIZE) || !at.flags.contains(SWP_NOMOVE) {
                    note(format!(
                        "WM_WINDOWPOSCHANGED to {} x {} at {}, {} (flags {:#x})",
                        at.cx, at.cy, at.x, at.y, at.flags.0
                    ));
                }
            }
            WM_SYSCOMMAND => note(format!("WM_SYSCOMMAND {}", command(wparam.0))),
            WM_NCLBUTTONDOWN => note(format!("a press on the frame: {}", frame_part(wparam.0))),
            WM_NCLBUTTONDBLCLK => note(format!("a double click on the frame: {}", frame_part(wparam.0))),
            WM_ENTERSIZEMOVE => note("WM_ENTERSIZEMOVE (a drag or size by the user starts)".to_owned()),
            WM_EXITSIZEMOVE => note("WM_EXITSIZEMOVE (the drag or size ends)".to_owned()),
            WM_SHOWWINDOW => note(format!("WM_SHOWWINDOW {} (reason {})", wparam.0 != 0, lparam.0)),
            WM_DISPLAYCHANGE => note(format!("WM_DISPLAYCHANGE {low} x {high}")),
            WM_DPICHANGED => note(format!("WM_DPICHANGED to {} dpi", wparam.0 & 0xffff)),
            // Only the work area (the taskbar moved or changed size): other settings come often and say nothing here.
            WM_SETTINGCHANGE if wparam.0 == 0x2f => note("WM_SETTINGCHANGE work area".to_owned()),
            _ => {}
        }
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
        listen(message, wparam, lparam);
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
