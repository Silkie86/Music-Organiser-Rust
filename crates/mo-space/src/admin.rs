//! The administrator step: one step the app needs administrator rights for (a plugin folder Windows guards, such as
//! Spectrasonics' in ProgramData, renamed for a move; a link it guards pointed somewhere else; a file it guards sent to
//! the Recycle Bin). The app starts itself again as administrator for just that step, Windows asks you once, and the app
//! waits for it, the same as Explorer's "You'll need to provide administrator permission". The C# app's `AdminStep`.
//!
//! How the two copies of the app talk: the app writes what the step is to do in a list in your Temp folder, and starts
//! itself again with [`MOVE_FOLDER`] or [`RECYCLE_BIN`], the list, where to write how it went, and the list's
//! fingerprint (its SHA-256). Any program of yours could change the list while Windows asks, but not the fingerprint,
//! which is on the step's command line: a list that doesn't match does nothing. The step checks each line again before
//! it acts (a path reached through a link is left alone, so a link put in meanwhile can't steer it somewhere else), and
//! writes how it went only into a new file, never over one. The app then looks at what's there now rather than taking
//! the step's word for it.
//!
//! The app's start-up hands what it was started with to [`run_step`] before anything else, and stops when it says so.

use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::after_move::relink;
use crate::clean::{Bin, NEEDS_ADMIN, SystemBin};
use crate::folder_move::{CantMove, leads_to, swap, unswap};
use crate::key;
use crate::size::is_link;

/// The flag the app is started with to rename or link a folder Windows guards: `--move-folder swap|unswap|relink
/// <list> <result> <fingerprint>`.
pub const MOVE_FOLDER: &str = "--move-folder";

/// The flag the app is started with to send files Windows guards to the Recycle Bin: `--recycle-bin <list> <result>
/// <fingerprint>`.
pub const RECYCLE_BIN: &str = "--recycle-bin";

/// What the step says when even administrator rights weren't enough.
const EVEN_AS_ADMIN: &str = "Windows wouldn't allow it even as administrator.";

/// Windows' administrator question. The app uses [`AsAdmin`]; tests stand in one that doesn't ask anybody.
pub trait Admin: Sync {
    /// Whether it can ask at all (only Windows has the question).
    fn can_ask(&self) -> bool;
    /// Starts the app again as administrator with `args` (what it's started with, its own path left out) and waits
    /// for it. Says what stopped it: you said no, or it couldn't start.
    fn ask(&self, args: &[String]) -> Result<(), String>;
}

/// The real question: this same program, started again through Windows' administrator question.
pub struct AsAdmin;

impl Admin for AsAdmin {
    fn can_ask(&self) -> bool {
        cfg!(windows)
    }

    fn ask(&self, args: &[String]) -> Result<(), String> {
        if !cfg!(windows) {
            return Err("Only Windows has this administrator step.".to_owned());
        }
        let exe = std::env::current_exe().map_err(|_| "The app couldn't find itself to ask Windows.".to_owned())?;
        let out = ask_command(&exe, args)
            .output()
            .map_err(|e| format!("Windows couldn't start the administrator step ({e})."))?;
        asked(&String::from_utf8_lossy(&out.stdout))
    }
}

/// No question: for tests, and wherever nothing should run as administrator.
pub struct NoAdmin;

impl Admin for NoAdmin {
    fn can_ask(&self) -> bool {
        false
    }

    fn ask(&self, _args: &[String]) -> Result<(), String> {
        Err("Windows' administrator question isn't asked here.".to_owned())
    }
}

/// The command that starts the app again as administrator and waits for it: PowerShell's `Start-Process -Verb RunAs`,
/// which asks Windows' question the way Explorer does. The program and what it's started with reach it as settings,
/// never as part of the script, so nothing in them can run as a command. It says how it went on one line: `ended`,
/// `said no`, or `not started: why`.
fn ask_command(exe: &Path, args: &[String]) -> std::process::Command {
    const SCRIPT: &str = "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; \
        try { Start-Process -FilePath $env:MO_ADMIN_FILE -ArgumentList $env:MO_ADMIN_ARGS -Verb RunAs \
        -WindowStyle Hidden -Wait -ErrorAction Stop; 'ended' } \
        catch { $e = $_.Exception; $no = $false; \
        while ($e) { if ($e.NativeErrorCode -eq 1223) { $no = $true }; $e = $e.InnerException }; \
        if ($no) { 'said no' } else { 'not started: ' + $_.Exception.Message } }";
    // Windows' own PowerShell, never one that happens to sit beside the app or earlier on the PATH.
    let windows = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from("C:\\Windows"), PathBuf::from);
    let mut cmd = std::process::Command::new(windows.join(r"System32\WindowsPowerShell\v1.0\powershell.exe"));
    cmd.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", SCRIPT])
        .env("MO_ADMIN_FILE", exe)
        .env("MO_ADMIN_ARGS", command_line(args));
    // No black window flashing up for PowerShell itself.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// What the command says about how it went.
