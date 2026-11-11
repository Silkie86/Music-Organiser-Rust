//! Play on the selected track, against the stand-in Bitwig (`mo_dawlink::bitwig::fake`): no test reaches a real
//! Bitwig. The MIDI files are made in the test build's own fixtures folder under fixed names.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use mo_dawlink::bitwig::fake::{FakeBitwig, FakeTransport, World};
use mo_dawlink::bitwig::{Error, SongLoop, TakenOut, WithSong};
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
    assert_eq!(
        bitwig.bitwig().stop_playing(&playing, &turn()),
        Ok(TakenOut { deleted: true, loop_back: None }),
        "a part alone has no loop to put back"
    );
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
    assert!(bitwig.bitwig().stop_playing(&playing, &turn()).unwrap().deleted);
    let world = bitwig.world();
    assert!(world.playing);
    assert_eq!(world.requests.last().unwrap()["transport"], false);
}

#[test]
fn a_clip_you_renamed_is_yours_and_stays() {
    let bitwig = FakeBitwig::start().unwrap();
    let playing = bitwig.bitwig().play_on_selected(part("MO Play 3"), &turn(), &|| true).unwrap();
    bitwig.world().tracks[2].clips[0] = "C:\\MIDI\\Kept it.mid".into();
    assert!(!bitwig.bitwig().stop_playing(&playing, &turn()).unwrap().deleted);
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

/// Bars 9 to 16 of "Night Drive" in 4/4, as the stand-in has it open.
fn bars_9_to_16() -> SongLoop {
    SongLoop { project: "Night Drive".into(), start: 32.0, length: 32.0 }
}

#[test]
fn plays_a_part_with_its_songs_bars_going_round_and_puts_the_loop_back() {
    // Bitwig playing somewhere, with a loop of your own (off) and Play starting at bar 3.
    let yours = FakeTransport { looping: false, loop_start: 64.0, loop_length: 8.0, play_start: 8.0 };
    let bitwig = with(|w| {
        w.playing = true;
        w.transport = yours;
    });
    let file = part("MO Play 20");
    let playing = bitwig.bitwig().play_with_song(&file, Some(&bars_9_to_16()), &turn(), &|| true).unwrap();
    assert_eq!((playing.track.as_str(), &playing.with_song), ("Bass", &WithSong::Looping));
    {
        let world = bitwig.world();
        let asked = world.requests.iter().find(|r| r["cmd"] == "clip").unwrap();
        assert_eq!(asked["loop"], json!({ "start": 32.0, "length": 32.0 }));
        assert_eq!((asked["launch"].clone(), asked["audition"].clone()), (json!(true), json!(true)));
        assert_eq!(asked["onlyIn"], "Night Drive", "only ever in the song's own project");
        // The arranger goes round the bars, and Bitwig started again from the first of them with the part.
        let looped = FakeTransport { looping: true, loop_start: 32.0, loop_length: 32.0, play_start: 32.0 };
        assert_eq!(world.transport, looped);
        assert_eq!(world.started_at, [32.0]);
        assert!(world.playing);
        assert_eq!(world.launched, [(2, 0)]);
    }
    assert_eq!(bitwig.bitwig().stop_playing(&playing, &turn()), Ok(TakenOut { deleted: true, loop_back: Some(true) }));
    let world = bitwig.world();
    // Stopping it stops Bitwig (the part started it at the bars), and your loop and play start are back.
    assert_eq!(world.requests.last().unwrap()["transport"], true);
    assert!(!world.playing);
    assert_eq!(world.transport, yours);
    assert_eq!(world.song_loop, None);
    assert_eq!(world.tracks[2].clips, [""]);
}

#[test]
fn what_you_moved_while_it_played_stays_yours() {
    let bitwig = FakeBitwig::start().unwrap();
    let before = bitwig.world().transport;
    let playing = bitwig.bitwig().play_with_song(part("MO Play 21"), Some(&bars_9_to_16()), &turn(), &|| true).unwrap();
    // You drag the loop's end out to 12 bars and click in bar 5 while it plays.
    {
        let mut world = bitwig.world();
        world.transport.loop_length = 48.0;
        world.transport.play_start = 16.0;
    }
    // A clip you renamed stays too; the loop still goes back.
    bitwig.world().tracks[2].clips[0] = "C:\\MIDI\\Kept it.mid".into();
    assert_eq!(bitwig.bitwig().stop_playing(&playing, &turn()), Ok(TakenOut { deleted: false, loop_back: Some(true) }));
    let world = bitwig.world();
    let left =
        FakeTransport { looping: before.looping, loop_start: before.loop_start, loop_length: 48.0, play_start: 16.0 };
    assert_eq!(world.transport, left, "only what's still as the link set it goes back");
    assert_eq!(world.song_loop, None);
}

#[test]
fn with_another_project_open_the_part_plays_alone() {
    let bitwig = with(|w| w.playing = true);
    let before = bitwig.world().transport;
    let elsewhere = SongLoop { project: "Sunrise".into(), ..bars_9_to_16() };
    let playing = bitwig.bitwig().play_with_song(part("MO Play 22"), Some(&elsewhere), &turn(), &|| true).unwrap();
    assert_eq!(playing.with_song, WithSong::OtherProject("Night Drive".into()));
    {
        let world = bitwig.world();
        let asked = world.requests.iter().find(|r| r["cmd"] == "clip").unwrap();
        assert!(asked.get("loop").is_none(), "no bars of another song are looped: {asked}");
        assert_eq!(world.transport, before);
        assert!(world.started_at.is_empty(), "Bitwig plays on where it was");
    }
    // As a part alone: Bitwig was playing, so it plays on.
    assert!(bitwig.bitwig().stop_playing(&playing, &turn()).unwrap().deleted);
    assert!(bitwig.world().playing);

    // The name as Bitwig shows it, give or take capitals.
    let capitals = SongLoop { project: "night drive".into(), ..bars_9_to_16() };
    let playing = bitwig.bitwig().play_with_song(part("MO Play 23"), Some(&capitals), &turn(), &|| true).unwrap();
    assert_eq!(playing.with_song, WithSong::Looping);
    assert!(bitwig.bitwig().stop_playing(&playing, &turn()).unwrap().deleted);

    // A link that can't keep a change to one project (or didn't say which is open) loops nothing.
    let unnamed = with(|w| w.project = String::new());
    let playing =
        unnamed.bitwig().play_with_song(part("MO Play 24"), Some(&bars_9_to_16()), &turn(), &|| true).unwrap();
    assert_eq!(playing.with_song, WithSong::OtherProject(String::new()));
    assert_eq!(unnamed.world().song_loop, None);
}

#[test]
fn an_older_extension_plays_the_part_alone() {
    let old = with(|w| {
        w.hello.insert("extension".into(), json!("3.21"));
    });
    let before = old.world().transport;
    let playing = old.bitwig().play_with_song(part("MO Play 25"), Some(&bars_9_to_16()), &turn(), &|| true).unwrap();
    assert_eq!(playing.with_song, WithSong::OldExtension("the Music Organiser extension 3.21".into()));
    let world = old.world();
    assert!(world.requests.iter().find(|r| r["cmd"] == "clip").unwrap().get("loop").is_none());
    assert_eq!((world.transport, world.playing), (before, true));
}

#[test]
fn a_bitwig_that_doesnt_let_the_link_reach_its_loop_plays_the_part_alone() {
    let bitwig = with(|w| {
        w.playing = true;
        w.hello.insert("songLoop".into(), json!(false));
    });
    let before = bitwig.world().transport;
    let playing = bitwig.bitwig().play_with_song(part("MO Play 30"), Some(&bars_9_to_16()), &turn(), &|| true).unwrap();
    assert_eq!(playing.with_song, WithSong::NoLoop);
    let world = bitwig.world();
    // Asking would have been refused, with nothing played.
    assert!(world.requests.iter().find(|r| r["cmd"] == "clip").unwrap().get("loop").is_none());
    assert_eq!((world.transport, world.playing), (before, true));
    assert_eq!(world.launched, [(2, 0)]);
}

#[test]
fn a_part_the_app_never_took_out_doesnt_keep_the_loop_from_going_back() {
    let yours = FakeTransport { looping: false, loop_start: 64.0, loop_length: 8.0, play_start: 8.0 };
    let bitwig = with(|w| w.transport = yours);
    // The first is never stopped (the app let go of it); you click in bar 5 after it.
    let _let_go = bitwig.bitwig().play_with_song(part("MO Play 31"), Some(&bars_9_to_16()), &turn(), &|| true).unwrap();
    bitwig.world().transport.play_start = 16.0;
    let bars_17_to_20 = SongLoop { project: "Night Drive".into(), start: 64.0, length: 16.0 };
    let second = bitwig.bitwig().play_with_song(part("MO Play 32"), Some(&bars_17_to_20), &turn(), &|| true).unwrap();
    assert_eq!(
        bitwig.world().transport,
        FakeTransport { looping: true, loop_start: 64.0, loop_length: 16.0, play_start: 64.0 }
    );
    // The second's stop puts back what was found before the first, and where you clicked since.
    assert_eq!(bitwig.bitwig().stop_playing(&second, &turn()), Ok(TakenOut { deleted: true, loop_back: Some(true) }));
    let world = bitwig.world();
    assert_eq!(world.transport, FakeTransport { play_start: 16.0, ..yours });
    assert_eq!(world.song_loop, None);
}

#[test]
fn a_stop_stopped_part_way_puts_the_loop_back_at_the_next_try() {
    let bitwig = FakeBitwig::start().unwrap();
    let before = bitwig.world().transport;
    let playing = bitwig.bitwig().play_with_song(part("MO Play 33"), Some(&bars_9_to_16()), &turn(), &|| true).unwrap();
    // Another project comes to the front while the clip comes out: the stop fails, with the bars still looped.
    bitwig.world().switch_during = Some(("deleteclip".into(), "Other Song".into()));
    assert!(bitwig.bitwig().stop_playing(&playing, &turn()).unwrap_err().is_elsewhere());
    assert!(bitwig.world().transport.looping);
    // Its project back in front, Stop again: the clip went the first time, and the loop goes back now.
    bitwig.world().project = "Night Drive".into();
    assert_eq!(bitwig.bitwig().stop_playing(&playing, &turn()), Ok(TakenOut { deleted: false, loop_back: Some(true) }));
    let world = bitwig.world();
    assert_eq!((world.transport, &world.song_loop), (before, &None));
}

#[test]
fn says_when_bitwig_doesnt_show_the_loop_back() {
    let bitwig = with(|w| w.loop_stays = true);
    let playing = bitwig.bitwig().play_with_song(part("MO Play 34"), Some(&bars_9_to_16()), &turn(), &|| true).unwrap();
    assert_eq!(bitwig.bitwig().stop_playing(&playing, &turn()), Ok(TakenOut { deleted: true, loop_back: Some(false) }));
}

#[test]
fn a_part_alone_says_the_song_wasnt_asked_for() {
    let bitwig = FakeBitwig::start().unwrap();
    let playing = bitwig.bitwig().play_on_selected(part("MO Play 26"), &turn(), &|| true).unwrap();
    assert_eq!(playing.with_song, WithSong::NotAsked);
    assert!(bitwig.world().requests.iter().find(|r| r["cmd"] == "clip").unwrap().get("loop").is_none());
}

#[test]
fn a_part_with_its_song_superseded_once_its_in_puts_the_loop_back() {
    let bitwig = with(|w| w.playing = true);
    let before = bitwig.world().transport;
    let asked = std::cell::Cell::new(0);
    // Wanted as its turn comes, not once the clip is in.
    let wanted = || asked.replace(asked.get() + 1) == 0;
    let played = bitwig.bitwig().play_with_song(part("MO Play 27"), Some(&bars_9_to_16()), &turn(), &wanted);
    assert_eq!(played, Err(Error::Cancelled));
    let world = bitwig.world();
    assert_eq!(world.transport, before);
    assert_eq!(world.song_loop, None);
    assert_eq!(world.tracks[2].clips, [""]);
    // The link stopped Bitwig to start it at the bars, so it stays stopped.
    assert!(!world.playing);
}

#[test]
fn a_part_bitwig_doesnt_load_leaves_the_loop_as_it_was() {
    let bitwig = with(|w| w.unloadable.push("MO Play 28.mid".into()));
    let before = bitwig.world().transport;
    let refused = bitwig.bitwig().play_with_song(part("MO Play 28"), Some(&bars_9_to_16()), &turn(), &|| true);
    assert!(matches!(refused, Err(Error::Bitwig(why)) if why.contains("didn't make a clip")));
    let world = bitwig.world();
    assert_eq!((world.transport, &world.song_loop), (before, &None));
}

#[test]
fn bars_bitwig_cant_loop_are_never_sent() {
    let bitwig = FakeBitwig::start().unwrap();
    for (start, length) in [(-4.0, 16.0), (0.0, 0.0), (f64::NAN, 16.0), (0.0, f64::INFINITY)] {
        let song = SongLoop { project: "Night Drive".into(), start, length };
        let refused = bitwig.bitwig().play_with_song(part("MO Play 29"), Some(&song), &turn(), &|| true);
        assert!(matches!(refused, Err(Error::Invalid(_))), "{start} {length}: {refused:?}");
    }
    assert!(bitwig.world().requests.is_empty());
}

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

#[test]
fn an_audition_is_told_as_soon_as_its_track_is_made() {
    // Before its preset loads, so an app closing while Bitwig loads a heavy one can still take the track out (the
    // 10 Nov review's D-2).
    let bitwig = FakeBitwig::start().unwrap();
    let told = std::sync::Mutex::new(None);
    let made = |a: &mo_dawlink::bitwig::Auditioning| {
        let devices = bitwig.world().tracks.iter().find(|t| t.name == a.track).map(|t| t.devices.len());
        *told.lock().unwrap() = Some((a.clone(), devices));
    };
    let auditioning = bitwig
        .bitwig()
        .audition_preset_telling(preset("Told"), part("MO Play 19"), "Told", &turn(), &|| true, &made)
        .unwrap();
    assert_eq!(told.into_inner().unwrap(), Some((auditioning.clone(), Some(0))), "told before the preset went in");
    assert!(bitwig.bitwig().stop_audition(&auditioning, &turn()).unwrap());
}
