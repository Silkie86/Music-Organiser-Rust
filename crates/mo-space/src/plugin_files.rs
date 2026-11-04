//! A plugin's own files (its `.vst3`, `.dll`, `.clap`, `.component` or `.aaxplugin`), for the Plugins page's Uninstall
//! when Windows has no uninstaller for it that's still there: the C# app's card that sends them to the Recycle Bin
//! instead. Each file is looked at first ([`look`]) and checked again just before it goes ([`remove`]): still the
//! plugin's, still a plugin file of its format, not a link, somewhere the Recycle Bin can keep it, and holding just
//! what it held when you looked. A WaveShell never goes here (every Waves plugin lives in it), nor a file another
//! plugin shares.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::clean::{Bin, Done, Problem, STOPPED, Sending, Sent, What};
use crate::plugin_space::{PluginSpace, SpaceFolder, SpaceKind};
use crate::plugins::{format_of, is_waves};
use crate::spare::{look as look_at, still_the_file};
use crate::{Stopped, key, real_path};

/// One of a plugin's files on the Uninstall card: whether it may go, and why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLine {
    pub path: String,
    /// Its plugin format ("VST3"), or "" when it isn't a plugin file.
    pub format: String,
    /// How much room it takes here now.
    pub bytes: u64,
    /// Why it stays, or `None` when it may go.
    pub stays: Option<String>,
    /// What it held when it was looked at.
    stamp: Option<String>,
}

/// Why a file that has gone stays on the list.
pub const NOT_THERE: &str = "It isn't there any more.";

/// Why a WaveShell stays.
pub const WAVES: &str = "Every Waves plugin lives in one WaveShell file, so it isn't removed here. Use Waves Central to remove Waves \
     plugins.";

/// Each of `space`'s plugin files, looked at now: what it takes and whether it may go. `names` gives another plugin's
/// name from its name key (for a file it shares).
pub fn look(space: &PluginSpace, names: &dyn Fn(&str) -> String, stop: &AtomicBool) -> Result<Vec<FileLine>, Stopped> {
    let mut out = Vec::new();
    for f in space.folders.iter().filter(|f| f.kind == SpaceKind::Program) {
        if stop.load(Ordering::Relaxed) {
            return Err(Stopped);
        }
        let format = format_of(&f.path).unwrap_or_default();
        let there = std::fs::symlink_metadata(&f.path).is_ok();
        let stays = if there { why_it_stays(space, f, &format, names) } else { Some(NOT_THERE.to_owned()) };
        let (bytes, stamp) = if stays.is_some() { (f.bytes, None) } else { look_at(&f.path, stop) };
        if stop.load(Ordering::Relaxed) {
            return Err(Stopped);
        }
        let stays = stays.or_else(|| {
            stamp.is_none().then(|| {
                "It can't go to the Recycle Bin from here (a link, on a network drive, only online in OneDrive, or \
                 some of it couldn't be read)."
                    .to_owned()
            })
        });
        out.push(FileLine { path: f.path.clone(), format, bytes, stays, stamp });
    }
    Ok(out)
}

fn why_it_stays(space: &PluginSpace, f: &SpaceFolder, format: &str, names: &dyn Fn(&str) -> String) -> Option<String> {
    if is_waves(&space.name, &f.path) {
        return Some(WAVES.to_owned());
    }
    if f.shared() {
        let others: Vec<String> = f.shared_with.iter().map(|k| names(k)).collect();
        return Some(format!("{} uses this file too, so it stays.", others.join(" and ")));
    }
    if format.is_empty() {
        return Some("It isn't a plugin file, so it stays.".to_owned());
    }
    None
}

