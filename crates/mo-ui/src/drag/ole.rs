//! Windows' own drag and drop (OLE), for files going out of the app.
//!
//! The files go as a file list (`CF_HDROP`), as Explorer hands files over, so every DAW and folder takes them. Only
//! copying is offered: whatever takes them can never move a sample away (rule 2). While the files are carried,
//! Windows runs the drag on the window's thread and holds it until they're let go, so:
//!
//! - The card that follows the mouse is a window of its own, on a thread of its own: a see-through, click-through
//!   window above every app, given a new picture each time the screen refreshes.
//! - The app's window paints nothing meanwhile (what it showed last stays on screen): winit can't draw while its thread
//!   is held, and asks to paint again at once each time it's told to, which would keep the processor busy for nothing.
//!   Once the files are let go, the window is drawn again.
//!
//! Everything that calls Windows here is `unsafe` in Rust's terms (Rust can't check what Windows does with what it's
//! given); each call says why it's sound.

use std::cell::Cell;
use std::ffi::c_void;
use std::mem::{ManuallyDrop, size_of};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use mo_app::drag_card::{Ending, Flight, Picture};
use mo_app::log;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{
    COLORREF, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, GlobalFree, HGLOBAL, HWND, LPARAM,
    LRESULT, POINT, S_OK, SIZE, WPARAM,
};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP, HDC, HGDIOBJ, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MonitorFromPoint, ReleaseDC, SelectObject, ValidateRect,
};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize,
    OleUninitialize,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MK_MBUTTON, MK_RBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::{DROPFILES, SHCreateDataObject};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GA_ROOT, GWLP_WNDPROC,
    GetAncestor, GetCursorPos, GetWindowLongPtrW, HTTRANSPARENT, HWND_TOPMOST, MA_NOACTIVATE, MSG, PM_REMOVE,
    PeekMessageW, RegisterClassExW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOSIZE,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WM_MOUSEACTIVATE,
    WM_NCHITTEST, WM_PAINT, WNDCLASSEXW, WNDPROC, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP, WindowFromPoint,
};
use windows::core::{BOOL, HRESULT, implement, w};

use super::Outcome;

/// How the drag is going, shared by the window's thread (Windows' drag) and the card's thread.
#[derive(Default)]
struct Shared {
    /// Over somewhere that takes the files.
    over: AtomicBool,
    /// [`GOING`] while it goes on, then how it ended.
    ended: AtomicU8,
}

const GOING: u8 = 0;
const LANDED: u8 = 1;
const NOT_LANDED: u8 = 2;

/// Hands `files` to Windows' drag and drop with `flight` following the mouse, and waits (on the window's thread, as
/// Windows needs) until they're let go. An error is what to tell the user: the drag couldn't start.
pub(super) fn drag(window: &slint::Window, files: &[PathBuf], flight: Flight) -> Result<Outcome, String> {
    let own = main_window(window);
    // SAFETY: called on the window's thread, which is the thread the drag runs on. winit has set the thread up for
    // OLE already, so this only counts one more use of it, and the matching OleUninitialize below hands it back.
    if let Err(e) = unsafe { OleInitialize(None) } {
        return Err(format!("Windows' drag and drop isn't ready on this window ({e})."));
    }
    let done = drag_files(own, files, flight);
    // SAFETY: matches the OleInitialize above, which succeeded.
    unsafe { OleUninitialize() };
    done
}

fn drag_files(own: Option<HWND>, files: &[PathBuf], flight: Flight) -> Result<Outcome, String> {
    let data = file_list(files).map_err(|e| format!("Couldn't hand the files to Windows ({e})."))?;
    let shared = Arc::new(Shared::default());
    let source: IDropSource = Source { shared: shared.clone(), own }.into();
    start_card(flight, shared.clone());
    let quiet = own.map(QuietPaint::start);
    let mut effect = DROPEFFECT_NONE;
    // SAFETY: `data` and `source` are live COM objects for the whole call, and `effect` outlives it.
    let hr = unsafe { DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect) };
    drop(quiet);
    // Let go over the app's own window: nothing there takes files, though the window says it does (winit's own file
    // drop, which the app doesn't use).
    let on_own = own.is_some_and(over_window);
    let landed = hr == DRAGDROP_S_DROP && effect.0 & DROPEFFECT_COPY.0 != 0 && !on_own;
    shared.ended.store(if landed { LANDED } else { NOT_LANDED }, Ordering::Release);
    if hr == DRAGDROP_S_DROP || hr == DRAGDROP_S_CANCEL {
        Ok(if landed { Outcome::Landed } else { Outcome::Cancelled })
    } else {
        Err(format!("Windows couldn't carry the files ({hr:?})."))
    }
}

