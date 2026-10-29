//! Activity cards: every job reports through here, and the window shows one card per job, bottom right: started,
//! progress, finished or failed (spec 3.1, kept). A running card can be put away; "N running" brings them back.
//!
//! Jobs run on any thread. They hold a [`Job`] and report through it; the feed keeps the cards and calls the window
//! back when something changed, at most every 250 ms for progress (rule 12: no progress flood). Each start and end
//! also goes into the session log as a `[Job]` line, and the freeze watchdog reads the running jobs from here without
//! touching the window.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::log;

/// Progress is passed on at most this often.
pub const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// How long a finished card stays before it goes by itself.
pub const DONE_STAYS: Duration = Duration::from_secs(6);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Done,
    Failed,
}

/// One card.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: u64,
    pub title: String,
    pub detail: String,
    /// 0 to 1, or `None` while it can't say how far it is.
    pub progress: Option<f32>,
    pub state: State,
    /// Put away by its × while it runs.
    pub put_away: bool,
    pub started: Instant,
    pub ended: Option<Instant>,
}

type Notify = Box<dyn Fn() + Send + Sync>;

struct Inner {
    cards: Vec<Card>,
    notify: Option<Arc<Notify>>,
    last_progress: Instant,
}

/// Every job's card. Cheap to clone; clones share the cards.
#[derive(Clone)]
pub struct Feed {
    inner: Arc<Mutex<Inner>>,
    next_id: Arc<AtomicU64>,
}

impl Default for Feed {
    fn default() -> Feed {
        Feed {
            inner: Arc::new(Mutex::new(Inner {
                cards: Vec::new(),
                notify: None,
                last_progress: Instant::now() - PROGRESS_EVERY,
            })),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }
}

impl Feed {
    /// Called (on whichever thread changed something) when the cards change. The window uses it to redraw them.
    pub fn on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        self.lock().notify = Some(Arc::new(Box::new(f)));
    }

    /// Starts a job's card.
    pub fn start(&self, title: &str, detail: &str) -> Job {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.lock().cards.push(Card {
            id,
            title: title.to_owned(),
            detail: detail.to_owned(),
            progress: None,
            state: State::Running,
            put_away: false,
            started: Instant::now(),
            ended: None,
        });
        log::info("Job", &format!("Started: {title}{}", bracket(detail)));
        self.changed();
        Job { feed: self.clone(), id, finished: false }
    }

    /// The cards to show now, oldest first: put-away ones and finished ones past their time are left out.
    pub fn visible(&self, now: Instant) -> Vec<Card> {
        self.lock()
            .cards
            .iter()
            .filter(|c| !c.put_away && c.ended.is_none_or(|e| now.duration_since(e) < DONE_STAYS))
            .cloned()
            .collect()
    }

    /// How many jobs are running, put away or not.
    pub fn running(&self) -> usize {
        self.lock().cards.iter().filter(|c| c.state == State::Running).count()
    }

    /// How many running jobs are put away (for the "N running" chip).
    pub fn put_away(&self) -> usize {
        self.lock().cards.iter().filter(|c| c.state == State::Running && c.put_away).count()
    }

    /// The running jobs' titles, for the freeze watchdog. Never waits on the window.
    pub fn running_titles(&self) -> Vec<String> {
        self.lock().cards.iter().filter(|c| c.state == State::Running).map(|c| c.title.clone()).collect()
    }

    /// The × on a card: a running one is put away, a finished one goes.
    pub fn dismiss(&self, id: u64) {
        {
            let mut inner = self.lock();
            if let Some(i) = inner.cards.iter().position(|c| c.id == id) {
                if inner.cards[i].state == State::Running {
                    inner.cards[i].put_away = true;
                } else {
                    inner.cards.remove(i);
                }
            }
        }
        self.changed();
    }

    /// "N running": brings the put-away cards back.
    pub fn bring_back(&self) {
        for c in self.lock().cards.iter_mut() {
            c.put_away = false;
        }
        self.changed();
    }

