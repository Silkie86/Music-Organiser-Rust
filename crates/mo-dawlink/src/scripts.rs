//! Installing the Music Organiser links in the DAWs (Settings › DAWs), as the C# app's `DawScripts` does:
//! - FL Studio: the app's copy of `extensions/fl-studio/device_MusicOrganiser.py` goes into
//!   `Settings\Hardware\Music Organiser` in each of FL Studio's user data folders. FL Studio picks it up when it starts.
//! - Bitwig: the built extension (`extensions/bitwig-java`, `MusicOrganiser.bwextension`) goes into `Extensions` in
//!   each of Bitwig's user folders, and the older JavaScript script it replaces comes out of `Controller Scripts`.
//!
//! Only those files are ever written, never anything of the DAWs' own. Each is written beside itself first
//! (`.mo-part`), read back to check, and only then put in place, so a copy that can't be written leaves the old one
//! as it was. FL's `LET_THE_APP_MAKE_CHANGES` line is the user's: an update keeps the one their copy has.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The script's file name, as FL Studio wants it (`device_` first).
pub const FL_SCRIPT_NAME: &str = "device_MusicOrganiser.py";

/// The app's own copy of the script.
pub const FL_SCRIPT: &str = include_str!("../../../extensions/fl-studio/device_MusicOrganiser.py");

/// The start of the script's setting for letting the app make changes, which an update keeps.
pub const FL_KEPT: &str = "LET_THE_APP_MAKE_CHANGES = ";

/// Whether a folder's copy of a script is the app's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptState {
    NotInstalled,
    /// Every copy is the app's own (apart from the kept setting).
    Installed,
    /// A copy is older (or otherwise not the app's own): Install puts the app's in.
    Outdated,
}

/// FL Studio's user data folder has Settings, Presets and Projects folders in it.
pub fn is_fl_user_data(folder: &Path) -> bool {
    ["Settings", "Presets", "Projects"].iter().all(|d| folder.join(d).is_dir())
}

/// FL Studio's user data folders: the usual one in `documents` (`Image-Line\FL Studio`), and any that holds one of the
/// project folders (FL Studio keeps its Projects folder beside its Settings, and can be told to use another place,
/// like `I:\Fl Studio Data\FL Studio`). Each one once, in that order.
pub fn fl_user_data_folders(documents: &Path, project_folders: &[String]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut add = |folder: &Path| {
        let key = mo_formats::case::key(&folder.to_string_lossy());
        if !found.iter().any(|f| mo_formats::case::key(&f.to_string_lossy()) == key) {
            found.push(folder.to_owned());
        }
    };
    let usual = documents.join("Image-Line").join("FL Studio");
    if is_fl_user_data(&usual) {
        add(&usual);
    }
    for root in project_folders {
        for folder in Path::new(root).ancestors() {
            if folder.as_os_str().is_empty() {
                break;
            }
            if is_fl_user_data(folder) {
                add(folder);
                break;
            }
        }
    }
    found
}

/// Where the script goes in each user data folder.
pub fn fl_script_folders(user_data: &[PathBuf]) -> Vec<PathBuf> {
    user_data.iter().map(|f| f.join("Settings").join("Hardware").join("Music Organiser")).collect()
}

/// Whether the script in `folders` is the app's own.
pub fn fl_state(folders: &[PathBuf]) -> ScriptState {
    let shipped = lines(FL_SCRIPT);
    let mut any = false;
    for folder in folders {
        let Ok(theirs) = fs::read_to_string(folder.join(FL_SCRIPT_NAME)) else { continue };
        any = true;
        if lines(&theirs) != shipped {
            return ScriptState::Outdated;
        }
    }
    if any { ScriptState::Installed } else { ScriptState::NotInstalled }
}

/// What Install did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Installed {
    /// The folders the script went into.
    pub went: Vec<PathBuf>,
    /// The folders that couldn't take it, and the system's words for why.
    pub failed: Vec<(PathBuf, String)>,
}

