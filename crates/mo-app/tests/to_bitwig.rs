//! Send to Bitwig from the Sounds page, against the stand-in Bitwig (`mo_dawlink::bitwig::fake`): no test reaches a
//! real Bitwig, and none uses the link's own port (rule 11). The sounds are files made in the test build's own fixtures
//! folder under fixed names, never a random temp one.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mo_app::activity::{Card, Feed, State};
use mo_app::sounds::SoundList;
use mo_app::to_bitwig::{self, Bitwig, Kits, kit_name, kit_pads};
use mo_dawlink::bitwig::fake::{FakeBitwig, FakeDevice, FakeTrack};
use mo_dawlink::bitwig::{DeviceId, KitTarget, SOUND_PLACE};
use mo_sounds::Sound;
use mo_sounds::kits::KitSlot;

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A sound file: what's in it doesn't matter to the link, only that it's there.
fn sound(dir: &Path, name: &str) -> String {
    let path = dir.join(name);
    fs::write(&path, b"RIFF\0\0\0\0WAVE").unwrap();
    path.to_string_lossy().into_owned()
}

/// The one card on show: its headline, its line and how it stands.
fn card(feed: &Feed) -> (String, String, State) {
    let cards: Vec<Card> = feed.visible(Instant::now());
    assert_eq!(cards.len(), 1, "{cards:?}");
    (cards[0].title.clone(), cards[0].detail.clone(), cards[0].state)
}

fn slot(name: &str, kind: &str, sound: Option<i64>) -> KitSlot {
    KitSlot { name: name.into(), types: vec![kind.into()], locked: false, sound }
}

#[test]
fn a_sound_goes_on_a_sampler_and_its_card_says_so() {
    let dir = fixtures("to-bitwig-sound");
    let kick = sound(&dir, "Kick 909.wav");
    let bitwig = FakeBitwig::start().unwrap();
    let feed = Feed::default();

    // The card shows as soon as it's asked for, before Bitwig answers.
    let job = to_bitwig::sound_card(&feed, "Kick 909");
    assert_eq!(card(&feed), ("Putting Kick 909 into Bitwig".into(), SOUND_PLACE.into(), State::Running));
    assert!(to_bitwig::send_sound(job, &bitwig.bitwig(), &kick, "Kick 909"));
    assert_eq!(card(&feed), ("Kick 909 is in Bitwig".into(), SOUND_PLACE.into(), State::Done));

    let world = bitwig.world();
    let track = world.track("Kick 909").unwrap();
    assert_eq!(
        track.devices.iter().map(|d| (d.name.as_str(), d.sample.as_str())).collect::<Vec<_>>(),
        [("Sampler", kick.as_str())]
    );
}

#[test]
fn a_sound_that_didnt_go_in_says_why_on_its_card() {
    let dir = fixtures("to-bitwig-sound-not-in");
    let kick = sound(&dir, "Kick 909.wav");

    // Bitwig quietly didn't load it: its track is there, empty.
    let bitwig = FakeBitwig::start().unwrap();
    bitwig.world().unloadable = vec!["Kick 909.wav".into()];
    let feed = Feed::default();
    assert!(!to_bitwig::send_sound(to_bitwig::sound_card(&feed, "Kick 909"), &bitwig.bitwig(), &kick, "Kick 909"));
    assert_eq!(
        card(&feed),
        ("Kick 909 didn't go in cleanly".into(), "Bitwig didn't load Kick 909.wav.".into(), State::Failed)
    );

    // "Let the app make changes" is off: nothing changes in Bitwig, and the card says where to turn it on.
    let bitwig = FakeBitwig::start().unwrap();
    bitwig.world().changes = false;
    let feed = Feed::default();
    assert!(!to_bitwig::send_sound(to_bitwig::sound_card(&feed, "Kick 909"), &bitwig.bitwig(), &kick, "Kick 909"));
    let (title, why, state) = card(&feed);
    assert_eq!((title.as_str(), state), ("Couldn't put it into Bitwig", State::Failed));
    assert!(why.contains("\"Let the app make changes\""), "{why}");
    assert!(bitwig.world().track("Kick 909").is_none());
}

#[test]
fn with_bitwig_closed_the_card_says_what_to_check() {
    let dir = fixtures("to-bitwig-closed");
    let kick = sound(&dir, "Kick 909.wav");
    // A port nothing listens on: one the system had free, let go again. Another test's stand-in may be given it
    // meanwhile, so another is tried then.
    for _ in 0..10 {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let closed = Bitwig { port, connect_timeout: Duration::from_millis(500), ..Bitwig::default() };
        let feed = Feed::default();
        assert!(!to_bitwig::send_sound(to_bitwig::sound_card(&feed, "Kick 909"), &closed, &kick, "Kick 909"));
        let (title, why, state) = card(&feed);
        assert_eq!((title.as_str(), state), ("Couldn't put it into Bitwig", State::Failed));
        if why.starts_with("Bitwig isn't answering. Check that Bitwig Studio is open") {
            // It stays long enough to read.
            assert_eq!(feed.visible(Instant::now() + mo_app::activity::DONE_STAYS).len(), 1);
            return;
        }
    }
    panic!("something answered on every port tried");
}

