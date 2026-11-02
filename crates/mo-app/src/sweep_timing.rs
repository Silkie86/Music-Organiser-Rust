//! How long a sound swept in the galaxy takes to be heard, step by step, summed up in the session log every 10 seconds
//! of sweeping: the C# app's `SweepTiming`. It's the evidence for where a sound that trails the mouse loses its time,
//! so nothing is changed before it's measured.
//!
//! The steps, from the mouse reaching a star to hearing it:
//! 1. the mouse reaching the app (how late the window heard about the move; Slint doesn't say, so it's "not known"
//!    unless a caller can tell),
//! 2. waiting its turn (the sweep's gap between two sounds),
//! 3. the preview starting (the reading thread getting going),
//! 4. opening the file, split by whether its start was read ahead or not,
//! 5. the output's next mix (half its mixing period, on average),
//! 6. the output delay (the system's and the audio interface's).
//!
//! Every call only takes a short lock and never waits on the disk: the summary goes to the session log's own writer.
//! The galaxy calls [`reached`] and [`asked`] on the window's thread; the reading thread calls the rest.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::log;

/// How much sweeping is summed up in one log line.
pub const EVERY: Duration = Duration::from_secs(10);
/// A star the mouse reached this long before its turn came was passed by and asked for again later (a click on it,
/// say): its wait is counted from the ask, not from the old pass.
pub const STALE: Duration = Duration::from_secs(2);
/// Asked for this long ago and never started: dropped without a word, so it's forgotten.
pub const LOST: Duration = Duration::from_secs(30);
/// Above this many sounds waiting, the ones that never will be heard are forgotten.
const PENDING_MOST: usize = 64;
/// How many of the files read ahead lately are remembered, to tell a read-ahead open from a cold one.
const READ_AHEAD_KEPT: usize = 32;

/// One step's times, in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Step {
    pub total: f64,
    pub most: f64,
    pub count: u32,
}

impl Step {
    pub fn add(&mut self, ms: f64) {
        self.total += ms;
        self.most = self.most.max(ms);
        self.count += 1;
    }

    pub fn average(&self) -> f64 {
        if self.count == 0 { 0.0 } else { self.total / f64::from(self.count) }
    }
}

/// A sound on its way to being heard.
#[derive(Clone, Copy, Debug)]
struct Pending {
    /// When the mouse reached its star.
    reached: Instant,
    /// How late the window heard about that move, when it's known.
    input_ms: Option<f64>,
    /// When its turn came and it was asked for.
    asked: Option<Instant>,
    /// When the reading thread began opening it, and whether its start had been read ahead by then.
    opening: Option<(Instant, bool)>,
}

/// The sweep's timings. The app keeps one (see the free functions below); tests make their own.
#[derive(Debug)]
pub struct SweepTiming {
    pending: HashMap<String, Pending>,
    read_ahead: VecDeque<String>,
    input: Step,
    turn: Step,
    start: Step,
    open: Step,
    open_ahead: Step,
    open_cold: Step,
    sounds: u32,
    mix_ms: Option<f64>,
    output_ms: Option<f64>,
    since: Instant,
}

fn ms(from: Instant, to: Instant) -> f64 {
    to.saturating_duration_since(from).as_secs_f64() * 1000.0
}

impl SweepTiming {
    pub fn new(now: Instant) -> SweepTiming {
        SweepTiming {
            pending: HashMap::new(),
            read_ahead: VecDeque::new(),
            input: Step::default(),
            turn: Step::default(),
            start: Step::default(),
            open: Step::default(),
            open_ahead: Step::default(),
            open_cold: Step::default(),
            sounds: 0,
            mix_ms: None,
            output_ms: None,
            since: now,
        }
    }

    /// How many sounds are waiting to be heard.
    pub fn waiting(&self) -> usize {
        self.pending.len()
    }

    /// The mouse reached a star. `input_ms` is how long after the move happened the window got to it, when known.
    pub fn reached(&mut self, path: &str, input_ms: Option<f64>, now: Instant) {
        // A long sweep's passed stars that never played are dropped, never one asked for just now or being opened;
        // one asked for long ago that never started is gone too, so the list can't grow all session.
        if self.pending.len() > PENDING_MOST {
            self.pending.retain(|_, p| p.asked.is_some_and(|asked| now.saturating_duration_since(asked) <= LOST));
        }
        // Reached again before it was asked for: it waits from now, not from the first pass (seconds ago, perhaps).
        if self.pending.get(path).is_none_or(|p| p.asked.is_none()) {
            let input_ms = input_ms.map(|i| i.max(0.0));
            self.pending.insert(path.to_owned(), Pending { reached: now, input_ms, asked: None, opening: None });
        }
    }

