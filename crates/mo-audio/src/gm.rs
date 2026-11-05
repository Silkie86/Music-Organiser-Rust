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

/// The part as a stereo loop at [`RATE`], ready for `Player::play_looped`. Takes a while for a long part: call it
/// off the window's thread.
pub fn render_loop(font: &Font, part: &Part) -> Result<AudioData, String> {
    let tempo = if part.tempo.is_finite() && part.tempo > 0.0 { part.tempo.clamp(20.0, 400.0) } else { 120.0 };
    let frames_per_tick = f64::from(RATE) * 60.0 / (tempo * part.ppq.max(1) as f64);
    let at = |ticks: i64| (ticks.max(0) as f64 * frames_per_tick).round() as usize;
    let longest = (LONGEST_SECONDS * f64::from(RATE)) as usize;
    let len = at(part.length).min(longest);
    if len == 0 {
        return Err("The part has no length".to_owned());
    }

    let mut settings = SynthesizerSettings::new(RATE as i32);
    settings.maximum_polyphony = 128;
    settings.enable_reverb_and_chorus = true;
    let mut synth = Synthesizer::new(font, &settings).map_err(|e| format!("The synth wouldn't start: {e:?}"))?;
    let channel = if part.drums { DRUM_CHANNEL } else { 0 };
    if !part.drums {
        synth.process_midi_message(channel, 0xC0, i32::from(part.program & 0x7f), 0);
    }

    // Every note on and off, for both times round: (frame, off before on at the same frame, key, velocity).
    let mut events: Vec<(usize, bool, u8, u8)> = Vec::with_capacity(part.notes.len() * 4);
    for round in 0..2 {
        let offset = round * len;
        for n in part.notes {
            let on = at(n.start);
            if on >= len {
                continue;
            }
            let off = at(n.start + n.length.max(1)).max(on + 1);
            events.push((offset + on, false, n.key.min(127), n.velocity.clamp(1, 127)));
            events.push((offset + off, true, n.key.min(127), 0));
        }
    }
    // A note ending where the next starts lets go first.
    events.sort_by_key(|&(frame, off, key, _)| (frame, !off, key));

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
    // How many notes hold each key down: a key is let go only when the last of them ends, so a note left over from
    // the first time round doesn't cut short the same note starting again (the synth lets go of every voice of a key).
    let mut held = [0u32; 128];
    let mut done = 0;
    for (frame, off, key, velocity) in events {
        let to = frame.min(total);
        if to > done {
            render_to(&mut synth, done, to);
            done = to;
        }
        let k = usize::from(key);
        if off {
            held[k] = held[k].saturating_sub(1);
            if held[k] == 0 {
                synth.note_off(channel, i32::from(key));
            }
        } else {
            held[k] += 1;
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

#[cfg(test)]
mod tests {
    use super::*;

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