#[test]
fn the_kit_on_show_goes_on_a_new_drum_machine() {
    let dir = fixtures("to-bitwig-kit");
    let list = SoundList::new(vec![
        Sound::new(1, &sound(&dir, "Boom Kick.wav")),
        Sound::new(2, &sound(&dir, "Dusty Snare.wav")),
        Sound::new(3, &sound(&dir, "Closed Hat.wav")),
        Sound::new(4, &sound(&dir, "Riser.wav")),
    ]);
    // An empty pad, and a sound no longer in the list, are left out; the rest keep the kit's order.
    let slots = [
        slot("Kick", "Kick", Some(1)),
        slot("Clap", "Clap", None),
        slot("Snare", "Snare", Some(2)),
        slot("Closed hat", "Closed hat", Some(3)),
        slot("FX", "FX", Some(4)),
        slot("Tom", "Tom", Some(99)),
    ];
    let sounds = to_bitwig::kit_sounds(&slots, |id| list.find(id).and_then(|i| list.sound(i)));
    let names: Vec<_> = sounds.iter().map(|s| (s.name.as_str(), s.kind.as_str())).collect();
    assert_eq!(names, [("Kick", "Kick"), ("Snare", "Snare"), ("Closed hat", "Closed hat"), ("FX", "FX")]);
    assert_eq!(sounds[0].path, dir.join("Boom Kick.wav").to_string_lossy());

    let pads = kit_pads(&sounds);
    let bitwig = FakeBitwig::start().unwrap();
    let kits = Kits::new();
    let feed = Feed::default();
    let job = to_bitwig::kit_card(&feed, pads.len());
    assert_eq!(card(&feed), ("Sending the kit to Bitwig".into(), "4 pads".into(), State::Running));
    let line = to_bitwig::send_kit(job, &bitwig.bitwig(), &kits, &kit_name(12), &pads, None);
    let said = "Kit #12 is on a Drum Machine on a new track in Bitwig: Kick on C1, FX on C#1, Snare on D1, Closed hat \
                on F#1.";
    assert_eq!(line, said);
    assert_eq!(card(&feed), ("Kit #12 is in Bitwig".into(), said.into(), State::Done));
    assert!(!kits.on_the_selected());
    let world = bitwig.world();
    let track = world.tracks.iter().position(|t| t.name == "Kit #12").unwrap();
    assert_eq!(world.pads(track).unwrap().keys().copied().collect::<Vec<_>>(), [36, 37, 38, 42]);
}

#[test]
fn once_bitwig_adds_no_drum_machine_kits_go_on_the_selected_one() {
    let dir = fixtures("to-bitwig-kit-selected");
    let pads = kit_pads(&[
        mo_app::to_bitwig::KitSound { name: "Kick".into(), kind: "Kick".into(), path: sound(&dir, "Kick.wav") },
        mo_app::to_bitwig::KitSound { name: "Snare".into(), kind: "Snare".into(), path: sound(&dir, "Snare.wav") },
    ]);
    let bitwig = FakeBitwig::start().unwrap();
    let kits = Kits::new();

    // The ID the C# app kept is used while there is one; Bitwig's own otherwise.
    assert_eq!(kits.target(None), KitTarget::NewDrumMachine(DeviceId::DRUM_MACHINE));
    // An ID that isn't the Drum Machine's (here the Sampler's) adds something else.
    let wrong = DeviceId::SAMPLER.to_string();
    assert_eq!(kits.target(Some(&wrong)), KitTarget::NewDrumMachine(DeviceId::SAMPLER));
    let feed = Feed::default();
    let line =
        to_bitwig::send_kit(to_bitwig::kit_card(&feed, 2), &bitwig.bitwig(), &kits, "Kit #4", &pads, Some(&wrong));
    assert_eq!(
        line,
        "Bitwig didn't add a Drum Machine. Add one to a track yourself, select it, and send the kit again."
    );
    assert_eq!(
        card(&feed),
        (
            "Bitwig didn't add a Drum Machine".into(),
            "Add one to a track yourself, select it, and send the kit again.".into(),
            State::Failed
        )
    );

    // From then on, the selected one, whatever the setting says.
    assert!(kits.on_the_selected());
    assert_eq!(kits.target(None), KitTarget::Selected);
    {
        let mut world = bitwig.world();
        let bass = world.tracks.iter().position(|t| t.name == "Bass").unwrap();
        world.selected_device = Some((bass, 0));
    }
    let feed = Feed::default();
    let line = to_bitwig::send_kit(to_bitwig::kit_card(&feed, 2), &bitwig.bitwig(), &kits, "Kit #4", &pads, None);
    assert_eq!(line, "The selected device isn't a Drum Machine. Select one and send the kit again.");
    assert_eq!(card(&feed), ("Couldn't send the kit".into(), line.clone(), State::Failed));

    // Once one is selected, the kit goes on it.
    {
        let mut world = bitwig.world();
        let drum_machine = FakeDevice { pads: Some(BTreeMap::new()), ..FakeDevice::new("Drum Machine", "instrument") };
        let mut drums = FakeTrack::new("Drums 2", "Instrument", vec![drum_machine]);
        drums.id = "test-drums".into();
        world.tracks.insert(0, drums);
        world.selected_device = Some((0, 0));
    }
    let feed = Feed::default();
    let line = to_bitwig::send_kit(to_bitwig::kit_card(&feed, 2), &bitwig.bitwig(), &kits, "Kit #4", &pads, None);
    assert_eq!(line, "Kit #4 is on the Drum Machine selected in Bitwig: Kick on C1, Snare on D1.");
    assert_eq!(card(&feed).0, "Kit #4 is in Bitwig");
    assert_eq!(bitwig.world().pads(0).unwrap().keys().copied().collect::<Vec<_>>(), [36, 38]);
}
