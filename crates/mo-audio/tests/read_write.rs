//! Reading WAV and AIFF files made here byte by byte, and writing WAV files. The files are made in the test build's
//! own fixtures folder, under fixed names, never a random temp one.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use mo_audio::{AudioError, read, read_from, write_wav};

fn fixture(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-audio");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn chunk(id: &[u8; 4], body: &[u8], big: bool) -> Vec<u8> {
    let mut c = id.to_vec();
    let n = body.len() as u32;
    c.extend_from_slice(&if big { n.to_be_bytes() } else { n.to_le_bytes() });
    c.extend_from_slice(body);
    if body.len() % 2 == 1 {
        c.push(0);
    }
    c
}

fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = chunks.concat();
    let mut f = b"RIFF".to_vec();
    f.extend_from_slice(&(4 + body.len() as u32).to_le_bytes());
    f.extend_from_slice(b"WAVE");
    f.extend(body);
    f
}

fn fmt(format: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&format.to_le_bytes());
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * u32::from(channels) * u32::from(bits / 8)).to_le_bytes());
    b.extend_from_slice(&(channels * bits / 8).to_le_bytes());
    b.extend_from_slice(&bits.to_le_bytes());
    chunk(b"fmt ", &b, false)
}

fn pcm16(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

#[test]
fn reads_a_stereo_wav_with_its_acid_and_sampler_notes() {
    let mut acid = vec![0u8; 24];
    acid[0] = 2; // root note set, not a one-shot
    acid[4] = 48;
    acid[20..24].copy_from_slice(&128f32.to_le_bytes());
    let mut smpl = vec![0u8; 36];
    smpl[12] = 40;
    let path = fixture(
        "stereo.wav",
        &riff(&[
            fmt(1, 2, 8, 16),
            chunk(b"LIST", b"odd", false),
            chunk(b"acid", &acid, false),
            chunk(b"smpl", &smpl, false),
            chunk(b"data", &pcm16(&[16384, -16384, 32767, -32768, 0, 0]), false),
        ]),
    );
    let a = read(&path, 60.0).unwrap();
    assert_eq!(a.sample_rate, 8);
    assert_eq!(a.channels, [vec![0.5, 32767.0 / 32768.0, 0.0], vec![-0.5, -1.0, 0.0]]);
    assert_eq!(a.file_seconds, 3.0 / 8.0);
    assert_eq!(a.embedded_tempo, Some(128.0));
    // The ACID chunk's root note wins over the sampler chunk's.
    assert_eq!(a.embedded_root_note, Some(48));
    assert_eq!(a.embedded_is_loop, Some(true));
    assert_eq!(a.mono().as_ref(), [0.0, (32767.0 / 32768.0 - 1.0) / 2.0, 0.0]);
    assert_eq!(a.seconds(), 3.0 / 8.0);

    // Part way in, and only so long.
    let part = read_from(&path, 1.0 / 8.0, 1).unwrap();
    assert_eq!(part.channels, [vec![32767.0 / 32768.0], vec![-1.0]]);
    assert_eq!(part.file_seconds, 3.0 / 8.0);
    assert!(read_from(&path, 1.0, 10).unwrap().is_empty());
}

#[test]
fn reads_float_extensible_and_cut_short_wavs() {
    let floats: Vec<u8> = [0.25f32, -0.75].iter().flat_map(|f| f.to_le_bytes()).collect();
    let a = read(&fixture("float.wav", &riff(&[fmt(3, 1, 10, 32), chunk(b"data", &floats, false)])), 60.0).unwrap();
    assert_eq!(a.channels, [vec![0.25, -0.75]]);

    // WAVE_FORMAT_EXTENSIBLE, 24-bit PCM inside.
    let mut ext = fmt(0xFFFE, 1, 10, 24);
    ext[4] = 40; // chunk size
    ext.truncate(8 + 16);
    ext.extend_from_slice(&[22, 0, 24, 0, 4, 0, 0, 0, 1, 0]);
    ext.extend_from_slice(&[0; 14]);
    let a = read(&fixture("ext.wav", &riff(&[ext, chunk(b"data", &[0, 0, 0x40, 0, 0, 0xC0], false)])), 60.0).unwrap();
    assert_eq!(a.channels, [vec![0.5, -0.5]]);

    // A data chunk that says it's longer than the file: what's there is read, whole frames only.
    let mut cut = riff(&[fmt(1, 1, 10, 16)]);
    cut.extend_from_slice(b"data");
    cut.extend_from_slice(&1000u32.to_le_bytes());
    cut.extend_from_slice(&[0, 0x40, 0, 0xC0, 7]);
    let a = read(&fixture("cut.wav", &cut), 60.0).unwrap();
    assert_eq!(a.channels, [vec![0.5, -0.5]]);

    // An ACID chunk the file cuts short is left out, not read as "a loop".
    let mut cut_acid = riff(&[fmt(1, 1, 10, 16), chunk(b"data", &[0, 0x40], false)]);
    cut_acid.extend_from_slice(b"acid");
    cut_acid.extend_from_slice(&24u32.to_le_bytes());
    cut_acid.extend_from_slice(&[0, 0]);
    let a = read(&fixture("cut-acid.wav", &cut_acid), 60.0).unwrap();
    assert_eq!((a.channels, a.embedded_is_loop), (vec![vec![0.5]], None));

    // No end to how long: the whole file.
    let a = read(&fixture("all.wav", &riff(&[fmt(1, 1, 10, 16), chunk(b"data", &[0, 0x40], false)])), f64::INFINITY);
    assert_eq!(a.unwrap().channels, [vec![0.5]]);
}

#[test]
fn reads_aiff_and_aifc() {
    let comm = |bits: u16, comp: Option<&[u8; 4]>| {
        let mut b = Vec::new();
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&2u32.to_be_bytes());
        b.extend_from_slice(&bits.to_be_bytes());
        b.extend_from_slice(&[0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]); // 44100
        if let Some(c) = comp {
            b.extend_from_slice(c);
        }
        chunk(b"COMM", &b, true)
    };
    let form = |kind: &[u8; 4], chunks: &[Vec<u8>]| {
        let body: Vec<u8> = chunks.concat();
        let mut f = b"FORM".to_vec();
        f.extend_from_slice(&(4 + body.len() as u32).to_be_bytes());
        f.extend_from_slice(kind);
        f.extend(body);
        f
    };
    let ssnd = |samples: &[u8]| {
        let mut b = vec![0u8; 8];
        b.extend_from_slice(samples);
        chunk(b"SSND", &b, true)
    };
    let a = read(
        &fixture(
            "big.aif",
            &form(b"AIFF", &[comm(16, None), chunk(b"INST", &[36, 0], true), ssnd(&[0x40, 0, 0xC0, 0])]),
        ),
        60.0,
    )
    .unwrap();
    assert_eq!((a.sample_rate, a.channels.clone(), a.embedded_root_note), (44100, vec![vec![0.5, -0.5]], Some(36)));
    assert_eq!(a.file_seconds, 2.0 / 44100.0);

    let a = read(&fixture("little.aifc", &form(b"AIFC", &[comm(16, Some(b"sowt")), ssnd(&[0, 0x40, 0, 0xC0])])), 60.0)
        .unwrap();
    assert_eq!(a.channels, [vec![0.5, -0.5]]);

    let e = read(&fixture("ima.aifc", &form(b"AIFC", &[comm(16, Some(b"ima4")), ssnd(&[0; 4])])), 60.0).unwrap_err();
    assert!(e.to_string().contains("Compressed AIFF (ima4)"));
}

