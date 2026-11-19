//! Updates from inside the app (the installer plan, step 4; docs/installer.md). Once a day, a while after start and on
//! a low-priority thread, an installed copy asks GitHub's Releases page whether a newer version is out. That one
//! question is all the app sends online, and Settings › General turns it off (rule 1: the app works fully offline).
//! A newer setup is fetched in the background into the data folder's `updates` folder, checked against the SHA-256
//! the release lists, and only then offered: "Restart to update". On a click the setup is checked and started, and
//! the app closes as it always does; the setup waits for it to end, then opens the new version (`/UPDATED=1`,
//! `/WAITPID`, installer/setup.iss), or the old one again when it couldn't go in ([`failed_update`] tells it).
//!
//! Only a copy the installer put in place updates itself: one built by the launcher, or run from a build folder, never
//! does ([`installed`]). Talking to GitHub goes through Windows' own `curl.exe` (System32), so no web library is built
//! in and Windows' own certificates are used. A proxy set only in Windows' settings is not (curl.exe reads only the
//! `HTTPS_PROXY` and `ALL_PROXY` environment variables), so behind one the look fails and says so in the log (UP-8).
//! Tests use a stand-in server on this computer, never GitHub.
//!
//! Rule 28: a setup is removed once its version is the one running (or older), and a half-fetched one
//! (`.mo-part`) at the next look ([`clear_old`]). Whenever GitHub answers, a setup it no longer lists goes too, so a
//! release taken back is never offered and at most one setup waits ([`clear_unlisted`], UP-3, UP-5); and when the look
//! is turned off, a fetch going is stopped ([`stop_fetches`]) and every fetched setup goes (UP-4).

use std::cmp::Ordering;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering as Atomic};
use std::time::Duration;

/// The setting that turns the daily look off ("0"). On unless turned off.
pub const CHECK_KEY: &str = "update_check";

/// When the last look was made, in seconds since 1970.
pub const CHECKED_KEY: &str = "update_checked";

/// How long between looks.
pub const EVERY_SECS: u64 = 20 * 3600;

/// The folder in the data folder that setups are fetched into.
pub const FOLDER: &str = "updates";

/// GitHub's API, where the releases are listed.
pub const GITHUB_API: &str = "https://api.github.com";

/// The repository the releases come from (product.toml's website).
pub const REPO: &str = "Silkie86/Music-Organiser-Rust";

/// How a setup's name starts: `DAWmate-Setup-` (installer/build.ps1 makes the product's name safe for a file name
/// the same way). The app keeps, offers and removes only files named so.
pub fn setup_prefix() -> String {
    let mut safe = String::new();
    for c in crate::product::NAME.chars() {
        if c.is_ascii_alphanumeric() {
            safe.push(c);
        } else if !safe.ends_with('-') {
            safe.push('-');
        }
    }
    format!("{}-Setup-", safe.trim_matches('-'))
}

/// The version in one of the app's own setup names (`DAWmate-Setup-1.0.1.exe`), else none.
fn setup_version(name: &str) -> Option<Version> {
    Version::parse(name.strip_prefix(&setup_prefix())?.strip_suffix(".exe")?)
}

/// A file in the updates folder that is of no more use (rule 28): a half-fetched setup (`.mo-part`), or a setup, or
/// its `.sha256`, for `current` or older. Only names the app gives.
pub fn is_spent(name: &str, current: &str) -> bool {
    let Some(current) = Version::parse(current) else { return false };
    if let Some(part) = name.strip_suffix(".mo-part") {
        return setup_version(part).is_some();
    }
    let setup = name.strip_suffix(".sha256").or_else(|| name.strip_suffix(".log")).unwrap_or(name);
    setup_version(setup).is_some_and(|v| v <= current)
}

/// A fetch cut short (`.mo-part` after a setup's name): of no use to anyone, whichever copy of the app runs.
pub fn is_part(name: &str) -> bool {
    name.strip_suffix(".mo-part").is_some_and(|part| setup_version(part).is_some())
}

/// The setup a file in the updates folder belongs to, by its version: the setup, its `.sha256`, its `.log` or a fetch
/// of it cut short.
fn belongs_to(name: &str) -> Option<Version> {
    let setup = [".sha256", ".log", ".mo-part"].iter().find_map(|end| name.strip_suffix(end)).unwrap_or(name);
    setup_version(setup)
}

