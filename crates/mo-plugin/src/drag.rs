//! Dragging a sound out of the panel onto a track, as its own file.
//!
//! CLAP itself has nothing for dragging out: a plugin hands the file to the system's own drag and drop, and the DAW
//! takes it as it takes a file from a folder. On Windows that's OLE, as the app's own lists do it (mo-ui's
//! `drag/ole.rs`). The file goes as the very thing Explorer hands over when the same file is dragged from a folder
//! (the shell's own data object for it: the file list `CF_HDROP` and the shell's other forms), so a DAW that takes
//! a file from Explorer finds what it looks for; if the shell can't make that, as a plain file list. Copying and
//! linking are offered, never moving: whatever takes the file can use it where it is or copy it, and can never move
//! the sample away (rule 2). How each drag went is in the plugin's log. Windows runs the drag on the window's thread (in a DAW, its main thread) and holds it until
//! the file is let go, as it does for a drag from the DAW's own browser.
//!
//! Not on Mac or Linux yet: there the lines can't be dragged.

use std::path::Path;

/// Whether lines can be dragged out on this system.
pub const CAN: bool = cfg!(windows);

/// How a drag ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Something took the file.
    Landed,
    /// Let go where nothing takes files, or called off (Esc, or another mouse button).
    Cancelled,
}

/// Hands `file` to the system's drag and drop, and waits until it's let go. Call it on the window's thread, with
/// nothing borrowed that the window's timer needs: on Windows the timer still runs meanwhile. An error is what to tell
/// the user: the drag couldn't start.
pub fn file(file: &Path) -> Result<Outcome, String> {
    hand_over(file)
}

#[cfg(not(windows))]
fn hand_over(_file: &Path) -> Result<Outcome, String> {
    Err("Dragging out works on Windows only for now".into())
}