/// Puts the app's script into every folder, keeping the `LET_THE_APP_MAKE_CHANGES` line each copy has. A folder that
/// can't take it (a drive that isn't there now) doesn't stop the others.
pub fn install_fl(folders: &[PathBuf]) -> Installed {
    let mut done = Installed::default();
    for folder in folders {
        match copy_into(folder) {
            Ok(()) => done.went.push(folder.clone()),
            Err(e) => done.failed.push((folder.clone(), e.to_string())),
        }
    }
    done
}

/// Writes the script into `folder`: beside itself first, checked, then in place.
fn copy_into(folder: &Path) -> io::Result<()> {
    let target = folder.join(FL_SCRIPT_NAME);
    let text = with_kept(FL_SCRIPT, fs::read_to_string(&target).ok().as_deref());
    write_checked(folder, FL_SCRIPT_NAME, text.as_bytes())
}

/// Writes `bytes` as `name` in `folder` (made if it isn't there): as `name.mo-part` first, synced and read back, then
/// renamed into place, so a write that fails part way leaves the old file as it was. The part file never stays.
fn write_checked(folder: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let target = folder.join(name);
    fs::create_dir_all(folder)?;
    let part = folder.join(format!("{name}.mo-part"));
    let written = (|| {
        let mut file = fs::File::create(&part)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if fs::read(&part)? != bytes {
            return Err(io::Error::other("the copy read back wasn't the one written"));
        }
        fs::rename(&part, &target)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&part);
    }
    written
}

/// The shipped script with the kept line from `theirs`, when their copy has one.
fn with_kept(shipped: &str, theirs: Option<&str>) -> String {
    let Some(kept) = theirs.and_then(|t| t.lines().find(|l| l.starts_with(FL_KEPT))) else {
        return shipped.to_owned();
    };
    let kept = kept.trim_end_matches('\r');
    let mut out = String::with_capacity(shipped.len());
    let mut done = false;
    for line in shipped.split_inclusive('\n') {
        if !done && line.starts_with(FL_KEPT) {
            let end = if line.ends_with("\r\n") {
                "\r\n"
            } else if line.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            out.push_str(kept);
            out.push_str(end);
            done = true;
        } else {
            out.push_str(line);
        }
    }
    out
}

/// The script's lines, whichever line endings it has, without the kept setting and the empty lines at its end.
fn lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> =
        text.lines().map(|l| l.trim_end_matches('\r')).filter(|l| !l.starts_with(FL_KEPT)).collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

/// The Bitwig extension's file name.
pub const BITWIG_EXTENSION_NAME: &str = "MusicOrganiser.bwextension";

/// The older JavaScript script's file name, which the extension replaces.
pub const BITWIG_SCRIPT_NAME: &str = "MusicOrganiser.control.js";

/// The built extension the app installs: next to the app in `extensions` (where the launcher puts it), or, when the
/// app runs from a build in a copy of the repo, the one Maven built there (`extensions/bitwig-java/target`).
pub fn bitwig_extension_source(app: &Path) -> Option<PathBuf> {
    let beside = app.parent()?.join("extensions").join(BITWIG_EXTENSION_NAME);
    if beside.is_file() {
        return Some(beside);
    }
    app.ancestors()
        .map(|d| d.join("extensions").join("bitwig-java").join("target").join(BITWIG_EXTENSION_NAME))
        .find(|f| f.is_file())
}

/// Bitwig's user folders: the one chosen (`bitwig_user_folder`, like `K:\Bitwig Studio`, on Settings › DAWs or before
/// in the C# app), and every `Bitwig Studio` folder in a Documents folder that's there. Bitwig may load extensions from
/// one while it keeps the projects in another, and every copy is the same, so the extension goes in each (as the C#
/// app does).
pub fn bitwig_user_folders(chosen: Option<&str>, documents: &[PathBuf]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut add = |f: PathBuf| {
        let key = mo_formats::case::key(f.to_string_lossy().trim_end_matches(['\\', '/']));
        if !found.iter().any(|g| mo_formats::case::key(g.to_string_lossy().trim_end_matches(['\\', '/'])) == key) {
            found.push(f);
        }
    };
    if let Some(chosen) = chosen.map(str::trim).filter(|c| !c.is_empty()) {
        add(PathBuf::from(chosen));
    }
    for d in documents {
        let f = d.join("Bitwig Studio");
        if f.is_dir() {
            add(f);
        }
    }
    found
}

