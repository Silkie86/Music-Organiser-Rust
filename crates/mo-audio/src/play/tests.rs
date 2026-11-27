//! The player with a stand-in for the computer's outputs: no test opens a real one (rule 11).

use std::sync::mpsc::{Receiver, channel};

use super::*;

#[derive(Default)]
struct Fake {
    /// The outputs there are, the default first.
    names: Vec<String>,
    /// When set, nothing opens.
    broken: bool,
    /// Outputs that are listed but won't open (a DAW has them).
    taken: Vec<String>,
    /// What each open was asked for (`None`: the default).
    asked: Vec<Option<String>>,
    render: Option<Render>,
    events: Option<Events>,
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

    fn open(&mut self, name: Option<&str>, render: Render, events: Events) -> Result<Opened, String> {
        let mut f = lock(&self.0);
        f.asked.push(name.map(str::to_owned));
        if f.broken {
            return Err("The output is busy".into());
        }
        if let Some(n) = name.filter(|n| f.taken.iter().any(|t| t == n)) {
            return Err(format!("{n} is in use by another program"));
        }
        let name = match name {
            Some(n) if f.names.iter().any(|x| x == n) => n.to_owned(),
            Some(n) => return Err(format!("{n} isn't connected")),
            None => f.names.first().cloned().ok_or("There's no sound output")?,
        };
        f.render = Some(render);
        f.events = Some(events);
        Ok(Opened {
            name,
            sample_rate: f.rate,
            channels: 2,
            delay: Arc::new(AtomicU64::new(12_000)),
            period: Arc::new(AtomicU64::new(10_000)),
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
        if let Some(events) = &f.events {
            events(Event::Lost("The device was unplugged".into()));
        }
    }

    fn moved(&self, to: &str) {
        let mut f = lock(&self.0);
        f.names.retain(|n| n != to);
        f.names.insert(0, to.to_owned());
        if let Some(events) = &f.events {
            events(Event::Moved);
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
    let player = Player::with_outputs(Box::new(outputs.clone()), move |key, _| {
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
    assert_eq!(info.mix_every_ms, Some(10.0));
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
fn a_sound_no_longer_wanted_as_it_starts_leaves_the_one_playing_alone() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, _ended) = player(&outputs);
    assert_eq!(player.play_if(ramp(), "1", 2.0, || Ok::<(), &str>(())), Ok(()));
    // A newer sound or a stop was asked for while this one was read: it isn't started, and the first plays on.
    assert_eq!(player.play_if(ramp(), "2", 0.0, || Err("overtaken")), Err("overtaken"));
    assert_eq!(player.playing().as_deref(), Some("1"));
    settle(&player);
    let heard = outputs.pull(200);
    assert!((heard[100] - 0.21).abs() < 1e-4, "{}", heard[100]);
}

#[test]
fn a_loop_is_swapped_for_another_where_it_is_and_only_while_its_the_one_playing() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, _ended) = player(&outputs);
    // Nothing playing: nothing is swapped in.
    assert!(!player.swap_looped(ramp(), "a", Some));
    assert_eq!(player.playing(), None);
    player.play_looped(ramp(), "a", 2.0);
    settle(&player);
    outputs.pull(100);
    // Another key: left alone.
    assert!(!player.swap_looped(ramp(), "b", Some));
    // Its own, but the caller says no: left alone.
    assert!(!player.swap_looped(ramp(), "a", |_| None));
    // Its own: the new sound goes on from where `from` says, given where the loop was (2.1 s in).
    assert!(player.swap_looped(ramp(), "a", |at| Some(at + 5.0)));
    assert_eq!(player.playing().as_deref(), Some("a"));
    settle(&player);
    let heard = outputs.pull(100);
    assert!((heard[50] - 0.715).abs() < 1e-3, "{}", heard[50]);
    // Stopped: a tone made meanwhile doesn't start it again.
    player.stop();
    assert!(!player.swap_looped(ramp(), "a", Some));
    assert_eq!(player.playing(), None);
}

#[test]
fn a_loop_made_again_goes_on_from_where_the_old_one_is_as_it_goes_in() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, _ended) = player(&outputs);
    // Nothing playing: `from` is told so.
    assert!(player.play_looped_from(ramp(), "a", |playing| if playing.is_none() { Ok(2.0) } else { Err(()) }).is_ok());
    settle(&player);
    outputs.pull(100);
    // The new one is made while the old one plays on (another 0.1 s): it goes on from where the old one is then.
    outputs.pull(100);
    let mut seen = None;
    assert!(
        player
            .play_looped_from(ramp(), "b", |playing| {
                seen = playing.map(|(key, at)| (key.to_owned(), at));
                Ok::<_, ()>(playing.map_or(0.0, |(_, at)| at + 5.0))
            })
            .is_ok()
    );
    let (key, at) = seen.unwrap();
    assert_eq!(key, "a");
    assert!((at - 2.2).abs() < 1e-3, "{at}");
    assert_eq!(player.playing().as_deref(), Some("b"));
    settle(&player);
    let heard = outputs.pull(100);
    assert!((heard[50] - 0.725).abs() < 1e-3, "{}", heard[50]);
    // Said no: nothing changes.
    assert_eq!(player.play_looped_from(ramp(), "c", |_| Err(())), Err(()));
    assert_eq!(player.playing().as_deref(), Some("b"));
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
    let (player, ended) = player(&outputs);
    player.play(ramp(), "a", 0.0);
    settle(&player);
    for _ in 0..5 {
        outputs.lose();
        settle(&player);
    }
    assert_eq!(outputs.asked().len(), 1 + MAX_REOPENS as usize);
    assert_eq!(player.info().problem.as_deref(), Some("The device was unplugged"));
    // The sound stops, and the app is told, rather than showing as playing for ever.
    assert_eq!(ended.try_recv().as_deref(), Ok("a"));
    assert_eq!(player.playing(), None);
    // The next sound tries again.
    player.play(ramp(), "b", 0.0);
    settle(&player);
    assert_eq!(outputs.asked().len(), 2 + MAX_REOPENS as usize);
}

/// The whole-app review of 26 Nov, AD-3: every loss of the output counted towards giving up, however long the sound had
/// played in between, so in one long play (a whole song or mix) the fourth change of the default output on Windows
/// ended it, though the new output had opened fine. Only losses close together count now.
#[test]
fn an_output_lost_now_and_then_after_playing_a_while_is_opened_again_each_time() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, ended) = player(&outputs);
    player.play_looped(ramp(), "a", 0.0);
    settle(&player);
    for _ in 0..2 * MAX_REOPENS {
        // Ten seconds of sound (at 1000 frames a second) on each output before it goes.
        outputs.pull(STEADY_SECONDS as usize * 1000);
        outputs.lose();
        settle(&player);
    }
    assert_eq!(outputs.asked().len(), 1 + 2 * MAX_REOPENS as usize);
    assert_eq!(player.playing().as_deref(), Some("a"));
    assert!(ended.try_recv().is_err());
    let heard = outputs.pull(100);
    assert!(heard[50] > 0.0, "it plays on: {}", heard[50]);
    // Lost again before it has played a while, as many times as allowed: then it's given up on.
    outputs.pull(100);
    for _ in 0..MAX_REOPENS + 1 {
        outputs.lose();
        settle(&player);
    }
    assert_eq!(ended.try_recv().as_deref(), Ok("a"));
    assert_eq!(player.playing(), None);
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

