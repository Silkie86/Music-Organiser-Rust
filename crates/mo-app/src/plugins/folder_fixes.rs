//! Your say on a plugin's folders, on the Plugins page's On disk tab, as the C# app's Space card has it: "Not its" on a
//! guessed folder that isn't the plugin's (left out from now on), "Take off" on one you added, and "Add a folder…" for
//! one the guess missed (a sound library on another drive). It's kept in library.db (`mo_library::plugin_folders`), so
//! the C# app's next measure follows it. Until then, the page shows the change itself ([`Plugins::apply_folder_fixes`]),
//! and keeps showing it over every new read this session.

use chrono::NaiveDateTime;
use mo_library::plugins::{StoredSpace, added_folders_key, ruled_out_key};
use mo_library::scan::is_inside;
use mo_space::Measured;
use mo_space::plugin_space::{PluginSpace, SpaceFolder, SpaceKind, is_install_folder, kind_of};

use super::Plugins;

/// One change to a plugin's folders, by the plugin's name key (its room's key), with when its room was measured then:
/// once the old app measures it again, the measure follows the change itself and this is dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderFix {
    /// A folder you added, measured when you added it.
    Added { key: String, folder: SpaceFolder, measured: Option<NaiveDateTime> },
    /// A folder that isn't the plugin's any more: ruled out, or one you added taken off.
    Removed { key: String, path: String, measured: Option<NaiveDateTime> },
}

impl FolderFix {
    fn key(&self) -> &str {
        match self {
            FolderFix::Added { key, .. } | FolderFix::Removed { key, .. } => key,
        }
    }

    fn measured(&self) -> Option<NaiveDateTime> {
        match self {
            FolderFix::Added { measured, .. } | FolderFix::Removed { measured, .. } => *measured,
        }
    }
}

/// Your say on each stored plugin's folders, as the library keeps it (`plugin_space_not:<key>` and
/// `plugin_space_added:<key>`), as fixes to apply over the stored measure ([`Plugins::apply_folder_fixes`]): a folder
/// ruled out, one the measure has as added that's no longer on your list (taken off), and one on your list the measure
/// hasn't got (added since), measured here. A list that can't be read is an error, never no say: a ruled-out folder
/// read as the plugin's could be ticked in Clean up. Reads the disk for an added folder: call it away from the window.
pub fn saved(lib: &mo_library::Library, spaces: &[StoredSpace]) -> Result<Vec<FolderFix>, mo_library::LibraryError> {
    let mut fixes = Vec::new();
    for s in spaces {
        let not = lib.strict_list_setting(&ruled_out_key(&s.key))?;
        let added = lib.strict_list_setting(&added_folders_key(&s.key))?;
        let on_list = |path: &str| added.iter().any(|a| same(a, path));
        for path in not.iter().filter(|p| !p.trim().is_empty()) {
            fixes.push(FolderFix::Removed { key: s.key.clone(), path: path.clone(), measured: s.measured });
        }
        for f in s.folders.iter().filter(|f| f.added && !on_list(&f.path)) {
            fixes.push(FolderFix::Removed { key: s.key.clone(), path: f.path.clone(), measured: s.measured });
        }
        for path in added.iter().filter(|a| !a.trim().is_empty() && !s.folders.iter().any(|f| same(&f.path, a))) {
            let Some(measured) = measured_once(path) else { continue };
            let inside: Vec<(String, u64)> = spaces
                .iter()
                .flat_map(|o| &o.folders)
                // One gone since isn't in the fresh measure either, so it isn't taken off twice.
                .filter(|f| !same(&f.path, path) && is_inside(&f.path, path) && is_real(&f.path))
                .map(|f| (f.path.clone(), f.bytes))
                .collect();
            let folder = added_folder(path, measured, &inside);
            fixes.push(FolderFix::Added { key: s.key.clone(), folder, measured: s.measured });
        }
    }
    Ok(fixes)
}

/// The sizes of the folders you added, measured once a run (the page reads the library again often; the folder rarely
/// changes while the app runs), by path key. Adding a folder puts its fresh measure here ([`remember_measure`]).
static SIZES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Measured>>> =
    std::sync::LazyLock::new(Default::default);

