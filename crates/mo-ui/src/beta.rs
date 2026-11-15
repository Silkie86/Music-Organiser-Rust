//! Fills the beta checklist on Settings › Developer from `mo_app::beta` and keeps what's marked: in
//! `beta-checklist.json` in the data folder (written off the window's thread, newest first) and as a `[Beta]` line in
//! the session log. A Problem keeps the log's last lines with it, so Claude can see why it went wrong.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mo_app::activity::Feed;
use mo_app::background::Newest;
use mo_app::beta::{self, Mark, Marks, Moment, Step};
use mo_app::log;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::{AppWindow, BetaLineInfo, SettingsData};

struct Checklist {
    steps: Vec<Step>,
    marks: Marks,
    /// The data folder, where the file is.
    folder: PathBuf,
    /// Whether marks are written (not for the screenshots, nor when the file was there but couldn't be read: it's
    /// left as it is).
    writable: bool,
    /// Notes typed on a step not yet marked, kept until it is (and while the lines are drawn again).
    waiting: HashMap<&'static str, String>,
    to_try: bool,
    message: String,
    model: Rc<VecModel<BetaLineInfo>>,
    feed: Feed,
}

thread_local! {
    static LIST: RefCell<Option<Checklist>> = const { RefCell::new(None) };
    static WRITER: RefCell<Option<Newest<(PathBuf, String)>>> = const { RefCell::new(None) };
}

/// Reads the marks so far and answers the card. `live` is false for the screenshots: nothing is read or written.
pub fn connect(app: &AppWindow, folder: &Path, live: bool, feed: Feed) {
    let (marks, writable, message) = if live {
        match Marks::load(folder) {
            Ok(m) => (m, true, String::new()),
            Err(e) => {
                log::warning("Beta", &format!("Couldn't read {}: {e}", beta::FILE));
                (
                    Marks::default(),
                    false,
                    format!("Your marks in {} couldn't be read ({e}), so nothing is kept this run.", beta::FILE),
                )
            }
        }
    } else {
        (Marks::default(), false, String::new())
    };
    if live {
        match Newest::start("Save the beta checklist", |(folder, json): (PathBuf, String)| {
            if let Err(e) = Marks::save(&folder, &json) {
                log::error("Beta", &format!("Couldn't save {}: {e}", beta::FILE));
                let why = e.to_string();
                let _ = slint::invoke_from_event_loop(move || {
                    LIST.with(|l| {
                        if let Some(l) = l.borrow_mut().as_mut() {
                            l.message = format!("Your last mark couldn't be kept: {why}");
                        }
                    });
                });
            }
        }) {
            Ok(w) => WRITER.with(|slot| *slot.borrow_mut() = Some(w)),
            Err(e) => log::error("Beta", &format!("Couldn't start the checklist's writer: {e}")),
        }
    }
    let steps = beta::steps();
    let done = steps.iter().filter(|s| marks.get(s.id).is_some()).count();
    if live {
        log::info("Beta", &format!("Checklist: {} steps, {done} marked so far", steps.len()));
    }
    let model = Rc::new(VecModel::default());
    let data = app.global::<SettingsData>();
    data.set_beta_lines(ModelRc::from(model.clone()));
    LIST.with(|l| {
        *l.borrow_mut() = Some(Checklist {
            steps,
            marks,
            folder: folder.to_owned(),
            writable: live && writable,
            waiting: HashMap::new(),
            to_try: false,
            message,
            model,
            feed,
        })
    });
    redraw(app, true);

    let weak = app.as_weak();
    data.on_beta_mark(move |id, mark, note| {
        let Some(app) = weak.upgrade() else { return };
        marked(&app, &id, mark, &note);
    });
    let weak = app.as_weak();
    data.on_beta_note(move |id, note| {
        let Some(app) = weak.upgrade() else { return };
        noted(&app, &id, &note);
    });
    let weak = app.as_weak();
    data.on_beta_toggle_to_try(move || {
        let Some(app) = weak.upgrade() else { return };
        LIST.with(|l| {
            if let Some(l) = l.borrow_mut().as_mut() {
                l.to_try = !l.to_try;
            }
        });
        redraw(&app, true);
    });
    data.on_beta_show_file(move || {
        let found = LIST.with(|l| l.borrow().as_ref().map(|l| (l.folder.clone(), l.feed.clone())));
        let Some((folder, feed)) = found else { return };
        let file = folder.join(beta::FILE);
        if file.exists() {
            crate::files::reveal(&feed, &file.display().to_string(), &folder.display().to_string());
        } else {
            crate::files::open_folder(&feed, &folder.display().to_string());
        }
    });
}

