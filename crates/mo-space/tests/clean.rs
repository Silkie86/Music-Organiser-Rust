//! Clean up: preview, removing to a bin and putting back. The bin here is a folder in the test build's own fixtures
//! folder, never the computer's Recycle Bin, and every file is made there under a fixed name.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use mo_space::clean::{Bin, Guard, Problem, Refused, Removed, apply, preview, undo};
use mo_space::key;
use mo_space::song::{CompactItem, CompactKind};

mod common;

use common::folder_link;

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) -> String {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, bytes).unwrap();
    p.to_string_lossy().into_owned()
}

fn line(kind: CompactKind, path: &str) -> CompactItem {
    CompactItem {
        kind,
        path: path.to_owned(),
        bytes: fs::metadata(path).map_or(0, |m| m.len()),
        modified: None,
        same_as: None,
    }
}

fn copy_of(path: &str, twin: &str) -> CompactItem {
    CompactItem { same_as: Some(twin.to_owned()), ..line(CompactKind::SameSave, path) }
}

/// A bin that's a folder: each thing goes in under a number, and comes back if nothing has its name.
struct FolderBin {
    dir: PathBuf,
    held: RefCell<Vec<(String, PathBuf)>>,
}

impl FolderBin {
    fn new(dir: PathBuf) -> FolderBin {
        fs::create_dir_all(&dir).unwrap();
        FolderBin { dir, held: RefCell::new(Vec::new()) }
    }
}

impl Bin for FolderBin {
    fn send(&self, path: &Path) -> Result<(), String> {
        let mut held = self.held.borrow_mut();
        let to = self.dir.join(held.len().to_string());
        fs::rename(path, &to).map_err(|e| e.to_string())?;
        held.push((path.to_string_lossy().into_owned(), to));
        Ok(())
    }

    fn find(&self, removed: &mut [Removed]) -> Vec<Problem> {
        let held = self.held.borrow();
        for r in removed {
            r.bin_id =
                held.iter().rev().find(|(p, _)| key(p) == key(&r.real)).map(|(_, at)| at.to_string_lossy().into());
        }
        Vec::new()
    }

    fn put_back(&self, removed: &[Removed]) -> Vec<Problem> {
        let held = self.held.borrow();
        let mut problems = Vec::new();
        for r in removed {
            let Some((_, at)) = held.iter().find(|(_, at)| r.bin_id.as_deref() == at.to_str()) else {
                problems.push(Problem { path: r.path.clone(), why: "not in the bin".into() });
                continue;
            };
            if Path::new(&r.path).exists() {
                problems.push(Problem { path: r.path.clone(), why: "something has its name".into() });
                continue;
            }
            fs::rename(at, &r.path).unwrap();
        }
        problems
    }
}

struct Library {
    roots: Vec<String>,
    samples: Vec<String>,
    keep: HashSet<String>,
}

fn no_sample_in_use(_: &str) -> bool {
    false
}

