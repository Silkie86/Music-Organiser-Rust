//! One zip per song, on folders made in the test build's own fixtures folder under fixed names, never a random temp
//! one and never a real folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use mo_backup::pack::{Packed, Packer, SONGS, lists, pack_names};

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(&dir).unwrap()
}

fn write(path: &Path, bytes: &[u8], secs_ago: u64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    set_time(path, secs_ago);
}

fn set_time(path: &Path, secs_ago: u64) {
    let when = SystemTime::now() - Duration::from_secs(secs_ago);
    fs::File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
}

fn names_in(zip: &Path) -> Vec<String> {
    let zip = zip::ZipArchive::new(fs::File::open(zip).unwrap()).unwrap();
    let mut names: Vec<String> = zip.file_names().map(|n| String::from(n.expect("a name"))).collect();
    names.sort();
    names
}

#[test]
fn a_song_packs_once_and_again_only_when_it_changes() {
    let root = fixtures("backup-pack");
    let music = root.join("music");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    let save = music.join("Song/Song.flp");
    let kick = music.join("Samples/Kick.wav");
    let other_kick = music.join("Other/Kick.wav");
    write(&save, &[7u8; 5000], 100);
    write(&kick, &[1u8; 3000], 100);
    write(&other_kick, &[2u8; 3000], 100);
    let files = vec![save.clone(), kick.clone(), other_kick.clone()];
    let stop = AtomicBool::new(false);
    let packer = Packer::new(&backup).unwrap();

    let name = &pack_names(&[("Song", 1)])[0];
    assert!(matches!(packer.pack_song(&files, name, 1, &stop).unwrap(), Packed::Written { .. }));
    let zip_path = backup.join(SONGS).join(name);
    let names = names_in(&zip_path);
    assert_eq!(names.len(), 4, "three files and the list");
    // Two samples with the same name keep their own paths.
    assert!(names.iter().any(|n| n.ends_with("Samples/Kick.wav")));
    assert!(names.iter().any(|n| n.ends_with("Other/Kick.wav")));
    let save_name = names.iter().find(|n| n.ends_with("Song.flp")).unwrap().clone();
    let mut zip = zip::ZipArchive::new(fs::File::open(&zip_path).unwrap()).unwrap();
    let mut entry = zip.by_name(&save_name).unwrap();
    // Each file keeps its date (a zip without one says 1980).
    assert!(entry.last_modified().unwrap().year() >= 2026);
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, vec![7u8; 5000]);
    drop(entry);
    drop(zip);

    assert_eq!(packer.pack_song(&files, name, 1, &stop).unwrap(), Packed::UpToDate { kept: 0 });
    // Only the time moved (the clocks changed on a FAT drive): the bytes are compared, nothing is packed again.
    set_time(&kick, 3700);
    assert_eq!(packer.pack_song(&files, name, 1, &stop).unwrap(), Packed::UpToDate { kept: 0 });
    write(&save, &[8u8; 5000], 10);
    assert!(matches!(packer.pack_song(&files, name, 1, &stop).unwrap(), Packed::Written { .. }));
    // No half-made zip is left.
    assert_eq!(fs::read_dir(backup.join(SONGS)).unwrap().count(), 1);
}

#[test]
fn a_missing_file_leaves_the_old_zip_as_it_was_and_names_the_file() {
    let root = fixtures("backup-pack-missing");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    let save = root.join("music/Song.flp");
    write(&save, b"song", 100);
    let stop = AtomicBool::new(false);
    let packer = Packer::new(&backup).unwrap();
    let name = "Song.zip";
    packer.pack_song(std::slice::from_ref(&save), name, 1, &stop).unwrap();
    let before = fs::read(backup.join(SONGS).join(name)).unwrap();
    let err = packer.pack_song(&[save.clone(), root.join("music/gone.wav")], name, 1, &stop).unwrap_err();
    assert!(err.to_string().contains("gone.wav"), "{err}");
    // An empty list doesn't empty the zip either.
    assert!(packer.pack_song(&[], name, 1, &stop).is_err());
    assert_eq!(fs::read(backup.join(SONGS).join(name)).unwrap(), before);
    assert_eq!(fs::read_dir(backup.join(SONGS)).unwrap().count(), 1);
}

#[test]
fn a_zip_of_another_song_is_never_written_over() {
    let root = fixtures("backup-pack-other");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    let a = root.join("music/A.flp");
    let b = root.join("music/B.flp");
    write(&a, b"a", 100);
    write(&b, b"b", 100);
    let stop = AtomicBool::new(false);
    let packer = Packer::new(&backup).unwrap();
    packer.pack_song(std::slice::from_ref(&a), "Beat.zip", 1, &stop).unwrap();
    let err = packer.pack_song(std::slice::from_ref(&b), "Beat.zip", 2, &stop).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(names_in(&backup.join(SONGS).join("Beat.zip")).iter().any(|n| n.ends_with("A.flp")));
    // Names that would lead outside the Songs folder are refused.
    for bad in ["../x.zip", "C:x.zip", "a/b.zip", "song.txt"] {
        assert!(packer.pack_song(std::slice::from_ref(&a), bad, 1, &stop).is_err(), "{bad}");
    }
}

