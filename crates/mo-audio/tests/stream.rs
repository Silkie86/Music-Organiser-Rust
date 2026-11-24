//! A sound read as it plays (Solomon's F12 note of 24 Nov 2026: a 5:11 song stopped a minute in, "same as alot of
//! other tracks"): it plays to the end of its file however long it is, moves past its first minute, and carries on from
//! where it was when its output is opened again. A stand-in plays the output: no test opens a real one (rule 11).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use mo_audio::play::{Event, Events, Opened, Outputs, Player, Render, Stream};

/// The stand-in output's rate, and the test sound's: a frame a millisecond.
const RATE: u32 = 1000;

/// How long the test sound is: past the first minute, which was all a sound's read held before.
const FRAMES: usize = 75 * RATE as usize;

#[derive(Default)]
struct Inner {
    render: Option<Render>,
    events: Option<Events>,
    opened: usize,
}

/// One output, "Speakers", mono at [`RATE`]; the test plays it by hand.
#[derive(Clone, Default)]
struct Fake(Arc<Mutex<Inner>>);

impl Outputs for Fake {
    fn names(&mut self) -> Vec<String> {
        vec!["Speakers".into()]
    }

    fn default_name(&mut self) -> Option<String> {
        Some("Speakers".into())
    }

    fn open(&mut self, _name: Option<&str>, render: Render, events: Events) -> Result<Opened, String> {
        let mut f = self.inner();
        f.render = Some(render);
        f.events = Some(events);
        f.opened += 1;
        Ok(Opened {
            name: "Speakers".into(),
            sample_rate: RATE,
            channels: 1,
            delay: Arc::default(),
            period: Arc::default(),
            stream: Box::new(()),
        })
    }
}

