//! Starting the background helper ([`crate::helper`]): at login, through Windows' own per-user startup list (no
//! administrator rights), and from the app when it opens.
//!
//! It's a choice in Settings ([`KEY`]), off unless turned on. While it's on, each start of the app makes sure
//! ([`turn_on`]) the startup list names this build's program (the launcher puts each build in a folder of its own),
//! and that the helper running is this build's: one left from an older build is asked to quit and this build's
//! started. Turning it off ([`turn_off`]) takes the line out of the startup list and asks the helper to quit.
//!
//! The startup list is behind [`LoginList`], so tests never touch the real one (rule 11).

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::app_settings::AppSettings;
use crate::helper::{self, FLAG};

/// The setting: "1" keeps the helper running (started at login and by the app). Off unless set.
pub const KEY: &str = "background_helper";
/// The helper's name in the startup list.
pub const ENTRY: &str = "MusicOrganiserHelper";
/// How long a helper from another build gets to quit before this build's is started all the same (it then finds the
/// port taken and stops, and the next start of the app tries again).
const QUIT_WAIT: Duration = Duration::from_secs(5);
/// How long a helper just started gets to answer, before the start counts as failed.
const ANSWER_WAIT: Duration = Duration::from_secs(8);

/// Whether the helper is wanted.
pub fn wanted(settings: &AppSettings) -> bool {
    settings.get(KEY) == Some("1")
}

/// The line the startup list runs: the program in quotes (its folder may have spaces), then the helper's flag.
pub fn command(exe: &Path) -> String {
    format!("\"{}\" {FLAG}", exe.display())
}

/// The system's list of what starts at login, as far as the helper's own line goes.
pub trait LoginList {
    /// The helper's line, if it's there.
    fn get(&self) -> io::Result<Option<String>>;
    /// Puts the helper's line in (or changes it).
    fn set(&self, command: &str) -> io::Result<()>;
    /// Takes the helper's line out; fine when it isn't there.
    fn remove(&self) -> io::Result<()>;
}

/// What became of the helper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Running {
    /// This build's was already running.
    Already,
    /// Started now.
    Started,
    /// One from another build (its version) was asked to quit, and this build's started.
    Replaced(String),
    /// Asked to quit.
    Stopped,
    /// None was running.
    NotRunning,
    /// It couldn't be started, and why.
    Failed(String),
}

/// How [`turn_on`] or [`turn_off`] went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Done {
    /// Why the startup list couldn't be changed, when it couldn't (`None` on a system without one, too:
    /// [`Done::login_list`] says which).
    pub login_problem: Option<String>,
    /// Whether this system has a startup list the app knows (Windows).
    pub login_list: bool,
    pub running: Running,
}

impl Done {
    /// One line for the log, and for the note in Settings.
    pub fn line(&self) -> String {
        let running = match &self.running {
            Running::Already => "The helper is running".to_owned(),
            Running::Started => "The helper was started".to_owned(),
            Running::Replaced(old) => format!("The helper from another build ({old}) was replaced by this one's"),
            Running::Stopped => "The helper was asked to quit".to_owned(),
            Running::NotRunning => "The helper wasn't running".to_owned(),
            Running::Failed(why) => format!("The helper couldn't be started: {why}"),
        };
        match (&self.login_problem, self.login_list) {
            (Some(why), _) => format!("{running}. The Windows startup list couldn't be changed: {why}"),
            (None, false) => format!("{running}. Starting at login is only on Windows for now"),
            (None, true) => running,
        }
    }
}

