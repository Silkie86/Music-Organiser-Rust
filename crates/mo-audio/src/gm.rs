//! MIDI parts played with General MIDI sounds from a SoundFont (.sf2), through `rustysynth` (the Rust port of
//! MeltySynth, which the C# app used). The SoundFont is GeneralUser GS by S. Christian Collins (free to use in
//! software, about 31 MB), the copy the C# app downloaded into the app's folder, or any .sf2 you pick.
//!
//! A part is made into sound once, off the window's thread, as a loop that goes round with no gap: it's played twice
//! and the second time is kept, so the notes still sounding at its end (a held chord, a reverb's tail) are heard
//! under its start, as when it goes round in a DAW. The player then plays that sound round and round.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};

use crate::AudioData;

/// The rate parts are made at: the SoundFont's own (GeneralUser GS is 44.1 kHz); the player changes it to the output's.
pub const RATE: u32 = 44_100;

/// The longest loop made, in seconds: a long part is cut here rather than taking memory and time (about 40 MB of
/// sound a minute, made twice).
pub const LONGEST_SECONDS: f64 = 120.0;

/// The MIDI channel drums play on (channel 10, counted from 0).
const DRUM_CHANNEL: i32 = 9;

/// A SoundFont, read once and kept: it's read whole into memory (rule 12, read big files once).
pub type Font = Arc<SoundFont>;

/// Whether the file starts like a SoundFont: "RIFF", its size, then "sfbk".
pub fn looks_like_soundfont(path: &Path) -> bool {
    let mut head = [0u8; 12];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok()
        && &head[..4] == b"RIFF"
        && &head[8..12] == b"sfbk"
}

