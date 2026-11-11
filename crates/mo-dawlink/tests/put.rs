//! Put in the song, against the stand-in Bitwig (`mo_dawlink::bitwig::fake`): no test reaches a real Bitwig. The MIDI
//! files are made in the test build's own fixtures folder under fixed names.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use mo_dawlink::bitwig::fake::{FakeArrangerClip, FakeBitwig, World};
use mo_dawlink::bitwig::{Error, PutIn, Seen, SongLoop};

/// A turn in the app's line of changes, as a screen takes one.
fn turn() -> mo_dawlink::bitwig::Turn<'static> {
    mo_dawlink::bitwig::ticket().wait()
}

fn part(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("put-{name}"));
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

/// Bars 9 to 12 of the song the stand-in has open: 16 beats from beat 32, in 4/4.
fn bars_9_to_12() -> SongLoop {
    SongLoop { project: "Night Drive".into(), start: 32.0, length: 16.0 }
}

fn put(bitwig: &FakeBitwig, name: &str) -> Result<PutIn, Error> {
    bitwig.bitwig().put_in_song(part(name), &bars_9_to_12(), &turn(), &|| true)
}

#[test]
fn records_a_part_into_the_songs_bars_on_the_selected_track() {
    let bitwig = with(|w| w.tracks[2].clips = vec!["C:\\MIDI\\Yours.mid".into()]);
    let file = part("Saw Lead");
    let went = bitwig.bitwig().put_in_song(&file, &bars_9_to_12(), &turn(), &|| true).unwrap();
    assert_eq!(went, PutIn { track: "Bass".into(), seen: Seen::Whole, not_back: Vec::new(), clip_left: false });
    let world = bitwig.world();
    let bass = world.tracks[2].id.clone();
    assert_eq!(
        world.arranger,
        [FakeArrangerClip {
            track: bass.clone(),
            at: 32.0,
            length: 16.0,
            file: file.to_string_lossy().into_owned(),
            armed: true
        }]
    );
    // The clip it was recorded from is out of the launcher again; yours stays.
    assert_eq!(world.tracks[2].clips, ["C:\\MIDI\\Yours.mid", ""]);
    assert!(world.auditions.is_empty(), "the link no longer has a clip noted as the app's");
    assert!(!world.playing);
    assert_eq!(world.commands(), ["selectedtrack", "transport", "transport", "clip", "record", "deleteclip"]);
    let clip = &world.requests[3];
    assert_eq!(clip["trackId"], bass.as_str());
    assert_eq!(clip["audition"], true);
    assert!(clip.get("launch").is_none(), "the record job launches it, with record on");
    let record = &world.requests[4];
    assert_eq!(record["trackId"], bass.as_str());
    assert_eq!(record["slot"], 1);
    assert_eq!(record["at"], 32.0);
    assert_eq!(record["length"], 16.0);
    assert_eq!(record["arm"], true);
    assert_eq!(record["deleteLauncherClip"], true);
    assert_eq!(record["replay"], false);
    // Every step is kept to the song's project.
    for request in &world.requests[3..] {
        assert_eq!(request["onlyIn"], "Night Drive", "{request}");
    }
    let last = world.requests.last().unwrap();
    assert_eq!(last["transport"], false, "Bitwig is stopped already: Stop then would move the play start");
}

#[test]
fn only_into_the_songs_own_project() {
    let bitwig = with(|w| w.project = "Glass".into());
    assert_eq!(put(&bitwig, "Elsewhere"), Err(Error::NotTheSong { open: "Glass".into(), song: "Night Drive".into() }));
    let world = bitwig.world();
    assert!(world.arranger.is_empty());
    assert!(world.requests.is_empty(), "nothing is asked of another project");
    drop(world);
    // The same save, typed another way, is the song's.
    let bitwig = with(|w| w.project = "night drive".into());
    assert!(put(&bitwig, "Same song").is_ok());
    // A Bitwig that doesn't say what it has open is left alone.
    let bitwig = with(|w| w.project = String::new());
    let said = put(&bitwig, "Unnamed").unwrap_err().to_string();
    assert!(said.starts_with("Bitwig didn't say which project it has open"), "{said}");
}