/// The app's window, as Windows knows it.
fn main_window(window: &slint::Window) -> Option<HWND> {
    let handle = window.window_handle();
    match HasWindowHandle::window_handle(&handle).ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

/// Whether the mouse is over `own` (or a window inside it).
fn over_window(own: HWND) -> bool {
    let mut at = POINT::default();
    // SAFETY: plain calls with a point to fill and window handles; a handle that's gone gives null, not a fault.
    unsafe {
        if GetCursorPos(&mut at).is_err() {
            return false;
        }
        let under = WindowFromPoint(at);
        !under.is_invalid() && GetAncestor(under, GA_ROOT) == own
    }
}

/// The files as Windows' file list: a `DROPFILES` header, then each whole path in UTF-16 ending in a 0, then one more
/// 0. Windows' own data object holds it, as Explorer's does.
fn file_list(files: &[PathBuf]) -> windows::core::Result<IDataObject> {
    let mut wide: Vec<u16> = Vec::new();
    for f in files {
        wide.extend(f.as_os_str().encode_wide().filter(|&c| c != 0));
        wide.push(0);
    }
    wide.push(0);
    let header = size_of::<DROPFILES>();
    let bytes = header + wide.len() * size_of::<u16>();
    // SAFETY: an empty shell data object, with nothing pointed at.
    let data: IDataObject = unsafe { SHCreateDataObject(None, None, None::<&IDataObject>)? };
    // SAFETY: the memory is `bytes` long, locked while it's written, and either handed to the data object (which frees
    // it) or freed here.
    unsafe {
        let memory: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes)?;
        let at = GlobalLock(memory).cast::<u8>();
        if at.is_null() {
            let _ = GlobalFree(Some(memory));
            return Err(windows::core::Error::from_thread());
        }
        at.cast::<DROPFILES>().write_unaligned(DROPFILES {
            pFiles: header as u32,
            pt: POINT::default(),
            fNC: BOOL(0),
            fWide: BOOL(1),
        });
        std::ptr::copy_nonoverlapping(wide.as_ptr().cast::<u8>(), at.add(header), wide.len() * size_of::<u16>());
        // Unlocking the last lock "fails" with no error: that's how Windows says it's unlocked.
        let _ = GlobalUnlock(memory);
        let format = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let medium = STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: memory },
            pUnkForRelease: ManuallyDrop::new(None),
        };
        if let Err(e) = data.SetData(&format, &medium, true) {
            let _ = GlobalFree(Some(memory));
            return Err(e);
        }
    }
    Ok(data)
}

/// What Windows asks the app while the files are carried: whether to go on, and how it's going.
#[implement(IDropSource)]
struct Source {
    shared: Arc<Shared>,
    own: Option<HWND>,
}

impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() || keys.0 & (MK_RBUTTON.0 | MK_MBUTTON.0) != 0 {
            // Esc, or another button pressed as well: called off, as Explorer does.
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, effect: DROPEFFECT) -> HRESULT {
        let takes = effect.0 & DROPEFFECT_COPY.0 != 0 && !self.own.is_some_and(over_window);
        self.shared.over.store(takes, Ordering::Relaxed);
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

// ---- The app's window paints nothing while the drag holds its thread -------------------------------------------

thread_local! {
    /// The window's own procedure while [`QuietPaint`] stands in front of it.
    static WINDOW_PROC: Cell<WNDPROC> = const { Cell::new(None) };
    static QUIET: Cell<bool> = const { Cell::new(false) };
}

/// Stands in front of the app's window procedure for the drag: paint messages are marked done without painting, and
/// everything else goes on to winit as before. Dropping it takes it away again and asks for a fresh paint.
struct QuietPaint {
    window: HWND,
}

impl QuietPaint {
    fn start(window: HWND) -> QuietPaint {
        // SAFETY: the window belongs to this thread; its procedure is swapped for ours, which passes everything on to
        // it, and is put back in `drop` on the same thread.
        // The old one is noted before ours goes in, so ours always has it to pass things on to.
        unsafe {
            let old = GetWindowLongPtrW(window, GWLP_WNDPROC);
            if old != 0 {
                WINDOW_PROC.with(|p| p.set(std::mem::transmute::<isize, WNDPROC>(old)));
                if SetWindowLongPtrW(window, GWLP_WNDPROC, quiet_proc_address()) != 0 {
                    QUIET.with(|q| q.set(true));
                }
            }
        }
        QuietPaint { window }
    }
}

impl Drop for QuietPaint {
    fn drop(&mut self) {
        QUIET.with(|q| q.set(false));
        // SAFETY: as in `start`. Only put back when ours is still the one in front: if something else stood in front
        // of it meanwhile, ours stays (quiet off, it only passes things on) so theirs keeps working.
        unsafe {
            if GetWindowLongPtrW(self.window, GWLP_WNDPROC) == quiet_proc_address()
                && let Some(old) = WINDOW_PROC.with(Cell::get)
            {
                SetWindowLongPtrW(self.window, GWLP_WNDPROC, old as usize as isize);
            }
            let _ = InvalidateRect(Some(self.window), None, false);
        }
    }
}

/// [`quiet_proc`]'s address, the way a window keeps its procedure.
fn quiet_proc_address() -> isize {
    let procedure: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = quiet_proc;
    procedure as usize as isize
}

unsafe extern "system" fn quiet_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_PAINT && QUIET.with(Cell::get) {
        // SAFETY: marks the window painted; it's this thread's window.
        let _ = unsafe { ValidateRect(Some(window), None) };
        return LRESULT(0);
    }
    // SAFETY: the window's own procedure, as it was before, with the message as it came.
    unsafe { CallWindowProcW(WINDOW_PROC.with(Cell::get), window, message, wparam, lparam) }
}

// ---- The card ----------------------------------------------------------------------------------------------------

/// Starts the card's thread. Not waited for: it lands or goes away by itself after the drag, while the window gets on.
fn start_card(flight: Flight, shared: Arc<Shared>) {
    // Not lowered like background work: it's what the user is looking at.
    let started = std::thread::Builder::new().name("Drag card".into()).spawn(move || {
        if let Err(e) = run_card(flight, &shared) {
            log::warning("Drag", &format!("The card that follows the mouse couldn't be shown: {e}"));
        }
    });
    if let Err(e) = started {
        log::warning("Drag", &format!("Couldn't start the card that follows the mouse: {e}"));
    }
}

/// The card's window and its drawing surface.
struct CardWindow {
    window: HWND,
    screen: HDC,
    memory: HDC,
    bitmap: Option<(HBITMAP, HGDIOBJ, *mut u8, u32, u32)>,
    /// The hot point of the picture up now, in it.
    hot: (i32, i32),
    shown: bool,
}

impl Drop for CardWindow {
    fn drop(&mut self) {
        // SAFETY: everything here was made by this thread for this window and is let go once.
        unsafe {
            if let Some((bitmap, old, ..)) = self.bitmap.take() {
                SelectObject(self.memory, old);
                let _ = DeleteObject(bitmap.into());
            }
            let _ = DeleteDC(self.memory);
            ReleaseDC(None, self.screen);
            let _ = DestroyWindow(self.window);
        }
    }
}

fn run_card(mut flight: Flight, shared: &Shared) -> windows::core::Result<()> {
    let mut card = CardWindow::new()?;
    let mut over = None;
    let mut ended = GOING;
    let mut last_scale = 0.0;
    loop {
        card.pump();
        let now = Instant::now();
        let is_over = shared.over.load(Ordering::Relaxed);
        if over != Some(is_over) {
            flight.set_over_target(is_over, now);
            over = Some(is_over);
        }
        let end = shared.ended.load(Ordering::Acquire);
        if end != GOING && ended == GOING {
            flight.end(if end == LANDED { Ending::Landed } else { Ending::Cancelled }, now);
            ended = end;
        }
        let mut at = POINT::default();
        // SAFETY: a point to fill.
        unsafe { GetCursorPos(&mut at)? };
        let scale = scale_at(at);
        if card.bitmap.is_some() && scale == last_scale && flight.still(now, scale) {
            card.move_to(at)?;
        } else {
            let Some(picture) = flight.frame(now, scale) else { break };
            card.show(&picture, at)?;
            last_scale = scale;
        }
        // Once a frame: Windows says when the screen has been refreshed. Without it, a short sleep.
        // SAFETY: a plain call.
        if unsafe { DwmFlush() }.is_err() {
            std::thread::sleep(Duration::from_millis(8));
        }
    }
    Ok(())
}