#[test]
fn the_preview_refuses_what_must_stay_and_changes_nothing() {
    let dir = fixtures("mo-space-clean-preview");
    let projects = dir.join("Projects");
    let song = projects.join("Glass Harbor");
    let newest = write(&song, "Glass Harbor v3.bwproject", b"3");
    let copy = write(&song, "Glass Harbor v1.bwproject", b"same");
    let twin = write(&song, "Glass Harbor v2.bwproject", b"same");
    let auto = write(&song, "auto-backups/Glass Harbor 0.bwproject", b"auto");
    let not_auto = write(&song, "Glass Harbor old.bwproject", b"old");
    let sample = write(&song, "Kit/Kick.wav", b"kick");
    let used = write(&song, "Snare.wav", b"snare");
    let unused = write(&song, "Bounce.wav", b"bounce");
    let readme = write(&song, "readme.txt", b"txt");
    let outside = write(&dir, "Elsewhere/Thing.bwproject", b"x");
    let gone = song.join("Gone.bwproject").to_string_lossy().into_owned();
    let lib = Library {
        roots: vec![projects.to_string_lossy().into_owned()],
        samples: vec![song.join("Kit").to_string_lossy().into_owned()],
        keep: [key(&newest)].into(),
    };
    let in_use = |name: &str| name.eq_ignore_ascii_case("snare.wav");
    let guard =
        Guard { project_roots: &lib.roots, sample_folders: &lib.samples, keep: &lib.keep, sample_in_use: &in_use };

    let picked = [
        line(CompactKind::ThinnedSave, &newest),
        copy_of(&copy, &twin),
        line(CompactKind::Backup, &auto),
        line(CompactKind::Backup, &auto),
        line(CompactKind::Backup, &not_auto),
        line(CompactKind::UnusedAudio, &sample),
        line(CompactKind::UnusedAudio, &used),
        line(CompactKind::UnusedAudio, &unused),
        line(CompactKind::UnusedAudio, &readme),
        line(CompactKind::ThinnedSave, &readme),
        line(CompactKind::ThinnedSave, &outside),
        line(CompactKind::ThinnedSave, &gone),
        line(CompactKind::Cache, &song.to_string_lossy()),
        line(CompactKind::Cache, &lib.roots[0]),
    ];
    let plan = preview(&picked, &guard);
    let going: Vec<&str> = plan.lines.iter().map(|l| l.item.path.as_str()).collect();
    assert_eq!(going, [copy.as_str(), auto.as_str(), unused.as_str()]);
    assert_eq!(plan.bytes(), 4 + 4 + 6);
    let refused: Vec<(&str, Refused)> = plan.refused.iter().map(|(i, why)| (i.path.as_str(), *why)).collect();
    let song_folder = song.to_string_lossy();
    assert_eq!(
        refused,
        [
            (newest.as_str(), Refused::KeptSave),
            (not_auto.as_str(), Refused::NotAnAutosave),
            (sample.as_str(), Refused::InSampleFolder),
            (used.as_str(), Refused::SampleInUse),
            (readme.as_str(), Refused::NotAudio),
            (outside.as_str(), Refused::OutsideProjects),
            (gone.as_str(), Refused::Gone),
            // The song's folder holds the sample folder, and isn't a cache anyway.
            (&*song_folder, Refused::InSampleFolder),
            (lib.roots[0].as_str(), Refused::ProjectFolder),
        ]
    );
    // Nothing moved.
    for p in [&newest, &copy, &twin, &auto, &not_auto, &sample, &used, &unused, &readme, &outside] {
        assert!(Path::new(p).exists(), "{p}");
    }
}

#[test]
fn a_copy_stays_when_its_twin_goes_too_or_has_changed() {
    let dir = fixtures("mo-space-clean-twins");
    let projects = dir.join("Projects");
    let a = write(&projects, "Song/A.flp", b"same");
    let b = write(&projects, "Song/B.flp", b"same");
    let c = write(&projects, "Song/C.flp", b"same");
    let lib =
        Library { roots: vec![projects.to_string_lossy().into_owned()], samples: Vec::new(), keep: HashSet::new() };
    let guard = Guard {
        project_roots: &lib.roots,
        sample_folders: &lib.samples,
        keep: &lib.keep,
        sample_in_use: &no_sample_in_use,
    };

    // A copy of B, and B (picked as thinned) going too: A stays, so what they held isn't lost.
    let plan = preview(&[copy_of(&a, &b), line(CompactKind::ThinnedSave, &b)], &guard);
    assert_eq!(plan.lines.len(), 1);
    assert_eq!(plan.refused, [(copy_of(&a, &b), Refused::TwinGoesToo)]);

    // C is a copy of B, but B changes before the clean-up runs: C stays.
    let plan = preview(&[copy_of(&c, &b)], &guard);
    assert_eq!(plan.lines.len(), 1);
    fs::write(&b, b"SAME").unwrap();
    let bin = FolderBin::new(dir.join("bin"));
    let done = apply(&plan, &guard, &bin, &AtomicBool::new(false));
    assert!(done.removed.is_empty());
    assert_eq!(done.problems, [Problem { path: c.clone(), why: Refused::NotACopyNow.text().into() }]);
    assert!(Path::new(&c).exists());
}

