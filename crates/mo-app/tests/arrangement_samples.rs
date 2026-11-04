//! Looking for an arrangement's audio files after its save is read, in the test build's own fixtures folder under fixed
//! names.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::Cell;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use mo_app::arrangement::{find_samples, notes};
use mo_formats::arrangement::{Arrangement, AudioPart, LaneKind, PreviewClip, PreviewLane};
use mo_library::samples::SampleLocator;

fn part(path: &str) -> AudioPart {
    AudioPart {
        path: path.into(),
        start: 0.0,
        length: 4.0,
        from: 0.0,
        seconds_per_beat: Some(0.5),
        whole_file_beats: None,
        reversed: false,
    }
}

fn song(paths: &[&str]) -> Arrangement {
    let mut lane = PreviewLane::new("Audio", LaneKind::Audio);
    lane.clips = paths
        .iter()
        .enumerate()
        .map(|(i, p)| PreviewClip {
            start: i as f64 * 4.0,
            length: 4.0,
            audio: vec![part(p)],
            ..PreviewClip::default()
        })
        .collect();
    Arrangement {
        view: "FL Studio playlist",
        tempo: 120.0,
        beats_per_bar: 4,
        lanes: vec![lane],
        markers: Vec::new(),
        notes: Vec::new(),
    }
}

#[test]
fn an_arrangements_audio_files_are_looked_for_once_each() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-app-arrangement-samples");
    let _ = fs::remove_dir_all(&dir);
    let kick = dir.join("Night Drive/samples/kick.wav");
    fs::create_dir_all(kick.parent().unwrap()).unwrap();
    fs::write(&kick, b"x").unwrap();
    let project = dir.join("Night Drive/Night Drive.flp");
    let factory = "%FLStudioFactoryData%/Data/Patches/Packs/Drums/clap.wav";
    // The kick twice: looked for once.
    let views = vec![Arc::new(song(&["/gone/kick.wav", factory, "/gone/vox.wav", "/gone/kick.wav"]))];
    let locator = SampleLocator::new(Box::new(|_| Vec::new()));
    let asked = Cell::new(0);
    let wanted = || {
        asked.set(asked.get() + 1);
        true
    };
    let found = find_samples(&views, &project, &locator, wanted).expect("still wanted");
    assert_eq!(asked.get(), 3, "once for each file");
    // Moved beside the project; FL Studio's own content counts as here, at the path as saved; the vox is gone.
    assert_eq!(found.get("/gone/kick.wav"), Some(&kick));
    assert_eq!(found.get(factory), Some(&PathBuf::from(factory)));
    assert_eq!(found.len(), 2);
    assert_eq!(
        notes(&views[0], Some(&found)),
        ["1 audio clip's file isn't on this computer, so it's drawn without its sound."]
    );
    // Another song picked meanwhile: it stops, with nothing to show for this one.
    assert_eq!(find_samples(&views, &project, &locator, || false), None);
    // No audio clips: nothing to look for.
    assert_eq!(find_samples(&[Arc::new(song(&[]))], &project, &locator, || true), Some(Default::default()));
}