fn asked(said: &str) -> Result<(), String> {
    let line = said.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    if line == "ended" {
        return Ok(());
    }
    if line == "said no" {
        return Err("You said no to Windows' administrator question.".to_owned());
    }
    match line.strip_prefix("not started: ") {
        Some(why) => Err(format!("Windows couldn't start the administrator step ({why}).")),
        None => Err("The administrator step didn't say how it went.".to_owned()),
    }
}

/// What the app is started with, as one line the way Windows programs (this one included) split it up again: each part
/// with a space or tab in it goes in quotes, with the backslashes before a quote doubled.
pub(crate) fn command_line(args: &[String]) -> String {
    args.iter().map(|a| quoted(a)).collect::<Vec<_>>().join(" ")
}

fn quoted(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        let keep = if c == '"' { slashes * 2 + 1 } else { slashes };
        out.extend(std::iter::repeat_n('\\', keep));
        slashes = 0;
        out.push(c);
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// A step and what its list's lines hold (separated by tabs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// `from`, `to`, `aside`: the folder becomes `aside` and a link to `to` takes its name.
    Swap,
    /// `from`, `restore`, `to`: the link to `to` goes and `restore` takes its name.
    Unswap,
    /// `at`, `target`: the link at `at` leads to `target` instead.
    Relink,
    /// `path`: it goes to the Recycle Bin.
    Bin,
}

impl Step {
    fn args(self, list: &Path, result: &Path, fingerprint: String) -> Vec<String> {
        let mut args = match self {
            Step::Swap => vec![MOVE_FOLDER.to_owned(), "swap".to_owned()],
            Step::Unswap => vec![MOVE_FOLDER.to_owned(), "unswap".to_owned()],
            Step::Relink => vec![MOVE_FOLDER.to_owned(), "relink".to_owned()],
            Step::Bin => vec![RECYCLE_BIN.to_owned()],
        };
        args.extend([text(list), text(result), fingerprint]);
        args
    }
}

/// How one line of a step went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Done,
    /// Nothing changed, and why.
    Kept(String),
    /// Something changed part way and things aren't as they were: what to do about it.
    Halfway(String),
}

/// Runs a step as administrator for `lines` (each a list of fields) and says how each went, in the same order. Writes
/// the list, asks, reads how it went, and leaves neither file behind. `Err` when the step didn't run at all (you said
/// no, say). Callers look at what's there afterwards: this is only what the step said.
pub(crate) fn ask_for(admin: &dyn Admin, step: Step, lines: &[Vec<String>]) -> Result<Vec<Outcome>, String> {
    if lines.iter().flatten().any(|f| f.contains(['\t', '\n', '\r'])) {
        return Err("A name has a tab or a line break in it, which the administrator step can't take.".to_owned());
    }
    let (list, result) = temp_names();
    let rows: Vec<String> = lines.iter().map(|fields| fields.join("\t")).collect();
    let fingerprint = write_list(&list, &rows)
        .map_err(|e| format!("The list for the administrator step couldn't be written ({e})."))?;
    let asked = admin.ask(&step.args(&list, &result, fingerprint));
    let said = fs::read_to_string(&result);
    let _ = fs::remove_file(&list);
    let _ = fs::remove_file(&result);
    asked?;
    let said = said.map_err(|_| "The administrator step didn't finish.".to_owned())?;
    let mut by_path: Vec<(String, Outcome)> = said.lines().filter_map(read_outcome).collect();
    Ok(lines
        .iter()
        .map(|fields| {
            let first = key(fields.first().map_or("", String::as_str));
            match by_path.iter().position(|(path, _)| key(path) == first) {
                Some(at) => by_path.swap_remove(at).1,
                None => Outcome::Kept("The administrator step didn't get to it.".to_owned()),
            }
        })
        .collect())
}

/// A list and a result file in your Temp folder, under names no other run uses.
fn temp_names() -> (PathBuf, PathBuf) {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let name = format!("mo-admin-{}-{nanos}-{}", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed));
    let temp = std::env::temp_dir();
    (temp.join(format!("{name}.txt")), temp.join(format!("{name}-result.txt")))
}

/// Writes the list of what the step is to do, as a new file (never over one), and returns its fingerprint to start
/// the step with.
pub fn write_list(path: &Path, lines: &[String]) -> io::Result<String> {
    let bytes: Vec<u8> = lines.iter().flat_map(|l| l.bytes().chain(std::iter::once(b'\n'))).collect();
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(fingerprint(&bytes))
}

