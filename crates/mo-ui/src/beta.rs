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
    /// A change not yet saved by the writer, for the last save as the app closes.
    unsaved: bool,
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

/// One save at a time: the writer thread and the last save as the app closes ([`finish_up`]) both write through the
/// same `beta-checklist.json.mo-part`. Holding it, the close sets the `bool` to true, and the writer then skips any
/// save it hadn't started (an older one than the close's own).
static SAVING: std::sync::Mutex<bool> = std::sync::Mutex::new(false);

/// Counts this run's `[Beta]` lines, so the same mark said again isn't taken for a repeat and only counted.
static SAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn beta_log(problem: bool, line: &str) {
    let n = SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let line = format!("#{n} {line}");
    if problem { log::warning("Beta", &line) } else { log::info("Beta", &line) }
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
    let (mut writable, mut message) = (live && writable, message);
    if writable {
        let weak = app.as_weak();
        match Newest::start("Save the beta checklist", move |(folder, json): (PathBuf, String)| {
            let saved = {
                let closed = SAVING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if *closed {
                    return;
                }
                Marks::save(&folder, &json)
            };
            if let Err(e) = &saved {
                log::error("Beta", &format!("Couldn't save {}: {e}", beta::FILE));
            }
            let said = saved.err().map(|e| format!("Your last mark couldn't be kept: {e}"));
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                LIST.with(|l| {
                    if let Some(l) = l.borrow_mut().as_mut() {
                        match said {
                            Some(said) => l.message = said,
                            // A save that worked: an earlier failure is mended, and nothing waits to be kept.
                            None => {
                                if l.message.starts_with("Your last mark") {
                                    l.message.clear();
                                }
                                if l.marks.to_json() == json {
                                    l.unsaved = false;
                                }
                            }
                        }
                    }
                });
                if let Some(app) = weak.upgrade() {
                    redraw(&app, false);
                }
            });
        }) {
            Ok(w) => WRITER.with(|slot| *slot.borrow_mut() = Some(w)),
            Err(e) => {
                log::error("Beta", &format!("Couldn't start the checklist's writer: {e}"));
                writable = false;
                message = format!("Your marks can't be kept this run: the app couldn't start saving them ({e}).");
            }
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
            writable,
            unsaved: false,
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
        // The folder when there's no file yet (looked at off the window's thread).
        let file = folder.join(beta::FILE);
        crate::files::reveal(&feed, &file.display().to_string(), &folder.display().to_string());
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
        // The box shows a waiting note, so what's in it now is the note (an emptied box, no note).
        let note = note.to_owned();
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
        beta_log(mark == Some(Mark::Problem), &beta::log_line(&step, mark, &note));
        l.marks.set(&step, mark, &note, moment);
        if mark.is_some() || note.trim().is_empty() {
            l.waiting.remove(step.id);
        } else {
            l.waiting.insert(step.id, note);
        }
        l.unsaved |= l.writable;
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
        beta_log(false, &format!("{}: note changed: {}", step.id, note.trim()));
        l.unsaved |= l.writable;
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

/// As the app closes: a mark the writer hasn't saved yet is saved now, so the last one isn't lost.
pub fn finish_up() {
    let job = LIST.with(|l| {
        let l = l.borrow();
        let l = l.as_ref()?;
        (l.unsaved && l.writable).then(|| (l.folder.clone(), l.marks.to_json()))
    });
    // Waits for a save the writer is part way through, and stops it starting another.
    let mut closed = SAVING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *closed = true;
    if let Some((folder, json)) = job
        && let Err(e) = Marks::save(&folder, &json)
    {
        log::error("Beta", &format!("Couldn't save {} as the app closed: {e}", beta::FILE));
    }
}

/// Whether `new` is `old` with some lines taken out (a mark took a step out of Only to try, and maybe its area's
/// heading): then the rows to take out, last first.
fn taken_out(old: &VecModel<BetaLineInfo>, new: &[BetaLineInfo]) -> Option<Vec<usize>> {
    let same = |a: &BetaLineInfo, b: &BetaLineInfo| a.id == b.id && a.area == b.area;
    let mut out = Vec::new();
    let mut j = 0;
    for i in 0..old.row_count() {
        let row = old.row_data(i)?;
        if j < new.len() && same(&row, &new[j]) {
            j += 1;
        } else {
            out.push(i);
        }
    }
    (j == new.len()).then(|| {
        out.reverse();
        out
    })
}

/// Shows the lines again. With `whole` the list is made anew; otherwise the lines a mark took out of Only to try are
/// taken out and each line that changed is changed in place, so the card keeps its place, the box you're typing in
/// keeps the keys, and words typed in other boxes stay.
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
        let gone = if whole { None } else { taken_out(&l.model, &lines) };
        if let Some(gone) = gone {
            for i in gone {
                l.model.remove(i);
            }
        }
        if whole || lines.len() != l.model.row_count() {
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
