//! Updates from inside the app (the installer plan, step 4; docs/installer.md). Once a day, a while after start and on
//! a low-priority thread, an installed copy asks GitHub's Releases page whether a newer version is out. That one
//! question is all the app sends online, and Settings › General turns it off (rule 1: the app works fully offline).
//! A newer setup is fetched in the background into the data folder's `updates` folder, checked against the SHA-256
//! the release lists, and only then offered: "Restart to update". On a click the app closes as it always does and the
//! setup runs silently, then opens the new version (`/UPDATED=1`, installer/setup.iss).
//!
//! Only a copy the installer put in place updates itself: one built by the launcher, or run from a build folder, never
//! does ([`installed`]). Talking to GitHub goes through Windows' own `curl.exe` (System32), so no web library is built
//! in and Windows' own certificates and proxy settings are used. Tests use a stand-in server on this computer, never
//! GitHub.
//!
//! Rule 28: a setup is removed once its version is the one running (or older), and a half-fetched one
//! (`.mo-part`) at the next look ([`clear_old`]).

use std::cmp::Ordering;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

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

/// The setup's ending on a release: `DAWmate-Setup-1.0.0.exe` (installer/build.ps1).
const SETUP_MARK: &str = "-Setup-";

/// Whether the daily look is on.
pub fn wanted(setting: Option<&str>) -> bool {
    setting != Some("0")
}

/// Whether a look is due: none yet, or the last one [`EVERY_SECS`] ago (or a clock set back).
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
        if parts.next().is_some() || pre.iter().any(String::is_empty) {
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
        let shown = r.tag_name.trim_start_matches('v').to_owned();
        let ending = format!("{SETUP_MARK}{shown}.exe");
        let Some(setup) = r.assets.iter().find(|a| a.name.ends_with(&ending)) else { continue };
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
    let out = command.output().map_err(|e| format!("couldn't start curl: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    String::from_utf8(out.stdout).map_err(|_| "the answer wasn't text".to_owned())
}

/// Asks `api` (GitHub's, or a test's stand-in) for the releases and picks what's newer than `current`.
pub fn look(api: &str, current: &str) -> Result<Option<Offer>, String> {
    let json = fetch(&format!("{api}/repos/{REPO}/releases?per_page=20"), None, 30)?;
    newest(&json, current)
}

/// Fetches the offer's setup into `folder` (written as `.mo-part`, checked against its SHA-256, then named), and gives
/// its path. One there already and whole is kept, not fetched again.
pub fn fetch_setup(offer: &Offer, folder: &Path) -> Result<PathBuf, String> {
    let file = folder.join(&offer.setup_name);
    if offer.setup_name.contains(['/', '\\']) || offer.setup_name.starts_with('.') {
        return Err(format!("the setup's name {} isn't a plain file name", offer.setup_name));
    }
    let want = sha_in(&fetch(&offer.sha_url, None, 30)?).ok_or("the release's SHA-256 can't be read")?;
    if file.is_file() && sha_of(&file).is_ok_and(|h| h == want) {
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
    Ok(file)
}

/// A file's SHA-256, in lower-case hex.
pub fn sha_of(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Removes from `folder` what's of no more use (rule 28): half-fetched setups, and setups for this version or older.
/// Only files the app names itself: `<name>-Setup-<version>.exe` and those with `.mo-part` after them.
pub fn clear_old(folder: &Path, current: &str) {
    let Some(current) = Version::parse(current) else { return };
    let Ok(entries) = fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else { continue };
        if !kind.is_file() {
            continue;
        }
        let setup = name.strip_suffix(".mo-part").unwrap_or(&name);
        let Some(version) = setup.strip_suffix(".exe").and_then(|s| s.rsplit_once(SETUP_MARK)).map(|(_, v)| v) else {
            continue;
        };
        if name.ends_with(".mo-part") || Version::parse(version).is_some_and(|v| v <= current) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// A setup fetched and checked on an earlier day and not put in yet (the app was closed instead): the newest in
/// `folder` newer than `current`, with its version. Only whole setups are named so (a half-fetched one is `.mo-part`).
pub fn ready_in(folder: &Path, current: &str) -> Option<(String, PathBuf)> {
    let current = Version::parse(current)?;
    let mut best: Option<(Version, String, PathBuf)> = None;
    for entry in fs::read_dir(folder).ok()?.flatten() {
        if !entry.file_type().is_ok_and(|k| k.is_file()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(shown) = name.strip_suffix(".exe").and_then(|s| s.rsplit_once(SETUP_MARK)).map(|(_, v)| v.to_owned())
        else {
            continue;
        };
        let Some(version) = Version::parse(&shown) else { continue };
        if version <= current || (version.is_test() && !current.is_test()) {
            continue;
        }
        if best.as_ref().is_none_or(|(b, _, _)| version > *b) {
            best = Some((version, shown, entry.path()));
        }
    }
    best.map(|(_, shown, path)| (shown, path))
}

/// Starts the fetched setup silently, to put the new version in place and open it (installer/setup.iss: it closes
/// what's still running of the app, and `/UPDATED=1` starts the app once it's done). Windows only.
pub fn run_setup(setup: &Path) -> io::Result<()> {
    if !cfg!(windows) {
        return Err(io::Error::other("updates are put in by the Windows installer"));
    }
    Command::new(setup)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/UPDATED=1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
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
        for bad in ["1.2", "1.2.3.4", "x.1.2", "1.2.3-", "1.2.3-a..b", ""] {
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
            "DAWmate-Setup-0.9.0-beta.1.exe",
            "DAWmate-Setup-1.0.1.exe",
            "DAWmate-Setup-1.0.1.exe.mo-part",
            "notes.txt",
            "Other-Setup-x.exe",
        ] {
            fs::write(dir.join(name), "x").unwrap();
        }
        clear_old(&dir, "1.0.0");
        let mut left: Vec<String> =
            fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["DAWmate-Setup-1.0.1.exe", "Other-Setup-x.exe", "notes.txt"]);
        // What's left is offered at the next start, without asking GitHub again.
        let (version, path) = ready_in(&dir, "1.0.0").unwrap();
        assert_eq!((version.as_str(), path), ("1.0.1", dir.join("DAWmate-Setup-1.0.1.exe")));
        assert_eq!(ready_in(&dir, "1.0.1"), None);
        fs::write(dir.join("DAWmate-Setup-1.1.0-beta.1.exe"), "x").unwrap();
        assert_eq!(ready_in(&dir, "1.0.0").unwrap().0, "1.0.1", "a test version only for a tester");
        assert_eq!(ready_in(&dir, "1.0.1-beta.1").unwrap().0, "1.1.0-beta.1");
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
        use std::io::Write;
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
        if Command::new(curl()).arg("--version").output().is_err() {
            eprintln!("no curl here: skipped");
            return;
        }
        let setup = b"the new setup".to_vec();
        let hex = {
            let mut h = Sha256::new();
            h.update(&setup);
            h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
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

        // A setup that isn't the one the release lists is never kept.
        fs::remove_file(&got).unwrap();
        let wrong = Offer { sha_url: format!("{files}/bad.sha256"), ..offer.clone() };
        assert!(fetch_setup(&wrong, &dir).unwrap_err().contains("SHA-256"));
        assert!(fs::read_dir(&dir).unwrap().next().is_none(), "nothing left behind");

        // GitHub not answering is an error, not an offer.
        assert!(look(&format!("{files}/nowhere"), "1.0.0").is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
