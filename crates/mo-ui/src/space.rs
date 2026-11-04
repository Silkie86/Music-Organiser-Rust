//! The Space page: the drives your projects, samples, backup and the app's own files are on, how full each is, what
//! the app's own files take (part 1), Bitwig's copies of Library multisamples in your projects (part 2), with Fix for
//! the saves you tick, and the old copies moving plugin folders to another drive left (part 3). mo_app's `space`,
//! `space_bitwig` and `space_old` decide every word. It looks the first time the page shows, on low-priority threads,
//! and again when it shows after five minutes or on Measure again, so nothing on the window's thread waits for a disk
//! (rule 12). Only Fix writes, and only new "(fixed)" projects beside the saves you ticked; the library is only read.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mo_app::activity::Feed;
use mo_app::background::{self, Newest};
use mo_app::places::Place;
use mo_app::shortcuts::what_of;
use mo_app::space::{self, Drive, Fill, Holds, Opens, Part};
use mo_app::space_bitwig::{self, BITWIG_COPIES, Copies, Fixes, Fixing, ToFix};
use mo_app::space_old::{self, Found, OLD_COPIES};
use mo_app::{backup, log, paths, shell};
use mo_space::app_files::{self, AppFilesPart};
use mo_space::bitwig::{self, LibraryIndex};
use mo_space::plugin_space;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{AppWindow, CardAction, CardPart, DriveCard, RoomLine, SpaceData};

/// How long what the page shows stays fresh: shown again after this, it looks again (the C# app looked every five
/// minutes).
const FRESH: Duration = Duration::from_secs(5 * 60);

/// How often a look for Bitwig copies tells the window how far it is, at most (rule 12).
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// The buttons' ids, from the one shortcut list (rule 15): Measure again, Show in folder and Fix are the page's own
/// (`mo_app::shortcuts::SPACE_ACTIONS`); the Recycle Bin is the app-wide action, so its button shows the same key.
const MEASURE: &str = "space.measure";
const REVEAL: &str = "space.reveal";
const FIX: &str = "space.fix";
const RECYCLE_BIN: &str = "app.recycleBin";

/// The name the library knows Bitwig by ("Bitwig Studio"), for the program picked for it in Settings.
const BITWIG: &str = mo_formats::bitwig::reader::DAW;

/// One look at the drives and the app's files.
struct Ask {
    id: u64,
    library: PathBuf,
    app_folder: PathBuf,
    stop: Arc<AtomicBool>,
}

/// One look for Bitwig copies. It reads every copy, far more than a look at the drives, so it has a thread of its own
/// and is only asked for on the page's first look, on Measure again, and while no look has finished: never on the
/// five-minute look.
struct BitwigAsk {
    id: u64,
    library: PathBuf,
    stop: Arc<AtomicBool>,
    feed: Feed,
}

/// One look for old copies. It measures each one, which can take a while for a big sound library, so it has a thread
/// of its own. It's asked for with the page's own looks (the first, the five-minute one and Measure again), unless one
/// is still on.
struct OldAsk {
    id: u64,
    library: PathBuf,
    stop: Arc<AtomicBool>,
}

/// What a look brings back, a step at a time: the drives first (quick), then the app's files once they're measured;
/// from the look for Bitwig copies, how far it is, then what it found; and what the look for old copies found. A look
/// that crashed (the crash is logged) says so, so the page doesn't wait for it for ever. Fix brings back how each save
/// went, then that it has finished, always, even when it crashed.
enum Step {
    Drives(u64, Vec<Drive>),
    AppFiles(u64, Vec<AppFilesPart>),
    Crashed(u64),
    BitwigLooking(u64, (usize, usize)),
    Bitwig(u64, Copies),
    BitwigCrashed(u64),
    Old(u64, Found),
    OldCrashed(u64),
    Fixed(PathBuf, Fixing),
    FixDone,
}

/// One of the page's looks: its low-priority thread (started the first time a look is asked for), how many looks were
/// asked for, so an answer to an older one that arrives late is dropped, and the stop of the one still on.
struct Looks<J: Send + 'static> {
    worker: Option<Newest<J>>,
    asked: u64,
    stop: Arc<AtomicBool>,
}

impl<J: Send + 'static> Default for Looks<J> {
    fn default() -> Self {
        Looks { worker: None, asked: 0, stop: Arc::default() }
    }
}