/// Whenever GitHub has answered: removes from `folder` every fetched setup newer than `current` but `listed` (the
/// version GitHub offers now; none when it offers nothing newer), with its `.sha256`. A release taken off GitHub is
/// never offered from what was fetched before, and no more than one setup waits (UP-3, UP-5). With `listed` none it
/// also serves the look being turned off (UP-4). A setup's log stays: [`failed_update`] tells it once and takes it.
pub fn clear_unlisted(folder: &Path, current: &str, listed: Option<&str>) {
    let (Some(current), Ok(entries)) = (Version::parse(current), fs::read_dir(folder)) else { return };
    let listed = listed.and_then(Version::parse);
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|k| k.is_file()) {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        if name.ends_with(".log") {
            continue;
        }
        if belongs_to(&name).is_some_and(|v| v > current && Some(&v) != listed.as_ref()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Bumped to stop every fetch going ([`stop_fetches`]).
static STOPS: AtomicU64 = AtomicU64::new(0);

/// What a stopped fetch says.
pub const STOPPED: &str = "stopped";

/// Stops every fetch going (curl is ended, and a setup cut short removed by its fetch): the look was turned off, or
/// the app is closing, so nothing goes on fetching once it's said no (UP-4).
pub fn stop_fetches() {
    STOPS.fetch_add(1, Atomic::Relaxed);
}

/// The setup's own log for `setup` (its name with `.log` after it, beside it): the app names it, so the next start
/// can say how the update went ([`failed_update`]), and it goes with its setup (rule 28, [`is_spent`]).
pub fn setup_log(setup: &Path) -> PathBuf {
    let mut log = setup.as_os_str().to_owned();
    log.push(".log");
    PathBuf::from(log)
}

/// An update that didn't go in: a setup's log in `folder` for a version newer than `current` (the setup ran, but
/// this is still the old version). Gives the version and the log's last few lines, and takes the log away, so it's
/// told once; the setup stays, to be offered again.
pub fn failed_update(folder: &Path, current: &str) -> Option<(String, String)> {
    let current = Version::parse(current)?;
    for entry in fs::read_dir(folder).ok()?.flatten() {
        if !entry.file_type().is_ok_and(|k| k.is_file()) {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        let Some(setup) = name.strip_suffix(".log") else { continue };
        let Some(version) = setup_version(setup) else { continue };
        if version <= current {
            continue;
        }
        let text = fs::read(entry.path()).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        // A setup still at work holds its log open, and Windows won't remove it: it isn't over yet, so nothing is
        // told until it is.
        if fs::remove_file(entry.path()).is_err() {
            continue;
        }
        let tail = why_in(&text);
        let shown = setup[setup_prefix().len()..setup.len() - ".exe".len()].to_owned();
        return Some((shown, tail));
    }
    None
}

/// The lines of a setup's log that say why it stopped: its error lines, else the last few (the end of a log is
/// mostly Inno taking itself down).
fn why_in(log: &str) -> String {
    let lines: Vec<&str> = log.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let errors: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| {
            let l = l.to_ascii_lowercase();
            ["error", "failed", "abort", "canceled", "code "].iter().any(|w| l.contains(w))
        })
        .collect();
    let keep = if errors.is_empty() { &lines } else { &errors };
    keep[keep.len().saturating_sub(6)..].join("\n")
}

/// Whether `setup` is still the whole setup that was fetched: its SHA-256 still the one kept beside it.
pub fn still_whole(setup: &Path) -> bool {
    let mut sha_file = setup.as_os_str().to_owned();
    sha_file.push(".sha256");
    let Some(want) = fs::read_to_string(sha_file).ok().as_deref().and_then(sha_in) else { return false };
    sha_of(setup).is_ok_and(|got| got == want)
}

/// Whether the daily look is on.
pub fn wanted(setting: Option<&str>) -> bool {
    setting != Some("0")
}

/// Whether a look is due: none yet, or the last one [`EVERY_SECS`] ago (or a clock set back). The first look after a
/// start is always made, whenever the last was (UP-9); this is for an app left open.
pub fn due(checked: Option<&str>, now: u64) -> bool {
    match checked.and_then(|c| c.trim().parse::<u64>().ok()) {
        Some(then) => now < then || now - then >= EVERY_SECS,
        None => true,
    }
}

/// Whether this copy of the app was put in place by the installer: its uninstaller is beside it. Only that copy may
/// update itself.
pub fn installed(exe: &Path) -> bool {
    exe.parent().is_some_and(|d| d.join("unins000.exe").is_file())
}

/// A version like `1.2.3` or `0.9.0-beta.1`, compared the way semantic versions are: a test version (with a dash)
/// comes before the same numbers without one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    numbers: [u64; 3],
    pre: Vec<String>,
}

impl Version {
    /// `1.2.3`, `v1.2.3` or `1.2.3-beta.1`; anything else is none.
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim().trim_start_matches('v');
        let (main, pre) = match text.split_once('-') {
            Some((m, p)) => (m, p.split('.').map(str::to_owned).collect::<Vec<_>>()),
            None => (text, Vec::new()),
        };
        let mut parts = main.split('.');
        let mut numbers = [0u64; 3];
        for n in &mut numbers {
            *n = parts.next()?.parse().ok()?;
        }
        // Semantic versions' own letters only, so a version is always safe in a file name.
        let plain = |p: &String| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if parts.next().is_some() || !pre.iter().all(plain) {
            return None;
        }
        Some(Version { numbers, pre })
    }

    /// Whether it's a test version (it has a dash).
    pub fn is_test(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.numbers.cmp(&other.numbers).then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(&other.pre) {
                    let by = match (a.parse::<u64>(), b.parse::<u64>()) {
                        (Ok(a), Ok(b)) => a.cmp(&b),
                        (Ok(_), Err(_)) => Ordering::Less,
                        (Err(_), Ok(_)) => Ordering::Greater,
                        (Err(_), Err(_)) => a.cmp(b),
                    };
                    if by != Ordering::Equal {
                        return by;
                    }
                }
                self.pre.len().cmp(&other.pre.len())
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A newer version on the Releases page, with its setup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// As the release names it: `1.0.1`.
    pub version: String,
    /// The setup's file name and where to fetch it.
    pub setup_name: String,
    pub setup_url: String,
    /// The `.sha256` beside it.
    pub sha_url: String,
    /// The release's own page, for "What's new".
    pub page_url: String,
}

#[derive(serde::Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(serde::Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// The newest release newer than `current` in GitHub's list of releases (`/repos/…/releases`), when it has a setup
/// and its `.sha256`. Test versions are offered only to a test version, so a beta tester stays on the betas until the
/// real release passes them.
pub fn newest(json: &str, current: &str) -> Result<Option<Offer>, String> {
    let current = Version::parse(current).ok_or_else(|| format!("this version ({current}) can't be read"))?;
    let releases: Vec<Release> =
        serde_json::from_str(json).map_err(|e| format!("GitHub's answer can't be read: {e}"))?;
    let mut best: Option<(Version, Offer)> = None;
    for r in releases {
        if r.draft || (r.prerelease && !current.is_test()) {
            continue;
        }
        let Some(version) = Version::parse(&r.tag_name) else { continue };
        if version <= current || (version.is_test() && !current.is_test()) {
            continue;
        }
        if best.as_ref().is_some_and(|(b, _)| *b >= version) {
            continue;
        }
        let shown = r.tag_name.trim().trim_start_matches('v').to_owned();
        let wanted = format!("{}{shown}.exe", setup_prefix());
        let Some(setup) = r.assets.iter().find(|a| a.name == wanted) else { continue };
        let sha = format!("{}.sha256", setup.name);
        let Some(sha) = r.assets.iter().find(|a| a.name == sha) else { continue };
        best = Some((
            version,
            Offer {
                version: shown,
                setup_name: setup.name.clone(),
                setup_url: setup.browser_download_url.clone(),
                sha_url: sha.browser_download_url.clone(),
                page_url: r.html_url,
            },
        ));
    }
    Ok(best.map(|(_, o)| o))
}

/// The SHA-256 a `.sha256` file names (`<hex>  <file>`, as the release workflow writes it).
pub fn sha_in(text: &str) -> Option<String> {
    let hex = text.trim_start_matches('\u{feff}').split_whitespace().next()?.to_ascii_lowercase();
    (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())).then_some(hex)
}

