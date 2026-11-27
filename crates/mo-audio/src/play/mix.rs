//! The part of the player that runs on the sound output's own thread: it takes the newest sound to play, changes its
//! rate and channels to the output's, and fades between sounds so a sweep doesn't click. It never waits, never reads a
//! file, and hands old sounds back to be freed elsewhere. Orders come through a fixed-size channel, so taking them
//! never takes or frees memory. A long sound comes a part at a time as it's read ([`Flow`]), through a fixed-size
//! ring that's never waited on either.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering, fence};
use std::sync::mpsc::{Receiver, TryRecvError};

use super::stream::{Parts, Said};
use crate::{AudioData, AudioError};

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
        /// Goes round to its start at its end, with no gap, until it's stopped or replaced.
        looping: bool,
        /// For a sound played as it's read: `sound` is its first part, and the rest come through this.
        flow: Option<Box<Flow>>,
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
    /// What's left of the sound `id`, played as it's read, to be let go off the output thread: its parts, and the
    /// channel they came through (which stops its reader).
    DoneFlow(u64, Box<Flow>),
}

/// A sound played as it's read ([`crate::play::Stream`]): what comes after the part playing, as it comes. Made on the
/// player's thread and handed back to it to be let go, so the output's thread never frees one.
pub(crate) struct Flow {
    /// The frame of the file the part playing starts at, for where it is in the file.
    pub(crate) offset: u64,
    /// The part played before it, for the curve's sample before its first.
    before: Option<Arc<AudioData>>,
    /// The part after it, once that has come.
    next: Option<Arc<AudioData>>,
    /// Where the parts come from. The reader waits while it's full, and stops once this is let go.
    more: Parts,
    /// What the read came to, once it has finished (none for parts made in memory): read only on the player's thread.
    said: Option<Said>,
    /// Every part has come: the reader has finished.
    all: bool,
}

impl Flow {
    /// The parts that come through `more`, after one starting at frame `offset` of the file, and what the read came to
    /// through `said`.
    pub(crate) fn new(offset: u64, more: Parts, said: Option<Said>) -> Flow {
        Flow { offset, before: None, next: None, more, said, all: false }
    }

    /// Why its read stopped before the file's end (the drive unplugged, the file moved or renamed), once every part it
    /// read has played. Never waits: the reader says it before it lets the parts go.
    pub(crate) fn why_ended(&self) -> Option<AudioError> {
        if !self.all {
            return None;
        }
        self.said.as_ref()?.try_recv().ok()?.err()
    }

    /// The part after the one playing, if it has come.
    fn take_next(&mut self) -> Option<Arc<AudioData>> {
        if let Some(next) = self.next.take() {
            return Some(next);
        }
        if let Ok(part) = self.more.pop() {
            return Some(part);
        }
        if !self.more.is_abandoned() {
            return None;
        }
        // The reader has finished: what it handed on before is all there now (rtrb's own advice, for Rust 1.74 on).
        fence(Ordering::Acquire);
        let last = self.more.pop().ok();
        self.all = last.is_none();
        last
    }
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
    /// How many frames the output has asked for while a sound played: it's working while this goes up.
    played: AtomicU64,
}

impl Place {
    /// How many frames the output has asked for while a sound played.
    pub(crate) fn played(&self) -> u64 {
        self.played.load(Ordering::Relaxed)
    }

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
    /// The sound, or for one played as it's read, the part playing.
    sound: Arc<AudioData>,
    /// Where it is, in the sound's own frames (a fraction when its rate differs from the output's).
    at: f64,
    gain: f32,
    target: f32,
    looping: bool,
    /// The parts after `sound`, for a sound played as it's read.
    flow: Option<Box<Flow>>,
}

impl Voice {
    /// Where it is in its file, in seconds.
    fn seconds(&self) -> f64 {
        let offset = self.flow.as_ref().map_or(0, |f| f.offset);
        (offset as f64 + self.at) / f64::from(self.sound.sample_rate.max(1))
    }
}

