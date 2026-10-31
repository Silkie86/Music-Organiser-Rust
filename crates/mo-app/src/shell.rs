//! Opening things with the operating system, as the C# app's `Shell` does: a project in its DAW, a file shown in its
//! folder. Windows first; a Mac and Linux use their own launchers. What would run is worked out apart from running it,
//! so the tests can check it without starting anything (rule 11: tests never reach a real DAW or folder).

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// The setting that holds the program picked in Settings for a DAW, the same key as the C# app's.
pub fn daw_exe_key(daw: &str) -> String {
    format!("daw_exe:{daw}")
}

/// The systems the app runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Linux,
}

impl Os {
    pub const THIS: Os = if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::Mac
    } else {
        Os::Linux
    };
}

/// A program to start and what to give it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    /// How to tell it worked: by its exit code, or not at all.
    pub check: Check,
}

/// How [`run`] tells a start worked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// A Mac's `open` and Linux's `xdg-open` end at once and say by their exit code whether something opened.
    ExitCode,
    /// Explorer ends at once, and its exit code says nothing (it's 1 when all went well).
    Ends,
    /// A DAW started straight away runs until it's closed: started is all there is to know.
    Started,
}

fn launch(program: &str, args: &[&str], check: Check) -> Launch {
    Launch { program: program.to_owned(), args: args.iter().map(|a| (*a).to_owned()).collect(), check }
}

/// The program picked in Settings, written as [`open`] reads it, when it's there: an .exe (any file), or a Mac's
/// .app folder. Asks the disk, so not on the window's thread.
pub fn usable_exe(exe: &str) -> bool {
    let exe = exe.trim();
    !exe.is_empty() && (Path::new(exe).is_file() || (is_app(exe) && Path::new(exe).is_dir()))
}

fn is_app(exe: &str) -> bool {
    exe.trim_end_matches('/').to_ascii_lowercase().ends_with(".app")
}

/// Opens `path` with `exe` (the DAW picked in Settings: an .exe on Windows, an .app on a Mac) when there is one,
/// else with whatever opens that kind of file, which for a project is the DAW that owns its ending.
pub fn open(path: &str, exe: Option<&str>, os: Os) -> Launch {
    let exe = exe.map(str::trim).filter(|e| !e.is_empty());
    match (os, exe) {
        (Os::Mac, Some(app)) if is_app(app) => launch("open", &["-a", app, path], Check::ExitCode),
        (_, Some(exe)) => launch(exe, &[path], Check::Started),
        (Os::Windows, None) => launch("explorer.exe", &[path], Check::Ends),
        (Os::Mac, None) => launch("open", &[path], Check::ExitCode),
        (Os::Linux, None) => launch("xdg-open", &[path], Check::ExitCode),
    }
}

/// Shows `path` picked in `folder`, the folder it's in. `exists` is whether the file is there: when it isn't, the
/// folder opens instead.
pub fn reveal(path: &str, folder: &str, exists: bool, os: Os) -> Launch {
    if !exists {
        return open(folder, None, os);
    }
    match os {
        // Explorer wants "/select," and the path as two words, as the C# app passes them.
        Os::Windows => launch("explorer.exe", &["/select,", path], Check::Ends),
        Os::Mac => launch("open", &["-R", path], Check::ExitCode),
        // Not every Linux file manager can pick a file; they all open a folder.
        Os::Linux => open(folder, None, os),
    }
}

/// The folder part of a path: everything before its file name, whichever way its slashes lean. A drive's own folder
/// keeps its slash ("C:\\", "/").
pub fn folder_of(path: &str) -> &str {
    let name = mo_formats::paths::file_name(path);
    let with_slash = path.get(..path.len() - name.len()).unwrap_or_default();
    let folder = with_slash.trim_end_matches(['\\', '/']);
    if (folder.is_empty() || folder.ends_with(':')) && with_slash.len() > folder.len() {
        with_slash.get(..folder.len() + 1).unwrap_or(folder)
    } else {
        folder
    }
}

