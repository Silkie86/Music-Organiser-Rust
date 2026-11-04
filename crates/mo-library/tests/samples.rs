//! Finding samples in real folders, made in the test build's own fixtures folder under fixed names.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mo_library::samples::{SampleLocator, SoundLibrary};

#[test]
fn finds_samples_beside_in_the_tree_and_in_the_daws_folders() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-samples");
    let _ = fs::remove_dir_all(&dir);
    for f in [
        "Songs/Night Drive/samples/kick.wav",
        "Songs/Night Drive/Bounce/deep/vox.wav",
        "Songs/Night Drive/Backup/old.wav",
        "Content/Drums/snare.wav",
        "Content/Other/hat.wav",
        "Content/Projects/Other Song/snare2.wav",
        "Content/Drums/clap.wav",
        "Content/Other/clap.wav",
        "Home/Sounds/vox8.wav",
        "Home/AppData/Local/Some App/vox9.wav",
        "Home/node_modules/pack/vox9.wav",
    ] {
        let p = dir.join(f);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }
    let project = dir.join("Songs/Night Drive/Night Drive.flp").to_str().unwrap().to_owned();
    let content = dir.join("Content").to_str().unwrap().to_owned();
    let locator = SampleLocator::new(Box::new(move |_| vec![content.clone()]));
    // Where the project says, relative to it.
    assert!(locator.found("samples/kick.wav", &project));
    // Gone from where it was, found beside the project in a samples folder.
    assert!(locator.found("/nowhere/at/all/kick.wav", &project));
    // Somewhere in the project's tree, but not in its Backup folder.
    assert!(locator.found("/old/vox.wav", &project));
    assert!(!locator.found("/old/old.wav", &project));
    // In the DAW's folders only when the folder it's in has the same name too.
    assert!(locator.found("/old/Drums/snare.wav", &project));
    assert!(!locator.found("/old/Drums/hat.wav", &project));
    assert!(!locator.found("/old/Other Song/snare2.wav", &project));
    // Saved as a bare file name: the name alone counts in the DAW's folders only when one file there has it.
    assert!(locator.found("hat.wav", &project));
    assert!(!locator.found("clap.wav", &project));
    // A sketch saved in the profile folder: its tree is looked through, but not programs' own folders in it.
    let sketch = dir.join("Home/beat.flp").to_str().unwrap().to_owned();
    assert!(locator.found("/old/vox8.wav", &sketch));
    assert!(!locator.found("/old/vox9.wav", &sketch));
    // FL Studio's own content always counts.
    assert!(locator.found("%FLStudioFactoryData%/Data/Patches/Packs/x.wav", &project));
    assert!(!locator.found("/old/missing.wav", &project));
    assert!(!locator.found("", &project));
}

#[test]
fn missing_samples_are_found_in_the_sounds_library() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-samples-library");
    let _ = fs::remove_dir_all(&dir);
    let files = [
        ("Sounds/Neptunian Drumz 2/N4K1.wav", 4),
        ("Sounds/Kits/kick.wav", 3),
        ("Sounds/Other Kit/kick.wav", 5),
        ("Sounds/A/clap verb 1.wav", 6),
        ("Sounds/B/clap verb 1.wav", 6),
        ("Sounds/Gone/N6P20.wav", 2),
    ];
    let mut known = Vec::new();
    for (f, size) in files {
        let p = dir.join(f);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        if !f.contains("Gone") {
            fs::write(&p, vec![0u8; size]).unwrap();
        }
        known.push((p.to_str().unwrap().to_owned(), size as i64));
    }
    fs::create_dir_all(dir.join("Projects")).unwrap();
    let project = dir.join("Projects/jazzy.flp").to_str().unwrap().to_owned();
    let plain = SampleLocator::new(Box::new(|_| Vec::new()));
    let library = Arc::new(SoundLibrary::from_files(known));
    assert_eq!(library.len(), 6);
    let locator = SampleLocator::new(Box::new(|_| Vec::new())).with_library(library);
    let n4k1 = r"D:\harrys samples\nept2\Neptunian Drumz 2\N4K1.wav";
    assert!(!plain.found(n4k1, &project));
    // The same name in a folder of the same name.
    assert_eq!(locator.locate(n4k1, &project), Some(dir.join(files[0].0).to_str().unwrap().to_owned()));
    // The same name in a folder of the same name, though two sounds share the name.
    assert!(locator.locate(r"D:\old\Kits\kick.wav", &project).unwrap().contains("Kits"));
    // Two different sounds share the name and neither folder matches: not a guess.
    assert!(!locator.found(r"D:\old\Drums\kick.wav", &project));
    // Two copies of one sound (the same size) in other folders: either is it.
    assert!(locator.found(r"C:\Desktop\Kit Samples\clap verb 1.wav", &project));
    // In the library, but no longer on the disk.
    assert!(!locator.found(r"D:\old\Gone\N6P20.wav", &project));
    assert!(SoundLibrary::read(&dir.join("no such.db")).is_empty());
}

