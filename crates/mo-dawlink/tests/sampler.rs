//! A sound tried in the selected Sampler with its settings kept (extension 3.25), against the stand-in Bitwig.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use mo_dawlink::bitwig::fake::{FakeBitwig, FakeDevice, FakeParam, World};
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

/// Bitwig with extension 3.25, a Sampler selected on Bass, set up with three settings.
fn with_sampler(change: impl FnOnce(&mut World)) -> FakeBitwig {
    let mut world = World::default();
    world.hello.insert("extension".into(), "3.25".into());
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
        w.hello.insert("extension".into(), "3.24".into());
    });
    let said = bitwig.bitwig().swap_into_sampler(sample("Hat"), &turn()).unwrap_err();
    assert!(matches!(said, Error::OldExtension { needs: "3.25", .. }), "{said:?}");

    let bitwig = with_sampler(|_| {});
    let gone = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("sampler-none").join("Gone.wav");
    let said = bitwig.bitwig().swap_into_sampler(gone, &turn()).unwrap_err();
    assert!(matches!(said, Error::Missing { .. }), "{said:?}");
}