/// Starts it with nothing tying it to this app, so a DAW keeps running when the app closes or the launcher restarts
/// it (the C# app's lesson: a DAW started otherwise went down with the app, losing unsaved work). A launcher is
/// waited for, so call this from a background thread; a DAW is left running.
pub fn run(l: &Launch) -> io::Result<()> {
    let mut command = Command::new(&l.program);
    command.args(&l.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = detached(&mut command)?;
    match l.check {
        Check::Started => {
            // Collected when it closes, on a thread of its own, so it doesn't linger after it ends.
            let _ = crate::background::spawn("DAW", move || child.wait());
            Ok(())
        }
        Check::Ends => child.wait().map(|_| ()),
        Check::ExitCode => match child.wait()? {
            s if s.success() => Ok(()),
            s => Err(io::Error::other(format!("{} said it couldn't ({s})", l.program))),
        },
    }
}

#[cfg(windows)]
fn detached(command: &mut Command) -> io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x8;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let first = command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB).spawn();
    match first {
        Ok(child) => Ok(child),
        // A job that doesn't let its programs leave it: start it detached anyway.
        Err(_) => command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP).spawn(),
    }
}

#[cfg(unix)]
fn detached(command: &mut Command) -> io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    // Its own process group, so Ctrl+C in the launcher's terminal doesn't reach it.
    command.process_group(0).spawn()
}

#[cfg(not(any(windows, unix)))]
fn detached(command: &mut Command) -> io::Result<std::process::Child> {
    command.spawn()
}

/// Whether the file is where the library says. Asks the disk, so not on the window's thread (rule 12).
pub fn exists(path: &str) -> bool {
    Path::new(path).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SONG: &str = r"C:\Music\Night Drive\Night Drive_14.flp";

    #[test]
    fn opens_with_the_daw_from_settings_or_the_system() {
        let fl = r"C:\Program Files\Image-Line\FL Studio 2024\FL64.exe";
        assert_eq!(open(SONG, Some(fl), Os::Windows), launch(fl, &[SONG], Check::Started));
        assert_eq!(open(SONG, Some("  "), Os::Windows), launch("explorer.exe", &[SONG], Check::Ends));
        assert_eq!(open(SONG, None, Os::Windows), launch("explorer.exe", &[SONG], Check::Ends));
        let bitwig = open("/m/a.bwproject", Some("/Applications/Bitwig Studio.APP/"), Os::Mac);
        assert_eq!((bitwig.args[0].as_str(), bitwig.check), ("-a", Check::ExitCode));
        assert_eq!(open("/m/a.bwproject", None, Os::Mac), launch("open", &["/m/a.bwproject"], Check::ExitCode));
        assert_eq!(open("/m/a.song", None, Os::Linux), launch("xdg-open", &["/m/a.song"], Check::ExitCode));
        assert!(!usable_exe("  "));
        assert!(!usable_exe("/no/such/FL64.exe"));
        assert_eq!(daw_exe_key("FL Studio"), "daw_exe:FL Studio");
    }

    #[test]
    fn reveal_picks_the_file_or_opens_its_folder() {
        let folder = r"C:\Music\Night Drive";
        assert_eq!(reveal(SONG, folder, true, Os::Windows), launch("explorer.exe", &["/select,", SONG], Check::Ends));
        assert_eq!(reveal(SONG, folder, false, Os::Windows), launch("explorer.exe", &[folder], Check::Ends));
        assert_eq!(reveal("/m/s/a.flp", "/m/s", true, Os::Mac), launch("open", &["-R", "/m/s/a.flp"], Check::ExitCode));
        assert_eq!(reveal("/m/s/a.flp", "/m/s", true, Os::Linux), launch("xdg-open", &["/m/s"], Check::ExitCode));
    }

    #[test]
    fn a_drives_own_folder_keeps_its_slash() {
        assert_eq!(folder_of(SONG), r"C:\Music\Night Drive");
        assert_eq!(folder_of(r"C:\song.flp"), r"C:\");
        assert_eq!(folder_of("/song.flp"), "/");
        assert_eq!(folder_of("/m/s/a.flp"), "/m/s");
        assert_eq!(folder_of("a.flp"), "");
    }
}