/// The list's lines, read once, or `None` when it isn't the list with that fingerprint (something changed it).
pub fn read_list(path: &Path, fingerprint_was: &str) -> io::Result<Option<Vec<String>>> {
    let bytes = fs::read(path)?;
    if !fingerprint(&bytes).eq_ignore_ascii_case(fingerprint_was) {
        return Ok(None);
    }
    let Ok(text) = String::from_utf8(bytes) else { return Ok(None) };
    Ok(Some(text.split('\n').map(|l| l.trim_end_matches('\r')).filter(|l| !l.is_empty()).map(str::to_owned).collect()))
}

fn fingerprint(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02X}")).collect()
}

/// One line of how the step went: the line's first field, then `done`, `kept` or `halfway`, then what it said.
fn outcome_line(first: &str, outcome: &Outcome) -> String {
    let plain = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    match outcome {
        Outcome::Done => format!("{first}\tdone\t"),
        Outcome::Kept(why) => format!("{first}\tkept\t{}", plain(why)),
        Outcome::Halfway(what) => format!("{first}\thalfway\t{}", plain(what)),
    }
}

fn read_outcome(line: &str) -> Option<(String, Outcome)> {
    let mut parts = line.splitn(3, '\t');
    let (first, how, said) = (parts.next()?, parts.next()?, parts.next().unwrap_or_default().to_owned());
    let outcome = match how {
        "done" => Outcome::Done,
        "kept" => Outcome::Kept(said),
        "halfway" => Outcome::Halfway(said),
        _ => return None,
    };
    Some((first.to_owned(), outcome))
}

/// Runs the administrator step when the app was started for one: `args` is what the app was started with, its own
/// path left out (`std::env::args().skip(1)`). Returns `true` when it was started with [`MOVE_FOLDER`] or
/// [`RECYCLE_BIN`] (the app then stops at once, whether or not the step did anything), `false` when it wasn't (the app
/// starts as usual). It never shows a window. Started with the flag but not as the app starts it, or with a list that
/// was changed, it does nothing at all.
pub fn run_step(args: &[String]) -> bool {
    run_step_with(args, &SystemBin)
}

/// [`run_step`], with the bin files go to (tests stand in a folder for the Recycle Bin).
pub fn run_step_with(args: &[String], bin: &dyn Bin) -> bool {
    let (step, rest) = match args {
        [flag, rest @ ..] if flag == RECYCLE_BIN => (Step::Bin, rest),
        [flag, name, rest @ ..] if flag == MOVE_FOLDER => match name.as_str() {
            "swap" => (Step::Swap, rest),
            "unswap" => (Step::Unswap, rest),
            "relink" => (Step::Relink, rest),
            _ => return true,
        },
        [flag, ..] if flag == MOVE_FOLDER => return true,
        _ => return false,
    };
    let [list, result, fingerprint_was] = rest else { return true };
    let Ok(Some(lines)) = read_list(Path::new(list), fingerprint_was) else { return true };
    // How it went goes only into a new file. The file is made before anything is done, so when it can't be (something
    // is there already), nothing is.
    let Ok(file) = OpenOptions::new().write(true).create_new(true).open(result) else { return true };
    let mut out = BufWriter::new(file);
    for line in &lines {
        let fields: Vec<&str> = line.split('\t').collect();
        let outcome = do_line(step, &fields, bin);
        let _ = writeln!(out, "{}", outcome_line(fields.first().copied().unwrap_or_default(), &outcome));
        let _ = out.flush();
    }
    true
}

/// Does one line of the step, as administrator, once its paths are checked again.
fn do_line(step: Step, fields: &[&str], bin: &dyn Bin) -> Outcome {
    let checked = match (step, fields) {
        (Step::Swap, [from, to, aside]) => swap_here(from, to, aside),
        (Step::Unswap, [from, restore, to]) => unswap_here(from, restore, to),
        (Step::Relink, [at, target]) => relink_here(at, target),
        (Step::Bin, [path]) => bin_here(path, bin),
        _ => Err("The list wasn't as the app writes it.".to_owned()),
    };
    checked.unwrap_or_else(Outcome::Kept)
}

fn swap_here(from: &str, to: &str, aside: &str) -> Result<Outcome, String> {
    plain(from)?;
    beside(aside, from)?;
    if is_link(Path::new(from)) || !is_dir(from) {
        return Err(format!("{from} isn't a real folder now, so it was left alone."));
    }
    if !is_dir(to) {
        return Err(format!("{to} isn't there."));
    }
    Ok(swap(from, Path::new(to), aside).map_or_else(|why| said(&why), |()| Outcome::Done))
}