#[test]
fn says_what_it_cannot_read() {
    let e = |name: &str, bytes: &[u8]| read(&fixture(name, bytes), 60.0).unwrap_err();
    // Not a real WMA: Windows can't decode it; elsewhere there's no WMA decoder at all.
    let wma = e("x.wma", b"0&");
    assert!(cfg!(windows) || matches!(wma, AudioError::NotSupported(_)), "{wma}");
    assert!(matches!(e("x.opus", b"0&"), AudioError::NotSupported(_)));
    assert!(!e("x.mp3", b"not an mp3 at all").to_string().is_empty());
    assert!(e("text.wav", b"hello there").to_string().contains("Not a WAV or AIFF"));
    assert!(e("empty.wav", b"").to_string().contains("Not a WAV or AIFF"));
    assert!(e("no-data.wav", &riff(&[fmt(1, 1, 10, 16)])).to_string().contains("no audio data"));
    assert!(
        e("adpcm.wav", &riff(&[fmt(2, 1, 10, 16), chunk(b"data", &[0; 4], false)])).to_string().contains("isn't PCM")
    );
    assert!(read(&PathBuf::from("does/not/exist.wav"), 60.0).is_err());
}

#[test]
fn writes_wavs_that_read_back() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-audio");
    std::fs::create_dir_all(&dir).unwrap();
    for bits in [16u16, 24] {
        let path = dir.join(format!("written-{bits}.wav"));
        let channels = vec![vec![0.5f32, -1.0, 2.0], vec![0.0, 0.25, -2.0]];
        write_wav(&path, &channels, 48000, bits).unwrap();
        assert!(!dir.join(format!("written-{bits}.wav.part")).exists());
        let a = read(&path, 60.0).unwrap();
        assert_eq!(a.sample_rate, 48000);
        let full = if bits == 16 { 32767.0 } else { 8_388_607.0 };
        let scale = if bits == 16 { 32768.0 } else { 8_388_608.0 };
        for (got, want) in a.channels.iter().flatten().zip(channels.iter().flatten()) {
            let expected = (want.clamp(-1.0, 1.0) * full).round_ties_even() / scale;
            assert_eq!(*got, expected);
        }
    }
    assert!(write_wav(&dir.join("bad.wav"), &[vec![0.0], vec![]], 48000, 16).is_err());
    assert!(write_wav(&dir.join("bad.wav"), &[vec![0.0]], 48000, 8).is_err());
    // Too many channels for a WAV's block size: an error, not a crash.
    assert!(write_wav(&dir.join("bad.wav"), &vec![vec![0.0]; 30_000], 48000, 24).is_err());
    assert!(!dir.join("bad.wav.part").exists());
}