impl<J: Send + 'static> Looks<J> {
    /// A new look's number and stop. The one still on stops: the newest wins (rule 12).
    fn next(&mut self) -> (u64, Arc<AtomicBool>) {
        self.stop.store(true, Ordering::Relaxed);
        self.stop = Arc::new(AtomicBool::new(false));
        self.asked += 1;
        (self.asked, self.stop.clone())
    }

    /// Gives `job` to the thread, which is started called `name`, doing `work`, the first time.
    fn ask(&mut self, name: &str, work: impl FnMut(J) + Send + 'static, job: J) -> std::io::Result<()> {
        if self.worker.is_none() {
            self.worker = Some(Newest::start(name, work)?);
        }
        if let Some(worker) = &self.worker {
            worker.ask(job);
        }
        Ok(())
    }
}

#[derive(Default)]
struct Page {
    library: PathBuf,
    /// Set once the page is wired; it looks at nothing before.
    feed: Option<Feed>,
    /// The look at the drives and the app's files.
    look: Looks<Ask>,
    /// When the newest look was asked for: none before the page first shows.
    looked: Option<Instant>,
    /// Whether a look is still measuring the app's files: Measure again waits for it.
    measuring: bool,
    drives: Vec<Drive>,
    /// The app's own files, once measured. A new look keeps the last sizes on show until its own are in.
    parts: Option<Vec<AppFilesPart>>,
    /// The look for Bitwig copies. Measure again doesn't wait for it: it starts it again, and as every fingerprint
    /// it read is kept, the new one goes quickly over what the last one read.
    bitwig_look: Looks<BitwigAsk>,
    /// What the last look for Bitwig copies found; it stays on show while the next one looks.
    bitwig: Option<Copies>,
    /// While a look for Bitwig copies is on: the project folders looked through, of how many (0 while they're still
    /// being found).
    bitwig_looking: Option<(usize, usize)>,
    /// What Fix did this time, the saves you ticked or unticked, and the projects whose other saves are on show.
    fixes: Fixes,
    /// The look for old copies, what the last one found (it stays on show while the next one looks), and whether one
    /// is on.
    old_look: Looks<OldAsk>,
    old: Option<Found>,
    old_looking: bool,
    /// The picked line's id, or "" when none is.
    picked: String,
    /// The parts on the card on show, for its ticks and links (they say which part by its place). The card's rows
    /// are only made again when these change.
    card_parts: Vec<Part>,
}

/// What a step that arrived changes on show.
enum Changed {
    /// The drives, the lists and the card.
    All,
    /// Biggest wins first and the card: what a look for Bitwig copies found or how far it is, or how Fix is doing.
    Wins,
    /// Fix has finished: the page looks for copies again, to see what Fix wrote as it is on disk.
    LookAgain,
    /// Nothing: an older look's step, or how far a later look for Bitwig copies is while the last one's stay on show.
    Nothing,
}

thread_local! {
    static PAGE: RefCell<Page> = RefCell::new(Page::default());
}

/// Wires the page. It reads nothing until it first shows ([`shown`]): the library at `library` (read-only) for your
/// project and sample folders, the program picked for Bitwig and the backup's settings for its drive. Opening a folder
/// or the Recycle Bin reports on `feed`'s cards if it can't, and the look for Bitwig copies and Fix show how far they
/// are there. The buttons' tooltips take their keys from the app's one list as it is now (`crate::shortcuts`).
pub fn connect(app: &AppWindow, library: PathBuf, feed: Feed) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.library = library;
        p.feed = Some(feed);
    });
    let data = app.global::<SpaceData>();
    let weak = app.as_weak();
    data.on_pick(move |id| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| p.borrow_mut().picked = id.to_string());
        show_card(&app);
    });
    let weak = app.as_weak();
    data.on_close_card(move || {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| p.borrow_mut().picked.clear());
        show_card(&app);
    });
    let weak = app.as_weak();
    data.on_chosen(move |id| {
        let Some(app) = weak.upgrade() else { return };
        chosen(&app, &id);
    });
    let weak = app.as_weak();
    data.on_tick_part(move |at, ticked| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            if let Some((save, _)) = part_at(&p, at).and_then(|part| part.tick.clone()) {
                p.fixes.ticks.insert(save, ticked);
            }
            // The tick box ticked itself, which undoes its tie to the card's rows: made again, they tie it again.
            p.card_parts.clear();
        });
        show_card(&app);
    });
    // A fixed project's link opens it in Bitwig; an old copy's shows it in its folder.
    data.on_open_part(move |at| {
        let (open, feed) = PAGE.with(|p| {
            let p = p.borrow();
            (part_at(&p, at).and_then(|part| part.open.clone()), p.feed.clone())
        });
        match (open, feed) {
            (Some((Opens::InBitwig, file)), _) => open_in_bitwig(file),
            (Some((Opens::InFolder, path)), Some(feed)) => {
                let path = path.to_string_lossy();
                crate::files::reveal(&feed, &path, shell::folder_of(&path));
            }
            _ => {}
        }
    });
    let weak = app.as_weak();
    data.on_more_part(move |at| {
        let Some(app) = weak.upgrade() else { return };
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            if let Some((_, folder)) = part_at(&p, at).and_then(|part| part.more.clone())
                && !p.fixes.shown.remove(&folder)
            {
                p.fixes.shown.insert(folder);
            }
        });
        show_card(&app);
    });
    show(app);
    // The app may start on this page, before it was wired.
    if app.get_place() == Place::Space.index() as i32 {
        shown(app);
    }
}