/// A mark picked: the one it has takes it off.
fn marked(app: &AppWindow, id: &str, mark: i32, note: &str) {
    let mark = usize::try_from(mark).ok().and_then(|i| Mark::ALL.get(i).copied());
    let job = LIST.with(|l| {
        let mut l = l.borrow_mut();
        let l = l.as_mut()?;
        let step = l.steps.iter().find(|s| s.id == id)?.clone();
        let had = l.marks.get(step.id).map(|m| m.mark);
        let mark = if had == mark { None } else { mark };
        let note =
            if note.trim().is_empty() { l.waiting.get(step.id).cloned().unwrap_or_default() } else { note.into() };
        let version = mo_app::version::version();
        let current = log::current();
        let moment = Moment {
            when: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
            version: &version,
            recent: if mark == Some(Mark::Problem) {
                current.map(|c| c.recent()).unwrap_or_default()
            } else {
                Vec::new()
            },
            log_file: current.and_then(|c| c.path()),
        };
        // The log's line goes first, so a Problem's lines end just before it.
        let line = beta::log_line(&step, mark, &note);
        if mark == Some(Mark::Problem) {
            log::warning("Beta", &line)
        } else {
            log::info("Beta", &line)
        }
        l.marks.set(&step, mark, &note, moment);
        if mark.is_some() {
            l.waiting.remove(step.id);
        } else if !note.trim().is_empty() {
            l.waiting.insert(step.id, note);
        }
        l.writable.then(|| (l.folder.clone(), l.marks.to_json()))
    });
    save(job);
    redraw(app, false);
}

/// A note kept with Enter: on a marked step it's saved; on an unmarked one it waits for the mark.
fn noted(app: &AppWindow, id: &str, note: &str) {
    let job = LIST.with(|l| {
        let mut l = l.borrow_mut();
        let l = l.as_mut()?;
        let step = l.steps.iter().find(|s| s.id == id)?.clone();
        if l.marks.get(step.id).is_none() {
            l.waiting.insert(step.id, note.to_owned());
            return None;
        }
        if !l.marks.set_note(step.id, note) {
            return None;
        }
        log::info("Beta", &format!("{}: note changed: {}", step.id, note.trim()));
        l.writable.then(|| (l.folder.clone(), l.marks.to_json()))
    });
    save(job);
    redraw(app, false);
}

fn save(job: Option<(PathBuf, String)>) {
    if let Some(job) = job {
        WRITER.with(|w| {
            if let Some(w) = w.borrow().as_ref() {
                w.ask(job);
            }
        });
    }
}

/// Shows the lines again. With `whole` (or while only the steps to try show, where a mark takes a step away) the list
/// is made anew; otherwise each line that changed is changed in place, so the card keeps its place and the box you're
/// typing in keeps the keys.
fn redraw(app: &AppWindow, whole: bool) {
    LIST.with(|l| {
        let l = l.borrow();
        let Some(l) = l.as_ref() else { return };
        let lines: Vec<BetaLineInfo> = beta::lines(&l.steps, &l.marks, l.to_try)
            .into_iter()
            .map(|line| BetaLineInfo {
                area: line.area.into(),
                id: line.id.into(),
                what: line.what.into(),
                expect: line.expect.into(),
                mark: line.mark.and_then(|m| Mark::ALL.iter().position(|x| *x == m)).map_or(-1, |i| i as i32),
                note: if line.note.is_empty() {
                    l.waiting.get(line.id).cloned().unwrap_or_default().into()
                } else {
                    line.note.into()
                },
                said: line.said.into(),
            })
            .collect();
        if whole || l.to_try || lines.len() != l.model.row_count() {
            l.model.set_vec(lines);
        } else {
            for (i, line) in lines.into_iter().enumerate() {
                if l.model.row_data(i).as_ref() != Some(&line) {
                    l.model.set_row_data(i, line);
                }
            }
        }
        let data = app.global::<SettingsData>();
        data.set_beta_progress(beta::progress(&l.steps, &l.marks).into());
        data.set_beta_to_try(l.to_try);
        data.set_beta_message(l.message.clone().into());
    });
}