#[test]
fn needs_an_instrument_track_selected() {
    let bitwig = with(|w| w.selected_track = Some(1));
    assert_eq!(
        put(&bitwig, "On audio"),
        Err(Error::NotAnInstrumentTrack { name: "Kick".into(), kind: "Audio".into() })
    );
    let bitwig = with(|w| w.selected_track = None);
    assert_eq!(put(&bitwig, "On nothing"), Err(Error::NoTrackSelected));
    assert!(bitwig.world().arranger.is_empty());
    assert_eq!(bitwig.world().commands(), ["selectedtrack"], "nothing was put in");
}

#[test]
fn needs_the_extension_3_19_and_changes_on() {
    let bitwig = with(|w| {
        w.hello.insert("extension".into(), "3.18".into());
    });
    assert_eq!(
        put(&bitwig, "Old"),
        Err(Error::OldExtension { has: "the Music Organiser extension 3.18".into(), needs: "3.19" })
    );
    let bitwig = with(|w| w.changes = false);
    assert_eq!(put(&bitwig, "Changes off"), Err(Error::ChangesOff));
    assert!(bitwig.world().requests.is_empty());
}

#[test]
fn a_bitwig_that_plays_on_is_left_alone() {
    let bitwig = with(|w| w.playing = true);
    let said = put(&bitwig, "Playing").unwrap_err().to_string();
    assert_eq!(said, "Bitwig is playing. Stop it, then try again.");
    let world = bitwig.world();
    assert!(world.playing, "the app doesn't stop a Bitwig it didn't start");
    assert!(world.arranger.is_empty());
    assert!(world.tracks[2].clips.is_empty(), "nothing went in the launcher either");
    assert!(world.commands().iter().all(|c| c == "selectedtrack" || c == "transport"));
}

#[test]
fn waits_for_a_bitwig_the_app_has_just_stopped() {
    let bitwig = with(|w| w.playing = true);
    // As after Stop in Bitwig: it shows stopped a moment later.
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(200));
            bitwig.world().playing = false;
        });
        assert_eq!(put(&bitwig, "Just stopped").map(|went| went.seen), Ok(Seen::Whole));
    });
    assert_eq!(bitwig.world().arranger.len(), 1);
}

#[test]
fn says_when_bitwig_doesnt_take_what_the_recording_needs() {
    let bitwig = with(|w| w.recording.refuses = vec!["pre-roll".into(), "launch quantize".into()]);
    let said = put(&bitwig, "Refused").unwrap_err().to_string();
    assert_eq!(
        said,
        "Bitwig didn't take what the recording needs (pre-roll, launch quantize), so nothing was recorded."
    );
    let world = bitwig.world();
    assert!(world.arranger.is_empty());
    // The clip it would have recorded comes out of the launcher again.
    assert_eq!(world.tracks[2].clips, [""]);
    assert_eq!(world.commands().last().map(String::as_str), Some("deleteclip"));
}

#[test]
fn bars_that_arent_whole_bars_in_bitwig_now_are_said_before_anything_goes_in() {
    // The song was saved in 4/4, and Bitwig is in 7/8 now.
    let bitwig = with(|w| w.time_signature = "7/8".into());
    let said = put(&bitwig, "Odd time").unwrap_err().to_string();
    assert_eq!(
        said,
        "Bitwig is in 7/8 now, and the song's bars as it was last saved aren't whole bars of that, so the part wasn't \
         recorded. Save the song in Bitwig, then look for parts that fit it again."
    );
    let world = bitwig.world();
    assert!(world.arranger.is_empty());
    assert!(world.tracks[2].clips.is_empty(), "nothing went in the launcher");
    assert!(!world.commands().iter().any(|c| c == "clip" || c == "record"));
    drop(world);
    // A time signature the app can't read is left to the link, which says so in its own words.
    let bitwig = with(|w| w.time_signature = "free".into());
    let said = put(&bitwig, "No time").unwrap_err().to_string();
    assert_eq!(said, "The link can't tell a bar's length from Bitwig's time signature \"free\".");
    assert_eq!(bitwig.world().tracks[2].clips, [""], "its clip came out again");
}

#[test]
fn what_the_link_refuses_in_its_own_words_leaves_nothing_behind() {
    // A launcher clip of yours playing (or waiting to) is in the way: the link says which track.
    let bitwig = with(|w| w.launched = vec![(1, 0)]);
    let said = put(&bitwig, "Busy").unwrap_err().to_string();
    assert!(said.starts_with("A launcher clip on \"Kick\" is playing"), "{said}");
    assert_eq!(bitwig.world().tracks[2].clips, [""]);
}

