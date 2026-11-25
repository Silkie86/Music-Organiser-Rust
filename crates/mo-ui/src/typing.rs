//! Windows' typing service is loaded ahead of the first click in a text box.
//!
//! The first time a text box takes the keys, Slint tells Windows the window takes typed text (winit's
//! `set_ime_allowed`), and Windows then starts its text service for the window's thread: it loads several of its own
//! libraries, there and then, on the window's thread. On a busy disk that took 5 seconds on Solomon's PC (23 Nov 2026,
//! a click in the Curves search while his plugins were being measured): the window stood still for all of it.
//!
//! The service itself can only be started on the thread that types, but the reading from disk is the slow part, and
//! that can be done anywhere: [`get_ready`] loads the same libraries on a background thread a little after the start,
//! and keeps them, so the first click finds them in memory.

/// Loads the libraries Windows' typing service needs, in the background at low priority. Call it once the window is
/// up. Nothing to do on Mac and Linux.
pub fn get_ready() {
    #[cfg(windows)]
    if let Err(e) = mo_app::background::spawn("Typing service", on_windows::load) {
        mo_app::log::info("Typing", &format!("Couldn't load Windows' typing service ahead of time: {e}"));
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod on_windows {
    use std::time::Instant;

    use windows_sys::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};

    /// What the text service loads when it starts for a thread: itself, the text box side of it, and what those two
    /// wait to load until they're used (the freeze's stack was inside such a wait, in the second). Names only:
    /// each is looked for in Windows' own folder and nowhere else. One that a version of Windows doesn't have is
    /// passed over.
    const LIBRARIES: [&str; 6] = [
        "msctf.dll",
        "textinputframework.dll",
        "coremessaging.dll",
        "coreuicomponents.dll",
        "wintypes.dll",
        "ntmarta.dll",
    ];

    pub(super) fn load() {
        let began = Instant::now();
        let loaded = LIBRARIES.iter().filter(|name| keep_loaded(name)).count();
        mo_app::log::info(
            "Typing",
            &format!(
                "Windows' typing service loaded ahead of the first text box: {loaded} of {} libraries in {} ms",
                LIBRARIES.len(),
                began.elapsed().as_millis()
            ),
        );
    }

    /// Loads `name` from Windows' own folder and keeps it for the rest of the run; whether it was there.
    fn keep_loaded(name: &str) -> bool {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` ends with a zero and lives through the call; the library is one of Windows' own, loaded from
        // its system folder only, and never let go of.
        let handle = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        !handle.is_null()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_text_service_is_found_and_a_made_up_library_is_passed_over() {
            assert!(keep_loaded("msctf.dll"));
            assert!(!keep_loaded("mo-no-such-library.dll"));
        }
    }
}
