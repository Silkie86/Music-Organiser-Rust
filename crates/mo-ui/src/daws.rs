//! The rail's "In your DAWs" (spec 3.1): what Bitwig has open now, asked on a low-priority thread of its own
//! (`mo_app::rail_daws::BitwigWatch`), so nothing here waits on the window's thread (rule 12). The window only gets a
//! line when it changes. A click shows the song on Songs, as the C# app did, or Settings' DAWs tab when there's none.

use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mo_app::places::Place;
use mo_app::rail_daws::{BitwigWatch, RailDaw, SLOW};
use mo_app::settings::Tab;
use mo_app::{background, log};
use mo_dawlink::bitwig::Bitwig;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{AppWindow, DawLine};

thread_local! {
    /// The lines on show, kept so a change only changes the lines that differ: a new list would take away the line
    /// under the mouse, and its tooltip's way of putting itself away with it.
    static LINES: Rc<VecModel<DawLine>> = Rc::new(VecModel::default());
    /// What each line on show stands for, for a click.
    static SHOWN: RefCell<Vec<RailDaw>> = const { RefCell::new(Vec::new()) };
}

/// Set as the app closes: the watch stops asking Bitwig.
static STOP: AtomicBool = AtomicBool::new(false);
/// The watch's thread, woken as the app closes.
static WATCH: OnceLock<std::thread::Thread> = OnceLock::new();

/// Wires the lines and starts the watch. Not for the screenshots, which never reach a real DAW (rule 11).
pub fn connect(app: &AppWindow) {
    wire(app);
    let weak = app.as_weak();
    let started = background::spawn("Rail DAW watch", move || {
        let mut watch = BitwigWatch::new(Bitwig::default());
        while !STOP.load(Ordering::Relaxed) {
            let wait = match mo_catch::catch_crash(AssertUnwindSafe(|| watch.ask())) {
                Ok(asked) => {
                    if let Some(line) = asked.line {
                        let weak = weak.clone();
                        // No event loop to take it: the window has gone for good.
                        if slint::invoke_from_event_loop(move || {
                            if let Some(app) = weak.upgrade() {
                                show(&app, &[line]);
                            }
                        })
                        .is_err()
                        {
                            return;
                        }
                    }
                    asked.wait
                }
                // A bug in an ask: logged as a crash, in full. The watch starts afresh after a while, so the line
                // says how Bitwig stands again rather than stay as it was.
                Err(_) => {
                    watch = BitwigWatch::new(Bitwig::default());
                    SLOW
                }
            };
            rest(wait);
        }
    });
    match started {
        Ok(watch) => {
            let _ = WATCH.set(watch.thread().clone());
        }
        Err(e) => log::warning("Rail", &format!("Couldn't start watching what Bitwig has open: {e}")),
    }
}

/// Waits `wait` before the next ask, or until the app closes.
fn rest(wait: Duration) {
    let until = Instant::now() + wait;
    while !STOP.load(Ordering::Relaxed) {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        std::thread::park_timeout(left);
    }
}

/// As the app closes: the watch stops asking Bitwig (it closes its link as it ends; nothing waits for it).
pub fn finish_up() {
    STOP.store(true, Ordering::Relaxed);
    if let Some(watch) = WATCH.get() {
        watch.unpark();
    }
}

/// A click on a line shows the song open in that DAW on Songs, when it's one of yours (the newest save with the name
/// the DAW shows, as the C# app found it); otherwise Settings on the DAWs tab, which says how the link stands.
pub fn wire(app: &AppWindow) {
    let weak = app.as_weak();
    app.on_daw_line_clicked(move |i| {
        let Some(app) = weak.upgrade() else { return };
        let line = SHOWN.with(|s| usize::try_from(i).ok().and_then(|i| s.borrow().get(i).cloned()));
        let save = line
            .filter(|l| l.daw == "Bitwig" && !l.project.is_empty())
            .and_then(|l| crate::songs::bitwig_save_named(&l.project));
        match save {
            Some(save) => {
                crate::go_to(&app, Place::Songs.index() as i32);
                crate::songs::show_save(&app, save);
            }
            None => crate::settings::open_on(&app, Tab::Daws),
        }
    });
}

/// Puts the lines under "In your DAWs" (the screenshots show made-up ones), changing only the lines that differ.
pub fn show(app: &AppWindow, lines: &[RailDaw]) {
    let model = LINES.with(Rc::clone);
    let line = |l: &RailDaw| DawLine {
        daw: l.daw.into(),
        text: l.text.as_str().into(),
        linked: l.linked,
        tip: l.tip.as_str().into(),
    };
    if model.row_count() == lines.len() {
        for (at, l) in lines.iter().enumerate() {
            let new = line(l);
            if model.row_data(at).as_ref() != Some(&new) {
                model.set_row_data(at, new);
            }
        }
    } else {
        model.set_vec(lines.iter().map(line).collect::<Vec<_>>());
    }
    SHOWN.with(|s| *s.borrow_mut() = lines.to_vec());
    app.set_daw_lines(ModelRc::from(model));
}
