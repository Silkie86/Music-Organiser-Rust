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
    assert!(matches!(e("x.mp3", b"ID3"), AudioError::NotSupported(_)));
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
}
