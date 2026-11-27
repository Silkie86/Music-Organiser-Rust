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

/// The starts of every setting line of the script an update keeps and the "is it the app's own copy" check leaves
/// out: [`FL_KEPT`], and the switch for the one-off measure, which is turned on by hand (the 19 Nov review's LK-6).
/// Only a line that is the setting with `True` or `False` counts ([`kept_setting`]): one mistyped by hand makes the
/// copy out of date, and the update writes the app's own line.
const FL_KEPT_LINES: [&str; 2] = [FL_KEPT, "MEASURE_IN_LIVE_TEST = "];

/// Whether `line` is the kept setting that starts with `setting`, on or off, and nothing else.
fn kept_setting(line: &str, setting: &str) -> bool {
    let line = line.trim_end_matches('\r');
    line.strip_prefix(setting).is_some_and(|value| value == "True" || value == "False")
}

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

/// Puts the app's script into every folder, keeping the setting lines each copy has ([`FL_KEPT_LINES`]; or, when the
/// folder has none yet, the ones the copy from before the rename has), then takes that older copy out. A folder that
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

/// Writes the script into `folder`: beside itself first, checked, then in place. The kept settings
/// ([`FL_KEPT_LINES`]) are the copy's there, else the ones in `first` (the copy from before the rename).
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

/// The shipped script with the kept lines ([`FL_KEPT_LINES`]) from `theirs`, each when their copy has one.
fn with_kept(shipped: &str, theirs: Option<&str>) -> String {
    let mut text = shipped.to_owned();
    for setting in FL_KEPT_LINES {
        text = with_kept_line(&text, theirs, setting);
    }
    text
}