/// The page came on show: it looks the first time, and again once what it shows is five minutes old, old copies too,
/// so the reminder of them keeps up with what's there. It looks for Bitwig copies the first time, and again only while
/// no look for them has finished: it reads every project.
pub fn shown(app: &AppWindow) {
    let (stale, bitwig, old) = PAGE.with(|p| {
        let p = p.borrow();
        let wired = p.feed.is_some();
        let stale = wired && p.looked.is_none_or(|at| at.elapsed() >= FRESH);
        let bitwig = wired && p.bitwig.is_none() && p.bitwig_looking.is_none();
        (stale, bitwig, wired && !p.old_looking && (stale || p.old.is_none()))
    });
    if stale {
        look_again(app);
    }
    if bitwig {
        look_for_copies(app);
    }
    if old {
        look_for_old(app);
    }
}

/// Runs one of the page's own shortcuts ([`mo_app::shortcuts::SPACE_ACTIONS`]). Returns whether it did anything,
/// so an unused key goes on.
pub fn run_action(app: &AppWindow, id: &str) -> bool {
    let (measuring, picked, can_fix) = PAGE.with(|p| {
        let p = p.borrow();
        let can_fix = p.fixes.button(p.bitwig.as_ref()).is_some_and(|(_, enabled)| enabled);
        (p.measuring, p.picked.clone(), can_fix)
    });
    match id {
        MEASURE if !measuring => chosen(app, MEASURE),
        REVEAL if picked == space::APP_FILES => chosen(app, REVEAL),
        FIX if picked == BITWIG_COPIES && can_fix => chosen(app, FIX),
        _ => return false,
    }
    true
}

/// The part at `at` on the card on show.
fn part_at(p: &Page, at: i32) -> Option<&Part> {
    usize::try_from(at).ok().and_then(|at| p.card_parts.get(at))
}

/// A button on the page or its card.
fn chosen(app: &AppWindow, id: &str) {
    let feed = PAGE.with(|p| p.borrow().feed.clone());
    match (id, feed) {
        (MEASURE, _) => {
            look_again(app);
            look_for_copies(app);
            // A look for old copies that's on goes on: it read the moves just now, and nothing it measured is kept
            // for a new one.
            if !PAGE.with(|p| p.borrow().old_looking) {
                look_for_old(app);
            }
        }
        (REVEAL, Some(feed)) => crate::files::open_folder(&feed, &paths::data_dir().to_string_lossy()),
        (FIX, Some(feed)) => fix_ticked(app, feed),
        (RECYCLE_BIN, Some(feed)) => crate::files::open_recycle_bin(&feed),
        _ => {}
    }
}

/// The buttons: each one's id, words and whether it can be pressed now, with its tooltip and key from the shortcut
/// list as it is now (rule 15).
fn actions(buttons: Vec<(&str, String, bool)>) -> ModelRc<CardAction> {
    let keys = crate::shortcuts();
    let actions: Vec<CardAction> = buttons
        .into_iter()
        .map(|(id, text, enabled)| CardAction {
            text: text.into(),
            tip: what_of(id).into(),
            key: keys.key_of(id).into(),
            id: id.into(),
            enabled,
        })
        .collect();
    ModelRc::new(VecModel::from(actions))
}

