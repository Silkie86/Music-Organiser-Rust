//! Playing a sound through the track the plugin is on.
//!
//! The DAW's audio thread must never wait, read a file or make or free memory: a stall there is a click in the
//! song. So a sound is read and turned to the DAW's sample rate on a background thread at low priority
//! ([`Player::play`]), then handed to the audio thread ready to play through a small queue that never waits
//! ([`rtrb`]). When the audio thread is done with a sound, it hands it back through a second queue, and the panel's
//! thread frees it ([`Player::tidy`]). The newest pick wins: a sound still being read when another is picked is
//! dropped when it's ready.

use rtrb::{Consumer, Producer, RingBuffer};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// The longest stretch of a sound that is read to play: a long loop plays its first minute.
pub const MOST_SECONDS: f64 = 60.0;

/// How loud the sounds play: a little under full, as a preview.
const LEVEL: f32 = 0.8;

/// A sound ready to play: one list per side (a mono sound plays the same on both), at the DAW's sample rate.
pub struct Voice {
    left: Vec<f32>,
    right: Vec<f32>,
    at: usize,
}

impl Voice {
    /// `channels` at `from_rate`, turned to `to_rate` (straight-line steps between samples: good enough to preview).
    pub fn new(channels: &[Vec<f32>], from_rate: u32, to_rate: u32) -> Voice {
        let left = channels.first().map(|c| resample(c, from_rate, to_rate)).unwrap_or_default();
        let right = match channels.get(1) {
            Some(c) => resample(c, from_rate, to_rate),
            None => left.clone(),
        };
        Voice { left, right, at: 0 }
    }

    fn done(&self) -> bool {
        self.at >= self.left.len().min(self.right.len())
    }
}

/// `samples` at `from` per second, as they'd be at `to` per second.
fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 || to == 0 || samples.is_empty() {
        return samples.to_vec();
    }
    let step = f64::from(from) / f64::from(to);
    let len = (samples.len() as f64 / step).floor() as usize;
    (0..len)
        .map(|i| {
            let at = i as f64 * step;
            let first = at.floor() as usize;
            let part = (at - first as f64) as f32;
            let a = samples.get(first).copied().unwrap_or(0.0);
            let b = samples.get(first + 1).copied().unwrap_or(a);
            a + (b - a) * part
        })
        .collect()
}

/// What the panel asks the audio thread to do.
pub enum Ask {
    Play(Box<Voice>),
    Stop,
}

/// The audio thread's side: on the DAW's audio thread, from when the DAW starts the track's audio to when it stops it.
pub struct Playing {
    asks: Consumer<Ask>,
    done: Producer<Box<Voice>>,
    now: Option<Box<Voice>>,
}

impl Playing {
    /// Plays into `left` and `right` (the same length), taking any new asks first. Never waits, never makes or frees
    /// memory: a sound it's done with goes back to the panel's thread to be freed.
    pub fn fill(&mut self, left: &mut [f32], right: &mut [f32]) {
        // An ask is taken only while there's room to hand back the sound it replaces, so nothing is freed here.
        while !self.done.is_full() {
            match self.asks.pop() {
                Ok(Ask::Play(voice)) => self.retire(Some(voice)),
                Ok(Ask::Stop) => self.retire(None),
                Err(_) => break,
            }
        }
        let mut written = 0;
        if let Some(voice) = &mut self.now {
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                if voice.done() {
                    break;
                }
                *l = voice.left[voice.at] * LEVEL;
                *r = voice.right[voice.at] * LEVEL;
                voice.at += 1;
                written += 1;
            }
        }
        left[written..].fill(0.0);
        right[written..].fill(0.0);
        if self.now.as_ref().is_some_and(|v| v.done()) && !self.done.is_full() {
            self.retire(None);
        }
    }

    /// Hands the sound playing now back to the panel's thread and plays `next` instead. Only called while the queue
    /// back has room.
    fn retire(&mut self, next: Option<Box<Voice>>) {
        let old = std::mem::replace(&mut self.now, next);
        if let Some(old) = old
            && let Err(rtrb::PushError::Full(old)) = self.done.push(old)
        {
            // Not reached (the callers check there's room), but if it were, the old sound plays on rather than being
            // freed here.
            self.now = Some(old);
        }
    }
}

/// The panel's side, shared by everything in the plugin that isn't the audio thread.
#[derive(Default)]
pub struct Player {
    asks: Mutex<Option<Producer<Ask>>>,
    done: Mutex<Option<Consumer<Box<Voice>>>>,
    /// The DAW's sample rate while the track's audio runs; 0 when it doesn't.
    rate: AtomicU32,
    /// Counts picks, so only the newest is played.
    picks: AtomicU64,
}

/// What became of a pick, for the panel's line under the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Played {
    Playing(String),
    /// Couldn't play it, and why, in plain words.
    Not(String),
}