fn unswap_here(from: &str, restore: &str, to: &str) -> Result<Outcome, String> {
    plain(from)?;
    beside(restore, from)?;
    if !is_link(Path::new(from)) || !leads_to(from, Path::new(to)) {
        return Err(format!("{from} isn't the link to {to} now, so it was left alone."));
    }
    if is_link(Path::new(restore)) || !is_dir(restore) {
        return Err(format!("{restore} isn't there."));
    }
    Ok(unswap(from, restore, Path::new(to)).map_or_else(|why| said(&why), |()| Outcome::Done))
}

fn relink_here(at: &str, target: &str) -> Result<Outcome, String> {
    plain(at)?;
    Ok(match relink(at, Path::new(target), true) {
        Ok(()) => Outcome::Done,
        Err(not) if not.gone => Outcome::Halfway(not.why),
        Err(not) if not.guarded => Outcome::Kept(EVEN_AS_ADMIN.to_owned()),
        Err(not) => Outcome::Kept(not.why),
    })
}

fn bin_here(path: &str, bin: &dyn Bin) -> Result<Outcome, String> {
    plain(path)?;
    match fs::symlink_metadata(path) {
        // Gone already: nothing to do.
        Err(_) => return Ok(Outcome::Done),
        Ok(m) if m.file_type().is_symlink() => return Err(format!("{path} is a link now, so it was left alone.")),
        Ok(_) => {}
    }
    Ok(match bin.send(Path::new(path)) {
        Ok(()) => Outcome::Done,
        Err(why) if why == NEEDS_ADMIN => Outcome::Kept(EVEN_AS_ADMIN.to_owned()),
        Err(why) => Outcome::Kept(why),
    })
}

/// What a move's own answer says, as the step says it.
fn said(why: &CantMove) -> Outcome {
    match why {
        CantMove::LeftHalfway(what) => Outcome::Halfway(what.clone()),
        CantMove::NeedsAdmin => Outcome::Kept(EVEN_AS_ADMIN.to_owned()),
        other => Outcome::Kept(other.text().trim_end_matches("Nothing was moved.").trim_end().to_owned()),
    }
}

/// The folder a path is in is where it says, with no link on the way: as administrator, nothing is done through a link
/// that could lead somewhere else.
fn plain(path: &str) -> Result<(), String> {
    let parent = Path::new(path).parent().filter(|p| !p.as_os_str().is_empty());
    let real = parent.and_then(|p| fs::canonicalize(p).ok());
    match (parent, real) {
        (Some(parent), Some(real)) if key(&text(&real)) == key(&text(parent)) => Ok(()),
        (Some(_), Some(_)) => Err(format!("{path} is reached through a link, so it was left alone.")),
        _ => Err(format!("The folder {path} is in isn't there.")),
    }
}

/// `path` sits in the same folder as `beside`.
fn beside(path: &str, beside: &str) -> Result<(), String> {
    let folder = |p: &str| Path::new(p).parent().map(|f| key(&text(f)));
    match (folder(path), folder(beside)) {
        (Some(a), Some(b)) if a == b => Ok(()),
        _ => Err(format!("{path} isn't beside {beside}, so it was left alone.")),
    }
}

fn is_dir(path: &str) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_dir())
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_app_is_started_with_is_quoted_as_windows_reads_it() {
        let line = |args: &[&str]| command_line(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>());
        assert_eq!(line(&["--move-folder", "swap"]), "--move-folder swap");
        assert_eq!(
            line(&[r"C:\Users\Jo Smith\AppData\Local\Temp\mo-admin-1.txt", "AB12"]),
            r#""C:\Users\Jo Smith\AppData\Local\Temp\mo-admin-1.txt" AB12"#
        );
        // Backslashes are only doubled before a quote, the closing one included.
        assert_eq!(line(&[r"C:\a b\"]), r#""C:\a b\\""#);
        assert_eq!(line(&[r#"say "hi""#]), r#""say \"hi\"""#);
        assert_eq!(line(&[""]), r#""""#);
    }

    #[test]
    fn the_command_says_how_it_went() {
        assert_eq!(asked("ended\r\n"), Ok(()));
        assert_eq!(asked("said no"), Err("You said no to Windows' administrator question.".to_owned()));
        assert!(asked("not started: The file isn't there.").is_err_and(|e| e.contains("isn't there")));
        assert!(asked("").is_err());
    }

    #[test]
    fn how_a_line_went_reads_back() {
        for outcome in [Outcome::Done, Outcome::Kept("in\tuse".to_owned()), Outcome::Halfway("rename it".to_owned())] {
            let line = outcome_line(r"C:\X", &outcome);
            let (first, back) = read_outcome(&line).unwrap_or_else(|| (String::new(), Outcome::Done));
            assert_eq!(first, r"C:\X");
            let plain = match outcome {
                Outcome::Kept(why) => Outcome::Kept(why.replace('\t', " ")),
                other => other,
            };
            assert_eq!(back, plain);
        }
        assert_eq!(read_outcome("x\tmaybe\t"), None);
    }
}
