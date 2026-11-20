//! Installing the DAWmate links in the DAWs (Settings › DAWs), as the C# app's `DawScripts` does:
//! - FL Studio: the app's copy of `extensions/fl-studio/device_DAWmate.py` goes into `Settings\Hardware\DAWmate` in
//!   each of FL Studio's user data folders. FL Studio picks it up when it starts.
//! - Bitwig: the built extension (`extensions/bitwig-java`, `DAWmate.bwextension`) goes into `Extensions` in each of
//!   Bitwig's user folders, and the older JavaScript script it replaces comes out of `Controller Scripts`.
//!
//! The app was called Music Organiser until 15 Nov 2026, and so were its links: `device_MusicOrganiser.py` in
//! `Settings\Hardware\Music Organiser`, and `MusicOrganiser.bwextension`. Installing takes those copies out (only those
//! files, and the script's folder when that's left empty), so a DAW never runs the link twice, and a copy of them
//! counts as one to update. The FL script's setting comes over from the old copy.
//!
//! Only those files are ever written, never anything of the DAWs' own. Each is written beside itself first
//! (`.mo-part`), read back to check, and only then put in place, so a copy that can't be written leaves the old one
//! as it was. FL's `LET_THE_APP_MAKE_CHANGES` line is the user's: an update keeps the one their copy has.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The script's file name, as FL Studio wants it (`device_` first).
pub const FL_SCRIPT_NAME: &str = "device_DAWmate.py";

/// The folder in FL Studio's `Settings\Hardware` the script goes in.
pub const FL_FOLDER: &str = "DAWmate";

/// The script's file name before the rename, which installing takes out.
pub const FIRST_FL_SCRIPT_NAME: &str = "device_MusicOrganiser.py";

/// The folder in `Settings\Hardware` it was in, beside [`FL_FOLDER`].
pub const FIRST_FL_FOLDER: &str = "Music Organiser";

/// The app's own copy of the script.
pub const FL_SCRIPT: &str = include_str!("../../../extensions/fl-studio/device_DAWmate.py");

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
    user_data.iter().map(|f| f.join("Settings").join("Hardware").join(FL_FOLDER)).collect()
}