#[test]
fn removing_goes_to_the_bin_and_undo_puts_it_back() {
    let dir = fixtures("mo-space-clean-undo");
    let projects = dir.join("Projects");
    let song = projects.join("Slow Orbit");
    write(&song, "Slow Orbit.song", b"song");
    let cache = song.join("Cache");
    write(&cache, "a.peak", b"peak");
    let inside_cache = write(&cache, "b.peak", b"peak2");
    let auto = write(&song, "History/Slow Orbit (1).song", b"old");
    let changing = write(&song, "History/Slow Orbit (2).song", b"old");
    let lib =
        Library { roots: vec![projects.to_string_lossy().into_owned()], samples: Vec::new(), keep: HashSet::new() };
    let guard = Guard {
        project_roots: &lib.roots,
        sample_folders: &lib.samples,
        keep: &lib.keep,
        sample_in_use: &no_sample_in_use,
    };
    let cache_line = CompactItem { bytes: 9, ..line(CompactKind::Cache, &cache.to_string_lossy()) };
    let plan = preview(
        &[
            cache_line,
            // A file in the cache isn't a cache folder of its own.
            line(CompactKind::Cache, &inside_cache),
            line(CompactKind::Backup, &auto),
            line(CompactKind::Backup, &changing),
        ],
        &guard,
    );
    assert_eq!(plan.lines.len(), 3);
    assert_eq!(plan.refused, [(line(CompactKind::Cache, &inside_cache), Refused::NotACache)]);
    fs::write(&changing, b"newer and longer").unwrap();

    let bin = FolderBin::new(dir.join("bin"));
    let done = apply(&plan, &guard, &bin, &AtomicBool::new(false));
    assert_eq!(done.removed.len(), 2);
    assert_eq!(done.bytes(), 9 + 3);
    assert_eq!(done.problems, [Problem { path: changing.clone(), why: Refused::Changed.text().into() }]);
    assert!(!cache.exists() && !Path::new(&auto).exists() && Path::new(&changing).exists());

    // What was removed is kept by the app as it is, to undo after a restart.
    let kept: mo_space::clean::Done = serde_json::from_str(&serde_json::to_string(&done).unwrap()).unwrap();
    assert_eq!(kept, done);

    // Something new has the autosave's name now: it stays in the bin; the cache comes back.
    write(&song, "History/Slow Orbit (1).song", b"new");
    let problems = undo(&kept, &bin);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].path, auto);
    assert!(Path::new(&inside_cache).exists());
    assert_eq!(fs::read(&auto).unwrap(), b"new");
}

#[test]
fn each_kind_of_line_is_only_ever_what_it_says() {
    let dir = fixtures("mo-space-clean-kinds");
    let projects = dir.join("Projects");
    write(&projects, "Slow Orbit/Slow Orbit.song", b"song");
    write(&projects, "Slow Orbit/Cache/a.peak", b"peak");
    write(&projects, "Slow Orbit/Audio/Take 1.wav", b"take");
    write(&projects, "No Song/Cache/a.peak", b"peak");
    write(&projects, "Third/Third.song", b"song");
    let lone = write(&projects, "Third/Cache", b"not a folder");
    let kick = write(&projects, "Beat/Kick.wav", b"kick");
    let kick_copy = write(&projects, "Beat/Kick copy.wav", b"kick");
    let notes = write(&projects, "Beat/notes.txt", b"notes");
    let vocal = write(&projects, "Beat/Backup/Vocal take.wav", b"vocal");
    let lib =
        Library { roots: vec![projects.to_string_lossy().into_owned()], samples: Vec::new(), keep: HashSet::new() };
    let guard = Guard {
        project_roots: &lib.roots,
        sample_folders: &lib.samples,
        keep: &lib.keep,
        sample_in_use: &no_sample_in_use,
    };
    let folder = |rel: &str| line(CompactKind::Cache, &projects.join(rel).to_string_lossy());
    let picked = [
        // Studio One's Cache folder, beside its song: the one that goes.
        folder("Slow Orbit/Cache"),
        // Recorded takes beside a song, a Cache folder with no song beside it, and a file called Cache.
        folder("Slow Orbit/Audio"),
        folder("No Song/Cache"),
        line(CompactKind::Cache, &lone),
        // Audio the same as other audio isn't a save, nor are notes; audio in a Backup folder isn't an autosave.
        copy_of(&kick_copy, &kick),
        line(CompactKind::ThinnedSave, &notes),
        line(CompactKind::Backup, &vocal),
    ];
    let plan = preview(&picked, &guard);
    let going: Vec<&str> = plan.lines.iter().map(|l| l.item.path.as_str()).collect();
    assert_eq!(going, [picked[0].path.as_str()]);
    let refused: Vec<(&str, Refused)> = plan.refused.iter().map(|(i, why)| (i.path.as_str(), *why)).collect();
    assert_eq!(
        refused,
        [
            (picked[1].path.as_str(), Refused::NotACache),
            (picked[2].path.as_str(), Refused::NotACache),
            (lone.as_str(), Refused::NotACache),
            (kick_copy.as_str(), Refused::NotASave),
            (notes.as_str(), Refused::NotASave),
            (vocal.as_str(), Refused::NotAnAutosave),
        ]
    );
}