#[test]
fn zip_names_are_safe_and_never_shared() {
    let names = pack_names(&[("Beat?", 5), ("Beat*", 3), ("beat_", 9), ("con", 1), ("", 2), (&"x".repeat(300), 4)]);
    assert_eq!(names[1], "Beat_.zip", "the older song keeps the plain name");
    assert_eq!(names[0], "Beat_ (2).zip");
    assert_eq!(names[2], "beat_ (3).zip");
    assert_eq!(names[3], "_con.zip");
    assert_eq!(names[4], "untitled.zip");
    assert!(names[5].len() < 200);
}

fn bytes_in(zip: &Path, ends: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(fs::File::open(zip).unwrap()).unwrap();
    let name =
        zip.file_names().map(|n| String::from(n.expect("a name"))).find(|n| n.ends_with(ends)).expect("in the zip");
    let mut bytes = Vec::new();
    zip.by_name(&name).unwrap().read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn a_file_gone_from_your_folders_stays_in_the_songs_zip() {
    let root = fixtures("backup-pack-gone");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    let save = root.join("music/Song/Song.flp");
    let kick = root.join("music/Samples/Kick.wav");
    let snare = root.join("music/Samples/Snare.wav");
    write(&save, &[7u8; 500], 100);
    write(&kick, &[1u8; 3000], 100);
    write(&snare, &[2u8; 3000], 100);
    let stop = AtomicBool::new(false);
    let packer = Packer::new(&backup).unwrap();
    let zip = backup.join(SONGS).join("Song.zip");
    let packed = packer.pack_song(&[save.clone(), kick.clone(), snare.clone()], "Song.zip", 1, &stop).unwrap();
    assert!(matches!(packed, Packed::Written { kept: 0, .. }), "{packed:?}");

    // The snare is deleted: the song no longer lists it, and its zip is up to date with the snare still in it.
    fs::remove_file(&snare).unwrap();
    let now = [save.clone(), kick.clone()];
    assert_eq!(packer.pack_song(&now, "Song.zip", 1, &stop).unwrap(), Packed::UpToDate { kept: 1 });
    // The song changes: packed again, and the snare goes along, its bytes as they were.
    write(&save, &[8u8; 500], 10);
    let packed = packer.pack_song(&now, "Song.zip", 1, &stop).unwrap();
    assert!(matches!(&packed, Packed::Written { kept: 1, damaged, .. } if damaged.is_empty()), "{packed:?}");
    assert_eq!(bytes_in(&zip, "Samples/Snare.wav"), vec![2u8; 3000]);
    assert_eq!(bytes_in(&zip, "Song/Song.flp"), vec![8u8; 500]);

    // The kick is no longer the song's, but it's still in your folders: it goes from the zip (you still have it).
    write(&save, &[9u8; 500], 5);
    let packed = packer.pack_song(std::slice::from_ref(&save), "Song.zip", 1, &stop).unwrap();
    assert!(matches!(packed, Packed::Written { kept: 1, .. }), "{packed:?}");
    let names = names_in(&zip);
    assert_eq!(names.len(), 3, "the save, the kept snare and the list: {names:?}");
    assert!(names.iter().any(|n| n.ends_with("Samples/Snare.wav")));
    assert!(!names.iter().any(|n| n.ends_with("Samples/Kick.wav")));

    // Every song zip's list, as the warning about gone files reads them.
    let found = lists(&backup, &stop).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].zip, zip);
    let mut inside: Vec<(u64, &str)> = found[0].files.iter().map(|(size, _, at)| (*size, at.as_str())).collect();
    inside.sort();
    assert_eq!(inside.len(), 2);
    assert!(inside[0].0 == 500 && inside[0].1.ends_with("Song/Song.flp"), "{inside:?}");
    assert!(inside[1].0 == 3000 && inside[1].1.ends_with("Samples/Snare.wav"), "{inside:?}");
}

#[test]
fn a_kept_file_damaged_in_the_zip_before_is_left_out_and_named() {
    let root = fixtures("backup-pack-damaged");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    let save = root.join("music/Song/Song.flp");
    let snare = root.join("music/Samples/Snare.wav");
    write(&save, &[7u8; 500], 100);
    write(&snare, &[2u8; 3000], 100);
    let stop = AtomicBool::new(false);
    let packer = Packer::new(&backup).unwrap();
    let zip = backup.join(SONGS).join("Song.zip");
    packer.pack_song(&[save.clone(), snare.clone()], "Song.zip", 1, &stop).unwrap();
    fs::remove_file(&snare).unwrap();
    // A sound is stored as it is: one byte of it changed in the zip is damage its check number finds.
    let mut bytes = fs::read(&zip).unwrap();
    let at = bytes.windows(3000).position(|w| w.iter().all(|&b| b == 2)).expect("the snare's bytes");
    bytes[at + 1500] = 3;
    fs::write(&zip, &bytes).unwrap();

    write(&save, &[8u8; 500], 10);
    let packed = packer.pack_song(std::slice::from_ref(&save), "Song.zip", 1, &stop).unwrap();
    let Packed::Written { kept, damaged, .. } = packed else { panic!("packed again: {packed:?}") };
    assert_eq!(kept, 0);
    assert_eq!(damaged.len(), 1);
    assert!(damaged[0].ends_with("Samples/Snare.wav"), "{damaged:?}");
    // The new zip is whole: the save and the list, read back to the end.
    assert_eq!(names_in(&zip).len(), 2);
    assert_eq!(bytes_in(&zip, "Song/Song.flp"), vec![8u8; 500]);
    // And up to date the next time.
    assert_eq!(
        packer.pack_song(std::slice::from_ref(&save), "Song.zip", 1, &stop).unwrap(),
        Packed::UpToDate { kept: 0 }
    );
}
