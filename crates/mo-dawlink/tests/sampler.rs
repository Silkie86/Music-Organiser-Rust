//! A sound tried in the selected Sampler with its settings kept (extension 3.29), against the stand-in Bitwig.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use mo_dawlink::bitwig::fake::{FakeBitwig, FakeDevice, FakeParam, SelectOn, World};
use mo_dawlink::bitwig::{Error, Swapped};

fn turn() -> mo_dawlink::bitwig::Turn<'static> {
    mo_dawlink::bitwig::ticket().wait()
}

fn sample(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sampler-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.wav"));
    fs::write(&path, b"RIFF\0\0\0\0WAVE").unwrap();
    path
}

/// Bitwig with extension 3.29, a Sampler selected on Bass, set up with three settings.
fn with_sampler(change: impl FnOnce(&mut World)) -> FakeBitwig {
    let mut world = World::default();
    let mut sampler = FakeDevice::sampler("C:\\Samples\\Old.wav");
    sampler.params = vec![
        FakeParam::new("CONTENTS/GAIN", "Gain", 0.8),
        FakeParam::new("CONTENTS/PITCH", "Pitch", 0.5),
        FakeParam::new("CONTENTS/RELEASE", "Release", 0.3),
        FakeParam { value: None, ..FakeParam::new("CONTENTS/MOD", "Mod", 0.0) },
    ];
    world.tracks[2].devices[0] = sampler;
    // A new Sampler comes with its defaults, and no Release.
    world.new_sampler = vec![
        FakeParam::new("CONTENTS/GAIN", "Gain", 0.5),
        FakeParam::new("CONTENTS/PITCH", "Pitch", 0.5),
        FakeParam::new("CONTENTS/MOD", "Mod", 0.0),
    ];
    change(&mut world);
    FakeBitwig::with(world).unwrap()
}

#[test]
fn the_sound_goes_in_the_samplers_place_with_its_settings_set_back() {
    let bitwig = with_sampler(|_| {});
    let file = sample("Kick 04");
    let swapped = bitwig.bitwig().swap_into_sampler(&file, &turn()).unwrap();
    assert_eq!(
        swapped,
        Swapped {
            track: "Bass".into(),
            carried: vec!["Pitch".into()],
            set_back: vec!["Gain".into()],
            didnt_take: Vec::new(),
            gone: vec!["Release".into()],
            unread: vec!["Mod".into()],
            same_sound: false,
            trouble: None,
        }
    );
    let world = bitwig.world();
    let now = &world.tracks[2].devices[0];
    assert_eq!(now.sample, file.to_string_lossy());
    assert_eq!(now.params[0], FakeParam::new("CONTENTS/GAIN", "Gain", 0.8));
    assert!(swapped.summary().contains("Not on the new one: Release."), "{}", swapped.summary());
}

#[test]
fn a_setting_bitwig_wont_change_is_said() {
    let bitwig = with_sampler(|w| w.fixed_params = vec!["CONTENTS/GAIN".into()]);
    let swapped = bitwig.bitwig().swap_into_sampler(sample("Snare"), &turn()).unwrap();
    assert_eq!(swapped.didnt_take, ["Gain"]);
    assert!(swapped.set_back.is_empty());
}

#[test]
fn only_a_selected_sampler_and_a_new_enough_extension() {
    // Serum is selected: nothing changes.
    let bitwig = with_sampler(|w| w.tracks[2].devices[0] = FakeDevice::new("Serum", "instrument"));
    let said = bitwig.bitwig().swap_into_sampler(sample("Hat"), &turn()).unwrap_err();
    assert!(matches!(&said, Error::Invalid(why) if why.contains("not a Sampler")), "{said:?}");
    assert_eq!(bitwig.world().tracks[2].devices[0].name, "Serum");

    let bitwig = with_sampler(|w| w.selected_device = None);
    let said = bitwig.bitwig().swap_into_sampler(sample("Hat"), &turn()).unwrap_err();
    assert!(matches!(&said, Error::Invalid(why) if why.contains("Select a Sampler")), "{said:?}");

    let bitwig = with_sampler(|w| {
        w.hello.insert("extension".into(), "3.28".into());
    });
    let said = bitwig.bitwig().swap_into_sampler(sample("Hat"), &turn()).unwrap_err();
    assert!(matches!(said, Error::OldExtension { needs: "3.29", .. }), "{said:?}");

    let bitwig = with_sampler(|_| {});
    let gone = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("sampler-none").join("Gone.wav");
    let said = bitwig.bitwig().swap_into_sampler(gone, &turn()).unwrap_err();
    assert!(matches!(said, Error::Missing { .. }), "{said:?}");
}

/// The review's BW-1: a file Bitwig quietly ignores is said as that, not as a sound that went in with every setting
/// "come over by itself".
#[test]
fn a_file_bitwig_ignores_is_said_and_nothing_changes() {
    let bitwig = with_sampler(|w| w.unloadable = vec!["Odd.wav".into()]);
    let said = bitwig.bitwig().swap_into_sampler(sample("Odd"), &turn()).unwrap_err();
    assert!(
        matches!(&said, Error::Bitwig(why) if why.starts_with("Bitwig didn't take Odd.wav: nothing changed.")),
        "{said:?}"
    );
    let world = bitwig.world();
    assert_eq!(world.tracks[2].devices[0].sample, "C:\\Samples\\Old.wav");
    assert_eq!(world.tracks[2].devices[0].params[0], FakeParam::new("CONTENTS/GAIN", "Gain", 0.8));
    assert!(!world.requests.iter().any(|r| r["cmd"] == "set"), "nothing is set on a Sampler that wasn't swapped");
}

