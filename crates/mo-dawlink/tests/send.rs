//! Send to Bitwig, against the stand-in Bitwig (`mo_dawlink::bitwig::fake`): no test reaches a real Bitwig, and none
//! uses the link's own port. The sounds are files made in the test build's own fixtures folder under fixed names,
//! never a random temp one.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;

use mo_dawlink::bitwig::fake::{CHANGES_OFF, FakeBitwig, FakeDevice, World};
use mo_dawlink::bitwig::{
    DRUM_MACHINE_KEY, DeviceId, Error, KIT_FAILED, KitSound, KitTarget, Pad, PadContents, SOUND_FAILED, SOUND_PLACE,
    drum_machine, kit_name, kit_pads, note_name, sending_kit, sending_sound,
};
use serde_json::json;

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A sound file: what's in it doesn't matter to the link, only that it's there.
fn sound(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, b"RIFF\0\0\0\0WAVE").unwrap();
    path
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn with(change: impl FnOnce(&mut World)) -> FakeBitwig {
    let mut world = World::default();
    change(&mut world);
    FakeBitwig::with(world).unwrap()
}

fn kit_sound(name: &str, kind: &str, path: &Path) -> KitSound {
    KitSound { name: name.into(), kind: kind.into(), path: text(path) }
}

fn notes(pads: &[Pad]) -> Vec<u8> {
    pads.iter().map(|p| p.note).collect()
}

#[test]
fn puts_a_sound_on_a_sampler_on_a_new_track() {
    let dir = fixtures("dawlink-sound");
    let kick = sound(&dir, "Kick 909.wav");
    let bitwig = FakeBitwig::start().unwrap();
    let sent = bitwig.bitwig().send_sound(&kick, "Kick 909").unwrap();
    assert!(sent.went_in());
    assert_eq!((sent.title(), sent.detail()), ("Kick 909 is in Bitwig".to_owned(), SOUND_PLACE.to_owned()));
    assert_eq!(sending_sound("Kick 909"), "Putting Kick 909 into Bitwig");

    let world = bitwig.world();
    let track = &world.tracks[sent.track];
    assert_eq!((track.name.as_str(), track.kind.as_str()), ("Kick 909", "Instrument"));
    assert_eq!(track.devices.len(), 1);
    assert_eq!((track.devices[0].name.as_str(), track.devices[0].sample.as_str()), ("Sampler", text(&kick).as_str()));
    // Only those two changes, each kept to the project open as it began.
    assert_eq!(world.commands(), ["newtrack", "insert"]);
    assert!(world.requests.iter().all(|r| r["onlyIn"] == "Night Drive"));
}

#[test]
fn says_when_the_sound_didnt_go_on_a_sampler() {
    let dir = fixtures("dawlink-sound-not-loaded");
    let kick = sound(&dir, "Kick 909.wav");

    // Bitwig quietly didn't load it: the track is there, empty.
    let bitwig = with(|w| w.unloadable = vec!["Kick 909.wav".into()]);
    let sent = bitwig.bitwig().send_sound(&kick, "Kick 909").unwrap();
    assert!(!sent.went_in());
    assert_eq!(sent.title(), "Kick 909 didn't go in cleanly");
    assert_eq!(sent.detail(), "Bitwig didn't load Kick 909.wav.");
    assert!(bitwig.world().tracks[sent.track].devices.is_empty());

    // It went on something else.
    let bitwig = with(|w| w.sample_device = "Polymer".into());
    let sent = bitwig.bitwig().send_sound(&kick, "Kick 909").unwrap();
    assert_eq!(sent.detail(), "Bitwig put it on Polymer rather than a Sampler.");

    // The connection went once the track was made: the track stays, and the words say the sound may not be on it.
    let bitwig = with(|w| w.hang_up_on = vec!["insert".into()]);
    let sent = bitwig.bitwig().send_sound(&kick, "Kick 909").unwrap();
    assert_eq!(sent.title(), "Kick 909 didn't go in cleanly");
    assert_eq!(sent.detail(), "The track is in Bitwig, but the sound may not be on it: Bitwig closed the connection.");
    assert_eq!(bitwig.world().tracks[sent.track].name, "Kick 909");
}

