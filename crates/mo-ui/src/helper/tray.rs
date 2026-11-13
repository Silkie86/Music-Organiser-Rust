//! The helper's icon by the clock on Windows: a dot that's green while it has Bitwig, amber while paused and grey
//! otherwise, its words on hover, and a menu (right-click): Open Music Organiser, Pause watching Bitwig, Quit. A
//! double-click opens the app.
//!
//! Windows' own tray calls (Shell_NotifyIconW) through a hidden window of the helper's, on the helper's main thread.
//! The window is never shown; it's there for the icon's messages, a timer that looks at the helper every
//! [`LOOK_MS`] ms, and the message Explorer sends when it starts again (the icon is put back then).

use std::cell::RefCell;
use std::sync::Arc;

use mo_app::helper::{self, Helper};
use mo_app::log;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, HICON, KillTimer, LR_DEFAULTCOLOR, MF_CHECKED,
    MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
    SetForegroundWindow, SetTimer, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONDBLCLK, WM_NULL, WM_RBUTTONUP,
    WM_TIMER, WNDCLASSW,
};
use windows::core::{PCWSTR, w};

/// How often the icon looks at the helper, in milliseconds: its colour and words, and whether it was asked to quit.
const LOOK_MS: u32 = 2000;
/// The message the icon sends the window.
const ICON_MESSAGE: u32 = WM_APP + 1;
const TIMER: usize = 1;
const OPEN: usize = 1;
const PAUSE: usize = 2;
const QUIT: usize = 3;

/// The icon's colours: connected, paused, anything else.
const GREEN: [u8; 3] = [0x3c, 0xb3, 0x71];
const AMBER: [u8; 3] = [0xe0, 0xa1, 0x2e];
const GREY: [u8; 3] = [0x8a, 0x8f, 0x98];

struct Tray {
    helper: Arc<Helper>,
    window: HWND,
    icon: Option<HICON>,
    /// The colour and words shown now.
    shown: Option<([u8; 3], String)>,
    /// The message Explorer sends all windows when the taskbar comes back.
    taskbar_back: u32,
}

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

/// Shows the icon and runs its messages until the helper quits.
pub fn run(helper: &Arc<Helper>) {
    if let Err(e) = show(helper) {
        log::warning("Helper", &format!("Couldn't show the icon by the clock ({e}); running without it"));
        helper.wait(None);
        return;
    }
    let mut msg = MSG::default();
    // SAFETY: the usual message loop on the thread that made the window; `msg` lives across the calls.
    unsafe {
        // 0 is the end (WM_QUIT) and -1 an error: either ends the loop, rather than spin on an error.
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    TRAY.with(|t| t.borrow_mut().take());
}

fn show(helper: &Arc<Helper>) -> windows::core::Result<()> {
    // SAFETY: plain Win32 calls with arguments made here; the class name and title are static wide strings.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = w!("MusicOrganiserHelper");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        // A top-level window that's never shown (a message-only one would miss Explorer's "taskbar back").
        let window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("Music Organiser helper"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let taskbar_back = RegisterWindowMessageW(w!("TaskbarCreated"));
        TRAY.with(|t| {
            *t.borrow_mut() = Some(Tray { helper: Arc::clone(helper), window, icon: None, shown: None, taskbar_back })
        });
        refresh(true);
        SetTimer(Some(window), TIMER, LOOK_MS, None);
    }
    Ok(())
}

/// Puts the icon's colour and words right; `again` puts the icon in afresh (at the start, and when Explorer came
/// back).
fn refresh(again: bool) {
    let ending = TRAY.with(|t| t.borrow().as_ref().filter(|t| t.helper.quitting()).map(|t| t.window));
    if let Some(window) = ending {
        // Outside the borrow: destroying the window calls window_proc (WM_DESTROY) straight away, which borrows too.
        // SAFETY: the window is this thread's own.
        unsafe {
            let _ = DestroyWindow(window);
        }
        return;
    }
    TRAY.with(|t| {
        let mut t = t.borrow_mut();
        let Some(tray) = t.as_mut() else { return };
        let state = tray.helper.bitwig();
        let colour = match &state {
            None => AMBER,
            Some(mo_dawlink::bitwig::watch::State::Watching) => GREEN,
            Some(_) => GREY,
        };
        let words = format!("Music Organiser helper: {}", helper::bitwig_words(state.as_ref()));
        let now = (colour, words);
        if !again && tray.shown.as_ref() == Some(&now) {
            return;
        }
        let changed_colour = tray.shown.as_ref().is_none_or(|(c, _)| *c != colour);
        if changed_colour || tray.icon.is_none() {
            let old = tray.icon.take();
            tray.icon = make_icon(colour);
            if let Some(old) = old {
                // SAFETY: an icon this module made and no longer shows once the next call below replaces it.
                unsafe {
                    let _ = DestroyIcon(old);
                }
            }
        }
        tray.shown = Some(now);
        let data = icon_data(tray);
        // SAFETY: `data` is filled in for this window's icon 1.
        unsafe {
            // Adding fails when it's already there (Explorer kept it): it's changed instead.
            if !again || !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
            }
        }
    });
}

fn icon_data(tray: &Tray) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: tray.window,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: ICON_MESSAGE,
        ..Default::default()
    };
    if let Some(icon) = tray.icon {
        data.hIcon = icon;
    }
    let words = tray.shown.as_ref().map_or("Music Organiser helper", |(_, w)| w.as_str());
    // The last place stays 0, the end of the words.
    let room = data.szTip.len() - 1;
    for (slot, unit) in data.szTip.iter_mut().take(room).zip(words.encode_utf16()) {
        *slot = unit;
    }
    data
}

extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let taskbar_back = TRAY.with(|t| t.borrow().as_ref().map(|t| t.taskbar_back));
    match message {
        ICON_MESSAGE => {
            match (lparam.0 & 0xffff) as u32 {
                WM_RBUTTONUP | WM_CONTEXTMENU => menu(window),
                WM_LBUTTONDBLCLK => open_app(),
                _ => {}
            }
            LRESULT(0)
        }
        WM_TIMER => {
            refresh(false);
            LRESULT(0)
        }
        WM_DESTROY => {
            TRAY.with(|t| {
                if let Some(tray) = t.borrow().as_ref() {
                    let data = icon_data(tray);
                    // SAFETY: removes this window's own icon and timer.
                    unsafe {
                        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
                        let _ = KillTimer(Some(window), TIMER);
                    }
                    if let Some(icon) = tray.icon {
                        // SAFETY: the icon is gone from the tray now.
                        unsafe {
                            let _ = DestroyIcon(icon);
                        }
                    }
                }
            });
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        m if Some(m) == taskbar_back => {
            refresh(true);
            LRESULT(0)
        }
        // SAFETY: what Windows does with every other message.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// The right-click menu, at the mouse.
fn menu(window: HWND) {
    let Some(helper) = TRAY.with(|t| t.borrow().as_ref().map(|t| Arc::clone(&t.helper))) else { return };
    // SAFETY: a menu made, shown and destroyed here, for this thread's own window.
    let chosen = unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let pause_flags = if helper.paused() { MF_STRING | MF_CHECKED } else { MF_STRING };
        // Windows' own menu by the clock, not one of the app's: its keys are the underlined letters (O, P, Q).
        let _ = AppendMenuW(menu, MF_STRING, OPEN, w!("&Open Music Organiser"));
        let _ = AppendMenuW(menu, pause_flags, PAUSE, w!("&Pause watching Bitwig"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, QUIT, w!("&Quit the helper"));
        let mut at = POINT::default();
        let _ = GetCursorPos(&mut at);
        // Without this the menu stays up when the mouse clicks elsewhere.
        let _ = SetForegroundWindow(window);
        let chosen =
            TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY, at.x, at.y, None, window, None);
        let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        chosen.0 as usize
    };
    match chosen {
        OPEN => open_app(),
        PAUSE => {
            helper.pause(!helper.paused());
            refresh(false);
        }
        QUIT => {
            log::info("Helper", "Quit from its icon");
            helper.quit();
            refresh(false);
        }
        _ => {}
    }
}

/// Starts the full app: this same program, without the helper's flag, at normal priority (it would take on the
/// helper's below-normal otherwise).
fn open_app() {
    use std::os::windows::process::CommandExt;
    const NORMAL_PRIORITY_CLASS: u32 = 0x0000_0020;
    let opened = std::env::current_exe()
        .and_then(|exe| std::process::Command::new(exe).creation_flags(NORMAL_PRIORITY_CLASS).spawn());
    if let Err(e) = opened {
        log::warning("Helper", &format!("Couldn't open Music Organiser: {e}"));
    }
}

/// A 32 by 32 icon: a round dot of `colour` in a dark ring.
fn make_icon(colour: [u8; 3]) -> Option<HICON> {
    const SIZE: usize = 32;
    let bits = icon_bits(SIZE, colour);
    // SAFETY: `bits` is a whole icon image (header, colours, mask) as CreateIconFromResourceEx reads one.
    unsafe { CreateIconFromResourceEx(&bits, true, 0x0003_0000, SIZE as i32, SIZE as i32, LR_DEFAULTCOLOR).ok() }
}

/// The bytes of an icon image as an .ico file holds one: a BITMAPINFOHEADER (twice the height: colours, then mask),
/// the colours bottom row first (blue, green, red, alpha), then the mask (all 0: the alpha says what shows).
fn icon_bits(size: usize, colour: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(40 + size * size * 4 + size * 4);
    let size_i = size as i32;
    for v in [40i32, size_i, size_i * 2] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bits a pixel
    for _ in 0..6 {
        out.extend_from_slice(&0i32.to_le_bytes()); // no compression, sizes and colours left to Windows
    }
    let middle = (size as f32 - 1.0) / 2.0;
    let (outer, inner) = (size as f32 * 0.47, size as f32 * 0.34);
    for y in (0..size).rev() {
        for x in 0..size {
            let d = ((x as f32 - middle).powi(2) + (y as f32 - middle).powi(2)).sqrt();
            // Soft edges: a pixel's share inside each circle.
            let in_outer = (outer - d + 0.5).clamp(0.0, 1.0);
            let in_inner = (inner - d + 0.5).clamp(0.0, 1.0);
            let ring: [u8; 3] = [0x2a, 0x2d, 0x33];
            let mix = |c: usize| (f32::from(colour[c]) * in_inner + f32::from(ring[c]) * (1.0 - in_inner)) as u8;
            out.extend_from_slice(&[mix(2), mix(1), mix(0), (in_outer * 255.0) as u8]);
        }
    }
    // The mask: one bit a pixel, rows padded to 4 bytes.
    out.resize(out.len() + size.div_ceil(32) * 4 * size, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_a_whole_image() {
        let bits = icon_bits(32, GREEN);
        assert_eq!(bits.len(), 40 + 32 * 32 * 4 + 4 * 32);
        // The middle is the colour, fully shown; a corner shows nothing.
        let at = |x: usize, y: usize| 40 + ((31 - y) * 32 + x) * 4;
        let middle = at(16, 16);
        assert_eq!(&bits[middle..middle + 4], &[GREEN[2], GREEN[1], GREEN[0], 255]);
        assert_eq!(bits[at(0, 0) + 3], 0);
    }
}
