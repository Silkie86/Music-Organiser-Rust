//! Plugin sounds a save lost, taken from older saves of the song (the C# `LostSoundsTests`), on made-up saves in the
//! test build's own fixtures folder.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(unused_imports)]
#[path = "../../mo-formats/tests/common/mod.rs"]
mod common;

use std::path::PathBuf;

use mo_convert::lost::from_older_saves;
use mo_formats::fl::{events, song};

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-convert").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn read(bytes: &[u8]) -> song::FlSong {
    song::read_song(&events::load(bytes).unwrap())
}

#[test]
fn a_lost_sound_comes_from_the_newest_older_save_that_has_it() {
    let dir = folder("lost sounds");
    // funk step_8 (FL 9) still has LegacyCell's sound; step_9 (FL 25) lost it; step_10 is newer, so never used.
    std::fs::write(dir.join("funk step_7.flp"), common::fl_studio_9_project(b"OLDEST")).unwrap();
    std::fs::write(dir.join("funk step_8.flp"), common::fl_studio_9_project(b"CELL")).unwrap();
    std::fs::write(dir.join("funk step_10.flp"), common::fl_studio_9_project(b"NEWER")).unwrap();
    std::fs::write(dir.join("other song_8.flp"), common::fl_studio_9_project(b"OTHER")).unwrap();
    let path = dir.join("funk step_9.flp");
    let bytes = common::fl_studio_25_nothing_saved();
    std::fs::write(&path, &bytes).unwrap();

    let mut song = read(&bytes);
    let taken = from_older_saves(&mut song, &path);
    assert_eq!(taken, ["Cell: LegacyCell's sound, from funk step_8"]);
    let cell = song.channels[0].instrument.as_ref().unwrap();
    assert!(!cell.nothing_saved);
    let want = read(&common::fl_studio_9_project(b"CELL")).channels[0].instrument.clone().unwrap().state;
    assert_eq!(cell.state, want);
}

#[test]
fn a_song_that_lost_nothing_reads_no_other_save() {
    let dir = folder("nothing lost");
    std::fs::write(dir.join("funk step_8.flp"), b"not an FL project").unwrap();
    let path = dir.join("funk step_9.flp");
    let bytes = common::fl_studio_9_project(b"CELL");
    let mut song = read(&bytes);
    let before = song.clone();
    assert!(from_older_saves(&mut song, &path).is_empty());
    assert_eq!(song, before);
    // Lost, but no older save has it (and a damaged one is passed over): converted as it is.
    let mut lost = read(&common::fl_studio_25_nothing_saved());
    assert!(from_older_saves(&mut lost, &path).is_empty());
    assert!(lost.channels[0].instrument.as_ref().unwrap().nothing_saved);
}

#[test]
fn the_convert_says_which_sounds_it_took_and_from_where() {
    use common::bitwig::{Template, bitwig_template};
    let dir = folder("lost sounds convert");
    std::fs::write(dir.join("funk step_8.flp"), common::fl_studio_9_project(b"CELL")).unwrap();
    // Fix's copy of the same save counts as that save, and the save itself is used first.
    std::fs::write(dir.join("funk step_8 (fixed).flp"), common::fl_studio_9_project(b"FIXED")).unwrap();
    let path = dir.join("funk step_9.flp");
    std::fs::write(&path, common::fl_studio_25_nothing_saved()).unwrap();
    let template = bitwig_template(Template::default());
    let saved = mo_convert::save::bitwig_from_fl(
        &path,
        &dir.join("Exports"),
        &template,
        "funk step_9",
        None,
        mo_convert::Options::default(),
        &|_| None,
    )
    .unwrap();
    assert_eq!(saved.taken, ["Cell: LegacyCell's sound, from funk step_8"]);
    let list = std::fs::read_to_string(saved.file.with_file_name("funk step_9 - not carried over.txt")).unwrap();
    assert!(
        list.contains(
            "Sounds this save lost (the plugin gave FL Studio none), taken from an older save of the song:\r\n  Cell: \
             LegacyCell's sound, from funk step_8\r\n"
        ),
        "{list}"
    );
}
