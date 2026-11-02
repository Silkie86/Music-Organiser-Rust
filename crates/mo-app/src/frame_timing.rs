//! Frame timing (rule 12, no lag): what the window's thread spends its time on, summed up in the session log every
//! 10 seconds as a `[Frames]` line, so a hitch can be pinned on a step instead of guessed at.
//!
//! Two things are counted:
//! - **Frames.** Each frame is the window drawing itself (Slint laying out and drawing the items, between its
//!   "before" and "after rendering" calls) plus the time of every marked step (`watchdog::doing`) since the last
//!   frame: packing, uploading and drawing the galaxy, setting the window's properties. A frame over [`SLOW`] is slow,
//!   and the line says where its time went. Each marked step counts only its own time, not the steps inside it, so
//!   nothing is counted twice.
//! - **Late answers.** The freeze watchdog knocks on the window's thread five times a second. An answer later than
//!   [`SLOW`] means the thread was busy; the line says in which marked step it was, or in none (then Slint itself:
//!   handling input, laying out, or handing the frame to the screen). Freezes over 250 ms still get their own
//!   `[Freeze]` line.
//!
//! Nothing here waits: each call takes a short lock, and the line goes to the session log's own writer.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::log;

/// A frame (or a late answer) over this is a hitch: two frames missed at 60 frames a second.
pub const SLOW: Duration = Duration::from_millis(33);
/// How much time is summed up in one log line.
pub const EVERY: Duration = Duration::from_secs(10);
/// A quiet stretch (no slow frames, no late answers) is only written when the window drew at least this many frames:
/// a sweep or a glide, worth a line saying it went smoothly.
const BUSY_FRAMES: u32 = 100;
/// What Slint drawing the window is called in the lines.
pub const DRAWING: &str = "drawing the window";
/// What a late answer outside every marked step is called.
pub const UNMARKED: &str = "no marked step (Slint handling input, laying out or showing the frame)";
/// At most this many parts are named in a line, the biggest first.
const PARTS_NAMED: usize = 6;

/// Where some of the time went: one step's total, in how many frames (or late answers) and its longest, in ms.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Part {
    what: &'static str,
    total: f64,
    count: u32,
    most: f64,
}