/// Measure again, which waits while the drives and the app's files are being measured.
fn measure_button(p: &Page) -> (&'static str, String, bool) {
    (MEASURE, "Measure again".to_owned(), !p.measuring)
}

/// Fix: writes a "(fixed)" project beside each ticked save, one after another on a low-priority thread, with an
/// activity card saying how far it is. Each save's line says how it went once it's done; then the page looks for
/// copies again, so it sees the fixed projects as they are on disk. The saves and their copies aren't changed.
fn fix_ticked(app: &AppWindow, feed: Feed) {
    let jobs = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let p = &mut *p;
        if p.fixes.busy() {
            return Vec::new();
        }
        let jobs = p.bitwig.as_ref().map(|found| p.fixes.to_fix(found)).unwrap_or_default();
        if !jobs.is_empty() {
            p.fixes.start(&jobs);
        }
        jobs
    });
    if jobs.is_empty() {
        return;
    }
    let weak = app.as_weak();
    let started = background::spawn("Space fix", move || {
        // A crash outside a save's own fix (logged as one) still finishes Fix, so the page isn't busy for ever: the
        // saves it didn't get to say so.
        let _ = catch_unwind(AssertUnwindSafe(|| fix_each(&jobs, &feed, &weak)));
        arrive(&weak, Step::FixDone);
    });
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start fixing: {e}"));
        PAGE.with(|p| p.borrow_mut().fixes.finished());
    }
    show_wins(app);
}

/// Fix's own work, off the window's thread: each save in turn, inside the low-priority pool (rule 21), with an
/// activity card saying how far it is and how it went, and each save's line told how it went.
fn fix_each(jobs: &[ToFix], feed: &Feed, weak: &slint::Weak<AppWindow>) {
    let card = feed.start(space_bitwig::FIXING, "");
    let (mut written, mut failed) = (0, 0);
    for (n, job) in jobs.iter().enumerate() {
        card.progress(n as f32 / jobs.len() as f32, Some(&space_bitwig::fix_progress_words(n, jobs.len())));
        // A crash (logged as one) fails this save only: the rest are still tried.
        let done = catch_unwind(AssertUnwindSafe(|| background::on_pool(|| space_bitwig::fix(job))))
            .unwrap_or_else(|_| Fixing::Failed("it stopped with an error, which the log says".to_owned()));
        let save = job.save.display();
        match &done {
            Fixing::Written(file) => {
                written += 1;
                log::info("Space", &format!("Fixed {save}: wrote {}", file.display()));
            }
            Fixing::Failed(why) => {
                failed += 1;
                log::warning("Space", &format!("Couldn't fix {save}: {why}"));
            }
            Fixing::Writing => {}
        }
        arrive(weak, Step::Fixed(job.save.clone(), done));
    }
    match space_bitwig::fixed_words(written, failed) {
        Ok(words) => card.done(&words),
        Err(words) => card.failed(&words),
    }
}

/// Opens a fixed project in Bitwig: the program picked for it in Settings (shared with the C# app), else whatever opens
/// Bitwig projects. Off the window's thread, with an activity card saying how it went.
fn open_in_bitwig(file: PathBuf) {
    let Some((feed, library)) = PAGE.with(|p| {
        let p = p.borrow();
        Some((p.feed.clone()?, p.library.clone()))
    }) else {
        return;
    };
    let started = background::spawn("Open", move || {
        let path = file.to_string_lossy().into_owned();
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
        if !shell::exists(&path) {
            feed.start(&format!("Can't find {name}"), "")
                .failed("It has moved or been deleted since. Measure again to see what's there now.");
            return;
        }
        let exe = open_library(&library)
            .and_then(|lib| lib.setting(&shell::daw_exe_key(BITWIG)).ok().flatten())
            .filter(|e| shell::usable_exe(e));
        let card = feed.start(&format!("Opening {name}"), "");
        match shell::run(&shell::open(&path, exe.as_deref(), shell::Os::THIS)) {
            Ok(()) => card.done("Sent to Bitwig. It opens there in a moment."),
            Err(e) => {
                log::warning("Open", &format!("{path}: {e}"));
                card.failed(&format!("Couldn't open it in Bitwig: {e}"));
            }
        }
    });
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start opening a fixed project: {e}"));
    }
}