#[test]
fn an_end_is_noticed_with_no_order_to_wake_the_player() {
    // The output's thread hands the end back without waking the player's thread (MP-3): it looks by itself.
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, ended) = player(&outputs);
    let short = Arc::new(AudioData { sample_rate: 1000, channels: vec![vec![0.5; 50]], ..AudioData::default() });
    player.play(short, "a", 0.0);
    settle(&player);
    outputs.pull(100);
    assert_eq!(ended.recv_timeout(Duration::from_secs(5)).as_deref(), Ok("a"));
    assert_eq!(player.playing(), None);
}

#[test]
fn a_chosen_output_thats_listed_but_taken_doesnt_cut_the_sound_on_the_default() {
    let outputs = FakeOutputs::new(&["Speakers", "Focusrite"]);
    lock(&outputs.0).taken.push("Focusrite".into());
    let (player, _ended) = player(&outputs);
    player.choose(Some("Focusrite".into()));
    player.play(ramp(), "a", 0.0);
    settle(&player);
    assert_eq!(player.info().name.as_deref(), Some("Speakers"));
    outputs.pull(500);
    player.check();
    settle(&player);
    // It tried the chosen one without closing the default first, so the sound carries on uncut.
    assert_eq!(outputs.asked(), vec![Some("Focusrite".into()), None, Some("Focusrite".into())]);
    let info = player.info();
    assert_eq!(info.name.as_deref(), Some("Speakers"));
    assert_eq!(info.problem.as_deref(), Some("Focusrite is in use by another program, so sound stays on Speakers"));
}