/// Makes sure the helper starts at login and runs now, from `exe` at `version` ([`crate::version::version`]).
/// `start` starts it ([`start_detached`]). Call it off the window's thread: it may wait for an older helper to quit.
pub fn turn_on(
    list: &dyn LoginList,
    port: u16,
    exe: &Path,
    version: &str,
    start: impl FnOnce(&Path) -> io::Result<()>,
) -> Done {
    let wanted = command(exe);
    // Written whenever it isn't already right, even when it couldn't be read (another kind of value under the name).
    let (login_problem, login_list) = match list.get() {
        Ok(Some(there)) if there == wanted => (None, true),
        Err(e) if e.kind() == io::ErrorKind::Unsupported => (None, false),
        _ => change(list.set(&wanted)),
    };
    let exe_text = exe.display().to_string();
    let ours = |hello: &helper::Hello| {
        hello.helper == version && (hello.exe.is_empty() || same_program(&hello.exe, &exe_text))
    };
    let replaced = match helper::hello(port) {
        Some(hello) if ours(&hello) => return Done { login_problem, login_list, running: Running::Already },
        Some(hello) => {
            helper::ask_to_quit(port);
            let until = Instant::now() + QUIT_WAIT;
            while helper::hello(port).is_some() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(100));
            }
            Some(hello.helper)
        }
        None => None,
    };
    let running = match start(exe) {
        Err(e) => Running::Failed(e.to_string()),
        // Started isn't running: only this build's helper answering on the port is.
        Ok(()) => {
            let until = Instant::now() + ANSWER_WAIT;
            loop {
                match helper::hello(port) {
                    Some(hello) if ours(&hello) => break replaced.map_or(Running::Started, Running::Replaced),
                    Some(hello) if Instant::now() >= until => {
                        break Running::Failed(format!("the helper from another build ({}) didn't quit", hello.helper));
                    }
                    None if Instant::now() >= until => break Running::Failed("it started but doesn't answer".into()),
                    _ => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        }
    };
    Done { login_problem, login_list, running }
}

/// Whether two paths name the same program: as Windows compares them (any case, with or without the `\\?\` in
/// front), exactly elsewhere.
fn same_program(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        let plain = |p: &str| p.strip_prefix(r"\\?\").unwrap_or(p).replace('/', "\\").to_lowercase();
        plain(a) == plain(b)
    } else {
        a == b
    }
}

/// Takes the helper out of the startup list and asks the one running to quit.
pub fn turn_off(list: &dyn LoginList, port: u16) -> Done {
    // Taken out without looking first: removing is fine when it isn't there, and works on a line that can't be read.
    let (login_problem, login_list) = change(list.remove());
    let running = if helper::ask_to_quit(port) { Running::Stopped } else { Running::NotRunning };
    Done { login_problem, login_list, running }
}

/// A change to the startup list as [`Done`] tells it: the problem, and whether there's a list at all.
fn change(done: io::Result<()>) -> (Option<String>, bool) {
    match done {
        Ok(()) => (None, true),
        Err(e) if e.kind() == io::ErrorKind::Unsupported => (None, false),
        Err(e) => (Some(e.to_string()), true),
    }
}

/// Starts `exe` as the helper, cut loose from this app as a DAW the app opens is ([`crate::shell::run`]): it has no
/// console (the launcher's would end it when its window closes, and Ctrl+C there would reach it), runs at the app's
/// own priority until it lowers its own, and outlives the app.
pub fn start_detached(exe: &Path) -> io::Result<()> {
    crate::shell::run(&crate::shell::Launch {
        program: exe.display().to_string(),
        args: vec![FLAG.to_owned()],
        check: crate::shell::Check::Started,
    })
}

/// The system's own startup list: on Windows the current user's `Run` key in the registry; elsewhere there's none
/// the app knows yet (every call says [`io::ErrorKind::Unsupported`]).
pub struct SystemList;

#[cfg(not(windows))]
impl LoginList for SystemList {
    fn get(&self) -> io::Result<Option<String>> {
        Err(io::ErrorKind::Unsupported.into())
    }

    fn set(&self, _command: &str) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }

    fn remove(&self) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows_list {
    use std::io;

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegDeleteValueW,
        RegGetValueW, RegOpenKeyExW, RegSetValueExW,
    };

    use super::{ENTRY, LoginList, SystemList};

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    fn check(code: u32) -> io::Result<()> {
        if code == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(code as i32)) }
    }

    /// The Run key, open for `rights`, closed when dropped.
    struct Key(HKEY);

    impl Key {
        fn open(rights: u32) -> io::Result<Key> {
            let mut key: HKEY = std::ptr::null_mut();
            // SAFETY: a null-ended path, and room for the handle it gives back.
            check(unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide(RUN).as_ptr(), 0, rights, &raw mut key) })?;
            Ok(Key(key))
        }
    }

    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: the handle RegOpenKeyExW gave, closed once.
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }

    impl LoginList for SystemList {
        fn get(&self) -> io::Result<Option<String>> {
            let key = Key::open(KEY_QUERY_VALUE)?;
            let name = wide(ENTRY);
            // Room for a long path; asked again with what it says it needs when that's more.
            let mut text = vec![0u16; 1024];
            loop {
                let mut bytes = (text.len() * 2) as u32;
                // SAFETY: `text` has `bytes` bytes of room, and `bytes` is given back as what was written or needed.
                let code = unsafe {
                    RegGetValueW(
                        key.0,
                        std::ptr::null(),
                        name.as_ptr(),
                        RRF_RT_REG_SZ,
                        std::ptr::null_mut(),
                        text.as_mut_ptr().cast(),
                        &raw mut bytes,
                    )
                };
                match code {
                    ERROR_SUCCESS => {
                        let len = (bytes as usize / 2).min(text.len());
                        let end = text[..len].iter().position(|c| *c == 0).unwrap_or(len);
                        return Ok(Some(String::from_utf16_lossy(&text[..end])));
                    }
                    ERROR_FILE_NOT_FOUND => return Ok(None),
                    ERROR_MORE_DATA if (bytes as usize) > text.len() * 2 => text.resize(bytes as usize / 2 + 1, 0),
                    other => return check(other).map(|()| None),
                }
            }
        }

        fn set(&self, command: &str) -> io::Result<()> {
            let key = Key::open(KEY_SET_VALUE)?;
            let text = wide(command);
            // SAFETY: `text` is a null-ended string of the size given, read during the call only.
            check(unsafe {
                RegSetValueExW(key.0, wide(ENTRY).as_ptr(), 0, REG_SZ, text.as_ptr().cast(), (text.len() * 2) as u32)
            })
        }

        fn remove(&self) -> io::Result<()> {
            let key = Key::open(KEY_SET_VALUE)?;
            // SAFETY: a null-ended name.
            match unsafe { RegDeleteValueW(key.0, wide(ENTRY).as_ptr()) } {
                ERROR_FILE_NOT_FOUND => Ok(()),
                code => check(code),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, TcpListener};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::helper::{Helper, Start};

    /// A startup list in memory.
    #[derive(Default)]
    struct FakeList {
        line: Mutex<Option<String>>,
        broken: bool,
    }

    impl LoginList for FakeList {
        fn get(&self) -> io::Result<Option<String>> {
            Ok(self.line.lock().unwrap().clone())
        }

        fn set(&self, command: &str) -> io::Result<()> {
            if self.broken {
                return Err(io::Error::other("the list is locked"));
            }
            *self.line.lock().unwrap() = Some(command.to_owned());
            Ok(())
        }

        fn remove(&self) -> io::Result<()> {
            *self.line.lock().unwrap() = None;
            Ok(())
        }
    }

    fn free_port() -> u16 {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port()
    }

    /// A helper in this test's own process, standing in for the program `start` would run.
    fn helper_on(port: u16) -> Arc<Helper> {
        helper_as(&crate::version::version(), port)
    }

    /// One that says it's build `version`.
    fn helper_as(version: &str, port: u16) -> Arc<Helper> {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            match Helper::start_as(version.to_owned(), port, free_port()).unwrap() {
                Start::Started(h) => return h,
                Start::AlreadyRunning => assert!(Instant::now() < until, "the port stayed taken"),
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn this_exe() -> PathBuf {
        std::env::current_exe().unwrap()
    }

    #[test]
    fn the_startup_line_quotes_the_program() {
        assert_eq!(
            command(Path::new(r"C:\My Apps\music-organiser.exe")),
            r#""C:\My Apps\music-organiser.exe" --helper"#
        );
    }

    #[test]
    fn off_unless_turned_on() {
        let mut settings = AppSettings::default();
        assert!(!wanted(&settings));
        settings.set(KEY, "1");
        assert!(wanted(&settings));
        settings.set(KEY, "0");
        assert!(!wanted(&settings));
    }

    #[test]
    fn turning_on_writes_the_list_and_starts_the_helper_once() {
        let list = FakeList::default();
        let port = free_port();
        let exe = this_exe();
        let started = Mutex::new(Vec::new());
        let start = |exe: &Path| {
            assert_eq!(exe, this_exe());
            started.lock().unwrap().push(helper_on(port));
            Ok(())
        };
        let done = turn_on(&list, port, &exe, &crate::version::version(), start);
        assert_eq!(done, Done { login_problem: None, login_list: true, running: Running::Started });
        assert_eq!(list.get().unwrap(), Some(command(&exe)));
        // Again: nothing to start.
        let done = turn_on(&list, port, &exe, &crate::version::version(), |_| panic!("started twice"));
        assert_eq!(done.running, Running::Already);
        assert_eq!(done.line(), "The helper is running");

        let done = turn_off(&list, port);
        assert_eq!(done.running, Running::Stopped);
        assert_eq!(list.get().unwrap(), None);
        assert!(started.lock().unwrap()[0].wait(Some(Duration::from_secs(5))));
        assert_eq!(turn_off(&list, free_port()).running, Running::NotRunning);
    }

    #[test]
    fn a_helper_from_another_build_is_replaced() {
        let list = FakeList::default();
        *list.line.lock().unwrap() = Some("\"C:\\old\\music-organiser.exe\" --helper".into());
        let port = free_port();
        let old = helper_on(port);
        let new = Mutex::new(None);
        let done = turn_on(&list, port, &this_exe(), "9.9.9 (newer)", |_| {
            *new.lock().unwrap() = Some(helper_as("9.9.9 (newer)", port));
            Ok(())
        });
        assert_eq!(done.running, Running::Replaced(crate::version::version()));
        assert!(old.quitting());
        assert_eq!(list.get().unwrap(), Some(command(&this_exe())), "the list names this build now");
        new.lock().unwrap().take().unwrap().quit();
    }

    #[test]
    fn says_when_the_list_or_the_start_fails() {
        let list = FakeList { broken: true, ..FakeList::default() };
        let done = turn_on(&list, free_port(), &this_exe(), "1", |_| Err(io::Error::other("no such program")));
        assert_eq!(done.running, Running::Failed("no such program".into()));
        assert_eq!(done.login_problem.as_deref(), Some("the list is locked"));
        assert!(done.line().contains("couldn't be started") && done.line().contains("startup list"));
    }

    #[test]
    fn a_line_that_cant_be_read_is_still_written_and_removed() {
        struct Unreadable(FakeList);
        impl LoginList for Unreadable {
            fn get(&self) -> io::Result<Option<String>> {
                Err(io::Error::other("another kind of value"))
            }
            fn set(&self, command: &str) -> io::Result<()> {
                self.0.set(command)
            }
            fn remove(&self) -> io::Result<()> {
                self.0.remove()
            }
        }
        let list = Unreadable(FakeList::default());
        let done = turn_on(&list, free_port(), &this_exe(), "1", |_| Err(io::Error::other("no")));
        assert_eq!(done.login_problem, None);
        assert_eq!(list.0.get().unwrap(), Some(command(&this_exe())));
        assert_eq!(turn_off(&list, free_port()).login_problem, None);
        assert_eq!(list.0.get().unwrap(), None);
    }

    #[test]
    fn a_helper_that_never_answers_is_a_failed_start() {
        // Nothing is started at all: the port stays silent.
        let done = turn_on(&FakeList::default(), free_port(), &this_exe(), "1", |_| Ok(()));
        assert_eq!(done.running, Running::Failed("it started but doesn't answer".into()));
    }

    #[test]
    fn the_same_program_whatever_its_spelling_on_windows() {
        assert!(same_program("/a/b", "/a/b"));
        assert_eq!(same_program(r"\\?\C:\Run\App.exe", r"c:\run\app.exe"), cfg!(windows));
    }

    #[test]
    fn a_system_without_a_list_still_runs_the_helper() {
        struct NoList;
        impl LoginList for NoList {
            fn get(&self) -> io::Result<Option<String>> {
                Err(io::ErrorKind::Unsupported.into())
            }
            fn set(&self, _: &str) -> io::Result<()> {
                Err(io::ErrorKind::Unsupported.into())
            }
            fn remove(&self) -> io::Result<()> {
                Err(io::ErrorKind::Unsupported.into())
            }
        }
        let port = free_port();
        let started = Mutex::new(None);
        let done = turn_on(&NoList, port, &this_exe(), &crate::version::version(), |_| {
            *started.lock().unwrap() = Some(helper_on(port));
            Ok(())
        });
        assert_eq!(done, Done { login_problem: None, login_list: false, running: Running::Started });
        assert!(done.line().ends_with("only on Windows for now"));
        assert_eq!(turn_off(&NoList, port), Done { login_problem: None, login_list: false, running: Running::Stopped });
    }
}
