//! Fills the Backup page from `mo_app::backup` and answers its buttons. Everything slow (walking the folders,
//! reading the library, the backup itself, reading and removing older versions) runs on low-priority threads (rule
//! 21); the window only ever shows what they send back (rule 12). Working out what goes is redone after every change,
//! newest ask first; the drive's older versions are read again when the drive changes and after each run or removal.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Local;
use mo_app::background::{self, Newest};
use mo_app::backup::{
    self, BackedUp, DriveRoom, PageLines, PlanState, Prepared, Progress, Settings, level_at, library_folders,
    page_lines,
};
use mo_app::backup_older::{self, KeptState};
use mo_app::log;
use mo_backup::folders::Mode;
use mo_backup::older::{self, Kept, Run};
use mo_library::Library;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use crate::{AppWindow, BackupData, BackupFolderLine, LeftOutLine, MenuItem};

/// What the page knows, on the window's thread.
#[derive(Default)]
struct Section {
    settings: Settings,
    settings_file: PathBuf,
    /// When each song last reached the drive, and its file.
    backed: BackedUp,
    backed_file: PathBuf,
    library: PathBuf,
    /// The drive's room, and the drive it was measured on.
    room: Option<(PathBuf, DriveRoom)>,
    prepared: Option<Arc<Prepared>>,
    failed: Option<String>,
    /// Counts the asks, so a plan made for an older folder list is dropped.
    asked: u64,
    running: Option<Arc<AtomicBool>>,
    /// Stops the plan being worked out when a newer one is asked for.
    plan_stop: Arc<AtomicBool>,
    /// The page has been shown, so its folders are walked (not before: nobody may open it).
    shown: bool,
    /// The older versions on the drive as last read, and the drive they were read from; or why they couldn't be.
    kept: Option<(PathBuf, Result<Arc<Kept>, String>)>,
    /// Counts the reads of the older versions, so one for a drive picked before is dropped.
    older_asked: u64,
    /// Stops the read of the older versions when a newer one is asked for.
    older_stop: Arc<AtomicBool>,
    /// The question asked before older versions go: the drive, and exactly what goes on a yes.
    older_ask: Option<(PathBuf, backup_older::Ask)>,
    /// What the last removal did.
    older_done: String,
    /// The removal of older versions running now: what stops it.
    removing: Option<Arc<AtomicBool>>,
}

/// A job for the planner: the ask's number, the settings then, the library's file, and what stops it.
type Job = (u64, Settings, PathBuf, Arc<AtomicBool>);

/// A job for the older versions' reader: the ask's number, the drive, and what stops it.
type OlderJob = (u64, PathBuf, Arc<AtomicBool>);

thread_local! {
    static SECTION: RefCell<Section> = RefCell::default();
    static PLANNER: RefCell<Option<Newest<Job>>> = const { RefCell::new(None) };
    static OLDER: RefCell<Option<Newest<OlderJob>>> = const { RefCell::new(None) };
}