#[test]
fn when_nothing_opens_the_problem_says_so() {
    let outputs = FakeOutputs::new(&[]);
    let (player, _ended) = player(&outputs);
    player.choose(Some("Focusrite".into()));
    player.play(ramp(), "a", 0.0);
    settle(&player);
    let info = player.info();
    assert_eq!(info.name, None);
    assert_eq!(
        info.problem.as_deref(),
        Some("Focusrite isn't connected, and the default output wouldn't open either: There's no sound output")
    );
}

#[test]
fn check_and_choosing_the_same_output_open_and_cut_nothing() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, _ended) = player(&outputs);
    player.check();
    settle(&player);
    assert!(outputs.asked().is_empty());
    player.play(ramp(), "a", 0.0);
    player.choose(None);
    player.check();
    settle(&player);
    assert_eq!(outputs.asked(), vec![None]);
}

#[test]
fn when_the_system_moves_the_sound_to_a_new_default_the_name_follows() {
    let outputs = FakeOutputs::new(&["Speakers", "Headphones"]);
    let (player, _ended) = player(&outputs);
    player.play(ramp(), "a", 0.0);
    settle(&player);
    outputs.moved("Headphones");
    settle(&player);
    assert_eq!(player.info().name.as_deref(), Some("Headphones"));
    assert_eq!(outputs.asked(), vec![None]);
}

#[test]
fn dropping_the_player_when_a_sound_ends_doesnt_hang() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let slot: Arc<Mutex<Option<Player>>> = Arc::default();
    let in_end = Arc::clone(&slot);
    let (done, rx) = channel();
    let player = Player::with_outputs(Box::new(outputs.clone()), move |_, _| {
        drop(lock(&in_end).take());
        let _ = done.send(());
    });
    let short = Arc::new(AudioData { sample_rate: 1000, channels: vec![vec![0.5; 10]], ..AudioData::default() });
    player.play(short, "a", 0.0);
    settle(&player);
    *lock(&slot) = Some(player);
    outputs.pull(20);
    assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
}

#[test]
fn how_often_the_output_mixes_comes_from_its_buffers_length() {
    use super::output::period_us;
    // The 10 ms buffer the player asks for, stereo at 48 kHz and at 44.1 kHz.
    assert_eq!(period_us(480 * 2, 2, 48_000), Some(10_000));
    assert_eq!(period_us(441 * 2, 2, 44_100), Some(10_000));
    // An output that chose its own: 1024 frames of 8 channels at 48 kHz.
    assert_eq!(period_us(1024 * 8, 8, 48_000), Some(21_333));
    assert_eq!(period_us(512, 0, 48_000), Some(10_666));
    assert_eq!(period_us(480, 1, 0), None);
}

/// A sound read as it plays, made in memory: `samples` (mono, 1000 frames a second) in parts of `part` frames, starting
/// `start` frames into its file.
fn stream_of(samples: &[f32], part: usize, start: u64) -> Stream {
    let parts = samples
        .chunks(part)
        .map(|p| Arc::new(AudioData { sample_rate: 1000, channels: vec![p.to_vec()], ..AudioData::default() }))
        .collect();
    Stream::of_parts(parts, start)
}

#[test]
fn a_sound_read_as_it_plays_plays_every_part_and_says_where_it_is_in_its_file() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, ended) = player(&outputs);
    let samples: Vec<f32> = (0..300).map(|i| 0.5 + i as f32 / 1000.0).collect();
    // From 5 s into its file.
    assert_eq!(player.play_stream_if(stream_of(&samples, 100, 5_000), "long", || Ok::<(), ()>(())), Ok(()));
    assert_eq!(player.playing().as_deref(), Some("long"));
    assert_eq!(player.position(), Some(5.0));
    settle(&player);
    let heard = outputs.pull(150);
    // It fades in over 3 ms (3 frames here), as it starts part way into its file, then plays sample for sample, on
    // through the joins between its parts.
    assert!(heard[0] > 0.0 && heard[0] < samples[0], "{}", heard[0]);
    assert_eq!(&heard[3..], &samples[3..150]);
    assert_eq!(player.position(), Some(5.15));
    let heard = outputs.pull(200);
    assert_eq!(&heard[..150], &samples[150..]);
    assert!(heard[150..].iter().all(|s| *s == 0.0));
    assert_eq!(ended.recv_timeout(Duration::from_secs(5)).as_deref(), Ok("long"));
    assert_eq!(player.playing(), None);
}

