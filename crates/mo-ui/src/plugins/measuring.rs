//! Measuring your plugins (review X1-2), on a low-priority thread with the disk work on the bulk pool (rules 12 and
//! 21): which are installed and how much room each takes ([`mo_app::plugins::measure`]), kept in the app's own
//! `songs.db` ([`mo_library::plugin_writes`]) and never in `library.db`. When the app looks at the plugins (this page's
//! read, or Space's look; at most once an hour) and the last measure is a day old or there's none; and whenever you
//! press Measure again. While `songs.db` isn't the app's own yet nothing is measured, and the page says so. Closing
//! the app stops a measure part way; nothing is kept then.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDateTime};
use mo_app::log;
use mo_app::plugins::measure::{self, Roots, due};
use mo_library::Library;
use slint::ComponentHandle;

use super::PAGE;
use super::clean::back;
use crate::{AppWindow, PluginsData};

/// A measure is running (only one at a time).
static RUNNING: AtomicBool = AtomicBool::new(false);
/// The app is closing: a measure stops part way.
static STOP: AtomicBool = AtomicBool::new(false);
/// The song library isn't the app's own yet, as the last look found: nothing can be measured into it.
static NOT_OWN: AtomicBool = AtomicBool::new(false);
/// When the app last looked whether a measure is due.
static LOOKED: Mutex<Option<Instant>> = Mutex::new(None);

/// How often the app looks whether a measure is due, at most.
const LOOK_EVERY: Duration = Duration::from_secs(60 * 60);

/// When the window has closed: a measure stops (nothing is kept from it).
pub(super) fn stop() {
    STOP.store(true, Ordering::Relaxed);
}

/// What the page says of measuring: while one runs, that it does; that it can't yet; else when sizes were last
/// measured.
pub(super) fn line(measured: Option<NaiveDateTime>) -> String {
    if RUNNING.load(Ordering::Relaxed) {
        return "Measuring your plugins…".to_owned();
    }
    if NOT_OWN.load(Ordering::Relaxed) {
        return "Plugins are measured once the app's own song library is in use (it's made at a start).".to_owned();
    }
    match measured {
        Some(t) => format!("Plugins measured {}.", t.format("%-d %b %Y, %H:%M")),
        None => "Your plugins aren't measured yet.".to_owned(),
    }
}

/// Measures when the last measure is due ([`due`]), looking at most once an hour: the oldest measure is read from the
/// library first, on the thread. Quick on the window's thread.
pub fn when_due(app: &AppWindow) {
    let look = {
        let mut looked = LOOKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let look = looked.is_none_or(|at| at.elapsed() >= LOOK_EVERY);
        if look {
            *looked = Some(Instant::now());
        }
        look
    };
    if look {
        start(app, false);
    }
}

/// Measures now, whenever the last measure was (Measure again).
pub(super) fn now(app: &AppWindow) {
    start(app, true);
}

fn start(app: &AppWindow, whatever: bool) {
    let (library, feed) = PAGE.with(|p| {
        let p = p.borrow();
        (p.library.clone(), p.feed.clone())
    });
    if library.as_os_str().is_empty() || RUNNING.swap(true, Ordering::Relaxed) {
        return;
    }
    shown(app);
    let weak = app.as_weak();
    let spawned = mo_app::background::spawn("Plugins measure", move || {
        let own = mo_library::songs_db::own_since(&library).is_some();
        NOT_OWN.store(!own, Ordering::Relaxed);
        let kept = if !own || (!whatever && !is_due(&library)) {
            false
        } else {
            let card = feed.start("Measuring your plugins", "Looking through your plugin folders");
            let found = mo_app::background::guarded(|| measure_and_keep(&library)).and_then(|r| r);
            match &found {
                Ok(text) => {
                    log::info("Plugins", &format!("Measured: {text}"));
                    card.done(text);
                }
                Err(why) => {
                    log::warning("Plugins", &format!("Plugins weren't measured: {why}"));
                    card.failed(&format!("Nothing changed: {why}"));
                }
            }
            found.is_ok()
        };
        RUNNING.store(false, Ordering::Relaxed);
        back(&weak, move |app| {
            shown(app);
            if kept {
                // This page reads again only once it has read (else its first show reads it); Space looks again.
                if PAGE.with(|p| p.borrow().plugins.is_some()) {
                    super::read_again();
                }
                crate::space::plugins_measured(app);
            }
        });
    });
    if let Err(e) = spawned {
        RUNNING.store(false, Ordering::Relaxed);
        log::error("Plugins", &format!("Couldn't start measuring your plugins: {e}"));
        shown(app);
    }
}

/// Whether the library's measure is due: the oldest measure in it is a day old, or there's none. One that can't be
/// read isn't measured (said in the log).
fn is_due(library: &Path) -> bool {
    match Library::open_read_only(library).and_then(|lib| lib.oldest_plugin_measure()) {
        Ok(oldest) => due(oldest, Local::now().naive_local()),
        Err(e) => {
            log::warning("Plugins", &format!("Couldn't read when your plugins were measured: {e}"));
            false
        }
    }
}

/// Measures your plugins and keeps what's found in place of the last measure; says what was found.
fn measure_and_keep(library: &Path) -> Result<String, String> {
    let now = Local::now().naive_local();
    let found = mo_app::background::on_bulk_pool(|| measure::measure(library, &Roots::system(), now, &STOP))?;
    if STOP.load(Ordering::Relaxed) {
        return Err("the app closed first".to_owned());
    }
    super::moving::kept_when_free(|| mo_library::plugin_writes::write(library, &found))?;
    Ok(measure::done_text(&found))
}

/// Puts the measure line and its button on the page.
pub(super) fn shown(app: &AppWindow) {
    let measured = PAGE.with(|p| p.borrow().plugins.as_ref().and_then(|pl| pl.measured));
    let data = app.global::<PluginsData>();
    data.set_measure_text(line(measured).into());
    data.set_can_measure(!RUNNING.load(Ordering::Relaxed) && !NOT_OWN.load(Ordering::Relaxed));
}