pub fn connect(app: &AppWindow, library: PathBuf) {
    let settings_file = mo_app::paths::data_dir().join(backup::SETTINGS_FILE);
    let settings = Settings::load(&settings_file);
    let backed_file = mo_app::paths::data_dir().join(backup::SONGS_FILE);
    let backed = BackedUp::load(&backed_file);
    SECTION.with(|s| {
        let mut s = s.borrow_mut();
        s.settings = settings;
        s.settings_file = settings_file;
        s.backed = backed;
        s.backed_file = backed_file;
        s.library = library;
    });
    let weak = app.as_weak();
    let planner = Newest::start("Backup plan", move |(asked, settings, library, stop): Job| {
        let room = settings.drive.as_ref().and_then(|d| {
            mo_space::app_files::drive_space(d).map(|r| (d.clone(), DriveRoom { free: r.free, total: r.total }))
        });
        // Nothing to walk yet: only the drive's room is wanted.
        let result = (!settings.folders.is_empty()).then(|| {
            background::on_pool(|| {
                let lib = Library::open_read_only(&library).map_err(|e| e.to_string())?;
                backup::prepare(&lib, &settings, &stop)
            })
        });
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            SECTION.with(|s| {
                let mut s = s.borrow_mut();
                if s.asked != asked {
                    return;
                }
                s.room = room;
                match result {
                    Some(Ok(p)) => {
                        s.prepared = Some(Arc::new(p));
                        s.failed = None;
                    }
                    Some(Err(e)) => {
                        s.prepared = None;
                        s.failed = Some(e);
                    }
                    None => {}
                }
            });
            show(&weak);
        });
    });
    match planner {
        Ok(p) => PLANNER.with(|slot| *slot.borrow_mut() = Some(p)),
        Err(e) => log::error("Backup", &format!("Couldn't start the backup planner: {e}")),
    }
    // The older versions on the drive, and the drive's room (which a removal changes).
    let weak = app.as_weak();
    let reader = Newest::start("Backup older versions", move |(asked, drive, stop): OlderJob| {
        let room = mo_space::app_files::drive_space(&drive).map(|r| DriveRoom { free: r.free, total: r.total });
        let read = background::on_pool(|| older::read(&drive, &stop));
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            SECTION.with(|s| {
                let mut s = s.borrow_mut();
                if s.older_asked != asked {
                    return;
                }
                if let Some(room) = room {
                    s.room = Some((drive.clone(), room));
                }
                s.kept = Some((drive, read.map(Arc::new).map_err(|e| kept_error(&e))));
            });
            show(&weak);
        });
    });
    match reader {
        Ok(r) => OLDER.with(|slot| *slot.borrow_mut() = Some(r)),
        Err(e) => log::error("Backup", &format!("Couldn't start reading the older versions: {e}")),
    }

    let data = app.global::<BackupData>();
    let weak = app.as_weak();
    data.on_level_picked(move |i| change(&weak, |s| s.level = level_at(usize::try_from(i).unwrap_or(1))));
    let weak = app.as_weak();
    data.on_format_picked(move |i| {
        // What goes doesn't depend on the format: kept and shown, not worked out again.
        let format = if i == 1 { backup::Format::Zips } else { backup::Format::Folders };
        keep(|s| s.format = format);
        show(&weak);
    });
    let weak = app.as_weak();
    data.on_mode_picked(move |row, m| {
        let mode = if m == 0 { Mode::Whole } else { Mode::Slimmed };
        change(&weak, |s| s.set_mode(usize::try_from(row).unwrap_or(usize::MAX), mode));
    });
    let weak = app.as_weak();
    data.on_remove_folder(move |row| change(&weak, |s| s.remove_folder(usize::try_from(row).unwrap_or(usize::MAX))));
    let weak = app.as_weak();
    data.on_add_folder(move || {
        let weak = weak.clone();
        pick_folder("Pick a folder to back up", move |path| change(&weak, |s| s.add_folder(path)));
    });
    let weak = app.as_weak();
    data.on_pick_drive(move || {
        let weak = weak.clone();
        pick_folder("Pick the backup drive (or a folder on it)", move |path| {
            // A question asked about the old drive's older versions isn't for this one.
            SECTION.with(|s| {
                let mut s = s.borrow_mut();
                s.older_ask = None;
                s.older_done.clear();
            });
            change(&weak, |s| s.drive = Some(path));
            reread_older();
        });
    });
    let weak = app.as_weak();
    data.on_add_library_folders(move || {
        let library = SECTION.with(|s| s.borrow().library.clone());
        let weak = weak.clone();
        let started = background::spawn("Backup folders", move || {
            let found = Library::open_read_only(&library).and_then(|lib| library_folders(&lib));
            let _ = slint::invoke_from_event_loop(move || match found {
                Ok(list) => change(&weak, |s| list.into_iter().for_each(|p| s.add_folder(p))),
                Err(e) => log::warning("Backup", &format!("Couldn't read your folders from the library: {e}")),
            });
        });
        if let Err(e) = started {
            log::error("Backup", &format!("Couldn't start reading your folders: {e}"));
        }
    });
    let weak = app.as_weak();
    data.on_run(move || start_run(&weak));
    data.on_stop(|| {
        SECTION.with(|s| {
            let s = s.borrow();
            for stop in [&s.running, &s.removing].into_iter().flatten() {
                stop.store(true, Ordering::Relaxed);
            }
        });
    });
    let weak = app.as_weak();
    data.on_older_picked(move |id| {
        let now = Local::now().naive_local();
        SECTION.with(|s| {
            let mut s = s.borrow_mut();
            if s.running.is_some() || s.removing.is_some() {
                return;
            }
            let Some(drive) = s.settings.drive.clone() else { return };
            let ask = match &s.kept {
                Some((d, Ok(kept))) if *d == drive => backup_older::ask(kept, &id, now),
                _ => None,
            };
            s.older_ask = ask.map(|a| (drive, a));
            s.older_done.clear();
        });
        show(&weak);
    });
    let weak = app.as_weak();
    data.on_older_cancel(move || {
        SECTION.with(|s| s.borrow_mut().older_ask = None);
        show(&weak);
    });
    let weak = app.as_weak();
    data.on_older_remove(move || start_removal(&weak));
    show(&app.as_weak());
    // The app opened on the page.
    if app.get_place() == mo_app::places::Place::Backup.index() as i32 {
        shown();
    }
}

