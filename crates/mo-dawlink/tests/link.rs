//! The connection to the link in Bitwig, against the stand-in Bitwig (`mo_dawlink::bitwig::fake`): no test reaches a
//! real Bitwig, and none uses the link's own port.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::TcpListener;
use std::time::Duration;

use mo_dawlink::bitwig::fake::{CHANGES_OFF, FakeBitwig, OnConnect, World};
use mo_dawlink::bitwig::{DeviceId, Error, Inserted, Link, Pad};
use serde_json::{Value, json};

fn connect(bitwig: &FakeBitwig) -> Link {
    bitwig.bitwig().connect().unwrap()
}

fn with(change: impl FnOnce(&mut World)) -> FakeBitwig {
    let mut world = World::default();
    change(&mut world);
    FakeBitwig::with(world).unwrap()
}

fn pad(note: u8, path: &str, name: &str) -> Pad {
    Pad { note, path: path.into(), name: name.into() }
}

#[test]
fn greets_and_says_what_bitwig_has_open() {
    let bitwig = FakeBitwig::start().unwrap();
    let mut link = connect(&bitwig);
    let hello = link.hello().clone();
    assert_eq!((hello.link, hello.product.as_str(), hello.version.as_str()), (3, "Bitwig Studio", "6.1.5"));
    assert_eq!((hello.project.as_str(), hello.modified, hello.engine), ("Night Drive", false, true));
    assert_eq!((hello.changes, hello.only_in, hello.loopback), (true, true, Some(true)));
    assert_eq!(hello.answering(), "the Music Organiser extension 3.21");
    assert_eq!(hello.can_send(), Ok(()));

    // Asking again sees what changed in Bitwig meanwhile.
    bitwig.world().project = "Other Song".into();
    bitwig.world().modified = true;
    let again = link.refresh().unwrap();
    assert_eq!((again.project.as_str(), again.modified), ("Other Song", true));
    assert_eq!(link.hello().project, "Other Song");
    assert!(link.is_open());
    // Only the asking reached Bitwig: connecting sends nothing.
    assert_eq!(bitwig.world().commands(), ["hello"]);
}

#[test]
fn reads_messages_that_come_without_their_length() {
    let bitwig = with(|w| w.lengths = false);
    let mut link = connect(&bitwig);
    assert_eq!(link.hello().project, "Night Drive");
    assert_eq!(link.new_track("Kick").unwrap().name, "Kick");
}

#[test]
fn says_when_nothing_answers() {
    // A port nothing listens on: one the system had free, let go again. Another test's stand-in may be given it
    // meanwhile, so another is tried then.
    let mut answered = Vec::new();
    let err = loop {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        match Link::connect(port, Duration::from_millis(500)) {
            Err(Error::NotRunning) => break Error::NotRunning,
            other => answered.push(other.map(|link| link.hello().project.clone())),
        }
        assert!(answered.len() < 10, "something answered on every port tried: {answered:?}");
    };
    assert!(err.to_string().starts_with("Bitwig isn't answering. Check that Bitwig Studio is open"), "{err}");
    assert!(err.to_string().contains("\u{201c}Music Organiser Bitwig\u{201d}"), "{err}");
}

#[test]
fn something_else_on_the_port_isnt_taken_for_the_link() {
    for on_connect in [OnConnect::Nothing, OnConnect::Send(b"HTTP/1.1 400 Bad Request\r\n\r\n".to_vec())] {
        let bitwig = with(|w| w.on_connect = on_connect.clone());
        let err = Link::connect(bitwig.port(), Duration::from_millis(300)).unwrap_err();
        assert_eq!(err, Error::NotTheLink { port: bitwig.port() }, "{on_connect:?}");
        assert_eq!(
            err.to_string(),
            format!(
                "Something answered on port {}, but not the Music Organiser extension in Bitwig. If Bitwig is busy \
                 (opening a project, say), try again in a moment.",
                bitwig.port()
            )
        );
    }
    // One that closes the connection straight away.
    let bitwig = with(|w| w.on_connect = OnConnect::HangUp);
    assert_eq!(Link::connect(bitwig.port(), Duration::from_secs(2)).unwrap_err(), Error::Closed);
}

#[test]
fn a_connection_bitwig_closed_stays_closed() {
    let bitwig = with(|w| w.hang_up_on = vec!["hello".into()]);
    let mut link = connect(&bitwig);
    assert_eq!(link.refresh().unwrap_err(), Error::Closed);
    assert!(!link.is_open());
    // From then on it says so at once, without sending anything.
    let err = link.new_track("Kick").unwrap_err();
    assert_eq!((err.clone(), err.to_string()), (Error::NotConnected, "The connection to Bitwig is closed.".to_owned()));
    assert_eq!(bitwig.world().commands(), ["hello"]);
}

