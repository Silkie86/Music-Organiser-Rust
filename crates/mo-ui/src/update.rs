//! The window's side of updates (`mo_app::update`): the daily look a while after start, the fetch as a job card, the
//! rail's "Restart to update" line, and Settings › General's switch. The setup itself runs once the app has closed
//! and written everything as it always does ([`asked`] and [`run`], the very end of main).

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Mutex;
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

/// The fetched setup to run once the app has closed (the rail's "Restart to update").
static RUN_AT_EXIT: Mutex<Option<PathBuf>> = Mutex::new(None);

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
            // Turned off: the rail stops offering what was fetched.
            if !on {
                app.set_update_ready("".into());
                app.set_update_setup("".into());
            }
        }
    });
    let weak = app.as_weak();
    app.on_restart_to_update(move || {
        let Some(app) = weak.upgrade() else { return };
        let setup = app.get_update_setup().to_string();
        if setup.is_empty() {
            return;
        }
        log::info("Update", &format!("Restarting to update with {setup}"));
        if let Ok(mut s) = RUN_AT_EXIT.lock() {
            *s = Some(PathBuf::from(setup));
        }
        // Closes as the window's own close does, so everything waiting is written first.
        let _ = slint::quit_event_loop();
    });
    if !live || !installed() {
        return;
    }
    let weak = app.as_weak();
    slint::Timer::single_shot(FIRST_LOOK_AFTER, move || {
        let Some(app) = weak.upgrade() else { return };
        look_if_due(&app, &feed);
        let weak = app.as_weak();
        TIMER.with(|t| {
            t.start(slint::TimerMode::Repeated, LOOK_EVERY, move || {
                if let Some(app) = weak.upgrade() {
                    look_if_due(&app, &feed);
                }
            })
        });
    });
}

/// What Settings says under the switch.
fn note() -> &'static str {
    if installed() {
        "Once a day the app asks GitHub whether a new version is out. That's all it sends online. A new version is \
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

/// Once a day GitHub is asked, and a newer setup fetched; between looks (or offline), a setup fetched on an earlier
/// day and checked again is offered.
fn look_if_due(app: &AppWindow, feed: &Feed) {
    let wanted = update::wanted(crate::settings::value(CHECK_KEY).as_deref());
    let due = update::due(crate::settings::value(CHECKED_KEY).as_deref(), now_secs());
    let busy = BUSY.with(|b| *b.borrow());
    if !wanted || busy || !app.get_update_ready().is_empty() {
        return;
    }
    BUSY.with(|b| *b.borrow_mut() = true);
    if due {
        crate::settings::remember(CHECKED_KEY, &now_secs().to_string());
    }
    let folder = mo_app::paths::data_dir().join(update::FOLDER);
    let current = env!("CARGO_PKG_VERSION").to_owned();
    let feed = feed.clone();
    let answer = Answer { found: None, window: app.as_weak() };
    let started = mo_app::background::spawn("Update look", move || {
        // Tells the window when it's dropped, so a panic in here still frees the next look.
        let mut answer = answer;
        update::clear_old(&folder, &current);
        if !due {
            answer.found = update::ready_in(&folder, &current);
            return;
        }
        let offer = match update::look(update::GITHUB_API, &current) {
            Ok(Some(offer)) => offer,
            // Nothing newer listed: a setup fetched earlier and since taken off the Releases page isn't offered.
            Ok(None) => {
                log::info("Update", &format!("No newer version than {current}"));
                return;
            }
            // Offline, or GitHub not answering: a setup fetched before still is, and GitHub is asked again tomorrow.
            Err(e) => {
                log::info("Update", &format!("Couldn't ask GitHub for new versions: {e}"));
                answer.found = update::ready_in(&folder, &current);
                return;
            }
        };
        let job = feed.start(&format!("Getting {} {}", mo_app::product::NAME, offer.version), "Fetching the setup");
        match update::fetch_setup(&offer, &folder) {
            Ok(setup) => {
                log::info("Update", &format!("{} is ready: {}", offer.version, setup.display()));
                job.done_as(&format!("{} {} is ready", mo_app::product::NAME, offer.version), "Restart to update");
                answer.found = Some((offer.version, setup));
            }
            Err(e) => {
                log::warning("Update", &format!("Couldn't fetch {}: {e}", offer.version));
                job.failed(&e);
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
    window: slint::Weak<AppWindow>,
}

impl Drop for Answer {
    fn drop(&mut self) {
        let found = self.found.take();
        let window = self.window.clone();
        let _ = slint::invoke_from_event_loop(move || {
            BUSY.with(|b| *b.borrow_mut() = false);
            let (Some((version, setup)), Some(app)) = (found, window.upgrade()) else { return };
            // Turned off while it looked: nothing offered.
            if !update::wanted(crate::settings::value(CHECK_KEY).as_deref()) {
                return;
            }
            app.set_update_ready(version.into());
            app.set_update_setup(setup.display().to_string().into());
        });
    }
}

/// Once the window has closed and everything is written, before the log closes: whether "Restart to update" was
/// clicked, and its setup is there to run (logged when it isn't).
pub fn asked() -> Option<PathBuf> {
    let setup = RUN_AT_EXIT.lock().ok().and_then(|mut s| s.take())?;
    if setup.is_file() {
        log::info("Update", &format!("The setup {} starts once the log is closed", setup.display()));
        Some(setup)
    } else {
        log::last(log::Level::Error, "Update", &format!("The setup {} isn't there any more", setup.display()));
        None
    }
}

/// Last of all, after the log has closed: starts the setup [`asked`] gave. It closes what's left of the app and puts
/// the new version in, so nothing may still be writing. Where it can't start, the next start looks again.
pub fn run(setup: &std::path::Path) {
    if let Err(e) = update::run_setup(setup) {
        eprintln!("Couldn't start the setup {}: {e}", setup.display());
    }
}
