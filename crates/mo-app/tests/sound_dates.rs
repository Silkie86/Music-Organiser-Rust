//! When sounds' files were made and last opened, read from their folders for the Sounds year strip. The files are made
//! in the test build's own fixtures folder, under fixed names, never a random temp one.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use chrono::{Local, NaiveDateTime};
use mo_app::sound_dates::read;
use mo_sounds::Sound;

fn sound(id: i64, path: String) -> Sound {
    Sound {
        id,
        path,
        added: NaiveDateTime::default(),
        error: None,
        analysis: None,
        name_type: None,
        user_type: None,
        sound_type: "Kick".into(),
        type_source: "name".into(),
        type_confidence: 1.0,
        favourite: false,
        rating: 0,
        plays: 0,
        tags: Vec::new(),
        hidden: false,
        activity: Default::default(),
    }
}

#[test]
fn the_files_dates_are_read_from_their_folders() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-app-sound-dates");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("few")).unwrap();
    let mut sounds = Vec::new();
    // More than a few in one folder, so it's listed; and one in a folder of its own, asked for by its path.
    for n in 0..6 {
        let path = dir.join(format!("{n}.wav"));
        std::fs::write(&path, b"RIFF").unwrap();
        sounds.push(sound(n, path.display().to_string()));
    }
    std::fs::write(dir.join("not in the library.wav"), b"RIFF").unwrap();
    let alone = dir.join("few").join("alone.wav");
    std::fs::write(&alone, b"RIFF").unwrap();
    sounds.push(sound(6, alone.display().to_string()));
    sounds.push(sound(50, dir.join("gone.wav").display().to_string()));
    // No folder: nothing to say where its file is.
    sounds.push(sound(51, "kick.wav".into()));
    let dates = read(&sounds);
    let now = Local::now().naive_local();
    for n in 0..=6 {
        let opened = dates.opened(n).expect("every system keeps when a file was opened");
        assert!((now - opened).num_hours().abs() < 24, "{opened} for {n}");
        // Linux keeps when a file was made only on some file systems.
        if let Some(made) = dates.made(n) {
            assert!((now - made).num_hours().abs() < 24);
        }
    }
    for n in [50, 51] {
        assert_eq!((dates.made(n), dates.opened(n)), (None, None));
    }
}