#[test]
fn a_link_that_gave_up_waiting_is_closed() {
    let bitwig = with(|w| w.late = vec![("hello".into(), Duration::from_millis(700))]);
    let mut link = connect(&bitwig);
    link.timeout = Duration::from_millis(200);
    assert_eq!(link.refresh().unwrap_err(), Error::TimedOut);
    assert_eq!(Error::TimedOut.to_string(), "Bitwig didn't answer in time.");
    // On Windows a read that ran out of time can lose what comes next, so the link isn't used again.
    assert!(!link.is_open());
    assert_eq!(link.refresh().unwrap_err(), Error::NotConnected);
    // A new one works.
    assert_eq!(connect(&bitwig).refresh().unwrap().project, "Night Drive");
}

#[test]
fn messages_that_arent_the_answer_are_passed_over() {
    // A greeting again, an answer to a request long gone, and something that isn't for the app, before the answer.
    let bitwig = with(|w| {
        w.before_answer = vec![
            json!({ "event": "hello", "result": { "link": 3, "project": "Stale" } }),
            json!({ "id": 99, "ok": true, "result": { "project": "Gone" } }),
            json!({ "something": "else" }),
        ];
    });
    let mut link = connect(&bitwig);
    bitwig.world().project = "Later".into();
    assert_eq!(link.refresh().unwrap().project, "Later");
    assert!(link.is_open());
}

#[test]
fn bitwigs_own_words_come_back_as_they_are() {
    let bitwig = FakeBitwig::start().unwrap();
    let mut link = connect(&bitwig);
    assert_eq!(
        link.command("frobnicate", Value::Null).unwrap_err(),
        Error::Bitwig("Unknown command: frobnicate".into())
    );
    let err = link.insert_file(99, "C:\\Kit\\Kick.wav").unwrap_err();
    assert_eq!(err.to_string(), "There's no track 99 in the project listing.");
    let err = link.fill_pads(&[pad(36, "C:\\Kit\\Kick.wav", "Kick")], Some((2, 0))).unwrap_err();
    assert_eq!(err.to_string(), "Device 0 on track 2 isn't a Drum Machine.");
    // A request's fields are a JSON object, which the app checks before sending.
    let err = link.command("hello", json!([1])).unwrap_err();
    assert_eq!(err, Error::Invalid("A request's fields are a JSON object, not [1].".into()));
    // A command this crate has no call for yet comes back as Bitwig answered it.
    assert_eq!(link.command("hello", json!({})).unwrap()["project"], "Night Drive");
    assert!(link.is_open());
}

#[test]
fn requests_are_laid_out_as_the_link_reads_them() {
    let bitwig = FakeBitwig::start().unwrap();
    let mut link = connect(&bitwig);
    let track = link.new_track("Kit #12").unwrap();
    link.insert_device(track.index, DeviceId::DRUM_MACHINE).unwrap();
    link.insert_file(track.index, "C:\\Samples\\Kick.wav").unwrap();
    link.fill_pads(&[pad(36, "C:\\Kit\\Kick.wav", "Kick"), pad(38, "C:\\Kit\\Snare.wav", "")], Some((track.index, 0)))
        .unwrap();
    assert_eq!(
        bitwig.world().requests,
        [
            json!({ "id": 1, "cmd": "newtrack", "name": "Kit #12" }),
            json!({ "id": 2, "cmd": "insert", "track": 3, "kind": "bitwig", "device": "8ea97e45-0255-40fd-bc7e-94419741e9d1" }),
            json!({ "id": 3, "cmd": "insert", "track": 3, "kind": "file", "path": "C:\\Samples\\Kick.wav" }),
            json!({
                "id": 4, "cmd": "pads", "track": 3, "device": 0,
                "pads": [
                    { "note": 36, "path": "C:\\Kit\\Kick.wav", "name": "Kick" },
                    { "note": 38, "path": "C:\\Kit\\Snare.wav", "name": null },
                ],
            }),
        ]
    );
    // A pad given no name has its sample's, as in Bitwig, whichever way the path's folders are written.
    link.fill_pads(&[pad(42, "/Users/solomon/Kit/Hat.wav", "")], Some((track.index, 0))).unwrap();
    let world = bitwig.world();
    let names: Vec<&str> = world.pads(track.index).unwrap().values().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Kick", "Snare", "Hat"]);
}

#[test]
fn adds_a_track_after_the_main_tracks_and_a_sample_on_a_sampler() {
    let bitwig = FakeBitwig::start().unwrap();
    let mut link = connect(&bitwig);
    let track = link.new_track("Kick 909").unwrap();
    // Before the Master.
    assert_eq!((track.index, track.name.as_str()), (3, "Kick 909"));
    assert!(track.id.is_some());
    let inserted = link.insert_file(track.index, "C:\\Samples\\Kick 909.wav").unwrap();
    assert!(inserted.added);
    assert_eq!(inserted.index, Some(0));
    let device = inserted.device.unwrap();
    assert_eq!(
        (device.name.as_str(), device.kind.as_str(), device.plugin, device.enabled),
        ("Sampler", "instrument", false, true)
    );
    // Bitwig quietly does nothing with an ID it doesn't know.
    let unknown = link.insert_device(track.index, DeviceId::from_bytes([7; 16])).unwrap();
    assert_eq!(unknown, Inserted { added: false, index: None, device: None });

    let world = bitwig.world();
    let names: Vec<&str> = world.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Drums", "Kick", "Bass", "Kick 909", "Master"]);
    assert_eq!(world.tracks[3].devices.len(), 1);
    assert_eq!(world.selected_track, Some(3));
    assert!(world.modified);
}

