//! The player with a stand-in for the computer's outputs: no test opens a real one (rule 11).

use std::sync::mpsc::{Receiver, channel};

use super::*;

#[derive(Default)]
struct Fake {
    /// The outputs there are, the default first.
    names: Vec<String>,
    /// When set, nothing opens.
    broken: bool,
    /// What each open was asked for (`None`: the default).
    asked: Vec<Option<String>>,
    render: Option<Render>,
    lost: Option<Lost>,
    rate: u32,
}

#[derive(Clone)]
struct FakeOutputs(Arc<Mutex<Fake>>);

impl Outputs for FakeOutputs {
    fn names(&mut self) -> Vec<String> {
        lock(&self.0).names.clone()
    }

    fn default_name(&mut self) -> Option<String> {
        lock(&self.0).names.first().cloned()
    }

    fn open(&mut self, name: Option<&str>, render: Render, lost: Lost) -> Result<Opened, String> {
        let mut f = lock(&self.0);
        f.asked.push(name.map(str::to_owned));
        if f.broken {
            return Err("The output is busy".into());
        }
        let name = match name {
            Some(n) if f.names.iter().any(|x| x == n) => n.to_owned(),
            Some(n) => return Err(format!("{n} isn't connected")),
            None => f.names.first().cloned().ok_or("There's no sound output")?,
        };
        f.render = Some(render);
        f.lost = Some(lost);
        Ok(Opened {
            name,
            sample_rate: f.rate,
            channels: 2,
            delay: Arc::new(AtomicU64::new(12_000)),
            stream: Box::new(()),
        })
    }
}

impl FakeOutputs {
    fn new(names: &[&str]) -> FakeOutputs {
        FakeOutputs(Arc::new(Mutex::new(Fake {
            names: names.iter().map(|n| (*n).to_owned()).collect(),
            rate: 1000,
            ..Fake::default()
        })))
    }

    /// Plays `frames` of what's open (stereo), as the output's thread would, and gives the left side.
    fn pull(&self, frames: usize) -> Vec<f32> {
        let mut f = lock(&self.0);
        let rate = f.rate;
        let mut out = vec![0.0; frames * 2];
        if let Some(render) = f.render.as_mut() {
            render(&mut out, rate, 2);
        }
        out.iter().step_by(2).copied().collect()
    }

    fn lose(&self) {
        let f = lock(&self.0);
        if let Some(lost) = &f.lost {
            lost("The device was unplugged".into());
        }
    }

    fn asked(&self) -> Vec<Option<String>> {
        lock(&self.0).asked.clone()
    }
}

/// Waits until the player's thread has done everything asked of it so far.
fn settle(player: &Player) {
    player.outputs();
}

/// A sound whose samples say where they are: sample `i` is `i / 10_000`, at 1000 frames a second (10 s long).
fn ramp() -> Arc<AudioData> {
    Arc::new(AudioData {
        sample_rate: 1000,
        channels: vec![(0..10_000).map(|i| i as f32 / 10_000.0).collect()],
        ..AudioData::default()
    })
}

fn player(outputs: &FakeOutputs) -> (Player, Receiver<String>) {
    let (ended, rx) = channel();
    let player = Player::with_outputs(Box::new(outputs.clone()), move |key| {
        let _ = ended.send(key.to_owned());
    });
    (player, rx)
}

#[test]
fn nothing_opens_until_a_sound_plays_and_then_it_plays_to_the_end() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, ended) = player(&outputs);
    player.choose(None);
    settle(&player);
    assert!(outputs.asked().is_empty());
    assert_eq!(player.info(), OutputInfo::default());

    let short = Arc::new(AudioData { sample_rate: 1000, channels: vec![vec![0.5; 300]], ..AudioData::default() });
    player.play(short, "kick.wav", 0.0);
    assert_eq!(player.playing().as_deref(), Some("kick.wav"));
    assert_eq!(player.position(), Some(0.0));
    settle(&player);
    assert_eq!(outputs.asked(), vec![None]);
    let info = player.info();
    assert_eq!(info.name.as_deref(), Some("Speakers"));
    assert_eq!(info.delay_ms, Some(12.0));
    assert_eq!(outputs.pull(100), vec![0.5; 100]);
    assert_eq!(player.position(), Some(0.1));
    outputs.pull(300);
    settle(&player);
    assert_eq!(ended.try_recv().as_deref(), Ok("kick.wav"));
    assert_eq!(player.playing(), None);
    assert_eq!(player.position(), None);
}

#[test]
fn stopped_and_replaced_sounds_dont_count_as_ended() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, ended) = player(&outputs);
    player.play(ramp(), "a", 0.0);
    player.play(ramp(), "b", 0.0);
    settle(&player);
    outputs.pull(100);
    player.stop();
    settle(&player);
    outputs.pull(100);
    assert_eq!(player.playing(), None);
    settle(&player);
    assert!(ended.try_recv().is_err());
}

