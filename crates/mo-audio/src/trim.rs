//! Trim silence: where a sound's quiet start and quiet end are, so it can be played without them. Nothing here
//! changes a file: the app and the plugin only skip the silence as they play (rule 2).

use crate::AudioData;

/// The levels you can pick for what counts as silence, in dB below full scale, loudest first: -40 cuts a noisy
/// recording's hiss too, -70 keeps a reverb's last whisper.
pub const LEVELS_DB: [i32; 4] = [-40, -50, -60, -70];

/// The level picked until you pick another: -60 dB (a thousandth of full scale). Quiet enough that a sample pack's
/// noise floor or dither still counts as silence, loud enough that a soft fade-in doesn't.
pub const DEFAULT_DB: i32 = -60;

/// One of [`LEVELS_DB`] as kept in a setting ("-50"); [`DEFAULT_DB`] for anything else.
pub fn level_db(setting: Option<&str>) -> i32 {
    setting.and_then(|s| s.trim().parse().ok()).filter(|db| LEVELS_DB.contains(db)).unwrap_or(DEFAULT_DB)
}

/// A level in dB below full scale as a sample's size (-60 dB: 0.001).
pub fn level_of_db(db: i32) -> f32 {
    10f32.powf(db as f32 / 20.0)
}

/// How long the sound keeps going after its last sample above the level, so a tail fading out isn't cut
/// in the middle of its last swing (10 ms, a few cycles of a low note).
const TAIL_SECONDS: f64 = 0.010;

/// The part of `audio` that isn't silence, as frames `start..end` (`end` not included): from the first frame louder
/// than `level` ([`level_of_db`]) on any channel to just past the last (plus a short tail). `None` when it's silent
/// all through (or empty): play it as it is.
pub fn sound_span(audio: &AudioData, level: f32) -> Option<(usize, usize)> {
    let frames = audio.len();
    let loud = |i: usize| audio.channels.iter().any(|c| c.get(i).is_some_and(|s| s.abs() > level));
    let first = (0..frames).find(|&i| loud(i))?;
    let last = (first..frames).rev().find(|&i| loud(i)).unwrap_or(first);
    let tail = (TAIL_SECONDS * f64::from(audio.sample_rate)).ceil() as usize;
    Some((first, (last + 1 + tail).min(frames)))
}

/// What to play of `audio` for a play asked to start at frame `from`, with Trim silence on: frames `start..end`,
/// starting past the silence at the start (or at `from`, when that's later) and ending with the sound ([`sound_span`]).
/// `None` when there's nothing to trim to: it's silent all through, or `from` is in the silence at the end (play it
/// as asked).
pub fn play_span(audio: &AudioData, from: usize, level: f32) -> Option<(usize, usize)> {
    let (first, end) = sound_span(audio, level)?;
    let start = from.max(first);
    (start < end).then_some((start, end))
}

impl AudioData {
    /// The seconds of silence at the start ([`sound_span`]); 0 when it starts loud or is silent all through.
    pub fn lead_silence_seconds(&self, level: f32) -> f64 {
        match sound_span(self, level) {
            Some((start, _)) if self.sample_rate > 0 => start as f64 / f64::from(self.sample_rate),
            _ => 0.0,
        }
    }

    /// Cuts the silence at the end off ([`sound_span`]), keeping the start where it is (so a place in the sound, a
    /// waveform's click or a play's position, still means the same second of the file). Nothing is cut from a sound
    /// that's silent all through.
    pub fn cut_end_silence(&mut self, level: f32) {
        if let Some((_, end)) = sound_span(self, level) {
            for channel in &mut self.channels {
                channel.truncate(end);
            }
        }
    }