/// Asks for a new look at the drives and the app's files, on the page's own low-priority thread. One still measuring
/// stops: the newest wins (rule 12).
fn look_again(app: &AppWindow) {
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let (id, stop) = p.look.next();
        p.looked = Some(Instant::now());
        p.measuring = true;
        Ask { id, library: p.library.clone(), app_folder: paths::data_dir(), stop }
    });
    let weak = app.as_weak();
    // A crash in a look (logged as one) leaves the thread for the next look, and Measure again for trying again.
    let work = move |ask: Ask| {
        let id = ask.id;
        if catch_unwind(AssertUnwindSafe(|| look(ask, &weak))).is_err() {
            arrive(&weak, Step::Crashed(id));
        }
    };
    let started = PAGE.with(|p| p.borrow_mut().look.ask("Space", work, ask));
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start looking at the drives: {e}"));
        // Measure again can try again, and so can the page the next time it shows.
        PAGE.with(|p| {
            let mut p = p.borrow_mut();
            p.looked = None;
            p.measuring = false;
        });
    }
    show(app);
}

/// One look, off the window's thread: the drives your folders are on, then the app's own files.
fn look(ask: Ask, weak: &slint::Weak<AppWindow>) {
    let folders = folders_of(&ask.library, &ask.app_folder);
    let drives = space::drives(&folders, space::look);
    arrive(weak, Step::Drives(ask.id, drives));
    let data = &ask.app_folder;
    let databases = app_files::databases(data.join("library.db"), data.join("sounds.db"), data.join("midi.db"));
    // Spread over the low-priority pool's threads (rule 21). A look stopped for a newer one says nothing: the newer
    // one's sizes follow.
    if let Ok(parts) = background::on_pool(|| app_files::measure(data, &databases, &ask.stop)) {
        arrive(weak, Step::AppFiles(ask.id, parts));
    }
}

/// Asks for a new look for Bitwig copies, on the page's second low-priority thread. One still looking stops: the
/// newest wins (rule 12).
fn look_for_copies(app: &AppWindow) {
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let feed = p.feed.clone()?;
        let (id, stop) = p.bitwig_look.next();
        p.bitwig_looking = Some((0, 0));
        Some(BitwigAsk { id, library: p.library.clone(), stop, feed })
    });
    let Some(ask) = ask else { return };
    let weak = app.as_weak();
    // The Library as the last look read it, kept on the thread so a file isn't read again while it's unchanged. A
    // crash in a look (logged as one) leaves the thread for the next look; the Library is read again then, in case
    // the crash came from what was kept.
    let mut index = None;
    let work = move |ask: BitwigAsk| {
        let id = ask.id;
        if catch_unwind(AssertUnwindSafe(|| look_for_copies_now(ask, &mut index, &weak))).is_err() {
            index = None;
            arrive(&weak, Step::BitwigCrashed(id));
        }
    };
    let started = PAGE.with(|p| p.borrow_mut().bitwig_look.ask("Space Bitwig", work, ask));
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start looking for Bitwig copies: {e}"));
        // Measure again can try again, and so can the page the next time it shows.
        PAGE.with(|p| p.borrow_mut().bitwig_looking = None);
    }
    show(app);
}

/// One look for Bitwig copies, off the window's thread: the project folders from the library (read-only), then each
/// project's copies, with an activity card saying how far it is.
fn look_for_copies_now(ask: BitwigAsk, index: &mut Option<LibraryIndex>, weak: &slint::Weak<AppWindow>) {
    let roots: Vec<PathBuf> = project_roots(&ask.library).into_iter().map(PathBuf::from).collect();
    let documents = bitwig::documents();
    let job = ask.feed.start(space_bitwig::LOOKING, space_bitwig::LOOKING_WHY);
    // How far it is reaches the window at most every PROGRESS_EVERY, and always at the start and the end.
    let mut told: Option<Instant> = None;
    // Spread over the low-priority pool's threads (rule 21).
    let found = background::on_pool(|| {
        space_bitwig::look(&roots, documents.as_deref(), index, &ask.stop, |done, of| {
            if of > 0 {
                job.progress(done as f32 / of as f32, Some(&space_bitwig::progress_words(done, of)));
            }
            if done == 0 || done == of || told.is_none_or(|at| at.elapsed() >= PROGRESS_EVERY) {
                told = Some(Instant::now());
                arrive(weak, Step::BitwigLooking(ask.id, (done, of)));
            }
        })
    });
    match found {
        Ok(copies) => {
            let (headline, words) = space_bitwig::found_words(&copies);
            job.done_as(headline, &words);
            arrive(weak, Step::Bitwig(ask.id, copies));
        }
        // Stopped for a newer look, whose own card follows.
        Err(_) => job.done_as(space_bitwig::LOOK_STOPPED.0, space_bitwig::LOOK_STOPPED.1),
    }
}

