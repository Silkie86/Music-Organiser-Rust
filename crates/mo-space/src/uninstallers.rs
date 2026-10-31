//! A plugin's own uninstaller, as Windows lists it under Settings, Apps, Installed apps: the C# app's `Uninstallers`.
//! Running a plugin's uninstaller tidies its licence, registry entries and shortcuts the way its maker intended, which
//! moving its files to the Recycle Bin can't. Only Windows keeps such a list; elsewhere there are none.

use std::path::Path;
use std::process::Command;

use mo_formats::case;

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
            let Ok(command) = app.get_string("UninstallString") else { continue };
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
/// stranger. The same version
/// ("Serum 2" for Serum 2) comes before another, and another version's ("iZotope Ozone 11" for Ozone 9) comes last
/// (see [`is_other_version`]). `installed` is the installed plugins' names, which tell another version from this one's.
pub fn for_plugin(plugin: &str, vendor: &str, all: &[Uninstaller], installed: &[String]) -> Vec<Uninstaller> {
    let fam = family(plugin);
    if fam.chars().count() < 2 {
        return Vec::new();
    }
    let (maker, full) = (norm(vendor), norm(plugin));
    let mut found: Vec<(&Uninstaller, u32)> =
        all.iter().map(|u| (u, score(u, &fam, &maker, &full))).filter(|(_, s)| *s > 0).collect();
    found.sort_by_key(|(u, s)| {
        (is_other_version(plugin, u, installed), std::cmp::Reverse(*s), u.name.encode_utf16().count())
    });
    found.into_iter().map(|(u, _)| u.clone()).collect()
}

/// Whether the program's name gives another version than the plugin's: "iZotope Ozone 11" for Ozone 9. Running it
/// would remove a plugin you may still use, so it's never picked for you. A program whose name gives no version isn't
/// another one. Nor is one for a plugin whose own name gives none, as it could be any version (Kontakt 6 installs as
/// "Kontakt", under "Native Instruments Kontakt 6"), unless that version is installed too: "Xfer Serum 2" is Serum 2's
/// when Serum 2 is beside Serum.
pub fn is_other_version(plugin: &str, u: &Uninstaller, installed: &[String]) -> bool {
    let fam = family(plugin);
    if fam.chars().count() < 2 {
        return false;
    }
    let Some(theirs) = major_after(&u.name, &fam) else { return false };
    if let Some(mine) = major_after(plugin, &fam) {
        return theirs != mine;
    }
    installed.iter().any(|n| family(n) == fam && major_after(n, &fam) == Some(theirs))
}

/// The whole number written straight after the plugin's family in a name ("iZotope Ozone 11 Advanced": 11, "Serum2":
/// 2, "Diva v1.4.5": 1), or `None` when none follows it.
fn major_after(name: &str, family: &str) -> Option<u32> {
    // Each letter or digit of the name in small letters, with where it ends, so the family is found the way names are
    // compared.
    let mut kept = String::new();
    let mut ends = Vec::new();
    for (at, c) in name.char_indices() {
        let small = case::lower(c);
        if small.is_alphabetic() || small.is_numeric() {
            kept.push(small);
            ends.push(at + c.len_utf8());
        }
    }
    let start = kept.find(family)?;
    let last = kept[..start].chars().count() + family.chars().count().checked_sub(1)?;
    let mut rest = name.get(*ends.get(last)?..)?;
    rest = rest.trim_start_matches([' ', '-', '_', '.']);
    if let Some(after_v) = rest.strip_prefix(['v', 'V']).filter(|r| r.starts_with(|c: char| c.is_ascii_digit())) {
        rest = after_v;
    }
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
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
    let (file, _) = split(command);
    !is_rooted(&file) || Path::new(&expand(&file)).is_file()
}

/// Whether a path starts at a drive or the top of one (`C:\…`, `\…`, `/…`), as .NET's `Path.IsPathRooted` says.
fn is_rooted(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with(['\\', '/']) || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
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

/// What Windows said when you turned down its administrator question (`ERROR_CANCELLED`).
const SAID_NO: i32 = 1223;

/// The command that starts an uninstaller the way Settings, Apps does: in its own window, which asks its own questions
/// (and Windows' administrator question when it needs one), then waits for it to finish. PowerShell's `Start-Process`
/// does what Explorer does; the program and what's passed to it reach it as settings, never as part of the script, so
/// nothing in them can run as a command.
fn command_for(u: &Uninstaller) -> Command {
    const SCRIPT: &str = "try { $a = @{ FilePath = $env:MO_UNINSTALL_FILE; PassThru = $true; ErrorAction = 'Stop' }; \
        if ($env:MO_UNINSTALL_ARGS) { $a.ArgumentList = $env:MO_UNINSTALL_ARGS }; \
        $p = Start-Process @a; $p.WaitForExit(); exit 0 } \
        catch [System.InvalidOperationException] { [Console]::Error.WriteLine($_.Exception.Message); exit 1223 } \
        catch { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }";
    let (file, args) = split(&u.command);
    let mut cmd = Command::new("powershell.exe");
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

/// Runs a plugin's uninstaller and waits until it's finished (it asks its own questions in its own window). Call it
/// away from the window's thread. Says why when it couldn't start, or when you said no to Windows' administrator
/// question.
pub fn run(u: &Uninstaller) -> Result<(), String> {
    if !cfg!(windows) {
        return Err("Only Windows lists uninstallers: on this computer, remove the plugin's files.".to_owned());
    }
    if !can_run(&u.command) {
        return Err("Its uninstaller isn't there any more.".to_owned());
    }
    let out = command_for(u).output().map_err(|e| format!("Windows couldn't start it: {e}"))?;
    match out.status.code() {
        Some(0) => Ok(()),
        Some(code) => {
            let said = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            // Start-Process says "canceled by the user" when you turn down the administrator question.
            if code == SAID_NO && said.to_ascii_lowercase().contains("cancel") {
                Err("You said no to Windows' administrator question, so nothing changed.".to_owned())
            } else if said.is_empty() {
                Err("It didn't start.".to_owned())
            } else {
                Err(said)
            }
        }
        None => Err("It was stopped.".to_owned()),
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
    fn settings_in_a_path_are_filled_in() {
        let set = std::env::var("PATH").unwrap_or_default();
        assert_eq!(expand("%PATH%"), set);
        assert_eq!(expand("%MO_SURELY_NOT_SET_42%\\x.exe"), "%MO_SURELY_NOT_SET_42%\\x.exe");
        assert_eq!(expand("100% sure"), "100% sure");
    }
}
