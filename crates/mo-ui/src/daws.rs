//! The rail's "In your DAWs" (spec 3.1): what Bitwig has open now, from a link to it kept open on a low-priority thread
//! and asked again every few seconds. The link doesn't note these asks in its reports, so they cost nothing, and nothing
//! here waits on the window's thread (rule 12). The rail said "Nothing open" whatever was open before (the whole-app
//! review of 6 Nov, X3-9). FL Studio gets its line once its link is built (phase 5); until then the rail says nothing
//! about it rather than something untrue.

use std::time::Duration;

use mo_app::places::Place;
use mo_app::settings::{RailDaw, Tab, bitwig_rail};
use mo_app::{background, log};
use mo_dawlink::bitwig::{Bitwig, Error, Hello, Link};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{AppWindow, DawLine};

/// How long the watch waits between asks.
const EVERY: Duration = Duration::from_secs(5);

/// Wires the lines (a click opens Settings on the DAWs tab, which says more) and starts the watch. Not for the
/// screenshots, which never reach a real DAW (rule 11).
pub fn connect(app: &AppWindow) {
    wire(app);
    let weak = app.as_weak();
    let started = background::spawn("Rail DAW watch", move || {
        let bitwig = Bitwig::default();
        let mut link: Option<Link> = None;
        let mut said: Option<RailDaw> = None;
        loop {
            let line = bitwig_rail(&ask(&bitwig, &mut link));
            if said.as_ref() != Some(&line) {
                said = Some(line.clone());
                let weak = weak.clone();
                // The window has closed when its event loop has gone: the watch ends with it.
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
            std::thread::sleep(EVERY);
        }
    });
    if let Err(e) = started {
        log::warning("Rail", &format!("Couldn't start watching what Bitwig has open: {e}"));
    }
}

/// A click on a line opens Settings on the DAWs tab.
pub fn wire(app: &AppWindow) {
    let weak = app.as_weak();
    app.on_daw_line_clicked(move |_| {
        if let Some(app) = weak.upgrade() {
            crate::go_to(&app, Place::Settings.index() as i32);
            crate::settings::show_tab(&app, Tab::Daws);
        }
    });
}

/// What Bitwig has open now: the kept link asked again, or a new one when there's none (Bitwig was closed, or has
/// closed it since). A link that stops answering is let go, to connect afresh next time.
fn ask(bitwig: &Bitwig, link: &mut Option<Link>) -> Result<Hello, Error> {
    if let Some(open) = link.as_mut().filter(|l| l.is_open()) {
        match open.refresh() {
            Ok(hello) => return Ok(hello.clone()),
            Err(_) => *link = None,
        }
    }
    let connected = bitwig.connect()?;
    let hello = connected.hello().clone();
    *link = Some(connected);
    Ok(hello)
}

/// Puts the lines under "In your DAWs" (the screenshots show made-up ones).
pub fn show(app: &AppWindow, lines: &[RailDaw]) {
    let lines: Vec<DawLine> = lines
        .iter()
        .map(|l| DawLine {
            daw: l.daw.into(),
            text: l.text.as_str().into(),
            linked: l.linked,
            tip: l.tip.as_str().into(),
        })
        .collect();
    app.set_daw_lines(ModelRc::new(VecModel::from(lines)));
}