#[cfg(windows)]
fn hand_over(file: &Path) -> Result<Outcome, String> {
    ole::drag(file)
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod ole {
    use super::Outcome;
    use std::mem::{ManuallyDrop, size_of};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows::Win32::Foundation::{
        DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, GlobalFree, HGLOBAL, POINT, S_OK,
    };
    use windows::Win32::System::Com::IBindCtx;
    use windows::Win32::System::Com::{
        DVASPECT_CONTENT, FORMATETC, IDataObject, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
    };
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::{
        CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_NONE, DoDragDrop, IDropSource,
        IDropSource_Impl, OleInitialize, OleUninitialize,
    };
    use windows::Win32::System::SystemServices::{MK_LBUTTON, MK_MBUTTON, MK_RBUTTON, MODIFIERKEYS_FLAGS};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    use windows::Win32::UI::Shell::{
        BHID_DataObject, DROPFILES, IShellItem, SHCreateDataObject, SHCreateItemFromParsingName,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SWAPBUTTON};
    use windows::core::{BOOL, HRESULT, PCWSTR, implement};

    pub fn drag(file: &Path) -> Result<Outcome, String> {
        // SAFETY: called on the window's thread, which is the thread the drag runs on. The window (baseview) has set
        // the thread up for OLE already, so this only counts one more use of it, and the matching OleUninitialize
        // below hands it back. In a DAW whose main thread is set up another way this fails, and the drag is off.
        if let Err(e) = unsafe { OleInitialize(None) } {
            return Err(format!("Windows' drag and drop isn't ready in this DAW ({e})"));
        }
        let done = drag_file(file);
        // SAFETY: matches the OleInitialize above, which succeeded.
        unsafe { OleUninitialize() };
        done
    }

    fn drag_file(file: &Path) -> Result<Outcome, String> {
        // Let go already (the button came up while the file was looked for): Windows' drag would drop it at once,
        // wherever the mouse is now, so the drag is off.
        if !main_button_down() {
            return Ok(Outcome::Cancelled);
        }
        let (data, handed) = match shell_data(file) {
            Ok(data) => (data, "as Explorer hands it"),
            Err(why) => {
                crate::log::line("Drag", &format!("The shell couldn't hand the file over ({why}): a plain file list"));
                let list = file_list(file).map_err(|e| format!("Couldn't hand the file to Windows ({e})"))?;
                (list, "as a plain file list")
            }
        };
        // What whatever was under the mouse said it would do with the file, all through the drag.
        let offered = Arc::new(AtomicU32::new(0));
        let source: IDropSource = Source { offered: Arc::clone(&offered) }.into();
        let allowed = DROPEFFECT_COPY | DROPEFFECT_LINK;
        let mut effect = DROPEFFECT_NONE;
        // SAFETY: `data` and `source` are live COM objects for the whole call, and `effect` outlives it.
        let hr = unsafe { DoDragDrop(&data, &source, allowed, &mut effect) };
        crate::log::line(
            "Drag",
            &format!(
                "Handed {handed}; Windows ended it with {hr:?}, what took it did {} (1 copy, 4 link, 0 nothing); \
                 while carried, what was under the mouse offered {}",
                effect.0,
                offered.load(Ordering::Relaxed)
            ),
        );
        if hr == DRAGDROP_S_DROP {
            Ok(if effect.0 & allowed.0 != 0 { Outcome::Landed } else { Outcome::Cancelled })
        } else if hr == DRAGDROP_S_CANCEL {
            Ok(Outcome::Cancelled)
        } else {
            Err(format!("Windows couldn't carry the file ({hr:?})"))
        }
    }

    /// Whether the main mouse button (the left one, unless they're swapped in Windows' settings) is down right now.
    fn main_button_down() -> bool {
        // SAFETY: plain calls that read a setting and the mouse buttons' state.
        unsafe {
            // The buttons as they are, not as they're swapped: the main one is the right when they are.
            let main = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { VK_RBUTTON } else { VK_LBUTTON };
            GetAsyncKeyState(i32::from(main.0)) < 0
        }
    }

    /// The file as Explorer hands it over: the shell's own data object for it.
    fn shell_data(file: &Path) -> windows::core::Result<IDataObject> {
        let mut wide: Vec<u16> = file.as_os_str().encode_wide().filter(|&c| c != 0).collect();
        wide.push(0);
        // SAFETY: `wide` is a 0-ended UTF-16 path that lives through the call; the shell item is a live COM object
        // while it's asked for its data object.
        unsafe {
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None::<&IBindCtx>)?;
            item.BindToHandler(None::<&IBindCtx>, &BHID_DataObject)
        }
    }

    /// The file as Windows' file list: a `DROPFILES` header, then the whole path in UTF-16 ending in a 0, then one
    /// more 0. Windows' own data object holds it, as Explorer's does.
    fn file_list(file: &Path) -> windows::core::Result<IDataObject> {
        let mut wide: Vec<u16> = file.as_os_str().encode_wide().filter(|&c| c != 0).collect();
        wide.extend([0, 0]);
        let header = size_of::<DROPFILES>();
        let bytes = header + wide.len() * size_of::<u16>();
        // SAFETY: an empty shell data object, with nothing pointed at.
        let data: IDataObject = unsafe { SHCreateDataObject(None, None, None::<&IDataObject>)? };
        // SAFETY: the memory is `bytes` long, locked while it's written, and either handed to the data object (which
        // frees it) or freed here.
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

    /// What Windows asks the plugin while the file is carried: whether to go on.
    #[implement(IDropSource)]
    struct Source {
        /// Every effect offered by what was under the mouse, added together (for the log).
        offered: Arc<AtomicU32>,
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
            // The top bit only says the target is scrolling.
            self.offered.fetch_or(effect.0 & 0x7, Ordering::Relaxed);
            DRAGDROP_S_USEDEFAULTCURSORS
        }
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    // On Windows the drag itself needs a held mouse button and somewhere to drop: never run by a test.
    #[test]
    fn off_windows_a_drag_says_it_cant() {
        assert!(file(Path::new("kick.wav")).is_err());
    }
}