/// The screen's scale under the mouse: 1 at 96 dots an inch, 1.5 at 144.
fn scale_at(at: POINT) -> f32 {
    let (mut x, mut y) = (96, 96);
    // SAFETY: a monitor handle Windows gives for the point, and two numbers to fill.
    let found =
        unsafe { GetDpiForMonitor(MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST), MDT_EFFECTIVE_DPI, &mut x, &mut y) };
    if found.is_ok() && x > 0 { x as f32 / 96.0 } else { 1.0 }
}

impl CardWindow {
    fn new() -> windows::core::Result<CardWindow> {
        // SAFETY: a window class (once) and a window of it, made and used only on this thread.
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("MusicOrganiserDragCard");
            let info = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(card_proc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..WNDCLASSEXW::default()
            };
            // 0 once it's registered already (an earlier drag): that's fine.
            RegisterClassExW(&info);
            let window = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                class,
                w!("Music Organiser drag"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            let screen = GetDC(None);
            let memory = CreateCompatibleDC(Some(screen));
            Ok(CardWindow { window, screen, memory, bitmap: None, hot: (0, 0), shown: false })
        }
    }

    /// Answers whatever Windows sends the card's window, without waiting.
    fn pump(&self) {
        let mut message = MSG::default();
        // SAFETY: this thread's own messages.
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    /// Puts up `picture` with its hot point under the mouse at `at`.
    fn show(&mut self, picture: &Picture, at: POINT) -> windows::core::Result<()> {
        let (w, h) = (picture.width.max(1), picture.height.max(1));
        if !self.bitmap.as_ref().is_some_and(|b| b.3 == w && b.4 == h) {
            self.new_bitmap(w, h)?;
        }
        let Some((_, _, bits, ..)) = self.bitmap else { return Ok(()) };
        let pixels = (w * h) as usize;
        // SAFETY: the bitmap is `w` × `h` pixels of 4 bytes, made just for this size; `rgba` is read only as far as it
        // goes.
        unsafe {
            let out = std::slice::from_raw_parts_mut(bits, pixels * 4);
            for (o, &[r, g, b, a]) in out.as_chunks_mut::<4>().0.iter_mut().zip(picture.rgba.as_chunks::<4>().0) {
                // Windows wants blue, green, red, alpha, all already times alpha (as the card's are).
                *o = [b, g, r, a];
            }
        }
        let to = POINT { x: at.x - picture.hot_x, y: at.y - picture.hot_y };
        let size = SIZE { cx: w as i32, cy: h as i32 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // SAFETY: this thread's window and drawing surfaces, and values that outlive the call.
        unsafe {
            UpdateLayeredWindow(
                self.window,
                Some(self.screen),
                Some(&to),
                Some(&size),
                Some(self.memory),
                Some(&POINT::default()),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )?;
            if !self.shown {
                let _ = ShowWindow(self.window, SW_SHOWNOACTIVATE);
                self.shown = true;
            }
        }
        self.hot = (picture.hot_x, picture.hot_y);
        Ok(())
    }

    /// Moves the picture already up so its hot point is under the mouse at `at`.
    fn move_to(&self, at: POINT) -> windows::core::Result<()> {
        // SAFETY: this thread's window.
        unsafe {
            SetWindowPos(
                self.window,
                Some(HWND_TOPMOST),
                at.x - self.hot.0,
                at.y - self.hot.1,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            )
        }
    }

    fn new_bitmap(&mut self, w: u32, h: u32) -> windows::core::Result<()> {
        // SAFETY: a top-down 32-bit bitmap for this thread's drawing surface; the one before is let go first.
        unsafe {
            if let Some((bitmap, old, ..)) = self.bitmap.take() {
                SelectObject(self.memory, old);
                let _ = DeleteObject(bitmap.into());
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    // Upside down in Windows' terms: the first row is the top one, as the card's are.
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..BITMAPINFOHEADER::default()
                },
                ..BITMAPINFO::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(Some(self.screen), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            if bits.is_null() {
                let _ = DeleteObject(bitmap.into());
                return Err(windows::core::Error::from_thread());
            }
            let old = SelectObject(self.memory, bitmap.into());
            self.bitmap = Some((bitmap, old, bits.cast::<u8>(), w, h));
        }
        Ok(())
    }
}

unsafe extern "system" fn card_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        // The mouse goes straight through to what's under the card, and the card never takes the focus.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // SAFETY: Windows' own handling of everything else, with the message as it came.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}