#[test]
fn the_sounds_library_is_read_only_when_a_sample_is_nowhere_else() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-samples-library-later");
    let _ = fs::remove_dir_all(&dir);
    for f in ["Songs/Night Drive/samples/kick.wav", "Sounds/Kits/snare.wav"] {
        let p = dir.join(f);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }
    let project = dir.join("Songs/Night Drive/Night Drive.flp").to_str().unwrap().to_owned();
    let snare = dir.join("Sounds/Kits/snare.wav").to_str().unwrap().to_owned();
    let reads = Arc::new(AtomicUsize::new(0));
    let counted = reads.clone();
    let locator = SampleLocator::new(Box::new(|_| Vec::new())).with_library_from(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        Arc::new(SoundLibrary::from_files([(snare.clone(), 1)]))
    });
    // Found in the project's own folders: the library isn't read.
    assert!(locator.found("samples/kick.wav", &project));
    assert!(locator.found(r"D:\old\kick.wav", &project));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    // Nowhere else: it's read, once, however many samples it's asked about.
    assert!(locator.found(r"D:\old\Kits\snare.wav", &project));
    assert!(!locator.found(r"D:\old\Kits\gone.wav", &project));
    assert!(locator.found(r"E:\other\Kits\snare.wav", &project));
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}

#[test]
fn the_sounds_library_is_read_from_sounds_db_with_its_sizes() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-samples-sounds-db");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("Projects")).unwrap();
    let db = dir.join("sounds.db");
    let c = rusqlite::Connection::open(&db).unwrap();
    // The C# app's sounds table (`SoundLibrary.Schema`), kept in WAL mode as it keeps it.
    c.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE sounds (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, size INTEGER,
             mtime INTEGER, error TEXT, analysis TEXT, features BLOB, clap BLOB, name_type TEXT, user_type TEXT,
             type TEXT, type_source TEXT, type_conf REAL, favourite INTEGER DEFAULT 0, rating INTEGER DEFAULT 0,
             plays INTEGER DEFAULT 0, last_played INTEGER)",
    )
    .unwrap();
    // (id, file, size in the library, on the disk): ids out of path order, as a library that grew over years has them.
    let files = [
        (7, "Sounds/Neptunian Drumz 2/N4K1.wav", Some(4), true),
        (3, "Sounds/Kits/kick.wav", Some(3), true),
        (5, "Sounds/Other Kit/kick.wav", Some(5), true),
        (2, "Sounds/A/clap verb 1.wav", Some(6), true),
        (9, "Sounds/B/clap verb 1.wav", Some(6), true),
        (4, "Sounds/Gone/N6P20.wav", Some(2), false),
        (6, "Sounds/Unsized/rim.wav", None, true),
        (8, "Sounds/Z/Loops/break.wav", Some(7), true),
        (10, "Sounds/A/Loops/break.wav", Some(7), true),
    ];
    for (id, f, size, on_disk) in files {
        let p = dir.join(f);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        if on_disk {
            fs::write(&p, vec![0u8; usize::try_from(size.unwrap_or(1)).unwrap()]).unwrap();
        }
        // A fingerprint beside each path, as every analysed sound has.
        c.execute(
            "INSERT INTO sounds (id, path, size, features) VALUES (?1, ?2, ?3, zeroblob(2048))",
            rusqlite::params![id, p.to_str().unwrap(), size],
        )
        .unwrap();
    }
    let path_of = |f: &str| dir.join(f).to_str().unwrap().to_owned();
    let project = path_of("Projects/jazzy.flp");
    let library = SoundLibrary::read(&db);
    assert_eq!(library.len(), files.len());
    assert!(library.is_complete());
    let locator = SampleLocator::new(Box::new(|_| Vec::new())).with_library(Arc::new(library));
    // The same name in a folder of the same name.
    assert_eq!(locator.locate(r"D:\harrys samples\Neptunian Drumz 2\N4K1.wav", &project), Some(path_of(files[0].1)));
    assert_eq!(locator.locate(r"D:\old\Kits\kick.wav", &project), Some(path_of(files[1].1)));
    // Two in folders of that name: the one that joined the library first, as before.
    assert_eq!(locator.locate(r"D:\old\Loops\break.wav", &project), Some(path_of(files[7].1)));
    // Two different sounds share the name, by the library's sizes, and neither folder matches: not a guess.
    assert!(!locator.found(r"D:\old\Drums\kick.wav", &project));
    // Two copies of one sound (the same size in the library) elsewhere: the first to join.
    assert_eq!(locator.locate(r"C:\Desktop\Kit Samples\clap verb 1.wav", &project), Some(path_of(files[3].1)));
    // In the library, but no longer on the disk.
    assert!(!locator.found(r"D:\old\Gone\N6P20.wav", &project));
    // The library has no size for it, so it can't tell it's the same sound: not a guess, unless the folder matches.
    assert!(!locator.found(r"D:\old\Drums\rim.wav", &project));
    assert!(locator.found(r"D:\old\Unsized\rim.wav", &project));
    // What's been worked out about a name is kept as long as the library: asked again after the sizes have changed
    // in the database, the answers are the same (the app reads the library again once the database changes).
    c.execute_batch("UPDATE sounds SET size = 3 WHERE id IN (3, 5); UPDATE sounds SET size = 99 WHERE id = 9;")
        .unwrap();
    assert_eq!(locator.locate(r"E:\Kits\clap verb 1.wav", &project), Some(path_of(files[3].1)));
    assert!(!locator.found(r"E:\Drums\kick.wav", &project));
    // A library read now sees the new sizes.
    let again = SampleLocator::new(Box::new(|_| Vec::new())).with_library(Arc::new(SoundLibrary::read(&db)));
    assert!(!again.found(r"E:\Kits\clap verb 1.wav", &project));
    assert_eq!(again.locate(r"E:\Drums\kick.wav", &project), Some(path_of(files[1].1)));
}

