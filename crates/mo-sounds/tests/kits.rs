//! Exporting a kit into a folder of its own. Everything is made in the test build's own fixtures folder, under a fixed
//! name, never a random temp one.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use mo_sounds::Sound;
use mo_sounds::kits::{ExportOptions, default_drum_kit, export};

fn sound(id: i64, path: &std::path::Path) -> Sound {
    // A sound as the library lists it; only the id and path matter here.
    let mut s = Sound::new(id, &path.to_string_lossy());
    s.sound_type = "Kick".into();
    s
}

#[test]
fn exports_copies_and_converted_pads_and_never_overwrites_a_kit() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-sounds-kits");
    let _ = fs::remove_dir_all(&dir);
    let samples = dir.join("samples");
    fs::create_dir_all(&samples).unwrap();
    // A stereo 44.1 kHz kick at half level.
    let kick = samples.join("Big Kick.wav");
    let left: Vec<f32> = (0..4410).map(|i| 0.5 * (i as f32 * 0.05).sin()).collect();
    mo_audio::write_wav(&kick, &[left.clone(), left], 44_100, 16).unwrap();
    let snare = samples.join("Snare: 2.wav".replace(':', "-"));
    fs::copy(&kick, &snare).unwrap();
    let sounds = [sound(1, &kick), sound(2, &snare)];
    let get = |id: i64| sounds.iter().find(|s| s.id == id);

    let mut kit = default_drum_kit();
    kit[0].sound = Some(1);
    kit[1].sound = Some(2);
    let out = dir.join("kits");
    let first = export(&kit, get, &out, "My kit: one", ExportOptions::default()).unwrap();
    assert_eq!(first.folder, out.join("My kit- one"));
    let names: Vec<String> = first.pads.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["01 Kick - Big Kick.wav", "02 Snare - Snare- 2.wav"]);
    assert_eq!(fs::read(&first.pads[0]).unwrap(), fs::read(&kick).unwrap());
    let list = fs::read_to_string(first.folder.join("kit.txt")).unwrap();
    assert!(list.starts_with("My kit: one"), "{list}");
    assert!(list.contains(&format!("01 Kick: {}", kick.display())), "{list}");
    // Nothing left half-written.
    assert!(fs::read_dir(&first.folder).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".part")));

    // Again: a new folder beside it, converted for an Elektron, mono and as loud as it goes.
    let o = ExportOptions { elektron_ready: true, mono: true, normalise: true };
    let second = export(&kit, get, &out, "My kit: one", o).unwrap();
    assert_eq!(second.folder, out.join("My kit- one (2)"));
    let a = mo_audio::read(&second.pads[0], 60.0).unwrap();
    assert_eq!((a.sample_rate, a.channels.len()), (48_000, 1));
    let peak = a.channels[0].iter().fold(0f32, |m, x| m.max(x.abs()));
    assert!((peak - 0.98).abs() < 0.01, "{peak}");
    // The first kit is untouched, and so are the samples.
    assert_eq!(fs::read(&first.pads[0]).unwrap(), fs::read(&kick).unwrap());
    assert!(kick.exists() && snare.exists());

    // A pad whose sample has gone: the error names the sample, and no half pad is left.
    let gone = [sound(1, &samples.join("Gone.wav"))];
    let err =
        export(&kit[..1], |id| gone.iter().find(|s| s.id == id), &out, "Gone", ExportOptions::default()).unwrap_err();
    assert!(err.to_string().starts_with("Couldn't read the sample"), "{err}");
    assert_eq!(fs::read_dir(out.join("Gone")).unwrap().count(), 0);
}
