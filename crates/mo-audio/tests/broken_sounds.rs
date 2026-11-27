//! The sound readers on broken copies of the test tones: cut short at many places, and changed in a few bytes the way
//! a damaged disk or a half-copied file changes them. A reader may say the file is damaged, but must never panic or
//! take minutes. The copies are written in the test build's own folder, under fixed names (never a random temp one).
//!
//! The fuzzer (`fuzz/README.md`, target `audio`) does this far more deeply; this runs on every change, with the same
//! broken copies each time (a fixed random seed). A file it trips on is printed as how it was made.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

#[path = "../../mo-formats/tests/common/broken.rs"]
mod broken;

use broken::*;

fn tone(name: &str) -> Vec<u8> {
    std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
}

/// A 16-bit PCM WAV and a 24-bit AIFF, a tenth of a second of a tone, made here.
fn pcm_files() -> Vec<(&'static str, Vec<u8>)> {
    let frames = 4410u32;
    let samples: Vec<i16> = (0..frames).map(|i| ((i as f32 * 0.06).sin() * 12000.0) as i16).collect();
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut wav = b"RIFF".to_vec();
    wav.extend((36 + data.len() as u32).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(44100u32.to_le_bytes());
    wav.extend(88200u32.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((data.len() as u32).to_le_bytes());
    wav.extend(&data);

    let body: Vec<u8> = samples.iter().flat_map(|s| (i32::from(*s) << 8).to_be_bytes()[1..].to_vec()).collect();
    let mut comm = Vec::new();
    comm.extend(1u16.to_be_bytes());
    comm.extend(frames.to_be_bytes());
    comm.extend(24u16.to_be_bytes());
    // 44100 as an 80-bit float.
    comm.extend([0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0]);
    let mut aiff = b"FORM".to_vec();
    aiff.extend((4 + 8 + comm.len() as u32 + 8 + 8 + body.len() as u32).to_be_bytes());
    aiff.extend(b"AIFFCOMM");
    aiff.extend((comm.len() as u32).to_be_bytes());
    aiff.extend(&comm);
    aiff.extend(b"SSND");
    aiff.extend((8 + body.len() as u32).to_be_bytes());
    aiff.extend([0u8; 8]);
    aiff.extend(&body);
    vec![("wav", wav), ("aif", aiff)]
}

#[test]
fn sound_readers_survive_broken_files() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-audio-broken");
    std::fs::create_dir_all(&dir).unwrap();
    let mut files = pcm_files();
    for name in ["tone-ima-adpcm.wav", "tone-ms-adpcm.wav", "tone.mp3", "tone.flac", "tone.ogg", "tone.m4a", "tone.aac"]
    {
        files.push((name.rsplit('.').next().unwrap(), tone(name)));
    }
    let mut dice = Dice::named("sounds");
    let mut slowest = (Duration::ZERO, String::new());
    for (n, (ending, file)) in files.iter().enumerate() {
        let path = dir.join(format!("broken.{ending}"));
        for (how, bytes) in broken(file, 12, 60, Some(256), &mut dice) {
            std::fs::write(&path, &bytes).unwrap();
            let started = Instant::now();
            let said = panic_in(|| {
                let _ = mo_audio::read(&path, 30.0);
                let _ = mo_audio::read_from(&path, 5.0, 1000);
                let mut parts = 0;
                let _ = mo_audio::read_parts(&path, 1.0, &mut |_, _| {
                    parts += 1;
                    parts < 16
                });
                let _ = mo_audio::reads_exactly(&path);
            });
            if let Some(said) = said {
                panic!("{ending} file {n}, {how}: the reader panicked: {said}");
            }
            let took = started.elapsed();
            assert!(took < Duration::from_secs(20), "{ending} file {n}, {how}: it took {took:?}");
            if took > slowest.0 {
                slowest = (took, format!("{ending} file {n}, {how}"));
            }
        }
    }
    println!("slowest {:?} ({})", slowest.0, slowest.1);
}
