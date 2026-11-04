//! The Space page's songs, from the library's saves to what each song could free: songs made in the test build's own
//! fixtures folder under fixed names, never the computer's real project folders. The saves are made-up bytes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use chrono::NaiveDateTime;
use mo_app::space_songs::{FromLibrary, SongRoom, look};
use mo_library::Save;
use mo_space::song::{CompactKind, CompactSettings};

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// A save as the library lists it, written on disk with `bytes`.
fn save(id: i64, path: &Path, bytes: &[u8], modified: &str) -> Save {
    write(path, bytes);
    let text = path.to_string_lossy().into_owned();
    Save {
        id,
        name: path.file_stem().unwrap().to_string_lossy().into_owned(),
        daw: "Bitwig Studio".to_owned(),
        folder: path.parent().unwrap().to_string_lossy().into_owned(),
        size: i64::try_from(bytes.len()).unwrap(),
        modified: NaiveDateTime::parse_from_str(modified, "%Y-%m-%d %H:%M").unwrap(),
        path: text,
        ..Save::default()
    }
}

/// What a song could free, as (kind, file name) pairs.
fn items(song: &SongRoom) -> Vec<(CompactKind, String)> {
    let name = |p: &str| Path::new(p).file_name().unwrap().to_string_lossy().into_owned();
    song.report.items.iter().map(|i| (i.kind, name(&i.path))).collect()
}

#[test]
fn every_song_is_measured_and_the_save_it_opens_always_stays() {
    let dir = fixtures("mo-app-space-songs");
    let projects = dir.join("Projects");
    let night = projects.join("Night Drive");
    let bounce = projects.join("Bounce");
    // Night Drive, saved five times in one sitting. Its highest-numbered save, the one it opens, was saved before its
    // fourth: by time alone it's a save from the middle of the sitting. Its second and third saves are the same.
    let mut saves = vec![
        save(1, &night.join("Night Drive_1.bwproject"), b"first", "2026-10-10 10:00"),
        save(2, &night.join("Night Drive_2.bwproject"), b"second", "2026-10-10 10:20"),
        save(3, &night.join("Night Drive_3.bwproject"), b"second", "2026-10-10 10:30"),
        save(5, &night.join("Night Drive_5.bwproject"), b"the one it opens", "2026-10-10 10:50"),
        save(4, &night.join("Night Drive_4.bwproject"), b"fourth!", "2026-10-10 11:30"),
    ];
    // Seven auto-backups: the newest five stay.
    for n in 0..7u64 {
        let path = night.join("auto-backups").join(format!("Night Drive_4 ({n}).bwproject"));
        write(&path, &[b'a'; 10]);
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000 + n * 60);
        fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
    }
    // An old bounce no save names, a vocal its saves use, and a loop only another song's save uses.
    write(&night.join("Old bounce.wav"), &[0; 1000]);
    write(&night.join("Vox.wav"), &[0; 500]);
    write(&night.join("Shared loop.wav"), &[0; 300]);
    // Bounce: three saves in a morning, then one the next evening. Its second save is from the middle of a sitting.
    saves.extend([
        save(11, &bounce.join("Bounce_1.bwproject"), &[1; 200], "2026-10-11 09:00"),
        save(12, &bounce.join("Bounce_2.bwproject"), &[2; 210], "2026-10-11 09:10"),
        save(13, &bounce.join("Bounce_3.bwproject"), &[3; 220], "2026-10-11 09:20"),
        save(14, &bounce.join("Bounce_4.bwproject"), &[4; 230], "2026-10-12 18:00"),
    ]);
    // Gone: the library says its only save has gone, so it isn't measured.
    let gone = projects.join("Gone").join("Gone_1.bwproject");
    saves.push(Save { file_missing: true, ..save(21, &gone, &[5; 50], "2026-10-01 12:00") });
    fs::remove_file(&gone).unwrap();
    let from = FromLibrary {
        saves,
        samples: HashMap::from([
            (4, vec![r"C:\Old PC\Night Drive\Vox.wav".to_owned()]),
            (14, vec!["Shared loop.wav".to_owned()]),
        ]),
        roots: vec![projects.to_string_lossy().into_owned()],
        ..FromLibrary::default()
    };

    let found = look(&from, CompactSettings::default(), &AtomicBool::new(false)).unwrap();
    let mut songs: Vec<&SongRoom> = found.songs.iter().collect();
    songs.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<&str> = songs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Bounce", "Night Drive"]);
    let (bounce_room, night_room) = (songs[0], songs[1]);

    // The copy of the third save goes; the save it opens doesn't, though by time it's from the middle of the sitting.
    assert!(night_room.opens.ends_with("Night Drive_5.bwproject"));
    let mut night_items = items(night_room);
    night_items.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(
        night_items,
        [
            (CompactKind::SameSave, "Night Drive_2.bwproject".to_owned()),
            (CompactKind::Backup, "Night Drive_4 (0).bwproject".to_owned()),
            (CompactKind::Backup, "Night Drive_4 (1).bwproject".to_owned()),
            (CompactKind::UnusedAudio, "Old bounce.wav".to_owned()),
        ]
    );
    assert_eq!(night_room.saves, 5);
    assert_eq!(items(bounce_room), [(CompactKind::ThinnedSave, "Bounce_2.bwproject".to_owned())]);
    // Compact frees the copy and the old autosaves; the bounce goes only if you pick it.
    assert_eq!(found.compact_bytes(), 6 + 20);
    assert_eq!(found.thin_bytes(), 210);

    // Stopped, it says so.
    assert!(look(&from, CompactSettings::default(), &AtomicBool::new(true)).is_err());
}
