//! Activity: every job reports through here: started, progress, finished or failed (spec 3.1, kept). The window shows
//! the running and just-finished jobs as lines in the rail, under WORKING ON, and a job that failed, or one done that
//! stays until it's read ([`Feed::told`]), as a card bottom right, to close with its × ([`Card::to_close`]).
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
/// How long a failed card stays: three times as long, as in the C# app, so there's time to read why (Send to Bitwig's
/// "Bitwig isn't answering" says what to check).
pub const FAILED_STAYS: Duration = Duration::from_secs(18);

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
    pub started: Instant,
    pub ended: Option<Instant>,
}

impl Card {
    /// Whether it's a card to read and close with its ×, shown where the × is: one that failed, or one done that
    /// never goes by itself ([`Feed::told`]). The rest are jobs running or just done, which go by themselves.
    pub fn to_close(&self) -> bool {
        self.state == State::Failed || (self.state == State::Done && self.ended.is_none())
    }

    /// Whether it finished long enough ago to go: [`DONE_STAYS`] after it was done, [`FAILED_STAYS`] after it failed.
    fn gone_by(&self, now: Instant) -> bool {
        let stays = if self.state == State::Failed { FAILED_STAYS } else { DONE_STAYS };
        self.ended.is_some_and(|e| now.duration_since(e) >= stays)
    }
}

type Notify = Box<dyn Fn() + Send + Sync>;