/// The review's BW-3: the swap waited its turn in Bitwig, and Serum on Lead was selected by then. It's left alone.
#[test]
fn a_device_selected_while_the_swap_waited_is_left_alone() {
    let bitwig = with_sampler(|w| {
        w.tracks[1].devices = vec![FakeDevice::new("Serum", "instrument")];
        w.select_on = Some(SelectOn { command: "insert".into(), after: 0, device: (1, 0) });
    });
    let said = bitwig.bitwig().swap_into_sampler(sample("Kick"), &turn()).unwrap_err();
    assert!(matches!(&said, Error::Bitwig(why) if why.contains("nothing was replaced")), "{said:?}");
    let world = bitwig.world();
    assert_eq!(world.tracks[1].devices[0].name, "Serum");
    assert_eq!(world.tracks[2].devices[0].sample, "C:\\Samples\\Old.wav");
    // The request says which device it means, and stays in the project it began in.
    let insert = world.requests.iter().find(|r| r["cmd"] == "insert").unwrap();
    assert_eq!(
        insert["expect"],
        serde_json::json!({ "trackId": world.tracks[2].id, "position": 0, "name": "Sampler" })
    );
    assert_eq!(insert["onlyIn"], "Night Drive");
}

/// Another Sampler selected by then is left alone too: it's told from the first by its track and place.
#[test]
fn another_sampler_selected_while_the_swap_waited_is_left_alone() {
    let bitwig = with_sampler(|w| {
        let mut other = FakeDevice::sampler("C:\\Samples\\Other.wav");
        other.params = vec![FakeParam::new("CONTENTS/GAIN", "Gain", 0.1)];
        w.tracks[1].devices = vec![other];
        w.select_on = Some(SelectOn { command: "insert".into(), after: 0, device: (1, 0) });
    });
    let said = bitwig.bitwig().swap_into_sampler(sample("Kick"), &turn()).unwrap_err();
    assert!(matches!(&said, Error::Bitwig(why) if why.contains("nothing was replaced")), "{said:?}");
    assert_eq!(bitwig.world().tracks[1].devices[0].sample, "C:\\Samples\\Other.wav");
}

/// Once the sound is in, a selection that moves is told as trouble with the settings, not as "didn't go in", and the
/// old Sampler's settings never land on the other device.
#[test]
fn a_selection_that_moves_after_the_swap_is_told_with_the_sound_in() {
    // Before the settings are compared: nothing is set anywhere.
    let other = || {
        let mut other = FakeDevice::sampler("C:\\Samples\\Other.wav");
        other.params = vec![FakeParam::new("CONTENTS/GAIN", "Gain", 0.1)];
        other
    };
    let bitwig = with_sampler(|w| {
        w.tracks[1].devices = vec![other()];
        w.select_on = Some(SelectOn { command: "device".into(), after: 1, device: (1, 0) });
    });
    let file = sample("Kick");
    let swapped = bitwig.bitwig().swap_into_sampler(&file, &turn()).unwrap();
    assert_eq!(swapped.trouble.as_deref(), Some("another device is selected in Bitwig now."));
    assert!(swapped.summary().contains("weren't all set back"), "{}", swapped.summary());
    let world = bitwig.world();
    assert_eq!(world.tracks[2].devices[0].sample, file.to_string_lossy());
    assert_eq!(world.tracks[1].devices[0].params[0], FakeParam::new("CONTENTS/GAIN", "Gain", 0.1));
    assert!(!world.requests.iter().any(|r| r["cmd"] == "set"));

    // As the first setting is set: Serum has no such setting, the set fails, and that's told.
    let bitwig = with_sampler(|w| {
        w.tracks[1].devices = vec![FakeDevice::new("Serum", "instrument")];
        w.select_on = Some(SelectOn { command: "set".into(), after: 0, device: (1, 0) });
    });
    let swapped = bitwig.bitwig().swap_into_sampler(sample("Kick"), &turn()).unwrap();
    assert!(swapped.trouble.as_deref().is_some_and(|t| t.contains("no parameter")), "{swapped:?}");
    assert_eq!(swapped.didnt_take, ["Gain"]);
    assert!(swapped.set_back.is_empty());
}

/// The sound the Sampler holds already, tried again: Bitwig can't show the swap, and the report says so.
#[test]
fn the_same_sound_again_is_said_as_not_knowable() {
    let file = sample("Same");
    let path = file.to_string_lossy().into_owned();
    let bitwig = with_sampler(|w| w.tracks[2].devices[0].sample = path);
    let swapped = bitwig.bitwig().swap_into_sampler(&file, &turn()).unwrap();
    assert!(swapped.same_sound);
    assert!(swapped.summary().contains("held this sound already"), "{}", swapped.summary());
}
