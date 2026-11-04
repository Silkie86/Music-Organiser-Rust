//! One zip per song, on folders made in the test build's own fixtures folder under fixed names, never a random temp
//! one and never a real folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use mo_backup::pack::{Damaged, Packed, Packer, SONGS, lists, owners, pack_names};

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

fn read_in(zip: &Path, inside: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(fs::File::open(zip).unwrap()).unwrap();
    let mut bytes = Vec::new();
    zip.by_name(inside).unwrap().read_to_end(&mut bytes).unwrap();
    bytes
}

fn list_in(zip: &Path) -> String {
    String::from_utf8(read_in(zip, "pack.txt")).unwrap()
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

    let name = &pack_names(&[("Song", 1)], &HashMap::new())[0];
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
    // Only the time moved (the clocks changed on a FAT drive): the bytes are compared, and the zip is made again from
    // its own bytes so its list carries the new time (the next run doesn't read every file again).
    set_time(&kick, 3700);
    assert_eq!(packer.pack_song(&files, name, 1, &stop).unwrap(), Packed::UpToDate { kept: 0 });
    let kick_time = fs::metadata(&kick).unwrap().modified().unwrap().duration_since(SystemTime::UNIX_EPOCH).unwrap();
    let list = list_in(&zip_path);
    assert!(list.contains(&format!("3000\t{}\t", kick_time.as_nanos())), "{list}");
    assert_eq!(read_in(&zip_path, &save_name), vec![7u8; 5000]);
    assert_eq!(fs::read_dir(backup.join(SONGS)).unwrap().count(), 1);
    assert_eq!(packer.pack_song(&files, name, 1, &stop).unwrap(), Packed::UpToDate { kept: 0 });
    assert_eq!(list_in(&zip_path), list);
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
    let songs = [("Beat?", 5), ("Beat*", 3), ("beat_", 9), ("con", 1), ("", 2), (&"x".repeat(300), 4)];
    let names = pack_names(&songs, &HashMap::new());
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

/// A zip's bytes for one entry as they're stored (a sound is stored as it is, so these are its own bytes).
fn stored_in(zip: &Path, ends: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(fs::File::open(zip).unwrap()).unwrap();
    let names: Vec<String> = zip.file_names().map(|n| String::from(n.expect("a name"))).collect();
    let index = zip.index_for_name(names.iter().find(|n| n.ends_with(ends)).expect("in the zip")).unwrap();
    let mut bytes = Vec::new();
    zip.by_index_raw(index).unwrap().read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn a_kept_file_damaged_in_the_zip_before_is_kept_as_it_was_and_named() {
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
    let damaged_snare = stored_in(&zip, "Samples/Snare.wav");

    write(&save, &[8u8; 500], 10);
    let packed = packer.pack_song(std::slice::from_ref(&save), "Song.zip", 1, &stop).unwrap();
    let Packed::Written { kept, damaged, .. } = packed else { panic!("packed again: {packed:?}") };
    assert_eq!(kept, 1);
    assert_eq!(damaged.len(), 1);
    assert!(damaged[0].kept && damaged[0].inside.ends_with("Samples/Snare.wav"), "{damaged:?}");
    // Its bytes are just as they were (nothing more lost), and the rest of the zip is whole.
    assert_eq!(stored_in(&zip, "Samples/Snare.wav"), damaged_snare);
    assert_eq!(names_in(&zip).len(), 3);
    assert_eq!(bytes_in(&zip, "Song/Song.flp"), vec![8u8; 500]);
    // And up to date the next time.
    assert_eq!(
        packer.pack_song(std::slice::from_ref(&save), "Song.zip", 1, &stop).unwrap(),
        Packed::UpToDate { kept: 1 }
    );
}

#[test]
fn a_kept_file_its_zip_doesnt_have_is_named_and_left_off_the_list() {
    let root = fixtures("backup-pack-not-in-zip");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    let save = root.join("music/Song/Song.flp");
    let snare = root.join("music/Samples/Snare.wav");
    write(&save, &[7u8; 500], 100);
    write(&snare, &[2u8; 3000], 100);
    let stop = AtomicBool::new(false);
    let packer = Packer::new(&backup).unwrap();
    let zip_path = backup.join(SONGS).join("Song.zip");
    packer.pack_song(&[save.clone(), snare.clone()], "Song.zip", 1, &stop).unwrap();
    fs::remove_file(&snare).unwrap();
    // The zip loses the snare, while its list still names it.
    let mut old = zip::ZipArchive::new(fs::File::open(&zip_path).unwrap()).unwrap();
    let mut out = zip::ZipWriter::new(fs::File::create(root.join("cut.zip")).unwrap());
    for i in 0..old.len() {
        let entry = old.by_index_raw(i).unwrap();
        if !entry.name().is_ok_and(|n| n.ends_with("Snare.wav")) {
            out.raw_copy_file(entry).unwrap();
        }
    }
    out.finish().unwrap();
    drop(old);
    fs::rename(root.join("cut.zip"), &zip_path).unwrap();

    write(&save, &[8u8; 500], 10);
    let packed = packer.pack_song(std::slice::from_ref(&save), "Song.zip", 1, &stop).unwrap();
    let Packed::Written { kept, damaged, .. } = packed else { panic!("packed again: {packed:?}") };
    assert_eq!(kept, 0);
    assert!(matches!(damaged.as_slice(), [Damaged { kept: false, inside }] if inside.ends_with("Snare.wav")));
    let found = lists(&backup, &stop).unwrap();
    assert_eq!(found[0].files.len(), 1, "only the save is listed now");
}

#[test]
fn a_zip_whose_list_cant_be_read_is_never_packed_over() {
    let root = fixtures("backup-pack-unreadable");
    let backup = root.join("backup");
    let songs = backup.join(SONGS);
    fs::create_dir_all(&songs).unwrap();
    let save = root.join("music/Song.flp");
    write(&save, b"song", 100);
    let stop = AtomicBool::new(false);
    let packer = Packer::new(&backup).unwrap();
    // Not a whole zip (damaged, cut short), and a zip that isn't one of the app's (no list in it).
    fs::write(songs.join("Broken.zip"), b"PK\x03\x04 not much of a zip").unwrap();
    let mut theirs = zip::ZipWriter::new(fs::File::create(songs.join("Theirs.zip")).unwrap());
    theirs.start_file("notes.txt", zip::write::SimpleFileOptions::default()).unwrap();
    theirs.write_all(b"mine").unwrap();
    theirs.finish().unwrap();
    for name in ["Broken.zip", "Theirs.zip"] {
        let before = fs::read(songs.join(name)).unwrap();
        let err = packer.pack_song(std::slice::from_ref(&save), name, 1, &stop).unwrap_err();
        assert!(err.to_string().contains("left as it is"), "{err}");
        assert_eq!(fs::read(songs.join(name)).unwrap(), before, "{name} is as it was");
    }
    // Neither is listed for the warning; both keep their names from songs.
    let found = lists(&backup, &stop).unwrap();
    assert!(found.iter().all(|l| l.song.is_none() && l.files.is_empty()));
    let names = pack_names(&[("Broken", 1), ("Theirs", 2)], &owners(&found));
    assert_eq!(names, ["Broken (2).zip", "Theirs (2).zip"]);
}

#[test]
fn a_deleted_songs_zip_keeps_its_name_to_itself() {
    let root = fixtures("backup-pack-deleted-song");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    let a = root.join("music/A/Beat.flp");
    let b = root.join("music/B/Beat.flp");
    write(&a, b"a", 100);
    write(&b, b"b", 100);
    let stop = AtomicBool::new(false);
    let packer = Packer::new(&backup).unwrap();
    let names = pack_names(&[("Beat", 1), ("Beat", 2)], &owners(&lists(&backup, &stop).unwrap()));
    assert_eq!(names, ["Beat.zip", "Beat (2).zip"]);
    packer.pack_song(std::slice::from_ref(&a), &names[0], 1, &stop).unwrap();
    packer.pack_song(std::slice::from_ref(&b), &names[1], 2, &stop).unwrap();

    // The first song's folder is deleted: its zip stays, and the other song keeps its own zip's name.
    fs::remove_dir_all(root.join("music/A")).unwrap();
    let names = pack_names(&[("Beat", 2)], &owners(&lists(&backup, &stop).unwrap()));
    assert_eq!(names, ["Beat (2).zip"]);
    assert_eq!(packer.pack_song(std::slice::from_ref(&b), &names[0], 2, &stop).unwrap(), Packed::UpToDate { kept: 0 });
    // A new song of that title gets a name of its own too.
    let names = pack_names(&[("Beat", 2), ("Beat", 3)], &owners(&lists(&backup, &stop).unwrap()));
    assert_eq!(names, ["Beat (2).zip", "Beat (3).zip"]);
}

#[test]
fn a_kept_file_in_the_large_zip_form_copies_over_whole() {
    // A file of 4 GB or so is written in a zip's large-file form; one that small is written so here, to try it.
    let root = fixtures("backup-pack-large-form");
    let backup = root.join("backup");
    let songs = backup.join(SONGS);
    fs::create_dir_all(&songs).unwrap();
    let save = root.join("music/Song/Song.flp");
    let stem = root.join("music/Stems/Take.wav");
    write(&save, &[7u8; 500], 100);
    write(&stem, &[5u8; 4000], 100);
    let stop = AtomicBool::new(false);
    let packer = Packer::new(&backup).unwrap();
    packer.pack_song(&[save.clone(), stem.clone()], "Song.zip", 1, &stop).unwrap();
    // The same zip again, its stem in the large form.
    let zip_path = songs.join("Song.zip");
    let mut old = zip::ZipArchive::new(fs::File::open(&zip_path).unwrap()).unwrap();
    let mut out = zip::ZipWriter::new(fs::File::create(root.join("large.zip")).unwrap());
    for i in 0..old.len() {
        let mut entry = old.by_index(i).unwrap();
        let name = entry.name().unwrap().to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(entry.compression())
            .large_file(name.ends_with("Take.wav"));
        out.start_file(name, options).unwrap();
        out.write_all(&bytes).unwrap();
    }
    out.finish().unwrap();
    drop(old);
    fs::rename(root.join("large.zip"), &zip_path).unwrap();

    fs::remove_file(&stem).unwrap();
    write(&save, &[8u8; 500], 10);
    let packed = packer.pack_song(std::slice::from_ref(&save), "Song.zip", 1, &stop).unwrap();
    assert!(matches!(&packed, Packed::Written { kept: 1, damaged, .. } if damaged.is_empty()), "{packed:?}");
    assert_eq!(bytes_in(&zip_path, "Stems/Take.wav"), vec![5u8; 4000]);
    assert_eq!(bytes_in(&zip_path, "Song/Song.flp"), vec![8u8; 500]);
    // And once more, from a zip that was itself made by a copy.
    write(&save, &[9u8; 500], 5);
    let packed = packer.pack_song(std::slice::from_ref(&save), "Song.zip", 1, &stop).unwrap();
    assert!(matches!(&packed, Packed::Written { kept: 1, damaged, .. } if damaged.is_empty()), "{packed:?}");
    assert_eq!(bytes_in(&zip_path, "Stems/Take.wav"), vec![5u8; 4000]);
}
