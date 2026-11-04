//! Catching a panic on purpose, in one of two ways, so the crash handler (`mo-ui`'s `main.rs`) can tell what a panic
//! is as it starts:
//!
//! - [`catch`]: for work that can stop in a way the app expects. A reader that trips over a damaged file (release
//!   builds stop on arithmetic that overflows, too), or a library that stops when Windows won't do what it asks. The
//!   work gives an error and the app goes on; the session log gets one warning line, not a crash.
//! - [`catch_crash`]: for a bug the thread must live through (a page's worker going on to its next job). It's still a
//!   crash, logged in full, with its backtrace and a crash log of its own.
//!
//! A panic is Rust stopping what a thread is doing: it goes back out through the functions the thread was in, letting
//! go of what each one had, until something catches it (one of these) or the thread ends. Std calls the crash handler
//! as the panic starts, before any of that, so the handler has to know then what will catch it: the innermost catch
//! running on the panicking thread ([`panic_starting`]). So every catch in the app goes through here (clippy.toml
//! forbids `std::panic::catch_unwind` everywhere else), or the handler would take a panic one catch catches for
//! another's.
//!
//! The crash handler only hears of a panic as it starts, on the thread it starts on. So work inside a [`catch`] never:
//!
//! - waits for work on other threads (rayon's `join`, `scope` or a parallel iterator, or `std::thread::scope`): a
//!   rayon thread that waits runs other jobs meanwhile, whose panics would be taken for this catch's, and the panics
//!   those hand back come without the crash handler hearing of them;
//! - hands on a panic caught somewhere else (`std::panic::resume_unwind`, which clippy.toml forbids too): another panic
//!   as that one goes out (in a `Drop`) would be taken for caught, though nothing can catch it;
//! - lets a panic reach an `extern "C"` function: std ends the app there, and the panic before it is only a warning.

use std::cell::Cell;
use std::panic::UnwindSafe;

/// What catches a panic that's starting, as [`panic_starting`] tells the crash handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Catcher {
    /// A [`catch`]: the panic is one the app expects, not a crash.
    Catch,
    /// A [`catch_crash`]: a crash, though the thread goes on.
    CatchCrash,
    /// No catch of the app's: a crash. The thread ends (on the window's thread, so does the app), unless a library
    /// catches the panic on its way (rayon hands a job's panic to the thread waiting for it); or std ends the app.
    Nothing,
}

/// The innermost catch running on a thread.
#[derive(Clone, Copy)]
struct Innermost {
    /// Whether it's a [`catch`], not a [`catch_crash`].
    expected: bool,
    /// Whether a panic the crash handler was told of ([`panic_starting`]) is on its way out to it. Another panic as it
    /// goes (in a `Drop` it runs) can't be caught by anything: std ends the app. A library's own catch inside this one
    /// would leave the mark until this one ends, and a later panic in it would be taken for a crash (no library the
    /// app catches around has one).
    unwinding: bool,
}

thread_local! {
    /// The innermost catch running on this thread, if any. Each catch puts itself here as it starts, and puts back the
    /// one around it as it ends.
    static INNERMOST: Cell<Option<Innermost>> = const { Cell::new(None) };
}

/// Runs `work`, catching a panic in it as [`std::panic::catch_unwind`] does, for work that can stop in a way the app
/// expects (a damaged file trips a reader). The crash handler writes such a panic as one warning line, not a crash.
/// The work never waits for other threads' work (see the top of this file).
pub fn catch<R>(work: impl FnOnce() -> R + UnwindSafe) -> std::thread::Result<R> {
    run(true, work)
}

/// Runs `work`, catching a panic in it as [`std::panic::catch_unwind`] does, for a bug the thread must live through.
/// The crash handler still writes such a panic as a crash, in full; only the thread goes on.
pub fn catch_crash<R>(work: impl FnOnce() -> R + UnwindSafe) -> std::thread::Result<R> {
    run(false, work)
}

fn run<R>(expected: bool, work: impl FnOnce() -> R + UnwindSafe) -> std::thread::Result<R> {
    let around = swap(Some(Innermost { expected, unwinding: false }));
    // The one place the app catches a panic (see the top of this file).
    #[allow(clippy::disallowed_methods)]
    let caught = std::panic::catch_unwind(work);
    swap(around);
    caught
}

/// Puts `innermost` in [`INNERMOST`], and gives what was there.
fn swap(innermost: Option<Innermost>) -> Option<Innermost> {
    INNERMOST.try_with(|i| i.replace(innermost)).ok().flatten()
}

