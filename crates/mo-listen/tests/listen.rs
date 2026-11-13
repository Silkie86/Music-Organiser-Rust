//! The listener against made-up sounds: sines, chords, clicks and silence.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::f64::consts::PI;

use mo_listen::{FFT, RING, Track, tap};

const RATE: f64 = 48_000.0;

/// `seconds` of the given notes (MIDI numbers) played together, each at `level`.
fn notes(midi: &[f64], seconds: f64, level: f32) -> Vec<f32> {
    let n = (seconds * RATE) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / RATE;
            let sum: f64 = midi.iter().map(|m| (2.0 * PI * 440.0 * 2f64.powf((m - 69.0) / 12.0) * t).sin()).sum();
            (sum / midi.len() as f64) as f32 * level
        })
        .collect()
}

/// Pushes `audio` in host-sized blocks.
fn push_all(tap: &mut mo_listen::Tap, audio: &[f32], block: usize) {
    for b in audio.chunks(block) {
        tap.push(&[b, b]);
    }
}

fn loudest(chroma: &[f32; 12], n: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..12).collect();
    order.sort_by(|a, b| chroma[*b].total_cmp(&chroma[*a]));
    let mut top = order[..n].to_vec();
    top.sort_unstable();
    top
}

#[test]
fn hears_the_note_a_sine_plays() {
    let (mut tap, mut ear) = tap(RATE, 2);
    push_all(&mut tap, &notes(&[69.0], 0.5, 0.5), 512);
    let heard = ear.hear().unwrap();
    assert_eq!(loudest(&heard.chroma, 1), [9], "A: {:?}", heard.chroma);
    assert_eq!(heard.chroma[9], 1.0);
    assert_eq!(heard.samples_to, (0.5 * RATE) as u64);
    assert!((heard.seconds - 0.5).abs() < 1e-9);
    // A sine at half scale: peak 0.5, RMS 0.5 / root 2.
    assert!((heard.peak - 0.5).abs() < 0.01, "{}", heard.peak);
    assert!((heard.rms - 0.3536).abs() < 0.01, "{}", heard.rms);
    // 440 Hz is mid.
    assert!(heard.bands[1] > 0.95, "{:?}", heard.bands);
    // Nothing new, nothing heard.
    assert_eq!(ear.hear(), None);
}

#[test]
fn hears_a_chord_and_where_its_sound_sits() {
    let (mut tap, mut ear) = tap(RATE, 1);
    // C major low down: C3 E3 G3.
    push_all(&mut tap, &notes(&[48.0, 52.0, 55.0], 0.4, 0.6), 1024);
    let heard = ear.hear().unwrap();
    assert_eq!(loudest(&heard.chroma, 3), [0, 4, 7], "{:?}", heard.chroma);
    assert!(heard.bands[0] > 0.9, "C3 to G3 are all below 250 Hz: {:?}", heard.bands);

    // A high note: E8 (5274 Hz) is high but past the chroma's 5 kHz.
    push_all(&mut tap, &notes(&[112.0], 0.4, 0.6), 1024);
    let heard = ear.hear().unwrap();
    assert!(heard.bands[2] > 0.95, "{:?}", heard.bands);
}

#[test]
fn silence_has_no_notes_and_no_level() {
    let (mut tap, mut ear) = tap(RATE, 2);
    push_all(&mut tap, &vec![0.0; FFT * 2], 256);
    let heard = ear.hear().unwrap();
    assert_eq!(heard.chroma, [0.0; 12]);
    assert_eq!(heard.bands, [0.0; 3]);
    assert_eq!((heard.rms, heard.peak), (0.0, 0.0));
    assert!(heard.hits.is_empty());
}