impl Player {
    /// The DAW starts the track's audio at `rate` samples a second: the audio thread's side, to give the DAW.
    pub fn start(&self, rate: u32) -> Playing {
        let (asks_in, asks) = RingBuffer::new(4);
        let (done, done_out) = RingBuffer::new(8);
        // Anything still in the old queues is freed here, on the DAW's main thread.
        *lock(&self.asks) = Some(asks_in);
        *lock(&self.done) = Some(done_out);
        self.rate.store(rate, Ordering::Release);
        Playing { asks, done, now: None }
    }

    /// The DAW stopped the track's audio (its side of the queues has gone with it).
    pub fn stop_all(&self) {
        self.rate.store(0, Ordering::Release);
        *lock(&self.asks) = None;
    }

    /// Plays the sound at `path` (on a background thread at low priority); `told` hears how it went, unless a newer
    /// pick came first.
    pub fn play(self: &Arc<Self>, path: PathBuf, told: impl FnOnce(Played) + Send + 'static) {
        let pick = self.picks.fetch_add(1, Ordering::AcqRel) + 1;
        let me = Arc::clone(self);
        let started = std::thread::Builder::new().name("mo-plugin-audition".into()).spawn(move || {
            let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let read = mo_audio::read(&path, MOST_SECONDS);
            if me.picks.load(Ordering::Acquire) != pick {
                return;
            }
            let rate = me.rate.load(Ordering::Acquire);
            let played = match read {
                Err(e) => Played::Not(format!("Couldn't read {name}: {e}")),
                Ok(_) if rate == 0 => Played::Not("The DAW hasn't started this track's audio yet".into()),
                Ok(audio) => {
                    let voice = Box::new(Voice::new(&audio.channels, audio.sample_rate, rate));
                    me.ask(Ask::Play(voice), Played::Playing(name))
                }
            };
            told(played);
        });
        if started.is_err() {
            // No thread to read it on (the system is out of them): nothing plays, and nothing waits.
            self.picks.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Stops what's playing, and any sound still being read.
    pub fn stop(&self) {
        self.picks.fetch_add(1, Ordering::AcqRel);
        let _ = self.ask(Ask::Stop, Played::Not(String::new()));
    }

    fn ask(&self, ask: Ask, then: Played) -> Played {
        match lock(&self.asks).as_mut().map(|asks| asks.push(ask)) {
            Some(Ok(())) => then,
            Some(Err(_)) => Played::Not("Busy: try again".into()),
            None => Played::Not("The DAW hasn't started this track's audio yet".into()),
        }
    }

    /// Frees the sounds the audio thread is done with. Called often from the panel's thread.
    pub fn tidy(&self) {
        if let Some(done) = lock(&self.done).as_mut() {
            while done.pop().is_ok() {}
        }
    }
}

/// A lock that a panic elsewhere left poisoned is still used: what it holds (a queue's end) is always whole.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_halves_and_keeps_the_same_rate() {
        let s = [0.0, 1.0, 2.0, 3.0];
        assert_eq!(resample(&s, 48_000, 48_000), s.to_vec());
        assert_eq!(resample(&s, 48_000, 24_000), vec![0.0, 2.0]);
        assert_eq!(resample(&s, 24_000, 48_000), vec![0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.0]);
    }

    #[test]
    fn a_mono_sound_plays_on_both_sides() {
        let v = Voice::new(&[vec![0.5, 0.25]], 44_100, 44_100);
        assert_eq!(v.left, v.right);
    }

    #[test]
    fn plays_then_hands_the_sound_back_and_goes_quiet() {
        let player = Arc::new(Player::default());
        let mut playing = player.start(48_000);
        assert_eq!(
            player.ask(Ask::Play(Box::new(Voice::new(&[vec![1.0; 3]], 48_000, 48_000))), Played::Playing("a".into())),
            Played::Playing("a".into())
        );
        let (mut l, mut r) = (vec![9.0; 4], vec![9.0; 4]);
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![LEVEL, LEVEL, LEVEL, 0.0]);
        assert_eq!(r, l);
        // Done: it went back to be freed off the audio thread.
        assert!(playing.now.is_none());
        assert_eq!(lock(&player.done).as_ref().unwrap().slots(), 1);
        player.tidy();
        assert_eq!(lock(&player.done).as_ref().unwrap().slots(), 0);
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![0.0; 4]);
    }

    #[test]
    fn stop_hands_the_sound_back() {
        let player = Arc::new(Player::default());
        let mut playing = player.start(48_000);
        player.ask(Ask::Play(Box::new(Voice::new(&[vec![1.0; 100]], 48_000, 48_000))), Played::Playing("a".into()));
        let (mut l, mut r) = (vec![0.0; 4], vec![0.0; 4]);
        playing.fill(&mut l, &mut r);
        player.stop();
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![0.0; 4]);
        assert!(playing.now.is_none());
    }

    #[test]
    fn nothing_plays_before_the_daw_starts_the_audio() {
        let player = Player::default();
        assert!(matches!(player.ask(Ask::Stop, Played::Playing("a".into())), Played::Not(_)));
    }
}