fn add(parts: &mut Vec<Part>, what: &'static str, ms: f64) {
    match parts.iter_mut().find(|p| p.what == what) {
        Some(p) => {
            p.total += ms;
            p.count += 1;
            p.most = p.most.max(ms);
        }
        None => parts.push(Part { what, total: ms, count: 1, most: ms }),
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// The window's frame timings. The app keeps one (see the free functions below); tests make their own.
#[derive(Debug)]
pub struct FrameTiming {
    since: Instant,
    page: &'static str,
    frames: u32,
    draw_total: f64,
    draw_most: f64,
    /// The marked steps' own times since the last frame, one entry a step (so it can't grow).
    this_frame: Vec<(&'static str, f64)>,
    slow: u32,
    slowest: f64,
    slow_parts: Vec<Part>,
    late: u32,
    late_parts: Vec<Part>,
}

impl FrameTiming {
    pub fn new(now: Instant) -> FrameTiming {
        FrameTiming {
            since: now,
            page: "?",
            frames: 0,
            draw_total: 0.0,
            draw_most: 0.0,
            this_frame: Vec::new(),
            slow: 0,
            slowest: 0.0,
            slow_parts: Vec::new(),
            late: 0,
            late_parts: Vec::new(),
        }
    }

    /// A marked step on the window's thread took `own` (its time less the steps inside it).
    pub fn step(&mut self, what: &'static str, own: Duration) {
        let t = ms(own);
        match self.this_frame.iter_mut().find(|(w, _)| *w == what) {
            Some((_, total)) => *total += t,
            None => self.this_frame.push((what, t)),
        }
    }

    /// A frame was drawn, which took `draw`, on `page`. Gives the summary line when one is due.
    pub fn drawn(&mut self, draw: Duration, page: &'static str, now: Instant) -> Option<String> {
        let draw = ms(draw);
        self.page = page;
        self.frames += 1;
        self.draw_total += draw;
        self.draw_most = self.draw_most.max(draw);
        let total = draw + self.this_frame.iter().map(|(_, t)| t).sum::<f64>();
        if total >= ms(SLOW) {
            self.slow += 1;
            self.slowest = self.slowest.max(total);
            add(&mut self.slow_parts, DRAWING, draw);
            for &(what, t) in &self.this_frame {
                add(&mut self.slow_parts, what, t);
            }
        }
        self.this_frame.clear();
        self.due(now)
    }

    /// The watchdog's knock waited `waited` for an answer, while the window was in the marked step `busy` (or none).
    pub fn late(&mut self, waited: Duration, busy: Option<&'static str>, now: Instant) -> Option<String> {
        if waited >= SLOW {
            self.late += 1;
            add(&mut self.late_parts, busy.unwrap_or(UNMARKED), ms(waited));
        }
        self.due(now)
    }

    fn due(&mut self, now: Instant) -> Option<String> {
        if now.saturating_duration_since(self.since) < EVERY {
            return None;
        }
        self.note(now)
    }

    /// The summary line, or `None` for a quiet stretch; the counts start again either way.
    pub fn note(&mut self, now: Instant) -> Option<String> {
        let quiet = self.slow == 0 && self.late == 0 && self.frames < BUSY_FRAMES;
        let line = (!quiet).then(|| self.line(now));
        let page = self.page;
        *self = FrameTiming::new(now);
        self.page = page;
        line
    }

    fn line(&self, now: Instant) -> String {
        let secs = now.saturating_duration_since(self.since).as_secs_f64();
        let average = if self.frames == 0 { 0.0 } else { self.draw_total / f64::from(self.frames) };
        let mut line = format!(
            "{} frames in {secs:.0} s on {}, {DRAWING} took {average:.1} ms on average (slowest {:.0} ms); ",
            self.frames, self.page, self.draw_most
        );
        if self.slow == 0 {
            line.push_str("no frame over 33 ms. ");
        } else {
            line.push_str(&format!(
                "{} over 33 ms (slowest {:.0} ms), their time went to: {}. ",
                self.slow,
                self.slowest,
                named(&self.slow_parts, "in")
            ));
        }
        if self.late == 0 {
            line.push_str("The window answered every knock within 33 ms.");
        } else {
            line.push_str(&format!(
                "The window answered late (over 33 ms) {} time{}, busy with: {}.",
                self.late,
                if self.late == 1 { "" } else { "s" },
                named(&self.late_parts, "time")
            ));
        }
        line
    }
}

/// The parts, the biggest first: "Galaxy: draw on the card 90 ms (in 2, longest 45)".
fn named(parts: &[Part], count_word: &str) -> String {
    let mut parts: Vec<Part> = parts.iter().copied().filter(|p| p.total >= 0.5).collect();
    parts.sort_by(|a, b| b.total.total_cmp(&a.total));
    let more = parts.len().saturating_sub(PARTS_NAMED);
    let mut said: Vec<String> = parts
        .iter()
        .take(PARTS_NAMED)
        .map(|p| match count_word {
            "in" => format!("{} {:.0} ms (in {}, longest {:.0})", p.what, p.total, p.count, p.most),
            _ => {
                format!("{} {} time{} (longest {:.0} ms)", p.what, p.count, if p.count == 1 { "" } else { "s" }, p.most)
            }
        })
        .collect();
    if more > 0 {
        said.push(format!("{more} more"));
    }
    said.join(", ")
}

/// The app's frame timings.
static TIMING: Mutex<Option<FrameTiming>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut FrameTiming, Instant) -> T) -> T {
    let now = Instant::now();
    let mut t = TIMING.lock().unwrap_or_else(PoisonError::into_inner);
    f(t.get_or_insert_with(|| FrameTiming::new(now)), now)
}

fn write(line: Option<String>) {
    if let Some(line) = line {
        log::info("Frames", &line);
    }
}

/// See [`FrameTiming::step`].
pub fn step(what: &'static str, own: Duration) {
    with(|t, _| t.step(what, own));
}

/// See [`FrameTiming::drawn`]; a summary that's due goes to the session log as a `[Frames]` line.
pub fn drawn(draw: Duration, page: &'static str) {
    write(with(|t, now| t.drawn(draw, page, now)));
}

/// See [`FrameTiming::late`]; a summary that's due goes to the session log.
pub fn late(waited: Duration, busy: Option<&'static str>) {
    write(with(|t, now| t.late(waited, busy, now)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    const fn d(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn a_slow_frame_says_where_its_time_went() {
        let t0 = Instant::now();
        let mut f = FrameTiming::new(t0);
        f.step("Galaxy: draw on the card", d(30));
        f.step("Galaxy: move the picture", d(4));
        f.step("Galaxy: draw on the card", d(10));
        assert_eq!(f.drawn(d(8), "Sounds", at(t0, 100)), None);
        // A quick frame isn't counted as slow, and its steps don't carry over.
        f.step("Galaxy: hover", d(2));
        f.drawn(d(5), "Sounds", at(t0, 116));
        let line = f.note(at(t0, 10_000)).expect("a slow frame is worth a line");
        assert!(
            line.starts_with("2 frames in 10 s on Sounds, drawing the window took 6.5 ms on average (slowest 8 ms)")
        );
        assert!(line.contains("1 over 33 ms (slowest 52 ms)"), "{line}");
        assert!(
            line.contains(
                "their time went to: Galaxy: draw on the card 40 ms (in 1, longest 40), drawing the window 8 ms (in 1, \
                 longest 8), Galaxy: move the picture 4 ms (in 1, longest 4)."
            ),
            "{line}"
        );
        assert!(!line.contains("hover"), "{line}");
        assert!(line.ends_with("The window answered every knock within 33 ms."), "{line}");
    }

    #[test]
    fn late_answers_say_which_step_held_the_window() {
        let t0 = Instant::now();
        let mut f = FrameTiming::new(t0);
        f.late(d(20), None, at(t0, 0));
        f.late(d(60), Some(DRAWING), at(t0, 200));
        f.late(d(40), None, at(t0, 400));
        f.late(d(332), None, at(t0, 600));
        let line = f.late(d(5), None, at(t0, 10_000)).expect("due after 10 s");
        assert!(line.contains("0 frames in 10 s"), "{line}");
        assert!(line.contains("no frame over 33 ms"), "{line}");
        assert!(
            line.ends_with(&format!(
                "late (over 33 ms) 3 times, busy with: {UNMARKED} 2 times (longest 332 ms), drawing the window 1 time \
                 (longest 60 ms)."
            )),
            "{line}"
        );
    }

    #[test]
    fn a_quiet_stretch_writes_nothing_and_a_smooth_busy_one_says_so() {
        let t0 = Instant::now();
        let mut f = FrameTiming::new(t0);
        for i in 0..10 {
            f.drawn(d(3), "Songs", at(t0, i * 16));
        }
        assert_eq!(f.drawn(d(3), "Songs", at(t0, 10_000)), None);
        let start = at(t0, 10_000);
        for i in 0..600 {
            f.drawn(d(4), "Sounds", start + d(i * 16));
        }
        let line = f.note(start + d(10_000)).expect("a busy stretch");
        assert!(line.starts_with("600 frames in 10 s on Sounds, drawing the window took 4.0 ms"), "{line}");
        assert!(line.contains("no frame over 33 ms"), "{line}");
        // The counts started again.
        assert_eq!(f.note(start + d(20_000)), None);
    }

    #[test]
    fn steps_merge_by_name_so_the_list_cannot_grow_without_frames() {
        let t0 = Instant::now();
        let mut f = FrameTiming::new(t0);
        for _ in 0..10_000 {
            f.step("Galaxy: sweep", d(1));
        }
        assert_eq!(f.this_frame.len(), 1);
        f.drawn(d(1), "Sounds", at(t0, 1));
        let line = f.note(at(t0, 10_000)).expect("a slow frame");
        assert!(line.contains("Galaxy: sweep 10000 ms (in 1, longest 10000)"), "{line}");
    }

    #[test]
    fn only_the_biggest_parts_are_named() {
        const NAMES: [&str; 9] = ["a", "b", "c", "d", "e", "f", "g", "h", "i"];
        let t0 = Instant::now();
        let mut f = FrameTiming::new(t0);
        for (i, n) in NAMES.iter().enumerate() {
            f.step(n, d(10 + i as u64));
        }
        f.drawn(d(0), "Sounds", at(t0, 1));
        let line = f.note(at(t0, 10_000)).expect("a slow frame");
        assert!(line.contains("their time went to: i 18 ms"), "{line}");
        assert!(line.contains("d 13 ms (in 1, longest 13), 3 more."), "{line}");
    }
}
