//! Shows the activity feed's cards in the window. Jobs change the feed on any thread; the window is only touched on
//! its own thread, and only one redraw is ever waiting, however fast the jobs report (rule 12).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mo_app::activity::{Card, Feed, State};
use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};

use crate::{AppWindow, CardInfo};

/// Ties the window's cards to `feed`: redraws when it changes, passes the ×s and "N running" back, and lets finished
/// cards go when their time is up.
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
    app.on_bring_back_cards(move || f.bring_back());
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

/// Puts the feed's cards on screen now. Call on the window's thread. When the same cards are on screen, only their
/// rows change, so the cards aren't rebuilt and their progress bars glide.
pub fn refresh(app: &AppWindow, feed: &Feed) {
    let cards: Vec<CardInfo> = feed.visible(Instant::now()).iter().map(card_info).collect();
    let shown = app.get_cards();
    let same_cards = shown.row_count() == cards.len()
        && cards.iter().enumerate().all(|(i, c)| shown.row_data(i).is_some_and(|s| s.id == c.id));
    match shown.as_any().downcast_ref::<VecModel<CardInfo>>() {
        Some(model) if same_cards => {
            for (i, c) in cards.into_iter().enumerate() {
                if model.row_data(i).as_ref() != Some(&c) {
                    model.set_row_data(i, c);
                }
            }
        }
        Some(model) => model.set_vec(cards),
        None => app.set_cards(ModelRc::new(VecModel::from(cards))),
    }
    app.set_put_away(feed.put_away() as i32);
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

/// Settings' "Try an activity card": a pretend job of about four seconds that changes nothing, so the cards can be
/// seen before any real job exists. It can't say how far it is for its first second, like a real scan starting up.
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