#[test]
fn a_sound_read_as_it_plays_isnt_started_when_no_longer_wanted() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, _ended) = player(&outputs);
    player.play(ramp(), "1", 2.0);
    let samples = vec![0.9; 100];
    assert_eq!(player.play_stream_if(stream_of(&samples, 50, 0), "2", || Err("overtaken")), Err("overtaken"));
    assert_eq!(player.playing().as_deref(), Some("1"));
    settle(&player);
    let heard = outputs.pull(200);
    assert!((heard[100] - 0.21).abs() < 1e-4, "{}", heard[100]);
}

#[test]
fn a_sound_read_as_it_plays_is_stopped_and_replaced_like_any_other() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, ended) = player(&outputs);
    let samples = vec![0.9; 1000];
    player.play_stream_if(stream_of(&samples, 100, 0), "a", || Ok::<(), ()>(())).unwrap();
    settle(&player);
    outputs.pull(100);
    player.play(ramp(), "b", 0.0);
    settle(&player);
    let heard = outputs.pull(100);
    // Faded out over 3 ms, then only the ramp.
    assert!((heard[50] - 0.005).abs() < 1e-4, "{}", heard[50]);
    player.stop();
    settle(&player);
    outputs.pull(100);
    settle(&player);
    assert!(ended.try_recv().is_err());
    assert_eq!(player.playing(), None);
}

#[test]
fn a_sound_read_as_it_plays_with_nothing_to_play_ends_at_once() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, ended) = player(&outputs);
    // Asked to play past its file's end: nothing came.
    player.play_stream_if(stream_of(&[], 100, 0), "past", || Ok::<(), ()>(())).unwrap();
    settle(&player);
    assert!(outputs.pull(10).iter().all(|s| *s == 0.0));
    assert_eq!(ended.recv_timeout(Duration::from_secs(5)).as_deref(), Ok("past"));
}

/// A sound made as it plays: the ramp ([`ramp`]) made from wherever it's asked to start, in parts of 100 frames, with
/// each place it was asked to start from kept.
struct Ramp {
    asked: Mutex<Vec<f64>>,
}

impl Maker for Ramp {
    fn rate(&self) -> u32 {
        1000
    }

    fn make(&self, seconds: f64, put: &mut dyn FnMut(Vec<Vec<f32>>) -> bool) -> Result<(), String> {
        if seconds < 0.0 {
            return Err("The synth wouldn't start".to_owned());
        }
        self.asked.lock().unwrap().push(seconds);
        let from = (seconds * 1000.0).round() as usize;
        for start in (from..10_000).step_by(100) {
            let part: Vec<f32> = (start..(start + 100).min(10_000)).map(|i| i as f32 / 10_000.0).collect();
            if !put(vec![part]) {
                return Ok(());
            }
        }
        Ok(())
    }
}

/// Pulls `frames` frames once the made parts have had time to come.
fn pull_made(outputs: &FakeOutputs, frames: usize) -> Vec<f32> {
    std::thread::sleep(Duration::from_millis(150));
    outputs.pull(frames)
}

#[test]
fn a_sound_made_as_it_plays_starts_where_asked_and_is_made_again_from_where_its_moved_to() {
    let outputs = FakeOutputs::new(&["Speakers"]);
    let (player, ended) = player(&outputs);
    let ramp = Arc::new(Ramp { asked: Mutex::new(Vec::new()) });
    let stream = Stream::made(ramp.clone(), 2.0, &|| true).unwrap();
    assert_eq!(stream.start_seconds(), 2.0);
    player.play_stream_if(stream, "song", || Ok::<(), ()>(())).unwrap();
    assert_eq!(player.position(), Some(2.0));
    settle(&player);
    let heard = pull_made(&outputs, 200);
    assert!((heard[100] - 0.21).abs() < 1e-4, "{}", heard[100]);

    player.seek(7.0);
    settle(&player);
    let heard = pull_made(&outputs, 100);
    assert!((heard[50] - 0.705).abs() < 1e-4, "{}", heard[50]);
    assert_eq!(*ramp.asked.lock().unwrap(), vec![2.0, 7.0]);

    // On to its end, where it ends like any other.
    for _ in 0..40 {
        outputs.pull(100);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(ended.recv_timeout(Duration::from_secs(5)).as_deref(), Ok("song"));
    assert_eq!(player.playing(), None);
}

#[test]
fn a_sound_made_past_its_end_has_nothing_to_play() {
    let ramp = Arc::new(Ramp { asked: Mutex::new(Vec::new()) });
    let stream = Stream::made(ramp.clone(), 12.0, &|| true).unwrap();
    assert_eq!(stream.sample_rate(), 0);
    // One that couldn't be made says why.
    let failed = Stream::made(ramp, -1.0, &|| true).err().map(|e| e.to_string());
    assert_eq!(failed.as_deref(), Some("The synth wouldn't start"));
}
