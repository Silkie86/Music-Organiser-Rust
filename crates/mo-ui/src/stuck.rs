//! Where the window's thread is when it's stuck, for the freeze watchdog's lines (rule 12). A freeze "in no marked
//! step" says only that Slint itself was busy: handling input, laying out, or handing the frame to the screen. To
//! say which, the watchdog looks at the window's thread once a freeze has lasted [`crate::watchdog::STILL_STUCK`]:
//! it stops the thread for a moment, notes the functions it's in (up to [`MOST`]), lets it go again, and only then
//! looks up their names. The names go into the log as a `[Freeze]` line of their own.
//!
//! Also here: how much the app's memory had to be read back from the disk while it was stuck (page faults), since a
//! window left alone on a PC short of memory has its memory written out to the page file, and then takes seconds to
//! answer again.
//!
//! Only on Windows (64-bit Intel and AMD, Solomon's PC), the one place the freezes are seen; elsewhere nothing is
//! looked at and the lines say nothing more.

/// At most this many functions are noted, from the innermost.
#[cfg(all(windows, target_arch = "x86_64"))]
pub const MOST: usize = 48;

/// The window's thread, kept so the watchdog can look at it. Call it on the window's thread.
pub fn remember_this_thread() {
    #[cfg(all(windows, target_arch = "x86_64"))]
    windows_x64::remember_this_thread();
}

/// The functions the window's thread is in now, innermost first, each as "name (file:line)" where it's known, else
/// as "module+offset". `None` when it can't be looked at here.
pub fn window_thread_now() -> Option<Vec<String>> {
    #[cfg(all(windows, target_arch = "x86_64"))]
    return windows_x64::window_thread_now();
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    None
}

/// How often the app has had to find a page of its memory (most found at once in memory, some read back from the
/// disk) so far, and how much of its memory is in RAM now, in bytes. `None` where that isn't known.
pub fn memory_now() -> Option<Memory> {
    #[cfg(windows)]
    return windows_memory::memory_now();
    #[cfg(not(windows))]
    None
}

/// The app's memory at one moment; see [`memory_now`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Memory {
    pub page_faults: u64,
    pub in_ram: u64,
}

