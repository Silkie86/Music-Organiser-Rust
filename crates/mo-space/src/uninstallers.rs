//! A plugin's own uninstaller, as Windows lists it under Settings, Apps, Installed apps: the C# app's `Uninstallers`.
//! Running a plugin's uninstaller tidies its licence, registry entries and shortcuts the way its maker intended, which
//! moving its files to the Recycle Bin can't. Only Windows keeps such a list; elsewhere there are none.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mo_formats::case;
use mo_formats::paths::is_rooted;

use crate::plugins::{family, norm};

/// A program's own uninstaller: its name and maker as Windows shows them, the command that runs it, and the room
/// Windows says the program takes (0 when it doesn't say).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uninstaller {
    pub name: String,
    pub publisher: String,
    pub command: String,
    pub bytes: u64,
}

/// Every uninstaller Windows knows of, for this user and for everyone (64- and 32-bit programs), each once. One whose
/// program has gone (a drive that's no longer there) is left out. Empty on a Mac or Linux.
pub fn all() -> Vec<Uninstaller> {
    let mut found = registry();
    let mut seen = std::collections::HashSet::new();
    found.retain(|u| seen.insert((u.name.clone(), u.command.clone())));
    found
}

#[cfg(windows)]
fn registry() -> Vec<Uninstaller> {
    use windows_registry::{CURRENT_USER, Key, LOCAL_MACHINE};
    const LIST: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
    const LIST_32: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";
    let mut found = Vec::new();
    for (hive, path) in [(LOCAL_MACHINE, LIST), (LOCAL_MACHINE, LIST_32), (CURRENT_USER, LIST)] {
        let hive: &Key = hive;
        let Ok(list) = hive.open(path) else { continue };
        let Ok(names) = list.keys() else { continue };
        for sub in names {
            let Ok(app) = list.open(&sub) else { continue };
            let Ok(name) = app.get_string("DisplayName") else { continue };
            // Parts of Windows and of other programs, hidden from Settings too.
            if name.is_empty() || app.get_u32("SystemComponent").is_ok_and(|v| v == 1) {
                continue;
            }
            // Often written with %ProgramFiles% and the like (REG_EXPAND_SZ), which .NET fills in as it reads.
            let Ok(command) = app.get_string("UninstallString").map(|c| expand(&c)) else { continue };
            // Left behind by a program that's gone (I:\Current\unins000.exe on Solomon's PC): there's nothing to run.
            if command.is_empty() || !can_run(&command) {
                continue;
            }
            let kb = app.get_u32("EstimatedSize").unwrap_or(0);
            let publisher = app.get_string("Publisher").unwrap_or_default();
            found.push(Uninstaller { name, publisher, command, bytes: u64::from(kb) * 1024 });
        }
    }
    found
}

#[cfg(not(windows))]
fn registry() -> Vec<Uninstaller> {
    Vec::new()
}

/// The uninstallers that look like this plugin's, best first: the program's name holds the plugin's (without its
/// version or format), and the maker's name counts in its favour. "Serum" finds "Xfer Serum 2"; a name of three letters
/// or fewer only matches a program named just that, and a family shorter than two ("M1") none, so neither can pick a
/// stranger. The same version ("Serum 2" for Serum 2) comes before another, and another version's ("iZotope Ozone 11"
/// for Ozone 9), or another plugin's whose name holds this one's ("Native Instruments Massive X" for Massive), comes
/// last (see [`is_other_version`]). `installed` is the installed plugins' names, which tell another version from this
/// one's.
pub fn for_plugin(plugin: &str, vendor: &str, all: &[Uninstaller], installed: &[String]) -> Vec<Uninstaller> {
    let fam = family(plugin);
    if fam.chars().count() < 2 {
        return Vec::new();
    }
    let (maker, full) = (norm(vendor), norm(plugin));
    let mut found: Vec<(bool, std::cmp::Reverse<u32>, usize, &Uninstaller)> = all
        .iter()
        .map(|u| (u, score(u, &fam, &maker, &full)))
        .filter(|(_, s)| *s > 0)
        .map(|(u, s)| (is_other_version(plugin, u, installed), std::cmp::Reverse(s), u.name.encode_utf16().count(), u))
        .collect();
    // Stable, as .NET's OrderBy is: uninstallers that tie keep Windows' order.
    found.sort_by_key(|(other, s, len, _)| (*other, *s, *len));
    found.into_iter().map(|(.., u)| u.clone()).collect()
}

