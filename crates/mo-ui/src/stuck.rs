//! Where the window's thread is when it's stuck, for the freeze watchdog's lines (rule 12). A freeze "in no marked
//! step" says only that Slint itself was busy: handling input, laying out, or handing the frame to the screen. To
//! say which, the watchdog looks at the window's thread once a freeze has lasted [`crate::watchdog::LOOK_AT_THREAD`]
//! (and again at [`crate::watchdog::STILL_STUCK`]):
//! it stops the thread for a moment, copies its registers and the used part of its stack, lets it go again, and only
//! then works out from the copy which functions it was in (up to `MOST`) and looks up their names. The names go into the log as a `[Freeze]` line of their own.
//!
//! Also here: how many pages of memory the app (all its threads) had to find while it was stuck (page faults), since a
//! window left alone on a PC short of memory has its memory written out to the page file, and then takes seconds to
//! answer again. The count includes pages found again in RAM and every thread's, so it's a hint, not proof.
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

/// What happened to the memory over a freeze, less `own` faults (the watchdog's own looks), for its line: "41,200 page
/// faults in the app while stuck, 812 MB in RAM".
pub fn memory_words(before: Option<Memory>, after: Option<Memory>, own: u64) -> Option<String> {
    let (before, after) = (before?, after?);
    let faults = after.page_faults.saturating_sub(before.page_faults).saturating_sub(own);
    Some(format!(
        "{} page faults in the app while stuck, {} MB in RAM",
        thousands(faults),
        after.in_ram / (1024 * 1024)
    ))
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

    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, HMODULE};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        CONTEXT, CONTEXT_FULL_AMD64, GetThreadContext, IMAGEHLP_LINEW64, RtlLookupFunctionEntry, RtlVirtualUnwind,
        SYMBOL_INFOW, SYMOPT_DEFERRED_LOADS, SYMOPT_LOAD_LINES, SYMOPT_UNDNAME, SymFromAddrW, SymGetLineFromAddrW64,
        SymGetOptions, SymInitializeW, SymSetOptions, UNW_FLAG_NHANDLER,
    };
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
        GetModuleHandleExW,
    };
    use windows_sys::Win32::System::Threading::{
        CreateMutexA, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, GetCurrentThreadStackLimits,
        INFINITE, OpenThread, ReleaseMutex, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION,
        THREAD_SUSPEND_RESUME, WaitForSingleObject,
    };

    use super::MOST;

    /// The window's thread, opened so it can be stopped and looked at (kept open for the app's life), and its stack's
    /// lowest and highest addresses.
    struct Window {
        thread: usize,
        low: u64,
        high: u64,
    }

    static WINDOW: OnceLock<Window> = OnceLock::new();

    /// Room past the copied stack's top, so a step of the walk that reads a little past it reads zeros, not
    /// whatever memory comes next.
    const SLACK: usize = 64 * 1024;

    pub fn remember_this_thread() {
        let (mut low, mut high) = (0usize, 0usize);
        // SAFETY: the documented calls, for this thread's own id and stack; the handle is checked before it's kept.
        let handle = unsafe {
            GetCurrentThreadStackLimits(&mut low, &mut high);
            OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION, 0, GetCurrentThreadId())
        };
        if handle.is_null() {
            return;
        }
        let window = Window { thread: handle as usize, low: low as u64, high: high as u64 };
        if low >= high || WINDOW.set(window).is_err() {
            // SAFETY: the handle was just opened here and is kept nowhere else.
            unsafe { CloseHandle(handle) };
        }
    }

    /// CONTEXT must sit on a 16-byte boundary for GetThreadContext.
    #[repr(C, align(16))]
    struct Aligned(CONTEXT);

    pub fn window_thread_now() -> Option<Vec<String>> {
        let window = WINDOW.get()?;
        let (frames, count) = walk(window)?;
        Some(names(&frames[..count]))
    }

    /// Stops the thread, copies its registers and the used part of its stack, and lets it go at once. Only then is
    /// the copy walked for the return addresses: walking asks Windows for each function's unwind data, which can take
    /// a lock (a DLL being loaded, say) that the stopped thread might hold, and that would hang it for good. While
    /// it's stopped nothing here takes a lock or asks for memory: the room for the copy is made before.
    fn walk(window: &Window) -> Option<([u64; MOST], usize)> {
        let size = usize::try_from(window.high - window.low).ok()?;
        // The stack as it was, low to high, with zeros past its top.
        let mut copy = vec![0u8; size + SLACK];
        let thread = window.thread as HANDLE;
        // SAFETY: CONTEXT is plain data; all zeros is a valid start.
        let mut context: Aligned = unsafe { std::mem::zeroed() };
        context.0.ContextFlags = CONTEXT_FULL_AMD64;
        // SAFETY: the documented calls on a live handle opened with these rights (the window's thread lives as long
        // as the app), and the thread is let go on every path. The copy reads only the stack between the thread's
        // stack pointer and its stack's top, which is all in use, so all there to read; it lands in `copy` at the
        // same distance from its start as from the stack's lowest address, within its length.
        let copied = unsafe {
            if SuspendThread(thread) == u32::MAX {
                return None;
            }
            // GetThreadContext also waits for the stop to have happened (SuspendThread only asks for it).
            let got = GetThreadContext(thread, &mut context.0) != 0;
            let rsp = context.0.Rsp;
            let usable = got && rsp >= window.low && rsp < window.high && rsp.is_multiple_of(8);
            if usable {
                let from = (rsp - window.low) as usize;
                std::ptr::copy_nonoverlapping(rsp as *const u8, copy.as_mut_ptr().add(from), size - from);
            }
            ResumeThread(thread);
            usable
        };
        if !copied {
            return None;
        }
        Some(unwind(&mut context.0, window, &copy))
    }

    /// Walks a copied stack: each register that points into the stack is moved to the same place in the copy, so
    /// every read the walk makes lands in the copy, and each step is checked to stay in it.
    fn unwind(ctx: &mut CONTEXT, window: &Window, copy: &[u8]) -> ([u64; MOST], usize) {
        let start = copy.as_ptr() as u64;
        let delta = start.wrapping_sub(window.low);
        let used_from = ctx.Rsp.wrapping_add(delta);
        let end = start + (window.high - window.low);
        let in_copy = |at: u64| at >= used_from && at < end && at.is_multiple_of(8);
        let on_stack = |at: u64| at >= window.low && at < window.high;
        for reg in [&mut ctx.Rsp, &mut ctx.Rbp, &mut ctx.Rbx, &mut ctx.Rsi, &mut ctx.Rdi, &mut ctx.R12] {
            if on_stack(*reg) {
                *reg = reg.wrapping_add(delta);
            }
        }
        for reg in [&mut ctx.R13, &mut ctx.R14, &mut ctx.R15] {
            if on_stack(*reg) {
                *reg = reg.wrapping_add(delta);
            }
        }
        let mut frames = [0u64; MOST];
        let mut count = 0;
        while count < MOST && ctx.Rip != 0 && in_copy(ctx.Rsp) {
            frames[count] = ctx.Rip;
            count += 1;
            let mut image_base = 0u64;
            // SAFETY: the documented calls. Every stack read they make is at or above Rsp, which is in the copy (with
            // zeros past its top); the frame register, when a function uses one, is checked to be in the copy too.
            unsafe {
                let entry = RtlLookupFunctionEntry(ctx.Rip, &mut image_base, std::ptr::null_mut());
                if entry.is_null() {
                    // A leaf function: its return address is at the top of its stack.
                    ctx.Rip = *(ctx.Rsp as *const u64);
                    ctx.Rsp += 8;
                    continue;
                }
                // The unwind data's fourth byte: the frame register in its low four bits (0 for none).
                let info = image_base + u64::from((*entry).Anonymous.UnwindInfoAddress);
                let frame_register = *((info + 3) as *const u8) & 0x0F;
                if frame_register != 0 && !in_copy(register(ctx, frame_register)) {
                    break;
                }
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
        (frames, count)
    }

    /// A general register by its number in unwind data (0 RAX, 1 RCX, 2 RDX, 3 RBX, 4 RSP, 5 RBP, 6 RSI, 7 RDI, then
    /// R8 to R15).
    fn register(ctx: &CONTEXT, n: u8) -> u64 {
        match n {
            0 => ctx.Rax,
            1 => ctx.Rcx,
            2 => ctx.Rdx,
            3 => ctx.Rbx,
            4 => ctx.Rsp,
            5 => ctx.Rbp,
            6 => ctx.Rsi,
            7 => ctx.Rdi,
            8 => ctx.R8,
            9 => ctx.R9,
            10 => ctx.R10,
            11 => ctx.R11,
            12 => ctx.R12,
            13 => ctx.R13,
            14 => ctx.R14,
            _ => ctx.R15,
        }
    }

    /// Windows' symbol helper (dbghelp) works for one caller at a time in the whole process. Rust's own backtraces
    /// (a crash log's) take a named lock for it, by this name; holding the same lock keeps the two apart.
    fn dbghelp_lock_name() -> [u8; 33] {
        let mut name = *b"Local\\RustBacktraceMutex00000000\0";
        // SAFETY: the documented call.
        let mut id = unsafe { GetCurrentProcessId() };
        let mut index = name.len() - 1;
        while id > 0 {
            name[index - 1] = match (id & 0xF) as u8 {
                h @ 0..=9 => b'0' + h,
                h => b'A' + (h - 10),
            };
            id >>= 4;
            index -= 1;
        }
        name
    }

    /// The symbol helper's lock, made once and kept for the app's life.
    static DBGHELP_LOCK: OnceLock<usize> = OnceLock::new();

    /// Each address's function name and line, where the app's debugging info has them, else its module and offset.
    fn names(frames: &[u64]) -> Vec<String> {
        let lock = *DBGHELP_LOCK.get_or_init(|| {
            let name = dbghelp_lock_name();
            // SAFETY: the documented call with a NUL-ended name; a failure gives a null handle, checked below.
            unsafe { CreateMutexA(std::ptr::null(), 0, name.as_ptr()) as usize }
        });
        if lock == 0 {
            return frames.iter().map(|&at| module_offset(at)).collect();
        }
        // SAFETY: the documented dbghelp calls, under the lock Rust's backtraces take. The symbol helper is set up once
        // and kept, as Rust's backtraces keep theirs: whichever sets it up first, the other's set-up fails harmlessly
        // and it uses that one. Symbols load as each module is first asked about, so the app's own symbols are read
        // once.
        unsafe {
            if WaitForSingleObject(lock as HANDLE, INFINITE) != 0 {
                return frames.iter().map(|&at| module_offset(at)).collect();
            }
            SET_UP.get_or_init(|| {
                SymSetOptions(SymGetOptions() | SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS | SYMOPT_LOAD_LINES);
                let ok = SymInitializeW(GetCurrentProcess(), std::ptr::null(), 1) != 0;
                (ok, if ok { 0 } else { GetLastError() })
            });
            let process = GetCurrentProcess();
            let named = frames.iter().map(|&at| name(process, at)).collect();
            ReleaseMutex(lock as HANDLE);
            named
        }
    }

    /// Whether this app set the symbol helper up, and Windows' error number if it couldn't.
    static SET_UP: OnceLock<(bool, u32)> = OnceLock::new();
    /// Windows' error number from the last name that couldn't be found.
    static LAST_MISS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    /// How the symbol helper is doing, for a test that fails: "set up: (true, 0), last miss: 126".
    pub fn symbol_state() -> String {
        format!("set up: {:?}, last miss: {}", SET_UP.get(), LAST_MISS.load(std::sync::atomic::Ordering::Relaxed))
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
                LAST_MISS.store(GetLastError(), std::sync::atomic::Ordering::Relaxed);
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
    fn module_offset(at: u64) -> String {
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
            memory_words(Some(before), Some(after), 0).as_deref(),
            Some("41,200 page faults in the app while stuck, 812 MB in RAM")
        );
        assert_eq!(
            memory_words(Some(before), Some(after), 1_200).as_deref(),
            Some("40,000 page faults in the app while stuck, 812 MB in RAM")
        );
        assert_eq!(memory_words(None, Some(after), 0), None);
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
        assert!(
            frames.iter().any(|f| f.contains("spin_here_for_the_test")),
            "{frames:#?} ({})",
            windows_x64::symbol_state()
        );
    }
}