#[test]
fn a_file_written_again_at_the_same_size_after_the_preview_stays() {
    let dir = fixtures("mo-space-clean-same-size");
    let projects = dir.join("Projects");
    let bounce = write(&projects, "Song/Bounce.wav", b"first mix");
    let lib =
        Library { roots: vec![projects.to_string_lossy().into_owned()], samples: Vec::new(), keep: HashSet::new() };
    let guard = Guard {
        project_roots: &lib.roots,
        sample_folders: &lib.samples,
        keep: &lib.keep,
        sample_in_use: &no_sample_in_use,
    };
    // Unused audio is listed without a time, so only the look the preview took can tell.
    let plan = preview(&[line(CompactKind::UnusedAudio, &bounce)], &guard);
    assert_eq!(plan.lines.len(), 1);
    // Bounced again after the preview: the same length, a later time.
    fs::write(&bounce, b"other mix").unwrap();
    let later = SystemTime::now() + Duration::from_secs(3600);
    fs::File::options().write(true).open(&bounce).unwrap().set_modified(later).unwrap();
    let bin = FolderBin::new(dir.join("bin"));
    let done = apply(&plan, &guard, &bin, &AtomicBool::new(false));
    assert!(done.removed.is_empty());
    assert_eq!(done.problems, [Problem { path: bounce.clone(), why: Refused::Changed.text().into() }]);
    assert_eq!(fs::read(&bounce).unwrap(), b"other mix");
}

#[cfg(unix)]
#[test]
fn a_line_on_a_network_drive_is_refused_at_the_preview() {
    let dir = fixtures("mo-space-clean-network");
    let projects = dir.join("Projects");
    let auto = write(&projects, "Song/Backup/Song (autosave).flp", b"auto");
    // Two slashes in front: a network path to Linux (\\server on Windows), though here it reaches the same file.
    let roots = vec![format!("/{}", projects.to_string_lossy())];
    let keep = HashSet::new();
    let guard = Guard { project_roots: &roots, sample_folders: &[], keep: &keep, sample_in_use: &no_sample_in_use };
    let network = line(CompactKind::Backup, &format!("/{auto}"));
    let plan = preview(std::slice::from_ref(&network), &guard);
    assert!(plan.lines.is_empty());
    assert_eq!(plan.refused, [(network, Refused::NetworkDrive)]);
}