/// The program that fetches from the web: Windows' own curl (in System32, never one found on the PATH), elsewhere the
/// system's.
fn curl() -> PathBuf {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        root.join("System32").join("curl.exe")
    } else {
        PathBuf::from("curl")
    }
}

/// Only https, from GitHub and wherever it sends on to; plain http only for a test's stand-in on this computer.
fn protocols(url: &str) -> &'static str {
    if url.starts_with("http://127.0.0.1:") { "=http" } else { "=https" }
}

/// Fetches `url`: into `to` when given (and nothing back), else its words back. A page that isn't there or doesn't
/// answer in time is an error.
pub fn fetch(url: &str, to: Option<&Path>, seconds: u32) -> Result<String, String> {
    let mut command = Command::new(curl());
    command
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            protocols(url),
            "--proto-redir",
            "=https",
        ])
        .args(["--max-time", &seconds.to_string()])
        .args(["--user-agent", &format!("{}-update-check", crate::product::NAME)])
        .args(["--header", "Accept: application/vnd.github+json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(to) = to {
        command.arg("--output").arg(to);
    }
    command.arg(url);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashing up.
        command.creation_flags(0x0800_0000);
    }
    let stops = STOPS.load(Atomic::Relaxed);
    let mut child = command.spawn().map_err(|e| format!("couldn't start curl: {e}"))?;
    // Read as it comes, on threads of their own, so a long answer never fills the pipe while this waits.
    let reader = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let out = reader(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = reader(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let status = loop {
        if STOPS.load(Atomic::Relaxed) != stops {
            let _ = child.kill();
            let _ = child.wait();
            return Err(STOPPED.to_owned());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("couldn't wait for curl: {e}")),
        }
    };
    let out = out.join().unwrap_or_default();
    let err = err.join().unwrap_or_default();
    if !status.success() {
        return Err(String::from_utf8_lossy(&err).trim().to_owned());
    }
    String::from_utf8(out).map_err(|_| "the answer wasn't text".to_owned())
}

