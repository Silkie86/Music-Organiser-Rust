//! The part of the player that runs on the sound output's own thread: it takes the newest sound to play, changes its
//! rate and channels to the output's, and fades between sounds so a sweep doesn't click. It never waits, never reads a
//! file and never frees a sound (it hands old ones back to be freed elsewhere).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering, fence};
use std::sync::mpsc::{Receiver, TryRecvError};

use crate::AudioData;

/// How long a sound takes to fade out when another replaces it or it's stopped, and to fade in when it starts part way
/// through: 3 ms, too short to hear as a fade, long enough that the jump isn't a click.
const FADE_SECONDS: f32 = 0.003;

/// How much of each channel past the first two goes into each side: -3 dB.
const FOLD_SHARE: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// What the player tells the output thread.
pub(crate) enum ToMixer {
    /// Play this sound from this frame (in the sound's own frames), replacing what's playing.
    Play {
        id: u64,
        sound: Arc<AudioData>,
        from: f64,
    },
    /// Move the sound `id` to this frame, if it's still the one playing; from then on it's called `new_id`, so its
    /// place isn't mistaken for the one before the move.
    Seek {
        id: u64,
        new_id: u64,
        from: f64,
    },
    Stop,
}

/// What the output thread tells the player.
pub(crate) enum FromMixer {
    /// The sound `id` played to its end.
    Ended(u64),
    /// A sound the output has finished with, to be freed off the output thread.
    Done(Arc<AudioData>),
}

/// Where the sound playing is, for the player to read at any time without waiting: its id and its place in seconds.
///
/// The output thread is the only writer. A count that's odd while it writes tells a reader to try again, so a place
/// is never matched with the wrong sound.
#[derive(Default)]
pub(crate) struct Place {
    count: AtomicU64,
    id: AtomicU64,
    seconds: AtomicU64,
}

impl Place {
    /// The place of the sound `id`, or `None` if the output hasn't played any of it yet.
    pub(crate) fn of(&self, id: u64) -> Option<f64> {
        // The writer takes a few nanoseconds, so this tries again only a handful of times; the bound is only a guard.
        for _ in 0..10_000 {
            let before = self.count.load(Ordering::Acquire);
            if before % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let read = self.id.load(Ordering::Relaxed);
            let seconds = f64::from_bits(self.seconds.load(Ordering::Relaxed));
            fence(Ordering::Acquire);
            if self.count.load(Ordering::Relaxed) == before {
                return (read == id).then_some(seconds);
            }
        }
        None
    }

    fn set(&self, id: u64, seconds: f64) {
        let count = self.count.load(Ordering::Relaxed);
        self.count.store(count.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.id.store(id, Ordering::Relaxed);
        self.seconds.store(seconds.to_bits(), Ordering::Relaxed);
        self.count.store(count.wrapping_add(2), Ordering::Release);
    }
}

struct Voice {
    id: u64,
    sound: Arc<AudioData>,
    /// Where it is, in the sound's own frames (a fraction when its rate differs from the output's).
    at: f64,
    gain: f32,
    target: f32,
}

/// Mixes the sound playing (and the one fading out) into the output's buffers.
/// How the mixer tells the player something; it must not wait.
pub(crate) type Tell = Box<dyn FnMut(FromMixer) + Send>;

pub(crate) struct Mixer {
    from_player: Receiver<ToMixer>,
    to_player: Tell,
    place: Arc<Place>,
    voice: Option<Voice>,
    fading: Option<Voice>,
}

impl Mixer {
    pub(crate) fn new(from_player: Receiver<ToMixer>, to_player: Tell, place: Arc<Place>) -> Mixer {
        Mixer { from_player, to_player, place, voice: None, fading: None }
    }

