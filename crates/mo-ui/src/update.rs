//! The window's side of updates (`mo_app::update`): the daily look a while after start, the fetch as a job card, the
//! rail's "Restart to update" line, and Settings › General's switch. The click checks the setup and starts it, and
//! the app then closes as it always does: the setup waits for it to end before it touches a file (`/WAITPID`), so a
//! setup that can't start is told on the spot, and the app stays open (UP-1).

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

use mo_app::activity::Feed;
use mo_app::log;
use mo_app::update::{self, CHECK_KEY, CHECKED_KEY};
use slint::ComponentHandle;

use crate::{AppWindow, SettingsData};

/// How long after the start the first look is: out of the start-up work's way.
const FIRST_LOOK_AFTER: Duration = Duration::from_secs(180);

/// Then every hour it looks whether a day has gone by, for an app left open for days.
const LOOK_EVERY: Duration = Duration::from_secs(3600);

thread_local! {
    static TIMER: slint::Timer = slint::Timer::default();
    /// A look or fetch is going.
    static BUSY: RefCell<bool> = const { RefCell::new(false) };
}

/// Sets up the switch and the rail's line, and the daily look for an installed copy. `live` is false for the
/// screenshots: nothing is looked up.
pub fn connect(app: &AppWindow, feed: Feed, live: bool) {
    let data = app.global::<SettingsData>();
    data.set_update_check(update::wanted(crate::settings::value(CHECK_KEY).as_deref()));
    data.set_update_note(note().into());
    let weak = app.as_weak();
    data.on_toggle_update_check(move || {
        let on = !update::wanted(crate::settings::value(CHECK_KEY).as_deref());
        crate::settings::remember(CHECK_KEY, if on { "1" } else { "0" });
        log::info(
            "Update",
            if on { "Daily look for new versions turned on" } else { "Daily look for new versions turned off" },
        );
        if let Some(app) = weak.upgrade() {
            app.global::<SettingsData>().set_update_check(on);
            // Turned off: a fetch going stops, the rail stops offering what was fetched, and the fetched setups go
            // (UP-4).
            if !on {
                app.set_update_ready("".into());
                app.set_update_setup("".into());
                update::stop_fetches();
                let _ = mo_app::background::spawn("Update clear", || {
                    let folder = mo_app::paths::data_dir().join(update::FOLDER);
                    update::clear_unlisted(&folder, env!("CARGO_PKG_VERSION"), None);
                });
            }
        }
    });
    let weak = app.as_weak();
    let feed_for_click = feed.clone();
    app.on_restart_to_update(move || {
        let Some(app) = weak.upgrade() else { return };
        let setup = app.get_update_setup().to_string();
        if setup.is_empty() {
            return;
        }
        // Asked once: a second click while the setup is checked does nothing.
        app.set_update_setup("".into());
        let weak = app.as_weak();
        let feed = feed_for_click.clone();
        let kept = setup.clone();
        let started = mo_app::background::spawn("Update start", move || {
            let setup = PathBuf::from(setup);
            let outcome = if !update::still_whole(&setup) {
                // Not the setup that was checked: it goes, and the next look fetches it again.
                let _ = std::fs::remove_file(&setup);
                Err("the fetched setup isn't whole any more, so it's fetched again on the next look".to_owned())
            } else {
                update::run_setup(&setup, std::process::id()).map_err(|e| format!("Windows couldn't start it: {e}"))
            };
            let _ = slint::invoke_from_event_loop(move || match outcome {
                Ok(()) => {
                    log::info("Update", &format!("Started {}: it waits for the app to close", setup.display()));
                    // Closes as the window's own close does, so everything waiting is written first. A copy started
                    // meanwhile waits for this one to end.
                    crate::one_copy::closing();
                    let _ = slint::quit_event_loop();
                }
                Err(why) => {
                    log::warning("Update", &format!("Couldn't start the setup {}: {why}", setup.display()));
                    feed.notice("The update couldn't start", &format!("{why}. The app stays as it is."));
                    if let Some(app) = weak.upgrade() {
                        app.set_update_ready("".into());
                    }
                }
            });
        });
        // No thread to start it on: the line stays, to be clicked again.
        if started.is_err() {
            app.set_update_setup(kept.into());
        }
    });
    if !live || !installed() {
        return;
    }
    // The last "Restart to update" ran its setup, and this is still the old version: told once (UP-6).
    let told = feed.clone();
    let _ = mo_app::background::spawn("Update result", move || {
        let folder = mo_app::paths::data_dir().join(update::FOLDER);
        if let Some((version, tail)) = update::failed_update(&folder, env!("CARGO_PKG_VERSION")) {
            log::warning("Update", &format!("The update to {version} didn't go in. The setup's last lines:\n{tail}"));
            told.notice(
                &format!("The update to {version} didn't go in"),
                "The app is still the version it was. Restart to update is offered again; the log has the setup's \
                 own words.",
            );
        }
    });
    let weak = app.as_weak();
    slint::Timer::single_shot(FIRST_LOOK_AFTER, move || {
        let Some(app) = weak.upgrade() else { return };
        // GitHub is asked at every start, however long since the last look (UP-9).
        look_if_due(&app, &feed, true);
        let weak = app.as_weak();
        TIMER.with(|t| {
            t.start(slint::TimerMode::Repeated, LOOK_EVERY, move || {
                if let Some(app) = weak.upgrade() {
                    look_if_due(&app, &feed, false);
                }
            })
        });
    });
}