/// Asks `api` (GitHub's, or a test's stand-in) for the releases and picks what's newer than `current`.
pub fn look(api: &str, current: &str) -> Result<Option<Offer>, String> {
    let json = fetch(&format!("{api}/repos/{REPO}/releases?per_page=20"), None, 30)?;
    newest(&json, current)
}

/// Fetches the offer's setup into `folder` (written as `.mo-part`, checked against its SHA-256, then named, with the
/// SHA-256 kept beside it for [`ready_in`]), and gives its path. One there already and whole is kept, not fetched
/// again.
pub fn fetch_setup(offer: &Offer, folder: &Path) -> Result<PathBuf, String> {
    // Only the app's own name for this version: nothing in the name can lead out of the folder.
    if setup_version(&offer.setup_name).is_none_or(|v| Some(v) != Version::parse(&offer.version)) {
        return Err(format!("the setup's name {} isn't the one the app gives it", offer.setup_name));
    }
    let file = folder.join(&offer.setup_name);
    let sha_file = folder.join(format!("{}.sha256", offer.setup_name));
    let want = sha_in(&fetch(&offer.sha_url, None, 30)?).ok_or("the release's SHA-256 can't be read")?;
    let keep_sha = || {
        fs::write(&sha_file, format!("{want}  {}", offer.setup_name))
            .map_err(|e| format!("couldn't keep the setup's SHA-256: {e}"))
    };
    if file.is_file() && sha_of(&file).is_ok_and(|h| h == want) {
        keep_sha()?;
        return Ok(file);
    }
    fs::create_dir_all(folder).map_err(|e| format!("couldn't make {}: {e}", folder.display()))?;
    let part = folder.join(format!("{}.mo-part", offer.setup_name));
    let fetched = fetch(&offer.setup_url, Some(&part), 900).and_then(|_| {
        let got = sha_of(&part).map_err(|e| format!("couldn't read the fetched setup: {e}"))?;
        if got == want {
            Ok(())
        } else {
            Err("the fetched setup isn't the one the release lists (its SHA-256 differs)".to_owned())
        }
    });
    if let Err(e) = fetched {
        let _ = fs::remove_file(&part);
        return Err(e);
    }
    fs::rename(&part, &file).map_err(|e| format!("couldn't name the fetched setup: {e}"))?;
    keep_sha()?;
    Ok(file)
}

/// A file's SHA-256, in lower-case hex.
pub fn sha_of(path: &Path) -> io::Result<String> {
    let never = std::sync::atomic::AtomicBool::new(false);
    let hash = mo_space::size::file_hash(path, &never)
        .ok_or_else(|| io::Error::other(format!("{} couldn't be read whole", path.display())))?;
    Ok(hash.iter().map(|b| format!("{b:02x}")).collect())
}

