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
            unknown: false,
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
    let said = bitwig.bitwig().swap_into_sampler(sample("Kick 1"), &turn()).unwrap_err();
    assert!(matches!(&said, Error::Bitwig(why) if why.contains("nothing was replaced")), "{said:?}");
    let world = bitwig.world();
    assert_eq!(world.tracks[1].devices[0].name, "Serum");
    assert_eq!(world.tracks[2].devices[0].sample, "C:\\Samples\\Old.wav");
    // The request says which device it means, and stays in the project it began in.
    let insert = world.requests.iter().find(|r| r["cmd"] == "insert").unwrap();
    assert_eq!(
        insert["expect"],
        serde_json::json!({ "trackId": world.tracks[2].id, "position": 0, "name": "Sampler", "sample": "Old.wav" })
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
    let said = bitwig.bitwig().swap_into_sampler(sample("Kick 2"), &turn()).unwrap_err();
    assert!(matches!(&said, Error::Bitwig(why) if why.contains("nothing was replaced")), "{said:?}");
    assert_eq!(bitwig.world().tracks[1].devices[0].sample, "C:\\Samples\\Other.wav");
}

/// Once the sound is in, a selection that moves is told as trouble with the settings, not as "didn't go in", and the
/// old Sampler's settings never land on the other device.
#[test]
fn a_selection_that_moves_after_the_swap_is_told_with_the_sound_in() {
    // Another Sampler, with the same settings' IDs: a set would go through on it.
    let other = || {
        let mut other = FakeDevice::sampler("C:\\Samples\\Other.wav");
        other.params = vec![FakeParam::new("CONTENTS/GAIN", "Gain", 0.1)];
        other
    };
    // Before the settings are compared, and as the first one is set: nothing lands on the other Sampler.
    for (command, after) in [("device", 1), ("set", 0)] {
        let bitwig = with_sampler(|w| {
            w.tracks[1].devices = vec![other()];
            w.select_on = Some(SelectOn { command: command.into(), after, device: (1, 0) });
        });
        let file = sample("Kick 3");
        let swapped = bitwig.bitwig().swap_into_sampler(&file, &turn()).unwrap();
        assert!(swapped.trouble.is_some(), "{command}: {swapped:?}");
        assert!(swapped.set_back.is_empty(), "{command}: {swapped:?}");
        assert!(swapped.summary().contains("weren't all set back"), "{}", swapped.summary());
        let world = bitwig.world();
        assert_eq!(world.tracks[2].devices[0].sample, file.to_string_lossy());
        assert_eq!(world.tracks[1].devices[0].params[0], FakeParam::new("CONTENTS/GAIN", "Gain", 0.1), "{command}");
    }
    // After the settings were set, before they're read back: said too, not as settings Bitwig refused.
    let bitwig = with_sampler(|w| {
        w.tracks[1].devices = vec![other()];
        w.select_on = Some(SelectOn { command: "device".into(), after: 2, device: (1, 0) });
    });
    let swapped = bitwig.bitwig().swap_into_sampler(sample("Kick 4"), &turn()).unwrap();
    assert_eq!(swapped.trouble.as_deref(), Some("another device is selected in Bitwig now."));
    assert_eq!(bitwig.world().tracks[2].devices[0].params[0], FakeParam::new("CONTENTS/GAIN", "Gain", 0.8));
}

/// A sound of the name the Sampler holds already, on a Bitwig that shows no sign of the swap: the report says it
/// can't tell, and every setting is set again instead of counted as come over.
/// The 19 Nov review's LK-3: the selection moved before Bitwig showed the swap, so nothing says the sound went in.
#[test]
fn a_selection_that_moves_before_the_swap_shows_is_said_as_not_knowable() {
    let bitwig = with_sampler(|w| w.moves_during_swap = true);
    let swapped = bitwig.bitwig().swap_into_sampler(sample("Kick 7"), &turn()).unwrap();
    assert!(swapped.unknown, "{swapped:?}");
    assert!(swapped.set_back.is_empty() && swapped.carried.is_empty(), "{swapped:?}");
    let said = swapped.summary();
    assert!(said.contains("it can't be told whether the sound went in: another device is selected"), "{said}");
    assert!(!said.contains("0 settings"), "{said}");
    assert!(!bitwig.world().commands().contains(&"set".to_owned()));
}

/// The 19 Nov review's LK-7: another pad's Sampler, clicked while Bitwig swapped, sits at the same place on the same
/// track. Only a Sampler showing the sound that was sent gets the old settings.
#[test]
fn a_sampler_showing_another_sample_after_the_swap_gets_no_settings() {
    let bitwig = with_sampler(|w| w.shows_other_sample = Some("Snare D1.wav".into()));
    let swapped = bitwig.bitwig().swap_into_sampler(sample("Kick 8"), &turn()).unwrap();
    assert!(swapped.unknown, "{swapped:?}");
    assert!(swapped.trouble.as_deref().is_some_and(|t| t.contains("Snare D1.wav")), "{swapped:?}");
    assert!(!bitwig.world().commands().contains(&"set".to_owned()));
    // A Bitwig that names the sample without its ending, or in another case, is still the sound sent.
    for shown in ["kick 9", "KICK 9.WAV"] {
        let bitwig = with_sampler(|w| w.shows_other_sample = Some(shown.into()));
        let swapped = bitwig.bitwig().swap_into_sampler(sample("Kick 9"), &turn()).unwrap();
        assert!(!swapped.unknown, "{shown}: {swapped:?}");
    }
}

#[test]
fn a_sound_of_the_same_name_with_no_sign_of_the_swap_is_said_as_not_knowable() {
    let file = sample("Old");
    let bitwig = with_sampler(|w| w.quiet_same_name = true);
    let swapped = bitwig.bitwig().swap_into_sampler(&file, &turn()).unwrap();
    assert!(swapped.same_sound);
    assert!(swapped.carried.is_empty(), "{swapped:?}");
    assert_eq!(swapped.set_back, ["Gain", "Pitch"]);
    assert!(swapped.summary().contains("showed no sign"), "{}", swapped.summary());

    // With the sign (Bitwig told the Sampler's parameters again), it's a swap like any other.
    let bitwig = with_sampler(|_| {});
    let swapped = bitwig.bitwig().swap_into_sampler(&file, &turn()).unwrap();
    assert!(!swapped.same_sound);
    assert_eq!(swapped.carried, ["Pitch"]);
}

/// A Bitwig that doesn't tell the Sampler's sample can't show a swap: nothing is tried.
#[test]
fn a_bitwig_that_doesnt_tell_the_sample_is_refused_before_anything_changes() {
    let bitwig = with_sampler(|w| w.no_sample_names = true);
    let said = bitwig.bitwig().swap_into_sampler(sample("Kick 5"), &turn()).unwrap_err();
    assert!(matches!(&said, Error::Invalid(why) if why.contains("nothing was tried")), "{said:?}");
    assert!(!bitwig.world().requests.iter().any(|r| r["cmd"] == "insert"));
}
