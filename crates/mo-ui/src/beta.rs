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
use mo_app::self_check;
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
    /// Words typed in a step's box but not yet kept (by Enter or a mark), so they stay while the lines are drawn
    /// again (the 19 Nov review, BT-1).
    drafts: HashMap<&'static str, String>,
    to_try: bool,
    /// The self-check is running.
    checking: bool,
    message: String,
    model: Rc<VecModel<BetaLineInfo>>,
    feed: Feed,
}

thread_local! {
    static LIST: RefCell<Option<Checklist>> = const { RefCell::new(None) };
    static WRITER: RefCell<Option<Newest<(PathBuf, String)>>> = const { RefCell::new(None) };
    /// Looks each hour whether the daily self-check is due.
    static DAILY_TIMER: slint::Timer = slint::Timer::default();
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
    let done = steps.iter().filter(|s| marks.current(s).is_some()).count();
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
            drafts: HashMap::new(),
            to_try: false,
            checking: false,
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
    data.on_beta_draft(|id, words| {
        LIST.with(|l| {
            let mut l = l.borrow_mut();
            let Some(l) = l.as_mut() else { return };
            if let Some(step) = l.steps.iter().find(|s| s.id == id.as_str()) {
                l.drafts.insert(step.id, words.to_string());
            }
        });
    });
    let weak = app.as_weak();
    data.on_beta_toggle_to_try(move || {
        let Some(app) = weak.upgrade() else { return };
        LIST.with(|l| {
            if let Some(l) = l.borrow_mut().as_mut() {
                l.to_try = !l.to_try;
            }
        });
        // Only the lines that come or go change, so the rows that stay keep their words.
        redraw(&app, false);
    });
    let weak = app.as_weak();
    data.on_beta_self_check(move || {
        let Some(app) = weak.upgrade() else { return };
        run_self_check(&app, "asked for");
    });
    // Once a day by itself: a while after the start (out of its way), then each hour it looks whether a day has
    // gone by, for an app left open for days.
    if live {
        let weak = app.as_weak();
        slint::Timer::single_shot(SELF_CHECK_AFTER, move || {
            let Some(app) = weak.upgrade() else { return };
            self_check_if_due(&app);
            let weak = app.as_weak();
            DAILY_TIMER.with(|t| {
                t.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(3600), move || {
                    if let Some(app) = weak.upgrade() {
                        self_check_if_due(&app);
                    }
                })
            });
        });
    }
    data.on_beta_show_file(move || {
        let found = LIST.with(|l| l.borrow().as_ref().map(|l| (l.folder.clone(), l.feed.clone())));
        let Some((folder, feed)) = found else { return };
        // The folder when there's no file yet (looked at off the window's thread).
        let file = folder.join(beta::FILE);
        crate::files::reveal(&feed, &file.display().to_string(), &folder.display().to_string());
    });
}

/// How long after the start the daily self-check runs.
const SELF_CHECK_AFTER: std::time::Duration = std::time::Duration::from_secs(90);
/// The self-check runs by itself when it last ran at least this long ago (seconds): a little under a day, so it runs
/// at each day's first start.
const DAILY: u64 = 20 * 3600;

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Runs the self-check when it last ran a day ago or more (and marks can be kept).
fn self_check_if_due(app: &AppWindow) {
    let due = LIST.with(|l| {
        l.borrow().as_ref().is_some_and(|l| l.writable && now_secs().saturating_sub(l.marks.self_checked) >= DAILY)
    });
    if due {
        run_self_check(app, "daily");
    }
}

/// Runs the self-check off the window's thread, at low priority, then marks its steps.
fn run_self_check(app: &AppWindow, why: &'static str) {
    let go = LIST.with(|l| {
        let mut l = l.borrow_mut();
        let l = l.as_mut()?;
        if l.checking {
            return None;
        }
        l.checking = true;
        Some(())
    });
    if go.is_none() {
        return;
    }
    redraw(app, false);
    let data = mo_app::paths::data_dir();
    let logs = mo_app::paths::logs_dir();
    let weak = app.as_weak();
    log::info("Beta", &format!("Self-check started ({why})"));
    let started = mo_app::background::spawn("Self-check", move || {
        let now = std::time::SystemTime::now();
        let found = match self_check::read_logs(&logs, self_check::since(now)) {
            Ok(facts) => {
                for u in &facts.unread {
                    log::warning("Beta", &format!("Self-check couldn't read {u}"));
                }
                Ok(self_check::findings(&facts, &data, cfg!(windows)))
            }
            Err(e) => Err(format!("Couldn't read the logs folder {}: {e}", logs.display())),
        };
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(app) = weak.upgrade() {
                self_checked(&app, found);
            }
        });
    });
    if let Err(e) = started {
        log::error("Beta", &format!("Couldn't start the self-check: {e}"));
        LIST.with(|l| {
            if let Some(l) = l.borrow_mut().as_mut() {
                l.checking = false;
                l.message = format!("The self-check couldn't start: {e}");
            }
        });
        redraw(app, false);
    }
}