/// The page is on show: the first time, its folders are walked to work out what goes, and the drive's older versions
/// are read. They're read again each time it's shown after a read that failed (the drive may be plugged in now).
pub fn shown() {
    let (first, failed) = SECTION.with(|s| {
        let mut s = s.borrow_mut();
        (!std::mem::replace(&mut s.shown, true), matches!(s.kept, Some((_, Err(_)))))
    });
    if first {
        replan();
    }
    if first || failed {
        reread_older();
    }
}

/// Asks for the drive's older versions (and its room) to be read again.
fn reread_older() {
    let job = SECTION.with(|s| {
        let mut s = s.borrow_mut();
        s.older_asked += 1;
        // A read still going is for a drive picked before, or is out of date: it stops.
        s.older_stop.store(true, Ordering::Relaxed);
        s.older_stop = Arc::new(AtomicBool::new(false));
        s.settings.drive.clone().map(|d| (s.older_asked, d, s.older_stop.clone()))
    });
    if let Some(job) = job {
        OLDER.with(|r| {
            if let Some(r) = r.borrow().as_ref() {
                r.ask(job);
            }
        });
    }
}

/// Why the older versions couldn't be read, as the card says it.
fn kept_error(e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        "The backup drive isn't plugged in, so its older versions can't be seen.".to_owned()
    } else {
        format!("Couldn't read the older versions on the backup drive: {e}")
    }
}

/// Opens the system's folder picker without holding up the window, and hands on the folder picked.
fn pick_folder(title: &'static str, then: impl FnOnce(PathBuf) + 'static) {
    let asked = slint::spawn_local(async move {
        if let Some(folder) = rfd::AsyncFileDialog::new().set_title(title).pick_folder().await {
            then(folder.path().to_path_buf());
        }
    });
    if let Err(e) = asked {
        log::error("Backup", &format!("Couldn't open the folder picker: {e}"));
    }
}

/// Changes the settings and keeps them. A backup already running goes on with the settings it started with.
fn keep(edit: impl FnOnce(&mut Settings)) {
    let saved = SECTION.with(|s| {
        let mut s = s.borrow_mut();
        edit(&mut s.settings);
        s.settings.save(&s.settings_file)
    });
    if let Err(e) = saved {
        log::warning("Backup", &format!("Couldn't keep the backup settings: {e}"));
    }
}

/// Changes the settings, keeps them, and works out what goes again.
fn change(weak: &Weak<AppWindow>, edit: impl FnOnce(&mut Settings)) {
    keep(edit);
    replan();
    show(weak);
}

/// Asks the planner to work out what goes for the settings as they are now.
fn replan() {
    let job = SECTION.with(|s| {
        let mut s = s.borrow_mut();
        s.asked += 1;
        s.prepared = None;
        s.failed = None;
        // The plan being worked out now is for older choices: it stops.
        s.plan_stop.store(true, Ordering::Relaxed);
        s.plan_stop = Arc::new(AtomicBool::new(false));
        s.settings.drive.is_some().then(|| (s.asked, s.settings.clone(), s.library.clone(), s.plan_stop.clone()))
    });
    if let Some(job) = job {
        PLANNER.with(|p| {
            if let Some(p) = p.borrow().as_ref() {
                p.ask(job);
            }
        });
    }
}