struct Inner {
    cards: Vec<Card>,
    notify: Option<Arc<Notify>>,
    /// One clock for every job, so the window redraws at most four times a second however many jobs run.
    last_progress: Instant,
    /// Progress came in too soon after the last redraw and hasn't been shown yet.
    unsent: bool,
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
                unsent: false,
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
            started: Instant::now(),
            ended: None,
        });
        log::info("Job", &format!("Started: {title}{}", bracket(detail)));
        self.changed();
        Job { feed: self.clone(), id, finished: false }
    }

    /// A card that isn't a job: a warning that stays until its × takes it away (it never times out, as a failed job's
    /// card does), for something true the whole run, such as the app using an old copy of its data.
    pub fn notice(&self, title: &str, detail: &str) {
        self.stays(title, detail, State::Failed);
        log::warning("Notice", &format!("{title}{}", bracket(detail)));
    }

    /// A card that isn't a job, for something done you'd want to have seen however long you were away (the old app's
    /// databases put away): it stays until its × takes it away, like [`Feed::notice`], but as a job done, not a
    /// warning.
    pub fn told(&self, title: &str, detail: &str) {
        self.stays(title, detail, State::Done);
        log::info("Notice", &format!("{title}{}", bracket(detail)));
    }

    /// A card with no end time, so it never goes by itself.
    fn stays(&self, title: &str, detail: &str, state: State) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.lock().cards.push(Card {
            id,
            title: title.to_owned(),
            detail: detail.to_owned(),
            progress: None,
            state,
            started: Instant::now(),
            ended: None,
        });
        self.changed();
    }

    /// The cards to show now, oldest first: finished ones past their time are left out.
    pub fn visible(&self, now: Instant) -> Vec<Card> {
        self.lock().cards.iter().filter(|c| !c.gone_by(now)).cloned().collect()
    }

    /// How many jobs are running.
    pub fn running(&self) -> usize {
        self.lock().cards.iter().filter(|c| c.state == State::Running).count()
    }

    /// The running jobs' titles, for the freeze watchdog. Never waits on the window.
    pub fn running_titles(&self) -> Vec<String> {
        self.lock().cards.iter().filter(|c| c.state == State::Running).map(|c| c.title.clone()).collect()
    }

    /// The × on a card: it goes. Only one that's finished has a × ([`Card::to_close`]); a job running is a line in
    /// the rail, which has none, so it stays (the whole-app review of 14 Nov, AP-10: the "N running" chip that
    /// brought put-away cards back went with #444).
    pub fn dismiss(&self, id: u64) {
        {
            let mut inner = self.lock();
            if let Some(i) = inner.cards.iter().position(|c| c.id == id && c.state != State::Running) {
                inner.cards.remove(i);
            }
        }
        self.changed();
    }

    /// Forgets finished cards whose time is up. The window calls it on a timer.
    pub fn tidy(&self, now: Instant) -> bool {
        let mut inner = self.lock();
        let before = inner.cards.len();
        inner.cards.retain(|c| !c.gone_by(now));
        before != inner.cards.len()
    }

    /// Passes on progress held back by the 250 ms limit. The window calls it on a timer, so the newest numbers always
    /// show even when a job goes quiet after them.
    pub fn send_held_progress(&self) {
        let held = std::mem::take(&mut self.lock().unsent);
        if held {
            self.changed();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Tells the window, outside the lock, so its callback can read the cards.
    fn changed(&self) {
        let notify = {
            let mut inner = self.lock();
            inner.unsent = false;
            inner.notify.clone()
        };
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
            } else {
                inner.unsent = true;
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

    /// Done, with a headline that says what came of it ("Kick is in Bitwig" where it said "Putting Kick into
    /// Bitwig").
    pub fn done_as(mut self, title: &str, detail: &str) {
        self.retitle(title);
        self.end(State::Done, detail);
    }

    /// Done, but with something to read and act on that a line gone in a few seconds would lose: the card stays
    /// until its × takes it away, as [`Feed::told`]'s does, and isn't a failure.
    pub fn done_to_read(mut self, title: &str, detail: &str) {
        self.retitle(title);
        self.end(State::Done, detail);
        self.feed.update(self.id, |c| c.ended = None);
        self.feed.changed();
    }

    /// Failed, with a headline that says what came of it.
    pub fn failed_as(mut self, title: &str, why: &str) {
        self.retitle(title);
        self.end(State::Failed, why);
    }

    fn retitle(&self, title: &str) {
        self.feed.update(self.id, |c| c.title = title.to_owned());
    }

    fn end(&mut self, state: State, detail: &str) {
        self.finished = true;
        let card = self.feed.update(self.id, |c| {
            c.state = state;
            c.ended = Some(Instant::now());
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
    fn a_running_job_stays_and_a_failure_shows() {
        let feed = Feed::default();
        let job = feed.start("Analysing sounds", "");
        // A running job has no × (review AP-10): it stays, so a failure is never hidden.
        feed.dismiss(job.id);
        assert_eq!(feed.visible(Instant::now()).len(), 1);
        assert_eq!(feed.running(), 1);
        job.failed("The folder went away");
        assert_eq!(feed.visible(Instant::now())[0].state, State::Failed);
        // And it stays longer than a card that's done, so there's time to read why.
        assert_eq!(feed.visible(Instant::now() + DONE_STAYS).len(), 1);
        assert!(feed.visible(Instant::now() + FAILED_STAYS).is_empty());
        assert!(feed.tidy(Instant::now() + FAILED_STAYS));
    }

    #[test]
    fn a_job_can_end_with_a_headline_that_says_how_it_went() {
        let feed = Feed::default();
        feed.start("Putting Kick into Bitwig", "On a Sampler, on a new track").done_as("Kick is in Bitwig", "");
        feed.start("Putting Snare into Bitwig", "").failed_as("Couldn't put it into Bitwig", "Bitwig isn't answering.");
        let cards = feed.visible(Instant::now());
        let shown: Vec<_> = cards.iter().map(|c| (c.title.as_str(), c.detail.as_str(), c.state)).collect();
        assert_eq!(
            shown,
            [
                ("Kick is in Bitwig", "On a Sampler, on a new track", State::Done),
                ("Couldn't put it into Bitwig", "Bitwig isn't answering.", State::Failed),
            ]
        );
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
        // The newest number is still kept, and the window's timer passes it on.
        assert_eq!(feed.visible(Instant::now())[0].progress, Some(0.999));
        let before = calls.load(Ordering::Relaxed);
        feed.send_held_progress();
        assert_eq!(calls.load(Ordering::Relaxed), before + 1);
        feed.send_held_progress();
        assert_eq!(calls.load(Ordering::Relaxed), before + 1, "only once");
        job.done("");
    }

    #[test]
    fn a_notice_stays_until_its_cross() {
        let feed = Feed::default();
        feed.notice("Using the old copy of your data", "The drive is missing");
        let later = Instant::now() + FAILED_STAYS * 100;
        assert_eq!(feed.visible(later).len(), 1);
        assert!(!feed.tidy(later));
        assert_eq!(feed.running(), 0);
        feed.dismiss(feed.visible(later)[0].id);
        assert!(feed.visible(later).is_empty());
    }

    // Review finding AP-8: a told card was shown as a line in the rail, which has no ×, so it stayed all session.
    #[test]
    fn only_what_stays_or_failed_is_a_card_to_close() {
        let feed = Feed::default();
        let running = feed.start("Reading songs", "");
        feed.start("Reading sounds", "").done("All read");
        feed.start("Backing up", "").failed("The drive is full");
        feed.told("Put the old app's databases away", "In Old databases");
        feed.notice("Using the old copy of your data", "The drive is missing");
        feed.start("Sending Keys", "").done_to_read("Keys is in Bitwig, but its bar isn't marked", "Why");
        let cards = feed.visible(Instant::now() + FAILED_STAYS);
        let kept = cards.iter().find(|c| c.title.starts_with("Keys")).unwrap();
        assert_eq!((kept.state, kept.to_close()), (State::Done, true), "done, and it stays to be read");
        let close: Vec<bool> = feed.visible(Instant::now()).iter().map(Card::to_close).collect();
        assert_eq!(close, [false, false, true, true, true, true]);
        drop(running);
    }
}