/// What happened to the memory over a freeze, for its line: "41,200 page faults while stuck, 812 MB in RAM".
pub fn memory_words(before: Option<Memory>, after: Option<Memory>) -> Option<String> {
    let (before, after) = (before?, after?);
    let faults = after.page_faults.saturating_sub(before.page_faults);
    Some(format!("{} page faults while stuck, {} MB in RAM", thousands(faults), after.in_ram / (1024 * 1024)))
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows_memory {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    pub fn memory_now() -> Option<super::Memory> {
        let mut counters =
            PROCESS_MEMORY_COUNTERS { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
        // SAFETY: the documented call, with a counters struct of the size it's told; GetCurrentProcess is a
        // pseudo handle that needs no closing.
        let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
        (ok != 0).then(|| super::Memory {
            page_faults: u64::from(counters.PageFaultCount),
            in_ram: counters.WorkingSetSize as u64,
        })
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[allow(unsafe_code)]
mod windows_x64 {
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HMODULE};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        CONTEXT, CONTEXT_FULL_AMD64, GetThreadContext, IMAGEHLP_LINEW64, RtlLookupFunctionEntry, RtlVirtualUnwind,
        SYMBOL_INFOW, SYMOPT_DEFERRED_LOADS, SYMOPT_LOAD_LINES, SYMOPT_UNDNAME, SymCleanup, SymFromAddrW,
        SymGetLineFromAddrW64, SymInitializeW, SymSetOptions, UNW_FLAG_NHANDLER,
    };
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
        GetModuleHandleExW,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT,
        THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME,
    };

    use super::MOST;

    /// The window's thread, opened so it can be stopped and looked at (kept open for the app's life).
    static WINDOW: OnceLock<usize> = OnceLock::new();

    pub fn remember_this_thread() {
        // SAFETY: the documented call for this thread's own id; the handle is checked before it's kept.
        let handle = unsafe {
            OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION, 0, GetCurrentThreadId())
        };
        if !handle.is_null() && WINDOW.set(handle as usize).is_err() {
            // SAFETY: the handle was just opened here and is kept nowhere else.
            unsafe { CloseHandle(handle) };
        }
    }

    /// CONTEXT must sit on a 16-byte boundary for GetThreadContext.
    #[repr(C, align(16))]
    struct Aligned(CONTEXT);

    pub fn window_thread_now() -> Option<Vec<String>> {
        let thread = *WINDOW.get()? as HANDLE;
        let (frames, count) = walk(thread)?;
        Some(names(&frames[..count]))
    }

    /// Stops the thread, notes the return addresses on its stack and lets it go. While it's stopped nothing here
    /// takes a lock or asks for memory (the stopped thread might hold the lock), so this can't hang the app: the
    /// addresses go into a fixed array and only get their names after the thread runs again.
    fn walk(thread: HANDLE) -> Option<([u64; MOST], usize)> {
        let mut frames = [0u64; MOST];
        let mut count = 0;
        // SAFETY: the documented calls. The thread is a live handle opened with these rights (the window's thread
        // lives as long as the app), and it's let go on every path below.
        unsafe {
            if SuspendThread(thread) == u32::MAX {
                return None;
            }
            // SAFETY: CONTEXT is plain data, all zeros is a valid start.
            let mut context: Aligned = std::mem::zeroed();
            context.0.ContextFlags = CONTEXT_FULL_AMD64;
            // GetThreadContext also waits for the stop to have happened (SuspendThread only asks for it).
            let got = GetThreadContext(thread, &mut context.0) != 0;
            if got {
                let ctx = &mut context.0;
                while count < MOST && ctx.Rip != 0 {
                    frames[count] = ctx.Rip;
                    count += 1;
                    let mut image_base = 0u64;
                    let entry = RtlLookupFunctionEntry(ctx.Rip, &mut image_base, std::ptr::null_mut());
                    if entry.is_null() {
                        // A leaf function: its return address is at the top of its stack.
                        if ctx.Rsp == 0 || !ctx.Rsp.is_multiple_of(8) {
                            break;
                        }
                        ctx.Rip = *(ctx.Rsp as *const u64);
                        ctx.Rsp += 8;
                    } else {
                        let mut handler_data = std::ptr::null_mut();
                        let mut frame = 0u64;
                        RtlVirtualUnwind(
                            UNW_FLAG_NHANDLER,
                            image_base,
                            ctx.Rip,
                            entry,
                            ctx,
                            &mut handler_data,
                            &mut frame,
                            std::ptr::null_mut(),
                        );
                    }
                }
            }
            ResumeThread(thread);
            got.then_some((frames, count))
        }
    }

    /// Each address's function name and line, where the app's debugging info has them, else its module and offset.
    fn names(frames: &[u64]) -> Vec<String> {
        // SAFETY: the documented dbghelp calls, from this one thread. The symbol handler is set up here and closed
        // again before returning, so a crash log's backtrace (which sets up its own) finds it free. If something set
        // it up already, it's used as it is and left open.
        unsafe {
            let process = GetCurrentProcess();
            SymSetOptions(SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS | SYMOPT_LOAD_LINES);
            let ours = SymInitializeW(process, std::ptr::null(), 1) != 0;
            let named = frames.iter().map(|&at| name(process, at)).collect();
            if ours {
                SymCleanup(process);
            }
            named
        }
    }

    /// Room for a name of up to 500 UTF-16 units after SYMBOL_INFOW's own fields.
    #[repr(C, align(8))]
    struct SymbolRoom {
        info: SYMBOL_INFOW,
        rest: [u16; 500],
    }

    unsafe fn name(process: HANDLE, at: u64) -> String {
        // SAFETY (for the whole function): the documented calls with structs of the sizes they're told.
        unsafe {
            // A return address points just past its call: look up the call itself.
            let call = at.saturating_sub(1);
            let mut room: SymbolRoom = std::mem::zeroed();
            room.info.SizeOfStruct = std::mem::size_of::<SYMBOL_INFOW>() as u32;
            room.info.MaxNameLen = 500;
            let mut offset = 0u64;
            let function = if SymFromAddrW(process, call, &mut offset, &mut room.info) != 0 {
                let len = (room.info.NameLen as usize).min(500);
                // The name runs on past SYMBOL_INFOW's one-unit array into the room after it.
                let start = (&raw const room).cast::<u8>().add(std::mem::offset_of!(SYMBOL_INFOW, Name)).cast::<u16>();
                let name = std::slice::from_raw_parts(start, len);
                Some(String::from_utf16_lossy(name))
            } else {
                None
            };
            let mut line: IMAGEHLP_LINEW64 = std::mem::zeroed();
            line.SizeOfStruct = std::mem::size_of::<IMAGEHLP_LINEW64>() as u32;
            let mut column = 0u32;
            let place = if SymGetLineFromAddrW64(process, call, &mut column, &mut line) != 0 && !line.FileName.is_null()
            {
                let len = (0..).take_while(|&i| *line.FileName.add(i) != 0).count();
                let file = String::from_utf16_lossy(std::slice::from_raw_parts(line.FileName, len));
                let short = file.rsplit(['\\', '/']).next().unwrap_or(&file).to_owned();
                Some(format!("{short}:{}", line.LineNumber))
            } else {
                None
            };
            match (function, place) {
                (Some(f), Some(p)) => format!("{f} ({p})"),
                (Some(f), None) => f,
                _ => module_offset(at),
            }
        }
    }

    /// "nvoglv64.dll+0x1a2b3c": the module an address is in and how far in.
    unsafe fn module_offset(at: u64) -> String {
        // SAFETY: the documented calls; the module isn't kept (its count isn't changed) and the name buffer's size
        // is passed.
        unsafe {
            let mut module: HMODULE = std::ptr::null_mut();
            let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
            if GetModuleHandleExW(flags, at as usize as *const u16, &mut module) == 0 {
                return format!("0x{at:x}");
            }
            let mut buffer = [0u16; 260];
            let len = GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) as usize;
            let path = String::from_utf16_lossy(&buffer[..len.min(buffer.len())]);
            let file = path.rsplit(['\\', '/']).next().unwrap_or(&path);
            format!("{file}+0x{:x}", at.saturating_sub(module as u64))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_over_a_freeze_in_words() {
        let before = Memory { page_faults: 1_000, in_ram: 0 };
        let after = Memory { page_faults: 42_200, in_ram: 812 * 1024 * 1024 };
        assert_eq!(
            memory_words(Some(before), Some(after)).as_deref(),
            Some("41,200 page faults while stuck, 812 MB in RAM")
        );
        assert_eq!(memory_words(None, Some(after)), None);
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    /// On Windows: a thread stuck in a function of ours is found in it.
    #[cfg(all(windows, target_arch = "x86_64"))]
    #[test]
    fn a_stuck_thread_is_found_where_it_is() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;
        static STOP: AtomicBool = AtomicBool::new(false);
        #[inline(never)]
        fn spin_here_for_the_test() {
            while !STOP.load(Ordering::Relaxed) {
                std::hint::spin_loop();
            }
        }
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            remember_this_thread();
            tx.send(()).unwrap();
            spin_here_for_the_test();
        });
        rx.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let frames = window_thread_now();
        STOP.store(true, Ordering::Relaxed);
        worker.join().unwrap();
        let frames = frames.expect("the thread was looked at");
        assert!(frames.iter().any(|f| f.contains("spin_here_for_the_test")), "{frames:#?}");
    }
}