/// `shipped` with its first line that starts with `setting` taken from `theirs`, when their copy has one.
fn with_kept_line(shipped: &str, theirs: Option<&str>, setting: &str) -> String {
    let Some(kept) = theirs.and_then(|t| t.lines().find(|l| kept_setting(l, setting))) else {
        return shipped.to_owned();
    };
    let kept = kept.trim_end_matches('\r');
    let mut out = String::with_capacity(shipped.len());
    let mut done = false;
    for line in shipped.split_inclusive('\n') {
        if !done && line.starts_with(setting) {
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

/// The script's lines, whichever line endings it has, without the kept settings and the empty lines at its end.
fn lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !FL_KEPT_LINES.iter().any(|kept| kept_setting(l, kept)))
        .collect();
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

/// The older script's folder in Bitwig's `Controller Scripts`.
const BITWIG_SCRIPT_FOLDER: &str = "Music Organiser";

/// The built extension the app installs: next to the app in `extensions` (where the launcher and the installer put
/// it; in a Mac app bundle, `Contents/Resources/extensions`), or, when the app runs from a build in a copy of the repo,
/// the one Maven built there (`extensions/bitwig-java/target`).
pub fn bitwig_extension_source(app: &Path) -> Option<PathBuf> {
    // A Mac app bundle keeps it in Contents/Resources/extensions.
    let beside = match mac_app_contents(app) {
        Some(contents) => contents.join("Resources").join("extensions").join(BITWIG_EXTENSION_NAME),
        None => app.parent()?.join("extensions").join(BITWIG_EXTENSION_NAME),
    };
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
    // Each folder's place in `unique`, worked out once, before anything is made (a folder made later would have a
    // real path then that it hadn't before).
    let mut unique: Vec<(&PathBuf, PathBuf)> = Vec::new();
    let mut place_of: Vec<usize> = Vec::with_capacity(folders.len());
    for folder in folders {
        let real = fs::canonicalize(folder).unwrap_or_else(|_| folder.clone());
        match unique.iter().position(|(_, r)| *r == real) {
            Some(at) => place_of.push(at),
            None => {
                place_of.push(unique.len());
                unique.push((folder, real));
            }
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
    let mut went: Vec<bool> = vec![false; unique.len()];
    for (at, (((folder, _), part), older)) in unique.iter().zip(&parts).zip(&olders).enumerate() {
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
        went[at] = true;
        if let Some(o) = older {
            take_out_aside(o, &mut done);
        }
    }
    // A folder listed twice went (or didn't) as its first listing did.
    done.installed.went = folders.iter().zip(&place_of).filter(|(_, at)| went[**at]).map(|(f, _)| f.clone()).collect();
    for user in user_folders {
        if !done.installed.went.iter().any(|f| f.parent() == Some(user.as_path())) {
            continue;
        }
        let script_folder = user.join("Controller Scripts").join(BITWIG_SCRIPT_FOLDER);
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

/// The CLAP plugin's file name in a CLAP folder. The installer puts it beside the app as this too, in `plugins`.
pub const CLAP_PLUGIN_NAME: &str = "DAWmate.clap";

/// The names the same plugin (one ID) has when `tools/dev/install-test-plugin.ps1` puts it in the CLAP folder. While
/// one is there the app leaves its own out: a DAW would list the plugin twice.
pub const TEST_CLAP_PLUGIN_NAMES: [&str; 2] = ["DAWmate (test).clap", "Music Organiser (test).clap"];

/// The `Contents` folder of the Mac app bundle `app` runs from (`DAWmate.app/Contents/MacOS/music-organiser`), if it
/// runs from one. What the app brings with it is there, as a Mac app keeps it: the Bitwig extension in
/// `Resources/extensions` and the CLAP plugin bundle in `PlugIns` (`installer/mac/package.sh`).
pub fn mac_app_contents(app: &Path) -> Option<&Path> {
    let macos = app.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let named = |p: &Path, name: &str| p.file_name().is_some_and(|n| n == name);
    let is_app = bundle.extension().is_some_and(|e| e.eq_ignore_ascii_case("app"));
    (named(macos, "MacOS") && named(contents, "Contents") && is_app).then_some(contents)
}

/// The plugin the app brought with it, if it's there: on Windows the file the installer put beside the app
/// (`plugins\DAWmate.clap`); in a Mac app bundle the plugin's own bundle folder in `Contents/PlugIns`.
pub fn clap_plugin_source(app: &Path) -> Option<PathBuf> {
    if let Some(contents) = mac_app_contents(app) {
        let bundle = contents.join("PlugIns").join(CLAP_PLUGIN_NAME);
        return real_folder(&bundle).then_some(bundle);
    }
    Some(app.parent()?.join("plugins").join(CLAP_PLUGIN_NAME)).filter(|f| f.is_file())
}

/// Whether the plugin in `folder` is the one `source` holds, byte for byte: one file, or (on a Mac) a bundle folder,
/// every file in it with the same name and bytes and nothing more.
pub fn clap_state(source: &Path, folder: &Path) -> ScriptState {
    let theirs = folder.join(CLAP_PLUGIN_NAME);
    if real_folder(source) {
        if fs::symlink_metadata(&theirs).is_err() {
            return ScriptState::NotInstalled;
        }
        return if same_tree(source, &theirs).unwrap_or(false) {
            ScriptState::Installed
        } else {
            ScriptState::Outdated
        };
    }
    let Ok(theirs) = fs::read(theirs) else { return ScriptState::NotInstalled };
    match fs::read(source) {
        Ok(built) if built == theirs => ScriptState::Installed,
        _ => ScriptState::Outdated,
    }
}

/// Puts the plugin from `source` into `folder` as [`CLAP_PLUGIN_NAME`]: written beside itself, read back, then put in
/// place, so one that can't be (a DAW has it open) leaves the old one as it was. A bundle folder (a Mac's plugin) is
/// copied whole the same way ([`install_bundle`]).
pub fn install_clap(source: &Path, folder: &Path) -> Installed {
    let mut done = Installed::default();
    let placed = if real_folder(source) {
        install_bundle(source, folder, CLAP_PLUGIN_NAME)
    } else {
        fs::read(source).and_then(|bytes| write_checked(folder, CLAP_PLUGIN_NAME, &bytes))
    };
    match placed {
        Ok(()) => done.went.push(folder.to_owned()),
        Err(e) => done.failed.push((folder.to_owned(), e.to_string())),
    }
    done
}

/// Whether `path` is a folder itself (not a link to one).
fn real_folder(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

/// Every file in the folder `root`, by its path inside it, with its size, in name order. A link anywhere inside, or
/// anything that isn't a plain file or folder, is an error: a plugin bundle the app made holds none.
fn listing(root: &Path) -> io::Result<Vec<(PathBuf, u64)>> {
    fn walk(root: &Path, inside: &Path, out: &mut Vec<(PathBuf, u64)>) -> io::Result<()> {
        let mut entries = fs::read_dir(root.join(inside))?.collect::<io::Result<Vec<_>>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let kind = entry.file_type()?;
            let path = inside.join(entry.file_name());
            if kind.is_dir() {
                walk(root, &path, out)?;
            } else if kind.is_file() {
                out.push((path, entry.metadata()?.len()));
            } else {
                return Err(io::Error::other(format!("{} is a link", root.join(&path).display())));
            }
        }
        Ok(())
    }
    if !real_folder(root) {
        return Err(io::Error::other(format!("{} isn't a folder", root.display())));
    }
    let mut out = Vec::new();
    walk(root, Path::new(""), &mut out)?;
    Ok(out)
}

/// Whether the folders `a` and `b` hold the same files (names, sizes, then bytes), one file read at a time.
fn same_tree(a: &Path, b: &Path) -> io::Result<bool> {
    let files = listing(a)?;
    if listing(b)? != files {
        return Ok(false);
    }
    for (inside, _) in &files {
        if fs::read(a.join(inside))? != fs::read(b.join(inside))? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Copies the bundle folder `source` into `folder` (made if it isn't there) as `name`: first whole beside it
/// (`name.mo-part`), each file synced, then read back and checked against `source`; then the copy that's there (only a
/// folder of that name, never a link or a file) is moved aside (`name.mo-old`), the new one put in its place, and the
/// old one taken out. If the new one can't be put in place, the old one goes back. What a cut-off install left beside
/// it (`.mo-part`, `.mo-old` folders of this name) is cleared first; nothing else in `folder` is touched (rule 28).
fn install_bundle(source: &Path, folder: &Path, name: &str) -> io::Result<()> {
    let files = listing(source)?;
    fs::create_dir_all(folder)?;
    let target = folder.join(name);
    let part = folder.join(format!("{name}.mo-part"));
    let old = folder.join(format!("{name}.mo-old"));
    if fs::symlink_metadata(&target).is_ok_and(|m| !m.is_dir()) {
        return Err(io::Error::other(format!("{} is there and isn't the plugin's folder", target.display())));
    }
    clear_own(&part)?;
    clear_own(&old)?;
    let copied = (|| {
        for (inside, _) in &files {
            let to = part.join(inside);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            // fs::copy keeps the file's permissions: a Mac plugin's program keeps its x bit.
            fs::copy(source.join(inside), &to)?;
            fs::File::open(&to)?.sync_all()?;
        }
        if !same_tree(source, &part)? {
            return Err(io::Error::other("the copy read back wasn't the one written"));
        }
        Ok(())
    })();
    if let Err(e) = copied {
        let _ = clear_own(&part);
        return Err(e);
    }
    let had_one = real_folder(&target);
    if had_one && let Err(e) = fs::rename(&target, &old) {
        let _ = clear_own(&part);
        return Err(e);
    }
    if let Err(e) = fs::rename(&part, &target) {
        if had_one {
            let _ = fs::rename(&old, &target);
        }
        let _ = clear_own(&part);
        return Err(e);
    }
    // The new one is in place; an old one that can't be taken out now is cleared by the next install.
    let _ = clear_own(&old);
    Ok(())
}

/// Takes out `path` if it's there: only a real folder (a link or a file of that name is an error, never followed).
fn clear_own(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(m) if m.is_dir() => fs::remove_dir_all(path),
        Ok(_) => Err(io::Error::other(format!("{} is there and isn't a folder", path.display()))),
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

/// The app's own files of FL Studio's link in its folders (`folders`, as [`fl_script_folders`] gives them): the
/// script, the one from before the rename, and a part file an install cut off left. Only plain files of those names,
/// never reached through a link (see [`take_out`]); one file reached two ways is listed once.
pub fn fl_installed_files(folders: &[PathBuf]) -> Vec<PathBuf> {
    let found = folders.iter().flat_map(|folder| {
        let mine = [folder.join(FL_SCRIPT_NAME), folder.join(format!("{FL_SCRIPT_NAME}.mo-part"))];
        mine.into_iter().chain(first_fl_script(folder))
    });
    own_files(found)
}

/// The app's own files of Bitwig's link in its folders: in each Extensions folder (`folders`) the extension, the one
/// from before the rename, that one moved aside and a part file an install cut off left; in each user folder
/// (`user_folders`) the older JavaScript script. Only plain files of those names, never reached through a link (see
/// [`take_out`]); one file reached two ways is listed once.
pub fn bitwig_installed_files(folders: &[PathBuf], user_folders: &[PathBuf]) -> Vec<PathBuf> {
    let part = format!("{BITWIG_EXTENSION_NAME}.mo-part");
    let in_extensions = folders.iter().flat_map(|f| {
        [BITWIG_EXTENSION_NAME, FIRST_BITWIG_EXTENSION_NAME, FIRST_BITWIG_ASIDE, part.as_str()].map(|name| f.join(name))
    });
    let scripts =
        user_folders.iter().map(|u| u.join("Controller Scripts").join(BITWIG_SCRIPT_FOLDER).join(BITWIG_SCRIPT_NAME));
    own_files(in_extensions.chain(scripts).collect::<Vec<_>>())
}

/// The ones of `files` that [`take_out`] may take out, each once (by its real path).
fn own_files(files: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<(PathBuf, PathBuf)> = Vec::new();
    for file in files {
        if !may_take_out(&file) {
            continue;
        }
        let real = fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
        if !out.iter().any(|(_, r)| *r == real) {
            out.push((file, real));
        }
    }
    out.into_iter().map(|(f, _)| f).collect()
}

/// The app's own file names in the DAWs' folders, and its part files for them.
const OWN_FILES: [&str; 8] = [
    FL_SCRIPT_NAME,
    FIRST_FL_SCRIPT_NAME,
    BITWIG_EXTENSION_NAME,
    FIRST_BITWIG_EXTENSION_NAME,
    FIRST_BITWIG_ASIDE,
    BITWIG_SCRIPT_NAME,
    "device_DAWmate.py.mo-part",
    "DAWmate.bwextension.mo-part",
];

/// Whether `file` is one [`take_out`] may take out: a plain file of one of the app's own names, in a folder that isn't
/// a link and isn't in one (two folders up: the app's own folder or a DAW's Extensions, and the one it's in), so
/// nothing is ever removed through a link (rule 28).
fn may_take_out(file: &Path) -> bool {
    let own = file.file_name().is_some_and(|n| OWN_FILES.contains(&&*n.to_string_lossy()));
    let not_link = |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| !m.file_type().is_symlink());
    own && plain_file(file) && file.ancestors().skip(1).take(2).all(not_link)
}

/// What taking a DAW's link out did: the files taken out, and those that couldn't be with the system's words.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkRemoved {
    pub removed: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
}

/// Takes the app's link out of a DAW: each of `files` (as [`fl_installed_files`] or [`bitwig_installed_files`] found
/// them) that is still one [`may_take_out`] allows, then its folder when that's left empty and is one of the app's own
/// (`DAWmate` or `Music Organiser`), never a DAW's own folder such as Extensions. Removed for good, as Install's own
/// files are: FL's script takes the user's setting for letting the app make changes with it. On Windows a DAW that's
/// open holds its link, so that one stays: close the DAW first.
pub fn take_out(files: &[PathBuf]) -> LinkRemoved {
    const OWN_FOLDERS: [&str; 3] = [FL_FOLDER, FIRST_FL_FOLDER, BITWIG_SCRIPT_FOLDER];
    let mut done = LinkRemoved::default();
    for file in files {
        if !may_take_out(file) {
            continue;
        }
        match fs::remove_file(file) {
            Ok(()) => done.removed.push(file.clone()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                done.failed.push((file.clone(), e.to_string()));
                continue;
            }
        }
        if let Some(folder) = file.parent()
            && folder.file_name().is_some_and(|n| OWN_FOLDERS.contains(&&*n.to_string_lossy()))
            && fs::read_dir(folder).is_ok_and(|mut d| d.next().is_none())
        {
            let _ = fs::remove_dir(folder);
        }
    }
    done
}