/// How the mixer tells the player something; it must not wait.
pub(crate) type Tell = Box<dyn FnMut(FromMixer) + Send>;

/// Mixes the sound playing (and the one fading out) into the output's buffers.
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
        if self.voice.is_some() {
            let frames = (out.len() / channels) as u64;
            self.place.played.store(self.place.played().saturating_add(frames), Ordering::Relaxed);
        }
        let fade_step = 1.0 / (FADE_SECONDS * rate as f32).max(1.0);
        if let Some(v) = &mut self.fading
            && add(v, out, rate, channels, fade_step, &mut self.to_player)
        {
            self.retire_fading();
        }
        if let Some(v) = &mut self.voice {
            let ended = add(v, out, rate, channels, fade_step, &mut self.to_player);
            self.place.set(v.id, v.seconds());
            if ended {
                let id = v.id;
                if let Some(v) = self.voice.take() {
                    self.retire(v);
                }
                (self.to_player)(FromMixer::Ended(id));
            }
        }
    }

    fn take_orders(&mut self) {
        loop {
            match self.from_player.try_recv() {
                Ok(ToMixer::Play { id, sound, from, looping, flow }) => {
                    self.fade_out_current();
                    // Part way into its file, it fades in, so the jump doesn't click.
                    let gain = if from > 0.0 || flow.as_ref().is_some_and(|f| f.offset > 0) { 0.0 } else { 1.0 };
                    let at =
                        if looping && !sound.is_empty() { from.max(0.0) % sound.len() as f64 } else { from.max(0.0) };
                    self.voice = Some(Voice { id, sound, at, gain, target: 1.0, looping, flow });
                }
                Ok(ToMixer::Seek { id, new_id, from }) => {
                    let Some(v) = self.voice.as_ref().filter(|v| v.id == id) else {
                        // It ended (or was replaced) before the move came: say the moved one has ended too, so a
                        // move just as a sound ends doesn't leave it "playing" for ever. A replaced one's end is
                        // ignored, as its id is no longer the one playing.
                        (self.to_player)(FromMixer::Ended(new_id));
                        continue;
                    };
                    if v.flow.is_some() {
                        // Only the parts near where it is are here: the player reads it again from there instead
                        // (a new play), so this never comes. Should it, it stops, rather than play on unseen.
                        self.fade_out_current();
                        (self.to_player)(FromMixer::Ended(new_id));
                        continue;
                    }
                    let at = if v.looping && !v.sound.is_empty() {
                        from.max(0.0) % v.sound.len() as f64
                    } else {
                        from.max(0.0)
                    };
                    let moved = Voice {
                        id: new_id,
                        sound: Arc::clone(&v.sound),
                        at,
                        gain: 0.0,
                        target: 1.0,
                        looping: v.looping,
                        flow: None,
                    };
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
            self.retire(v);
        }
    }

    /// Hands what a voice holds back to be freed: freeing a long sound's memory can take a while, so it's never done
    /// on the output's thread.
    fn retire(&mut self, v: Voice) {
        (self.to_player)(FromMixer::Done(v.sound));
        if let Some(flow) = v.flow {
            (self.to_player)(FromMixer::DoneFlow(v.id, flow));
        }
    }
}

/// Adds `v` into `out`, whichever way it plays ([`mix`], or [`mix_flow`] for one played as it's read). True when it's
/// finished: played to the end, or faded out. `tell` takes the parts a flow has played.
fn add(v: &mut Voice, out: &mut [f32], rate: u32, channels: usize, fade_step: f32, tell: &mut Tell) -> bool {
    if v.flow.is_some() {
        mix_flow(v, out, rate, channels, fade_step, tell)
    } else {
        mix(v, out, rate, channels, fade_step)
    }
}

/// Moves a voice's `gain` a step towards its `target`. True once it has faded out (or was let go before it had faded
/// in at all).
fn fade(gain: &mut f32, target: f32, fade_step: f32) -> bool {
    if *gain < target {
        *gain = (*gain + fade_step).min(target);
    } else if *gain > target {
        *gain = (*gain - fade_step).max(target);
    }
    target <= 0.0 && *gain <= 0.0
}

/// Puts the two sides of a frame into `frame` (one speaker gets both sides mixed).
fn put(frame: &mut [f32], l: f32, r: f32) {
    match frame {
        [only] => *only += 0.5 * (l + r),
        [a, b, ..] => {
            *a += l;
            *b += r;
        }
        [] => {}
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
    let wrap = v.looping.then_some(len);
    for frame in out.chunks_exact_mut(channels) {
        if v.at >= len as f64 {
            if !v.looping {
                return true;
            }
            v.at -= len as f64;
        }
        if fade(&mut v.gain, v.target, fade_step) {
            return true;
        }
        // Channels past the first two (centre, surrounds, LFE) go into both sides at -3 dB, so nothing in a quad or
        // 5.1 file goes silent (as C#).
        let folded: f32 = rest.iter().map(|ch| cubic(ch, v.at, wrap)).sum::<f32>() * FOLD_SHARE;
        let l = (cubic(left, v.at, wrap) + folded) * v.gain;
        let r = (cubic(right, v.at, wrap) + folded) * v.gain;
        put(frame, l, r);
        v.at += step;
    }
    // A loop's place is always inside it, also at the end of a buffer.
    if v.looping && v.at >= len as f64 {
        v.at -= len as f64;
    }
    false
}

/// [`mix`] for a sound played as it's read: on through its parts as they come, the curve reaching into the parts
/// either side of a join, so the joins can't be heard. When the next part hasn't come yet (the read fell behind), it
/// waits silent where it is and fades in again once it comes. Finished once its last part has played. The parts it has
/// played go to `tell`, to be freed off the output's thread.
fn mix_flow(v: &mut Voice, out: &mut [f32], rate: u32, channels: usize, fade_step: f32, tell: &mut Tell) -> bool {
    let Some(flow) = v.flow.as_deref_mut() else { return true };
    if v.sound.sample_rate == 0 {
        return true;
    }
    for frame in out.chunks_exact_mut(channels) {
        // On to the next part once this one has played.
        while v.at >= v.sound.len() as f64 {
            let Some(part) = flow.take_next() else {
                // The last part played, or it was fading out anyway.
                if flow.all || v.target <= 0.0 {
                    return true;
                }
                // Silent until more comes; then it fades in, so the gap doesn't click.
                v.gain = 0.0;
                return false;
            };
            let played = v.sound.len();
            if let Some(gone) = flow.before.replace(std::mem::replace(&mut v.sound, part)) {
                tell(FromMixer::Done(gone));
            }
            v.at -= played as f64;
            flow.offset += played as u64;
        }
        // The curve near the end of a part needs the next one's first samples.
        if flow.next.is_none() && v.at + 3.0 >= v.sound.len() as f64 {
            flow.next = flow.take_next();
        }
        if fade(&mut v.gain, v.target, fade_step) {
            return true;
        }
        let near = Near { before: flow.before.as_deref(), now: &v.sound, next: flow.next.as_deref() };
        let (i, t) = (v.at.floor(), (v.at - v.at.floor()) as f32);
        let i = i as isize;
        let at = |c: usize| {
            let p1 = near.get(c, i);
            if t == 0.0 { p1 } else { curve(near.get(c, i - 1), p1, near.get(c, i + 1), near.get(c, i + 2), t) }
        };
        let count = v.sound.channels.len();
        let (left, right) = match count {
            0 => (0.0, 0.0),
            1 => (at(0), at(0)),
            _ => (at(0), at(1)),
        };
        let folded: f32 = (2..count).map(at).sum::<f32>() * FOLD_SHARE;
        put(frame, (left + folded) * v.gain, (right + folded) * v.gain);
        v.at += f64::from(v.sound.sample_rate) / f64::from(rate);
    }
    false
}

/// The parts about a place in a sound played as it's read: the one playing and those either side of it.
struct Near<'a> {
    before: Option<&'a AudioData>,
    now: &'a AudioData,
    next: Option<&'a AudioData>,
}

impl Near<'_> {
    /// Channel `c`'s sample `k` frames into the part playing (before its start, or past its end, in the part there),
    /// or silence where there's none.
    fn get(&self, c: usize, k: isize) -> f32 {
        let len = self.now.len() as isize;
        let (part, k) = if k < 0 {
            (self.before, k + self.before.map_or(0, |b| b.len() as isize))
        } else if k >= len {
            (self.next, k - len)
        } else {
            (Some(self.now), k)
        };
        let k = usize::try_from(k).ok();
        part.and_then(|p| p.channels.get(c)).and_then(|ch| ch.get(k?)).copied().unwrap_or(0.0)
    }
}