    /// Its turn came: the preview was asked for.
    pub fn asked(&mut self, path: &str, now: Instant) {
        // Reached long ago and passed by (a click on it now, say): its wait starts now. Counted from the old pass,
        // one such star once read as a 42 s wait in the C# app and made a 10 s average 1.8 s.
        let fresh = self.pending.get(path).filter(|p| now.saturating_duration_since(p.reached) < STALE).copied();
        let p = match fresh {
            Some(p) => Pending { asked: Some(now), opening: None, ..p },
            None => Pending { reached: now, input_ms: None, asked: Some(now), opening: None },
        };
        self.pending.insert(path.to_owned(), p);
    }

    /// It won't be heard after all: a newer preview took its place before it started, or it couldn't be played.
    pub fn dropped(&mut self, path: &str) {
        self.pending.remove(path);
    }

    /// The start of this file was just read ahead, so the disk has it ready.
    pub fn read_ahead(&mut self, path: &str) {
        if let Some(i) = self.read_ahead.iter().position(|p| p == path) {
            self.read_ahead.remove(i);
        }
        self.read_ahead.push_back(path.to_owned());
        while self.read_ahead.len() > READ_AHEAD_KEPT {
            self.read_ahead.pop_front();
        }
    }

    /// The reading thread is opening it now.
    pub fn opening(&mut self, path: &str, now: Instant) {
        let ahead = self.read_ahead.iter().any(|p| p == path);
        if let Some(p) = self.pending.get_mut(path)
            && p.asked.is_some()
        {
            p.opening = Some((now, ahead));
        }
    }

    /// Read and handed to the output, which mixes it in from its next pass. Gives the summary line when one is due.
    pub fn started(&mut self, path: &str, now: Instant) -> Option<String> {
        let p = self.pending.remove(path)?;
        let (asked, (opening, ahead)) = (p.asked?, p.opening?);
        if let Some(i) = p.input_ms {
            self.input.add(i);
        }
        self.turn.add(ms(p.reached, asked));
        self.start.add(ms(asked, opening));
        let open = ms(opening, now);
        self.open.add(open);
        if ahead { &mut self.open_ahead } else { &mut self.open_cold }.add(open);
        self.sounds += 1;
        (now.saturating_duration_since(self.since) >= EVERY).then(|| self.note(now))
    }

    /// What the output adds after that: the wait for its next mix and how long a mixed sound takes to come out. `None`
    /// when not known.
    pub fn output(&mut self, mix_ms: Option<f64>, output_ms: Option<f64>) {
        (self.mix_ms, self.output_ms) = (mix_ms, output_ms);
    }

    /// The summary line, and the counts start again.
    pub fn note(&mut self, now: Instant) -> String {
        let (input, turn, start, open) = (self.input, self.turn, self.start, self.open);
        let (ahead, cold) = (self.open_ahead, self.open_cold);
        let mouse = if input.count == 0 {
            "mouse reaching the app not known (Slint doesn't say when the move happened)".to_owned()
        } else {
            format!("mouse reaching the app {:.0} ms (slowest {:.0})", input.average(), input.most)
        };
        let mix = self.mix_ms.map_or(String::new(), |m| format!("the output's next mix about {m:.0} ms, "));
        let out = match self.output_ms {
            Some(o) => format!(
                "then out of the speakers after {o:.0} ms (the output's buffer, the system and the interface). "
            ),
            None => "output delay not known. ".to_owned(),
        };
        let total = input.average()
            + turn.average()
            + start.average()
            + open.average()
            + self.mix_ms.unwrap_or(0.0).max(0.0)
            + self.output_ms.unwrap_or(0.0).max(0.0);
        let note = format!(
            "{} swept sounds in {:.0} s, from the mouse over a star to hearing it: {mouse}, waiting its turn {:.0} ms \
             (slowest {:.0}), preview starting {:.1} ms (slowest {:.0}), opening the file {:.1} ms (slowest {:.0}; {} \
             read ahead took {:.1} ms, {} not read ahead {:.1} ms), {mix}{out}About {total:.0} ms in all.",
            self.sounds,
            now.saturating_duration_since(self.since).as_secs_f64(),
            turn.average(),
            turn.most,
            start.average(),
            start.most,
            open.average(),
            open.most,
            ahead.count,
            ahead.average(),
            cold.count,
            cold.average(),
        );
        for s in [
            &mut self.input,
            &mut self.turn,
            &mut self.start,
            &mut self.open,
            &mut self.open_ahead,
            &mut self.open_cold,
        ] {
            *s = Step::default();
        }
        self.sounds = 0;
        self.since = now;
        note
    }
}

