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
//! A sound whose read was overtaken by a newer one (the mouse moved on while it was being read) is never heard, but
//! how long its read took still counts: slow, cold opens are the ones overtaken most, so leaving them out would hide
//! them. The line gives them their own step, "dropped while being read".
//!
//! Every call only takes a short lock and never waits on the disk: the summary goes to the session log's own writer.
//! The galaxy calls [`reached`] and [`asked`] on the window's thread; the reading thread calls the rest, with the
//! number of its read, so an old read finishing never touches a newer ask for the same sound.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::log;

/// How much sweeping is summed up in one log line: from the first sound counted.
pub const EVERY: Duration = Duration::from_secs(10);
/// A star the mouse reached this long before its turn came was passed by and asked for again later (a click on it,
/// say): its wait is counted from the ask, not from the old pass. Reached again within it, it still waits from its
/// first pass.
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
    /// Once asked for, when the mouse reached it again (swept back over it while it was being read, say): a new ask
    /// waits from then, not from the first pass.
    again: Option<Instant>,
    /// When the reading thread began opening it, which read that is, and whether its start had been read ahead.
    opening: Option<Opening>,
}

#[derive(Clone, Copy, Debug)]
struct Opening {
    at: Instant,
    read: u64,
    ahead: bool,
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
    /// Reads overtaken by a newer sound while they were being read: how long they had taken.
    overtaken: Step,
    sounds: u32,
    mix_ms: Option<f64>,
    output_ms: Option<f64>,
    /// When the first sound of this stretch was counted (`None` until one is).
    since: Option<Instant>,
}

fn ms(from: Instant, to: Instant) -> f64 {
    to.saturating_duration_since(from).as_secs_f64() * 1000.0
}

impl Default for SweepTiming {
    fn default() -> SweepTiming {
        SweepTiming::new()
    }
}

impl SweepTiming {
    pub fn new() -> SweepTiming {
        SweepTiming {
            pending: HashMap::new(),
            read_ahead: VecDeque::new(),
            input: Step::default(),
            turn: Step::default(),
            start: Step::default(),
            open: Step::default(),
            open_ahead: Step::default(),
            open_cold: Step::default(),
            overtaken: Step::default(),
            sounds: 0,
            mix_ms: None,
            output_ms: None,
            since: None,
        }
    }

    /// How many sounds are waiting to be heard.
    pub fn waiting(&self) -> usize {
        self.pending.len()
    }

    /// The mouse reached a star. `input_ms` is how long after the move happened the window got to it, when known.
    pub fn reached(&mut self, path: &str, input_ms: Option<f64>, now: Instant) {
        // A long sweep's passed stars that never played are dropped; never one asked for lately (it may be being
        // opened), nor one reached in the last moments (it may be waiting its turn). One asked for long ago that never
        // started is gone too, so the list can't grow all session.
        if self.pending.len() > PENDING_MOST {
            self.pending.retain(|_, p| {
                p.asked.is_some_and(|asked| now.saturating_duration_since(asked) <= LOST)
                    || now.saturating_duration_since(p.reached) < STALE
            });
        }
        // Reached again before it was asked for: it still waits from its first pass, unless that was long ago (then
        // it waits from now, not from seconds ago).
        // Once asked for, it stays (its read may be going on), and a new pass is kept for the next ask.
        if let Some(p) = self.pending.get_mut(path)
            && p.asked.is_some()
        {
            if p.again.is_none_or(|a| now.saturating_duration_since(a) >= STALE) {
                p.again = Some(now);
            }
            return;
        }
        let keep = self.pending.get(path).is_some_and(|p| now.saturating_duration_since(p.reached) < STALE);
        if !keep {
            let input_ms = input_ms.map(|i| i.max(0.0));
            self.pending
                .insert(path.to_owned(), Pending { reached: now, input_ms, asked: None, again: None, opening: None });
        }
    }