impl Fake {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Plays `frames` of what's open, as the output's thread would.
    fn pull(&self, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames];
        if let Some(render) = self.inner().render.as_mut() {
            render(&mut out, RATE, 1);
        }
        out
    }

    /// Plays on, a tenth of a second at a time with a moment between for the reader, until `done` says so (given what
    /// was just heard): a minute at the most, for a slow computer.
    fn play_until(&self, mut done: impl FnMut(&[f32]) -> bool) {
        let give_up = Instant::now() + Duration::from_secs(60);
        while Instant::now() < give_up {
            if done(&self.pull(RATE as usize / 10)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("it never got there");
    }

    /// Plays on until it's heard sound to the end of what it played, so from where it was moved to, not only the sound
    /// before fading out (after a move or a reopen, the read starts afresh), then gives the reader a moment to get ahead.
    fn play_until_heard(&self) {
        self.play_until(|heard| heard.last().is_some_and(|s| *s != 0.0));
        std::thread::sleep(Duration::from_millis(50));
    }

    fn lose(&self) {
        if let Some(events) = &self.inner().events {
            events(Event::Lost("The device was unplugged".into()));
        }
    }
}

/// A WAV file [`FRAMES`] long whose every sample says where it is: frame `i` is `i / FRAMES`.
fn long_file(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-audio-stream");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let ramp: Vec<f32> = (0..FRAMES).map(|i| i as f32 / FRAMES as f32).collect();
    mo_audio::write_wav(&path, &[ramp], RATE, 24).unwrap();
    path
}

/// The sample at `seconds` into [`long_file`].
fn at(seconds: f64) -> f32 {
    (seconds * f64::from(RATE)) as f32 / FRAMES as f32
}

fn player() -> (Player, Fake, Receiver<String>) {
    let fake = Fake::default();
    let (ended, rx) = channel();
    let player = Player::with_outputs(Box::new(fake.clone()), move |key| {
        let _ = ended.send(key.to_owned());
    });
    (player, fake, rx)
}

fn play(player: &Player, path: &Path, from: f64) {
    let stream = Stream::open(path, from, None, &|| true).unwrap();
    player.play_stream_if(stream, "long", || Ok::<(), ()>(())).unwrap();
    // Waits for the player's thread to have started it.
    player.outputs();
}

/// The sample heard `frames` after where the player says it is, checked against where that is in the file.
fn heard_from_where_it_is(player: &Player, fake: &Fake) -> (f64, f32) {
    let place = player.position().unwrap();
    let heard = fake.pull(100);
    (place, heard[50] - at(place) - 50.0 / FRAMES as f32)
}

#[test]
fn a_sound_longer_than_a_minute_plays_to_its_end() {
    let path = long_file("to-the-end.wav");
    let (player, fake, ended) = player();
    play(&player, &path, 0.0);
    let (mut loudest, mut furthest) = (0.0f32, 0.0f64);
    fake.play_until(|heard| {
        loudest = heard.iter().copied().fold(loudest, f32::max);
        furthest = furthest.max(player.position().unwrap_or(0.0));
        ended.try_recv().is_ok()
    });
    // It got past the first minute, to the file's last sample, and ended there by itself.
    assert!(furthest > 70.0, "{furthest}");
    assert!((loudest - at(75.0 - 0.001)).abs() < 1e-5, "{loudest}");
    assert_eq!(player.playing(), None);
}

#[test]
fn a_move_past_the_first_minute_plays_from_there() {
    let path = long_file("move.wav");
    let (player, fake, _ended) = player();
    play(&player, &path, 0.0);
    fake.pull(100);
    player.seek(65.0);
    assert_eq!(player.position(), Some(65.0));
    player.outputs();
    fake.play_until_heard();
    let (place, off) = heard_from_where_it_is(&player, &fake);
    assert!((65.0..66.0).contains(&place), "{place}");
    assert!(off.abs() < 1e-5, "{off}");
}

#[test]
fn a_sound_started_part_way_says_where_it_is_in_its_file() {
    let path = long_file("part-way.wav");
    let (player, fake, _ended) = player();
    play(&player, &path, 61.5);
    assert_eq!(player.position(), Some(61.5));
    fake.play_until_heard();
    let (place, off) = heard_from_where_it_is(&player, &fake);
    assert!((61.5..62.5).contains(&place), "{place}");
    assert!(off.abs() < 1e-5, "{off}");
}

#[test]
fn a_lost_output_is_opened_again_and_the_sound_read_again_from_where_it_was() {
    let path = long_file("reopen.wav");
    let (player, fake, _ended) = player();
    play(&player, &path, 0.0);
    fake.play_until(|_| player.position().is_some_and(|at| at >= 2.0));
    let before = player.position().unwrap();
    fake.lose();
    player.outputs();
    assert_eq!(fake.inner().opened, 2);
    fake.play_until_heard();
    let (place, off) = heard_from_where_it_is(&player, &fake);
    assert!(place >= before && place < before + 1.0, "{before} then {place}");
    assert!(off.abs() < 1e-5, "{off}");
}

#[test]
fn trim_silence_starts_a_stream_past_the_silence_at_its_start() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-audio-stream");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("quiet-start.wav");
    // Three seconds of silence (longer than a part), then sound.
    let samples: Vec<f32> = (0..10_000).map(|i| if i < 3_000 { 0.0 } else { 0.5 }).collect();
    mo_audio::write_wav(&path, &[samples], RATE, 16).unwrap();
    let level = mo_audio::trim::level_of_db(-60);
    let stream = Stream::open(&path, 0.0, Some(level), &|| true).unwrap();
    assert!((stream.start_seconds() - 3.0).abs() < 1e-9, "{}", stream.start_seconds());
    assert_eq!(stream.sample_rate(), RATE);
    // From part way in, it starts where it was asked to, as a click on the waveform says.
    let stream = Stream::open(&path, 1.0, Some(level), &|| true).unwrap();
    assert!((stream.start_seconds() - 1.0).abs() < 1e-9, "{}", stream.start_seconds());
}

#[test]
fn a_file_that_cant_be_read_says_so_as_it_opens() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-audio-stream");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(Stream::open(&dir.join("not-there.wav"), 0.0, None, &|| true).is_err());
    // Past its end: nothing to play, so it ends as soon as it starts.
    let path = long_file("past-the-end.wav");
    let stream = Stream::open(&path, 80.0, None, &|| true).unwrap();
    let (player, fake, ended) = player();
    player.play_stream_if(stream, "past", || Ok::<(), ()>(())).unwrap();
    player.outputs();
    fake.pull(10);
    assert_eq!(ended.recv_timeout(Duration::from_secs(5)).as_deref(), Ok("past"));
}

#[test]
fn a_stream_no_longer_wanted_stops_waiting_for_its_first_part() {
    // Another sound was asked for while it was on its way there (a place far into a long compressed file is only
    // reached by decoding all before it): it stops waiting, with nothing to play, so it's never started.
    let path = long_file("unwanted.wav");
    let stream = Stream::open(&path, 30.0, None, &|| false).unwrap();
    assert_eq!(stream.sample_rate(), 0);
    let (player, _fake, _ended) = player();
    assert_eq!(player.play_stream_if(stream, "unwanted", || Err("overtaken")), Err("overtaken"));
    assert_eq!(player.playing(), None);
}