fn start_run(weak: &Weak<AppWindow>) {
    let Some((prepared, settings, stop)) = SECTION.with(|s| {
        let mut s = s.borrow_mut();
        // Not while older versions are being removed: one job on the drive at a time.
        if s.running.is_some() || s.removing.is_some() {
            return None;
        }
        let prepared = s.prepared.clone()?;
        let stop = Arc::new(AtomicBool::new(false));
        s.running = Some(stop.clone());
        // The run changes the drive: a question about its older versions would be out of date.
        s.older_ask = None;
        Some((prepared, s.settings.clone(), stop))
    }) else {
        return;
    };
    set_busy(
        weak,
        &Progress::Copying { files: prepared.plan.copy.len(), bytes: prepared.plan.copy_bytes() }.line(),
        0.0,
    );
    // The question about older versions goes from the page at once.
    show(weak);
    let run_weak = weak.clone();
    let started = background::spawn("Backup", move || {
        let weak = run_weak;
        let progress_weak = weak.clone();
        let progress = move |p: Progress| {
            let weak = progress_weak.clone();
            let _ = slint::invoke_from_event_loop(move || set_busy(&weak, &p.line(), p.done()));
        };
        let ran = background::on_pool(|| backup::run(&prepared, &settings, &stop, &progress));
        let drive = settings.drive.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let last = ran.last;
            if !last.ok {
                log::warning("Backup", &last.line);
            }
            let (saved, backed_saved, reread) = SECTION.with(|s| {
                let mut s = s.borrow_mut();
                s.running = None;
                // The older versions as read are still right unless the run kept more, or they weren't read from
                // this drive (or couldn't be).
                let read = matches!(&s.kept, Some((d, Ok(_))) if Some(d) == s.settings.drive.as_ref());
                let reread = ran.kept_older > 0 || !read;
                // The drive the run went to, which may not be the one picked now.
                let backed_saved = match &drive {
                    Some(d) if !ran.went.is_empty() => {
                        s.backed.add(d, &ran.went, last.when);
                        s.backed.save(&s.backed_file)
                    }
                    _ => Ok(()),
                };
                s.settings.last_run = Some(last);
                (s.settings.save(&s.settings_file), backed_saved, reread)
            });
            if let Err(e) = saved {
                log::warning("Backup", &format!("Couldn't keep the backup settings: {e}"));
            }
            if let Err(e) = backed_saved {
                log::warning("Backup", &format!("Couldn't keep which songs are backed up: {e}"));
            }
            set_busy(&weak, "", 0.0);
            // The drive's room and what's new have changed (the planner measures the room again).
            replan();
            if reread {
                reread_older();
            }
            show(&weak);
        });
    });
    if let Err(e) = started {
        log::error("Backup", &format!("Couldn't start the backup: {e}"));
        SECTION.with(|s| s.borrow_mut().running = None);
        set_busy(weak, "", 0.0);
        show(weak);
    }
}

/// The most removals the log lists one by one.
const LOGGED_LEFT: usize = 20;