/// Reads a SoundFont. A damaged one is an error, not a crash: what trips the reader is caught (as a damaged file).
pub fn open(path: &Path) -> Result<Font, String> {
    if !looks_like_soundfont(path) {
        return Err(format!("{} isn't a SoundFont (.sf2)", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    match mo_catch::catch(move || SoundFont::new(&mut bytes.as_slice())) {
        Ok(Ok(font)) => Ok(Arc::new(font)),
        Ok(Err(e)) => Err(format!("Couldn't read the SoundFont {}: {e:?}", path.display())),
        Err(_) => Err(format!("The SoundFont {} is damaged", path.display())),
    }
}

/// One note of a part, in ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    pub start: i64,
    pub length: i64,
    pub key: u8,
    /// MIDI 1.0's velocity, 1 to 127.
    pub velocity: u8,
}

/// A part to make into a loop.
#[derive(Clone, Debug)]
pub struct Part<'a> {
    pub notes: &'a [Note],
    /// Ticks to a beat.
    pub ppq: i64,
    /// How long the loop is, in ticks (a whole number of bars, mostly).
    pub length: i64,
    /// Beats a minute.
    pub tempo: f64,
    /// The GM program, 0 to 127 (left alone for drums).
    pub program: u8,
    pub drums: bool,
}

/// How many frames of sound a tick takes at `tempo` (beats a minute; one that can't be is 120) with `ppq` ticks to a
/// beat.
fn frames_per_tick(tempo: f64, ppq: i64) -> f64 {
    let tempo = if tempo.is_finite() && tempo > 0.0 { tempo.clamp(20.0, 400.0) } else { 120.0 };
    f64::from(RATE) * 60.0 / (tempo * ppq.max(1) as f64)
}

/// The most ticks a loop can be at `tempo` with `ppq` ticks to a beat before it's cut ([`LONGEST_SECONDS`]): what's
/// played together for longer than this has to be made shorter by whole bars, by who asks.
pub fn longest_ticks(ppq: i64, tempo: f64) -> i64 {
    (LONGEST_SECONDS * f64::from(RATE) / frames_per_tick(tempo, ppq)).floor() as i64
}

/// The part as a stereo loop at [`RATE`], ready for `Player::play_looped`. Takes a while for a long part: call it
/// off the window's thread.
pub fn render_loop(font: &Font, part: &Part) -> Result<AudioData, String> {
    render_together(font, std::slice::from_ref(part))
}

/// The most voices the synth sounds at once: for one part, and for several played together (the most it takes).
const VOICES: usize = 128;
const VOICES_TOGETHER: usize = 256;

/// Several parts as one stereo loop at [`RATE`], played by one synth as a band plays: each sound on a channel of its
/// own (parts with the same sound share one), drums on theirs. Each part's notes are placed by its own tempo and
/// ticks to a beat, and the loop is as long as the longest part. One pass makes them all, so six parts take little
/// longer than one and no more memory. Call it off the window's thread.
pub fn render_together(font: &Font, parts: &[Part]) -> Result<AudioData, String> {
    let at = |part: &Part, ticks: i64| (ticks.max(0) as f64 * frames_per_tick(part.tempo, part.ppq)).round() as usize;
    let longest = (LONGEST_SECONDS * f64::from(RATE)) as usize;
    let len = parts.iter().map(|part| at(part, part.length)).max().unwrap_or(0).min(longest);
    if len == 0 {
        return Err("The part has no length".to_owned());
    }

    let mut settings = SynthesizerSettings::new(RATE as i32);
    settings.maximum_polyphony = if parts.len() > 1 { VOICES_TOGETHER } else { VOICES };
    settings.enable_reverb_and_chorus = true;
    let mut synth = Synthesizer::new(font, &settings).map_err(|e| format!("The synth wouldn't start: {e:?}"))?;
    let channels = channels_of(parts);
    for (part, &channel) in parts.iter().zip(&channels) {
        if !part.drums {
            synth.process_midi_message(channel, 0xC0, i32::from(part.program & 0x7f), 0);
        }
    }

    // Every note on and off, for both times round: (frame, off before on at the same frame, channel, key, velocity).
    let mut events: Vec<(usize, bool, i32, u8, u8)> =
        Vec::with_capacity(parts.iter().map(|part| part.notes.len()).sum::<usize>() * 4);
    for round in 0..2 {
        let offset = round * len;
        for (part, &channel) in parts.iter().zip(&channels) {
            for n in part.notes {
                let on = at(part, n.start);
                if on >= len {
                    continue;
                }
                let off = at(part, n.start + n.length.max(1)).max(on + 1);
                events.push((offset + on, false, channel, n.key.min(127), n.velocity.clamp(1, 127)));
                events.push((offset + off, true, channel, n.key.min(127), 0));
            }
        }
    }
    // A note ending where the next starts lets go first.
    events.sort_by_key(|&(frame, off, channel, key, _)| (frame, !off, channel, key));

    // The first time round is made into a small piece of memory used again and again, and only the second is kept:
    // a long loop takes the memory it keeps, not three times it.
    let total = 2 * len;
    let mut left = vec![0.0f32; len];
    let mut right = vec![0.0f32; len];
    let mut scratch = (vec![0.0f32; 4096], vec![0.0f32; 4096]);
    let mut render_to = |synth: &mut Synthesizer, from: usize, to: usize| {
        let mut at = from;
        while at < to.min(len) {
            let n = (to.min(len) - at).min(scratch.0.len());
            synth.render(&mut scratch.0[..n], &mut scratch.1[..n]);
            at += n;
        }
        let from = at.max(len);
        if to > from {
            synth.render(&mut left[from - len..to - len], &mut right[from - len..to - len]);
        }
    };
    // How many notes hold each key of each channel down: a key is let go only when the last of them ends, so a note
    // left over from the first time round doesn't cut short the same note starting again (the synth lets go of
    // every voice of a key).
    let mut held = [[0u32; 128]; CHANNELS];
    let mut done = 0;
    for (frame, off, channel, key, velocity) in events {
        let to = frame.min(total);
        if to > done {
            render_to(&mut synth, done, to);
            done = to;
        }
        let Some(holding) = held.get_mut(channel as usize).and_then(|keys| keys.get_mut(usize::from(key))) else {
            continue;
        };
        if off {
            *holding = holding.saturating_sub(1);
            if *holding == 0 {
                synth.note_off(channel, i32::from(key));
            }
        } else {
            *holding += 1;
            synth.note_on(channel, i32::from(key), i32::from(velocity));
        }
    }
    if done < total {
        render_to(&mut synth, done, total);
    }
    Ok(AudioData {
        sample_rate: RATE,
        channels: vec![left, right],
        file_seconds: len as f64 / f64::from(RATE),
        embedded_is_loop: Some(true),
        ..AudioData::default()
    })
}

/// The synth's MIDI channels.
const CHANNELS: usize = 16;

/// The channel each part plays on: drums on theirs, every other sound on one of its own, in the order the sounds
/// come; parts with the same sound share. With more sounds than the 15 channels there are, the last ones share the
/// last channel (and play its sound).
fn channels_of(parts: &[Part]) -> Vec<i32> {
    let mut given: Vec<(u8, i32)> = Vec::new();
    let mut free = (0..CHANNELS as i32).filter(|&channel| channel != DRUM_CHANNEL);
    let mut last = 0;
    parts
        .iter()
        .map(|part| {
            if part.drums {
                return DRUM_CHANNEL;
            }
            let program = part.program & 0x7f;
            if let Some(&(_, channel)) = given.iter().find(|(has, _)| *has == program) {
                return channel;
            }
            let channel = free.next().unwrap_or(last);
            last = channel;
            given.push((program, channel));
            channel
        })
        .collect()
}

/// How loud a loop is where it has sound: the root mean square of its frames, the silent ones (its rests) left out,
/// so a part of a few notes counts as loud as its notes are. 0 for silence.
pub fn level(sound: &AudioData) -> f32 {
    /// Under this a frame is a rest (about -80 dB).
    const QUIET: f32 = 1e-4;
    let Some(left) = sound.channels.first() else { return 0.0 };
    let right = sound.channels.get(1).unwrap_or(left);
    let (mut squares, mut frames) = (0.0f64, 0u64);
    for (l, r) in left.iter().zip(right) {
        if l.abs() > QUIET || r.abs() > QUIET {
            squares += f64::from(*l) * f64::from(*l) + f64::from(*r) * f64::from(*r);
            frames += 1;
        }
    }
    if frames == 0 { 0.0 } else { (squares / (2 * frames) as f64).sqrt() as f32 }
}

/// One loop among those played together ([`layered`]).
#[derive(Clone, Copy, Debug)]
pub struct Layer<'a> {
    /// The loop, as [`render_loop`] made it (at the tempo and ticks to a beat given to [`layered`]).
    pub sound: &'a AudioData,
    /// How long it is before it goes round, in ticks.
    pub length: i64,
    /// How loud it goes in: 1 for as it is.
    pub gain: f32,
}