/// The app's sweep timings.
static TIMING: Mutex<Option<SweepTiming>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut SweepTiming, Instant) -> T) -> T {
    let now = Instant::now();
    let mut t = TIMING.lock().unwrap_or_else(PoisonError::into_inner);
    f(t.get_or_insert_with(|| SweepTiming::new(now)), now)
}

/// See [`SweepTiming::reached`].
pub fn reached(path: &str, input_ms: Option<f64>) {
    with(|t, now| t.reached(path, input_ms, now));
}

/// See [`SweepTiming::asked`].
pub fn asked(path: &str) {
    with(|t, now| t.asked(path, now));
}

/// See [`SweepTiming::dropped`].
pub fn dropped(path: &str) {
    with(|t, _| t.dropped(path));
}

/// See [`SweepTiming::read_ahead`].
pub fn read_ahead(path: &str) {
    with(|t, _| t.read_ahead(path));
}

/// See [`SweepTiming::opening`].
pub fn opening(path: &str) {
    with(|t, now| t.opening(path, now));
}

/// See [`SweepTiming::started`]; a summary that's due goes to the session log as a `[Sweep]` line.
pub fn started(path: &str) {
    // Written after the lock is let go: the log only queues the line, but nothing else needs to wait for it.
    if let Some(note) = with(|t, now| t.started(path, now)) {
        log::info("Sweep", &note);
    }
}

