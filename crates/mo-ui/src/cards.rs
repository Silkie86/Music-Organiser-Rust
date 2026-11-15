//! Shows the activity feed's cards in the window. Jobs change the feed on any thread; the window is only touched on
//! its own thread, and only one redraw is ever waiting, however fast the jobs report (rule 12).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mo_app::activity::{Card, Feed, State};
use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};

use crate::{AppWindow, Busy, CardInfo};

/// Ties the window's jobs and cards to `feed`: redraws when it changes, passes the ×s back, and lets finished ones go
/// when their time is up.
pub fn connect(app: &AppWindow, feed: &Feed) {
    let waiting = Arc::new(AtomicBool::new(false));
    let weak = app.as_weak();
    let f = feed.clone();
    feed.on_change(move || {
        // A redraw is already on its way and will read the newest cards.
        if waiting.swap(true, Ordering::AcqRel) {
            return;
        }
        let f = f.clone();
        let w = waiting.clone();
        let sent = weak.upgrade_in_event_loop(move |app| {
            w.store(false, Ordering::Release);
            refresh(&app, &f);
        });
        if sent.is_err() {
            // The window has closed (or there is no event loop, as in mo-screens): nothing to redraw.
            waiting.store(false, Ordering::Release);
        }
    });

    let f = feed.clone();
    app.on_dismiss_card(move |id| f.dismiss(id as u64));
    let f = feed.clone();
    app.on_try_job(move || try_job(&f));

    // Finished cards stay a few seconds, then go by themselves; and progress held back by the 250 ms limit is shown.
    let weak = app.as_weak();
    let f = feed.clone();
    let tidy = Timer::default();
    tidy.start(TimerMode::Repeated, mo_app::activity::PROGRESS_EVERY, move || {
        if f.tidy(Instant::now())
            && let Some(app) = weak.upgrade()
        {
            refresh(&app, &f);
        }
        f.send_held_progress();
    });
    // The timer lives as long as the app: it stops when dropped, so it's let go of on purpose.
    std::mem::forget(tidy);

    refresh(app, feed);
}

/// How many jobs the rail lists; the rest are counted under them ("and 2 more").
const RAIL_JOBS: usize = 4;

/// Puts the feed's jobs on screen now: the running and just-finished ones as lines in the rail, the ones that failed
/// as cards (the UX review of 10 Nov, A3), and so the ones done that stay until closed (review finding AP-8: the
/// rail's lines have no ×). Call on the window's thread. When the same jobs are on screen, only their
/// rows change, so the lines aren't rebuilt and their meters glide.
pub fn refresh(app: &AppWindow, feed: &Feed) {
    let visible = feed.visible(Instant::now());
    let (cards, jobs): (Vec<&Card>, Vec<&Card>) = visible.iter().partition(|c| c.to_close());
    let cards: Vec<CardInfo> = cards.into_iter().map(card_info).collect();
    let mut jobs: Vec<CardInfo> = jobs.into_iter().map(card_info).collect();
    // The newest are at the end: they are the ones kept when there are too many for the rail.
    let more = jobs.len().saturating_sub(RAIL_JOBS);
    jobs.drain(..more);
    let unsure = cards.iter().chain(&jobs).any(|c| c.state == 0 && c.progress < 0.0);
    sweep(app, unsure);
    show(&app.get_cards(), cards, |m| app.set_cards(m));
    show(&app.get_jobs(), jobs, |m| app.set_jobs(m));
    app.set_more_jobs(more as i32);
}

/// How often the bar of a job that can't say how far it is moves on. Each move draws the whole window again, so it
/// steps eight times a second instead of gliding with every frame: in Solomon's log of 15 Nov 2026 the window was
/// drawn 50 times a second, 11 ms each, on a page where nothing else moved, for as long as the start-up jobs ran.
const SWEEP_EVERY: Duration = Duration::from_millis(125);
/// The bar goes across in this long, and back in as long again.
const SWEEP_ACROSS: Duration = Duration::from_millis(900);