    /// Fills `out` (frames of `channels` samples, one after another) with what's playing, at the output's `rate`.
    pub(crate) fn render(&mut self, out: &mut [f32], rate: u32, channels: usize) {
        out.fill(0.0);
        if channels == 0 || rate == 0 {
            return;
        }
        self.take_orders();
        let fade_step = 1.0 / (FADE_SECONDS * rate as f32).max(1.0);
        if let Some(v) = &mut self.fading
            && mix(v, out, rate, channels, fade_step)
        {
            self.retire_fading();
        }
        if let Some(v) = &mut self.voice {
            let ended = mix(v, out, rate, channels, fade_step);
            let seconds = v.at / f64::from(v.sound.sample_rate.max(1));
            self.place.set(v.id, seconds);
            if ended {
                let id = v.id;
                if let Some(v) = self.voice.take() {
                    self.give_back(v.sound);
                }
                (self.to_player)(FromMixer::Ended(id));
            }
        }
    }

    fn take_orders(&mut self) {
        loop {
            match self.from_player.try_recv() {
                Ok(ToMixer::Play { id, sound, from }) => {
                    self.fade_out_current();
                    let gain = if from > 0.0 { 0.0 } else { 1.0 };
                    self.voice = Some(Voice { id, sound, at: from.max(0.0), gain, target: 1.0 });
                }
                Ok(ToMixer::Seek { id, new_id, from }) => {
                    let Some(v) = self.voice.as_ref().filter(|v| v.id == id) else {
                        // It ended (or was replaced) before the move came: say the moved one has ended too, so a
                        // move just as a sound ends doesn't leave it "playing" for ever. A replaced one's end is
                        // ignored, as its id is no longer the one playing.
                        (self.to_player)(FromMixer::Ended(new_id));
                        continue;
                    };
                    let moved =
                        Voice { id: new_id, sound: Arc::clone(&v.sound), at: from.max(0.0), gain: 0.0, target: 1.0 };
                    self.fade_out_current();
                    self.voice = Some(moved);
                }
                Ok(ToMixer::Stop) => self.fade_out_current(),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }

    /// The sound playing starts fading out; one already fading out stops at once (only the newest two are heard).
    fn fade_out_current(&mut self) {
        if let Some(mut v) = self.voice.take() {
            v.target = 0.0;
            self.retire_fading();
            self.fading = Some(v);
        }
    }

    fn retire_fading(&mut self) {
        if let Some(v) = self.fading.take() {
            self.give_back(v.sound);
        }
    }

    fn give_back(&mut self, sound: Arc<AudioData>) {
        // Freeing a long sound's memory can take a while, so it's never done on the output's thread.
        (self.to_player)(FromMixer::Done(sound));
    }
}

/// Adds `v` into `out`. True when it's finished: played to the end, or faded out.
fn mix(v: &mut Voice, out: &mut [f32], rate: u32, channels: usize, fade_step: f32) -> bool {
    let sound = &*v.sound;
    let len = sound.len();
    if len == 0 || sound.sample_rate == 0 {
        return true;
    }
    let step = f64::from(sound.sample_rate) / f64::from(rate);
    let (left, right, rest) = match sound.channels.as_slice() {
        [] => return true,
        [only] => (only.as_slice(), only.as_slice(), &[][..]),
        [l, r, rest @ ..] => (l.as_slice(), r.as_slice(), rest),
    };
    for frame in out.chunks_exact_mut(channels) {
        if v.at >= len as f64 {
            return true;
        }
        if v.gain < v.target {
            v.gain = (v.gain + fade_step).min(v.target);
        } else if v.gain > v.target {
            v.gain = (v.gain - fade_step).max(v.target);
            if v.gain <= 0.0 {
                return true;
            }
        }
        // Channels past the first two (centre, surrounds, LFE) go into both sides at -3 dB, so nothing in a quad or
        // 5.1 file goes silent (as C#).
        let folded: f32 = rest.iter().map(|ch| cubic(ch, v.at)).sum::<f32>() * FOLD_SHARE;
        let l = (cubic(left, v.at) + folded) * v.gain;
        let r = (cubic(right, v.at) + folded) * v.gain;
        match frame {
            [only] => *only += 0.5 * (l + r),
            [a, b, ..] => {
                *a += l;
                *b += r;
            }
            [] => {}
        }
        v.at += step;
    }
    false
}

/// The sound between samples, by a cubic curve through the four nearest (silence before the start and after the end).
fn cubic(s: &[f32], at: f64) -> f32 {
    let i = at.floor();
    let t = (at - i) as f32;
    let i = i as isize;
    let get = |k: isize| usize::try_from(i + k).ok().and_then(|k| s.get(k)).copied().unwrap_or(0.0);
    let (p0, p1, p2, p3) = (get(-1), get(0), get(1), get(2));
    if t == 0.0 {
        return p1;
    }
    // Catmull-Rom: passes through every sample, so a sound at the output's own rate comes out unchanged.
    let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
    let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c = -0.5 * p0 + 0.5 * p2;
    ((a * t + b) * t + c) * t + p1
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, Sender, channel};

    use super::*;

    fn sound(rate: u32, channels: Vec<Vec<f32>>) -> Arc<AudioData> {
        Arc::new(AudioData { sample_rate: rate, channels, ..AudioData::default() })
    }

    fn mixer() -> (Mixer, Sender<ToMixer>, Receiver<FromMixer>, Arc<Place>) {
        let (to_mixer, from_player) = channel();
        let (to_player, from_mixer) = channel();
        let place = Arc::new(Place::default());
        let tell: Tell = Box::new(move |m| {
            let _ = to_player.send(m);
        });
        (Mixer::new(from_player, tell, Arc::clone(&place)), to_mixer, from_mixer, place)
    }

    #[test]
    fn same_rate_comes_out_unchanged_and_mono_goes_to_both_sides() {
        let (mut m, tx, rx, place) = mixer();
        let samples: Vec<f32> = (0..100).map(|i| (i as f32 / 100.0) - 0.5).collect();
        tx.send(ToMixer::Play { id: 1, sound: sound(48_000, vec![samples.clone()]), from: 0.0 }).unwrap();
        let mut out = vec![0.0; 2 * 60];
        m.render(&mut out, 48_000, 2);
        for (i, f) in out.as_chunks::<2>().0.iter().enumerate() {
            assert_eq!(f[0], samples[i]);
            assert_eq!(f[1], samples[i]);
        }
        assert_eq!(place.of(1), Some(60.0 / 48_000.0));
        assert_eq!(place.of(2), None);
        let mut out = vec![0.0; 2 * 60];
        m.render(&mut out, 48_000, 2);
        assert_eq!(out[2 * 39], samples[99]);
        assert!(out[2 * 40..].iter().all(|s| *s == 0.0));
        let got: Vec<_> = rx.try_iter().collect();
        assert!(matches!(got.as_slice(), [FromMixer::Done(_), FromMixer::Ended(1)]));
    }

    #[test]
    fn half_rate_plays_twice_as_long() {
        let (mut m, tx, rx, _) = mixer();
        tx.send(ToMixer::Play { id: 1, sound: sound(24_000, vec![vec![0.5; 100], vec![-0.5; 100]]), from: 0.0 })
            .unwrap();
        let mut out = vec![0.0; 2 * 300];
        m.render(&mut out, 48_000, 2);
        let heard = out.as_chunks::<2>().0.iter().take_while(|f| f[0] != 0.0).count();
        assert_eq!(heard, 200);
        assert_eq!(out[2 * 100], 0.5);
        assert_eq!(out[2 * 100 + 1], -0.5);
        assert!(rx.try_iter().any(|m| matches!(m, FromMixer::Ended(1))));
    }

    #[test]
    fn one_speaker_gets_both_sides_mixed() {
        let (mut m, tx, _rx, _) = mixer();
        tx.send(ToMixer::Play { id: 1, sound: sound(100, vec![vec![0.4; 10], vec![0.2; 10]]), from: 0.0 }).unwrap();
        let mut out = vec![0.0; 4];
        m.render(&mut out, 100, 1);
        assert!((out[1] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn a_new_sound_fades_the_old_one_out_without_a_click() {
        let (mut m, tx, rx, place) = mixer();
        tx.send(ToMixer::Play { id: 1, sound: sound(48_000, vec![vec![1.0; 48_000]]), from: 0.0 }).unwrap();
        let mut out = vec![0.0; 2 * 64];
        m.render(&mut out, 48_000, 2);
        tx.send(ToMixer::Play { id: 2, sound: sound(48_000, vec![vec![0.0; 48_000]]), from: 0.0 }).unwrap();
        let mut out = vec![0.0; 2 * 400];
        m.render(&mut out, 48_000, 2);
        // The old sound comes down a little each frame (3 ms is 144 frames at 48 kHz), not all at once.
        assert!(out[0] > 0.98 && out[0] < 1.0);
        let frames = out.as_chunks::<2>().0;
        assert!(frames.iter().zip(&frames[1..]).all(|(a, b)| a[0] - b[0] < 0.01));
        assert_eq!(out[2 * 200], 0.0);
        assert!(place.of(2).is_some());
        assert!(rx.try_iter().any(|m| matches!(m, FromMixer::Done(s) if s.channels[0][0] == 1.0)));
    }

    #[test]
    fn seek_moves_only_the_sound_playing() {
        let (mut m, tx, rx, place) = mixer();
        let ramp: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        tx.send(ToMixer::Play { id: 5, sound: sound(1000, vec![ramp]), from: 0.0 }).unwrap();
        let mut out = vec![0.0; 10];
        m.render(&mut out, 1000, 1);
        tx.send(ToMixer::Seek { id: 4, new_id: 6, from: 900.0 }).unwrap();
        m.render(&mut out, 1000, 1);
        assert_eq!(place.of(5), Some(0.02));
        assert!(rx.try_iter().any(|m| matches!(m, FromMixer::Ended(6))));
        tx.send(ToMixer::Seek { id: 5, new_id: 7, from: 500.0 }).unwrap();
        m.render(&mut out, 1000, 1);
        assert_eq!(place.of(5), None);
        assert_eq!(place.of(7), Some(0.51));
    }

    #[test]
    fn stop_fades_out_and_hands_the_sound_back() {
        let (mut m, tx, rx, _) = mixer();
        tx.send(ToMixer::Play { id: 1, sound: sound(1000, vec![vec![1.0; 1000]]), from: 0.0 }).unwrap();
        let mut out = vec![0.0; 10];
        m.render(&mut out, 1000, 1);
        tx.send(ToMixer::Stop).unwrap();
        m.render(&mut out, 1000, 1);
        assert!(out[0] > 0.0 && out[9] == 0.0);
        let got: Vec<_> = rx.try_iter().collect();
        assert!(matches!(got.as_slice(), [FromMixer::Done(_)]));
    }

    #[test]
    fn surround_channels_fold_into_both_sides() {
        let (mut m, tx, _rx, _) = mixer();
        let five = vec![vec![0.1; 10], vec![0.2; 10], vec![0.3; 10], vec![0.4; 10], vec![0.0; 10]];
        tx.send(ToMixer::Play { id: 1, sound: sound(100, five), from: 0.0 }).unwrap();
        let mut out = vec![0.0; 4];
        m.render(&mut out, 100, 2);
        assert!((out[0] - (0.1 + 0.7 * FOLD_SHARE)).abs() < 1e-6);
        assert!((out[1] - (0.2 + 0.7 * FOLD_SHARE)).abs() < 1e-6);
    }

    #[test]
    fn cubic_passes_through_the_samples_and_stays_silent_outside() {
        let s = [0.0, 1.0, 0.0, -1.0];
        assert_eq!(cubic(&s, 1.0), 1.0);
        assert_eq!(cubic(&s, 3.0), -1.0);
        assert_eq!(cubic(&s, 10.0), 0.0);
        let mid = cubic(&s, 1.5);
        assert!(mid > 0.0 && mid < 1.0);
    }
}