/// The sound between samples, by a cubic curve through the four nearest (silence before the start and after the end,
/// or, for a loop `wrap` frames long, the other end of the loop).
fn cubic(s: &[f32], at: f64, wrap: Option<usize>) -> f32 {
    let i = at.floor();
    let t = (at - i) as f32;
    let i = i as isize;
    let get = |k: isize| {
        let k = match wrap {
            Some(len) if len > 0 => Some((i + k).rem_euclid(len as isize) as usize),
            _ => usize::try_from(i + k).ok(),
        };
        k.and_then(|k| s.get(k)).copied().unwrap_or(0.0)
    };
    if t == 0.0 {
        return get(0);
    }
    curve(get(-1), get(0), get(1), get(2), t)
}

/// The curve through four samples in a row, `t` of the way from the second to the third: Catmull-Rom, which passes
/// through every sample, so a sound at the output's own rate comes out unchanged.
fn curve(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
    let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c = -0.5 * p0 + 0.5 * p2;
    ((a * t + b) * t + c) * t + p1
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_loop_goes_round_with_no_gap() {
        let (mut m, tx, rx, _) = mixer();
        let ramp: Vec<f32> = (0..10).map(|i| i as f32 / 10.0).collect();
        tx.send(ToMixer::Play { id: 1, sound: sound(100, vec![ramp.clone()]), from: 0.0, looping: true, flow: None })
            .unwrap();
        let mut out = vec![0.0; 25];
        m.render(&mut out, 100, 1);
        let want: Vec<f32> = (0..25).map(|i| ramp[i % 10]).collect();
        assert!(out.iter().zip(&want).all(|(a, b)| (a - b).abs() < 1e-6), "{out:?}");
        // It never says it ended.
        assert!(rx.try_iter().all(|e| !matches!(e, FromMixer::Ended(_))));
    }

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
        tx.send(ToMixer::Play {
            id: 1,
            sound: sound(48_000, vec![samples.clone()]),
            from: 0.0,
            looping: false,
            flow: None,
        })
        .unwrap();
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
        tx.send(ToMixer::Play {
            id: 1,
            sound: sound(24_000, vec![vec![0.5; 100], vec![-0.5; 100]]),
            from: 0.0,
            looping: false,
            flow: None,
        })
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
        tx.send(ToMixer::Play {
            id: 1,
            sound: sound(100, vec![vec![0.4; 10], vec![0.2; 10]]),
            from: 0.0,
            looping: false,
            flow: None,
        })
        .unwrap();
        let mut out = vec![0.0; 4];
        m.render(&mut out, 100, 1);
        assert!((out[1] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn a_new_sound_fades_the_old_one_out_without_a_click() {
        let (mut m, tx, rx, place) = mixer();
        tx.send(ToMixer::Play {
            id: 1,
            sound: sound(48_000, vec![vec![1.0; 48_000]]),
            from: 0.0,
            looping: false,
            flow: None,
        })
        .unwrap();
        let mut out = vec![0.0; 2 * 64];
        m.render(&mut out, 48_000, 2);
        tx.send(ToMixer::Play {
            id: 2,
            sound: sound(48_000, vec![vec![0.0; 48_000]]),
            from: 0.0,
            looping: false,
            flow: None,
        })
        .unwrap();
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
        tx.send(ToMixer::Play { id: 5, sound: sound(1000, vec![ramp]), from: 0.0, looping: false, flow: None })
            .unwrap();
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
        tx.send(ToMixer::Play {
            id: 1,
            sound: sound(1000, vec![vec![1.0; 1000]]),
            from: 0.0,
            looping: false,
            flow: None,
        })
        .unwrap();
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
        tx.send(ToMixer::Play { id: 1, sound: sound(100, five), from: 0.0, looping: false, flow: None }).unwrap();
        let mut out = vec![0.0; 4];
        m.render(&mut out, 100, 2);
        assert!((out[0] - (0.1 + 0.7 * FOLD_SHARE)).abs() < 1e-6);
        assert!((out[1] - (0.2 + 0.7 * FOLD_SHARE)).abs() < 1e-6);
    }

    /// A sound played as it's read, in `parts` (mono, at `rate`), after a first part starting at frame `offset` of its
    /// file: the first part, the flow with the rest in it, and the reader's end of the ring, unless `done` (the reader
    /// has finished, and let go of it).
    fn flow_of(
        rate: u32,
        parts: &[&[f32]],
        offset: u64,
        done: bool,
    ) -> (Arc<AudioData>, Box<Flow>, Option<rtrb::Producer<Arc<AudioData>>>) {
        let (mut tx, rx) = rtrb::RingBuffer::new(16);
        let mut parts = parts.iter().map(|p| sound(rate, vec![p.to_vec()]));
        let first = parts.next().unwrap_or_default();
        for p in parts {
            tx.push(p).unwrap();
        }
        (first, Box::new(Flow::new(offset, rx, None)), (!done).then_some(tx))
    }

    fn play_flow(tx: &Sender<ToMixer>, first: Arc<AudioData>, flow: Box<Flow>) {
        tx.send(ToMixer::Play { id: 1, sound: first, from: 0.0, looping: false, flow: Some(flow) }).unwrap();
    }

    #[test]
    fn a_flow_plays_its_parts_one_after_another_and_ends_after_the_last() {
        let (mut m, tx, rx, place) = mixer();
        let ramp: Vec<f32> = (0..12).map(|i| i as f32 / 12.0).collect();
        let (first, flow, _) = flow_of(100, &[&ramp[..4], &ramp[4..7], &ramp[7..]], 0, true);
        play_flow(&tx, first, flow);
        let mut out = vec![0.0; 20];
        m.render(&mut out, 100, 1);
        // Sample for sample across the joins, then silence after the last part.
        assert_eq!(&out[..12], &ramp[..]);
        assert!(out[12..].iter().all(|s| *s == 0.0));
        assert_eq!(place.of(1), Some(0.12));
        let got: Vec<_> = rx.try_iter().collect();
        assert!(matches!(got.last(), Some(FromMixer::Ended(1))));
        // The parts played, and what's left of the flow, go back to be freed.
        assert_eq!(got.iter().filter(|m| matches!(m, FromMixer::Done(_))).count(), 2);
        assert_eq!(got.iter().filter(|m| matches!(m, FromMixer::DoneFlow(1, _))).count(), 1);
    }

    #[test]
    fn a_flow_from_part_way_says_where_it_is_in_its_file_and_fades_in() {
        let (mut m, tx, _rx, place) = mixer();
        let (first, flow, _more) = flow_of(1000, &[&[0.6; 10], &[0.6; 10]], 1_000, false);
        play_flow(&tx, first, flow);
        let mut out = vec![0.0; 15];
        m.render(&mut out, 1000, 1);
        // 3 ms is 3 frames here.
        assert!((out[0] - 0.2).abs() < 1e-6 && (out[1] - 0.4).abs() < 1e-6, "{out:?}");
        assert_eq!(&out[2..], &[0.6; 13]);
        assert_eq!(place.of(1), Some(1.015));
    }

    #[test]
    fn a_flow_that_runs_dry_waits_where_it_is_and_fades_back_in() {
        let (mut m, tx, rx, place) = mixer();
        let (first, flow, more) = flow_of(1000, &[&[0.6; 10]], 0, false);
        play_flow(&tx, first, flow);
        let mut out = vec![0.0; 20];
        m.render(&mut out, 1000, 1);
        // Its part, then silence: waiting at the end of what came, not ended.
        assert_eq!(&out[..10], &[0.6; 10]);
        assert!(out[10..].iter().all(|s| *s == 0.0));
        assert_eq!(place.of(1), Some(0.01));
        assert!(rx.try_iter().all(|m| !matches!(m, FromMixer::Ended(_))));
        // More comes: it goes on from there, fading in, and waits again at its end.
        let mut more = more.unwrap();
        more.push(sound(1000, vec![vec![0.6; 10]])).unwrap();
        let mut out = vec![0.0; 20];
        m.render(&mut out, 1000, 1);
        assert!((out[0] - 0.2).abs() < 1e-6, "{}", out[0]);
        assert_eq!(&out[2..10], &[0.6; 8]);
        assert!(out[10..].iter().all(|s| *s == 0.0));
        assert_eq!(place.of(1), Some(0.02));
        // The reader finishes: it ends.
        drop(more);
        m.render(&mut out, 1000, 1);
        assert!(rx.try_iter().any(|m| matches!(m, FromMixer::Ended(1))));
    }

    #[test]
    fn at_another_rate_a_flow_sounds_as_its_whole_sound_would() {
        let wave: Vec<f32> = (0..40).map(|i| (i as f32 * 0.7).sin()).collect();
        let (mut whole, tx, _rx, _) = mixer();
        tx.send(ToMixer::Play { id: 1, sound: sound(300, vec![wave.clone()]), from: 0.0, looping: false, flow: None })
            .unwrap();
        let mut want = vec![0.0; 100];
        whole.render(&mut want, 700, 1);
        let (mut m, tx, _rx, _) = mixer();
        let (first, flow, _) = flow_of(300, &[&wave[..13], &wave[13..27], &wave[27..]], 0, true);
        play_flow(&tx, first, flow);
        let mut got = vec![0.0; 100];
        m.render(&mut got, 700, 1);
        assert!(got.iter().zip(&want).all(|(a, b)| (a - b).abs() < 1e-6), "{got:?}\n{want:?}");
        assert!(want[50] != 0.0 && want[95] == 0.0);
    }

    #[test]
    fn a_flow_stopped_hands_back_its_channel_which_stops_its_reader() {
        let (mut m, tx, rx, _) = mixer();
        let (first, flow, more) = flow_of(1000, &[&[0.5; 100]], 0, false);
        play_flow(&tx, first, flow);
        let mut out = vec![0.0; 10];
        m.render(&mut out, 1000, 1);
        tx.send(ToMixer::Stop).unwrap();
        m.render(&mut out, 1000, 1);
        let got: Vec<_> = rx.try_iter().collect();
        assert!(got.iter().any(|m| matches!(m, FromMixer::DoneFlow(1, _))));
        assert!(!got.iter().any(|m| matches!(m, FromMixer::Ended(_))));
        // Let go on the player's thread: the reader sees it has gone, so it stops.
        let more = more.unwrap();
        assert!(!more.is_abandoned());
        drop(got);
        assert!(more.is_abandoned());
    }

    #[test]
    fn cubic_passes_through_the_samples_and_stays_silent_outside() {
        let s = [0.0, 1.0, 0.0, -1.0];
        assert_eq!(cubic(&s, 1.0, None), 1.0);
        assert_eq!(cubic(&s, 3.0, None), -1.0);
        assert_eq!(cubic(&s, 10.0, None), 0.0);
        let mid = cubic(&s, 1.5, None);
        assert!(mid > 0.0 && mid < 1.0);
    }
}