/// The self-check's findings: each marks its step (a Problem keeps the lines that show it), and the file is saved.
fn self_checked(app: &AppWindow, found: Result<Vec<self_check::Found>, String>) {
    let job = LIST.with(|l| {
        let mut l = l.borrow_mut();
        let l = l.as_mut()?;
        l.checking = false;
        let found = match found {
            Ok(f) => f,
            Err(e) => {
                log::warning("Beta", &format!("Self-check: {e}"));
                l.message = format!("The self-check couldn't run: {e}");
                return None;
            }
        };
        let version = mo_app::version::version();
        let log_file = log::current().and_then(|c| c.path());
        let when = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
        let problems = found.iter().filter(|f| f.mark == Mark::Problem).count();
        for f in found {
            let Some(step) = l.steps.iter().find(|s| s.id == f.id).cloned() else { continue };
            beta_log(f.mark == Mark::Problem, &beta::log_line(&step, Some(f.mark), &format!("Self-check: {}", f.note)));
            let moment = Moment { when: when.clone(), version: &version, recent: f.lines, log_file };
            l.marks.set(&step, Some(f.mark), &f.note, moment);
        }
        log::info("Beta", &format!("Self-check done: {problems} problems"));
        l.marks.self_checked = now_secs();
        l.unsaved |= l.writable;
        l.writable.then(|| (l.folder.clone(), l.marks.to_json()))
    });
    save(job);
    redraw(app, false);
}

/// A mark picked: the one it has takes it off.
fn marked(app: &AppWindow, id: &str, mark: i32, note: &str) {
    let mark = usize::try_from(mark).ok().and_then(|i| Mark::ALL.get(i).copied());
    let job = LIST.with(|l| {
        let mut l = l.borrow_mut();
        let l = l.as_mut()?;
        let step = l.steps.iter().find(|s| s.id == id)?.clone();
        // The mark as shown: a step reworded since its mark shows none, so a click marks it afresh.
        let had = l.marks.current(&step).map(|m| m.mark);
        let mark = if had == mark { None } else { mark };
        // The box shows the step's note (or words typed), so what's in it now is the note (an emptied box, no note).
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
        l.drafts.remove(step.id);
        l.unsaved |= l.writable;
        l.writable.then(|| (l.folder.clone(), l.marks.to_json()))
    });
    save(job);
    redraw(app, false);
}

/// A note kept with Enter: saved, on a marked step or not (an unmarked step's goes with its mark later).
fn noted(app: &AppWindow, id: &str, note: &str) {
    let job = LIST.with(|l| {
        let mut l = l.borrow_mut();
        let l = l.as_mut()?;
        let step = l.steps.iter().find(|s| s.id == id)?.clone();
        l.drafts.remove(step.id);
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

/// Whether `new` is `old` with some lines put in (Only to try turned off, or a self-check bringing a step back): then
/// where they go in `new`, first first.
fn put_in(old: &VecModel<BetaLineInfo>, new: &[BetaLineInfo]) -> Option<Vec<usize>> {
    let same = |a: &BetaLineInfo, b: &BetaLineInfo| a.id == b.id && a.area == b.area;
    let mut out = Vec::new();
    let mut i = 0;
    for (j, line) in new.iter().enumerate() {
        if i < old.row_count() && old.row_data(i).is_some_and(|row| same(&row, line)) {
            i += 1;
        } else {
            out.push(j);
        }
    }
    (i == old.row_count()).then_some(out)
}

/// Shows the lines again. With `whole` (the first time) the list is made anew; otherwise the lines that went (a mark
/// in Only to try, or turning it on) are taken out, the lines that came are put in, and each line that changed is
/// changed in place, so the card keeps its place and every row that stays keeps its words and the keys. A row that
/// had the keys gives them back to the window as it goes (its chip, or Only to try, says so: [`SettingsData`]'s
/// beta-keys-back), and the rare redraw that must make every row anew does the same, so a key always works after.
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
                note: l.drafts.get(line.id).cloned().unwrap_or(line.note).into(),
                auto: line.auto,
                said: line.said.into(),
            })
            .collect();
        let gone = if whole { None } else { taken_out(&l.model, &lines) };
        if let Some(gone) = gone {
            for i in gone {
                l.model.remove(i);
            }
        } else if !whole && let Some(come) = put_in(&l.model, &lines) {
            for j in come {
                l.model.insert(j, lines[j].clone());
            }
        }
        let data = app.global::<SettingsData>();
        if whole || lines.len() != l.model.row_count() {
            l.model.set_vec(lines);
            // Every row is made anew, the one whose box had the keys too: the window takes them, but only while the
            // card is in sight (a self-check ending on another page must leave a search box there alone).
            let shown = app.get_place() == mo_app::places::Place::Settings.index() as i32
                && data.get_tab() == mo_app::settings::Tab::Developer.index() as i32;
            if !whole && shown {
                data.set_beta_keys_back(data.get_beta_keys_back().wrapping_add(1));
            }
        } else {
            for (i, line) in lines.into_iter().enumerate() {
                if l.model.row_data(i).as_ref() != Some(&line) {
                    l.model.set_row_data(i, line);
                }
            }
        }
        data.set_beta_progress(beta::progress(&l.steps, &l.marks).into());
        data.set_beta_to_try(l.to_try);
        data.set_beta_checking(l.checking);
        data.set_beta_message(l.message.clone().into());
    });
}