    /// Cuts the silence off both ends ([`sound_span`]), for a player that plays from the start; a sound that's silent
    /// all through stays as it is.
    pub fn trim_silence(&mut self, level: f32) {
        if let Some((start, end)) = sound_span(self, level) {
            for channel in &mut self.channels {
                channel.truncate(end);
                channel.drain(..start);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: f32 = 0.001;

    fn audio(channels: Vec<Vec<f32>>) -> AudioData {
        AudioData { sample_rate: 1_000, channels, ..AudioData::default() }
    }

    #[test]
    fn finds_the_loud_part_on_any_channel_with_a_short_tail() {
        // 1,000 frames a second: the tail is 10 frames.
        let mut left = vec![0.0; 100];
        let mut right = vec![0.0005; 100]; // A noise floor under -60 dB is still silence.
        left[20] = 0.5;
        right[40] = -0.2;
        let a = audio(vec![left, right]);
        assert_eq!(sound_span(&a, L), Some((20, 51)));
        assert!((a.lead_silence_seconds(L) - 0.020).abs() < 1e-9);
    }

    #[test]
    fn the_tail_stops_at_the_end_and_a_loud_start_cuts_nothing_there() {
        let a = audio(vec![vec![0.5; 100]]);
        assert_eq!(sound_span(&a, L), Some((0, 100)));
        assert_eq!(a.lead_silence_seconds(L), 0.0);
    }

    #[test]
    fn silence_all_through_or_nothing_is_left_as_it_is() {
        let mut quiet = audio(vec![vec![0.0005; 50], vec![0.0; 50]]);
        assert_eq!(sound_span(&quiet, L), None);
        quiet.trim_silence(L);
        quiet.cut_end_silence(L);
        assert_eq!(quiet.len(), 50);
        assert_eq!(sound_span(&audio(vec![]), L), None);
        assert_eq!(audio(vec![]).lead_silence_seconds(L), 0.0);
    }

    #[test]
    fn a_play_starts_past_the_silence_or_at_a_later_spot_and_ends_with_the_sound() {
        let mut s = vec![0.0; 100];
        s[20..50].fill(0.5);
        let a = audio(vec![s]);
        assert_eq!(play_span(&a, 0, L), Some((20, 60)));
        assert_eq!(play_span(&a, 30, L), Some((30, 60)));
        assert_eq!(play_span(&a, 70, L), None, "a spot in the quiet end plays as asked");
        assert_eq!(play_span(&audio(vec![vec![0.0; 10]]), 0, L), None);
    }

    #[test]
    fn levels_read_from_a_setting_and_turn_into_sizes() {
        assert_eq!(level_db(Some("-50")), -50);
        assert_eq!(level_db(Some("-55")), DEFAULT_DB, "only a level the menu offers");
        assert_eq!(level_db(Some("loud")), DEFAULT_DB);
        assert_eq!(level_db(None), DEFAULT_DB);
        assert!((level_of_db(-60) - 0.001).abs() < 1e-7);
        assert!((level_of_db(-40) - 0.01).abs() < 1e-6);
        // A sound at -45 dB is sound at -50 dB, silence at -40 dB.
        let a = audio(vec![vec![level_of_db(-45); 10]]);
        assert_eq!(sound_span(&a, level_of_db(-50)), Some((0, 10)));
        assert_eq!(sound_span(&a, level_of_db(-40)), None);
    }

    #[test]
    fn cutting_keeps_every_channel_the_same_length() {
        let mut left = vec![0.0; 100];
        left[30] = 1.0;
        let mut a = audio(vec![left.clone(), vec![0.0; 100]]);
        a.cut_end_silence(L);
        assert_eq!((a.channels[0].len(), a.channels[1].len()), (41, 41));
        assert_eq!(a.channels[0][30], 1.0, "the start stays where it was");
        let mut b = audio(vec![left, vec![0.0; 100]]);
        b.trim_silence(L);
        assert_eq!((b.channels[0].len(), b.channels[1].len()), (11, 11));
        assert_eq!(b.channels[0][0], 1.0, "it starts on the first loud frame");
    }
}