/// Where the extension goes: `Extensions` in each user folder, and the Extensions folder Bitwig was seen loading it
/// from (the link's `loadedFrom`), when that is one and is there.
pub fn bitwig_extension_folders(user_folders: &[PathBuf], loaded_from: Option<&str>) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = user_folders.iter().map(|f| f.join("Extensions")).collect();
    let seen = loaded_from.map(Path::new).filter(|f| {
        f.file_name().is_some_and(|n| n.eq_ignore_ascii_case(BITWIG_EXTENSION_NAME))
            && f.parent().and_then(Path::file_name).is_some_and(|n| n.eq_ignore_ascii_case("Extensions"))
    });
    if let Some(folder) = seen.and_then(Path::parent).filter(|f| f.is_dir()) {
        let key = mo_formats::case::key(&folder.to_string_lossy());
        if !folders.iter().any(|f| mo_formats::case::key(&f.to_string_lossy()) == key) {
            folders.push(folder.to_owned());
        }
    }
    folders
}

/// The version a built extension gives in its manifest (`Implementation-Version`), if it says.
pub fn extension_version(file: &Path) -> Option<String> {
    let mut zip = zip::ZipArchive::new(fs::File::open(file).ok()?).ok()?;
    let mut manifest = String::new();
    io::Read::read_to_string(&mut zip.by_name("META-INF/MANIFEST.MF").ok()?, &mut manifest).ok()?;
    manifest.lines().find_map(|l| {
        let (key, value) = l.split_once(':')?;
        key.trim().eq_ignore_ascii_case("Implementation-Version").then(|| value.trim().to_owned())
    })
}

/// Whether the extension in `folders` is the one `source` holds, byte for byte.
pub fn bitwig_state(source: &Path, folders: &[PathBuf]) -> ScriptState {
    let Ok(built) = fs::read(source) else { return ScriptState::NotInstalled };
    let mut any = false;
    for folder in folders {
        let Ok(theirs) = fs::read(folder.join(BITWIG_EXTENSION_NAME)) else { continue };
        any = true;
        if theirs != built {
            return ScriptState::Outdated;
        }
    }
    if any { ScriptState::Installed } else { ScriptState::NotInstalled }
}

/// Puts the extension from `source` into every folder, then takes out the older script
/// (`Controller Scripts\Music Organiser\MusicOrganiser.control.js`) from each user folder the extension went into, so
/// Bitwig doesn't run both: only that file, and its `Music Organiser` folder when that's left empty. On Windows,
/// Bitwig keeps its extensions open, so a copy it has open can't be replaced: close Bitwig first.
pub fn install_bitwig(source: &Path, folders: &[PathBuf], user_folders: &[PathBuf]) -> BitwigInstalled {
    let mut done = BitwigInstalled::default();
    let built = match fs::read(source) {
        Ok(b) => b,
        Err(e) => {
            done.installed.failed = folders.iter().map(|f| (f.clone(), e.to_string())).collect();
            return done;
        }
    };
    for folder in folders {
        match write_checked(folder, BITWIG_EXTENSION_NAME, &built) {
            Ok(()) => done.installed.went.push(folder.clone()),
            Err(e) => done.installed.failed.push((folder.clone(), e.to_string())),
        }
    }
    for user in user_folders {
        if !done.installed.went.iter().any(|f| f.parent() == Some(user.as_path())) {
            continue;
        }
        let script_folder = user.join("Controller Scripts").join("Music Organiser");
        let old = script_folder.join(BITWIG_SCRIPT_NAME);
        if !old.is_file() {
            continue;
        }
        match fs::remove_file(&old) {
            Ok(()) => {
                done.removed.push(old);
                if fs::read_dir(&script_folder).is_ok_and(|mut d| d.next().is_none()) {
                    let _ = fs::remove_dir(&script_folder);
                }
            }
            Err(e) => done.installed.failed.push((old, e.to_string())),
        }
    }
    done
}

/// What installing the Bitwig extension did: where it went, and the older scripts it took out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BitwigInstalled {
    pub installed: Installed,
    pub removed: Vec<PathBuf>,
}