#[test]
fn any_block_never_panics() {
    let (mut tap, mut ear) = tap(RATE, 2);
    let long = vec![0.25f32; RING * 3 + 17];
    let short = vec![0.25f32; 10];
    tap.push(&[]);
    tap.push(&[&[]]);
    tap.push(&[&long, &short]);
    tap.push(&[&short, &[]]);
    tap.push(&[&[f32::NAN, f32::INFINITY, 1.0]]);
    // More channels than asked for: the extra one is left out.
    tap.push(&[&short, &short, &[5.0; 10]]);
    assert_eq!(tap.frames(), (RING * 3 + 17 + 10 + 3 + 10) as u64);
    let heard = ear.hear().unwrap();
    assert_eq!(heard.samples_to, tap.frames());
    // Only the ring's worth is new audio; the NaN and infinity went in as silence, the 1.0 as itself.
    assert!((heard.seconds - RING as f64 / RATE).abs() < 1e-9);
    assert!((heard.peak - 1.0).abs() < 1e-6, "{}", heard.peak);
    assert!(heard.rms.is_finite());
}

#[test]
fn channels_of_different_lengths_average_what_they_have() {
    let (mut tap, mut ear) = tap(RATE, 2);
    // Left 0.8 for 4 samples, right 0.4 for 2: 0.6, 0.6, 0.8, 0.8.
    tap.push(&[&[0.8; 4], &[0.4; 2]]);
    let heard = ear.hear().unwrap();
    assert!((heard.peak - 0.8).abs() < 1e-6);
    let rms = ((0.36f64 * 2.0 + 0.64 * 2.0) / 4.0).sqrt() as f32;
    assert!((heard.rms - rms).abs() < 1e-6, "{} vs {rms}", heard.rms);
}

/// Clicks: a short burst every `every` seconds, for `seconds`.
fn clicks(every: f64, seconds: f64) -> Vec<f32> {
    let mut out = vec![0f32; (seconds * RATE) as usize];
    let mut at = 0.1;
    while at < seconds {
        let start = (at * RATE) as usize;
        for (i, s) in out.iter_mut().skip(start).take(400).enumerate() {
            *s = (2.0 * PI * 2000.0 * i as f64 / RATE).sin() as f32 * 0.8 * (1.0 - i as f32 / 400.0);
        }
        at += every;
    }
    out
}

#[test]
fn finds_hits_and_the_tempo_they_make() {
    let (mut tap, mut ear) = tap(RATE, 1);
    let mut track = Track::new(ear.sample_rate());
    let audio = clicks(0.5, 8.0);
    let mut hits = Vec::new();
    // Listened to about every 100 ms, as a plugin would.
    for part in audio.chunks(4800) {
        push_all(&mut tap, part, 480);
        let heard = ear.hear().unwrap();
        hits.extend(heard.hits.iter().map(|h| h.at));
        track.add(&heard, None);
    }
    assert_eq!(hits.len(), 16, "{hits:?}");
    for pair in hits.windows(2) {
        let gap = (pair[1] - pair[0]) as f64 / RATE;
        assert!((gap - 0.5).abs() < 0.01, "{gap}");
    }
    let tempo = track.tempo().unwrap();
    assert!((tempo - 120.0).abs() <= 1.0, "{tempo}");
}

#[test]
fn eighth_notes_still_give_the_beat() {
    let (mut tap, mut ear) = tap(RATE, 1);
    let mut track = Track::for_ear(&ear);
    for part in clicks(0.25, 8.0).chunks(4800) {
        push_all(&mut tap, part, 480);
        track.add(&ear.hear().unwrap(), None);
    }
    // 240 is out of the range, so the beat is 120 (eighth notes at 120 a minute).
    let tempo = track.tempo().unwrap();
    assert!((tempo - 120.0).abs() <= 1.0, "{tempo}");
}

#[test]
fn a_new_tap_forgets_the_old_hits() {
    let (mut tap, mut ear) = tap(RATE, 1);
    let mut track = Track::for_ear(&ear);
    for part in clicks(0.5, 8.0).chunks(4800) {
        push_all(&mut tap, part, 480);
        track.add(&ear.hear().unwrap(), None);
    }
    assert!(track.tempo().is_some());
    // The plugin made a new tap (the host restarted it): its count starts again.
    let (mut tap, mut ear) = mo_listen::tap(RATE, 1);
    push_all(&mut tap, &clicks(0.5, 1.0), 480);
    track.add(&ear.hear().unwrap(), None);
    assert_eq!(track.tempo(), None, "2 new hits, the old ones forgotten");
}