    /// Its turn came: the preview was asked for.
    pub fn asked(&mut self, path: &str, now: Instant) {
        // Reached long ago and passed by (a click on it now, say): its wait starts now. Counted from the old pass,
        // one such star once read as a 42 s wait in the C# app and made a 10 s average 1.8 s.
        // Asked for before (its read may still be going on, and keeps its place so it can be counted): this ask waits
        // from the mouse's newer pass over it.
        let was = self.pending.get(path).copied();
        let reached = match was {
            Some(p) if p.asked.is_some() => p.again,
            Some(p) => Some(p.reached),
            None => None,
        }
        .filter(|r| now.saturating_duration_since(*r) < STALE);
        let p = Pending {
            reached: reached.unwrap_or(now),
            input_ms: if reached.is_some() && was.is_some_and(|p| p.asked.is_none()) {
                was.and_then(|p| p.input_ms)
            } else {
                None
            },
            asked: Some(now),
            again: None,
            opening: was.and_then(|p| p.opening),
        };
        self.pending.insert(path.to_owned(), p);
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

    /// The reading thread is opening it now, as its read number `read`. A newer read for the same sound takes over
    /// (the older one's thread may only now be getting going); an older one never does.
    pub fn opening(&mut self, path: &str, read: u64, now: Instant) {
        let ahead = self.read_ahead.iter().any(|p| p == path);
        if let Some(p) = self.pending.get_mut(path)
            && p.asked.is_some()
            && p.opening.is_none_or(|o| read > o.read)
        {
            p.opening = Some(Opening { at: now, read, ahead });
        }
    }

    /// Takes the sound being opened by read `read`. When the sound was asked for again after that read began, the
    /// newer ask stays, waiting for its own read.
    fn take(&mut self, path: &str, read: u64) -> Option<(Pending, Opening)> {
        let p = self.pending.get_mut(path)?;
        let o = p.opening.filter(|o| o.read == read)?;
        let taken = *p;
        if p.asked.is_some_and(|asked| asked > o.at) {
            p.opening = None;
        } else {
            self.pending.remove(path);
        }
        Some((taken, o))
    }

    /// The first sound of a stretch starts its clock.
    fn counted(&mut self, now: Instant) -> Option<String> {
        let since = *self.since.get_or_insert(now);
        (now.saturating_duration_since(since) >= EVERY).then(|| self.note(now))
    }

    /// Read `read` couldn't be played: it's forgotten.
    pub fn dropped(&mut self, path: &str, read: u64) {
        self.take(path, read);
    }

    /// Read `read` was overtaken by a newer sound while it was being read: its read time is counted as dropped while
    /// being read. Gives the summary line when one is due.
    pub fn overtaken(&mut self, path: &str, read: u64, now: Instant) -> Option<String> {
        let (_, o) = self.take(path, read)?;
        self.overtaken.add(ms(o.at, now));
        self.counted(now)
    }

    /// Read `read` is done and handed to the output, which mixes it in from its next pass. Gives the summary line when
    /// one is due.
    pub fn started(&mut self, path: &str, read: u64, now: Instant) -> Option<String> {
        let (p, o) = self.take(path, read)?;
        let asked = p.asked?;
        if let Some(i) = p.input_ms {
            self.input.add(i);
        }
        self.turn.add(ms(p.reached, asked));
        self.start.add(ms(asked, o.at));
        let open = ms(o.at, now);
        self.open.add(open);
        if o.ahead { &mut self.open_ahead } else { &mut self.open_cold }.add(open);
        self.sounds += 1;
        self.counted(now)
    }

    /// What the output adds after that: the wait for its next mix and how long a mixed sound takes to come out. `None`
    /// when not known.
    pub fn output(&mut self, mix_ms: Option<f64>, output_ms: Option<f64>) {
        // An output just opened doesn't know yet: what was known before stands.
        self.mix_ms = mix_ms.or(self.mix_ms);
        self.output_ms = output_ms.or(self.output_ms);
    }

    /// The line for what's been counted so far, when anything has (as the app closes, say), and the counts start
    /// again.
    pub fn rest(&mut self, now: Instant) -> Option<String> {
        self.since.is_some().then(|| self.note(now))
    }

    /// The summary line, and the counts start again.
    pub fn note(&mut self, now: Instant) -> String {
        let (input, turn, start, open) = (self.input, self.turn, self.start, self.open);
        let (ahead, cold, overtaken) = (self.open_ahead, self.open_cold, self.overtaken);
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
        let dropped = if overtaken.count == 0 {
            String::new()
        } else {
            format!(
                " {} dropped while being read, overtaken by a newer sound (their reads took {:.1} ms on average, \
                 slowest {:.0}).",
                overtaken.count,
                overtaken.average(),
                overtaken.most
            )
        };
        let since = self.since.unwrap_or(now);
        let note = format!(
            "{} swept sounds in {:.0} s, from the mouse over a star to hearing it: {mouse}, waiting its turn {:.0} ms \
             (slowest {:.0}), preview starting {:.1} ms (slowest {:.0}), opening the file {:.1} ms (slowest {:.0}; {} \
             read ahead took {:.1} ms, {} not read ahead {:.1} ms), {mix}{out}About {total:.0} ms in all.{dropped}",
            self.sounds,
            now.saturating_duration_since(since).as_secs_f64(),
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
            &mut self.overtaken,
        ] {
            *s = Step::default();
        }
        self.sounds = 0;
        self.since = None;
        note
    }
}

/// The app's sweep timings.
static TIMING: Mutex<Option<SweepTiming>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut SweepTiming, Instant) -> T) -> T {
    let now = Instant::now();
    let mut t = TIMING.lock().unwrap_or_else(PoisonError::into_inner);
    f(t.get_or_insert_with(SweepTiming::new), now)
}

