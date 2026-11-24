//! Reading WAV and AIFF files made here byte by byte, and writing WAV files. The files are made in the test build's
//! own fixtures folder, under fixed names, never a random temp one.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use mo_audio::{AudioError, EMPTY_FILE, read, read_from, read_parts, write_wav};

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
    assert!(e("short.wav", b"RI").to_string().contains("Not a WAV or AIFF"));
    // A file of 0 bytes says so, whatever its name, read whole or a part at a time (Solomon's note of 23 Nov: two
    // empty .wav files on an old sample CD were said not to be WAV files).
    for name in ["empty.wav", "empty.aif", "empty.mp3", "empty.flac", "empty.wma"] {
        assert_eq!(e(name, b"").to_string(), EMPTY_FILE, "{name}");
        let parts = read_parts(&fixture(name, b""), 1.0, &mut |_, _| true).unwrap_err();
        assert_eq!(parts.to_string(), EMPTY_FILE, "{name}");
    }
    assert!(e("no-data.wav", &riff(&[fmt(1, 1, 10, 16)])).to_string().contains("no audio data"));
    assert!(
        e("mp3-in-wav.wav", &riff(&[fmt(85, 1, 10, 16), chunk(b"data", &[0; 4], false)]))
            .to_string()
            .contains("isn't PCM")
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
fn an_mp3_with_a_tag_over_a_megabyte_still_decodes() {
    // Cover art makes tags big; without symphonia's ID3v2 reader the probe looked through the tag a byte at a time
    // and gave up after 1 MB. The tone's own tag (ID3v2.4), made 1.5 MB bigger with padding.
    let mp3 = std::fs::read(tone("tone.mp3")).unwrap();
    let synchsafe = |b: &[u8]| b.iter().fold(0usize, |n, &b| n << 7 | usize::from(b & 0x7F));
    let tag_len = synchsafe(&mp3[6..10]);
    let padding = 1_500_000;
    let size = tag_len + padding;
    let mut big = mp3[..6].to_vec();
    big.extend([21, 14, 7, 0].map(|shift| ((size >> shift) & 0x7F) as u8));
    big.extend(&mp3[10..10 + tag_len]);
    big.extend(std::iter::repeat_n(0, padding));
    big.extend(&mp3[10 + tag_len..]);
    let a = read(&fixture("big tag.mp3", &big), 60.0).unwrap();
    assert!((a.seconds() - 0.5).abs() < 0.06, "{}", a.seconds());

    // A damaged tag that symphonia's tag reader refuses still lets the audio after it be read: a version it doesn't
    // know, or a size past the end of the file.
    let mut unknown = mp3.clone();
    unknown[3] = 9;
    let mut too_long = mp3.clone();
    too_long[6..10].copy_from_slice(&[0x7F; 4]);
    for (name, bytes) in [("unknown tag.mp3", unknown), ("tag too long.mp3", too_long)] {
        let a = read(&fixture(name, &bytes), 60.0).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!((a.seconds() - 0.5).abs() < 0.06, "{name}: {}", a.seconds());
    }
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

/// A WAV as FL Studio keeps its own sounds: format 0x674F ("Og"), a whole Ogg Vorbis stream in the data chunk.
#[test]
fn a_wav_with_ogg_vorbis_inside_is_decoded() {
    let ogg = std::fs::read(tone("tone.ogg")).unwrap();
    let mut acid = vec![0u8; 24];
    acid[20..24].copy_from_slice(&140f32.to_le_bytes());
    let path = fixture(
        "ogg-inside.wav",
        &riff(&[fmt(0x674F, 2, 22050, 16), chunk(b"acid", &acid, false), chunk(b"data", &ogg, false)]),
    );
    let plain = read(&tone("tone.ogg"), 60.0).unwrap();
    let a = read(&path, 60.0).unwrap();
    assert_eq!((a.sample_rate, a.channels.len()), (22050, 2));
    assert_eq!(a.channels, plain.channels);
    assert_eq!(a.file_seconds, plain.file_seconds);
    // The WAV's own notes still count.
    assert_eq!(a.embedded_tempo, Some(140.0));
    // Part way in, and only so long, with the whole length still known.
    let part = read_from(&path, 0.1, 1000).unwrap();
    assert_eq!(part.len(), 2205);
    assert_eq!(part.channels[0][..], plain.channels[0][1000..3205]);
    assert_eq!(read_from(&path, 60.0, 1000).unwrap().file_seconds, plain.file_seconds);
    // The kinds that keep the Vorbis headers apart say what they are; damage is an error, never a crash.
    let apart = fixture("ogg-apart.wav", &riff(&[fmt(0x6750, 2, 22050, 16), chunk(b"data", &[0u8; 64], false)]));
    assert!(read(&apart, 60.0).unwrap_err().to_string().contains("Ogg Vorbis"));
    let cut = fixture("ogg-inside-cut.wav", &riff(&[fmt(0x674F, 2, 22050, 16), chunk(b"data", &ogg[..40], false)]));
    assert!(read(&cut, 60.0).is_err());
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

/// CRC-32 as Ogg pages carry it (polynomial 0x04C11DB7, not reflected), over a page with its own CRC as zeros.
fn ogg_crc(page: &[u8]) -> u32 {
    page.iter().fold(0u32, |mut crc, &b| {
        crc ^= u32::from(b) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
        }
        crc
    })
}

/// An Ogg file with each page changed by `change` (its bytes, and which page it is from 0), and its CRC made again.
fn ogg_changed(ogg: &[u8], change: impl Fn(&mut [u8], usize)) -> Vec<u8> {
    let (mut out, mut at) = (Vec::new(), 0);
    while at < ogg.len() {
        assert_eq!(&ogg[at..at + 4], b"OggS");
        let segments = usize::from(ogg[at + 26]);
        let body: usize = ogg[at + 27..at + 27 + segments].iter().map(|&n| usize::from(n)).sum();
        let mut page = ogg[at..at + 27 + segments + body].to_vec();
        change(&mut page, out.len());
        page[22..26].fill(0);
        let crc = ogg_crc(&page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        at += page.len();
        out.push(page);
    }
    out.concat()
}

/// The whole-app review of 19 Nov, SN-11: a chained Ogg file (two joined into one, a radio recording) got no waveform
/// at all, as the read stopped with an error where the second stream begins, and was decoded again at every pick. It's
/// read on into the next stream, as players do. One whose sound doesn't match the first one's is silence as long as it
/// is (its code review: it ended the read, so a copy made from it would have been the first stream alone). A read
/// stopped part way gives the whole file's length: the first stream says only its own.
#[test]
fn a_chained_ogg_is_read_on_into_the_stream_joined_on() {
    let ogg = std::fs::read(tone("tone.ogg")).unwrap();
    let one = read(&tone("tone.ogg"), f64::INFINITY).unwrap();
    // The stream joined on has a serial number of its own, as an encoder gives it.
    let second = |rate: u32| {
        ogg_changed(&ogg, |page, n| {
            page[14..18].copy_from_slice(&0x5EC0_2D00u32.to_le_bytes());
            if n == 0 {
                // The first page holds the identification header alone: its sample rate is 12 bytes in.
                assert_eq!((page[26], &page[28..35]), (1, &b"\x01vorbis"[..]));
                page[40..44].copy_from_slice(&rate.to_le_bytes());
            }
        })
    };
    let chained = fixture("chained.ogg", &[ogg.clone(), second(22050)].concat());
    let both = read(&chained, f64::INFINITY).unwrap();
    assert_eq!((both.sample_rate, both.channels.len(), both.len()), (22050, 2, 2 * one.len()));
    for (b, o) in both.channels.iter().zip(&one.channels) {
        assert_eq!((&b[..o.len()], &b[o.len()..]), (&o[..], &o[..]), "the tone twice");
    }
    assert_eq!(both.file_seconds, 2.0 * one.len() as f64 / 22050.0);
    // A part at a time, as a waveform reads it: the same, and the whole length.
    let InParts { all, seconds, .. } = in_parts(&chained, 0.1).unwrap();
    assert_eq!((all, seconds), (both.channels.clone(), both.file_seconds));
    // Joined on at twice the sample rate: the first stream, then silence as long as the second (half as many frames
    // at the first one's rate).
    let other_rate = fixture("chained other rate.ogg", &[ogg.clone(), second(44100)].concat());
    let other = read(&other_rate, f64::INFINITY).unwrap();
    assert_eq!((other.sample_rate, other.len()), (22050, one.len() + one.len() / 2));
    for (b, o) in other.channels.iter().zip(&one.channels) {
        assert_eq!(&b[..o.len()], &o[..], "the first stream");
        assert!(b[o.len()..].iter().all(|&s| s == 0.0), "the second stream as silence");
    }
    assert_eq!(other.file_seconds, other.len() as f64 / 22050.0);
    let InParts { all, seconds, .. } = in_parts(&other_rate, 0.1).unwrap();
    assert_eq!((all, seconds), (other.channels.clone(), other.file_seconds));
    // A third stream after it, at the first one's rate again: the silence for the second is as long as the second at
    // its own rate, made before the third begins (its code review: what was owed was made silence at the third one's
    // rate), and the third is the tone where it was.
    let third = ogg_changed(&ogg, |page, _| page[14..18].copy_from_slice(&0x7E1A_0B00u32.to_le_bytes()));
    let three = fixture("chained three.ogg", &[ogg.clone(), second(44100), third].concat());
    let three = read(&three, f64::INFINITY).unwrap();
    assert_eq!(three.len(), other.len() + one.len());
    for (b, o) in three.channels.iter().zip(&one.channels) {
        assert_eq!((&b[..o.len()], &b[other.len()..]), (&o[..], &o[..]), "the first and third streams");
        assert!(b[o.len()..other.len()].iter().all(|&s| s == 0.0), "the second as silence");
    }
    // Read only so far (as a sound's analysis reads a long file): the whole file's length all the same, the rest
    // counted from its packets.
    for (path, whole) in [(&chained, &both), (&other_rate, &other)] {
        let start = read(path, 0.1).unwrap();
        assert_eq!((start.len(), start.file_seconds), (2205, whole.file_seconds), "{}", path.display());
    }
}

/// Where each FLAC frame starts and its block size (each has it, the last's shorter), from the file's STREAMINFO: after
/// the metadata blocks, at each sync code whose header's checksum (CRC-8) holds.
fn flac_frames(flac: &[u8]) -> (Vec<usize>, usize) {
    assert_eq!(&flac[..4], b"fLaC");
    let block = usize::from(u16::from_be_bytes([flac[8], flac[9]]));
    let mut at = 4;
    loop {
        let last = flac[at] & 0x80 != 0;
        at += 4 + (usize::from(flac[at + 1]) << 16 | usize::from(flac[at + 2]) << 8 | usize::from(flac[at + 3]));
        if last {
            break;
        }
    }
    let crc8 = |bytes: &[u8]| {
        bytes.iter().fold(0u8, |mut crc, &b| {
            crc ^= b;
            for _ in 0..8 {
                crc = if crc & 0x80 != 0 { crc << 1 ^ 0x07 } else { crc << 1 };
            }
            crc
        })
    };
    // The tone's headers: sync, block size and rate, channels and sample size, the frame's number in one byte (fewer
    // than 128 frames), maybe the block size in 8 or 16 bits, then the CRC-8.
    let frames = (at..flac.len().saturating_sub(8))
        .filter(|&i| flac[i] == 0xFF && flac[i + 1] == 0xF8)
        .filter(|&i| {
            let extra = match flac[i + 2] >> 4 {
                6 => 1,
                7 => 2,
                _ => 0,
            };
            let end = i + 5 + extra;
            crc8(&flac[i..end]) == flac[end]
        })
        .collect();
    (frames, block)
}

/// The code review of SN-18's fix: a FLAC frame whose checksum fails never comes as a packet (the reader passes over
/// it), so everything after it came early, as a refused frame's did. The jump in the packets' times is silence now.
#[test]
fn a_flac_frame_lost_to_damage_is_silence_and_what_follows_stays_in_place() {
    let flac = std::fs::read(tone("tone.flac")).unwrap();
    let whole = read(&tone("tone.flac"), f64::INFINITY).unwrap();
    let (frames, block) = flac_frames(&flac);
    assert!(frames.len() >= 3 && frames[0] < frames[1], "{frames:?}");
    // A byte in the second frame's samples changed: its checksum fails.
    let mut damaged = flac.clone();
    damaged[(frames[1] + frames[2]) / 2] ^= 0x5A;
    let a = read(&fixture("damaged frame.flac", &damaged), f64::INFINITY).unwrap();
    assert_eq!((a.sample_rate, a.len(), a.file_seconds), (22050, whole.len(), whole.file_seconds));
    for (x, y) in a.channels.iter().zip(&whole.channels) {
        assert_eq!(&x[..block], &y[..block], "the first frame");
        assert!(x[block..2 * block].iter().all(|&s| s == 0.0), "the lost one as silence");
        assert_eq!(&x[2 * block..], &y[2 * block..], "the rest, where it was");
    }
    let InParts { all, seconds, .. } = in_parts(&fixture("damaged frame.flac", &damaged), 0.1).unwrap();
    assert_eq!((all, seconds), (a.channels, whole.file_seconds));
}

/// Where each ADTS frame of a bare AAC file starts, and how long its header is (7 bytes, or 9 with a checksum).
fn adts_frames(aac: &[u8]) -> Vec<(usize, usize)> {
    let (mut frames, mut at) = (Vec::new(), 0);
    while at + 7 <= aac.len() {
        assert_eq!((aac[at], aac[at + 1] & 0xF0), (0xFF, 0xF0), "frame header at {at}");
        let header = if aac[at + 1] & 1 != 0 { 7 } else { 9 };
        frames.push((at, header));
        at += usize::from(aac[at + 3] & 3) << 11 | usize::from(aac[at + 4]) << 3 | usize::from(aac[at + 5] >> 5);
    }
    frames
}

/// The code review of SN-18's fix: damage in an AAC frame can read as something its decoder doesn't do (a coupling
/// channel, say), which fails as "not supported" rather than as damage, and that ended the whole read: no waveform and
/// no analysis for a file with one bad frame. Once the sound has come, that's a refused frame too, silence of its length.
#[test]
fn an_aac_frame_damaged_into_something_unsupported_is_silence() {
    let aac = std::fs::read(tone("tone.aac")).unwrap();
    let whole = read(&tone("tone.aac"), f64::INFINITY).unwrap();
    let frames = adts_frames(&aac);
    let k = frames.len() / 2;
    // The frame's first element says it's a coupling channel (element 2), which the decoder can't decode.
    let mut damaged = aac.clone();
    let (at, header) = frames[k];
    damaged[at + header] = damaged[at + header] & 0x1F | 2 << 5;
    let a = read(&fixture("damaged frame.aac", &damaged), f64::INFINITY).unwrap();
    assert_eq!((a.sample_rate, a.len(), a.file_seconds), (22050, whole.len(), whole.file_seconds));
    let silent = a.mono().windows(1024).any(|w| w.iter().all(|&s| s == 0.0));
    assert!(silent, "the refused frame is silence");
    // Two frames on (the decoder overlaps each frame with the one before), the tone is where it was, to within a
    // thousandth.
    let from = (k + 2) * 1024;
    let close = a.channels.iter().zip(&whole.channels).all(|(x, y)| {
        x.get(from..)
            .unwrap_or_default()
            .iter()
            .zip(y.get(from..).unwrap_or_default())
            .all(|(x, y)| (x - y).abs() < 1e-3)
    });
    assert!(close, "the tone after it where it was");
}

/// Where each MP3 frame starts, after the ID3v2 tag: the tone's frames are MPEG-2 layer III at 22,050 a second, 64k.
fn mp3_frames(mp3: &[u8]) -> Vec<usize> {
    let tag = 10 + mp3[6..10].iter().fold(0usize, |n, &b| n << 7 | usize::from(b & 0x7F));
    let (mut frames, mut at) = (Vec::new(), tag);
    while at + 4 <= mp3.len() {
        assert_eq!((mp3[at], mp3[at + 1], mp3[at + 2] & 0xF0), (0xFF, 0xF3, 0x80), "frame header at {at}");
        frames.push(at);
        at += 72 * 64_000 / 22_050 + usize::from(mp3[at + 2] >> 1 & 1);
    }
    frames
}

/// The whole-app review of 19 Nov, SN-18: a frame the decoder refuses (damaged on its own) was left out, so all the
/// sound after it came early (26 ms an MP3 frame), and the length came out short. An MP3 whose sound changes part way
/// (bounced in two halves, mono then stereo) has every frame after the change refused, so all the rest was lost. A
/// refused frame is silence of its length now: what comes after stays in its place, and the length is kept.
#[test]
fn a_damaged_frame_is_silence_and_what_follows_stays_in_place() {
    let mp3 = std::fs::read(tone("tone.mp3")).unwrap();
    let whole = read(&tone("tone.mp3"), f64::INFINITY).unwrap();
    let frames = mp3_frames(&mp3);
    let mono = |from: usize, to: usize| {
        let mut damaged = mp3.clone();
        // The channel mode a frame's header says: mono, in a stereo file, so the decoder refuses it.
        for &at in &frames[from..to] {
            damaged[at + 3] |= 0xC0;
        }
        damaged
    };
    // One frame half way refused: silence where it was (576 frames), and the tone after it in its place once the
    // decoder has settled (a frame or two: it carries bits and sound over from frame to frame).
    let k = frames.len() / 2;
    let one = fixture("damaged frame.mp3", &mono(k, k + 1));
    let a = read(&one, f64::INFINITY).unwrap();
    assert_eq!((a.sample_rate, a.channels.len(), a.len()), (22050, 2, whole.len()));
    let close = |from: usize| {
        a.channels
            .iter()
            .zip(&whole.channels)
            .all(|(x, y)| x[from..].iter().zip(&y[from..]).all(|(x, y)| (x - y).abs() < 1e-4))
    };
    assert!(close(whole.len() - 4 * 576), "the tone's last frames where they were");
    let silent = a.mono().windows(576).any(|w| w.iter().all(|&s| s == 0.0));
    assert!(silent, "the refused frame is silence");
    let InParts { all, seconds, .. } = in_parts(&one, 0.1).unwrap();
    assert_eq!((all, seconds), (a.channels.clone(), whole.file_seconds));
    // The sound changes half way: the rest is silence, and the length is the whole file's.
    let changed = fixture("changes half way.mp3", &mono(k, frames.len()));
    let b = read(&changed, f64::INFINITY).unwrap();
    assert_eq!((b.len(), b.file_seconds), (whole.len(), whole.file_seconds));
    assert!(b.mono()[b.len() - 4 * 576..].iter().all(|&s| s == 0.0), "the rest is silence");
    let InParts { all, seconds, .. } = in_parts(&changed, 0.1).unwrap();
    assert_eq!((all, seconds), (b.channels, whole.file_seconds));
}

/// An IMA ADPCM WAV of `blocks` 8-byte mono blocks (9 frames each), with a `fact` chunk saying `frames`.
fn ima_wav(blocks: u8, frames: u32) -> Vec<u8> {
    let mut f = Vec::new();
    for v in [0x11u16, 1] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(&8000u32.to_le_bytes());
    f.extend_from_slice(&4000u32.to_le_bytes());
    for v in [8u16, 4, 2, 9] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    let mut data = Vec::new();
    for b in 0..blocks {
        data.extend_from_slice(&(i16::from(b) * 1000).to_le_bytes());
        data.extend_from_slice(&[0, 0, 0x74, 0x21, 0x9C, 0x05]);
    }
    riff(&[chunk(b"fmt ", &f, false), chunk(b"fact", &frames.to_le_bytes(), false), chunk(b"data", &data, false)])
}

#[test]
fn adpcm_wavs_are_read_and_can_start_part_way() {
    let path = fixture("ima-adpcm.wav", &ima_wav(3, 20));
    let all = read(&path, 60.0).unwrap();
    assert_eq!(all.sample_rate, 8000);
    // The fact chunk's 20 frames, not the 27 the three blocks could hold.
    assert_eq!(all.len(), 20);
    assert!((all.file_seconds - 20.0 / 8000.0).abs() < 1e-12);
    // Each block starts afresh at its header's sample.
    assert_eq!(all.channels[0][9], 1000.0 / 32768.0);
    let part = read_from(&path, 60.0, 11).unwrap();
    assert_eq!(part.channels[0], all.channels[0][11..]);
    let short = read_from(&path, 4.0 / 8000.0, 5).unwrap();
    assert_eq!(short.channels[0], all.channels[0][5..9]);
    // A file without a fact chunk, cut short in its last block, gives the frames its bytes hold.
    let mut cut = ima_wav(2, 0);
    cut.truncate(cut.len() - 4);
    assert_eq!(read(&fixture("ima-adpcm-cut.wav", &cut), 60.0).unwrap().len(), 10);
}

/// FNV-1a over the samples as 16-bit little-endian, interleaved, as ffmpeg's `-f s16le` writes them.
fn s16_hash(a: &mo_audio::AudioData) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for i in 0..a.len() {
        for ch in &a.channels {
            for b in (((ch[i] * 32768.0).round()) as i16).to_le_bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    h
}

#[test]
fn adpcm_wavs_decode_as_ffmpeg_decodes_them() {
    for (name, hash) in [("tone-ms-adpcm.wav", 0xb4a0_b56d_1113_2391), ("tone-ima-adpcm.wav", 0x9d81_dd80_84bd_2239)] {
        let a = read(&tone(name), 60.0).unwrap();
        assert_eq!((a.sample_rate, a.channels.len(), a.len()), (22050, 2, 11025), "{name}");
        assert_eq!(s16_hash(&a), hash, "{name}");
    }
}

/// An AIFF's 80-bit sample rate: its exponent, then its 64 bits with the leading one.
fn extended80(rate: f64) -> [u8; 10] {
    let exponent = rate.log2().floor() as i32;
    let mut b = [0u8; 10];
    b[..2].copy_from_slice(&((exponent + 16383) as u16).to_be_bytes());
    b[2..].copy_from_slice(&((rate * 2f64.powi(63 - exponent)) as u64).to_be_bytes());
    b
}

/// What [`read_parts`] handed on ([`in_parts`]).
struct InParts {
    /// Each part's length and sample rate.
    parts: Vec<(usize, u32)>,
    /// All the parts put back together, a channel each.
    all: Vec<Vec<f32>>,
    /// The whole length it gave.
    seconds: f64,
}

/// What [`read_parts`] hands on, `part_seconds` at a time.
fn in_parts(path: &Path, part_seconds: f64) -> Result<InParts, AudioError> {
    let (mut parts, mut all) = (Vec::new(), Vec::<Vec<f32>>::new());
    let seconds = read_parts(path, part_seconds, &mut |rate, channels| {
        parts.push((channels[0].len(), rate));
        all.resize(channels.len(), Vec::new());
        for (a, c) in all.iter_mut().zip(channels) {
            a.extend_from_slice(c);
        }
        true
    })?;
    Ok(InParts { parts, all, seconds })
}

/// The beta F12 note of 15 Nov: a long clip's waveform on the Songs page stopped after the file's first minute, as
/// only that much was read. A whole file is read a part at a time instead, so all of it is there however long it is,
/// with never more than a part in memory.
#[test]
fn a_whole_file_is_read_a_part_at_a_time() {
    // A WAV: 2.5 seconds of stereo at 1,000 frames a second, in parts of a second, the last one shorter.
    let left: Vec<i16> = (0..2500).map(|i| (i * 13 % 2000 - 1000) as i16).collect();
    let samples: Vec<i16> = left.iter().flat_map(|&l| [l, -l]).collect();
    let wav = fixture("parts.wav", &riff(&[fmt(1, 2, 1000, 16), chunk(b"data", &pcm16(&samples), false)]));
    let InParts { parts, all, seconds } = in_parts(&wav, 1.0).unwrap();
    assert_eq!(parts, [(1000, 1000), (1000, 1000), (500, 1000)]);
    assert_eq!(all, read(&wav, f64::INFINITY).unwrap().channels);
    assert_eq!(seconds, 2.5);
    // A part shorter than a frame: a frame or two a part instead of nothing.
    let InParts { parts, all, seconds } = in_parts(&wav, 0.0001).unwrap();
    assert!(parts.len() > 1 && parts.iter().all(|&(n, _)| n <= 2), "{}", parts.len());
    assert_eq!((all, seconds), (read(&wav, f64::INFINITY).unwrap().channels, 2.5));
    // An AIFF at an old Mac rate, 11,127.27 a second (said as 11,127): its last two frames are still read.
    let rate = 11_127.272_727;
    let frames = 100_000u32;
    let mut comm = 1u16.to_be_bytes().to_vec();
    comm.extend_from_slice(&frames.to_be_bytes());
    comm.extend_from_slice(&16u16.to_be_bytes());
    comm.extend_from_slice(&extended80(rate));
    let mut ssnd = vec![0u8; 8];
    ssnd.extend((0..frames).flat_map(|i| ((i % 2000) as i16 - 1000).to_be_bytes()));
    let body = [chunk(b"COMM", &comm, true), chunk(b"SSND", &ssnd, true)].concat();
    let mut aiff = b"FORM".to_vec();
    aiff.extend_from_slice(&(4 + body.len() as u32).to_be_bytes());
    aiff.extend_from_slice(b"AIFF");
    aiff.extend(body);
    let aiff = fixture("parts-old-mac-rate.aif", &aiff);
    let InParts { parts, all, seconds } = in_parts(&aiff, 99_998.5 / rate).unwrap();
    assert_eq!(parts, [(99_998, 11_127), (2, 11_127)]);
    assert_eq!(all, read(&aiff, f64::INFINITY).unwrap().channels);
    assert!((seconds - f64::from(frames) / rate).abs() < 1e-9, "{seconds}");
    // An ADPCM WAV, its parts ending part way through its blocks (9 frames each, 20 in all).
    let adpcm = fixture("parts-ima-adpcm.wav", &ima_wav(3, 20));
    let InParts { parts, all, seconds } = in_parts(&adpcm, 7.0 / 8000.0).unwrap();
    assert_eq!(parts, [(7, 8000), (7, 8000), (6, 8000)]);
    assert_eq!(all, read(&adpcm, f64::INFINITY).unwrap().channels);
    assert_eq!(seconds, 20.0 / 8000.0);

    // The kinds decoded from their start, and a WAV holding Ogg Vorbis as FL Studio keeps its own sounds: each
    // decoded once, a tenth of a second at a time, the same as all at once.
    let ogg = std::fs::read(tone("tone.ogg")).unwrap();
    let ogg_wav = fixture("parts-ogg-inside.wav", &riff(&[fmt(0x674F, 2, 22050, 16), chunk(b"data", &ogg, false)]));
    let mut kinds: Vec<PathBuf> = ["tone.flac", "tone.mp3", "tone.m4a", "tone.ogg", "tone.aac"].map(tone).into();
    kinds.push(ogg_wav.clone());
    if cfg!(windows) {
        kinds.push(tone("tone.wma"));
    }
    for path in &kinds {
        let whole = read(path, f64::INFINITY).unwrap();
        let InParts { parts, all, seconds } = in_parts(path, 0.1).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(parts.len() >= 5 && parts.iter().all(|&(_, rate)| rate == 22050), "{}: {parts:?}", path.display());
        assert!(parts[..parts.len() - 1].iter().all(|&(n, _)| n == 2205), "{}: {parts:?}", path.display());
        assert_eq!(all, whole.channels, "{}", path.display());
        assert_eq!(seconds, whole.file_seconds, "{}", path.display());
    }
    assert_eq!(in_parts(&ogg_wav, 0.1).unwrap().all, read(&tone("tone.ogg"), f64::INFINITY).unwrap().channels);

    // Stopped after its first part: no more are handed on, and a WAV still says how long it is.
    for path in [wav.clone(), tone("tone.mp3"), ogg_wav] {
        let mut calls = 0;
        let seconds = read_parts(&path, 0.1, &mut |_, _| {
            calls += 1;
            false
        })
        .unwrap();
        assert_eq!(calls, 1, "{}", path.display());
        if path == wav {
            assert_eq!(seconds, 2.5);
        }
    }

    // A bug in what the parts go to is a crash in the log and an error, never taken for a damaged file.
    for path in [wav.clone(), tone("tone.mp3")] {
        let e = read_parts(&path, 0.1, &mut |_, _| panic!("a bug in the caller")).unwrap_err();
        assert!(e.to_string().contains("tripped"), "{}: {e}", path.display());
    }

    // What can't be read says why, as `read` does.
    let nothing = |path: &Path| read_parts(path, 1.0, &mut |_, _| true).unwrap_err();
    assert!(matches!(nothing(Path::new("does/not/exist.wav")), AudioError::Io(_)));
    assert!(matches!(nothing(Path::new("does/not/exist.mp3")), AudioError::Io(_)));
    assert!(nothing(&fixture("parts.xyz", b"not sound")).to_string().contains("No decoder for .xyz files"));
    if !cfg!(windows) {
        assert!(nothing(&tone("tone.wma")).to_string().contains("only be played on Windows"));
    }
}