/// What will catch the panic that's starting on this thread. Only for the crash handler, once for each panic, before
/// anything else: it also marks the catch as having that panic on its way out to it, so another panic while it goes is
/// known to be one nothing can catch. Never panics itself.
pub fn panic_starting() -> Catcher {
    // A build that ends the app at any panic (`panic = "abort"`) catches nothing.
    if cfg!(not(panic = "unwind")) {
        return Catcher::Nothing;
    }
    let expected = INNERMOST.try_with(|i| {
        let innermost = i.get().filter(|innermost| !innermost.unwinding)?;
        i.set(Some(Innermost { unwinding: true, ..innermost }));
        Some(innermost.expected)
    });
    match expected {
        Ok(Some(true)) => Catcher::Catch,
        Ok(Some(false)) => Catcher::CatchCrash,
        Ok(None) | Err(_) => Catcher::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Mutex, Once, PoisonError};
    use std::thread::ThreadId;

    use super::*;

    /// Each panic in this test program as it started: its thread, what was to catch it, and what it said.
    static SEEN: Mutex<Vec<(ThreadId, Catcher, String)>> = Mutex::new(Vec::new());
    /// In what the tests' own panics say, so the crash handler below doesn't report them as failures.
    const ON_PURPOSE: &str = "(on purpose)";

    /// Puts in this test program's crash handler, which notes what was to catch each panic (in [`SEEN`]). A test that
    /// fails (a panic not meant) is still reported, as std reports it.
    fn handled() {
        static SET: Once = Once::new();
        SET.call_once(|| {
            let default = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                let catcher = panic_starting();
                let said = info.payload_as_str().unwrap_or_default().to_owned();
                if !said.contains(ON_PURPOSE) {
                    default(info);
                }
                SEEN.lock().unwrap_or_else(PoisonError::into_inner).push((std::thread::current().id(), catcher, said));
            }));
        });
    }

    /// What was to catch each panic on `thread` since the last look, oldest first, with what it said.
    fn seen_on(thread: ThreadId) -> Vec<(Catcher, String)> {
        let mut seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
        let (here, others): (Vec<_>, Vec<_>) = seen.drain(..).partition(|(t, ..)| *t == thread);
        *seen = others;
        here.into_iter().map(|(_, catcher, said)| (catcher, said)).collect()
    }

    fn seen_here() -> Vec<(Catcher, String)> {
        seen_on(std::thread::current().id())
    }

    /// A panic, saying `said`.
    fn stops(said: &str) {
        panic!("{said} {ON_PURPOSE}");
    }

    /// `catcher` for a panic that said `said` (as [`stops`] says it).
    fn by(catcher: Catcher, said: &str) -> (Catcher, String) {
        (catcher, format!("{said} {ON_PURPOSE}"))
    }

    #[test]
    fn a_catch_gives_what_the_work_gives_or_its_panic() {
        handled();
        assert_eq!(catch(|| 7).ok(), Some(7));
        assert_eq!(catch_crash(|| "done").ok(), Some("done"));
        let panic = catch(|| stops("a damaged file")).expect_err("caught");
        assert_eq!(panic.downcast_ref::<String>().map(String::as_str), Some("a damaged file (on purpose)"));
        assert!(catch_crash(|| stops("a bug")).is_err());
        assert_eq!(seen_here(), [by(Catcher::Catch, "a damaged file"), by(Catcher::CatchCrash, "a bug")]);
    }

    #[test]
    fn a_panic_is_expected_only_where_a_catch_catches_it() {
        handled();
        let _ = catch(|| stops("in a catch"));
        let thread = std::thread::spawn(|| stops("on a thread nothing here catches it on"));
        let other = thread.thread().id();
        assert!(thread.join().is_err());
        let _ = catch_crash(|| stops("in a catch for crashes"));
        assert_eq!(seen_here(), [by(Catcher::Catch, "in a catch"), by(Catcher::CatchCrash, "in a catch for crashes")]);
        assert_eq!(seen_on(other), [by(Catcher::Nothing, "on a thread nothing here catches it on")]);
    }

    #[test]
    fn the_innermost_catch_catches_and_the_one_around_it_after() {
        handled();
        let _ = catch_crash(|| {
            let _ = catch(|| stops("a damaged file inside a catch for crashes"));
            stops("then a bug");
        });
        let _ = catch(|| {
            let _ = catch_crash(|| stops("a bug inside a catch"));
            stops("then a damaged file");
        });
        assert_eq!(
            seen_here(),
            [
                by(Catcher::Catch, "a damaged file inside a catch for crashes"),
                by(Catcher::CatchCrash, "then a bug"),
                by(Catcher::CatchCrash, "a bug inside a catch"),
                by(Catcher::Catch, "then a damaged file"),
            ]
        );
    }

    #[test]
    fn a_catch_in_a_drop_as_a_panic_goes_out_catches_its_own() {
        handled();
        struct CatchesItsOwn;
        impl Drop for CatchesItsOwn {
            fn drop(&mut self) {
                let _ = catch_crash(|| stops("a bug in a drop, as the first goes out"));
            }
        }
        let first = catch(|| {
            let _drops = CatchesItsOwn;
            stops("a damaged file");
        });
        assert!(first.is_err(), "the first is caught where it was to be");
        let _ = catch(|| stops("and the next"));
        assert_eq!(
            seen_here(),
            [
                by(Catcher::Catch, "a damaged file"),
                by(Catcher::CatchCrash, "a bug in a drop, as the first goes out"),
                by(Catcher::Catch, "and the next"),
            ]
        );
    }

    #[test]
    fn a_catch_is_marked_by_its_panic_until_it_ends() {
        assert_eq!(panic_starting(), Catcher::Nothing, "no catch running");
        assert_eq!(catch(|| (panic_starting(), panic_starting())).ok(), Some((Catcher::Catch, Catcher::Nothing)));
        assert_eq!(catch_crash(panic_starting).ok(), Some(Catcher::CatchCrash), "a new catch isn't marked");
        assert_eq!(panic_starting(), Catcher::Nothing, "the catch that ended took its mark with it");
        let inside = catch(|| {
            let marked = panic_starting();
            (marked, catch_crash(panic_starting).ok(), panic_starting())
        });
        assert_eq!(
            inside.ok(),
            Some((Catcher::Catch, Some(Catcher::CatchCrash), Catcher::Nothing)),
            "a catch inside a marked one isn't marked, and the marked one still is once it ends"
        );
    }

    /// Set for the copy of this test program that panics in a `Drop` as a caught panic goes out (see the test below).
    const IN_THE_COPY: &str = "MO_CATCH_TEST_PANIC_IN_A_DROP";
    /// What the copy prints before what was to catch each panic.
    const SAID: &str = "Caught by: ";

    /// Only in the copy the test below runs; in an ordinary run it does nothing. A panic in a `catch` meets a `Drop`
    /// that panics too as it goes out: std can't go on from that, and ends the copy. Each panic's catcher is printed.
    #[test]
    fn a_panic_in_a_drop_as_a_caught_one_goes_out_in_a_copy() {
        if std::env::var_os(IN_THE_COPY).is_none() {
            return;
        }
        std::panic::set_hook(Box::new(|info| {
            let catcher = panic_starting();
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{SAID}{catcher:?}: {}", info.payload_as_str().unwrap_or_default());
            let _ = out.flush();
        }));
        struct PanicsAsItGoes;
        impl Drop for PanicsAsItGoes {
            fn drop(&mut self) {
                if std::thread::panicking() {
                    panic!("a bug in a drop, as the first goes out");
                }
            }
        }
        let _ = catch(|| {
            let _drops = PanicsAsItGoes;
            stops("a damaged file");
        });
        println!("{SAID}nothing: the copy went on");
    }

    /// A panic in a `Drop` as a caught panic goes out isn't caught: std ends the app, so it's never the catch's.
    #[test]
    fn a_panic_in_a_drop_as_a_caught_one_goes_out_is_nothing_s() {
        let copy = std::process::Command::new(std::env::current_exe().expect("this test program"))
            .args(["--exact", "tests::a_panic_in_a_drop_as_a_caught_one_goes_out_in_a_copy", "--nocapture"])
            .arg("--test-threads=1")
            .env(IN_THE_COPY, "1")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("the copy ran");
        let out = String::from_utf8_lossy(&copy.stdout);
        // The first may follow the test program's own "test … " on its line.
        let caught: Vec<&str> = out.lines().filter_map(|l| l.split_once(SAID).map(|(_, said)| said)).collect();
        // On Windows std ends the copy as the second panic leaves its drop; elsewhere it first panics again, saying
        // so, where nothing can catch it either.
        let mut ends = vec!["Catch: a damaged file (on purpose)", "Nothing: a bug in a drop, as the first goes out"];
        if !cfg!(target_env = "msvc") {
            ends.push("Nothing: panic in a destructor during cleanup");
        }
        assert_eq!(caught, ends, "{out}");
        assert!(!copy.status.success(), "std ended the copy: {out}");
    }
}