#[test]
fn a_late_ear_counts_only_the_notes_it_heard() {
    let (mut tap, mut ear) = tap(RATE, 1);
    let mut track = Track::for_ear(&ear);
    // 0.5 s in one go: the notes come from the newest window only (8,192 samples).
    push_all(&mut tap, &notes(&[60.0], 0.5, 0.5), 480);
    let heard = ear.hear().unwrap();
    assert!((heard.seconds - 0.5).abs() < 1e-9);
    assert!((heard.chroma_seconds - FFT as f64 / RATE).abs() < 1e-9);
    track.add(&heard, Some(1.0));
    assert!((f64::from(track.all().seconds) - FFT as f64 / RATE).abs() < 1e-6);
}

#[test]
fn no_tempo_from_too_few_or_scattered_hits() {
    let (mut tap, mut ear) = tap(RATE, 1);
    let mut track = Track::new(RATE);
    push_all(&mut tap, &clicks(0.5, 2.0), 480);
    track.add(&ear.hear().unwrap(), None);
    assert_eq!(track.tempo(), None, "4 hits are too few");
    assert_eq!(Track::new(RATE).tempo(), None);

    // 24 hits at scattered gaps (0.13 to 0.9 s).
    let (mut tap, mut ear) = mo_listen::tap(RATE, 1);
    let mut track = Track::new(RATE);
    let mut audio = Vec::new();
    let mut seed = 7u32;
    for _ in 0..24 {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let gap = 0.13 + f64::from(seed >> 16) / 65_536.0 * 0.77;
        audio.extend(clicks(10.0, gap));
    }
    for part in audio.chunks(4800) {
        push_all(&mut tap, part, 480);
        track.add(&ear.hear().unwrap(), None);
    }
    assert_eq!(track.tempo(), None);
}

#[test]
fn a_track_adds_up_the_notes_per_bar() {
    let (mut tap, mut ear) = tap(RATE, 1);
    let mut track = Track::new(RATE).with_bar(4.0);
    // At 120 beats a minute a bar is 2 s. Bar 0: A minor (A C E), bar 1: F major (F A C), 100 ms at a time.
    let mut beat = 0.0;
    for chord in [[57.0, 60.0, 64.0], [53.0, 57.0, 60.0]] {
        let audio = notes(&chord, 2.0, 0.5);
        for part in audio.chunks(4800) {
            push_all(&mut tap, part, 480);
            beat += part.len() as f64 / RATE * 2.0;
            // The beat at the end of the audio heard: the last piece of each chord ends right on the bar line.
            track.add(&ear.hear().unwrap(), Some(beat));
        }
    }
    let bars: Vec<i64> = track.bars().map(|(n, _)| n).collect();
    assert_eq!(bars, [0, 1]);
    assert_eq!(loudest(&track.bar(0).unwrap().chroma, 3), [0, 4, 9]);
    assert_eq!(loudest(&track.bar(1).unwrap().chroma, 3), [0, 5, 9]);
    assert!((track.bar(0).unwrap().seconds - 2.0).abs() < 1e-3);
    assert_eq!(track.newest_bar(), Some(1));
    assert!((track.all().seconds - 4.0).abs() < 1e-3);
    // Without a play position the notes still count towards everything heard.
    push_all(&mut tap, &notes(&[57.0], 0.1, 0.5), 480);
    track.add(&ear.hear().unwrap(), None);
    assert!((track.all().seconds - 4.1).abs() < 1e-3);
    track.clear();
    assert_eq!(track.bars().count(), 0);
}

#[test]
fn the_audio_thread_and_the_ear_can_run_at_once() {
    let (mut tap, mut ear) = tap(RATE, 2);
    let audio = notes(&[60.0], 1.0, 0.5);
    let writer = std::thread::spawn(move || {
        for _ in 0..20 {
            push_all(&mut tap, &audio, 128);
        }
        tap.frames()
    });
    let mut last = 0;
    let mut heard_any = false;
    while !writer.is_finished() {
        if let Some(h) = ear.hear() {
            assert!(h.samples_to > last);
            last = h.samples_to;
            heard_any = true;
        }
        std::thread::yield_now();
    }
    let frames = writer.join().unwrap();
    let h = ear.hear();
    let to = h.as_ref().map_or(last, |h| h.samples_to);
    assert_eq!(to, frames);
    assert!(heard_any || h.is_some());
}