#[test]
fn a_folder_swapped_for_a_link_after_the_preview_stays_and_so_does_where_it_leads() {
    let dir = fixtures("mo-space-clean-swapped");
    let projects = dir.join("Projects");
    write(&projects, "Slow Orbit/Slow Orbit.song", b"song");
    write(&projects, "Slow Orbit/Cache/a.peak", b"peak");
    write(&dir, "Elsewhere/Cache/a.peak", b"peak");
    let lib =
        Library { roots: vec![projects.to_string_lossy().into_owned()], samples: Vec::new(), keep: HashSet::new() };
    let guard = Guard {
        project_roots: &lib.roots,
        sample_folders: &lib.samples,
        keep: &lib.keep,
        sample_in_use: &no_sample_in_use,
    };
    let cache = projects.join("Slow Orbit/Cache");
    let plan = preview(&[line(CompactKind::Cache, &cache.to_string_lossy())], &guard);
    assert_eq!(plan.lines.len(), 1);
    // After the preview, the Cache folder is swapped for a link to a copy just like it: the same name, size and time.
    let time = fs::metadata(cache.join("a.peak")).unwrap().modified().unwrap();
    fs::File::options().write(true).open(dir.join("Elsewhere/Cache/a.peak")).unwrap().set_modified(time).unwrap();
    fs::remove_dir_all(&cache).unwrap();
    folder_link(&dir.join("Elsewhere/Cache"), &cache);
    let bin = FolderBin::new(dir.join("bin"));
    let done = apply(&plan, &guard, &bin, &AtomicBool::new(false));
    assert!(done.removed.is_empty());
    assert_eq!(done.problems[0].why, Refused::Link.text());
    assert!(dir.join("Elsewhere/Cache/a.peak").exists());
}

#[test]
fn a_project_folder_called_backup_doesnt_make_its_songs_autosaves() {
    let dir = fixtures("mo-space-clean-backup-root");
    let root = dir.join("Backup");
    let song = write(&root, "Song.flp", b"song");
    let real = write(&root, "Song/Backup/Song (autosave).flp", b"auto");
    let named = write(&root, "Song (overwritten at 2213h05).flp", b"auto");
    let lib = Library { roots: vec![root.to_string_lossy().into_owned()], samples: Vec::new(), keep: HashSet::new() };
    let guard = Guard {
        project_roots: &lib.roots,
        sample_folders: &lib.samples,
        keep: &lib.keep,
        sample_in_use: &no_sample_in_use,
    };
    let plan = preview(
        &[line(CompactKind::Backup, &song), line(CompactKind::Backup, &real), line(CompactKind::Backup, &named)],
        &guard,
    );
    let going: Vec<&str> = plan.lines.iter().map(|l| l.item.path.as_str()).collect();
    assert_eq!(going, [real.as_str(), named.as_str()]);
    assert_eq!(plan.refused[0].1, Refused::NotAnAutosave);
}

#[cfg(unix)]
#[test]
fn links_are_never_followed_or_removed() {
    let dir = fixtures("mo-space-clean-links");
    let projects = dir.join("Projects");
    let real = write(&dir, "Samples/Kick.wav", b"kick");
    fs::create_dir_all(&projects).unwrap();
    let link = projects.join("Kick.wav");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let lib =
        Library { roots: vec![projects.to_string_lossy().into_owned()], samples: Vec::new(), keep: HashSet::new() };
    let guard = Guard {
        project_roots: &lib.roots,
        sample_folders: &lib.samples,
        keep: &lib.keep,
        sample_in_use: &no_sample_in_use,
    };
    let plan = preview(&[line(CompactKind::UnusedAudio, &link.to_string_lossy())], &guard);
    assert_eq!(plan.refused[0].1, Refused::Link);
    // And a size walk doesn't count what a link points to.
    let measured = mo_space::size::measure(&projects, &AtomicBool::new(false)).unwrap();
    assert_eq!(measured, mo_space::Measured::default());
}

#[cfg(unix)]
#[test]
fn a_file_with_two_names_is_measured_once() {
    let dir = fixtures("mo-space-clean-hard-link");
    let song = dir.join("Song");
    write(&song, "Audio/Take 1.wav", b"take one");
    write(&song, "Audio/Take 2.wav", b"two");
    // The same file on disk under a second name (a hard link), as some backup and tidy-up tools leave.
    fs::hard_link(song.join("Audio/Take 1.wav"), song.join("Bounce.wav")).unwrap();
    let measured = mo_space::size::measure(&song, &AtomicBool::new(false)).unwrap();
    assert_eq!(measured, mo_space::Measured { bytes: 11, sound_bytes: 11, files: 2 });
}