/// Removes from `folder` what's of no more use ([`is_spent`]); the start-up sweep (`tidy`) does the same.
pub fn clear_old(folder: &Path, current: &str) {
    let Ok(entries) = fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        // The listing's own look: a link is never a plain file here.
        if entry.file_type().is_ok_and(|k| k.is_file())
            && entry.file_name().to_str().is_some_and(|n| is_spent(n, current))
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// A setup fetched and checked earlier and not put in yet (the app was closed instead): the newest in `folder` newer
/// than `current`, with its version, when it still has the SHA-256 it was fetched with (checked again here).
pub fn ready_in(folder: &Path, current: &str) -> Option<(String, PathBuf)> {
    let current = Version::parse(current)?;
    let mut found: Vec<(Version, String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(folder).ok()?.flatten() {
        if !entry.file_type().is_ok_and(|k| k.is_file()) {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        let Some(version) = setup_version(&name) else { continue };
        if version <= current || (version.is_test() && !current.is_test()) {
            continue;
        }
        let shown = name[setup_prefix().len()..name.len() - ".exe".len()].to_owned();
        found.push((version, shown, entry.path()));
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().find_map(|(_, shown, path)| {
        let mut sha_file = path.clone().into_os_string();
        sha_file.push(".sha256");
        let want = sha_in(&fs::read_to_string(sha_file).ok()?)?;
        (sha_of(&path).ok()? == want).then_some((shown, path))
    })
}

/// The setup's switches for an update: no pages, `/UPDATED=1` opens the new version once it's in (or the old one
/// again when it couldn't go in), `/WAITPID` waits for this app (process `pid`) to end before any file is touched
/// (UP-14), and its log goes beside it ([`setup_log`], UP-6). Its own boxes are left to show: a file in use or a copy
/// that won't close then says so, with Retry, instead of quietly giving up (UP-2).
pub fn setup_args(setup: &Path, pid: u32) -> Vec<String> {
    vec![
        "/VERYSILENT".into(),
        "/NORESTART".into(),
        "/UPDATED=1".into(),
        format!("/WAITPID={pid}"),
        format!("/LOG={}", setup_log(setup).display()),
    ]
}

/// Starts the fetched setup, to put the new version in place once this app (process `pid`) has ended
/// (installer/setup.iss: it waits for it, closes what's still running of the app, and opens the app when it's
/// done). Windows only.
pub fn run_setup(setup: &Path, pid: u32) -> io::Result<()> {
    if !cfg!(windows) {
        return Err(io::Error::other("updates are put in by the Windows installer"));
    }
    let mut command = Command::new(setup);
    command.args(setup_args(setup, pid)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // Out of the app's job, as a DAW is started: it has to outlive the app, which it waits for.
    crate::shell::detached(command).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn versions_go_in_order() {
        assert!(v("1.0.0") > v("0.9.9"));
        assert!(v("v1.0.1") > v("1.0.0"));
        assert!(v("1.0.0") > v("1.0.0-beta.2"));
        assert!(v("1.0.0-beta.10") > v("1.0.0-beta.2"));
        assert!(v("1.0.0-beta.1") > v("1.0.0-alpha.3"));
        assert!(v("1.0.0-beta") < v("1.0.0-beta.1"));
        assert_eq!(v("1.2.3"), v("v1.2.3"));
        for bad in ["1.2", "1.2.3.4", "x.1.2", "1.2.3-", "1.2.3-a..b", "1.2.3-a:b", "1.2.3-a/b", ""] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_look_is_due_once_a_day() {
        assert!(due(None, 1000));
        assert!(due(Some("nonsense"), 1000));
        assert!(!due(Some("1000"), 1000 + EVERY_SECS - 1));
        assert!(due(Some("1000"), 1000 + EVERY_SECS));
        assert!(due(Some("5000"), 1000), "a clock set back looks again");
        assert!(wanted(None) && wanted(Some("1")) && !wanted(Some("0")));
    }

    fn release(tag: &str, pre: bool, draft: bool, with_sha: bool) -> String {
        let shown = tag.trim_start_matches('v');
        let sha = if with_sha {
            format!(
                r#",{{"name":"DAWmate-Setup-{shown}.exe.sha256","browser_download_url":"https://x/{shown}.sha256"}}"#
            )
        } else {
            String::new()
        };
        format!(
            r#"{{"tag_name":"{tag}","draft":{draft},"prerelease":{pre},"html_url":"https://x/r/{tag}","assets":[{{"name":"DAWmate-Setup-{shown}.exe","browser_download_url":"https://x/{shown}.exe"}}{sha}]}}"#
        )
    }

    #[test]
    fn the_newest_release_with_a_setup_is_offered() {
        let list = format!(
            "[{},{},{},{},{}]",
            release("v1.2.0", false, true, true),        // a draft: never
            release("v1.1.0-beta.1", true, false, true), // a test version
            release("v1.0.2", false, false, false),      // no SHA-256 beside it
            release("v1.0.1", false, false, true),
            release("v1.0.0", false, false, true),
        );
        let offer = newest(&list, "1.0.0").unwrap().unwrap();
        assert_eq!(offer.version, "1.0.1");
        assert_eq!(offer.setup_name, "DAWmate-Setup-1.0.1.exe");
        assert_eq!(offer.sha_url, "https://x/1.0.1.sha256");
        assert_eq!(offer.page_url, "https://x/r/v1.0.1");
        // A beta tester gets the betas too.
        assert_eq!(newest(&list, "1.0.0-beta.3").unwrap().unwrap().version, "1.1.0-beta.1");
        // Nothing newer: nothing offered.
        assert_eq!(newest(&list, "1.1.0").unwrap(), None);
        assert!(newest("not json", "1.0.0").is_err());
    }

    #[test]
    fn a_sha_file_is_read() {
        let hex = "ab".repeat(32);
        assert_eq!(sha_in(&format!("{hex}  DAWmate-Setup-1.0.0.exe")), Some(hex.clone()));
        assert_eq!(sha_in(&format!("\u{feff}{}\r\n", hex.to_uppercase())), Some(hex));
        assert_eq!(sha_in("abc  x.exe"), None);
    }

    #[test]
    fn only_the_apps_own_setup_names_count() {
        assert_eq!(setup_prefix(), "DAWmate-Setup-");
        assert!(is_spent("DAWmate-Setup-1.0.0.exe", "1.0.0"));
        assert!(is_spent("DAWmate-Setup-1.0.0.exe.sha256", "1.0.0"));
        assert!(is_spent("DAWmate-Setup-2.0.0.exe.mo-part", "1.0.0"));
        assert!(!is_spent("DAWmate-Setup-1.0.1.exe", "1.0.0"));
        assert!(!is_spent("Vital-Setup-1.0.0.exe", "1.0.0"));
        assert!(!is_spent("Vital-Setup-1.0.0.exe.mo-part", "1.0.0"));
        assert!(!is_spent("DAWmate-Setup-x.exe.mo-part", "1.0.0"));
        assert!(is_spent("DAWmate-Setup-1.0.0.exe.log", "1.0.0"));
        assert!(!is_spent("DAWmate-Setup-1.0.1.exe.log", "1.0.0"));
        assert!(!is_spent("Setup Log 2026-11-19 #001.txt", "1.0.0"));
        let offer = Offer {
            version: "1.0.1".into(),
            setup_name: r"C:DAWmate-Setup-1.0.1.exe".into(),
            setup_url: String::new(),
            sha_url: String::new(),
            page_url: String::new(),
        };
        assert!(fetch_setup(&offer, Path::new("unused")).unwrap_err().contains("isn't the one"));
    }

    fn test_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("mo-update-test").join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn old_and_half_fetched_setups_go_and_nothing_else() {
        let dir = test_folder("clear");
        for name in [
            "DAWmate-Setup-1.0.0.exe",
            "DAWmate-Setup-1.0.0.exe.sha256",
            "DAWmate-Setup-0.9.0-beta.1.exe",
            "DAWmate-Setup-1.0.1.exe",
            "DAWmate-Setup-1.0.2.exe.mo-part",
            "notes.txt",
            "Other-Setup-0.1.0.exe",
            "Other-Setup-0.1.0.exe.mo-part",
        ] {
            fs::write(dir.join(name), "x").unwrap();
        }
        clear_old(&dir, "1.0.0");
        let mut left: Vec<String> =
            fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(
            left,
            ["DAWmate-Setup-1.0.1.exe", "Other-Setup-0.1.0.exe", "Other-Setup-0.1.0.exe.mo-part", "notes.txt"]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    fn sha_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn a_setup_fetched_before_is_offered_only_while_whole() {
        let dir = test_folder("ready");
        let put = |version: &str, body: &str, sha: Option<&str>| {
            let name = format!("DAWmate-Setup-{version}.exe");
            fs::write(dir.join(&name), body).unwrap();
            if let Some(sha) = sha {
                fs::write(dir.join(format!("{name}.sha256")), format!("{sha}  {name}")).unwrap();
            }
        };
        put("1.0.1", "one", Some(&sha_hex(b"one")));
        put("1.0.3", "three", None); // no SHA-256 kept: never offered
        put("1.0.2", "changed", Some(&sha_hex(b"two"))); // not what was fetched: never offered
        put("1.1.0-beta.1", "beta", Some(&sha_hex(b"beta")));
        fs::write(dir.join("Other-Setup-9.0.0.exe"), "x").unwrap();
        let (version, path) = ready_in(&dir, "1.0.0").unwrap();
        assert_eq!((version.as_str(), path), ("1.0.1", dir.join("DAWmate-Setup-1.0.1.exe")));
        assert_eq!(ready_in(&dir, "1.0.1"), None);
        assert_eq!(ready_in(&dir, "1.0.1-beta.1").unwrap().0, "1.1.0-beta.1", "a test version only for a tester");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_setup_github_lists_waits() {
        let dir = test_folder("unlisted");
        for name in [
            "DAWmate-Setup-1.0.0.exe",
            "DAWmate-Setup-1.0.2.exe",
            "DAWmate-Setup-1.0.2.exe.sha256",
            "DAWmate-Setup-1.0.3.exe",
            "DAWmate-Setup-1.0.3.exe.sha256",
            "DAWmate-Setup-1.0.3.exe.log",
            "DAWmate-Setup-1.0.4.exe.mo-part",
            "Vital-Setup-2.0.0.exe",
        ] {
            fs::write(dir.join(name), "x").unwrap();
        }
        let left = || {
            let mut names: Vec<String> =
                fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            names
        };
        clear_unlisted(&dir, "1.0.1", Some("1.0.3"));
        assert_eq!(
            left(),
            [
                "DAWmate-Setup-1.0.0.exe",
                "DAWmate-Setup-1.0.3.exe",
                "DAWmate-Setup-1.0.3.exe.log",
                "DAWmate-Setup-1.0.3.exe.sha256",
                "Vital-Setup-2.0.0.exe"
            ],
            "the older one is clear_old's; another program's file is never touched"
        );
        clear_unlisted(&dir, "1.0.1", None);
        assert_eq!(left(), ["DAWmate-Setup-1.0.0.exe", "DAWmate-Setup-1.0.3.exe.log", "Vital-Setup-2.0.0.exe"]);
        assert!(is_part("DAWmate-Setup-1.0.4.exe.mo-part") && !is_part("DAWmate-Setup-1.0.4.exe"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Stopping stops every fetch going: the tests that fetch take turns.
    static FETCHES: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_stopped_fetch_says_so() {
        let _alone = FETCHES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if Command::new(curl()).arg("--version").output().is_err() {
            eprintln!("no curl here: skipped");
            return;
        }
        // A fetch that would take a while (nothing answers on this port, so curl waits for its connection): stopped.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/slow", listener.local_addr().unwrap().port());
        let stopper = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(300));
            stop_fetches();
        });
        let started = std::time::Instant::now();
        assert_eq!(fetch(&url, None, 30), Err(STOPPED.to_owned()));
        assert!(started.elapsed() < Duration::from_secs(10));
        stopper.join().unwrap();
        drop(listener);
    }

    #[test]
    fn an_update_that_didnt_go_in_is_told_once() {
        let dir = test_folder("failed");
        fs::write(dir.join("DAWmate-Setup-1.0.1.exe"), "x").unwrap();
        let lines: String = (1..=10).map(|n| format!("line {n}\r\n")).collect();
        fs::write(dir.join("DAWmate-Setup-1.0.1.exe.log"), lines).unwrap();
        fs::write(dir.join("DAWmate-Setup-1.0.0.exe.log"), "went in").unwrap();
        assert_eq!(failed_update(&dir, "1.0.1"), None, "the running version's log means it went in");
        let (version, tail) = failed_update(&dir, "1.0.0").unwrap();
        assert_eq!(version, "1.0.1");
        assert!(tail.starts_with("line 5") && tail.ends_with("line 10"), "{tail}");
        let log =
            "Starting the installation process.\nDeleteFile failed; code 5.\nRolling back changes.\nLog closed.\n";
        assert_eq!(why_in(log), "DeleteFile failed; code 5.", "the line that says why, not Inno's own ending");
        assert_eq!(failed_update(&dir, "1.0.0"), None, "told once");
        assert!(dir.join("DAWmate-Setup-1.0.1.exe").is_file(), "the setup stays, to be offered again");
        let setup = dir.join("DAWmate-Setup-1.0.1.exe");
        assert_eq!(setup_log(&setup), dir.join("DAWmate-Setup-1.0.1.exe.log"));
        let args = setup_args(&setup, 42);
        assert!(args.contains(&"/WAITPID=42".to_owned()) && args.contains(&"/UPDATED=1".to_owned()));
        assert!(!args.iter().any(|a| a == "/SUPPRESSMSGBOXES"), "the setup's own boxes say what's wrong");
        assert!(!still_whole(&setup), "no SHA-256 kept beside it");
        fs::write(dir.join("DAWmate-Setup-1.0.1.exe.sha256"), format!("{}  x", sha_hex(b"x"))).unwrap();
        assert!(still_whole(&setup));
        fs::write(&setup, "changed").unwrap();
        assert!(!still_whole(&setup));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_an_installed_copy_updates_itself() {
        let dir = test_folder("installed");
        let exe = dir.join("music-organiser.exe");
        assert!(!installed(&exe));
        fs::write(dir.join("unins000.exe"), "").unwrap();
        assert!(installed(&exe));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A stand-in for GitHub on this computer: answers each path from `pages`, everything else not found.
    fn stand_in(pages: Vec<(String, Vec<u8>)>) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).is_ok_and(|n| n == 1) {
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head);
                let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
                let page = pages.iter().find(|(p, _)| *p == path);
                let reply = match page {
                    Some((_, body)) => {
                        let mut r =
                            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                                .into_bytes();
                        r.extend_from_slice(body);
                        r
                    }
                    None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                };
                let _ = stream.write_all(&reply);
            }
        });
        address
    }

    #[test]
    fn a_newer_setup_is_found_fetched_and_checked() {
        let _alone = FETCHES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if Command::new(curl()).arg("--version").output().is_err() {
            eprintln!("no curl here: skipped");
            return;
        }
        let setup = b"the new setup".to_vec();
        let hex = sha_hex(&setup);
        // The stand-in's own address goes into the release list, so the list is made after it's up: a second
        // stand-in serves the files, the first the list.
        let files = stand_in(vec![
            ("/DAWmate-Setup-1.0.1.exe".into(), setup.clone()),
            ("/DAWmate-Setup-1.0.1.exe.sha256".into(), format!("{hex}  DAWmate-Setup-1.0.1.exe").into_bytes()),
            ("/bad.sha256".into(), format!("{}  DAWmate-Setup-1.0.1.exe", "0".repeat(64)).into_bytes()),
        ]);
        let list = format!(
            r#"[{{"tag_name":"v1.0.1","html_url":"{files}/r","assets":[{{"name":"DAWmate-Setup-1.0.1.exe","browser_download_url":"{files}/DAWmate-Setup-1.0.1.exe"}},{{"name":"DAWmate-Setup-1.0.1.exe.sha256","browser_download_url":"{files}/DAWmate-Setup-1.0.1.exe.sha256"}}]}}]"#
        );
        let api = stand_in(vec![(format!("/repos/{REPO}/releases?per_page=20"), list.into_bytes())]);

        let offer = look(&api, "1.0.0").unwrap().unwrap();
        assert_eq!(offer.version, "1.0.1");
        let dir = test_folder("fetch");
        let got = fetch_setup(&offer, &dir).unwrap();
        assert_eq!(fs::read(&got).unwrap(), setup);
        assert!(!dir.join("DAWmate-Setup-1.0.1.exe.mo-part").exists());
        assert_eq!(ready_in(&dir, "1.0.0").map(|r| r.0).as_deref(), Some("1.0.1"));

        // A setup that isn't the one the release lists is never kept.
        fs::remove_file(&got).unwrap();
        fs::remove_file(dir.join("DAWmate-Setup-1.0.1.exe.sha256")).unwrap();
        let wrong = Offer { sha_url: format!("{files}/bad.sha256"), ..offer.clone() };
        assert!(fetch_setup(&wrong, &dir).unwrap_err().contains("SHA-256"));
        assert!(fs::read_dir(&dir).unwrap().next().is_none(), "nothing left behind");

        // GitHub not answering is an error, not an offer.
        assert!(look(&format!("{files}/nowhere"), "1.0.0").is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