/// Written after the lock is let go: the log only queues the line, but nothing else needs to wait for it.
fn write(note: Option<String>) {
    if let Some(note) = note {
        log::info("Sweep", &note);
    }
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
pub fn dropped(path: &str, read: u64) {
    with(|t, _| t.dropped(path, read));
}

/// See [`SweepTiming::overtaken`]; a summary that's due goes to the session log as a `[Sweep]` line.
pub fn overtaken(path: &str, read: u64) {
    write(with(|t, now| t.overtaken(path, read, now)));
}

/// See [`SweepTiming::read_ahead`].
pub fn read_ahead(path: &str) {
    with(|t, _| t.read_ahead(path));
}

/// See [`SweepTiming::opening`].
pub fn opening(path: &str, read: u64) {
    with(|t, now| t.opening(path, read, now));
}

/// See [`SweepTiming::started`]; a summary that's due goes to the session log as a `[Sweep]` line.
pub fn started(path: &str, read: u64) {
    write(with(|t, now| t.started(path, read, now)));
}

/// See [`SweepTiming::output`].
pub fn output(mix_ms: Option<f64>, output_ms: Option<f64>) {
    with(|t, _| t.output(mix_ms, output_ms));
}

/// Writes what's been counted since the last line, as the app closes.
pub fn finish() {
    write(with(|t, now| t.rest(now)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    /// One sound through every step: reached at `reached`, asked at `asked`, opened at `opening`, started at `done`.
    fn heard(
        t: &mut SweepTiming,
        path: &str,
        read: u64,
        [reached, asked, opening, done]: [Instant; 4],
    ) -> Option<String> {
        t.reached(path, None, reached);
        t.asked(path, asked);
        t.opening(path, read, opening);
        t.started(path, read, done)
    }

    #[test]
    fn each_step_is_timed_from_the_mouse_to_the_output() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new();
        t.output(Some(5.0), Some(20.0));
        t.reached("/a.wav", Some(4.0), at(t0, 0));
        t.asked("/a.wav", at(t0, 90));
        t.opening("/a.wav", 1, at(t0, 92));
        assert_eq!(t.started("/a.wav", 1, at(t0, 102)), None, "no line before 10 s");
        let note = t.note(at(t0, 10_102));
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
    fn the_ten_seconds_are_counted_from_the_first_sound_not_the_clock() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new();
        // A minute of nothing, then sweeping: no line until 10 s after the first sound.
        let s = 60_000;
        assert!(heard(&mut t, "/a.wav", 1, [at(t0, s), at(t0, s + 30), at(t0, s + 31), at(t0, s + 41)]).is_none());
        let s = 70_000;
        let line = heard(&mut t, "/b.wav", 2, [at(t0, s), at(t0, s + 30), at(t0, s + 31), at(t0, s + 41)])
            .expect("a line 10 s after the first sound");
        assert!(line.starts_with("2 swept sounds in 10 s"), "{line}");
        assert!(line.contains("mouse reaching the app not known"), "{line}");
        assert!(line.contains("output delay not known"), "{line}");
        // The counts started again, and nothing's left to write.
        assert_eq!(t.rest(at(t0, 90_000)), None);
    }

    #[test]
    fn what_is_left_is_written_as_the_app_closes() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new();
        heard(&mut t, "/a.wav", 1, [at(t0, 0), at(t0, 20), at(t0, 21), at(t0, 30)]);
        let rest = t.rest(at(t0, 3_000)).expect("a sound was counted");
        assert!(rest.starts_with("1 swept sounds in 3 s"), "{rest}");
    }

    #[test]
    fn averages_and_the_slowest_are_over_every_sound() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new();
        for (i, wait) in [10u64, 30, 110].into_iter().enumerate() {
            let path = format!("/{i}.wav");
            let s = 1_000 * i as u64;
            t.reached(&path, Some(1.0), at(t0, s));
            t.asked(&path, at(t0, s + wait));
            t.opening(&path, i as u64, at(t0, s + wait));
            t.started(&path, i as u64, at(t0, s + wait + 3));
        }
        let note = t.note(at(t0, 10_000));
        assert!(note.contains("waiting its turn 50 ms (slowest 110)"), "{note}");
        assert!(note.starts_with("3 swept sounds"), "{note}");
    }

    #[test]
    fn a_star_passed_long_ago_waits_from_when_it_was_asked_for() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new();
        t.reached("/old.wav", Some(2.0), at(t0, 0));
        // Clicked on 42 s later: not a 42 s wait.
        t.asked("/old.wav", at(t0, 42_000));
        t.opening("/old.wav", 1, at(t0, 42_001));
        t.started("/old.wav", 1, at(t0, 42_005));
        let note = t.note(at(t0, 43_000));
        assert!(note.contains("waiting its turn 0 ms"), "{note}");
        assert!(note.contains("mouse reaching the app not known"), "{note}");
    }

    #[test]
    fn a_star_reached_on_every_move_waits_from_its_first_pass() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new();
        for ms in [0, 10, 20] {
            t.reached("/a.wav", None, at(t0, ms));
        }
        t.asked("/a.wav", at(t0, 90));
        t.opening("/a.wav", 1, at(t0, 90));
        t.started("/a.wav", 1, at(t0, 100));
        assert!(t.note(at(t0, 10_000)).contains("waiting its turn 90 ms"));
        // Reached again seconds later, before it was asked for: it waits from the newer pass.
        t.reached("/b.wav", None, at(t0, 0));
        t.reached("/b.wav", None, at(t0, 2_500));
        t.asked("/b.wav", at(t0, 2_540));
        // Once asked for, a new pass doesn't move its start.
        t.reached("/b.wav", None, at(t0, 2_560));
        t.opening("/b.wav", 2, at(t0, 2_560));
        t.started("/b.wav", 2, at(t0, 2_570));
        assert!(t.note(at(t0, 12_000)).contains("waiting its turn 40 ms"));
    }

    #[test]
    fn dropped_previews_are_forgotten_and_never_counted() {
        // As the C# test: 200 asked for, all but the newest replaced before they started.
        let t0 = Instant::now();
        let mut t = SweepTiming::new();
        let paths: Vec<String> = (0..200).map(|i| format!("/sweep test/{i}.wav")).collect();
        for (i, path) in paths.iter().enumerate() {
            t.reached(path, None, at(t0, i as u64));
            t.asked(path, at(t0, i as u64));
            t.opening(path, i as u64, at(t0, i as u64));
        }
        for (i, path) in paths[..199].iter().enumerate() {
            t.dropped(path, i as u64);
        }
        assert_eq!(t.waiting(), 1);
        t.started(&paths[199], 199, at(t0, 310));
        assert_eq!(t.waiting(), 0);
        assert!(t.note(at(t0, 10_000)).starts_with("1 swept sounds"));
    }

    #[test]
    fn a_read_overtaken_while_being_read_still_counts_how_long_it_took() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new();
        t.reached("/cold.wav", None, at(t0, 0));
        t.asked("/cold.wav", at(t0, 0));
        t.opening("/cold.wav", 1, at(t0, 1));
        // A slow, cold open: the mouse has moved on by the time it's read.
        t.overtaken("/cold.wav", 1, at(t0, 401));
        heard(&mut t, "/warm.wav", 2, [at(t0, 100), at(t0, 190), at(t0, 191), at(t0, 195)]);
        let note = t.note(at(t0, 10_000));
        assert!(note.starts_with("1 swept sounds"), "{note}");
        assert!(
            note.ends_with(
                "1 dropped while being read, overtaken by a newer sound (their reads took 400.0 ms on \
                                average, slowest 400)."
            ),
            "{note}"
        );
    }

    #[test]
    fn an_old_read_finishing_leaves_a_newer_ask_for_the_same_sound_alone() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new();
        t.reached("/a.wav", None, at(t0, 0));
        t.asked("/a.wav", at(t0, 0));
        t.opening("/a.wav", 1, at(t0, 1));
        // Asked for again (swept back over it) while read 1 is still reading; read 2 will open it.
        t.asked("/a.wav", at(t0, 500));
        t.dropped("/a.wav", 1);
        t.overtaken("/a.wav", 1, at(t0, 600));
        assert_eq!(t.waiting(), 1, "read 1 leaves the newer ask alone");
        t.opening("/a.wav", 2, at(t0, 601));
        t.started("/a.wav", 2, at(t0, 611));
        assert!(t.note(at(t0, 10_000)).contains("opening the file 10.0 ms"));
    }

    #[test]
    fn an_old_read_getting_going_late_doesnt_take_a_newer_reads_place() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new();
        t.reached("/a.wav", None, at(t0, 0));
        t.asked("/a.wav", at(t0, 0));
        // Swept back over it before read 1's thread got going: read 1 opens late, then read 2.
        t.asked("/a.wav", at(t0, 50));
        t.opening("/a.wav", 1, at(t0, 60));
        t.opening("/a.wav", 2, at(t0, 61));
        t.overtaken("/a.wav", 1, at(t0, 70));
        assert_eq!(t.waiting(), 1, "read 1 leaves read 2's ask alone");
        // And the other way round: read 2 first, then read 1 late, which doesn't take over.
        t.opening("/a.wav", 1, at(t0, 62));
        t.started("/a.wav", 2, at(t0, 71));
        assert!(t.note(at(t0, 10_000)).contains("opening the file 10.0 ms"));
    }

    #[test]
    fn swept_back_over_while_being_read_the_new_wait_is_from_the_new_pass_and_the_old_read_still_counts() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new();
        t.reached("/a.wav", None, at(t0, 0));
        t.asked("/a.wav", at(t0, 0));
        t.opening("/a.wav", 1, at(t0, 1));
        // Back over it at 100 ms while read 1 is still a slow, cold open; its turn comes at 120 ms.
        t.reached("/a.wav", None, at(t0, 100));
        t.asked("/a.wav", at(t0, 120));
        // Read 1 finishes, overtaken: its 400 ms count, and the newer ask stays for read 2.
        t.overtaken("/a.wav", 1, at(t0, 401));
        assert_eq!(t.waiting(), 1);
        t.opening("/a.wav", 2, at(t0, 402));
        t.started("/a.wav", 2, at(t0, 405));
        let note = t.note(at(t0, 10_000));
        assert!(note.contains("waiting its turn 20 ms"), "{note}");
        assert!(note.contains("1 dropped while being read, overtaken by a newer sound (their reads took 400.0 ms"));
    }

    #[test]
    fn an_output_that_doesnt_know_yet_keeps_what_was_known() {
        let mut t = SweepTiming::new();
        t.output(Some(5.0), Some(20.0));
        t.output(None, None);
        let t0 = Instant::now();
        heard(&mut t, "/a.wav", 1, [at(t0, 0), at(t0, 0), at(t0, 0), at(t0, 1)]);
        assert!(t.note(at(t0, 10_000)).contains("next mix about 5 ms, then out of the speakers after 20 ms"));
    }

    #[test]
    fn stars_passed_and_never_played_are_forgotten_on_a_long_sweep() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new();
        t.reached("/lost.wav", None, at(t0, 0));
        t.asked("/lost.wav", at(t0, 0));
        for i in 0..100 {
            t.reached(&format!("/passed {i}.wav"), None, at(t0, 31_000 + 100 * i));
        }
        // Past the limit, what's neither asked for lately nor reached in the last 2 s goes, the 31 s old ask too.
        assert!(t.waiting() <= PENDING_MOST + 1, "{}", t.waiting());
        assert!(!t.pending.contains_key("/lost.wav"));

        let mut t = SweepTiming::new();
        t.reached("/asked.wav", None, at(t0, 0));
        t.asked("/asked.wav", at(t0, 0));
        // The star waiting its turn, reached a moment ago.
        t.reached("/waiting.wav", None, at(t0, 5_000));
        for i in 0..100 {
            t.reached(&format!("/passed {i}.wav"), None, at(t0, 3_000 + i));
        }
        t.reached("/last.wav", None, at(t0, 5_050));
        assert!(t.pending.contains_key("/waiting.wav"), "the star waiting its turn is kept");
        // Asked for just now: kept, and still counted when it starts.
        t.opening("/asked.wav", 1, at(t0, 5_100));
        t.started("/asked.wav", 1, at(t0, 5_110));
        assert!(t.note(at(t0, 10_000)).starts_with("1 swept sounds"));
    }

    #[test]
    fn a_sound_not_asked_for_by_the_sweep_is_not_counted() {
        // A click's play goes through the same reading thread but was never reached or asked for by a sweep.
        let t0 = Instant::now();
        let mut t = SweepTiming::new();
        t.opening("/clicked.wav", 1, at(t0, 0));
        assert_eq!(t.started("/clicked.wav", 1, at(t0, 20_000)), None);
        // Reached but never asked for (passed by): not counted either.
        t.reached("/passed.wav", None, at(t0, 0));
        t.opening("/passed.wav", 2, at(t0, 1));
        assert_eq!(t.started("/passed.wav", 2, at(t0, 20_000)), None);
        assert_eq!(t.rest(at(t0, 20_000)), None);
    }

    #[test]
    fn opens_read_ahead_are_told_from_cold_ones() {
        let t0 = Instant::now();
        let mut t = SweepTiming::new();
        t.read_ahead("/warm.wav");
        for (read, (path, took)) in [("/warm.wav", 2u64), ("/cold.wav", 30)].into_iter().enumerate() {
            heard(&mut t, path, read as u64, [at(t0, 0), at(t0, 10), at(t0, 10), at(t0, 10 + took)]);
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