#[test]
fn another_project_coming_to_the_front_stops_it_and_says_what_to_look_at() {
    let bitwig = with(|w| w.switch_during = Some(("record".into(), "Glass".into())));
    let said = put(&bitwig, "Switched").unwrap_err().to_string();
    assert!(
        said.starts_with("Another project came to the front in Bitwig while the part was being recorded"),
        "{said}"
    );
    assert!(said.contains("the master's mute"), "{said}");
    // Its clip can't be taken out of a project that isn't in front, and the words say where it is.
    assert!(said.ends_with("The part's clip may still be in \u{201c}Bass\u{201d}'s clip launcher there."), "{said}");
    let world = bitwig.world();
    assert!(world.arranger.is_empty());
    assert_eq!(world.project, "Glass");
}

#[test]
fn says_when_only_some_of_it_may_be_there() {
    let bitwig = with(|w| w.recording.ends = Some("stoppedEarly".into()));
    assert_eq!(put(&bitwig, "Short").map(|went| went.seen), Ok(Seen::Short));
    let bitwig = with(|w| w.recording.unseen = true);
    assert_eq!(put(&bitwig, "Unseen").map(|went| went.seen), Ok(Seen::Unseen));
}

#[test]
fn says_what_bitwig_didnt_show_back() {
    let bitwig = with(|w| w.recording.not_back = vec!["master mute".into()]);
    let went = put(&bitwig, "Not back").unwrap();
    assert_eq!(went.not_back, ["master mute"]);
    assert_eq!(went.seen, Seen::Whole);
}

#[test]
fn one_no_longer_wanted_puts_nothing_in() {
    let bitwig = FakeBitwig::start().unwrap();
    let file = part("Unwanted");
    assert_eq!(bitwig.bitwig().put_in_song(&file, &bars_9_to_12(), &turn(), &|| false), Err(Error::Cancelled));
    assert!(bitwig.world().requests.is_empty());
    // No longer wanted once its clip is in (the app is closing): the clip comes out, and nothing is recorded.
    let asked = std::cell::Cell::new(0);
    let wanted = || {
        asked.set(asked.get() + 1);
        asked.get() == 1
    };
    assert_eq!(bitwig.bitwig().put_in_song(&file, &bars_9_to_12(), &turn(), &wanted), Err(Error::Cancelled));
    let world = bitwig.world();
    assert!(world.arranger.is_empty());
    assert_eq!(world.tracks[2].clips, [""]);
    assert!(!world.commands().iter().any(|c| c == "record"));
}

#[test]
fn a_part_that_isnt_there_or_bars_bitwig_cant_take_send_nothing() {
    let bitwig = FakeBitwig::start().unwrap();
    let gone = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("put-gone").join("Gone.mid");
    assert!(matches!(
        bitwig.bitwig().put_in_song(&gone, &bars_9_to_12(), &turn(), &|| true),
        Err(Error::Missing { .. })
    ));
    for (start, length) in [(-4.0, 16.0), (32.0, 0.0), (f64::NAN, 16.0), (32.0, f64::INFINITY)] {
        let bars = SongLoop { project: "Night Drive".into(), start, length };
        let said = bitwig.bitwig().put_in_song(part("Odd bars"), &bars, &turn(), &|| true);
        assert!(matches!(said, Err(Error::Invalid(_))), "{said:?}");
    }
    assert!(bitwig.world().requests.is_empty());
}

#[test]
fn bitwig_not_answering_in_time_says_it_may_still_be_recording() {
    // A fast song, so the test's wait is short: one bar and the job's two more take under 2 seconds at 666 BPM.
    let bitwig = with(|w| {
        w.silent = vec!["record".into()];
        w.tempo = 666.0;
    });
    let mut to = bitwig.bitwig();
    to.change_timeout = Duration::from_millis(300);
    let asked = std::time::Instant::now();
    let said = to.put_in_song(part("Silent"), &SongLoop { length: 4.0, ..bars_9_to_12() }, &turn(), &|| true);
    assert_eq!(
        said.unwrap_err().to_string(),
        "Bitwig didn't answer in time. Bitwig may still be recording the part: give it a moment, then have a look \
         there. If its clip stays in \u{201c}Bass\u{201d}'s clip launcher, delete it there."
    );
    // The answer was waited for as long as the part takes to play, on top of the wait any change gets.
    assert!(asked.elapsed() >= Duration::from_millis(1800 + 300), "{:?}", asked.elapsed());
    // Nothing more was asked of a Bitwig that may be recording.
    assert_eq!(bitwig.world().commands().last().map(String::as_str), Some("record"));
}