/// Whether the program's name gives another version than the plugin's ("iZotope Ozone 11" for Ozone 9), or is another
/// plugin's whose name holds this one's ("Native Instruments Massive X" for Massive: [`another_plugins`]). Running it
/// would remove a plugin you may still use, so it's never picked for you. A program whose name gives no version isn't
/// another one. Nor is one for a plugin whose own name gives none, as it could be any version (Kontakt 6 installs as
/// "Kontakt", under "Native Instruments Kontakt 6"), unless that version is installed too: "Xfer Serum 2" is Serum 2's
/// when Serum 2 is beside Serum.
pub fn is_other_version(plugin: &str, u: &Uninstaller, installed: &[String]) -> bool {
    let fam = family(plugin);
    if fam.chars().count() < 2 {
        return false;
    }
    if another_plugins(&u.name, &fam) {
        return true;
    }
    let Some(theirs) = major_after(&u.name, &fam) else { return false };
    if let Some(mine) = major_after(plugin, &fam) {
        return theirs != mine;
    }
    installed.iter().any(|n| family(n) == fam && major_after(n, &fam) == Some(theirs))
}

/// The whole number written straight after the plugin's family in a name ("iZotope Ozone 11 Advanced": 11, "Serum2":
/// 2, "Diva v1.4.5": 1), or `None` when none follows it. `family` is as [`crate::plugins::family`] gives it (small
/// letters and digits only), so a name with spaces or dashes ("Guitar Rig 7", "Pro-Q 3") is found too.
pub fn major_after(name: &str, family: &str) -> Option<u32> {
    let (_, rest) = *places(name, family).first()?;
    let mut rest = rest.trim_start_matches([' ', '-', '_', '.']);
    if let Some(after_v) = rest.strip_prefix(['v', 'V']).filter(|r| r.starts_with(|c: char| c.is_ascii_digit())) {
        rest = after_v;
    }
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Words that may follow a plugin's name in its own uninstaller's: a format, a build, or the uninstaller itself
/// ("Serum x64", "Diva VST3", "Arcade Uninstaller"). "v" is a version ("Analog Lab V"), as [`family`] takes it.
const NOT_ANOTHER: [&str; 20] = [
    "v",
    "x64",
    "x86",
    "x32",
    "64bit",
    "32bit",
    "win",
    "win64",
    "win32",
    "vst",
    "vst2",
    "vst3",
    "aax",
    "au",
    "clap",
    "plugin",
    "plugins",
    "uninstall",
    "uninstaller",
    "remove",
];

/// Words that may follow a plugin's version in its own uninstaller's name: its edition ("Ozone 9 Advanced").
const EDITIONS: [&str; 10] =
    ["advanced", "standard", "elements", "pro", "essentials", "intro", "lite", "le", "se", "edition"];

/// Whether a program's name holds the plugin's family only as part of another plugin's name: inside a longer word
/// ("ValhallaSupermassive" for Massive), or with a word straight after it ("Native Instruments Massive X", "iZotope
/// Ozone Imager" for Ozone), or after its version ("Battery 4 Factory Library" for Battery 4). A version after it alone
/// or with an edition ("Ozone 9 Advanced", "Diva v1.4.5"), "mk II", a word of [`NOT_ANOTHER`], or what's in brackets
/// ("Serum (64-bit)") isn't another plugin's; nor is a name that holds the family again on its own (a maker named as
/// the plugin is: "Vital Audio Vital"). The whole-app review of 19 Nov, PM-6: Uninstall picked Massive X's uninstaller
/// for Massive, without a word, when Massive had none of its own.
fn another_plugins(name: &str, family: &str) -> bool {
    let places = places(name, family);
    !places.is_empty() && places.iter().all(|&(before, after)| another_plugins_at(name, before, after))
}

/// Whether the plugin's family, found in a program's name between `before` and `after` ([`places`]), is part of
/// another plugin's name there ([`another_plugins`]).
fn another_plugins_at(name: &str, before: &str, after: &str) -> bool {
    // Inside a longer word: a letter or digit straight before it, with no change from small letters to a capital or
    // between letters and digits ("XferSerum" starts a word at its S).
    if let (Some(prev), Some(first)) = (before.chars().next_back(), name[before.len()..].chars().next())
        && prev.is_alphanumeric()
        && !(prev.is_lowercase() && first.is_uppercase())
        && prev.is_numeric() == first.is_numeric()
    {
        return true;
    }
    const BETWEEN: [char; 4] = [' ', '-', '_', '.'];
    let first_word = |text: &str| -> String {
        text.chars().take_while(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
    };
    let after = after.trim_start_matches(BETWEEN);
    let word = first_word(after);
    let version = |w: &str| w.starts_with(|c: char| c.is_ascii_digit());
    let mark = |w: &str| {
        w.strip_prefix("mk").is_some_and(|r| r.chars().all(|c| matches!(c, 'i' | 'v' | 'x') || c.is_ascii_digit()))
    };
    let this_plugins = |w: &str| w.is_empty() || mark(w) || NOT_ANOTHER.contains(&w);
    if version(&word) || word.strip_prefix('v').is_some_and(version) {
        // The word after the version ("9", "v1.4.5"): only an edition or one of NOT_ANOTHER's is this plugin's.
        let past = after.trim_start_matches(['v', 'V']).trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
        let next = first_word(past.trim_start_matches(BETWEEN));
        return !(this_plugins(&next) || EDITIONS.contains(&next.as_str()));
    }
    !this_plugins(&word)
}

/// Each place the plugin's family is in a program's name, first to last, found the way names are compared (small
/// letters and digits only): the name's own text before it and after it (`"Native Instruments "` and `" X"` in "Native
/// Instruments Massive X"). None when it isn't in the name.
fn places<'a>(name: &'a str, family: &str) -> Vec<(&'a str, &'a str)> {
    // Each letter or digit of the name in small letters, with where it starts and ends in the name.
    let mut kept = String::new();
    let mut spans = Vec::new();
    for (at, c) in name.char_indices() {
        let small = case::lower(c);
        if small.is_alphabetic() || small.is_numeric() {
            kept.push(small);
            spans.push((at, at + c.len_utf8()));
        }
    }
    let Some(more) = family.chars().count().checked_sub(1) else { return Vec::new() };
    kept.match_indices(family)
        .filter_map(|(start, _)| {
            let first = kept[..start].chars().count();
            Some((name.get(..spans.get(first)?.0)?, name.get(spans.get(first + more)?.1..)?))
        })
        .collect()
}

/// How well an uninstaller's name fits the plugin: 0 not at all.
fn score(u: &Uninstaller, family: &str, maker: &str, full: &str) -> u32 {
    let name = norm(&u.name);
    // Programs that hold plugins but aren't one: a DAW's own uninstaller must never come up for its devices.
    if is_daw(&name) {
        return 0;
    }
    let mut score = if family.chars().count() < 4 {
        if name == family { 3 } else { 0 }
    } else if name == family {
        3
    } else if name.contains(family) {
        2
    } else {
        0
    };
    if score == 0 {
        return 0;
    }
    if maker.chars().count() >= 3 && (norm(&u.publisher).contains(maker) || name.contains(maker)) {
        score += 2;
    }
    // The same version.
    if name.contains(full) {
        score += 2;
    }
    score
}

const DAWS: [&str; 10] = [
    "flstudio",
    "bitwigstudio",
    "studioone",
    "fenderstudio",
    "ableton",
    "cubase",
    "reaper",
    "cakewalk",
    "protools",
    "logicpro",
];

fn is_daw(name: &str) -> bool {
    DAWS.iter().any(|d| name.contains(d))
}

/// Whether the command's program is there: a full path must exist; a bare name (`MsiExec.exe`) is found by Windows.
pub fn can_run(command: &str) -> bool {
    let file = expand(&split(command).0);
    !is_rooted(&file) || Path::new(&file).is_file()
}

/// `%Name%` replaced with the setting of that name (`%ProgramFiles%`), as Windows does; one that isn't set stays.
fn expand(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('%') {
        let Some(close) = rest[open + 1..].find('%') else { break };
        let name = &rest[open + 1..open + 1 + close];
        out.push_str(&rest[..open]);
        match std::env::var(name) {
            Ok(value) => out.push_str(&value),
            Err(_) => out.push_str(&rest[open..open + close + 2]),
        }
        rest = &rest[open + close + 2..];
    }
    out.push_str(rest);
    out
}

/// An uninstall command split into the program and what's passed to it: `"C:\Program Files\UVI\unins000.exe" /x`,
/// `MsiExec.exe /X{GUID}`, or a path with spaces and no quotes (some installers write them like that).
pub fn split(command: &str) -> (String, String) {
    let c = command.trim();
    if let Some(rest) = c.strip_prefix('"') {
        return match rest.find('"') {
            Some(end) => (rest[..end].to_owned(), rest[end + 1..].trim().to_owned()),
            None => (c.trim_matches('"').to_owned(), String::new()),
        };
    }
    // ASCII small letters keep every byte where it was.
    if let Some(exe) = c.to_ascii_lowercase().find(".exe").filter(|&at| at > 0) {
        return (c[..exe + 4].to_owned(), c[exe + 4..].trim().to_owned());
    }
    match c.find(' ').filter(|&at| at > 0) {
        Some(space) => (c[..space].to_owned(), c[space + 1..].trim().to_owned()),
        None => (c.to_owned(), String::new()),
    }
}

/// The command that starts an uninstaller the way Settings, Apps does: in its own window, which asks its own questions
/// (and Windows' administrator question when it needs one), then waits for it and what it starts to finish (many copy
/// themselves to Temp, start the copy and end). PowerShell's `Start-Process` does what Explorer does; the program and
/// what's passed to it reach it as settings, never as part of the script, so nothing in them can run as a command. It
/// says how it went on one line: `ended N` with the uninstaller's exit code, `said no`, or `not started: why`. Windows
/// PowerShell's `Start-Process` passes on Windows' "cancelled by the user" only as text, so that's recognised by
/// Windows' own wording of it, in whatever language Windows speaks.
fn command_for(u: &Uninstaller) -> Command {
    const SCRIPT: &str = "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; \
        try { $a = @{ FilePath = $env:MO_UNINSTALL_FILE; PassThru = $true; Wait = $true; ErrorAction = 'Stop' }; \
        if ($env:MO_UNINSTALL_ARGS) { $a.ArgumentList = $env:MO_UNINSTALL_ARGS }; \
        $p = Start-Process @a; if ($p) { 'ended ' + $p.ExitCode } else { 'ended 0' } } \
        catch { $e = $_.Exception; $no = $false; \
        $c = (New-Object System.ComponentModel.Win32Exception 1223).Message; \
        while ($e) { if ($e.NativeErrorCode -eq 1223 -or $e.Message.Contains($c)) { $no = $true }; \
        $e = $e.InnerException }; \
        if ($no) { 'said no' } else { 'not started: ' + $_.Exception.Message } }";
    let (file, args) = split(&u.command);
    // Windows' own PowerShell, never one that happens to sit beside the app or earlier on the PATH.
    let windows = std::env::var_os("SystemRoot").map_or_else(|| "C:\\Windows".into(), std::path::PathBuf::from);
    let mut cmd = Command::new(windows.join(r"System32\WindowsPowerShell\v1.0\powershell.exe"));
    cmd.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", SCRIPT])
        .env("MO_UNINSTALL_FILE", file)
        .env("MO_UNINSTALL_ARGS", args);
    // No black window flashing up for PowerShell itself.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// How running an uninstaller went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ran {
    /// It ended saying it's done (or done once Windows restarts).
    Finished,
    /// It ended without saying it finished (a code that isn't "done", or nothing at all): what it said. Whether the
    /// plugin's files went says how it went.
    Unclear(String),
    /// Nothing changed: it didn't start, you said no to Windows' administrator question, or you cancelled it. Why.
    NothingChanged(String),
    /// You stopped waiting, or the wait ran out: it may still be open in its own window.
    StoppedWaiting,
}

impl Ran {
    /// How it went, in words ("" when it finished).
    pub fn text(&self) -> &str {
        match self {
            Ran::Finished | Ran::StoppedWaiting => "",
            Ran::Unclear(why) | Ran::NothingChanged(why) => why,
        }
    }
}

/// What the command says about how it went, as the app says it.
fn outcome(said: &str) -> Ran {
    let line = said.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    if line == "said no" {
        return Ran::NothingChanged("You said no to Windows' administrator question, so nothing changed.".to_owned());
    }
    if let Some(why) = line.strip_prefix("not started: ") {
        return Ran::NothingChanged(format!("It didn't start: {why}"));
    }
    match line.strip_prefix("ended ").and_then(|c| c.trim().parse::<i64>().ok()) {
        // Done, or done once Windows restarts (Windows Installer's 3010 and 1641).
        Some(0 | 3010 | 1641) => Ran::Finished,
        // Windows Installer's "cancelled".
        Some(1602) => Ran::NothingChanged("It was cancelled, so the plugin is still installed.".to_owned()),
        Some(code) => Ran::Unclear(format!("It ended with code {code}, so it may not have finished.")),
        None => Ran::Unclear("It didn't say how it went.".to_owned()),
    }
}

/// Runs a plugin's uninstaller and waits until it's finished (it asks its own questions in its own window). Call it
/// away from the window's thread. Anything it leaves running (a web page it opens in a browser that wasn't open) is
/// waited for too, so the app shouldn't hold anything else up meanwhile. Says why when it couldn't start, when you said
/// no to Windows' administrator question, or when it ended without finishing.
pub fn run(u: &Uninstaller) -> Result<(), String> {
    match run_until(u, &AtomicBool::new(false), None) {
        Ran::Finished => Ok(()),
        Ran::StoppedWaiting => Err("It stopped being waited for.".to_owned()),
        Ran::Unclear(why) => Err(format!("{why} The plugin list is checked again anyway.")),
        Ran::NothingChanged(why) => Err(why),
    }
}

/// [`run`], but waiting only until you `stop` or `until` comes (`None` for no end): then the app stops waiting and the
/// uninstaller carries on in its own window ([`Ran::StoppedWaiting`]).
pub fn run_until(u: &Uninstaller, stop: &AtomicBool, until: Option<Instant>) -> Ran {
    if !cfg!(windows) {
        return Ran::NothingChanged(
            "Only Windows lists uninstallers: on this computer, remove the plugin's files.".to_owned(),
        );
    }
    if !can_run(&u.command) {
        return Ran::NothingChanged("Its uninstaller isn't there any more.".to_owned());
    }
    let mut cmd = command_for(u);
    cmd.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return Ran::NothingChanged(format!("Windows couldn't start it: {e}")),
    };
    // What PowerShell says is one short line at the end: read it on the side, so a full pipe never holds it up.
    let out = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut said = String::new();
        if let Some(mut out) = out {
            let _ = out.read_to_string(&mut said);
        }
        said
    });
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return outcome(&reader.join().unwrap_or_default()),
            Ok(None) => {}
            Err(e) => return Ran::Unclear(format!("It couldn't be watched ({e}).")),
        }
        if stop.load(Ordering::Relaxed) || until.is_some_and(|u| Instant::now() > u) {
            // Only PowerShell, which waits for it: the uninstaller itself carries on.
            let _ = child.kill();
            let _ = child.wait();
            return Ran::StoppedWaiting;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_program_and_what_it_is_given_reach_powershell_as_settings_only() {
        let u = Uninstaller {
            name: "X".into(),
            publisher: String::new(),
            command: r#""C:\Program Files\X\unins000.exe" /SILENT; Remove-Item C:\"#.into(),
            bytes: 0,
        };
        let cmd = command_for(&u);
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.iter().all(|a| !a.contains("unins000") && !a.contains("Remove-Item")));
        let envs: Vec<(String, String)> = cmd
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned())))
            .collect();
        assert!(envs.contains(&("MO_UNINSTALL_FILE".into(), r"C:\Program Files\X\unins000.exe".into())));
        assert!(envs.contains(&("MO_UNINSTALL_ARGS".into(), r"/SILENT; Remove-Item C:\".into())));
    }

    #[test]
    fn how_it_went_is_told_plainly() {
        assert_eq!(outcome("ended 0\r\n"), Ran::Finished);
        assert_eq!(outcome("ended 3010"), Ran::Finished);
        assert!(matches!(outcome("said no"), Ran::NothingChanged(w) if w.starts_with("You said no")));
        assert!(matches!(outcome("ended 1602"), Ran::NothingChanged(w) if w.contains("cancelled")));
        assert!(matches!(outcome("ended 2"), Ran::Unclear(w) if w.contains("code 2")));
        assert_eq!(
            outcome("not started: Datei nicht gefunden"),
            Ran::NothingChanged("It didn't start: Datei nicht gefunden".into())
        );
        assert!(matches!(outcome(""), Ran::Unclear(_)));
    }

    #[test]
    fn settings_in_a_path_are_filled_in() {
        let set = std::env::var("PATH").unwrap_or_default();
        assert_eq!(expand("%PATH%"), set);
        assert_eq!(expand("%MO_SURELY_NOT_SET_42%\\x.exe"), "%MO_SURELY_NOT_SET_42%\\x.exe");
        assert_eq!(expand("100% sure"), "100% sure");
    }
}