#[test]
fn an_odd_length_wav_is_padded() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-audio");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("odd.wav");
    write_wav(&path, &[vec![0.5]], 48000, 24).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 44 + 3 + 1);
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize, bytes.len() - 8);
    assert_eq!(read(&path, 60.0).unwrap().channels, [vec![(0.5f32 * 8_388_607.0).round_ties_even() / 8_388_608.0]]);
}

/// The tones in tests/fixtures, made with ffmpeg (see its README).
fn tone(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

#[test]
fn decodes_mp3_flac_m4a_ogg_and_aac() {
    for name in ["tone.flac", "tone.mp3", "tone.m4a", "tone.ogg", "tone.aac"] {
        let a = read(&tone(name), 60.0).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(a.sample_rate, 22050, "{name}");
        assert_eq!(a.channels.len(), 2, "{name}");
        // Half a second, give or take an encoder's padding.
        assert!((a.seconds() - 0.5).abs() < 0.06, "{name}: {}", a.seconds());
        assert!((a.file_seconds - 0.5).abs() < 0.06, "{name}: {}", a.file_seconds);
        // A tone, not silence (ffmpeg makes it at an eighth of full level).
        let peak = a.mono().iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.05, "{name}: {peak}");
    }
    // Only so long, and the whole length still known.
    let a = read(&tone("tone.flac"), 0.1).unwrap();
    assert_eq!(a.len(), 2205);
    assert_eq!(a.file_seconds, 0.5);
    assert!(read_from(&tone("tone.mp3"), 60.0, 10).unwrap_err().to_string().contains("only read from the start"));
}

#[test]
fn decodes_wma_on_windows_only() {
    let path = tone("tone.wma");
    assert_eq!(mo_audio::can_decode(&path), cfg!(windows));
    assert!(mo_audio::can_decode(&tone("tone.flac")) && mo_audio::can_decode(&PathBuf::from("a.WAV")));
    assert!(!mo_audio::can_decode(&PathBuf::from("a.opus")));
    if !cfg!(windows) {
        assert!(read(&path, 60.0).unwrap_err().to_string().contains("only be played on Windows"));
        return;
    }
    let a = read(&path, 60.0).unwrap();
    assert_eq!((a.sample_rate, a.channels.len()), (22050, 2));
    // Half a second, give or take the encoder's padding, and a tone, not silence.
    assert!((a.seconds() - 0.5).abs() < 0.1, "{}", a.seconds());
    let peak = a.mono().iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.05, "{peak}");
    // Only so long, and the whole length still known.
    let short = read(&path, 0.1).unwrap();
    assert_eq!(short.len(), 2205);
    assert!((short.file_seconds - 0.5).abs() < 0.1, "{}", short.file_seconds);
    assert!(read_from(&path, 60.0, 10).unwrap_err().to_string().contains("only read from the start"));
    // A missing file is a read error, as for any other kind.
    assert!(matches!(read(&PathBuf::from("does/not/exist.wma"), 60.0), Err(AudioError::Io(_))));
}

#[test]
fn a_cut_short_mp3_flac_or_ogg_is_damage_not_a_read_failure() {
    let cut = |name: &str, keep: fn(usize) -> usize| {
        let bytes = std::fs::read(tone(name)).unwrap();
        read(&fixture(&format!("cut-{name}"), &bytes[..keep(bytes.len())]), 60.0)
    };
    // An MP3 cut part way: the part that's there.
    let a = cut("tone.mp3", |n| n / 2).unwrap();
    assert!(!a.is_empty() && a.channels.iter().all(|c| c.len() == a.len()));
    // Ones symphonia can't open cut short: a damaged file, not a drive or lock problem.
    for name in ["tone.flac", "tone.ogg"] {
        let e = cut(name, |n| n / 2).unwrap_err();
        assert!(matches!(e, AudioError::Format(_)), "{name}: {e:?}");
    }
}
