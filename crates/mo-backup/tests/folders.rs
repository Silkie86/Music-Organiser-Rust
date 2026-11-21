//! Backing up by folder, on folders made in the test build's own fixtures folder under fixed names.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_backup::folders::{Folder, Mode, from_text, gather, to_text};
use mo_space::size::{Walked, walked};

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(&dir).unwrap()
}

#[test]
fn folders_are_walked_once_and_keep_their_own_choice() {
    let root = fixtures("backup-folders");
    let music = root.join("Music");
    let bounces = music.join("Bounces");
    fs::create_dir_all(&bounces).unwrap();
    fs::write(music.join("Song.flp"), b"song").unwrap();
    fs::write(bounces.join("Mix.wav"), b"mix").unwrap();
    let list = vec![
        Folder { path: music.clone(), mode: Mode::Slimmed },
        Folder { path: bounces.clone(), mode: Mode::Whole },
        Folder { path: root.join("Unplugged"), mode: Mode::Whole },
    ];
    let got = gather(&list, &root.join("Backup"), &AtomicBool::new(false)).unwrap();
    assert_eq!(got.files.len(), 2, "the inner folder isn't walked twice");
    assert_eq!(got.missing, vec![root.join("Unplugged")]);
}

/// A folder link, as Windows has them (a junction) or a symbolic link elsewhere.
fn folder_link(target: &Path, at: &Path) {
    #[cfg(windows)]
    junction::create(target, at).unwrap();
    #[cfg(not(windows))]
    std::os::unix::fs::symlink(target, at).unwrap();
}

#[test]
fn a_listed_folder_that_is_a_link_is_walked_where_it_really_is() {
    // Review 2026-11-03: a listed junction (Space's folder move leaves one) was walked as empty and not reported.
    let root = fixtures("backup-folders-link");
    let real = root.join("K").join("FL Projects");
    fs::create_dir_all(real.join("Tune")).unwrap();
    fs::write(real.join("Tune").join("Tune.flp"), b"song").unwrap();
    let linked = root.join("Projects");
    folder_link(&real, &linked);
    let gone = root.join("Gone");
    folder_link(&root.join("Nowhere"), &gone);
    let list =
        vec![Folder { path: linked.clone(), mode: Mode::Slimmed }, Folder { path: gone.clone(), mode: Mode::Slimmed }];
    let got = gather(&list, &root.join("Backup"), &AtomicBool::new(false)).unwrap();
    let paths: Vec<&Path> = got.files.iter().map(|f| f.path.as_path()).collect();
    assert_eq!(paths, vec![linked.join("Tune").join("Tune.flp")], "the listed spelling, as projects name it");
    assert_eq!(got.missing, vec![gone], "a link to nowhere is missing");
}

#[test]
fn a_listed_folder_reached_through_a_link_is_walked_too() {
    // The outer folder's walk leaves the link alone, so the folder inside it isn't covered by that walk.
    let root = fixtures("backup-folders-through-link");
    let docs = root.join("Documents");
    let moved = root.join("D").join("Native Instruments");
    fs::create_dir_all(moved.join("Kontakt")).unwrap();
    fs::create_dir_all(&docs).unwrap();
    fs::write(docs.join("Notes.txt"), b"notes").unwrap();
    fs::write(moved.join("Kontakt").join("Pad.nki"), b"pad").unwrap();
    folder_link(&moved, &docs.join("Native Instruments"));
    let kontakt = docs.join("Native Instruments").join("Kontakt");
    let list =
        vec![Folder { path: docs.clone(), mode: Mode::Slimmed }, Folder { path: kontakt.clone(), mode: Mode::Whole }];
    let got = gather(&list, &root.join("Backup"), &AtomicBool::new(false)).unwrap();
    let mut paths: Vec<PathBuf> = got.files.iter().map(|f| f.path.clone()).collect();
    paths.sort();
    let mut want = vec![docs.join("Notes.txt"), kontakt.join("Pad.nki")];
    want.sort();
    assert_eq!(paths, want);
    assert!(got.missing.is_empty());

    // The same folder listed twice, once through the link and once where it is, is walked once.
    let list = vec![
        Folder { path: kontakt.clone(), mode: Mode::Whole },
        Folder { path: moved.join("Kontakt"), mode: Mode::Whole },
    ];
    let got = gather(&list, &root.join("Backup"), &AtomicBool::new(false)).unwrap();
    assert_eq!(got.files.len(), 1);
    assert_eq!(got.files[0].path, kontakt.join("Pad.nki"), "named by the first line");

    // The outer folder listed where it really is: the inner one's files keep the inner one's spelling.
    let list =
        vec![Folder { path: root.join("D"), mode: Mode::Slimmed }, Folder { path: kontakt.clone(), mode: Mode::Whole }];
    let got = gather(&list, &root.join("Backup"), &AtomicBool::new(false)).unwrap();
    assert_eq!(got.files.len(), 1, "walked once");
    assert_eq!(got.files[0].path, kontakt.join("Pad.nki"));
}