/// Asks for a new look for old copies, on the page's third low-priority thread. One still looking stops: the newest
/// wins (rule 12).
fn look_for_old(app: &AppWindow) {
    let ask = PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.feed.as_ref()?;
        let (id, stop) = p.old_look.next();
        p.old_looking = true;
        Some(OldAsk { id, library: p.library.clone(), stop })
    });
    let Some(ask) = ask else { return };
    let weak = app.as_weak();
    // A crash in a look (logged as one) leaves the thread for the next look. A look stopped for a newer one says
    // nothing: the newer one's finds follow.
    let work = move |ask: OldAsk| {
        let id = ask.id;
        match catch_unwind(AssertUnwindSafe(|| look_for_old_now(&ask))) {
            Ok(Some(found)) => arrive(&weak, Step::Old(id, found)),
            Ok(None) => {}
            Err(_) => arrive(&weak, Step::OldCrashed(id)),
        }
    };
    let started = PAGE.with(|p| p.borrow_mut().old_look.ask("Space old copies", work, ask));
    if let Err(e) = started {
        log::error("Space", &format!("Couldn't start looking for old copies: {e}"));
        // Measure again can try again, and so can the page the next time it shows.
        PAGE.with(|p| p.borrow_mut().old_looking = false);
    }
    // The old copies card's Measure again waits for it.
    show_card(app);
}

/// One look for old copies, off the window's thread: the moves as the library keeps them (read-only, and closed
/// again before anything is measured), then each old copy still there, measured, and any other "… (old copy)" folder
/// in the folders plugins keep their data in. None when it was stopped for a newer look.
fn look_for_old_now(ask: &OldAsk) -> Option<Found> {
    let (moved, homes) = match open_library(&ask.library) {
        Some(lib) => {
            let setting = |key: &str| {
                lib.setting(key).unwrap_or_else(|e| {
                    log::warning("Space", &format!("Couldn't read the moved folders ({key}): {e}"));
                    None
                })
            };
            (setting(space_old::MOVED_FOLDERS), setting(space_old::OLD_HOMES))
        }
        None => (None, None),
    };
    let look_in: Vec<String> =
        plugin_space::data_roots().iter().map(|root| root.to_string_lossy().into_owned()).collect();
    // On the pool for long work that waits on a disk (an old copy can be a whole sound library), so the pages' quick
    // jobs don't wait for it (rule 21).
    background::on_bulk_pool(|| space_old::look(moved.as_deref(), homes.as_deref(), &look_in, &ask.stop)).ok()
}

/// The library, opened read-only for the folders it knows; None when there's none yet or it can't be read (said in
/// the log).
fn open_library(library: &Path) -> Option<mo_library::Library> {
    if !library.exists() {
        return None;
    }
    mo_library::Library::open_read_only(library)
        .map_err(|e| log::warning("Space", &format!("Couldn't read the library for its folders: {e}")))
        .ok()
}

/// Your project folders, as the library lists them.
fn roots_in(lib: &mo_library::Library) -> Vec<String> {
    lib.roots().unwrap_or_else(|e| {
        log::warning("Space", &format!("Couldn't read the project folders: {e}"));
        Vec::new()
    })
}

/// Your project folders, from the library (read-only).
fn project_roots(library: &Path) -> Vec<String> {
    open_library(library).as_ref().map(roots_in).unwrap_or_default()
}

/// The folders to look at: your project and sample folders from the library (read-only), the backup drive from the
/// backup's settings, and the app's own folder.
fn folders_of(library: &Path, app_folder: &Path) -> Vec<(Holds, PathBuf)> {
    let lib = open_library(library);
    let roots = lib.as_ref().map(roots_in).unwrap_or_default();
    let samples = lib
        .as_ref()
        .map(|lib| {
            lib.list_setting("sample_folders").unwrap_or_else(|e| {
                log::warning("Space", &format!("Couldn't read the sample folders: {e}"));
                Vec::new()
            })
        })
        .unwrap_or_default();
    let backup_drive = backup::Settings::load(&app_folder.join(backup::SETTINGS_FILE)).drive;
    space::folders(&roots, &samples, backup_drive.as_deref(), app_folder)
}

