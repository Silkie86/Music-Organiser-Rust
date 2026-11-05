//! Your say on a plugin's folders, on the Plugins page's On disk tab, as the C# app's Space card has it: "Not its" on a
//! guessed folder that isn't the plugin's (left out from now on), "Take off" on one you added, and "Add a folder…" for
//! one the guess missed (a sound library on another drive). It's kept in library.db (`mo_library::plugin_folders`), so
//! the C# app's next measure follows it. Until then, the page shows the change itself ([`Plugins::apply_folder_fixes`]),
//! and keeps showing it over every new read this session.

use mo_library::scan::is_inside;
use mo_space::Measured;
use mo_space::plugin_space::{PluginSpace, SpaceFolder, SpaceKind, is_install_folder};

use super::Plugins;

/// One change to a plugin's folders, by the plugin's name key (its room's key).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderFix {
    /// A folder you added, measured when you added it.
    Added { key: String, folder: SpaceFolder },
    /// A folder that isn't the plugin's any more: ruled out, or one you added taken off.
    Removed { key: String, path: String },
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

    /// The room key of the plugin at `row`, when "Add a folder" can add to it: installed and measured.
    pub fn can_add_folder(&self, row: usize) -> Option<(String, PluginSpace)> {
        let r = self.rows.get(row)?;
        let room = r.room.as_ref().filter(|_| r.installed)?;
        Some((room.key.clone(), room.clone()))
    }

    /// Shows these changes until the old app measures again (each is shown once, however often it's applied).
    pub fn apply_folder_fixes(&mut self, fixes: &[FolderFix]) {
        if fixes.is_empty() {
            return;
        }
        for s in &mut self.spaces {
            apply(s, fixes);
        }
        share_added(&mut self.spaces.iter_mut().collect::<Vec<_>>(), fixes);
        let mut rooms: Vec<&mut PluginSpace> = self.rows.iter_mut().filter_map(|r| r.room.as_mut()).collect();
        for room in rooms.iter_mut() {
            apply(room, fixes);
        }
        share_added(&mut rooms, fixes);
        let cutoff = self.cutoff();
        for r in &mut self.rows {
            r.unused = super::unused(r, cutoff);
        }
        self.refilter();
    }
}

fn apply(space: &mut PluginSpace, fixes: &[FolderFix]) {
    for fix in fixes {
        match fix {
            FolderFix::Removed { key, path } => {
                if *key == space.key {
                    space.folders.retain(|f| !same(&f.path, path));
                } else {
                    // Another plugin's folder it was shared with is its alone now.
                    for f in space.folders.iter_mut().filter(|f| same(&f.path, path)) {
                        f.shared_with.retain(|k| k != key);
                    }
                }
            }
            FolderFix::Added { key, folder } => {
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
        let FolderFix::Added { key, folder } = fix else { continue };
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

/// Why `path` can't be added to `room`, or `None` when it can. `protected` are your project, sample and MIDI folders:
/// never a plugin's (rule 2), as the measure leaves them out. Looks at the disk: call it away from the window's thread.
pub fn add_problem(path: &str, room: &PluginSpace, protected: &[String]) -> Option<String> {
    let p = std::path::Path::new(path);
    if !p.is_dir() {
        return Some("That isn't a folder there now.".to_owned());
    }
    if mo_space::size::is_link(p) {
        return Some("That's a link to another place: pick the folder it leads to.".to_owned());
    }
    if room.folders.iter().any(|f| same(&f.path, path)) {
        return Some(format!("It's one of {}'s folders already.", room.name));
    }
    if is_install_folder(path) {
        return Some("It's where programs are installed: those go with Uninstall.".to_owned());
    }
    if let Some(mine) = protected.iter().find(|r| !r.trim().is_empty() && (is_inside(path, r) || is_inside(r, path))) {
        return Some(format!("It's part of your folder {mine}, so it isn't a plugin's."));
    }
    None
}

/// The folder line for a folder you added, as the measure would make it: mostly sounds is Sounds, else Presets.
pub fn added_folder(path: &str, measured: Measured) -> SpaceFolder {
    let sounds = measured.bytes > 0 && measured.sound_bytes.saturating_mul(2) >= measured.bytes;
    SpaceFolder {
        path: path.trim_end_matches(['/', '\\']).to_owned(),
        kind: if sounds { SpaceKind::Sounds } else { SpaceKind::Presets },
        bytes: measured.bytes,
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
        let fixes = [FolderFix::Removed { key: "kontakt7".into(), path: "c:/ni/library/".into() }];
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
        let fixes = [FolderFix::Added { key: "arcade".into(), folder: added_folder("E:\\Sounds\\", measured) }];
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
        assert!(add_problem(&kits, &room, std::slice::from_ref(&samples)).unwrap().contains("your folder"));
        assert!(add_problem(&kits, &room, &[]).is_none());
        assert!(add_problem(&d.join("nope").to_string_lossy(), &room, &[]).unwrap().contains("isn't a folder"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
