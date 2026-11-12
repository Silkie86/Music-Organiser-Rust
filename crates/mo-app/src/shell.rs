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

/// Opens the Recycle Bin (the Trash on a Mac or Linux), where everything the app removes goes, as the C# app's
/// `Shell.OpenRecycleBin` does. `home` is your home folder, where a Mac keeps the Trash.
pub fn recycle_bin(home: &str, os: Os) -> Launch {
    match os {
        Os::Windows => launch("explorer.exe", &["shell:RecycleBinFolder"], Check::Ends),
        Os::Mac => launch("open", &[&format!("{}/.Trash", home.trim_end_matches('/'))], Check::ExitCode),
        Os::Linux => launch("xdg-open", &["trash:///"], Check::ExitCode),
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

/// A path as a card shows it, with a break allowed after each slash (a zero-width space, which isn't drawn), so a long
/// path wraps at its folders instead of in the middle of a name or past the card's edge. Only for showing: copying
/// or opening uses the path itself.
pub fn wrappable_path(path: &str) -> String {
    let mut shown = String::with_capacity(path.len() + path.len() / 4);
    for c in path.chars() {
        shown.push(c);
        if matches!(c, '\\' | '/') {
            shown.push('\u{200B}');
        }
    }
    shown
}

/// A path as a card shows it on one line: the whole path, then with its first folders cut to "…" one at a time
/// ("…\Drums\Kick.wav"), down to its file's name ("…\Kick.wav"), longest first. The card shows the longest that
/// fits, so it keeps its height from sound to sound (Solomon, 12 Nov 2026: "should just show the last folder or
/// whatever can fit in the same space so it doesnt move around each time"). A network share's two slashes and the
/// first slash of a Mac or Linux path are no cut. Only for showing: copying or opening uses the path itself.
pub fn path_tails(path: &str) -> Vec<String> {
    let slash = |c: char| matches!(c, '\\' | '/');
    let mut tails = vec![path.to_owned()];
    let mut before: Option<char> = None;
    for (at, c) in path.char_indices() {
        if slash(c) && at > 0 && !before.is_some_and(slash) && at + 1 < path.len() {
            tails.push(format!("…{}", &path[at..]));
        }
        before = Some(c);
    }
    tails
}

/// Starts it with nothing tying it to this app, so a DAW keeps running when the app closes or the launcher restarts
/// it (the C# app's lesson: a DAW started otherwise went down with the app, losing unsaved work). A launcher is
/// waited for, so call this from a background thread; a DAW is left running.
pub fn run(l: &Launch) -> io::Result<()> {
    let mut command = Command::new(&l.program);
    command.args(&l.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = detached(command)?;
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
fn detached(mut command: Command) -> io::Result<std::process::Child> {
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

/// A program to start, and where to say how it went.
#[cfg(unix)]
type Start = (Command, std::sync::mpsc::Sender<io::Result<std::process::Child>>);

/// The thread programs are started from, at the window's priority (see [`get_ready`]).
#[cfg(unix)]
static STARTER: std::sync::OnceLock<std::sync::mpsc::Sender<Start>> = std::sync::OnceLock::new();

/// Makes the thread that starts DAWs and other programs. Call it once from the main thread, before any background
/// work: on Linux and the Mac a program takes the priority of the thread that starts it, and one started from a
/// low-priority background thread would run at the lowest priority for its whole life, with no way back up.
pub fn get_ready() {
    #[cfg(unix)]
    STARTER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Start>();
        let made = std::thread::Builder::new().name("Starter".to_owned()).spawn(move || {
            for (command, back) in rx {
                let _ = back.send(spawn_detached(command));
            }
        });
        if let Err(e) = made {
            crate::log::warning("Open", &format!("Couldn't make the thread programs start from: {e}"));
        }
        tx
    });
}

#[cfg(unix)]
fn detached(command: Command) -> io::Result<std::process::Child> {
    let Some(starter) = STARTER.get() else { return spawn_detached(command) };
    let (back, answer) = std::sync::mpsc::channel();
    match starter.send((command, back)) {
        Ok(()) => answer.recv().unwrap_or_else(|_| Err(io::Error::other("the thread that starts programs stopped"))),
        // The starter is gone: start it from here, at this thread's priority.
        Err(std::sync::mpsc::SendError((command, _))) => spawn_detached(command),
    }
}

#[cfg(unix)]
fn spawn_detached(mut command: Command) -> io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    // Its own process group, so Ctrl+C in the launcher's terminal doesn't reach it.
    command.process_group(0).spawn()
}

#[cfg(not(any(windows, unix)))]
fn detached(mut command: Command) -> io::Result<std::process::Child> {
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
    fn the_recycle_bin_opens_where_each_system_keeps_it() {
        assert_eq!(recycle_bin("", Os::Windows), launch("explorer.exe", &["shell:RecycleBinFolder"], Check::Ends));
        assert_eq!(recycle_bin("/Users/me/", Os::Mac), launch("open", &["/Users/me/.Trash"], Check::ExitCode));
        assert_eq!(recycle_bin("/home/me", Os::Linux), launch("xdg-open", &["trash:///"], Check::ExitCode));
    }

    #[test]
    fn a_drives_own_folder_keeps_its_slash() {
        assert_eq!(folder_of(SONG), r"C:\Music\Night Drive");
        assert_eq!(folder_of(r"C:\song.flp"), r"C:\");
        assert_eq!(folder_of("/song.flp"), "/");
        assert_eq!(folder_of("/m/s/a.flp"), "/m/s");
        assert_eq!(folder_of("a.flp"), "");
    }

    #[test]
    fn a_shown_path_may_wrap_after_each_slash_only() {
        let wrap = "\u{200B}";
        assert_eq!(wrappable_path(r"J:\Samples\Kick.wav"), format!(r"J:\{wrap}Samples\{wrap}Kick.wav"));
        assert_eq!(wrappable_path("/m/s/a.wav"), format!("/{wrap}m/{wrap}s/{wrap}a.wav"));
        assert_eq!(wrappable_path("Boom Kick.wav"), "Boom Kick.wav");
        // Taking the marks out gives the path back.
        assert_eq!(wrappable_path(SONG).replace(wrap, ""), SONG);
    }

    #[test]
    fn a_path_on_one_line_cuts_its_first_folders_one_at_a_time() {
        assert_eq!(
            path_tails(r"D:\Samples\Drums\Kick.wav"),
            [r"D:\Samples\Drums\Kick.wav", r"…\Samples\Drums\Kick.wav", r"…\Drums\Kick.wav", r"…\Kick.wav"]
        );
        // A network share's two slashes, and the first slash of a Mac or Linux path, are no cut.
        assert_eq!(path_tails(r"\\nas\share\Kick.wav"), [r"\\nas\share\Kick.wav", r"…\share\Kick.wav", r"…\Kick.wav"]);
        assert_eq!(path_tails("/s/a.wav"), ["/s/a.wav", "…/a.wav"]);
        assert_eq!(path_tails("Kick.wav"), ["Kick.wav"]);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod starter_tests {
    use super::*;

    /// The nice value a program started by `run` gets, read back from what it prints.
    fn nice_of_a_started_program() -> Option<i32> {
        let out = std::env::temp_dir().join(format!("mo-app-starter-{}.txt", std::process::id()));
        let l = Launch {
            program: "sh".to_owned(),
            args: vec!["-c".to_owned(), format!("cut -d' ' -f19 /proc/self/stat > '{}'", out.display())],
            check: Check::ExitCode,
        };
        run(&l).ok()?;
        let nice = std::fs::read_to_string(&out).ok()?.trim().parse().ok();
        let _ = std::fs::remove_file(&out);
        nice
    }

    #[test]
    fn a_program_started_from_a_background_thread_keeps_the_app_s_priority() {
        get_ready();
        let ours = nice_of_a_started_program();
        assert!(ours.is_some(), "the program didn't say its priority");
        let theirs = crate::background::spawn("Open", nice_of_a_started_program).unwrap().join().unwrap();
        assert_eq!(theirs, ours, "started at a background thread's priority");
    }
}