/// Sends the picked files to the Recycle Bin, each checked again just before it goes: it must still be one of
/// `space`'s plugin files (as new as you have it), not shared, not a WaveShell, and still what [`look`] found. Files
/// Windows guards (Program Files) all go in one administrator step at the end, so Windows asks once. Lines that must
/// stay are never sent; a file picked twice goes once.
pub fn remove(
    picked: &[FileLine],
    space: &PluginSpace,
    names: &dyn Fn(&str) -> String,
    bin: &dyn Bin,
    stop: &AtomicBool,
) -> Done {
    let mut done = Done::default();
    let mut sending = Sending::new(bin, stop);
    // Each file once, by where it really is: the first line sent for it.
    let mut seen: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
    for line in picked {
        let real = real_path(&line.path).unwrap_or_else(|| line.path.clone());
        if let Some(first) = seen.get(&key(&real)) {
            if key(first) != key(&line.path) {
                let why = format!("It's the same file as {first}, reached another way, so it's only sent once.");
                done.problems.push(Problem { path: line.path.clone(), why });
            }
            continue;
        }
        let check = move || -> Result<(), String> {
            if stop.load(Ordering::Relaxed) {
                return Err(STOPPED.to_owned());
            }
            if let Some(why) = &line.stays {
                return Err(why.clone());
            }
            let Some(now) =
                space.folders.iter().find(|f| f.kind == SpaceKind::Program && key(&f.path) == key(&line.path))
            else {
                return Err("It isn't one of the plugin's files any more, so it stays.".to_owned());
            };
            if let Some(why) = why_it_stays(space, now, &line.format, names) {
                return Err(why);
            }
            still_the_file(&line.path, &line.format, line.stamp.as_deref(), stop)
        };
        if let Err(why) = check() {
            done.problems.push(Problem { path: line.path.clone(), why });
            continue;
        }
        let real_key = key(&real);
        // Only once it went (or waits for Windows): if it stayed, a line reaching it another way is still tried.
        match sending.send(&line.path, real, line.bytes, What::Plugin(SpaceKind::Program), &mut done, check) {
            Sent::Went | Sent::Waits => {
                seen.insert(real_key, &line.path);
            }
            Sent::Stays => {}
        }
    }
    sending.finish(&mut done);
    done
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::clean::Removed;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    struct FolderBin {
        dir: PathBuf,
        sent: RefCell<Vec<String>>,
    }

    impl Bin for FolderBin {
        fn send(&self, path: &Path) -> Result<(), String> {
            let to = self.dir.join(self.sent.borrow().len().to_string());
            fs::create_dir_all(&self.dir).unwrap();
            fs::rename(path, to).map_err(|e| e.to_string())?;
            self.sent.borrow_mut().push(path.to_string_lossy().into_owned());
            Ok(())
        }
        fn find(&self, removed: &mut [Removed]) -> Vec<Problem> {
            for r in removed {
                r.bin_id = Some(r.real.clone());
            }
            Vec::new()
        }
        fn put_back(&self, _removed: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }
    }

    fn folder(path: &Path, shared_with: &[&str]) -> SpaceFolder {
        SpaceFolder {
            path: path.to_string_lossy().into_owned(),
            kind: SpaceKind::Program,
            bytes: 10,
            shared_with: shared_with.iter().map(|s| s.to_string()).collect(),
            added: false,
        }
    }

    #[test]
    fn its_own_plugin_files_go_and_a_shared_or_changed_one_stays() {
        let d = dir("mo-space-plugin-files");
        let vst3 = d.join("Arcade.vst3");
        fs::create_dir_all(vst3.join("Contents")).unwrap();
        fs::write(vst3.join("Contents/a.bin"), [1u8; 100]).unwrap();
        let clap = d.join("Arcade.clap");
        fs::write(&clap, [1u8; 50]).unwrap();
        let shared = d.join("Shared.dll");
        fs::write(&shared, [1u8; 5]).unwrap();
        let readme = d.join("readme.txt");
        fs::write(&readme, "hi").unwrap();
        let space = PluginSpace {
            key: "arcade".into(),
            name: "Arcade".into(),
            vendor: String::new(),
            folders: vec![folder(&vst3, &[]), folder(&clap, &[]), folder(&shared, &["other"]), folder(&readme, &[])],
        };
        let names = |k: &str| if k == "other" { "Other Synth".to_owned() } else { k.to_owned() };
        let stop = AtomicBool::new(false);
        let lines = look(&space, &names, &stop).unwrap();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].stays.is_none() && lines[0].format == "VST3" && lines[0].bytes == 100, "{:?}", lines[0]);
        assert!(lines[1].stays.is_none() && lines[1].format == "CLAP");
        assert!(lines[2].stays.as_deref().unwrap().contains("Other Synth uses this file too"));
        assert!(lines[3].stays.as_deref().unwrap().contains("isn't a plugin file"));

        // The CLAP changed since: it stays. A line that must stay isn't sent even when picked.
        fs::write(&clap, [2u8; 60]).unwrap();
        let bin = FolderBin { dir: d.join("bin"), sent: RefCell::new(Vec::new()) };
        let done = remove(&lines, &space, &names, &bin, &stop);
        assert_eq!(done.removed.len(), 1, "{:?}", done.problems);
        assert!(!vst3.exists() && clap.exists() && shared.exists() && readme.exists());
        assert!(done.problems.iter().any(|p| p.path.ends_with("Arcade.clap") && p.why.contains("changed")));
        assert_eq!(done.problems.len(), 3);
    }

    #[test]
    fn a_file_already_gone_says_so() {
        let d = dir("mo-space-plugin-files-missing");
        let space = PluginSpace {
            key: "arcade".into(),
            name: "Arcade".into(),
            vendor: String::new(),
            folders: vec![folder(&d.join("Arcade.vst3"), &[])],
        };
        let lines = look(&space, &|k: &str| k.to_owned(), &AtomicBool::new(false)).unwrap();
        assert_eq!(lines[0].stays.as_deref(), Some(NOT_THERE));
    }

    #[test]
    fn a_waveshell_never_goes_here() {
        let d = dir("mo-space-plugin-files-waves");
        let shell = d.join("WaveShell1-VST3 15.0_x64.vst3");
        fs::create_dir_all(&shell).unwrap();
        let space = PluginSpace {
            key: "waveshell".into(),
            name: "L2".into(),
            vendor: "Waves".into(),
            folders: vec![folder(&shell, &[])],
        };
        let names = |k: &str| k.to_owned();
        let stop = AtomicBool::new(false);
        let lines = look(&space, &names, &stop).unwrap();
        assert_eq!(lines[0].stays.as_deref(), Some(WAVES));
        let bin = FolderBin { dir: d.join("bin"), sent: RefCell::new(Vec::new()) };
        assert!(remove(&lines, &space, &names, &bin, &stop).removed.is_empty());
        assert!(shell.exists());
    }

    #[test]
    fn a_file_no_longer_the_plugins_stays() {
        let d = dir("mo-space-plugin-files-gone");
        let clap = d.join("Arcade.clap");
        fs::write(&clap, [1u8; 50]).unwrap();
        let mut space = PluginSpace {
            key: "arcade".into(),
            name: "Arcade".into(),
            vendor: String::new(),
            folders: vec![folder(&clap, &[])],
        };
        let names = |k: &str| k.to_owned();
        let stop = AtomicBool::new(false);
        let lines = look(&space, &names, &stop).unwrap();
        space.folders.clear();
        let bin = FolderBin { dir: d.join("bin"), sent: RefCell::new(Vec::new()) };
        let done = remove(&lines, &space, &names, &bin, &stop);
        assert!(done.removed.is_empty() && clap.exists());
        assert!(done.problems[0].why.contains("isn't one of the plugin's files"));
    }
}