/// A look's step reaches the window's thread, unless a newer look has been asked for since.
fn arrive(weak: &slint::Weak<AppWindow>, step: Step) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(app) = weak.upgrade() else { return };
        let _doing = crate::watchdog::doing("Space: a look arrived");
        let changed = PAGE.with(|p| {
            let mut p = p.borrow_mut();
            match step {
                Step::Drives(id, drives) if id == p.look.asked => {
                    p.drives = drives;
                    Changed::All
                }
                Step::AppFiles(id, parts) if id == p.look.asked => {
                    p.parts = Some(parts);
                    p.measuring = false;
                    Changed::All
                }
                Step::Crashed(id) if id == p.look.asked => {
                    p.measuring = false;
                    Changed::All
                }
                // Only the first look's progress is on the page; the activity card has a later one's.
                Step::BitwigLooking(id, at) if id == p.bitwig_look.asked => {
                    p.bitwig_looking = Some(at);
                    if p.bitwig.is_none() { Changed::Wins } else { Changed::Nothing }
                }
                Step::Bitwig(id, copies) if id == p.bitwig_look.asked => {
                    p.fixes.looked();
                    p.bitwig = Some(copies);
                    p.bitwig_looking = None;
                    Changed::Wins
                }
                // Shown again, the page looks again.
                Step::BitwigCrashed(id) if id == p.bitwig_look.asked => {
                    p.bitwig_looking = None;
                    Changed::Wins
                }
                Step::Old(id, found) if id == p.old_look.asked => {
                    p.old = Some(found);
                    p.old_looking = false;
                    Changed::Wins
                }
                // Shown again, the page looks again; what the last look found stays on show.
                Step::OldCrashed(id) if id == p.old_look.asked => {
                    p.old_looking = false;
                    Changed::Wins
                }
                Step::Fixed(save, fixing) => {
                    p.fixes.done(save, fixing);
                    Changed::Wins
                }
                Step::FixDone => {
                    p.fixes.finished();
                    Changed::LookAgain
                }
                _ => Changed::Nothing,
            }
        });
        match changed {
            Changed::All => show(&app),
            Changed::Wins => show_wins(&app),
            Changed::LookAgain => look_for_copies(&app),
            Changed::Nothing => {}
        }
    });
}

fn room_line(line: space::Line) -> RoomLine {
    RoomLine { id: line.id.into(), title: line.title.into(), why: line.why.into(), size: line.size.into() }
}

/// Shows what the page knows now: the summary, the drives, the lists and the picked line's card.
pub fn show(app: &AppWindow) {
    let data = app.global::<SpaceData>();
    PAGE.with(|p| {
        let p = p.borrow();
        let app_bytes = p.parts.as_deref().map_or(0, space::app_bytes);
        data.set_summary(space::summary(&p.drives, app_bytes).into());
        let cards: Vec<DriveCard> = p
            .drives
            .iter()
            .map(|d| DriveCard {
                name: d.name.as_str().into(),
                room: space::room_line(d).into(),
                used: d.used(),
                holds: space::holds_line(d).into(),
                note: space::fill_note(d, app_bytes).into(),
                fill: match d.fill(app_bytes) {
                    Fill::Roomy => 0,
                    Fill::Nearly => 1,
                    Fill::Almost => 2,
                },
                here: d.here,
            })
            .collect();
        data.set_drives(ModelRc::new(VecModel::from(cards)));
        let app_drive = space::app_drive(&p.drives);
        let also = vec![room_line(space::app_files_line(p.parts.as_deref(), app_drive.map(|d| d.name.as_str())))];
        data.set_also(ModelRc::new(VecModel::from(also)));
        data.set_actions(actions(vec![measure_button(&p), (RECYCLE_BIN, "Open Recycle Bin".to_owned(), true)]));
    });
    show_wins(app);
}

