//! The watch feed (extension 3.24), against the stand-in Bitwig (`mo_dawlink::bitwig::fake`): no test reaches a real
//! Bitwig.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{Ipv4Addr, TcpListener};
use std::thread;
use std::time::{Duration, Instant};

use mo_dawlink::bitwig::fake::{FakeBitwig, World};
use mo_dawlink::bitwig::watch::{Playing, State};
use mo_dawlink::bitwig::{Live, Watch};
use serde_json::json;

/// Waits (up to 5 s) for the watch's picture to pass `test`, and returns it.
fn until(watch: &Watch, test: impl Fn(&Live) -> bool) -> std::sync::Arc<Live> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let now = watch.now();
        if test(&now) {
            return now;
        }
        assert!(Instant::now() < deadline, "the watch never got there: {now:?}");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn starts_with_the_whole_picture_then_takes_what_changes() {
    let bitwig = FakeBitwig::start().unwrap();
    let watch = Watch::start(bitwig.port());
    let live = until(&watch, |l| l.state == State::Watching && !l.tracks.is_empty());
    assert_eq!(live.project, "Night Drive");
    assert_eq!(live.tracks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Drums", "Kick", "Bass", "Master"]);
    assert_eq!(live.selected.track, Some(2));
    assert_eq!(live.selected.device.as_ref().map(|d| d.name.as_str()), Some("Serum"));
    assert_eq!(live.transport.as_ref().map(|t| t.tempo), Some(120.0));
    assert!(live.sections.iter().any(|s| s == "levels"));
    // Watching only reads: nothing else asked of Bitwig.
    assert_eq!(bitwig.world().commands(), ["watch"]);

    let before = watch.version();
    assert_eq!(bitwig.send_watch(json!({ "notes": [{ "track": 2, "id": "fake-track-3", "notes": [[60, 100]] }] })), 1);
    let live = until(&watch, |l| !l.notes.is_empty());
    assert_eq!(live.notes, [Playing { track: 2, id: Some("fake-track-3".into()), notes: vec![(60, 100)] }]);
    assert!(watch.version() > before);
    // What the event didn't have stays.
    assert_eq!(live.tracks.len(), 4);
}

#[test]
fn says_when_the_extension_is_too_old_and_never_asks_it() {
    let mut world = World::default();
    world.hello.insert("extension".into(), "3.23".into());
    let bitwig = FakeBitwig::with(world).unwrap();
    let watch = Watch::start(bitwig.port());
    let live = until(&watch, |l| l.state != State::Connecting);
    assert_eq!(live.state, State::TooOld("the Music Organiser extension 3.23".into()));
    assert!(!bitwig.world().commands().contains(&"watch".to_owned()));
}

#[test]
fn says_when_bitwig_isnt_there() {
    // A port nothing listens on: taken, then let go.
    let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port();
    let watch = Watch::start(port);
    assert_eq!(until(&watch, |l| l.state != State::Connecting).state, State::NoBitwig);
}

#[test]
fn dropping_the_watch_closes_its_connection_at_once() {
    let bitwig = FakeBitwig::start().unwrap();
    let watch = Watch::start(bitwig.port());
    until(&watch, |l| l.state == State::Watching);
    let began = Instant::now();
    drop(watch);
    assert!(began.elapsed() < Duration::from_secs(1), "dropping never waits for Bitwig");
    // The stand-in finds the connection gone when it next sends.
    let deadline = Instant::now() + Duration::from_secs(5);
    while bitwig.send_watch(json!({ "levels": [] })) > 0 {
        assert!(Instant::now() < deadline, "the connection stayed open");
        thread::sleep(Duration::from_millis(10));
    }
}
