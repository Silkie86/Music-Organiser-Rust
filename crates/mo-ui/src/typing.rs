//! Windows' typing service is loaded ahead of the first click in a text box.
//!
//! The first time a text box takes the keys, Slint tells Windows the window takes typed text (winit's
//! `set_ime_allowed`), and Windows then starts its text service for the window's thread: it loads several of its own
//! libraries, there and then, on the window's thread. On a busy disk that took 5 seconds on Solomon's PC (23 Nov 2026,
//! a click in the Curves search while his plugins were being measured): the window stood still for all of it.
//!
//! The service itself can only be started on the thread that types, but the reading from disk is the slow part, and
//! that can be done anywhere: [`get_ready`] reads the same libraries on a thread of its own as the app starts, and
//! keeps them loaded, so the first click finds them in memory. Each file is first read through as a plain file, which
//! holds nothing up, and only then loaded: Windows loads one library at a time for the whole program, so a slow load
//! here would make the window wait for its own next one.
//!
//! Whether it worked shows in the log: the line this writes, and no "Freeze" line at the first click in a text box.

/// Loads the libraries Windows' typing service needs, on a thread of its own. Nothing to do on Mac and Linux.
///
/// Not at low priority, unlike the rest of the app's background work: while it loads a library the window can't load
/// one, so a busy computer mustn't leave it waiting halfway. It's a few megabytes, once.
pub fn get_ready() {
    #[cfg(windows)]
    if let Err(e) = std::thread::Builder::new().name("Typing service".to_owned()).spawn(on_windows::load) {
        mo_app::log::warning("Typing", &format!("Couldn't read Windows' typing service ahead of time: {e}"));
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod on_windows {
    use std::fs::File;
    use std::io::Read;
    use std::path::{Path, PathBuf};
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
        let folder = std::env::var_os("SystemRoot").map(|root| PathBuf::from(root).join("System32"));
        let mut read = 0u64;
        let mut missing = Vec::new();
        for name in LIBRARIES {
            if let Some(folder) = &folder {
                read += read_through(&folder.join(name));
            }
            if !keep_loaded(name) {
                missing.push(name);
            }
        }
        let loaded = LIBRARIES.len() - missing.len();
        let took = began.elapsed().as_millis();
        let size = read as f64 / (1024.0 * 1024.0);
        if loaded == 0 {
            mo_app::log::warning(
                "Typing",
                &format!(
                    "None of Windows' typing service could be loaded ahead of the first text box ({took} ms): the \
                     first click in one may wait for the disk"
                ),
            );
            return;
        }
        let passed_over =
            if missing.is_empty() { String::new() } else { format!(" (not on this Windows: {})", missing.join(", ")) };
        mo_app::log::info(
            "Typing",
            &format!(
                "Windows' typing service loaded ahead of the first text box: {loaded} of {} libraries, {size:.1} MB \
                 read, in {took} ms{passed_over}",
                LIBRARIES.len()
            ),
        );
    }

    /// Reads the file through and throws the bytes away, so Windows has it in memory when it's loaded straight
    /// after; how many bytes. A file that isn't there, or stops part-way, counts what was read.
    fn read_through(file: &Path) -> u64 {
        let Ok(mut file) = File::open(file) else { return 0 };
        let mut piece = vec![0u8; 256 * 1024];
        let mut read = 0u64;
        loop {
            match file.read(&mut piece) {
                Ok(0) | Err(_) => return read,
                Ok(n) => read += n as u64,
            }
        }
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

        #[test]
        fn a_file_is_read_to_its_end_and_a_missing_one_reads_nothing() {
            let folder = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
            let file = folder.join("msctf.dll");
            assert_eq!(read_through(&file), std::fs::metadata(&file).unwrap().len());
            assert_eq!(read_through(&folder.join("mo-no-such-library.dll")), 0);
        }
    }
}