/// Shows Biggest wins first and the picked line's card: all that changes while a look for Bitwig copies or old copies
/// goes on. A picked line that's gone from the list (none found this time) has its card closed.
fn show_wins(app: &AppWindow) {
    let data = app.global::<SpaceData>();
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        let bitwig = space_bitwig::line(p.bitwig.as_ref(), p.bitwig_looking);
        let old = space_old::line(p.old.as_ref());
        if (bitwig.is_none() && p.picked == BITWIG_COPIES) || (old.is_none() && p.picked == OLD_COPIES) {
            p.picked.clear();
        }
        let lines = space::biggest_first(bitwig.into_iter().chain(old).collect());
        let wins: Vec<RoomLine> = lines.into_iter().map(room_line).collect();
        data.set_wins(ModelRc::new(VecModel::from(wins)));
        data.set_wins_note(space_bitwig::wins_note(p.bitwig.as_ref()).into());
    });
    show_card(app);
}

/// Shows which line is picked and its card; picking a line or closing its card changes only this.
fn show_card(app: &AppWindow) {
    let data = app.global::<SpaceData>();
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        data.set_picked(p.picked.as_str().into());
        let (card, buttons) = match p.picked.as_str() {
            space::APP_FILES => (
                space::app_files_card(p.parts.as_deref(), space::app_drive(&p.drives)),
                vec![(REVEAL, "Show in folder".to_owned(), true), measure_button(&p)],
            ),
            BITWIG_COPIES => {
                let fix = p.fixes.button(p.bitwig.as_ref()).map(|(words, enabled)| (FIX, words, enabled));
                let card = space_bitwig::card(p.bitwig.as_ref(), p.bitwig_looking, &p.fixes);
                (card, fix.into_iter().chain([measure_button(&p)]).collect())
            }
            // Measure again waits while old copies are being measured too, which shows a look is on.
            OLD_COPIES => {
                let (id, words, enabled) = measure_button(&p);
                (space_old::card(p.old.as_ref()), vec![(id, words, enabled && !p.old_looking)])
            }
            _ => return,
        };
        data.set_card_kind(card.kind.into());
        data.set_card_title(card.title.into());
        data.set_card_big(card.big.into());
        data.set_card_big_note(card.big_note.into());
        data.set_card_lead(card.lead.into());
        data.set_card_ticks(card.ticks);
        // Made again only when they change, so how far a look or Fix is doesn't make every row again.
        if card.parts != p.card_parts {
            let parts: Vec<CardPart> = card.parts.iter().map(card_part).collect();
            data.set_card_parts(ModelRc::new(VecModel::from(parts)));
            p.card_parts = card.parts;
        }
        data.set_card_note(card.note.into());
        data.set_card_room(card.room.into());
        data.set_card_warning(card.warning.into());
        data.set_card_actions(actions(buttons));
    });
}

fn card_part(part: &Part) -> CardPart {
    CardPart {
        name: part.name.as_str().into(),
        what: part.what.as_str().into(),
        size: part.size.as_str().into(),
        under: part.under,
        can_tick: part.tick.is_some(),
        ticked: part.tick.as_ref().is_some_and(|(_, ticked)| *ticked),
        status: part.status.as_str().into(),
        problem: part.problem,
        open: part.open.as_ref().map(|(opens, _)| opens.words()).unwrap_or_default().into(),
        more: part.more.as_ref().map(|(words, _)| words.as_str()).unwrap_or_default().into(),
    }
}

/// Shows made-up drives, app files, Bitwig copies (with what Fix did to them) and old copies, without reading any
/// disk: for the screenshots.
pub fn show_sample(
    app: &AppWindow,
    drives: Vec<Drive>,
    parts: Vec<AppFilesPart>,
    bitwig: Copies,
    fixes: Fixes,
    old: Found,
    picked: &str,
) {
    PAGE.with(|p| {
        let mut p = p.borrow_mut();
        p.drives = drives;
        p.parts = Some(parts);
        p.bitwig = Some(bitwig);
        p.fixes = fixes;
        p.old = Some(old);
        p.picked = picked.to_owned();
        // Shown now, so showing the page doesn't look at the real drives or projects.
        p.looked = Some(Instant::now());
    });
    show(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_button_is_in_the_shortcut_list() {
        for id in [MEASURE, REVEAL, FIX, RECYCLE_BIN] {
            assert_ne!(what_of(id), "", "{id} isn't in the shortcut list");
        }
        let keys = mo_app::shortcuts::Shortcuts::default();
        assert_eq!(keys.page_action_for("F5", mo_app::shortcuts::SPACE_ACTIONS), Some(MEASURE));
        assert_eq!(keys.page_action_for("Ctrl+Shift+E", mo_app::shortcuts::SPACE_ACTIONS), Some(REVEAL));
    }
}