/// See [`SweepTiming::output`].
pub fn output(mix_ms: Option<f64>, output_ms: Option<f64>) {
    with(|t, _| t.output(mix_ms, output_ms));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn each_step_is_timed_from_the_mouse_to_the_output() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new(t0);
        t.output(Some(5.0), Some(20.0));
        t.reached("/a.wav", Some(4.0), at(t0, 0));
        t.asked("/a.wav", at(t0, 90));
        t.opening("/a.wav", at(t0, 92));
        assert_eq!(t.started("/a.wav", at(t0, 102)), None, "no line before 10 s");
        let note = t.note(at(t0, 10_000));
        assert!(note.starts_with("1 swept sounds in 10 s"), "{note}");
        assert!(note.contains("mouse reaching the app 4 ms (slowest 4)"), "{note}");
        assert!(note.contains("waiting its turn 90 ms (slowest 90)"), "{note}");
        assert!(note.contains("preview starting 2.0 ms"), "{note}");
        assert!(
            note.contains("opening the file 10.0 ms (slowest 10; 0 read ahead took 0.0 ms, 1 not read ahead 10.0 ms)")
        );
        assert!(note.contains("the output's next mix about 5 ms, then out of the speakers after 20 ms"), "{note}");
        // 4 + 90 + 2 + 10 + 5 + 20.
        assert!(note.ends_with("About 131 ms in all."), "{note}");
    }

    #[test]
    fn a_line_comes_once_ten_seconds_have_gone_and_the_counts_start_again() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new(t0);
        for (i, path) in ["/a.wav", "/b.wav"].into_iter().enumerate() {
            let s = 10_000 * i as u64;
            t.reached(path, None, at(t0, s));
            t.asked(path, at(t0, s + 30));
            t.opening(path, at(t0, s + 31));
            let line = t.started(path, at(t0, s + 41));
            if i == 0 {
                assert!(line.is_none());
            } else {
                let line = line.expect("a line after 10 s");
                assert!(line.starts_with("2 swept sounds in 10 s"), "{line}");
                assert!(line.contains("mouse reaching the app not known"), "{line}");
                assert!(line.contains("output delay not known"), "{line}");
            }
        }
        let next = t.note(at(t0, 21_000));
        assert!(next.starts_with("0 swept sounds in 11 s"), "{next}");
    }

    #[test]
    fn averages_and_the_slowest_are_over_every_sound() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new(t0);
        for (i, wait) in [10u64, 30, 110].into_iter().enumerate() {
            let path = format!("/{i}.wav");
            let s = 1_000 * i as u64;
            t.reached(&path, Some(1.0), at(t0, s));
            t.asked(&path, at(t0, s + wait));
            t.opening(&path, at(t0, s + wait));
            t.started(&path, at(t0, s + wait + 3));
        }
        let note = t.note(at(t0, 10_000));
        assert!(note.contains("waiting its turn 50 ms (slowest 110)"), "{note}");
        assert!(note.starts_with("3 swept sounds"), "{note}");
    }

    #[test]
    fn a_star_passed_long_ago_waits_from_when_it_was_asked_for() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new(t0);
        t.reached("/old.wav", Some(2.0), at(t0, 0));
        // Clicked on 42 s later: not a 42 s wait.
        t.asked("/old.wav", at(t0, 42_000));
        t.opening("/old.wav", at(t0, 42_001));
        t.started("/old.wav", at(t0, 42_005));
        let note = t.note(at(t0, 43_000));
        assert!(note.contains("waiting its turn 0 ms"), "{note}");
        assert!(note.contains("mouse reaching the app not known"), "{note}");
    }

    #[test]
    fn reached_again_before_its_turn_waits_from_the_newest_pass() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new(t0);
        t.reached("/a.wav", None, at(t0, 0));
        t.reached("/a.wav", None, at(t0, 500));
        t.asked("/a.wav", at(t0, 540));
        // Once asked for, a new pass doesn't move its start.
        t.reached("/a.wav", None, at(t0, 560));
        t.opening("/a.wav", at(t0, 560));
        t.started("/a.wav", at(t0, 570));
        assert!(t.note(at(t0, 10_000)).contains("waiting its turn 40 ms"));
    }

    #[test]
    fn dropped_previews_are_forgotten_and_never_counted() {
        // As the C# test: 200 asked for, all but the newest replaced before they started.
        let t0 = Instant::now();
        let mut t = SweepTiming::new(t0);
        let paths: Vec<String> = (0..200).map(|i| format!("/sweep test/{i}.wav")).collect();
        for (i, path) in paths.iter().enumerate() {
            t.reached(path, None, at(t0, i as u64));
            t.asked(path, at(t0, i as u64));
        }
        for path in &paths[..199] {
            t.dropped(path);
        }
        assert_eq!(t.waiting(), 1);
        t.opening(&paths[199], at(t0, 300));
        t.started(&paths[199], at(t0, 310));
        assert_eq!(t.waiting(), 0);
        assert!(t.note(at(t0, 10_000)).starts_with("1 swept sounds"));
    }

    #[test]
    fn stars_passed_and_never_played_are_forgotten_on_a_long_sweep() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new(t0);
        t.reached("/asked.wav", None, at(t0, 0));
        t.asked("/asked.wav", at(t0, 0));
        t.reached("/lost.wav", None, at(t0, 0));
        t.asked("/lost.wav", at(t0, 0));
        for i in 0..100 {
            t.reached(&format!("/passed {i}.wav"), None, at(t0, 31_000));
        }
        // Each time it's past the limit the passed ones go; the one asked for 31 s ago and never started goes too.
        assert!(t.waiting() <= PENDING_MOST + 1, "{}", t.waiting());
        let mut t = SweepTiming::new(t0);
        t.reached("/asked.wav", None, at(t0, 0));
        t.asked("/asked.wav", at(t0, 0));
        for i in 0..100 {
            t.reached(&format!("/passed {i}.wav"), None, at(t0, 100));
        }
        // Asked for just now: kept, and still counted when it starts.
        t.opening("/asked.wav", at(t0, 200));
        t.started("/asked.wav", at(t0, 210));
        assert!(t.note(at(t0, 10_000)).starts_with("1 swept sounds"));
    }

    #[test]
    fn a_sound_not_asked_for_by_the_sweep_is_not_counted() {
        // A click's play goes through the same reading thread but was never reached or asked for by a sweep.
        let t0 = Instant::now();
        let mut t = SweepTiming::new(t0);
        t.opening("/clicked.wav", at(t0, 0));
        assert_eq!(t.started("/clicked.wav", at(t0, 20_000)), None);
        // Reached but never asked for (passed by): not counted either.
        t.reached("/passed.wav", None, at(t0, 0));
        t.opening("/passed.wav", at(t0, 1));
        assert_eq!(t.started("/passed.wav", at(t0, 20_000)), None);
        assert!(t.note(at(t0, 20_000)).starts_with("0 swept sounds"));
    }

    #[test]
    fn opens_read_ahead_are_told_from_cold_ones() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new(t0);
        t.read_ahead("/warm.wav");
        for (path, took) in [("/warm.wav", 2u64), ("/cold.wav", 30)] {
            t.reached(path, None, at(t0, 0));
            t.asked(path, at(t0, 10));
            t.opening(path, at(t0, 10));
            t.started(path, at(t0, 10 + took));
        }
        let note = t.note(at(t0, 10_000));
        assert!(note.contains("1 read ahead took 2.0 ms, 1 not read ahead 30.0 ms"), "{note}");
        // Only the newest few read ahead are remembered.
        for i in 0..100 {
            t.read_ahead(&format!("/{i}.wav"));
        }
        assert_eq!(t.read_ahead.len(), READ_AHEAD_KEPT);
        assert!(!t.read_ahead.iter().any(|p| p == "/warm.wav"));
    }
}
