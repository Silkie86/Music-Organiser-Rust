//! Play on the selected track, against the stand-in Bitwig (`mo_dawlink::bitwig::fake`): no test reaches a real
//! Bitwig. The MIDI files are made in the test build's own fixtures folder under fixed names.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use mo_dawlink::bitwig::Error;
use mo_dawlink::bitwig::fake::{FakeBitwig, World};
use serde_json::json;

fn part(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("audition-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.mid"));
    fs::write(&path, b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0").unwrap();
    path
}

fn with(change: impl FnOnce(&mut World)) -> FakeBitwig {
    let mut world = World::default();
    change(&mut world);
    FakeBitwig::with(world).unwrap()
}

#[test]
fn plays_a_part_on_the_selected_track_and_takes_it_out_again() {
    let bitwig = with(|w| w.tracks[2].clips = vec!["C:\\MIDI\\Yours.mid".into()]);
    let file = part("MO Play 1");
    let playing = bitwig.bitwig().play_on_selected(&file).unwrap();
    assert_eq!(playing.track, "Bass");
    assert_eq!(playing.slot(), 1);
    {
        let world = bitwig.world();
        assert!(world.playing);
        assert_eq!(world.launched, [(2, 1)]);
        assert_eq!(world.tracks[2].clips[1], file.to_string_lossy());
    }
    assert!(bitwig.bitwig().stop_playing(&playing).unwrap());
    let world = bitwig.world();
    // Bitwig wasn't playing before, so it stops; your own clip stays.
    assert!(!world.playing);
    assert!(world.launched.is_empty());
    assert_eq!(world.tracks[2].clips, ["C:\\MIDI\\Yours.mid", ""]);
    let last = world.requests.last().unwrap();
    assert_eq!(last["cmd"], "deleteclip");
    assert_eq!(last["onlyIn"], "Night Drive");
    assert_eq!(last["transport"], true);
}

#[test]
fn leaves_bitwig_playing_when_it_already_was() {
    let bitwig = with(|w| w.playing = true);
    let playing = bitwig.bitwig().play_on_selected(part("MO Play 2")).unwrap();
    assert!(bitwig.bitwig().stop_playing(&playing).unwrap());
    let world = bitwig.world();
    assert!(world.playing);
    assert_eq!(world.requests.last().unwrap()["transport"], false);
}

#[test]
fn a_clip_you_renamed_is_yours_and_stays() {
    let bitwig = FakeBitwig::start().unwrap();
    let playing = bitwig.bitwig().play_on_selected(part("MO Play 3")).unwrap();
    bitwig.world().tracks[2].clips[0] = "C:\\MIDI\\Kept it.mid".into();
    assert!(!bitwig.bitwig().stop_playing(&playing).unwrap());
    // It still stops playing.
    let world = bitwig.world();
    assert_eq!(world.tracks[2].clips, ["C:\\MIDI\\Kept it.mid"]);
    assert!(world.launched.is_empty());
    assert!(!world.playing);
}

#[test]
fn needs_an_instrument_track_selected() {
    let none = with(|w| w.selected_track = None);
    assert_eq!(none.bitwig().play_on_selected(part("MO Play 4")), Err(Error::NoTrackSelected));
    let audio = with(|w| w.selected_track = Some(1));
    let refused = audio.bitwig().play_on_selected(part("MO Play 5")).unwrap_err();
    assert_eq!(refused, Error::NotAnInstrumentTrack { name: "Kick".into(), kind: "Audio".into() });
    assert_eq!(
        refused.to_string(),
        "\u{201c}Kick\u{201d} is an audio track in Bitwig, so it can't play notes. Select an instrument track."
    );
    // Nothing went in either time.
    assert!(none.world().requests.iter().all(|r| r["cmd"] != "clip"));
    assert!(audio.world().requests.iter().all(|r| r["cmd"] != "clip"));
}

#[test]
fn needs_the_extension_3_19() {
    let old = with(|w| {
        w.hello.insert("extension".into(), json!("3.18"));
    });
    let refused = old.bitwig().play_on_selected(part("MO Play 6")).unwrap_err();
    assert_eq!(refused, Error::OldExtension { has: "the Music Organiser extension 3.18".into(), needs: "3.19" });
    let script = with(|w| {
        w.hello.insert("link".into(), json!(2));
        w.hello.remove("runtime");
        w.hello.remove("extension");
    });
    assert_eq!(
        script.bitwig().play_on_selected(part("MO Play 7")),
        Err(Error::OldExtension { has: String::new(), needs: "3.19" })
    );
    // Only the greeting: nothing was asked of Bitwig.
    assert!(old.world().requests.is_empty());
}

#[test]
fn a_part_that_isnt_there_isnt_played() {
    let bitwig = FakeBitwig::start().unwrap();
    let gone = part("MO Play 8").with_file_name("Gone.mid");
    assert!(matches!(bitwig.bitwig().play_on_selected(&gone), Err(Error::Missing { .. })));
}