/// Yes to the question: removes exactly what it named, on a low-priority thread, then reads the older versions again.
fn start_removal(weak: &Weak<AppWindow>) {
    let Some((drive, ask, stop)) = SECTION.with(|s| {
        let mut s = s.borrow_mut();
        if s.running.is_some() || s.removing.is_some() {
            return None;
        }
        let (drive, ask) = s.older_ask.take()?;
        // Only on the drive it was asked about, which is still the one picked.
        if s.settings.drive.as_ref() != Some(&drive) {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        s.removing = Some(stop.clone());
        s.older_done.clear();
        Some((drive, ask, stop))
    }) else {
        show(weak);
        return;
    };
    let total: usize = ask.runs().iter().map(|r| r.versions.len()).sum();
    set_busy(weak, &backup_older::removing_line(0, total), 0.0);
    show(weak);
    let run_weak = weak.clone();
    let started = background::spawn("Backup remove older versions", move || {
        let weak = run_weak;
        let progress_weak = weak.clone();
        // The window hears each hundredth of the way, not every file.
        let heard = Cell::new(usize::MAX);
        let progress = move |done: usize, of: usize| {
            let step = done.saturating_mul(100) / of.max(1);
            if heard.replace(step) != step {
                let weak = progress_weak.clone();
                let part = done as f32 / of.max(1) as f32;
                let _ = slint::invoke_from_event_loop(move || {
                    set_busy(&weak, &backup_older::removing_line(done, of), part);
                });
            }
        };
        let removed = older::remove(&drive, &ask.runs(), &stop, &progress);
        let line = match &removed {
            Ok(r) => {
                for (path, why) in r.left.iter().take(LOGGED_LEFT) {
                    log::warning("Backup", &format!("Kept an older version: {}: {why}", path.display()));
                }
                if r.left.len() > LOGGED_LEFT {
                    log::warning("Backup", &format!("…and {} more kept", r.left.len() - LOGGED_LEFT));
                }
                backup_older::removed_line(r)
            }
            Err(_) => "The backup drive isn't there, so nothing was removed".to_owned(),
        };
        log::info("Backup", &format!("Older versions on {}: {line}", drive.display()));
        let _ = slint::invoke_from_event_loop(move || {
            SECTION.with(|s| {
                let mut s = s.borrow_mut();
                s.removing = None;
                s.older_done = line;
                // The listing from before is out of date: the card says it's looking until the new one is read, so
                // nothing in it can be asked about again.
                s.kept = None;
            });
            set_busy(&weak, "", 0.0);
            reread_older();
            show(&weak);
        });
    });
    if let Err(e) = started {
        log::error("Backup", &format!("Couldn't start removing older versions: {e}"));
        SECTION.with(|s| s.borrow_mut().removing = None);
        set_busy(weak, "", 0.0);
        show(weak);
    }
}

fn set_busy(weak: &Weak<AppWindow>, text: &str, done: f32) {
    if let Some(app) = weak.upgrade() {
        let data = app.global::<BackupData>();
        data.set_busy(SharedString::from(text));
        data.set_done(done);
    }
}

/// Puts the words on the page.
fn show(weak: &Weak<AppWindow>) {
    let Some(app) = weak.upgrade() else { return };
    let now = Local::now().naive_local();
    let (lines, older): (PageLines, OlderCard) = SECTION.with(|s| {
        let s = s.borrow();
        let state = match (&s.prepared, &s.failed) {
            (Some(p), _) => PlanState::Ready(p),
            (None, Some(why)) => PlanState::Failed(why),
            _ if s.settings.drive.is_none() || s.settings.folders.is_empty() => PlanState::Waiting,
            _ => PlanState::Working,
        };
        // The room of the drive picked now, not of one picked before.
        let room = s.room.as_ref().filter(|(d, _)| Some(d) == s.settings.drive.as_ref()).map(|(_, r)| *r);
        // And its older versions, not another drive's.
        let kept = match (&s.kept, &s.settings.drive) {
            (Some((d, Ok(kept))), Some(drive)) if d == drive => KeptState::Ready(kept),
            (Some((d, Err(why))), Some(drive)) if d == drive => KeptState::Failed(why),
            _ => KeptState::Waiting,
        };
        let older = OlderCard {
            lines: backup_older::lines(s.settings.drive.is_some(), kept, now),
            ask: s.older_ask.as_ref().map(|(_, a)| a.question.clone()).unwrap_or_default(),
            done: s.older_done.clone(),
        };
        (page_lines(&s.settings, room, state, now), older)
    });
    show_lines(&app, lines);
    show_older(&app, older);
    // The song panel's "Backed up" line follows the drive and the last run.
    crate::songs::backup_changed(&app);
}

/// What the Older versions card shows: its words, the question being asked (or none) and the last removal's line.
pub struct OlderCard {
    pub lines: backup_older::Lines,
    pub ask: String,
    pub done: String,
}

/// Puts the Older versions card's words on the page.
pub fn show_older(app: &AppWindow, card: OlderCard) {
    let data = app.global::<BackupData>();
    data.set_older_size(card.lines.size.into());
    data.set_older_count(card.lines.count.into());
    data.set_older_note(card.lines.note.into());
    let choices: Vec<MenuItem> = card
        .lines
        .choices
        .into_iter()
        .map(|c| MenuItem { what: c.what.into(), key: SharedString::new(), id: c.id.into(), enabled: c.enabled })
        .collect();
    data.set_older_choices(ModelRc::new(VecModel::from(choices)));
    data.set_older_ask(card.ask.into());
    data.set_older_done(card.done.into());
}

/// The song panel's "Backed up" line for a song (empty until a backup drive is picked).
pub fn song_line(song: &mo_library::songs::Song, newest: chrono::NaiveDateTime) -> String {
    SECTION.with(|s| {
        let s = s.borrow();
        s.backed.song_line(s.settings.drive.as_deref(), backup::song_id(song), newest, Local::now().naive_local())
    })
}

/// Puts a set of words on the page.
pub fn show_lines(app: &AppWindow, lines: PageLines) {
    let data = app.global::<BackupData>();
    data.set_drive(lines.drive.into());
    data.set_drive_fill(lines.drive_fill);
    data.set_drive_line(lines.drive_line.into());
    data.set_last_line(lines.last_line.into());
    data.set_level(lines.level as i32);
    data.set_level_note(lines.level_note.into());
    data.set_format(lines.format as i32);
    data.set_format_note(lines.format_note.into());
    let folders: Vec<BackupFolderLine> = lines
        .folders
        .into_iter()
        .map(|f| BackupFolderLine { path: f.path.into(), size: f.size.into(), mode: f.mode as i32 })
        .collect();
    data.set_folders(ModelRc::new(VecModel::from(folders)));
    data.set_goes(lines.goes.into());
    data.set_of_all(lines.of_all.into());
    data.set_goes_part(lines.goes_part);
    let left: Vec<LeftOutLine> = lines
        .left_out
        .into_iter()
        .map(|(why, note, size)| LeftOutLine { why: why.into(), note: note.into(), size: size.into() })
        .collect();
    data.set_left_out(ModelRc::new(VecModel::from(left)));
    data.set_plan_note(lines.plan_note.into());
}

/// Made-up words for the screenshots: a drive, five folders (one unplugged) and a worked-out plan.
pub fn sample_lines() -> PageLines {
    use mo_app::backup::LastRun;
    use mo_backup::folders::Folder;
    use mo_backup::plan::{LeftOut, Plan, Skipped};
    use mo_space::size::FileEntry;
    const GB: u64 = 1 << 30;
    let now = Local::now().naive_local();
    let folder = |p: &str, mode| Folder { path: PathBuf::from(p), mode };
    let settings = Settings {
        drive: Some(PathBuf::from(r"E:\Music Backup")),
        folders: vec![
            folder(r"I:\FL Studio Data\Projects", Mode::Slimmed),
            folder(r"I:\Samples", Mode::Slimmed),
            folder(r"K:\Bitwig Studio\Projects", Mode::Slimmed),
            folder(r"K:\Bitwig Studio\Library", Mode::Whole),
            folder(r"F:\Old Projects 2019", Mode::Slimmed),
        ],
        last_run: Some(LastRun {
            when: now - chrono::Duration::days(1),
            ok: true,
            line: "3,881 checked · 12 new or changed".to_owned(),
        }),
        ..Settings::default()
    };
    let file = |size| FileEntry { path: PathBuf::from("x"), size, modified: now };
    let skipped = |size, why| Skipped { file: file(size), why };
    let prepared = Prepared {
        plan: Plan {
            copy: vec![file(512 * GB)],
            left_out: vec![
                skipped(214 * GB, LeftOut::UnusedSound),
                skipped(31 * GB, LeftOut::MidSittingSave),
                skipped(13 * GB, LeftOut::OldAutosave),
                skipped(6 * GB, LeftOut::Cache),
            ],
        },
        all_bytes: 781 * GB,
        folder_bytes: vec![Some(184 * GB), Some(412 * GB), Some(96 * GB), Some(58 * GB), None],
        made_for: (settings.folders.clone(), settings.level),
        ..Prepared::default()
    };
    let room = Some(DriveRoom { free: 1100 * GB, total: 1862 * GB });
    page_lines(&settings, room, PlanState::Ready(&prepared), now)
}

/// Made-up older versions for the screenshots: 38 backups' worth over the past year, and (with `asking`) the question
/// asked before the ones more than 6 months old go.
pub fn sample_older(asking: bool) -> OlderCard {
    use mo_backup::older::Version;
    let now = Local::now().naive_local();
    let runs: Vec<Run> = (0..38u32)
        .rev()
        .map(|i| {
            let when = now - chrono::Duration::days(i64::from(10 * i + 1));
            let versions = (0..20 + i)
                .map(|n| Version { path: PathBuf::from(format!("x{n}")), size: 700_000, modified: None, shared: None })
                .collect();
            Run { stamp: older::stamp(when), when, versions }
        })
        .collect();
    let kept = Arc::new(Kept { runs, others: 0 });
    let ask = if asking { backup_older::ask(&kept, "older-6", now).map(|a| a.question) } else { None };
    OlderCard {
        lines: backup_older::lines(true, KeptState::Ready(&kept), now),
        ask: ask.unwrap_or_default(),
        done: String::new(),
    }
}