#[test]
fn a_sound_that_isnt_there_any_more_isnt_sent() {
    let dir = fixtures("dawlink-sound-gone");
    let gone = dir.join("Gone.wav");
    let bitwig = FakeBitwig::start().unwrap();
    let err = bitwig.bitwig().send_sound(&gone, "Gone").unwrap_err();
    assert_eq!(err, Error::Missing { file: "Gone.wav".into(), path: text(&gone) });
    assert_eq!(err.to_string(), format!("Gone.wav isn't at {} any more.", text(&gone)));
    assert!(bitwig.world().requests.is_empty());
    assert_eq!(SOUND_FAILED, "Couldn't put it into Bitwig");
}

#[test]
fn a_project_brought_to_the_front_stops_the_send_there() {
    let dir = fixtures("dawlink-sound-switch");
    let kick = sound(&dir, "Kick.wav");
    // Part way through making the track: nothing goes in the other project, and no track index is known.
    let bitwig = with(|w| w.switch_during = Some(("newtrack".into(), "Other Song".into())));
    let err = bitwig.bitwig().send_sound(&kick, "Kick").unwrap_err();
    assert!(err.is_elsewhere());
    assert!(!Error::Bitwig("Bitwig has \"x\" open.".into()).is_elsewhere() && !Error::TimedOut.is_elsewhere());
    assert_eq!(
        err,
        Error::Bitwig(
            "Bitwig has \"Other Song\" open, not \"Night Drive\", so the link stopped before changing anything there."
                .into()
        )
    );
    // Part way through putting the sound on its track: the track is in Bitwig, and the words say so.
    let bitwig = with(|w| w.switch_during = Some(("insert".into(), "Other Song".into())));
    let sent = bitwig.bitwig().send_sound(&kick, "Kick").unwrap();
    assert!(
        sent.detail().starts_with("The track is in Bitwig, but the sound may not be on it: Bitwig has \"Other Song\"")
    );
}

#[test]
fn wont_send_through_an_old_script_or_with_changes_off() {
    let dir = fixtures("dawlink-wont-send");
    let kick = sound(&dir, "Kick.wav");
    let pads = [Pad { note: 36, path: text(&kick), name: "Kick".into() }];
    let old = with(|w| _ = w.hello.insert("link".into(), json!(1)));
    let off = with(|w| w.changes = false);
    let broken = with(|w| _ = w.hello.insert("error".into(), json!("The Bitwig extension failed: x.")));
    let cases = [
        (old, Error::OldLink),
        (off, Error::ChangesOff),
        (broken, Error::NotStarted("The Bitwig extension failed: x.".into())),
    ];
    for (bitwig, error) in cases {
        assert_eq!(bitwig.bitwig().send_sound(&kick, "Kick").unwrap_err(), error);
        let target = KitTarget::NewDrumMachine(DeviceId::DRUM_MACHINE);
        assert_eq!(bitwig.bitwig().send_kit("Kit", &pads, target).unwrap_err(), error);
        assert_eq!(bitwig.bitwig().send_kit("Kit", &pads, KitTarget::Selected).unwrap_err(), error);
        // Nothing reached Bitwig.
        assert!(bitwig.world().requests.is_empty(), "{error}");
    }
    // The C# app's words for changes off, which Solomon has seen there.
    assert_eq!(
        Error::ChangesOff.to_string(),
        "Bitwig isn't letting the app make changes. Turn on \"Let the app make changes\" in Bitwig (Settings > \
         Controllers > Music Organiser), then send it again."
    );
    // A link that turned changes off after greeting says so in its words.
    let bitwig = FakeBitwig::start().unwrap();
    let mut link = bitwig.bitwig().connect().unwrap();
    bitwig.world().changes = false;
    let err = mo_dawlink::bitwig::sound(&mut link, &kick, "Kick").unwrap_err();
    assert_eq!(err, Error::Bitwig(CHANGES_OFF.into()));
}

