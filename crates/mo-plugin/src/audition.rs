//! Playing a sound through the track the plugin is on.
//!
//! The DAW's audio thread must never wait, read a file or make or free memory: a stall there is a click in the
//! song. So a sound is read and turned to the DAW's sample rate on a background thread at low priority
//! ([`Player::play`]), then handed to the audio thread ready to play through a small queue that never waits
//! ([`rtrb`]). When the audio thread is done with a sound, it hands it back through a second queue, and the panel's
//! thread frees it ([`Player::tidy`]).
//!
//! The newest pick wins: one reading thread at a time takes the newest pick waiting, so clicking down a list reads
//! only the sound you stop on (and the one being read when you started).

use crate::threads::lock;
use rtrb::{Consumer, Producer, RingBuffer};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The longest stretch of a sound that is read to play: a long loop plays its first minute.
pub const MOST_SECONDS: f64 = 60.0;

/// How loud the sounds play: a little under full, as a preview.
const LEVEL: f32 = 0.8;

const NOT_STARTED: &str = "The DAW hasn't started this track's audio yet";

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

/// What the audio thread counts for the panel (plain numbers only: nothing there waits).
#[derive(Default)]
struct Counts {
    /// Sounds it has started.
    started: AtomicU64,
    /// Sounds that have ended: played to the end, stopped or replaced.
    ended: AtomicU64,
}

/// The audio thread's side: on the DAW's audio thread, from when the DAW starts the track's audio to when it stops it.
pub struct Playing {
    asks: Consumer<Ask>,
    done: Producer<Box<Voice>>,
    now: Option<Box<Voice>>,
    /// A sound to hand back that didn't fit in the queue back: handed back first, before anything else is taken.
    /// (The queue is checked for room first, so this stays empty; it's here so nothing is ever freed on this thread.)
    waiting: Option<Box<Voice>>,
    counts: Arc<Counts>,
}

impl Playing {
    /// Plays into `left` and `right` (the same length), taking any new asks first. Never waits, never makes or frees
    /// memory: a sound it's done with goes back to the panel's thread to be freed.
    pub fn fill(&mut self, left: &mut [f32], right: &mut [f32]) {
        if let Some(back) = self.waiting.take()
            && let Err(rtrb::PushError::Full(back)) = self.done.push(back)
        {
            self.waiting = Some(back);
        }
        // An ask is taken only while there's room to hand back the sound it replaces.
        while self.waiting.is_none() && !self.done.is_full() {
            match self.asks.pop() {
                Ok(Ask::Play(voice)) => {
                    self.counts.started.fetch_add(1, Ordering::AcqRel);
                    self.retire(Some(voice));
                }
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
        if self.now.as_ref().is_some_and(|v| v.done()) && self.waiting.is_none() {
            self.retire(None);
        }
    }

    /// Hands the sound playing now back to the panel's thread and plays `next` instead.
    fn retire(&mut self, next: Option<Box<Voice>>) {
        if let Some(old) = std::mem::replace(&mut self.now, next) {
            self.counts.ended.fetch_add(1, Ordering::AcqRel);
            if let Err(rtrb::PushError::Full(old)) = self.done.push(old) {
                self.waiting = Some(old);
            }
        }
    }
}

/// What's to be read next.
struct Pick {
    path: PathBuf,
    number: u64,
    told: Box<dyn FnOnce(Played) + Send>,
}

/// What became of a pick, for the panel's line under the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Played {
    /// Playing the sound named: the `n`th handed to the audio thread ([`Player::still_playing`]).
    Playing(String, u64),
    /// Couldn't play it, and why, in plain words.
    Not(String),
}

/// The panel's side, shared by everything in the plugin that isn't the audio thread.
#[derive(Default)]
pub struct Player {
    asks: Mutex<Option<Producer<Ask>>>,
    done: Mutex<Option<Consumer<Box<Voice>>>>,
    /// The DAW's sample rate while the track's audio runs; 0 when it doesn't.
    rate: AtomicU32,
    /// The newest pick still to be read: picks made while a sound is read replace each other here.
    next: Mutex<Option<Pick>>,
    /// Whether the reading thread is running.
    reading: AtomicBool,
    /// Counts picks and stops, so a sound read for a pick that's no longer the newest isn't played.
    picks: AtomicU64,
    /// How many sounds were handed to the audio thread.
    handed: AtomicU64,
    counts: Arc<Counts>,
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
        self.settle();
        Playing { asks, done, now: None, waiting: None, counts: Arc::clone(&self.counts) }
    }

    /// The DAW stopped the track's audio (its side of the queues has gone with it, and what was playing).
    pub fn stop_all(&self) {
        self.rate.store(0, Ordering::Release);
        *lock(&self.asks) = None;
        self.settle();
    }

    /// Nothing is playing: every sound handed over counts as started and ended.
    fn settle(&self) {
        let handed = self.handed.load(Ordering::Acquire);
        self.counts.started.store(handed, Ordering::Release);
        self.counts.ended.store(handed, Ordering::Release);
    }