#[cfg(unix)]
#[test]
fn folder_links_back_up_the_tree_are_walked_once() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-samples-links");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("Song/deep")).unwrap();
    fs::write(dir.join("Song/deep/vox.wav"), b"x").unwrap();
    std::os::unix::fs::symlink(dir.join("Song"), dir.join("Song/deep/a")).unwrap();
    std::os::unix::fs::symlink(dir.join("Song"), dir.join("Song/deep/b")).unwrap();
    let project = dir.join("Song/Song.flp").to_str().unwrap().to_owned();
    let locator = SampleLocator::new(Box::new(|_| Vec::new()));
    assert!(locator.found("/old/vox.wav", &project));
    assert!(!locator.found("/old/gone.wav", &project));
}

#[test]
fn reads_a_project_file_into_a_save() {
    use mo_library::read::{Installed, read_save};
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-read");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Night Drive_15.flp");
    // Not a real FL Studio file: it's listed with what's wrong, as the C# app lists one.
    fs::write(&path, b"not an flp").unwrap();
    let path = path.to_str().unwrap();
    let detail = read_save(path, &Installed::default(), &SampleLocator::default()).unwrap();
    let save = &detail.save;
    assert_eq!((save.name.as_str(), save.daw.as_str(), save.size), ("Night Drive_15", "FL Studio", 10));
    assert_eq!(save.folder, dir.to_str().unwrap());
    assert!(save.mtime > 0 && save.id == 0);
    assert!(save.error.is_some() || !detail.warnings.is_empty());
    assert!(
        read_save(dir.join("kick.wav").to_str().unwrap(), &Installed::default(), &SampleLocator::default()).is_none()
    );
}

#[test]
fn a_bare_name_in_folders_inside_one_another_is_one_file() {
    // Bitwig's projects list both "Bitwig Studio" and "Bitwig Studio\Library": a file in the Library is in both.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-samples-overlap");
    let _ = fs::remove_dir_all(&dir);
    // Joined part by part, as the app joins them; the inner folder is written with "/" on purpose (it may be).
    let kick = dir.join("Bitwig Studio").join("Library").join("Drums").join("Only Kick.wav");
    fs::create_dir_all(kick.parent().unwrap()).unwrap();
    fs::write(&kick, b"x").unwrap();
    let project = dir.join("Songs/Night/Night.bwproject").to_str().unwrap().to_owned();
    let outer = dir.join("Bitwig Studio").to_str().unwrap().to_owned();
    let inner = dir.join("Bitwig Studio/Library").to_str().unwrap().to_owned();
    let locator = SampleLocator::new(Box::new(move |_| vec![outer.clone(), inner.clone()]));
    let found = locator.locate("Only Kick.wav", &project).unwrap();
    assert_eq!(fs::canonicalize(found).unwrap(), fs::canonicalize(&kick).unwrap());
}