    /// Forgets finished cards whose time is up. The window calls it on a timer.
    pub fn tidy(&self, now: Instant) -> bool {
        let mut inner = self.lock();
        let before = inner.cards.len();
        inner.cards.retain(|c| c.ended.is_none_or(|e| now.duration_since(e) < DONE_STAYS));
        before != inner.cards.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Tells the window, outside the lock, so its callback can read the cards.
    fn changed(&self) {
        let notify = self.lock().notify.clone();
        if let Some(f) = notify {
            f();
        }
    }

    fn update(&self, id: u64, f: impl FnOnce(&mut Card)) -> Option<Card> {
        let mut inner = self.lock();
        let card = inner.cards.iter_mut().find(|c| c.id == id)?;
        f(card);
        Some(card.clone())
    }
}

/// A running job's handle. Dropping it without `done` or `failed` counts as failed, so a job that panics or returns
/// early never leaves a card spinning forever.
pub struct Job {
    feed: Feed,
    id: u64,
    finished: bool,
}

impl Job {
    /// The card's id, as the window's × passes it back.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// How far it is, 0 to 1, with an optional new detail line. Passed on at most every 250 ms.
    pub fn progress(&self, fraction: f32, detail: Option<&str>) {
        let due = {
            let mut inner = self.feed.lock();
            let now = Instant::now();
            let due = now.duration_since(inner.last_progress) >= PROGRESS_EVERY;
            if due {
                inner.last_progress = now;
            }
            due
        };
        self.feed.update(self.id, |c| {
            c.progress = Some(fraction.clamp(0.0, 1.0));
            if let Some(d) = detail {
                c.detail = d.to_owned();
            }
        });
        if due {
            self.feed.changed();
        }
    }

    pub fn done(mut self, detail: &str) {
        self.end(State::Done, detail);
    }

    pub fn failed(mut self, why: &str) {
        self.end(State::Failed, why);
    }

    fn end(&mut self, state: State, detail: &str) {
        self.finished = true;
        let card = self.feed.update(self.id, |c| {
            c.state = state;
            c.ended = Some(Instant::now());
            c.put_away = false;
            c.progress = (state == State::Done).then_some(1.0);
            if !detail.is_empty() {
                c.detail = detail.to_owned();
            }
        });
        if let Some(c) = card {
            let took = c.ended.map(|e| e.duration_since(c.started)).unwrap_or_default();
            let took = if took >= Duration::from_millis(1) {
                format!(" after {:.1} s", took.as_secs_f32())
            } else {
                String::new()
            };
            let line = format!(
                "{}: {}{}{took}",
                if state == State::Done { "Done" } else { "Failed" },
                c.title,
                bracket(&c.detail)
            );
            if state == State::Failed {
                log::warning("Job", &line);
            } else {
                log::info("Job", &line);
            }
        }
        self.feed.changed();
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        if !self.finished {
            self.end(State::Failed, "It stopped before it finished.");
        }
    }
}

fn bracket(detail: &str) -> String {
    if detail.is_empty() { String::new() } else { format!(" ({detail})") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn a_job_runs_and_finishes() {
        let feed = Feed::default();
        let job = feed.start("Reading new FL saves", "");
        assert_eq!(feed.running(), 1);
        assert_eq!(feed.running_titles(), ["Reading new FL saves"]);
        job.progress(0.4, Some("212 of 540"));
        let now = Instant::now();
        assert_eq!(feed.visible(now)[0].progress, Some(0.4));
        assert_eq!(feed.visible(now)[0].detail, "212 of 540");
        job.done("540 saves read");
        assert_eq!(feed.running(), 0);
        let card = &feed.visible(Instant::now())[0];
        assert_eq!((card.state, card.progress), (State::Done, Some(1.0)));
        // A finished card goes by itself.
        assert!(feed.visible(Instant::now() + DONE_STAYS).is_empty());
        assert!(feed.tidy(Instant::now() + DONE_STAYS));
    }

    #[test]
    fn put_away_and_bring_back() {
        let feed = Feed::default();
        let job = feed.start("Analysing sounds", "");
        feed.dismiss(job.id);
        assert!(feed.visible(Instant::now()).is_empty());
        assert_eq!(feed.put_away(), 1);
        feed.bring_back();
        assert_eq!(feed.visible(Instant::now()).len(), 1);
        job.failed("The folder went away");
        // Ending brings a put-away card back, so a failure is never hidden.
        assert_eq!(feed.visible(Instant::now())[0].state, State::Failed);
    }

    #[test]
    fn a_dropped_job_fails_instead_of_spinning() {
        let feed = Feed::default();
        drop(feed.start("Scan", ""));
        assert_eq!(feed.running(), 0);
        assert_eq!(feed.visible(Instant::now())[0].state, State::Failed);
    }

    #[test]
    fn progress_is_passed_on_at_most_every_250_ms() {
        let feed = Feed::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        feed.on_change(move || {
            c.fetch_add(1, Ordering::Relaxed);
        });
        let job = feed.start("Many steps", "");
        let after_start = calls.load(Ordering::Relaxed);
        for i in 0..1000 {
            job.progress(i as f32 / 1000.0, None);
        }
        assert!(calls.load(Ordering::Relaxed) - after_start <= 1, "progress flooded the window");
        // The newest number is still kept, ready for the next redraw.
        assert_eq!(feed.visible(Instant::now())[0].progress, Some(0.999));
        job.done("");
    }
}