    /// Plays the sound at `path`, read on a background thread at low priority; `told` hears how it went, unless a
    /// newer pick came first.
    pub fn play(self: &Arc<Self>, path: PathBuf, told: impl FnOnce(Played) + Send + 'static) {
        let number = self.picks.fetch_add(1, Ordering::AcqRel) + 1;
        *lock(&self.next) = Some(Pick { path, number, told: Box::new(told) });
        if self.reading.swap(true, Ordering::AcqRel) {
            // The reading thread is running: it takes this pick next.
            return;
        }
        let me = Arc::clone(self);
        crate::threads::background("mo-plugin-audition", move || {
            // Marked as stopped however the thread ends (a read that panics on a damaged file too), so the next pick
            // starts a new one.
            struct Stopped<'a>(&'a AtomicBool);
            impl Drop for Stopped<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::Release);
                }
            }
            let _stopped = Stopped(&me.reading);
            loop {
                while let Some(pick) = take_next(&me) {
                    me.read(pick);
                }
                me.reading.store(false, Ordering::Release);
                // A pick made just as it stopped would find it running and leave the pick to it: so look once more,
                // and go on if there's one and no new thread has taken over.
                if lock(&me.next).is_none() || me.reading.swap(true, Ordering::AcqRel) {
                    break;
                }
            }
        });
    }

    /// Reads one pick and plays it, if it's still the newest.
    fn read(&self, pick: Pick) {
        if self.picks.load(Ordering::Acquire) != pick.number {
            return;
        }
        let name = pick.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let read = mo_audio::read(&pick.path, MOST_SECONDS);
        if self.picks.load(Ordering::Acquire) != pick.number {
            return;
        }
        let rate = self.rate.load(Ordering::Acquire);
        let played = match read {
            Err(e) => Played::Not(format!("Couldn't read {name}: {e}")),
            Ok(_) if rate == 0 => Played::Not(NOT_STARTED.into()),
            Ok(audio) => {
                let voice = Box::new(Voice::new(&audio.channels, audio.sample_rate, rate));
                self.hand_over(voice, name)
            }
        };
        (pick.told)(played);
    }

    fn hand_over(&self, voice: Box<Voice>, name: String) -> Played {
        match self.ask(Ask::Play(voice)) {
            Ok(()) => Played::Playing(name, self.handed.fetch_add(1, Ordering::AcqRel) + 1),
            Err(why) => Played::Not(why),
        }
    }

    /// Stops what's playing, and any sound still to be read.
    pub fn stop(&self) {
        self.picks.fetch_add(1, Ordering::AcqRel);
        lock(&self.next).take();
        let _ = self.ask(Ask::Stop);
    }

    /// Whether the `n`th sound handed to the audio thread ([`Played::Playing`]) may still be playing: it hasn't
    /// started yet, or it's the newest started and hasn't ended.
    pub fn still_playing(&self, n: u64) -> bool {
        let started = self.counts.started.load(Ordering::Acquire);
        started < n || (started == n && self.counts.ended.load(Ordering::Acquire) < started)
    }

    fn ask(&self, ask: Ask) -> Result<(), String> {
        match lock(&self.asks).as_mut().map(|asks| asks.push(ask)) {
            Some(Ok(())) => Ok(()),
            Some(Err(_)) => Err("Busy: try again".into()),
            None => Err(NOT_STARTED.into()),
        }
    }

    /// Frees the sounds the audio thread is done with. Called often from the panel's thread.
    pub fn tidy(&self) {
        if let Some(done) = lock(&self.done).as_mut() {
            while done.pop().is_ok() {}
        }
    }
}

fn take_next(player: &Player) -> Option<Pick> {
    lock(&player.next).take()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice(len: usize) -> Box<Voice> {
        Box::new(Voice::new(&[vec![1.0; len]], 48_000, 48_000))
    }

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
        let player = Player::default();
        let mut playing = player.start(48_000);
        let Played::Playing(_, n) = player.hand_over(voice(3), "a".into()) else { panic!("not handed over") };
        assert!(player.still_playing(n), "handed over, not started yet");
        let (mut l, mut r) = (vec![9.0; 4], vec![9.0; 4]);
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![LEVEL, LEVEL, LEVEL, 0.0]);
        assert_eq!(r, l);
        // Done: it went back to be freed off the audio thread, and the panel's line can go.
        assert!(playing.now.is_none());
        assert!(!player.still_playing(n));
        assert_eq!(lock(&player.done).as_ref().unwrap().slots(), 1);
        player.tidy();
        assert_eq!(lock(&player.done).as_ref().unwrap().slots(), 0);
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![0.0; 4]);
    }

    #[test]
    fn a_newer_sound_replaces_the_one_playing() {
        let player = Player::default();
        let mut playing = player.start(48_000);
        let Played::Playing(_, first) = player.hand_over(voice(100), "a".into()) else { panic!() };
        let (mut l, mut r) = (vec![0.0; 4], vec![0.0; 4]);
        playing.fill(&mut l, &mut r);
        let Played::Playing(_, second) = player.hand_over(voice(100), "b".into()) else { panic!() };
        playing.fill(&mut l, &mut r);
        assert!(!player.still_playing(first));
        assert!(player.still_playing(second));
    }

    #[test]
    fn stop_hands_the_sound_back() {
        let player = Player::default();
        let mut playing = player.start(48_000);
        let Played::Playing(_, n) = player.hand_over(voice(100), "a".into()) else { panic!() };
        let (mut l, mut r) = (vec![0.0; 4], vec![0.0; 4]);
        playing.fill(&mut l, &mut r);
        player.stop();
        playing.fill(&mut l, &mut r);
        assert_eq!(l, vec![0.0; 4]);
        assert!(playing.now.is_none());
        assert!(!player.still_playing(n));
    }

    #[test]
    fn nothing_plays_before_the_daw_starts_the_audio() {
        let player = Player::default();
        assert_eq!(player.hand_over(voice(1), "a".into()), Played::Not(NOT_STARTED.into()));
    }

    #[test]
    fn a_stopped_track_plays_nothing_it_was_handed() {
        let player = Player::default();
        let _playing = player.start(48_000);
        let Played::Playing(_, n) = player.hand_over(voice(100), "a".into()) else { panic!() };
        player.stop_all();
        assert!(!player.still_playing(n));
    }
}