/// A folder you added, measured once a run, or `None` when it isn't a real folder there now. The lock is held while it
/// measures, so the Plugins page and Space reading at once measure it once, not twice.
fn measured_once(path: &str) -> Option<Measured> {
    let key = mo_space::key(path);
    let mut sizes = SIZES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(m) = sizes.get(&key) {
        return Some(*m);
    }
    let p = std::path::Path::new(path);
    if !p.is_dir() || mo_space::size::is_link(p) {
        return None;
    }
    let m = mo_space::size::measure(p, &std::sync::atomic::AtomicBool::new(false)).ok()?;
    sizes.insert(key, m);
    Some(m)
}

/// Keeps a folder's fresh measure (taken when you add it) for [`saved`], over one taken before.
pub fn remember_measure(path: &str, measured: Measured) {
    SIZES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(mo_space::key(path), measured);
}

/// Whether a stored folder is a real folder there now (not a link: a measure doesn't go through one).
fn is_real(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

/// Whether a stored folder or plugin file has gone from where it was measured: the system says nothing is there and
/// its drive is there (one on a drive that isn't plugged in, or a network place that can't be reached, is still
/// counted), or it's a link now that the app put there moving it (`moved`, the moves' "from" folders as keys). A link
/// you made yourself (a library kept on another drive) is still the plugin's folder. One that can't be looked at (no
/// rights) counts as there.
fn is_gone(path: &str, moved: &std::collections::HashSet<String>) -> bool {
    let p = std::path::Path::new(path);
    match std::fs::symlink_metadata(p) {
        Ok(_) => mo_space::size::is_link(p) && moved.contains(&mo_space::key(path)),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound && drive_there(p),
    }
}

/// Whether the drive a path is on is there: its drive letter or network place on Windows; elsewhere a drive mounted
/// in /Volumes, /media, /run/media or /mnt, or the system's own.
fn drive_there(path: &std::path::Path) -> bool {
    use std::path::Component;
    let root = if cfg!(windows) {
        path.ancestors().last().map(std::path::Path::to_path_buf)
    } else {
        let names: Vec<&std::ffi::OsStr> =
            path.components().filter_map(|c| if let Component::Normal(n) = c { Some(n) } else { None }).collect();
        let is = |name: &std::ffi::OsStr, wanted: &str| name.to_str().is_some_and(|n| n.eq_ignore_ascii_case(wanted));
        let depth = match names.as_slice() {
            [volumes, _, ..] if is(volumes, "Volumes") => 2,
            [media, _, _, ..] if is(media, "media") => 3,
            [run, media, _, _, ..] if is(run, "run") && is(media, "media") => 4,
            [mnt, _, ..] if is(mnt, "mnt") => 2,
            _ => 0,
        };
        let mut root = std::path::PathBuf::from("/");
        root.extend(&names[..depth.min(names.len())]);
        Some(root)
    };
    root.is_some_and(|r| r.as_os_str().is_empty() || r.is_dir())
}

/// Each stored folder and plugin file (and each folder added since) that has gone from where it was measured
/// ([`is_gone`]): sent to the Recycle Bin, removed by an uninstaller, or moved to another drive (`moved`, the moves'
/// "from" folders, as the library keeps them). One look at each, without following links. Call it away from the
/// window's thread.
pub fn gone(spaces: &[StoredSpace], fixes: &[FolderFix], moved: &[String]) -> Vec<String> {
    let moved: std::collections::HashSet<String> = moved.iter().map(|m| mo_space::key(m)).collect();
    let added = fixes.iter().filter_map(|f| match f {
        FolderFix::Added { folder, .. } => Some(&folder.path),
        FolderFix::Removed { .. } => None,
    });
    let mut seen = std::collections::HashSet::new();
    spaces
        .iter()
        .flat_map(|s| s.folders.iter().map(|f| &f.path))
        .chain(added)
        .filter(|p| seen.insert(mo_space::key(p)))
        .filter(|p| is_gone(p, &moved))
        .cloned()
        .collect()
}

/// What "Not its" does to a folder line: rule out a guessed folder, or take off one you added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Say {
    RuleOut,
    TakeOff,
}