thread_local! {
    /// Moves the bar on; runs only while a job that can't say how far it is shows.
    static SWEEP: Timer = Timer::default();
}

/// Where the bar stands `since` it set off: 0 at the left, 1 at the right, easing at each end.
fn sweep_at(since: Duration) -> f32 {
    let turn = since.as_secs_f32() / SWEEP_ACROSS.as_secs_f32() * std::f32::consts::PI;
    0.5 + 0.5 * turn.sin()
}

/// Starts the bar moving when a job that can't say how far it is shows (`unsure`), and stops it when none does, so
/// nothing asks for a frame once the jobs are done.
fn sweep(app: &AppWindow, unsure: bool) {
    SWEEP.with(|timer| {
        if !unsure {
            timer.stop();
        } else if !timer.running() {
            let weak = app.as_weak();
            let began = Instant::now();
            timer.start(TimerMode::Repeated, SWEEP_EVERY, move || {
                if let Some(app) = weak.upgrade() {
                    app.global::<Busy>().set_sweep(sweep_at(began.elapsed()));
                }
            });
        }
    });
}

/// Makes `shown` say `now`, changing only the rows that differ when the same jobs are there.
fn show(shown: &ModelRc<CardInfo>, now: Vec<CardInfo>, set: impl FnOnce(ModelRc<CardInfo>)) {
    let same = shown.row_count() == now.len()
        && now.iter().enumerate().all(|(i, c)| shown.row_data(i).is_some_and(|s| s.id == c.id));
    match shown.as_any().downcast_ref::<VecModel<CardInfo>>() {
        Some(model) if same => {
            for (i, c) in now.into_iter().enumerate() {
                if model.row_data(i).as_ref() != Some(&c) {
                    model.set_row_data(i, c);
                }
            }
        }
        Some(model) => model.set_vec(now),
        None => set(ModelRc::new(VecModel::from(now))),
    }
}

fn card_info(c: &Card) -> CardInfo {
    CardInfo {
        id: c.id as i32,
        title: c.title.as_str().into(),
        detail: c.detail.as_str().into(),
        progress: c.progress.unwrap_or(-1.0),
        state: match c.state {
            State::Running => 0,
            State::Done => 1,
            State::Failed => 2,
        },
    }
}

/// Settings' "Try an activity card": a pretend job of about four seconds that changes nothing, so a job's line in the
/// rail can be seen before any real job exists. It can't say how far it is for its first second, like a real scan
/// starting up.
fn try_job(feed: &Feed) {
    let job = feed.start("Trying an activity card", "Getting ready");
    let spawned = mo_app::background::spawn("Pretend job", move || {
        std::thread::sleep(Duration::from_secs(1));
        const STEPS: u32 = 30;
        for i in 1..=STEPS {
            std::thread::sleep(Duration::from_millis(100));
            job.progress(i as f32 / STEPS as f32, Some(&format!("Step {i} of {STEPS}")));
        }
        job.done("Finished. Nothing was changed.");
    });
    if let Err(e) = spawned {
        mo_app::log::warning("Job", &format!("Couldn't start the pretend job: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unsure_bar_goes_across_and_back_in_steps_not_with_every_frame() {
        // From the middle to the right, back through the middle to the left, and to the middle again.
        let at = |parts: u32| sweep_at(SWEEP_ACROSS * parts / 2);
        assert!((at(0) - 0.5).abs() < 1e-3);
        assert!((at(1) - 1.0).abs() < 1e-3);
        assert!((at(2) - 0.5).abs() < 1e-3);
        assert!((at(3) - 0.0).abs() < 1e-3);
        assert!((at(4) - 0.5).abs() < 1e-3);
        // Never outside its groove, however long a job runs.
        assert!((0..10_000).all(|i| (0.0..=1.0).contains(&sweep_at(SWEEP_EVERY * i))));
        // A few moves a second: enough to see it's working, a fraction of a frame a second each.
        assert!(SWEEP_EVERY >= Duration::from_millis(100) && SWEEP_EVERY <= Duration::from_millis(250));
    }
}
