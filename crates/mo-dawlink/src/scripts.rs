//! Installing the Music Organiser script in FL Studio (Settings › DAWs), as the C# app's `DawScripts` does: the app's
//! own copy of `extensions/fl-studio/device_MusicOrganiser.py` goes into `Settings\Hardware\Music Organiser` in each of
//! FL Studio's user data folders. FL Studio picks the new copy up the next time it starts (or reloads its scripts).
//!
//! Only that one file is ever written, and only in a folder of its own (`Music Organiser`), never anything of FL's.
//! It's written beside itself first (`.mo-part`), read back to check, and only then put in place, so a copy that can't
//! be written leaves the old one as it was. The `LET_THE_APP_MAKE_CHANGES` line is the user's: an update keeps the
//! one their copy has.

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
    fs::create_dir_all(folder)?;
    let part = folder.join(format!("{FL_SCRIPT_NAME}.mo-part"));
    let written = (|| {
        let mut file = fs::File::create(&part)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if fs::read(&part)? != text.as_bytes() {
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