impl Plugins {
    /// The picked plugin's room key and what "Not its" would do to its folder `path`, or `None` when it can't (not one
    /// of its folders, or its plugin file: Uninstall is for those).
    pub fn not_its(&self, row: usize, path: &str) -> Option<(String, Say)> {
        let room = self.rows.get(row)?.room.as_ref()?;
        let f = room.folders.iter().find(|f| same(&f.path, path))?;
        if f.kind == SpaceKind::Program {
            return None;
        }
        Some((room.key.clone(), if f.added { Say::TakeOff } else { Say::RuleOut }))
    }

    /// Whether "Add a folder" can add to the plugin at `row`: installed and measured.
    pub fn can_add_folder(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(|r| r.installed && r.room.is_some())
    }

    /// The plugin at `row`'s room, for "Add a folder" ([`Plugins::can_add_folder`]).
    pub fn add_folder_room(&self, row: usize) -> Option<PluginSpace> {
        self.can_add_folder(row).then(|| self.rows[row].room.clone()).flatten()
    }

    /// When the plugin `key`'s room was measured, as this read has it.
    pub fn measured_of(&self, key: &str) -> Option<NaiveDateTime> {
        self.measured_at.get(key).copied()
    }

    /// Every measured folder (any plugin's) inside `path`, with its size: an added folder is measured without them,
    /// as the old app's measure does, so nothing is counted twice.
    pub fn folders_inside(&self, path: &str) -> Vec<(String, u64)> {
        let mut seen = std::collections::HashSet::new();
        self.spaces
            .iter()
            .flat_map(|s| &s.folders)
            .filter(|f| !same(&f.path, path) && is_inside(&f.path, path) && seen.insert(mo_space::key(&f.path)))
            .map(|f| (f.path.clone(), f.bytes))
            .collect()
    }

    /// Shows these changes until the old app measures the plugin again, and gives back the ones still to show (one
    /// for a plugin measured since is dropped: the measure follows it). Each is shown once, however often it's applied.
    pub fn apply_folder_fixes(&mut self, fixes: &[FolderFix]) -> Vec<FolderFix> {
        let fixes: Vec<FolderFix> =
            fixes.iter().filter(|f| self.measured_of(f.key()) == f.measured()).cloned().collect();
        if fixes.is_empty() {
            return fixes;
        }
        for s in &mut self.spaces {
            apply(s, &fixes);
        }
        share_added(&mut self.spaces.iter_mut().collect::<Vec<_>>(), &fixes);
        let mut rooms: Vec<&mut PluginSpace> = self.rows.iter_mut().filter_map(|r| r.room.as_mut()).collect();
        for room in rooms.iter_mut() {
            apply(room, &fixes);
        }
        share_added(&mut rooms, &fixes);
        self.refilter();
        fixes
    }
}

fn apply(space: &mut PluginSpace, fixes: &[FolderFix]) {
    for fix in fixes {
        match fix {
            FolderFix::Removed { key, path, .. } => {
                if *key == space.key {
                    space.folders.retain(|f| !same(&f.path, path));
                } else {
                    // Another plugin's folder it was shared with is its alone now.
                    for f in space.folders.iter_mut().filter(|f| same(&f.path, path)) {
                        f.shared_with.retain(|k| k != key);
                    }
                }
            }
            FolderFix::Added { key, folder, .. } => {
                let theirs = space.folders.iter_mut().find(|f| same(&f.path, &folder.path));
                match theirs {
                    // Already its: nothing to add.
                    Some(_) if *key == space.key => {}
                    // Another plugin's folder too: shared, both ways.
                    Some(f) => {
                        if !f.shared_with.contains(key) {
                            f.shared_with.push(key.clone());
                        }
                    }
                    None if *key == space.key => {
                        let mut folder = folder.clone();
                        folder.shared_with = Vec::new();
                        space.folders.push(folder);
                    }
                    None => {}
                }
            }
        }
    }
}

/// The other way round: a folder you added that another plugin also has is shared with it on the added plugin's line.
fn share_added(spaces: &mut [&mut PluginSpace], fixes: &[FolderFix]) {
    for fix in fixes {
        let FolderFix::Added { key, folder, .. } = fix else { continue };
        let others: Vec<String> = spaces
            .iter()
            .filter(|s| s.key != *key && s.folders.iter().any(|f| same(&f.path, &folder.path)))
            .map(|s| s.key.clone())
            .collect();
        for s in spaces.iter_mut().filter(|s| s.key == *key) {
            for f in s.folders.iter_mut().filter(|f| f.added && same(&f.path, &folder.path)) {
                f.shared_with = others.clone();
            }
        }
    }
}