#[test]
fn the_backup_is_left_out_when_a_listed_link_leads_into_it() {
    let root = fixtures("backup-folders-backup-inside");
    let real = root.join("E").join("Projects");
    fs::create_dir_all(real.join("Backup").join("E")).unwrap();
    fs::write(real.join("Song.flp"), b"song").unwrap();
    fs::write(real.join("Backup").join("E").join("Song.flp"), b"song").unwrap();
    let linked = root.join("Projects");
    folder_link(&real, &linked);
    let list = vec![Folder { path: linked.clone(), mode: Mode::Slimmed }];
    let got = gather(&list, &real.join("Backup"), &AtomicBool::new(false)).unwrap();
    let paths: Vec<&Path> = got.files.iter().map(|f| f.path.as_path()).collect();
    assert_eq!(paths, vec![linked.join("Song.flp")]);
}

/// What a walk of a song's folder would find comes from the walk that gathered the files, named as the folder is (the
/// whole-app review of 19 Nov, SB-3: the backup's plan walked every song's folder again), and nothing comes when that
/// walk can't speak for the folder.
#[test]
fn a_folders_files_come_from_the_walk_that_gathered_them() {
    let root = fixtures("backup-in-folders");
    let music = root.join("Music");
    let song = music.join("Song A");
    fs::create_dir_all(song.join("samples")).unwrap();
    fs::create_dir_all(song.join("auto-backups")).unwrap();
    fs::write(song.join("Song A.bwproject"), b"song").unwrap();
    fs::write(song.join("samples").join("Kick.wav"), b"kick").unwrap();
    fs::write(song.join("auto-backups").join("Song A-1.bwproject"), b"older").unwrap();
    // Beside it, a folder whose name starts the same.
    fs::create_dir_all(music.join("Song A 2")).unwrap();
    fs::write(music.join("Song A 2").join("Other.wav"), b"other").unwrap();
    fs::create_dir_all(music.join("Empty")).unwrap();
    let bounces = music.join("Bounces");
    fs::create_dir_all(&bounces).unwrap();
    fs::write(bounces.join("Mix.wav"), b"mix").unwrap();
    let elsewhere = root.join("Elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("Far.wav"), b"far").unwrap();
    folder_link(&elsewhere, &music.join("Linked"));
    // The same folder reached through a link.
    let through = root.join("Music link");
    folder_link(&music, &through);
    let list =
        vec![Folder { path: music.clone(), mode: Mode::Slimmed }, Folder { path: bounces.clone(), mode: Mode::Whole }];
    let stop = AtomicBool::new(false);
    let got = gather(&list, &root.join("Backup"), &stop).unwrap();
    let in_folders = got.in_folders();
    let sorted = |mut w: Walked| {
        w.files.sort_by(|a, b| a.path.cmp(&b.path));
        w.online_only.sort();
        w
    };
    let all = |_: &Path| false;
    let from_gather = |folder: &Path| in_folders.walked(&folder.to_string_lossy(), &all).map(sorted);
    for folder in [&song, &song.join("samples"), &music.join("Song A 2"), &bounces, &through.join("Song A")] {
        assert_eq!(from_gather(folder), Some(sorted(walked(folder, &stop).unwrap())), "{}", folder.display());
    }
    assert_eq!(from_gather(&song).unwrap().files.len(), 3);
    // Nothing for a folder another listed folder is inside, nor one where nothing was found, nor one outside the list
    // (a link to it too), nor one that isn't there.
    for folder in [&music, &root, &music.join("Empty"), &elsewhere, &music.join("Linked"), &music.join("Gone")] {
        assert_eq!(from_gather(folder), None, "{}", folder.display());
    }
    // What's given stops at the first file the test says ends it, that file too, as a walk of the folder would (its
    // own files first, then those in its folders by name: the song's save, then its autosave, then its sample).
    let ends_at = |p: &Path| p.ends_with("Song A-1.bwproject");
    let till = in_folders.walked(&song.to_string_lossy(), &ends_at).unwrap();
    let paths: Vec<PathBuf> = till.files.into_iter().map(|f| f.path).collect();
    assert_eq!(paths, [song.join("Song A.bwproject"), song.join("auto-backups").join("Song A-1.bwproject")]);
    // The same place listed twice: the second spelling is spoken for too, named as it's asked for.
    let twice = vec![
        Folder { path: music.clone(), mode: Mode::Slimmed },
        Folder { path: through.clone(), mode: Mode::Slimmed },
    ];
    let got = gather(&twice, &root.join("Backup"), &stop).unwrap();
    let in_folders = got.in_folders();
    for folder in [&song, &through.join("Song A")] {
        let given = in_folders.walked(&folder.to_string_lossy(), &all).map(sorted);
        assert_eq!(given, Some(sorted(walked(folder, &stop).unwrap())), "{}", folder.display());
    }
    // The backup in a song's folder: what was in it is left out, so that folder isn't spoken for; the others are.
    let got = gather(&list, &song.join("auto-backups"), &stop).unwrap();
    let in_folders = got.in_folders();
    assert_eq!(in_folders.walked(&song.to_string_lossy(), &all), None);
    assert_eq!(in_folders.walked(&music.join("Song A 2").to_string_lossy(), &all).map(|w| w.files.len()), Some(1));
    // A folder in the list reached through a link in a song's folder (one Space moved, listed by its old place): its
    // files are named as if they were in the song's folder, but a walk of that folder doesn't go through the link.
    let stems = song.join("Stems");
    folder_link(&elsewhere, &stems);
    let list =
        vec![Folder { path: music.clone(), mode: Mode::Slimmed }, Folder { path: stems.clone(), mode: Mode::Whole }];
    let got = gather(&list, &root.join("Backup"), &stop).unwrap();
    assert!(got.files.iter().any(|f| f.path == stems.join("Far.wav")), "{:?}", got.files);
    let in_folders = got.in_folders();
    assert_eq!(in_folders.walked(&song.to_string_lossy(), &all), None);
    assert!(in_folders.walked(&music.join("Song A 2").to_string_lossy(), &all).is_some());
}

/// In a folder FL Studio songs share, another song's save beside the song's own ends what's given before the sounds in
/// its folders are gone through, as a walk of it would end (the review of the SB-3 fix: by name alone, a big "Audio"
/// folder came first, for every song in the folder).
#[test]
fn a_folders_own_files_come_before_those_in_its_folders() {
    let root = fixtures("backup-in-folders-shared");
    let music = root.join("Music");
    let shared = music.join("FL shared");
    fs::create_dir_all(shared.join("Audio")).unwrap();
    for name in ["Audio/a.wav", "Audio/b.wav", "Beat.flp", "Other.flp"] {
        fs::write(shared.join(name), name).unwrap();
    }
    let list = vec![Folder { path: music.clone(), mode: Mode::Slimmed }];
    let got = gather(&list, &root.join("Backup"), &AtomicBool::new(false)).unwrap();
    let in_folders = got.in_folders();
    let paths = |w: Walked| -> Vec<PathBuf> { w.files.into_iter().map(|f| f.path).collect() };
    let other = |p: &Path| p.ends_with("Other.flp");
    let till = in_folders.walked(&shared.to_string_lossy(), &other).unwrap();
    assert_eq!(paths(till), [shared.join("Beat.flp"), shared.join("Other.flp")]);
    // Nothing ends it: everything, its own files first.
    let all = in_folders.walked(&shared.to_string_lossy(), &|_: &Path| false).unwrap();
    let audio = shared.join("Audio");
    assert_eq!(
        paths(all),
        [shared.join("Beat.flp"), shared.join("Other.flp"), audio.join("a.wav"), audio.join("b.wav")]
    );
}

#[test]
fn the_list_keeps_through_text() {
    let list = vec![
        Folder { path: PathBuf::from(r"I:\FL Studio Data"), mode: Mode::Slimmed },
        Folder { path: PathBuf::from(r"K:\Bitwig Studio"), mode: Mode::Whole },
    ];
    assert_eq!(from_text(&to_text(&list)), list);
    assert!(from_text("odd line\nwhole\t\n").is_empty());
}
