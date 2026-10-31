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
}

fn launch(program: &str, args: &[&str]) -> Launch {
    Launch { program: program.to_owned(), args: args.iter().map(|a| (*a).to_owned()).collect() }
}

/// Opens `path` with `exe` (the DAW picked in Settings: an .exe on Windows, an .app on a Mac) when there is one,
/// else with whatever opens that kind of file, which for a project is the DAW that owns its ending.
pub fn open(path: &str, exe: Option<&str>, os: Os) -> Launch {
    let exe = exe.map(str::trim).filter(|e| !e.is_empty());
    match (os, exe) {
        (Os::Mac, Some(app)) if app.trim_end_matches('/').to_ascii_lowercase().ends_with(".app") => {
            launch("open", &["-a", app, path])
        }
        (_, Some(exe)) => launch(exe, &[path]),
        (Os::Windows, None) => launch("explorer.exe", &[path]),
        (Os::Mac, None) => launch("open", &[path]),
        (Os::Linux, None) => launch("xdg-open", &[path]),
    }
}

/// Shows `path` picked in its folder. `exists` is whether the file is there: when it isn't, its folder opens instead.
pub fn reveal(path: &str, exists: bool, os: Os) -> Launch {
    if !exists {
        return open(folder_of(path), None, os);
    }
    match os {
        // Explorer wants "/select," and the path as two words, as the C# app passes them.
        Os::Windows => launch("explorer.exe", &["/select,", path]),
        Os::Mac => launch("open", &["-R", path]),
        // Not every Linux file manager can pick a file; they all open a folder.
        Os::Linux => open(folder_of(path), None, os),
    }
}

/// The folder part of a path: everything before its file name, whichever way its slashes lean.
pub fn folder_of(path: &str) -> &str {
    let name = mo_formats::paths::file_name(path);
    path.get(..path.len() - name.len()).unwrap_or_default().trim_end_matches(['\\', '/'])
}

/// Starts it with nothing tying it to this app, so a DAW keeps running when the app closes or the launcher restarts
/// it (the C# app's lesson: a DAW started otherwise went down with the app, losing unsaved work). Waits for it on
/// this thread, so call it from a background thread.
pub fn run(l: &Launch) -> io::Result<()> {
    let mut command = Command::new(&l.program);
    command.args(&l.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = detached(&mut command)?;
    // Explorer and the launchers end at once; a DAW started straight away runs until it's closed.
    let _ = child.wait();
    Ok(())
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
        assert_eq!(open(SONG, Some(fl), Os::Windows), launch(fl, &[SONG]));
        assert_eq!(open(SONG, Some("  "), Os::Windows), launch("explorer.exe", &[SONG]));
        assert_eq!(open(SONG, None, Os::Windows), launch("explorer.exe", &[SONG]));
        assert_eq!(open("/m/a.bwproject", Some("/Applications/Bitwig Studio.app/"), Os::Mac).args[0], "-a");
        assert_eq!(open("/m/a.bwproject", None, Os::Mac), launch("open", &["/m/a.bwproject"]));
        assert_eq!(open("/m/a.song", None, Os::Linux), launch("xdg-open", &["/m/a.song"]));
        assert_eq!(daw_exe_key("FL Studio"), "daw_exe:FL Studio");
    }

    #[test]
    fn reveal_picks_the_file_or_opens_its_folder() {
        assert_eq!(reveal(SONG, true, Os::Windows), launch("explorer.exe", &["/select,", SONG]));
        assert_eq!(reveal(SONG, false, Os::Windows), launch("explorer.exe", &[r"C:\Music\Night Drive"]));
        assert_eq!(reveal("/m/s/a.flp", true, Os::Mac), launch("open", &["-R", "/m/s/a.flp"]));
        assert_eq!(reveal("/m/s/a.flp", true, Os::Linux), launch("xdg-open", &["/m/s"]));
    }
}