/// The C# app's `PutsAKitOnADrumMachine`, on the stand-in: a new Drum Machine on a new track, then the same kit and
/// a crash on the one selected in Bitwig, which leaves the pads that have sounds alone.
#[test]
fn puts_a_kit_on_a_drum_machine() {
    let dir = fixtures("dawlink-kit");
    let pads = kit_pads(&[
        kit_sound("Kick", "Kick", &sound(&dir, "Kick.wav")),
        kit_sound("Snare", "Snare", &sound(&dir, "Snare.wav")),
        kit_sound("Closed hat", "Closed hat", &sound(&dir, "Hat.wav")),
        kit_sound("FX", "FX", &sound(&dir, "Riser.wav")),
    ]);
    let bitwig = FakeBitwig::start().unwrap();
    let kit = bitwig.bitwig().send_kit("Kit #12", &pads, KitTarget::NewDrumMachine(drum_machine(None))).unwrap();
    assert!(kit.drum_machine);
    assert!(kit.went_in());
    assert_eq!(notes(&kit.filled), [36, 37, 38, 42]);
    assert!(kit.skipped.is_empty() && kit.failed.is_empty() && kit.missing.is_empty());
    assert_eq!(kit.title(), "Kit #12 is in Bitwig");
    assert_eq!(
        kit.detail(),
        "Kit #12 is on a Drum Machine on a new track in Bitwig: Kick on C1, FX on C#1, Snare on D1, Closed hat on F#1."
    );
    {
        let world = bitwig.world();
        let track = &world.tracks[kit.track.unwrap()];
        assert_eq!(track.name, "Kit #12");
        assert_eq!(track.devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Drum Machine"]);
        let on = world.pads(kit.track.unwrap()).unwrap();
        assert_eq!(
            on.iter().map(|(n, p)| (*n, p.name.as_str())).collect::<Vec<_>>(),
            [(36, "Kick"), (37, "FX"), (38, "Snare"), (42, "Closed hat")]
        );
        assert_eq!(on[&42].devices[0].sample, text(&dir.join("Hat.wav")));
    }

    // Without the Drum Machine's ID, the kit goes on the one selected in Bitwig, leaving its sounds alone.
    let mut more = pads.clone();
    more.push(Pad { note: 49, path: text(&sound(&dir, "Crash.wav")), name: "Cymbal".into() });
    let again = bitwig.bitwig().send_kit("Kit #13", &more, KitTarget::Selected).unwrap();
    assert_eq!(again.track, None);
    assert_eq!(notes(&again.filled), [49]);
    assert_eq!(notes(&again.skipped), [36, 37, 38, 42]);
    assert_eq!(again.title(), "Kit #13 is in Bitwig");
    assert_eq!(
        again.detail(),
        "Kit #13 is on the Drum Machine selected in Bitwig: Cymbal on C#2. The pads for Kick (C1), FX (C#1), Snare (D1), \
         Closed hat (F#1) already had sounds, so they were left alone."
    );

    // The link says what's on a pad it leaves alone.
    let mut link = bitwig.bitwig().connect().unwrap();
    let kick =
        link.fill_pads(&[Pad { note: 36, path: text(&dir.join("Kick 2.wav")), name: String::new() }], None).unwrap();
    let left =
        PadContents { note: 36, name: "Kick".into(), devices: 1, first: "Sampler".into(), sample: "Kick.wav".into() };
    assert_eq!(kick.taken, [left]);
    assert!(kick.replaced.is_empty() && kick.filled.is_empty());
}

#[test]
fn a_drum_machine_with_empty_samplers_on_its_pads_takes_the_kit() {
    // Solomon's default Drum Machine has an empty Sampler on every pad: those pads count as free.
    let dir = fixtures("dawlink-kit-presets");
    let pads = kit_pads(&[
        kit_sound("Kick", "Kick", &sound(&dir, "Kick.wav")),
        kit_sound("Snare", "Snare", &sound(&dir, "Snare.wav")),
    ]);
    let bitwig = with(|w| w.drum_pads = vec![FakeDevice::sampler(""), FakeDevice::new("EQ+", "audio-effect")]);
    let kit = bitwig.bitwig().send_kit("Kit #3", &pads, KitTarget::NewDrumMachine(DeviceId::DRUM_MACHINE)).unwrap();
    assert!(kit.went_in());
    assert_eq!(notes(&kit.filled), [36, 38]);
    let world = bitwig.world();
    let on = world.pads(kit.track.unwrap()).unwrap();
    // The sample went in the empty Sampler's place, keeping the EQ after it.
    assert_eq!(on[&36].name, "Kick");
    assert_eq!(on[&36].devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Sampler", "EQ+"]);
    assert_eq!(on[&36].devices[0].sample, text(&dir.join("Kick.wav")));
    // The other pads are as the preset left them.
    assert_eq!((on[&37].name.as_str(), on[&37].devices[0].sample.as_str()), ("Sampler", ""));
}

#[test]
fn says_when_bitwig_adds_no_drum_machine() {
    let dir = fixtures("dawlink-kit-no-drum-machine");
    let pads = kit_pads(&[kit_sound("Kick", "Kick", &sound(&dir, "Kick.wav"))]);
    // An ID Bitwig doesn't know adds nothing; one read wrongly (here the Sampler's) adds some other device.
    for id in [DeviceId::from_bytes([7; 16]), DeviceId::SAMPLER] {
        let bitwig = FakeBitwig::start().unwrap();
        let kit = bitwig.bitwig().send_kit("Kit #4", &pads, KitTarget::NewDrumMachine(id)).unwrap();
        assert!(!kit.drum_machine && !kit.went_in(), "{id}");
        assert_eq!(notes(&kit.failed), [36]);
        assert_eq!(kit.title(), "Bitwig didn't add a Drum Machine");
        assert_eq!(kit.detail(), "Add one to a track yourself, select it, and send the kit again.");
        // Its track stays, and no pads were sent.
        assert_eq!(bitwig.world().commands(), ["newtrack", "insert"]);
    }

    // On the selected device, when it isn't a Drum Machine (the stand-in has Serum selected), Bitwig's words say so.
    let bitwig = FakeBitwig::start().unwrap();
    let err = bitwig.bitwig().send_kit("Kit #4", &pads, KitTarget::Selected).unwrap_err();
    assert_eq!(err.to_string(), "The selected device isn't a Drum Machine. Select one and send the kit again.");
    bitwig.world().selected_device = None;
    let err = bitwig.bitwig().send_kit("Kit #4", &pads, KitTarget::Selected).unwrap_err();
    assert_eq!(err.to_string(), "No device is selected in Bitwig. Select a Drum Machine and send the kit again.");
}

#[test]
fn a_kit_says_which_pads_didnt_go_on() {
    let dir = fixtures("dawlink-kit-part");
    let mut sounds = vec![
        kit_sound("Kick", "Kick", &sound(&dir, "Kick.wav")),
        kit_sound("Snare", "Snare", &sound(&dir, "Snare.wav")),
    ];
    // One Bitwig doesn't load, and one that isn't there any more.
    sounds.push(kit_sound("Clap", "Clap", &sound(&dir, "Clap.wav")));
    sounds.push(kit_sound("Open hat", "Open hat", &dir.join("Gone.wav")));
    let pads = kit_pads(&sounds);
    let bitwig = with(|w| w.unloadable = vec!["Clap.wav".into()]);
    let kit = bitwig.bitwig().send_kit("Kit #5", &pads, KitTarget::NewDrumMachine(DeviceId::DRUM_MACHINE)).unwrap();
    assert!(!kit.went_in());
    assert_eq!((notes(&kit.filled), notes(&kit.failed), notes(&kit.missing)), (vec![36, 38], vec![39], vec![46]));
    assert_eq!(kit.title(), "Kit #5 didn't go in cleanly");
    assert_eq!(
        kit.detail(),
        "Kit #5 is on a Drum Machine on a new track in Bitwig: Kick on C1, Snare on D1. Open hat (A#1) isn't where the \
         app last saw it, so it was left out. Bitwig didn't load Clap (D#1)."
    );
    // The missing one wasn't sent.
    let world = bitwig.world();
    let sent = world.requests.iter().find(|r| r["cmd"] == "pads").unwrap();
    assert_eq!(sent["pads"].as_array().unwrap().len(), 3);

    // None there any more: nothing is sent.
    let gone = kit_pads(&[kit_sound("Kick", "Kick", &dir.join("Gone.wav"))]);
    let bitwig = FakeBitwig::start().unwrap();
    let err = bitwig.bitwig().send_kit("Kit #6", &gone, KitTarget::Selected).unwrap_err();
    assert_eq!(err.to_string(), format!("Gone.wav isn't at {} any more.", text(&dir.join("Gone.wav"))));
    assert!(bitwig.world().requests.is_empty());
    assert_eq!(bitwig.bitwig().send_kit("Kit", &[], KitTarget::Selected).unwrap_err(), Error::NoPads);
    assert_eq!(Error::NoPads.to_string(), "Roll a kit first.");
}

#[test]
fn a_kit_cut_short_says_what_may_have_happened() {
    let dir = fixtures("dawlink-kit-cut");
    let pads = kit_pads(&[kit_sound("Kick", "Kick", &sound(&dir, "Kick.wav"))]);
    let target = KitTarget::NewDrumMachine(DeviceId::DRUM_MACHINE);

    let bitwig = with(|w| w.hang_up_on = vec!["pads".into()]);
    let kit = bitwig.bitwig().send_kit("Kit #7", &pads, target).unwrap();
    assert!(kit.drum_machine && !kit.went_in());
    assert!(kit.filled.is_empty() && kit.failed.is_empty());
    assert_eq!(kit.title(), "Kit #7 didn't go in cleanly");
    assert_eq!(
        kit.detail(),
        "The Drum Machine is in Bitwig, but the sounds may not be on it: Bitwig closed the connection."
    );

    let bitwig = with(|w| w.hang_up_on = vec!["insert".into()]);
    let kit = bitwig.bitwig().send_kit("Kit #7", &pads, target).unwrap();
    assert_eq!(
        kit.detail(),
        "The kit's track is in Bitwig, but the Drum Machine may not be on it: Bitwig closed the connection."
    );

    // On the selected Drum Machine, after the app's request went: the sounds may be on it.
    let bitwig = FakeBitwig::start().unwrap();
    bitwig.bitwig().send_kit("Kit #7", &pads, target).unwrap();
    bitwig.world().hang_up_on = vec!["pads".into()];
    let kit = bitwig.bitwig().send_kit("Kit #8", &pads, KitTarget::Selected).unwrap();
    assert_eq!(
        kit.detail(),
        "The sounds may not be on the Drum Machine selected in Bitwig: Bitwig closed the connection."
    );
}

#[test]
fn sends_made_at_once_each_go_in() {
    let dir = fixtures("dawlink-at-once");
    let files: Vec<PathBuf> = (1..=4).map(|i| sound(&dir, &format!("Hit {i}.wav"))).collect();
    let bitwig = FakeBitwig::start().unwrap();
    let place = bitwig.bitwig();
    thread::scope(|s| {
        let sends: Vec<_> = files
            .iter()
            .enumerate()
            .map(|(i, f)| s.spawn(move || place.send_sound(f, &format!("Hit {}", i + 1))))
            .collect();
        for send in sends {
            assert!(send.join().unwrap().unwrap().went_in());
        }
    });
    let world = bitwig.world();
    let mut names: Vec<&str> = world.tracks.iter().map(|t| t.name.as_str()).filter(|n| n.starts_with("Hit")).collect();
    names.sort_unstable();
    assert_eq!(names, ["Hit 1", "Hit 2", "Hit 3", "Hit 4"]);
    // Each on its own track.
    assert!(world.tracks.iter().filter(|t| t.name.starts_with("Hit")).all(|t| t.devices.len() == 1));
}

/// The C# app's `PlacesAKitTheGeneralMidiWay`.
#[test]
fn places_a_kit_the_general_midi_way() {
    let kit: Vec<KitSound> = mo_sounds::kits::default_drum_kit()
        .iter()
        .map(|s| KitSound { name: s.name.clone(), kind: s.types[0].clone(), path: format!("{}.wav", s.name) })
        .collect();
    let pads = kit_pads(&kit);
    let placed: Vec<(u8, &str)> = pads.iter().map(|p| (p.note, p.name.as_str())).collect();
    assert_eq!(
        placed,
        [
            (36, "Kick"),
            (37, "Percussion"),
            (38, "Snare"),
            (39, "Clap"),
            (40, "Percussion 2"),
            (41, "808 / Bass"),
            (42, "Closed hat"),
            (43, "FX"),
            (46, "Open hat"),
            (49, "Cymbal"),
        ]
    );

    // A second kick can't share the first's note, so it takes the next free one; a sound with no kind goes by its name.
    let two = kit_pads(&[
        KitSound { name: "Kick".into(), kind: "Kick".into(), path: "a.wav".into() },
        KitSound { name: "Kick 2".into(), kind: "Kick".into(), path: "b.wav".into() },
        KitSound { name: "Tom".into(), kind: String::new(), path: "c.wav".into() },
    ]);
    assert_eq!(
        two.iter().map(|p| (p.note, p.name.as_str())).collect::<Vec<_>>(),
        [(36, "Kick"), (37, "Kick 2"), (45, "Tom")]
    );
}

#[test]
fn a_kit_bigger_than_the_notes_leaves_the_rest_out() {
    let many: Vec<KitSound> = (0..100)
        .map(|i| KitSound { name: format!("Hit {i}"), kind: String::new(), path: format!("{i}.wav") })
        .collect();
    let pads = kit_pads(&many);
    // From C1 (36) to G8 (127).
    assert_eq!(pads.len(), 92);
    assert_eq!((pads[0].note, pads[91].note), (36, 127));
}

#[test]
fn words_for_the_card() {
    assert_eq!(kit_name(12), "Kit #12");
    assert_eq!(kit_name(0), "Kit");
    assert_eq!(sending_kit(10), ("Sending the kit to Bitwig".to_owned(), "10 pads".to_owned()));
    assert_eq!(sending_kit(1).1, "1 pad");
    assert_eq!(KIT_FAILED, "Couldn't send the kit");
    let names: Vec<String> = [36, 60, 39, 49, 0, 127].into_iter().map(note_name).collect();
    assert_eq!(names, ["C1", "C3", "D#1", "C#2", "C-2", "G8"]);
}

#[test]
fn picks_the_drum_machine_the_csharp_app_found() {
    assert_eq!(DRUM_MACHINE_KEY, "bitwig_drum_machine");
    // Nothing kept, or "" (the C# app's mark for an ID that didn't give a Drum Machine): Bitwig's own.
    assert_eq!(drum_machine(None), DeviceId::DRUM_MACHINE);
    assert_eq!(drum_machine(Some("")), DeviceId::DRUM_MACHINE);
    assert_eq!(drum_machine(Some("not an id")), DeviceId::DRUM_MACHINE);
    // What the C# app read from Bitwig's files, as .NET wrote it.
    let found = drum_machine(Some("D7D7D7D7-0000-4000-8000-000000000001"));
    assert_eq!(found.to_string(), "d7d7d7d7-0000-4000-8000-000000000001");
    assert_eq!(drum_machine(Some("{d7d7d7d7-0000-4000-8000-000000000001}")), found);
    assert_eq!(drum_machine(Some("d7d7d7d7000040008000000000000001")), found);
    assert_eq!(DeviceId::DRUM_MACHINE.to_string(), "8ea97e45-0255-40fd-bc7e-94419741e9d1");
    assert_eq!(DeviceId::SAMPLER.to_string(), "468bc14b-b2e7-45a1-9666-e83117fe404e");
    assert_eq!(DeviceId::NOTE_TRANSPOSE.to_string(), "0815cd9e-3a31-4429-a268-dabd952a3b68");
    // Not IDs: a "+" where a digit should be, hyphens in the wrong places.
    assert_eq!(DeviceId::parse("+7d7d7d7-0000-4000-8000-000000000001"), None);
    assert_eq!(DeviceId::parse("d7d7d7d70-000-4000-8000-000000000001"), None);
    assert_eq!(DeviceId::parse("d7d7d7d7-0000-4000-8000-00000000000"), None);
    assert_eq!(DeviceId::parse(&DeviceId::SAMPLER.to_string()), Some(DeviceId::SAMPLER));
    assert_eq!(DeviceId::from_bytes(DeviceId::SAMPLER.bytes()), DeviceId::SAMPLER);
}