/// How long a loop cut short takes to fade out, in frames (5 ms), and how many frames short of its end a loop may
/// stop without it counting as cut (where a bar's length in frames isn't a whole number).
const CUT_FADE: usize = (RATE / 200) as usize;
const CUT_SLACK: usize = 8;

/// Loops played together for `length` ticks as one loop: each goes round by its own length from the start, as clips
/// of different lengths do side by side in a DAW (a four-bar part twice over a song's eight bars). One that isn't
/// back at its start where the whole goes round is cut there, faded over a few milliseconds so it doesn't click.
/// Every loop is at `tempo` with `ppq` ticks to a beat. If together they'd be louder than full, all are turned down
/// alike. Quick (it only adds up sound already made), but it copies up to two minutes of it: call it off the
/// window's thread.
pub fn layered(layers: &[Layer], length: i64, ppq: i64, tempo: f64) -> Result<AudioData, String> {
    let frames_per_tick = frames_per_tick(tempo, ppq);
    let at = |ticks: i64| (ticks.max(0) as f64 * frames_per_tick).round() as usize;
    let longest = (LONGEST_SECONDS * f64::from(RATE)) as usize;
    let len = at(length).min(longest);
    if len == 0 {
        return Err("There's nothing to play together".to_owned());
    }
    let mut left = vec![0.0f32; len];
    let mut right = vec![0.0f32; len];
    for layer in layers {
        if layer.sound.sample_rate != RATE {
            return Err("A loop to play together isn't at the SoundFont's rate".to_owned());
        }
        // One channel plays on both sides.
        let Some(from_left) = layer.sound.channels.first() else { continue };
        let from_right = layer.sound.channels.get(1).unwrap_or(from_left);
        let has = from_left.len().min(from_right.len());
        // A loop with no length, or shorter than a frame, can't go round.
        let own = layer.length;
        if has == 0 || own <= 0 || (own as f64 * frames_per_tick) < 1.0 {
            continue;
        }
        let mut turn = 0i64;
        loop {
            // Each time round starts where its ticks say, so the turns never drift from the bar lines.
            let start = at(turn.saturating_mul(own));
            if start >= len {
                break;
            }
            turn += 1;
            let next = at(turn.saturating_mul(own)).max(start + 1);
            // A turn can be a frame longer than the loop's sound (a bar is seldom a whole number of frames, and
            // each turn starts on the frame nearest its bar line): it goes on with the loop's start, as the loop
            // does going round, so there's no frame of silence where two turns meet. A turn much longer than the
            // sound (a loop cut at the longest there is) has no more to give and rests.
            let whole = next.min(len) - start;
            let take = if next - start <= has + CUT_SLACK { whole } else { whole.min(has) };
            // Cut by the whole's end, with more of it still to come: faded out.
            let cut = next > len && has > take + CUT_SLACK;
            let fade = if cut { CUT_FADE.min(take) } else { 0 };
            for i in 0..take {
                let gain =
                    if i + fade >= take { layer.gain * (take - i) as f32 / fade.max(1) as f32 } else { layer.gain };
                if let (Some(l), Some(r), Some(a), Some(b)) =
                    (left.get_mut(start + i), right.get_mut(start + i), from_left.get(i % has), from_right.get(i % has))
                {
                    *l += a * gain;
                    *r += b * gain;
                }
            }
        }
    }
    let peak = left.iter().chain(&right).fold(0.0f32, |peak, s| peak.max(s.abs()));
    if peak > 1.0 {
        for s in left.iter_mut().chain(&mut right) {
            *s /= peak;
        }
    }
    Ok(AudioData {
        sample_rate: RATE,
        channels: vec![left, right],
        file_seconds: len as f64 / f64::from(RATE),
        embedded_is_loop: Some(true),
        ..AudioData::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A loop of `frames` frames, each frame its own place in the loop times `step` (left) and that halved (right).
    fn ramp(frames: usize, step: f32) -> AudioData {
        let left: Vec<f32> = (0..frames).map(|i| i as f32 * step).collect();
        let right: Vec<f32> = left.iter().map(|s| s / 2.0).collect();
        AudioData { sample_rate: RATE, channels: vec![left, right], ..AudioData::default() }
    }

    /// 60 beats a minute with 441 ticks to a beat: a tick is 100 frames exactly.
    const PPQ: i64 = 441;
    const TEMPO: f64 = 60.0;

    #[test]
    fn loops_played_together_each_go_round_by_their_own_length() {
        // Two ticks and three ticks, for six: three times round and twice.
        let (short, long) = (ramp(200, 0.001), ramp(300, 0.0001));
        let layers = [Layer { sound: &short, length: 2, gain: 1.0 }, Layer { sound: &long, length: 3, gain: 0.5 }];
        let both = layered(&layers, 6, PPQ, TEMPO).unwrap();
        assert_eq!((both.len(), both.channels.len(), both.sample_rate), (600, 2, RATE));
        assert_eq!(both.embedded_is_loop, Some(true));
        assert!((both.file_seconds - 600.0 / f64::from(RATE)).abs() < 1e-9);
        let near = |got: f32, want: f32| (got - want).abs() < 1e-6;
        for (frame, in_short, in_long) in [(0, 0, 0), (199, 199, 199), (200, 0, 200), (300, 100, 0), (599, 199, 299)] {
            let want = in_short as f32 * 0.001 + in_long as f32 * 0.0001 * 0.5;
            assert!(near(both.channels[0][frame], want), "left at {frame}: {}", both.channels[0][frame]);
            assert!(near(both.channels[1][frame], want / 2.0), "right at {frame}");
        }
        // One channel plays on both sides.
        let mono = AudioData { channels: vec![vec![0.25; 200]], ..ramp(0, 0.0) };
        let one = layered(&[Layer { sound: &mono, length: 2, gain: 1.0 }], 4, PPQ, TEMPO).unwrap();
        assert!(one.channels.iter().all(|c| c.len() == 400 && c.iter().all(|s| near(*s, 0.25))));
    }

    #[test]
    fn a_loop_cut_where_the_whole_goes_round_fades_out() {
        // Thirty ticks over fifty: the second time round is cut after twenty.
        let steady = AudioData { sample_rate: RATE, channels: vec![vec![0.5; 3000]; 2], ..AudioData::default() };
        let cut = layered(&[Layer { sound: &steady, length: 30, gain: 1.0 }], 50, PPQ, TEMPO).unwrap();
        let left = &cut.channels[0];
        assert_eq!(left.len(), 5000);
        // Whole the first time round and into the second; down to next to nothing at the cut, over the last 5 ms.
        assert!(left[..5000 - CUT_FADE].iter().all(|s| (*s - 0.5).abs() < 1e-6));
        assert!(left[4999] < 0.01 && left[4999] > 0.0, "the last frame: {}", left[4999]);
        assert!(left[5000 - CUT_FADE..].windows(2).all(|w| w[1] < w[0]), "it fades all the way");
        assert_eq!(cut.channels[1], *left);
        // A loop a few frames longer than its ticks say (a bar that isn't a whole number of frames) isn't cut: no fade.
        let over = AudioData { sample_rate: RATE, channels: vec![vec![0.5; 303]; 2], ..AudioData::default() };
        let whole = layered(&[Layer { sound: &over, length: 3, gain: 1.0 }], 6, PPQ, TEMPO).unwrap();
        assert!(whole.channels[0].iter().all(|s| (*s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn loops_together_are_never_louder_than_full_and_keep_to_the_bar_lines() {
        let loud = AudioData { sample_rate: RATE, channels: vec![vec![0.8; 200]; 2], ..AudioData::default() };
        let soft = AudioData { sample_rate: RATE, channels: vec![vec![-0.2; 100], vec![0.4; 100]], ..loud.clone() };
        let layers = [Layer { sound: &loud, length: 2, gain: 1.0 }, Layer { sound: &soft, length: 1, gain: 2.0 }];
        let both = layered(&layers, 2, PPQ, TEMPO).unwrap();
        // 0.8 + 0.8 on the right is the loudest: everything comes down by that much.
        assert!(both.channels[1].iter().all(|s| (*s - 1.0).abs() < 1e-6));
        assert!(both.channels[0].iter().all(|s| (*s - 0.4 / 1.6).abs() < 1e-6));
        // Quiet enough already: left as it is.
        let quiet = layered(&[Layer { sound: &soft, length: 1, gain: 1.0 }], 2, PPQ, TEMPO).unwrap();
        assert!(quiet.channels[1].iter().all(|s| (*s - 0.4).abs() < 1e-6));

        // At 128 beats a minute a bar of 4/4 is 82,687.5 frames: every time round still starts on its bar line, and
        // no frame of the whole is left silent.
        let bar = 4 * 96;
        let per_bar = |bars: i64| (bars as f64 * 82_687.5).round() as usize;
        let one_bar = AudioData { sample_rate: RATE, channels: vec![vec![0.5; per_bar(1)]; 2], ..AudioData::default() };
        let mut marked = one_bar.clone();
        marked.channels[0][0] = 0.9;
        let eight = layered(&[Layer { sound: &marked, length: bar, gain: 1.0 }], 8 * bar, 96, 128.0).unwrap();
        assert_eq!(eight.len(), per_bar(8));
        let starts: Vec<usize> =
            eight.channels[0].iter().enumerate().filter(|(_, s)| **s > 0.8).map(|(i, _)| i).collect();
        assert_eq!(starts, (0..8).map(per_bar).collect::<Vec<_>>());
        assert!(eight.channels[1].iter().all(|s| *s >= 0.5 - 1e-6), "no gap between one time round and the next");
    }

    #[test]
    fn nothing_to_play_together_is_an_error_not_silence() {
        let loop_ = ramp(200, 0.001);
        assert!(layered(&[Layer { sound: &loop_, length: 2, gain: 1.0 }], 0, PPQ, TEMPO).is_err());
        let other_rate = AudioData { sample_rate: 48_000, ..loop_.clone() };
        assert!(layered(&[Layer { sound: &other_rate, length: 2, gain: 1.0 }], 4, PPQ, TEMPO).is_err());
        // So short that the whole is less than a frame.
        assert!(layered(&[Layer { sound: &loop_, length: 2, gain: 1.0 }], 2, 1_000_000_000, TEMPO).is_err());
        // A loop with no sound in it, or with no length, adds nothing; the others still play.
        let empty = AudioData { sample_rate: RATE, ..AudioData::default() };
        let layers = [
            Layer { sound: &empty, length: 2, gain: 1.0 },
            Layer { sound: &loop_, length: 0, gain: 1.0 },
            Layer { sound: &loop_, length: 2, gain: 1.0 },
        ];
        assert_eq!(layered(&layers, 2, PPQ, TEMPO).unwrap().channels, loop_.channels);
        let silent = layered(&layers[..2], 4, PPQ, TEMPO).unwrap();
        assert!(silent.len() == 400 && silent.channels.iter().flatten().all(|s| *s == 0.0));
    }

    /// A loop whose length isn't a whole number of frames: some turns are a frame longer than its sound, and that
    /// frame is the loop's start again, never a frame of silence in the middle of the sound.
    #[test]
    fn where_two_turns_of_a_loop_meet_theres_no_frame_of_silence() {
        // 1,000 ticks to a beat at 60: a tick is 44.1 frames, so three ticks are 132.3 and the loop is made 132
        // frames long. Turns start at frames 0, 132, 265 (264.6), 397, 529...: the second is 133 frames.
        let ppq = 1000;
        let loop_ = ramp(132, 0.001);
        let both = layered(&[Layer { sound: &loop_, length: 3, gain: 1.0 }], 30, ppq, TEMPO).unwrap();
        assert_eq!(both.len(), 1323);
        let left = &both.channels[0];
        assert!((left[263] - 0.131).abs() < 1e-6 && (left[265] - 0.0).abs() < 1e-6);
        assert!((left[264] - 0.0).abs() < 1e-6, "the frame over is the loop's first again: {}", left[264]);
        let steady = AudioData { channels: vec![vec![0.25; 132]], ..ramp(0, 0.0) };
        let both = layered(&[Layer { sound: &steady, length: 3, gain: 1.0 }], 30, ppq, TEMPO).unwrap();
        assert!(both.channels.iter().flatten().all(|s| (*s - 0.25).abs() < 1e-6), "no hole anywhere");
        // A loop with far less sound than its length (it was cut at the longest there is) rests for what's left
        // of each turn: it isn't played again early.
        let short = AudioData { channels: vec![vec![0.25; 100]], ..ramp(0, 0.0) };
        let both = layered(&[Layer { sound: &short, length: 3, gain: 1.0 }], 6, ppq, TEMPO).unwrap();
        let left = &both.channels[0];
        assert!(left[..100].iter().all(|s| *s == 0.25) && left[100..132].iter().all(|s| *s == 0.0));
        assert!(left[132..232].iter().all(|s| *s == 0.25) && left[232..].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn each_sound_plays_on_a_channel_of_its_own_and_drums_on_theirs() {
        let part = |program: u8, drums: bool| Part { notes: &[], ppq: 96, length: 96, tempo: 120.0, program, drums };
        // The same sound shares a channel; drums are on channel 10 (9 from 0), which no other sound is given.
        let parts = [part(4, false), part(38, false), part(0, true), part(4, false), part(80, false)];
        assert_eq!(channels_of(&parts), [0, 1, 9, 0, 2]);
        // Twenty sounds: fifteen channels, and the last sounds share the last.
        let many: Vec<Part> = (0..20).map(|program| part(program, false)).collect();
        let channels = channels_of(&many);
        assert_eq!(channels[..10], [0, 1, 2, 3, 4, 5, 6, 7, 8, 10]);
        assert_eq!(channels[10..], [11, 12, 13, 14, 15, 15, 15, 15, 15, 15]);
        assert!(channels_of(&[]).is_empty());
    }

    #[test]
    fn how_loud_a_loop_is_leaves_its_rests_out() {
        // A steady tone for a quarter of the loop is as loud as the same tone all through.
        let tone = |frames: usize, of: usize| {
            let channel: Vec<f32> = (0..of).map(|i| if i < frames { 0.5 } else { 0.0 }).collect();
            AudioData { sample_rate: RATE, channels: vec![channel.clone(), channel], ..AudioData::default() }
        };
        assert!((level(&tone(400, 400)) - 0.5).abs() < 1e-6);
        assert!((level(&tone(100, 400)) - 0.5).abs() < 1e-6);
        // One channel counts for both sides; silence and no sound at all are 0.
        let mono = AudioData { sample_rate: RATE, channels: vec![vec![0.2; 50]], ..AudioData::default() };
        assert!((level(&mono) - 0.2).abs() < 1e-6);
        assert_eq!((level(&tone(0, 400)), level(&AudioData::default())), (0.0, 0.0));
    }

    #[test]
    fn the_longest_loop_in_ticks_follows_the_tempo() {
        // Two minutes at 120 with 96 ticks to a beat: 240 beats.
        assert_eq!(longest_ticks(96, 120.0), 240 * 96);
        assert_eq!(longest_ticks(96, 60.0), 120 * 96);
        // A tempo that can't be is 120, as when the loop is made.
        assert_eq!(longest_ticks(96, f64::NAN), 240 * 96);
    }

    #[test]
    fn only_a_soundfont_opens() {
        let dir = std::env::temp_dir().join(format!("mo-audio-gm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let not = dir.join("not.sf2");
        std::fs::write(&not, b"RIFF\0\0\0\0WAVEfmt ").unwrap();
        assert!(!looks_like_soundfont(&not));
        assert!(open(&not).is_err());
        let damaged = dir.join("damaged.sf2");
        std::fs::write(&damaged, b"RIFF\x04\0\0\0sfbkLIST").unwrap();
        assert!(looks_like_soundfont(&damaged));
        assert!(open(&damaged).is_err());
        assert!(open(&dir.join("missing.sf2")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With a real SoundFont (MO_SOUNDFONT=path): a chord, a bass and drums each make a loop of the right length with
    /// sound in it, and the held chord's end is heard under the loop's start.
    #[test]
    #[ignore = "needs a SoundFont: MO_SOUNDFONT=path cargo test -p mo-audio --features gm -- --ignored"]
    fn parts_make_loops_with_a_real_soundfont() {
        let Some(path) = std::env::var_os("MO_SOUNDFONT") else { return };
        let started = std::time::Instant::now();
        let font = open(Path::new(&path)).unwrap();
        println!("read in {:?}", started.elapsed());
        let chord: Vec<Note> =
            [60, 64, 67].iter().map(|&key| Note { start: 0, length: 4 * 96 * 2, key, velocity: 100 }).collect();
        let kick: Vec<Note> = (0..8).map(|i| Note { start: i * 96, length: 48, key: 36, velocity: 110 }).collect();
        for (notes, program, drums) in [(&chord, 4, false), (&chord, 38, false), (&kick, 0, true)] {
            let part = Part { notes, ppq: 96, length: 2 * 4 * 96, tempo: 120.0, program, drums };
            let started = std::time::Instant::now();
            let sound = render_loop(&font, &part).unwrap();
            println!("made in {:?}", started.elapsed());
            // Two bars at 120 = 4 seconds.
            assert_eq!(sound.len(), 4 * RATE as usize);
            let loud = sound.channels[0].iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(loud > 0.01 && loud < 1.5, "{loud}");
            // A chord held through the whole loop sounds at its very start (what the end left).
            if !drums {
                let start = sound.channels[0][..64].iter().fold(0.0f32, |m, s| m.max(s.abs()));
                assert!(start > 0.0005, "{start}");
            }
        }
    }
}
