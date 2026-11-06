//! Play on the selected track, against the stand-in Bitwig (`mo_dawlink::bitwig::fake`): no test reaches a real
//! Bitwig. The MIDI files are made in the test build's own fixtures folder under fixed names.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use mo_dawlink::bitwig::Error;
use mo_dawlink::bitwig::fake::{FakeBitwig, World};
use serde_json::json;

/// A turn in the app's line of changes, as a screen takes one.
fn turn() -> mo_dawlink::bitwig::Turn<'static> {
    mo_dawlink::bitwig::ticket().wait()
}

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
    let playing = bitwig.bitwig().play_on_selected(&file, &turn(), &|| true).unwrap();
    assert_eq!(playing.track, "Bass");
    assert_eq!(playing.slot(), 1);
    {
        let world = bitwig.world();
        assert!(world.playing);
        assert_eq!(world.launched, [(2, 1)]);
        assert_eq!(world.tracks[2].clips[1], file.to_string_lossy());
    }
    assert!(bitwig.bitwig().stop_playing(&playing, &turn()).unwrap());
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
    let playing = bitwig.bitwig().play_on_selected(part("MO Play 2"), &turn(), &|| true).unwrap();
    assert!(bitwig.bitwig().stop_playing(&playing, &turn()).unwrap());
    let world = bitwig.world();
    assert!(world.playing);
    assert_eq!(world.requests.last().unwrap()["transport"], false);
}

#[test]
fn a_clip_you_renamed_is_yours_and_stays() {
    let bitwig = FakeBitwig::start().unwrap();
    let playing = bitwig.bitwig().play_on_selected(part("MO Play 3"), &turn(), &|| true).unwrap();
    bitwig.world().tracks[2].clips[0] = "C:\\MIDI\\Kept it.mid".into();
    assert!(!bitwig.bitwig().stop_playing(&playing, &turn()).unwrap());
    // It still stops playing.
    let world = bitwig.world();
    assert_eq!(world.tracks[2].clips, ["C:\\MIDI\\Kept it.mid"]);
    assert!(world.launched.is_empty());
    assert!(!world.playing);
}

#[test]
fn needs_an_instrument_track_selected() {
    let none = with(|w| w.selected_track = None);
    assert_eq!(none.bitwig().play_on_selected(part("MO Play 4"), &turn(), &|| true), Err(Error::NoTrackSelected));
    let audio = with(|w| w.selected_track = Some(1));
    let refused = audio.bitwig().play_on_selected(part("MO Play 5"), &turn(), &|| true).unwrap_err();
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
    let refused = old.bitwig().play_on_selected(part("MO Play 6"), &turn(), &|| true).unwrap_err();
    assert_eq!(refused, Error::OldExtension { has: "the Music Organiser extension 3.18".into(), needs: "3.19" });
    let script = with(|w| {
        w.hello.insert("link".into(), json!(2));
        w.hello.remove("runtime");
        w.hello.remove("extension");
    });
    assert_eq!(
        script.bitwig().play_on_selected(part("MO Play 7"), &turn(), &|| true),
        Err(Error::OldExtension { has: String::new(), needs: "3.19" })
    );
    // Only the greeting: nothing was asked of Bitwig.
    assert!(old.world().requests.is_empty());
}

#[test]
fn a_part_that_isnt_there_isnt_played() {
    let bitwig = FakeBitwig::start().unwrap();
    let gone = part("MO Play 8").with_file_name("Gone.mid");
    assert!(matches!(bitwig.bitwig().play_on_selected(&gone, &turn(), &|| true), Err(Error::Missing { .. })));
}

// ---- Audition: a preset on a track of its own, playing a part (extension 3.20) ----

fn preset(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("audition-preset-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.bwpreset"));
    fs::write(&path, b"BtWg").unwrap();
    path
}

#[test]
fn auditions_a_preset_on_a_track_of_its_own_and_takes_it_out_again() {
    let bitwig = FakeBitwig::start().unwrap();
    let before = bitwig.world().tracks.len();
    let file = preset("Warm Pad");
    let auditioning = bitwig.bitwig().audition_preset(&file, part("MO Play 8"), "Warm Pad", &turn(), &|| true).unwrap();
    assert_eq!(auditioning.track, "Warm Pad");
    {
        let world = bitwig.world();
        assert_eq!(world.tracks.len(), before + 1);
        let track = &world.tracks[3];
        assert_eq!(track.name, "Warm Pad");
        assert_eq!(track.devices.len(), 1, "the preset's device is on it");
        assert!(world.playing);
        assert_eq!(world.launched, [(3, 0)]);
    }
    assert!(bitwig.bitwig().stop_audition(&auditioning, &turn()).unwrap());
    let world = bitwig.world();
    assert_eq!(world.tracks.len(), before);
    assert!(world.tracks.iter().all(|t| t.name != "Warm Pad"));
    assert!(!world.playing, "the audition started Bitwig, so it stops it");
    let last = world.requests.last().unwrap();
    assert_eq!(last["cmd"], "deletetrack");
    assert_eq!(last["onlyIn"], "Night Drive");
}

