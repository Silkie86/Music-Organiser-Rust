//! Your say on a plugin's folders, on the Plugins page's On disk tab, as the C# app's Space card has it: "Not its" on a
//! guessed folder that isn't the plugin's (left out from now on), "Take off" on one you added, and "Add a folder…" for
//! one the guess missed (a sound library on another drive). It's kept in library.db (`mo_library::plugin_folders`), so
//! the C# app's next measure follows it. Until then, the page shows the change itself ([`Plugins::apply_folder_fixes`]),
//! and keeps showing it over every new read this session.

use chrono::NaiveDateTime;
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
}