/// Where the script from before the rename is, for a folder [`fl_script_folders`] gives: beside it, in
/// `Settings\Hardware\Music Organiser`. None for a folder that isn't in a `Hardware` folder.
fn first_fl_script(folder: &Path) -> Option<PathBuf> {
    let hardware = folder.parent().filter(|h| h.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Hardware")))?;
    let first = hardware.join(FIRST_FL_FOLDER);
    let same = mo_formats::case::key(&first.to_string_lossy()) == mo_formats::case::key(&folder.to_string_lossy());
    (!same).then(|| first.join(FIRST_FL_SCRIPT_NAME))
}

/// Whether the script in `folders` is the app's own. A copy from before the rename is one to update.
pub fn fl_state(folders: &[PathBuf]) -> ScriptState {
    let shipped = lines(FL_SCRIPT);
    let mut any = false;
    for folder in folders {
        if first_fl_script(folder).is_some_and(|f| f.is_file()) {
            return ScriptState::Outdated;
        }
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
    /// The copies from before the rename it took out.
    pub removed: Vec<PathBuf>,
    /// Older copies it couldn't take out (the new one is in place), and the system's words for why.
    pub left: Vec<(PathBuf, String)>,
}

/// Puts the app's script into every folder, keeping the `LET_THE_APP_MAKE_CHANGES` line each copy has (or, when the
/// folder has none yet, the one the copy from before the rename has), then takes that older copy out. A folder that
/// can't take it (a drive that isn't there now) doesn't stop the others, and keeps its older copy.
pub fn install_fl(folders: &[PathBuf]) -> Installed {
    let mut done = Installed::default();
    for folder in folders {
        let first = first_fl_script(folder).filter(|f| f.is_file());
        match copy_into(folder, first.as_deref()) {
            Ok(()) => done.went.push(folder.clone()),
            Err(e) => {
                done.failed.push((folder.clone(), e.to_string()));
                continue;
            }
        }
        let Some(first) = first else { continue };
        match fs::remove_file(&first) {
            Ok(()) => {
                if let Some(old_folder) = first.parent()
                    && fs::read_dir(old_folder).is_ok_and(|mut d| d.next().is_none())
                {
                    let _ = fs::remove_dir(old_folder);
                }
                done.removed.push(first);
            }
            Err(e) => done.left.push((first, e.to_string())),
        }
    }
    done
}

/// Writes the script into `folder`: beside itself first, checked, then in place. The kept setting is the copy's
/// there, else the one in `first` (the copy from before the rename).
fn copy_into(folder: &Path, first: Option<&Path>) -> io::Result<()> {
    let target = folder.join(FL_SCRIPT_NAME);
    let theirs = fs::read_to_string(&target).ok().or_else(|| first.and_then(|f| fs::read_to_string(f).ok()));
    let text = with_kept(FL_SCRIPT, theirs.as_deref());
    write_checked(folder, FL_SCRIPT_NAME, text.as_bytes())
}

/// Writes `bytes` as `name` in `folder` (made if it isn't there): as `name.mo-part` first, synced and read back, then
/// renamed into place, so a write that fails part way leaves the old file as it was. The part file never stays.
fn write_checked(folder: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let part = stage(folder, name, bytes)?;
    let placed = fs::rename(&part, folder.join(name));
    if placed.is_err() {
        let _ = fs::remove_file(&part);
    }
    placed
}

/// Writes `bytes` beside `name` in `folder` (made if it isn't there) as `name.mo-part`, synced and read back, and
/// returns that part file's path, for the caller to rename into place. A write that fails takes its part file out.
fn stage(folder: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
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
        Ok(())
    })();
    match written {
        Ok(()) => Ok(part),
        Err(e) => {
            let _ = fs::remove_file(&part);
            Err(e)
        }
    }
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
pub const BITWIG_EXTENSION_NAME: &str = "DAWmate.bwextension";

/// The extension's file name before the rename, which installing takes out: it has the same ID, so Bitwig would list
/// the controller twice.
pub const FIRST_BITWIG_EXTENSION_NAME: &str = "MusicOrganiser.bwextension";

/// The copy from before the rename, moved aside while the new one is written (Bitwig loads only `.bwextension`
/// files): taken out once it is, and by the next install if that failed.
const FIRST_BITWIG_ASIDE: &str = "MusicOrganiser.bwextension.mo-old";

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
        f.file_name().is_some_and(|n| {
            n.eq_ignore_ascii_case(BITWIG_EXTENSION_NAME) || n.eq_ignore_ascii_case(FIRST_BITWIG_EXTENSION_NAME)
        }) && f.parent().and_then(Path::file_name).is_some_and(|n| n.eq_ignore_ascii_case("Extensions"))
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

/// Whether the extension in `folders` is the one `source` holds, byte for byte. A copy from before the rename is one
/// to update.
pub fn bitwig_state(source: &Path, folders: &[PathBuf]) -> ScriptState {
    let Ok(built) = fs::read(source) else { return ScriptState::NotInstalled };
    if folders.iter().any(|f| f.join(FIRST_BITWIG_EXTENSION_NAME).is_file() || plain_file(&f.join(FIRST_BITWIG_ASIDE)))
    {
        return ScriptState::Outdated;
    }
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

/// Puts the extension from `source` into every folder and takes out the copy from before the rename
/// (`MusicOrganiser.bwextension`) in each, then takes out the older script
/// (`Controller Scripts\Music Organiser\MusicOrganiser.control.js`) from each user folder the extension went into, so
/// Bitwig doesn't run two: only those files, and the script's `Music Organiser` folder when that's left empty.
///
/// All or nothing, so Bitwig never has two copies of the controller (one ID) in one folder or across two it loads
/// from: every older copy is moved aside (as `MusicOrganiser.bwextension.mo-old`, a name Bitwig never loads) and the
/// new one written beside itself in every folder first; if any of that fails, every step is undone and nothing is
/// put in place anywhere. Only then does each new copy go into place and each older one go. An older copy left aside
/// by an install that was cut off counts as the older copy. On Windows, Bitwig keeps its extensions open, so a copy
/// it has open can't be moved or replaced: close Bitwig first. Folders listed twice (a link to one) are done once.
pub fn install_bitwig(source: &Path, folders: &[PathBuf], user_folders: &[PathBuf]) -> BitwigInstalled {
    let mut done = BitwigInstalled::default();
    let built = match fs::read(source) {
        Ok(b) => b,
        Err(e) => {
            done.installed.failed = folders.iter().map(|f| (f.clone(), e.to_string())).collect();
            return done;
        }
    };
    let mut unique: Vec<(&PathBuf, PathBuf)> = Vec::new();
    for folder in folders {
        let real = fs::canonicalize(folder).unwrap_or_else(|_| folder.clone());
        if !unique.iter().any(|(_, r)| *r == real) {
            unique.push((folder, real));
        }
    }
    let fail_all = |why: String, at: &Path| -> Vec<(PathBuf, String)> {
        folders
            .iter()
            .map(|f| (f.clone(), if f == at { why.clone() } else { format!("not updated, as {why}") }))
            .collect()
    };
    let mut olders: Vec<Option<Older>> = Vec::with_capacity(unique.len());
    let undo = |olders: &[Option<Older>], parts: &[PathBuf]| {
        for part in parts {
            let _ = fs::remove_file(part);
        }
        // Back as it was before the install that moved it aside, this one or one cut off.
        for older in olders.iter().flatten().filter(|o| !o.first.exists()) {
            let _ = fs::rename(&older.aside, &older.first);
        }
    };
    for (folder, _) in &unique {
        let first = folder.join(FIRST_BITWIG_EXTENSION_NAME);
        let aside = folder.join(FIRST_BITWIG_ASIDE);
        let aside_there = plain_file(&aside);
        if !plain_file(&first) {
            olders.push(aside_there.then_some(Older { first, aside }));
            continue;
        }
        // An older copy and one left aside before: the one left aside goes, so this one can take its place.
        if aside_there && let Err(e) = fs::remove_file(&aside) {
            undo(&olders, &[]);
            done.installed.failed = fail_all(format!("{} can't be taken out ({e})", aside.display()), folder);
            return done;
        }
        if let Err(e) = fs::rename(&first, &aside) {
            undo(&olders, &[]);
            done.installed.failed = fail_all(format!("the older copy {} is in use ({e})", first.display()), folder);
            return done;
        }
        olders.push(Some(Older { first, aside }));
    }
    let mut parts: Vec<PathBuf> = Vec::with_capacity(unique.len());
    for (folder, _) in &unique {
        match stage(folder, BITWIG_EXTENSION_NAME, &built) {
            Ok(part) => parts.push(part),
            Err(e) => {
                undo(&olders, &parts);
                done.installed.failed = fail_all(format!("{} couldn't take it ({e})", folder.display()), folder);
                return done;
            }
        }
    }
    let mut went: Vec<&PathBuf> = Vec::new();
    for (((folder, _), part), older) in unique.iter().zip(&parts).zip(&olders) {
        if let Err(e) = fs::rename(part, folder.join(BITWIG_EXTENSION_NAME)) {
            // The copy there now (an earlier DAWmate one Bitwig has open) stays. The older copy goes back only where
            // there's no DAWmate copy beside it, so the folder never has two.
            let _ = fs::remove_file(part);
            if let Some(o) = older
                && !folder.join(BITWIG_EXTENSION_NAME).exists()
            {
                let _ = fs::rename(&o.aside, &o.first);
            } else if let Some(o) = older {
                take_out_aside(o, &mut done);
            }
            done.installed.failed.push(((*folder).clone(), e.to_string()));
            continue;
        }
        went.push(folder);
        if let Some(o) = older {
            take_out_aside(o, &mut done);
        }
    }
    // A folder listed twice went (or didn't) as its first listing did.
    done.installed.went = folders
        .iter()
        .filter(|f| {
            let real = fs::canonicalize(f).unwrap_or_else(|_| (*f).clone());
            unique.iter().any(|(u, r)| *r == real && went.contains(u))
        })
        .cloned()
        .collect();
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
            Err(e) => done.installed.left.push((old, e.to_string())),
        }
    }
    done
}

/// A folder's older copy of the extension, moved aside (by this install, or by one that was cut off): where it was,
/// and where it is now.
struct Older {
    first: PathBuf,
    aside: PathBuf,
}

/// Takes out an older copy moved aside, once the new one is in place: noted as taken out, or as left with why.
fn take_out_aside(older: &Older, done: &mut BitwigInstalled) {
    match fs::remove_file(&older.aside) {
        Ok(()) => done.removed.push(older.first.clone()),
        Err(e) => done.installed.left.push((older.aside.clone(), e.to_string())),
    }
}

/// Whether `path` is a plain file itself (not a link to one).
fn plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}

/// What installing the Bitwig extension did: where it went, and the older copies and scripts it took out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BitwigInstalled {
    pub installed: Installed,
    pub removed: Vec<PathBuf>,
}