#[test]
fn keep_leaves_the_track_and_stops_it_playing() {
    let bitwig = FakeBitwig::start().unwrap();
    let auditioning =
        bitwig.bitwig().audition_preset(preset("Keeper"), part("MO Play 9"), "Keeper", &turn(), &|| true).unwrap();
    // Kept, it's yours: keeping takes the audition, so the app can't stop it (and take its track out) later.
    bitwig.bitwig().keep_audition(auditioning, &turn()).unwrap();
    let world = bitwig.world();
    assert_eq!(world.tracks[3].name, "Keeper");
    assert_eq!(world.tracks[3].clips.len(), 1);
    assert!(world.launched.is_empty());
    assert!(!world.playing);
    let last = world.requests.last().unwrap();
    assert_eq!(last["cmd"], "stoptracks");
    assert_eq!(last["transport"], true);
}

#[test]
fn a_track_you_renamed_is_yours_and_stays() {
    let bitwig = with(|w| w.playing = true);
    let auditioning =
        bitwig.bitwig().audition_preset(preset("Renamed"), part("MO Play 10"), "Renamed", &turn(), &|| true).unwrap();
    bitwig.world().tracks[3].name = "My Pad".into();
    assert!(!bitwig.bitwig().stop_audition(&auditioning, &turn()).unwrap());
    let world = bitwig.world();
    assert_eq!(world.tracks[3].name, "My Pad");
    assert!(world.launched.is_empty(), "it still stops playing");
    assert!(world.playing, "Bitwig was playing before, so it plays on");
}

#[test]
fn a_preset_bitwig_doesnt_load_leaves_no_track() {
    let bitwig = with(|w| w.unloadable = vec!["Broken.bwpreset".into()]);
    let before = bitwig.world().tracks.len();
    let refused =
        bitwig.bitwig().audition_preset(preset("Broken"), part("MO Play 11"), "Broken", &turn(), &|| true).unwrap_err();
    assert_eq!(refused.to_string(), "Bitwig didn't load Broken.bwpreset.");
    let world = bitwig.world();
    assert_eq!(world.tracks.len(), before);
    assert!(world.requests.iter().all(|r| r["cmd"] != "clip"));
}

#[test]
fn audition_needs_the_extension_3_20() {
    let old = with(|w| {
        w.hello.insert("extension".into(), json!("3.19"));
    });
    let refused =
        old.bitwig().audition_preset(preset("Old"), part("MO Play 12"), "Old", &turn(), &|| true).unwrap_err();
    assert_eq!(refused, Error::OldExtension { has: "the Music Organiser extension 3.19".into(), needs: "3.20" });
    assert!(old.world().requests.is_empty());
}

// ---- Newest wins, and tracks found by their ID (the 6 Nov review: D-1, D-5) ----

#[test]
fn a_start_no_longer_wanted_as_its_turn_comes_changes_nothing() {
    let bitwig = FakeBitwig::start().unwrap();
    let refused = bitwig.bitwig().audition_preset(preset("Late"), part("MO Play 13"), "Late", &turn(), &|| false);
    assert_eq!(refused.unwrap_err(), Error::Cancelled);
    assert_eq!(bitwig.bitwig().play_on_selected(part("MO Play 14"), &turn(), &|| false), Err(Error::Cancelled));
    assert!(bitwig.world().requests.is_empty(), "it didn't even connect");
}

#[test]
fn a_start_superseded_part_way_takes_its_track_out_inside_its_turn() {
    let bitwig = FakeBitwig::start().unwrap();
    let before = bitwig.world().tracks.len();
    // Wanted as its turn comes, then no longer (a newer Audition was pressed meanwhile).
    let asked = std::cell::Cell::new(0);
    let wanted = || {
        asked.set(asked.get() + 1);
        asked.get() == 1
    };
    let refused =
        bitwig.bitwig().audition_preset(preset("Superseded"), part("MO Play 15"), "Superseded", &turn(), &wanted);
    assert_eq!(refused.unwrap_err(), Error::Cancelled);
    let world = bitwig.world();
    assert_eq!(world.tracks.len(), before);
    assert!(world.requests.iter().all(|r| r["cmd"] != "clip"), "it stopped before the clip launched");
    assert!(!world.playing);
}

#[test]
fn a_part_superseded_once_its_clip_is_in_takes_the_clip_out() {
    let bitwig = FakeBitwig::start().unwrap();
    let asked = std::cell::Cell::new(0);
    let wanted = || {
        asked.set(asked.get() + 1);
        asked.get() == 1
    };
    assert_eq!(bitwig.bitwig().play_on_selected(part("MO Play 16"), &turn(), &wanted), Err(Error::Cancelled));
    let world = bitwig.world();
    assert!(world.tracks[2].clips.iter().all(String::is_empty));
    assert!(!world.playing, "it started Bitwig, so it stops it again");
}

#[test]
fn the_preset_goes_on_the_new_track_by_its_id() {
    let bitwig = FakeBitwig::start().unwrap();
    bitwig.bitwig().audition_preset(preset("By Id"), part("MO Play 17"), "By Id", &turn(), &|| true).unwrap();
    let world = bitwig.world();
    let insert = world.requests.iter().find(|r| r["cmd"] == "insert").unwrap();
    assert_eq!(insert["trackId"], world.tracks[3].id.as_str());
    assert!(insert.get("track").is_none());
    // An extension older than 3.21 doesn't take the ID there: it gets the place.
    let old = with(|w| {
        w.hello.insert("extension".into(), json!("3.20"));
    });
    old.bitwig().audition_preset(preset("By Place"), part("MO Play 18"), "By Place", &turn(), &|| true).unwrap();
    let world = old.world();
    let insert = world.requests.iter().find(|r| r["cmd"] == "insert").unwrap();
    assert_eq!(insert["track"], 3);
}