#[test]
fn seek_plays_on_from_there() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, _ended) = player(&outputs);
    player.play(ramp(), "a", 2.0);
    settle(&player);
    let heard = outputs.pull(200);
    // It fades in over 3 ms (3 frames here) from part way, then plays the ramp from 2 s on.
    assert!((heard[100] - 0.21).abs() < 1e-4, "{}", heard[100]);
    player.seek(7.0);
    assert_eq!(player.position(), Some(7.0));
    settle(&player);
    let heard = outputs.pull(100);
    assert!((heard[50] - 0.705).abs() < 1e-4, "{}", heard[50]);
    assert_eq!(player.position(), Some(7.1));
}

#[test]
fn a_lost_output_is_opened_again_and_the_sound_carries_on() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, _ended) = player(&outputs);
    player.play(ramp(), "a", 0.0);
    settle(&player);
    outputs.pull(1000);
    outputs.lose();
    settle(&player);
    assert_eq!(outputs.asked(), vec![None, None]);
    let heard = outputs.pull(100);
    assert!((heard[50] - 0.105).abs() < 1e-4, "{}", heard[50]);
}

#[test]
fn an_output_that_keeps_going_is_given_up_on() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, _ended) = player(&outputs);
    player.play(ramp(), "a", 0.0);
    settle(&player);
    for _ in 0..5 {
        outputs.lose();
        settle(&player);
    }
    assert_eq!(outputs.asked().len(), 1 + MAX_REOPENS as usize);
    assert_eq!(player.info().problem.as_deref(), Some("The device was unplugged"));
    // The next sound tries again.
    player.play(ramp(), "b", 0.0);
    settle(&player);
    assert_eq!(outputs.asked().len(), 2 + MAX_REOPENS as usize);
}

#[test]
fn a_chosen_output_thats_away_plays_on_the_default_until_it_comes_back() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, _ended) = player(&outputs);
    player.choose(Some("Focusrite".into()));
    player.play(ramp(), "a", 0.0);
    settle(&player);
    let info = player.info();
    assert_eq!(info.name.as_deref(), Some("Speakers"));
    assert_eq!(info.problem.as_deref(), Some("Focusrite isn't connected, so sound goes to the default output"));
    outputs.pull(500);

    lock(&outputs.0).names.push("Focusrite".into());
    player.check();
    settle(&player);
    let info = player.info();
    assert_eq!(info.name.as_deref(), Some("Focusrite"));
    assert_eq!(info.problem, None);
    // It carried on from where it was.
    let heard = outputs.pull(10);
    assert!(heard[5] > 0.05, "{}", heard[5]);
    assert_eq!(outputs.asked(), vec![Some("Focusrite".into()), None, Some("Focusrite".into())]);
}

#[test]
fn choosing_another_output_moves_the_sound_playing() {
    let outputs = FakeOutputs::new(&["Speakers", "Headphones"]);
    let (player, _ended) = player(&outputs);
    player.play(ramp(), "a", 0.0);
    settle(&player);
    outputs.pull(100);
    player.choose(Some("Headphones".into()));
    settle(&player);
    assert_eq!(player.info().name.as_deref(), Some("Headphones"));
    assert_eq!(player.outputs(), vec!["Speakers".to_owned(), "Headphones".to_owned()]);
    let heard = outputs.pull(10);
    assert!(heard[5] > 0.009, "{}", heard[5]);
}

#[test]
fn a_busy_output_is_tried_again_a_moment_later_not_at_every_sound() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, _ended) = player(&outputs);
    lock(&outputs.0).broken = true;
    player.play(ramp(), "a", 0.0);
    settle(&player);
    assert_eq!(player.info().problem.as_deref(), Some("The output is busy"));
    lock(&outputs.0).broken = false;
    player.play(ramp(), "b", 0.0);
    settle(&player);
    assert_eq!(outputs.asked().len(), 1);
    // A couple of seconds later the sound waiting gets its output.
    let deadline = Instant::now() + Duration::from_secs(5);
    while player.info().name.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(player.info().name.as_deref(), Some("Speakers"));
    assert_eq!(outputs.asked().len(), 2);
    assert_eq!(player.playing().as_deref(), Some("b"));
}

#[test]
fn a_move_just_as_the_sound_ends_still_ends_it() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, ended) = player(&outputs);
    let short = Arc::new(AudioData { sample_rate: 1000, channels: vec![vec![0.5; 50]], ..AudioData::default() });
    player.play(short, "a", 0.0);
    settle(&player);
    outputs.pull(100);
    player.seek(0.01);
    settle(&player);
    outputs.pull(10);
    settle(&player);
    assert_eq!(ended.try_recv().as_deref(), Ok("a"));
    assert_eq!(player.playing(), None);
}