#[test]
fn a_link_into_a_sample_folder_cant_lead_a_clean_up_there() {
    let dir = fixtures("mo-space-clean-through-link");
    let projects = dir.join("Projects");
    let loop_wav = write(&dir, "Samples/Loops/Song/Loop.wav", b"loop");
    fs::create_dir_all(&projects).unwrap();
    folder_link(&dir.join("Samples/Loops"), &projects.join("Shared"));
    let through = projects.join("Shared/Song/Loop.wav").to_string_lossy().into_owned();
    // Even with no sample folders given, the link leads out of the project folder.
    let lib =
        Library { roots: vec![projects.to_string_lossy().into_owned()], samples: Vec::new(), keep: HashSet::new() };
    let guard = Guard {
        project_roots: &lib.roots,
        sample_folders: &lib.samples,
        keep: &lib.keep,
        sample_in_use: &no_sample_in_use,
    };
    let plan = preview(&[line(CompactKind::UnusedAudio, &through)], &guard);
    assert_eq!(plan.refused[0].1, Refused::ThroughLink);
    // And a sample folder reached by its real path through a link from the project folder is still a sample folder.
    let samples_via_link = vec![projects.join("Shared").to_string_lossy().into_owned()];
    let roots = vec![dir.to_string_lossy().into_owned()];
    let guard = Guard {
        project_roots: &roots,
        sample_folders: &samples_via_link,
        keep: &lib.keep,
        sample_in_use: &no_sample_in_use,
    };
    let plan = preview(&[line(CompactKind::UnusedAudio, &loop_wav)], &guard);
    assert_eq!(plan.refused[0].1, Refused::InSampleFolder);
    assert!(Path::new(&loop_wav).exists());
}

#[cfg(unix)]
#[test]
fn a_copy_whose_twin_is_a_link_to_it_stays() {
    let dir = fixtures("mo-space-clean-link-twin");
    let projects = dir.join("Projects");
    let a = write(&projects, "Song/A.flp", b"same");
    let b = projects.join("Song/B.flp");
    std::os::unix::fs::symlink(&a, &b).unwrap();
    let lib =
        Library { roots: vec![projects.to_string_lossy().into_owned()], samples: Vec::new(), keep: HashSet::new() };
    let guard = Guard {
        project_roots: &lib.roots,
        sample_folders: &lib.samples,
        keep: &lib.keep,
        sample_in_use: &no_sample_in_use,
    };
    let plan = preview(&[copy_of(&a, &b.to_string_lossy())], &guard);
    assert_eq!(plan.refused[0].1, Refused::NotACopyNow);
}

#[test]
fn a_file_written_again_since_it_was_measured_stays() {
    let dir = fixtures("mo-space-clean-measured");
    let projects = dir.join("Projects");
    let auto = write(&projects, "Song/Backup/Song (autosave).flp", b"auto");
    let lib =
        Library { roots: vec![projects.to_string_lossy().into_owned()], samples: Vec::new(), keep: HashSet::new() };
    let guard = Guard {
        project_roots: &lib.roots,
        sample_folders: &lib.samples,
        keep: &lib.keep,
        sample_in_use: &no_sample_in_use,
    };
    let measured = line(CompactKind::Backup, &auto);
    fs::write(&auto, b"autosaved again").unwrap();
    let plan = preview(std::slice::from_ref(&measured), &guard);
    assert_eq!(plan.refused, [(measured, Refused::Changed)]);
    // A line whose time is an hour off is another version too.
    let stale = CompactItem {
        modified: Some(chrono::Local::now().naive_local() - chrono::TimeDelta::hours(1)),
        ..line(CompactKind::Backup, &auto)
    };
    assert_eq!(preview(std::slice::from_ref(&stale), &guard).refused[0].1, Refused::Changed);
}

#[test]
fn paths_compare_whichever_way_they_are_written() {
    assert_eq!(key(r"\\?\C:\Music\Song.flp"), key("c:/music/song.FLP"));
    assert_eq!(key(r"\\?\UNC\nas\music\Song.flp"), key(r"\\NAS\Music\Song.flp"));
    assert_eq!(key("C:/Music/"), key(r"C:\Music"));
}