#[test]
fn changes_stay_in_the_project_they_began_in() {
    let bitwig = FakeBitwig::start().unwrap();
    let mut link = connect(&bitwig);
    assert_eq!(link.only_in(), None);
    link.stay_in_open_project();
    assert_eq!(link.only_in(), Some("Night Drive"));
    // Every change carries the project; asking what's open doesn't.
    link.new_track("Kick").unwrap();
    link.refresh().unwrap();
    {
        let world = bitwig.world();
        assert_eq!(world.requests[0]["onlyIn"], "Night Drive");
        assert!(world.requests[1].get("onlyIn").is_none());
    }

    // Another project came to the front: the link stops before changing anything there.
    bitwig.world().project = "Someone Else's".into();
    let err = link.new_track("Snare").unwrap_err();
    assert_eq!(
        err.to_string(),
        "Bitwig has \"Someone Else's\" open, not \"Night Drive\", so the link stopped before changing anything there."
    );
    assert!(err.is_elsewhere());
    assert!(!Error::Bitwig("Bitwig has \"x\" open.".into()).is_elsewhere() && !Error::TimedOut.is_elsewhere());
    assert!(bitwig.world().track("Snare").is_none());

    // And when it comes to the front part way through a change.
    bitwig.world().project = "Night Drive".into();
    bitwig.world().switch_during = Some(("insert".into(), "Other Song".into()));
    let err = link.insert_file(3, "C:\\Samples\\Snare.wav").unwrap_err();
    assert!(err.to_string().starts_with("Bitwig has \"Other Song\" open, not \"Night Drive\""), "{err}");

    // Without it, a change goes to whichever project is open.
    link.set_only_in(None);
    assert_eq!(link.only_in(), None);
    assert!(link.new_track("Snare").is_ok());
    assert!(bitwig.world().track("Snare").is_some());
}

#[test]
fn an_old_script_or_changes_off_take_nothing() {
    // A version 1 script: refused before anything is sent.
    let bitwig = with(|w| {
        w.hello.insert("link".into(), json!(1));
        w.hello.remove("runtime");
    });
    let mut link = connect(&bitwig);
    assert_eq!(link.hello().answering(), "the Music Organiser script");
    assert_eq!(link.new_track("Kick").unwrap_err(), Error::OldLink);
    assert_eq!(link.insert_file(2, "C:\\Kit\\Kick.wav").unwrap_err(), Error::OldLink);
    assert_eq!(link.fill_pads(&[pad(36, "C:\\Kit\\Kick.wav", "")], None).unwrap_err(), Error::OldLink);
    assert!(bitwig.world().requests.is_empty());

    // Changes off: Bitwig's own words, and nothing changed.
    let bitwig = with(|w| w.changes = false);
    let mut link = connect(&bitwig);
    assert_eq!(link.hello().can_send(), Err(Error::ChangesOff));
    assert_eq!(link.new_track("Kick").unwrap_err(), Error::Bitwig(CHANGES_OFF.into()));
    assert_eq!(bitwig.world().tracks.len(), 4);
}

#[test]
fn a_link_that_didnt_start_says_why() {
    let bitwig = with(|w| {
        w.hello.insert(
            "error".into(),
            json!("The Bitwig extension failed: no transport. Ask for a selftest to see what's wrong."),
        );
    });
    let link = connect(&bitwig);
    assert_eq!(link.hello().project, "");
    let err = link.hello().can_send().unwrap_err();
    assert!(matches!(err, Error::NotStarted(_)));
    assert_eq!(
        err.to_string(),
        "Part of the Music Organiser extension didn't start in Bitwig, so the app didn't send anything. Restarting \
         Bitwig may help. Bitwig said: The Bitwig extension failed: no transport. Ask for a selftest to see what's wrong."
    );
}

#[test]
fn names_and_paths_arrive_as_they_were() {
    let bitwig = FakeBitwig::start().unwrap();
    let mut link = connect(&bitwig);
    let name = "Caf\u{e9} \u{1f941} Kick";
    assert_eq!(link.new_track(name).unwrap().name, name);
    let path = "C:\\Samples\\Caf\u{e9}\\\u{1f941} Kick.wav";
    link.insert_file(3, path).unwrap();
    let world = bitwig.world();
    assert_eq!(world.tracks[3].name, name);
    assert_eq!(world.tracks[3].devices[0].sample, path);
}

#[test]
fn two_links_at_once_each_get_their_own_answers() {
    let bitwig = FakeBitwig::start().unwrap();
    let (mut a, mut b) = (connect(&bitwig), connect(&bitwig));
    let first = a.new_track("First").unwrap();
    let second = b.new_track("Second").unwrap();
    assert_eq!((first.index, second.index), (3, 4));
    assert_eq!(a.refresh().unwrap().project, "Night Drive");
    assert_eq!(bitwig.world().tracks[4].name, "Second");
}