/// Why `path` can't be added to `room`, or `None` when it can. `protected` are your project, sample and MIDI folders
/// and `plugin_roots` your plugin folders: never a plugin's own (rule 2), as the measure leaves them out. Looks at the
/// disk: call it away from the window's thread.
pub fn add_problem(path: &str, room: &PluginSpace, protected: &[String], plugin_roots: &[String]) -> Option<String> {
    let p = std::path::Path::new(path);
    if !p.is_dir() {
        return Some("That isn't a folder there now.".to_owned());
    }
    if mo_space::size::is_link(p) {
        return Some("That's a link to another place: pick the folder it leads to.".to_owned());
    }
    if p.parent().is_none() {
        return Some("That's a whole drive: pick the plugin's folder on it.".to_owned());
    }
    if let Some(f) = room.folders.iter().find(|f| is_inside(path, &f.path) || is_inside(&f.path, path)) {
        return Some(if same(&f.path, path) {
            format!("It's one of {}'s folders already.", room.name)
        } else {
            format!("It overlaps {}, one of {}'s folders already.", f.path, room.name)
        });
    }
    if is_install_folder(path) {
        return Some("It's where programs are installed: those go with Uninstall.".to_owned());
    }
    if let Some(system) = mo_space::plugin_clean::system_folder_in(path) {
        return Some(if same(&system, path) {
            "It's one of the computer's own folders: other programs keep their things there.".to_owned()
        } else {
            format!("It holds {system}, one of the computer's own folders: pick the plugin's folder inside it.")
        });
    }
    let overlaps = |roots: &[String]| -> Option<String> {
        roots.iter().find(|r| !r.trim().is_empty() && (is_inside(path, r) || is_inside(r, path))).cloned()
    };
    if let Some(mine) = overlaps(protected) {
        return Some(format!("It's part of your folder {mine}, so it isn't a plugin's."));
    }
    if let Some(plugins) = overlaps(plugin_roots) {
        return Some(format!(
            "It's part of your plugin folder {plugins}: a plugin's own files are counted from there."
        ));
    }
    None
}

/// The folder line for a folder you added, as the measure would make it: its size without `inside` (other measured
/// folders in it, which are theirs), and mostly sounds is Sounds, else Presets.
pub fn added_folder(path: &str, measured: Measured, inside: &[(String, u64)]) -> SpaceFolder {
    let theirs: u64 = inside.iter().map(|(_, b)| b).sum();
    SpaceFolder {
        path: path.trim_end_matches(['/', '\\']).to_owned(),
        kind: kind_of(measured),
        bytes: measured.bytes.saturating_sub(theirs),
        shared_with: Vec::new(),
        added: true,
    }
}

/// What the activity card says once it's kept.
pub fn done_text(name: &str, fix: &FolderFix) -> String {
    match fix {
        FolderFix::Added { folder, .. } => format!(
            "Added {} to {name}'s folders ({}).",
            mo_formats::paths::file_name(&folder.path),
            mo_space::size_text(folder.bytes)
        ),
        FolderFix::Removed { path, .. } => {
            format!("{} isn't counted as {name}'s from now on.", mo_formats::paths::file_name(path))
        }
    }
}