/// What Settings says under the switch.
fn note() -> &'static str {
    if installed() {
        "At each start, and once a day while it's open, the app asks GitHub whether a new version is out. That's all \
         it sends online. A new version is \
         fetched in the background and checked, and the rail offers Restart to update: nothing changes until you click."
    } else {
        "This copy wasn't put in by the installer (the launcher's builds, say), so it never updates itself."
    }
}

fn installed() -> bool {
    std::env::current_exe().is_ok_and(|e| update::installed(&e))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// GitHub is asked at the start (`first`) and then once a day, and a newer setup fetched; between looks (or offline),
/// a setup fetched on an earlier day and checked again is offered. Whenever GitHub answers, what it no longer lists
/// is neither kept nor offered (UP-3).
fn look_if_due(app: &AppWindow, feed: &Feed, first: bool) {
    let wanted = update::wanted(crate::settings::value(CHECK_KEY).as_deref());
    let due = first || update::due(crate::settings::value(CHECKED_KEY).as_deref(), now_secs());
    let busy = BUSY.with(|b| *b.borrow());
    // An offer showing is looked at again only when GitHub is asked: it may have been taken back.
    if !wanted || busy || (!due && !app.get_update_ready().is_empty()) {
        return;
    }
    BUSY.with(|b| *b.borrow_mut() = true);
    let folder = mo_app::paths::data_dir().join(update::FOLDER);
    let current = env!("CARGO_PKG_VERSION").to_owned();
    let feed = feed.clone();
    let answer = Answer { found: None, reached: false, again_soon: false, window: app.as_weak() };
    let started = mo_app::background::spawn("Update look", move || {
        // Tells the window when it's dropped, so a panic in here still frees the next look.
        let mut answer = answer;
        update::clear_old(&folder, &current);
        if !due {
            answer.found = update::ready_in(&folder, &current);
            return;
        }
        let offer = match update::look(update::GITHUB_API, &current) {
            Ok(offer) => offer,
            // Offline, or GitHub not answering: a setup fetched before still is offered, and GitHub is asked again
            // tomorrow (or at the next start).
            Err(e) => {
                log::info("Update", &format!("Couldn't ask GitHub for new versions: {e}"));
                answer.found = update::ready_in(&folder, &current);
                return;
            }
        };
        answer.reached = true;
        update::clear_unlisted(&folder, &current, offer.as_ref().map(|o| o.version.as_str()));
        let Some(offer) = offer else {
            // Nothing newer listed: a setup fetched earlier and since taken off the Releases page is gone.
            log::info("Update", &format!("No newer version than {current}"));
            return;
        };
        // Fetched already (an earlier day, or the offer showing now): offered again with no card.
        if let Some(ready) = update::ready_in(&folder, &current).filter(|(v, _)| *v == offer.version) {
            answer.found = Some(ready);
            return;
        }
        let name = mo_app::product::NAME;
        let job = feed.start(&format!("Getting {name} {}", offer.version), "Fetching the setup");
        match update::fetch_setup(&offer, &folder) {
            Ok(setup) => {
                log::info("Update", &format!("{} is ready: {}", offer.version, setup.display()));
                job.done_as(&format!("{name} {} is ready", offer.version), "Restart to update");
                answer.found = Some((offer.version, setup));
            }
            Err(e) if e == update::STOPPED => {
                log::info("Update", &format!("Stopped fetching {}", offer.version));
                job.done("Stopped");
            }
            // curl's own words for the log; plain words and what happens next on the card (UP-10).
            Err(e) => {
                log::warning("Update", &format!("Couldn't fetch {}: {e}", offer.version));
                answer.again_soon = true;
                job.failed_as(
                    &format!("Couldn't fetch {name} {}", offer.version),
                    "The connection dropped, or the download didn't arrive whole. Nothing has changed: the app tries \
                     again within the hour.",
                );
            }
        }
    });
    if started.is_err() {
        BUSY.with(|b| *b.borrow_mut() = false);
    }
}

/// What a look found, given to the window when the look ends, however it ends.
struct Answer {
    found: Option<(String, PathBuf)>,
    /// GitHub answered: the look counts, and an offer it no longer lists goes.
    reached: bool,
    /// The fetch failed: the next hourly tick tries again.
    again_soon: bool,
    window: slint::Weak<AppWindow>,
}

impl Drop for Answer {
    fn drop(&mut self) {
        let found = self.found.take();
        let (reached, again_soon) = (self.reached, self.again_soon);
        let window = self.window.clone();
        let _ = slint::invoke_from_event_loop(move || {
            BUSY.with(|b| *b.borrow_mut() = false);
            // Only a look that reached GitHub counts as the day's look (UP-9); a failed fetch is tried again soon
            // (UP-10).
            if again_soon {
                crate::settings::remember(CHECKED_KEY, "");
            } else if reached {
                crate::settings::remember(CHECKED_KEY, &now_secs().to_string());
            }
            let Some(app) = window.upgrade() else { return };
            // Turned off while it looked: nothing offered.
            if !update::wanted(crate::settings::value(CHECK_KEY).as_deref()) {
                return;
            }
            match found {
                Some((version, setup)) => {
                    app.set_update_ready(version.into());
                    app.set_update_setup(setup.display().to_string().into());
                }
                // GitHub no longer lists what was offered (UP-3).
                None if reached => {
                    app.set_update_ready("".into());
                    app.set_update_setup("".into());
                }
                None => {}
            }
        });
    }
}