fn same(a: &str, b: &str) -> bool {
    mo_space::key(a) == mo_space::key(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(key: &str, folders: Vec<SpaceFolder>) -> PluginSpace {
        PluginSpace { key: key.into(), name: key.into(), vendor: String::new(), folders }
    }

    fn folder(path: &str, kind: SpaceKind, shared_with: &[&str], added: bool) -> SpaceFolder {
        SpaceFolder {
            path: path.into(),
            kind,
            bytes: 10,
            shared_with: shared_with.iter().map(|s| s.to_string()).collect(),
            added,
        }
    }

    #[test]
    fn a_folder_ruled_out_goes_and_the_other_plugin_has_it_alone() {
        let shared = "C:\\NI\\Library";
        let mut a = space("kontakt7", vec![folder(shared, SpaceKind::Sounds, &["kontakt8"], false)]);
        let mut b = space("kontakt8", vec![folder(shared, SpaceKind::Sounds, &["kontakt7"], false)]);
        let fixes = [FolderFix::Removed { key: "kontakt7".into(), path: "c:/ni/library/".into(), measured: None }];
        apply(&mut a, &fixes);
        apply(&mut b, &fixes);
        // Applied twice, the same.
        apply(&mut a, &fixes);
        assert!(a.folders.is_empty());
        assert!(b.folders[0].shared_with.is_empty());
    }

    #[test]
    fn a_folder_added_shows_once_and_is_shared_with_a_plugin_that_has_it() {
        let lib = "E:\\Sounds";
        let mut a = space("arcade", Vec::new());
        let mut b = space("other", vec![folder(lib, SpaceKind::Sounds, &[], false)]);
        let measured = Measured { bytes: 100, sound_bytes: 80, files: 3 };
        let folder = added_folder("E:\\Sounds\\", measured, &[("E:\\Sounds\\Kit".into(), 30)]);
        assert_eq!(folder.bytes, 70);
        let fixes = [FolderFix::Added { key: "arcade".into(), folder, measured: None }];
        apply(&mut a, &fixes);
        apply(&mut a, &fixes);
        apply(&mut b, &fixes);
        share_added(&mut [&mut a, &mut b], &fixes);
        assert_eq!(a.folders.len(), 1);
        assert_eq!(a.folders[0].shared_with, ["other"]);
        assert!(a.folders[0].added && a.folders[0].kind == SpaceKind::Sounds && a.folders[0].path == lib);
        assert_eq!(b.folders[0].shared_with, ["arcade"]);
    }

    #[test]
    fn your_own_folders_and_program_files_cant_be_added() {
        let d = std::env::temp_dir().join(format!("mo-app-folder-fixes-{}", std::process::id()));
        let inside = d.join("Samples").join("Kits");
        std::fs::create_dir_all(&inside).unwrap();
        let room = space("arcade", Vec::new());
        let samples = d.join("Samples").to_string_lossy().into_owned();
        let kits = inside.to_string_lossy().into_owned();
        assert!(add_problem(&kits, &room, std::slice::from_ref(&samples), &[]).unwrap().contains("your folder"));
        assert!(add_problem(&kits, &room, &[], std::slice::from_ref(&samples)).unwrap().contains("plugin folder"));
        assert!(add_problem(&kits, &room, &[], &[]).is_none());
        assert!(add_problem(&d.join("nope").to_string_lossy(), &room, &[], &[]).unwrap().contains("isn't a folder"));
        assert!(add_problem("/", &room, &[], &[]).unwrap().contains("whole drive"));
        let has = space("arcade", vec![folder(&samples, SpaceKind::Sounds, &[], false)]);
        assert!(add_problem(&kits, &has, &[], &[]).unwrap().contains("overlaps"));
        let _ = std::fs::remove_dir_all(&d);
    }

    fn stored(key: &str, folders: Vec<mo_library::plugins::StoredFolder>) -> StoredSpace {
        StoredSpace { key: key.into(), name: key.into(), vendor: String::new(), folders, measured: None }
    }

    fn stored_folder(path: &str, bytes: u64, added: bool) -> mo_library::plugins::StoredFolder {
        mo_library::plugins::StoredFolder { path: path.into(), kind: 2, bytes, shared_with: Vec::new(), added }
    }

    /// A library holding only `settings`, with these (key, value) rows.
    fn library_with(dir: &std::path::Path, rows: &[(String, &str)]) -> mo_library::Library {
        let path = dir.join("library.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE projects (id INTEGER PRIMARY KEY); CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
        for (k, v) in rows {
            conn.execute("INSERT INTO settings (key, value) VALUES (?1, ?2)", [k.as_str(), v]).unwrap();
        }
        drop(conn);
        mo_library::Library::open_read_only(&path).unwrap()
    }

    /// Review E2-1: your say on a plugin's folders is read back from the library after a restart: a folder ruled out,
    /// one taken off your list (stored as added), and one added since the measure (measured, less what's inside it).
    #[test]
    fn your_say_on_folders_is_read_back_after_a_restart() {
        let d = std::env::temp_dir().join(format!("mo-app-folder-fixes-saved-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let new = d.join("New");
        std::fs::create_dir_all(new.join("Kit")).unwrap();
        std::fs::write(new.join("a.wav"), [0u8; 100]).unwrap();
        let (new_s, kit_s) = (new.to_string_lossy().into_owned(), new.join("Kit").to_string_lossy().into_owned());
        let added = serde_json::to_string(&[&new_s]).unwrap();
        let lib = library_with(
            &d,
            &[(ruled_out_key("arcade"), r#"["C:\\Guess"]"#), (added_folders_key("arcade"), added.as_str())],
        );
        let spaces = [
            stored("arcade", vec![stored_folder("C:\\Guess", 5, false), stored_folder("C:\\Old", 5, true)]),
            stored("other", vec![stored_folder(&kit_s, 40, false)]),
        ];
        let fixes = saved(&lib, &spaces).unwrap();
        assert_eq!(fixes.len(), 3, "{fixes:?}");
        assert!(fixes.contains(&FolderFix::Removed { key: "arcade".into(), path: "C:\\Guess".into(), measured: None }));
        assert!(fixes.contains(&FolderFix::Removed { key: "arcade".into(), path: "C:\\Old".into(), measured: None }));
        let Some(FolderFix::Added { key, folder, .. }) = fixes.iter().find(|f| matches!(f, FolderFix::Added { .. }))
        else {
            panic!("no added folder in {fixes:?}")
        };
        assert_eq!(key, "arcade");
        assert!(folder.added && same(&folder.path, &new_s));
        // What the other plugin's folder inside it holds is the other plugin's.
        assert!(folder.bytes < 100);
        // Only stored plugins are looked at: no say, no fixes.
        assert!(saved(&lib, &[stored("other", Vec::new())]).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A list that can't be read is an error, never "no say": a ruled-out folder read as the plugin's could be ticked.
    #[test]
    fn a_say_that_cant_be_read_is_an_error() {
        let d = std::env::temp_dir().join(format!("mo-app-folder-fixes-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let lib = library_with(&d, &[(ruled_out_key("arcade"), "not a list")]);
        assert!(saved(&lib, &[stored("arcade", Vec::new())]).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Review E2-2: a stored folder that isn't there now (binned, uninstalled) or that the app moved (a link now) is
    /// gone, each listed once; one that's there, a link you made yourself, and one on a drive that isn't there stay.
    #[test]
    fn folders_moved_or_removed_since_are_gone() {
        let d = std::env::temp_dir().join(format!("mo-app-folder-fixes-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (here, moved, yours, away) = (d.join("Here"), d.join("Moved"), d.join("Yours"), d.join("Elsewhere"));
        std::fs::create_dir_all(&here).unwrap();
        std::fs::create_dir_all(&away).unwrap();
        for link in [&moved, &yours] {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&away, link).unwrap();
            // A junction, as a move makes: a symbolic link needs administrator rights on Windows.
            #[cfg(windows)]
            junction::create(&away, link).unwrap();
        }
        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
        let binned = s(&d.join("Binned"));
        let unplugged = if cfg!(windows) { r"Q:\Not plugged in\Kits" } else { "/Volumes/Not plugged in/Kits" };
        let spaces = [
            stored("a", vec![stored_folder(&s(&here), 1, false), stored_folder(&binned, 1, false)]),
            stored("b", vec![stored_folder(&binned, 1, false), stored_folder(&s(&moved), 1, false)]),
            stored("c", vec![stored_folder(&s(&yours), 1, false), stored_folder(unplugged, 1, false)]),
        ];
        let mut gone = gone(&spaces, &[], &[s(&moved)]);
        gone.sort();
        let mut want = vec![binned, s(&moved)];
        want.sort();
        assert_eq!(gone, want);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Review E2-9: the computer's own folders, and a folder holding one, aren't a plugin's (here /tmp and the root).
    #[cfg(target_os = "linux")]
    #[test]
    fn the_computers_own_folders_cant_be_added() {
        let room = space("arcade", Vec::new());
        assert!(add_problem("/tmp", &room, &[], &[]).unwrap().contains("computer's own folders"));
        let d = std::env::temp_dir().join(format!("mo-app-system-folders-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(add_problem(&d.to_string_lossy(), &room, &[], &[]).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
