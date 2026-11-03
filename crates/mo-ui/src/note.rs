//! F12: the note to Claude. The picture is taken before the dialog opens, so it shows the page, not the dialog. What's
//! typed is kept in the draft file a moment after each pause in typing (and straight away if the app closes or
//! crashes first), and saving happens off the window's thread, ending in an activity card that says where it went.
//!
//! Only the Cancel button throws the words away. Esc or a click beside the dialog just puts the note aside: the next
//! F12 offers it back. Save hands the words to the save: the note's words are written to disk before its picture, a
//! save that fails puts them back as the draft, and `writing.lock` stays up while a note is open or still being saved,
//! so the launcher never restarts the app in the middle. The app waits a few seconds for a save still going before
//! it closes.
//!
//! The window's thread never touches the note's files (rule 12): the draft, the lock and the words of failed saves are
//! read and written in turn by one "Note files" thread, which always writes the newest draft and the lock as it is
//! now. A crash, and the app closing with that thread stuck, write the draft themselves, straight away, but in its
//! turn ([`DRAFT_TURN`]): never two writers at once.

use std::cell::RefCell;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError, TryLockError};
use std::time::{Duration, Instant};

use mo_app::activity::Feed;
use mo_app::{log, note};
use slint::{ComponentHandle, Timer};

use crate::AppWindow;

/// A picture of the window, as raw RGBA pixels until the note is saved.
struct Picture {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// Typed but not yet in the draft file. The words stay here until they're on disk, so a crash meanwhile still finds
/// them.
static UNSAVED: Mutex<Option<String>> = Mutex::new(None);
const DRAFT_PAUSE: Duration = Duration::from_millis(300);
/// Taken while the draft is written or cleared, from reading the words in `UNSAVED` until they're on disk, so it's
/// written by one thread at a time. Writing empties the file first: a crash that ends the app while another thread
/// was part way would lose the note, and two writers at once could leave older words over newer ones. The "Note
/// files" thread waits for its turn. A crash, and the app closing with that thread stuck, wait only a moment
/// ([`DRAFT_MOMENT`]) and then write anyway: the thread whose turn it is may be the one that crashed, or stuck on a
/// slow disk. The window's thread never waits for it: it only touches `UNSAVED`.
static DRAFT_TURN: Mutex<()> = Mutex::new(());
/// How long a crash, or the app closing, waits for the draft's turn.
const DRAFT_MOMENT: Duration = Duration::from_secs(1);
/// A note is open in the window.
static OPEN: AtomicBool = AtomicBool::new(false);
/// Saves still going.
static SAVING: AtomicUsize = AtomicUsize::new(0);
/// How many times a note has been opened, so the text from before reaches only the note it was read for.
static OPENINGS: AtomicUsize = AtomicUsize::new(0);
/// The open note shows the text from before (the draft and the words of failed saves), or there was none. Until then,
/// what's typed is kept with that text rather than over it.
static FILLED: AtomicBool = AtomicBool::new(true);
/// How long the app waits for them when it closes.
const SAVE_WAIT: Duration = Duration::from_secs(5);

/// What the "Note files" thread does, in the order it's asked.
enum FileJob {
    /// Writes what's typed but not yet kept.
    KeepDraft,
    /// The note was saved or cancelled: its draft has done its job.
    ClearDraft,
    /// Puts `writing.lock` up or takes it down, by whether a note is open or being saved now.
    Lock,
    /// Keeps the words of a note that failed to save, for the next F12.
    KeepFailed(String),
    /// Reads the note's starting text and puts it in the note opened that time (by `OPENINGS`).
    Fill(slint::Weak<AppWindow>, usize),
    /// The words of failed saves are in the draft now: their own file goes.
    ForgetFailed,
    /// The app is closing: keeps the draft, takes the lock down and says so.
    Finish(Sender<()>),
}

static FILES: OnceLock<Mutex<Option<Sender<FileJob>>>> = OnceLock::new();

/// How long writing or clearing the draft waits for its turn ([`DRAFT_TURN`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Turn {
    /// As long as it takes: the "Note files" thread.
    Wait,
    /// A moment ([`DRAFT_MOMENT`]), and then it goes ahead anyway: a crash, or the app closing with the "Note files"
    /// thread stuck.
    Moment,
    /// Not at all: a job done on the thread that asked (the window's, perhaps), as the "Note files" thread couldn't
    /// start. When it isn't its turn, the words wait in `UNSAVED` for the next try.
    Never,
}

/// The draft's turn, waiting as `turn` says; `None` when it didn't come.
fn draft_turn(turn: Turn) -> Option<MutexGuard<'static, ()>> {
    match turn {
        Turn::Wait => Some(DRAFT_TURN.lock().unwrap_or_else(PoisonError::into_inner)),
        Turn::Moment => lock_within(&DRAFT_TURN, DRAFT_MOMENT),
        Turn::Never => lock_within(&DRAFT_TURN, Duration::ZERO),
    }
}

/// Takes `lock`, trying for up to `limit`; `None` if it's still taken then. A crash mustn't wait for ever on a lock the
/// thread that crashed may hold.
fn lock_within<T>(lock: &Mutex<T>, limit: Duration) -> Option<MutexGuard<'_, T>> {
    let start = Instant::now();
    loop {
        match lock.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(e)) => return Some(e.into_inner()),
            Err(TryLockError::WouldBlock) if start.elapsed() >= limit => return None,
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(1)),
        }
    }
}

/// Hands a job to the "Note files" thread, started the first time. If that thread can't be started (or has
/// stopped), the job is done here instead: late is better than lost.
fn on_files(job: FileJob) {
    let slot = FILES.get_or_init(|| {
        let (tx, rx) = channel();
        let started = mo_app::background::spawn("Note files", move || do_file_jobs(&rx));
        if let Err(e) = &started {
            log::warning("Note", &format!("Couldn't start the thread for the note's files: {e}"));
        }
        Mutex::new(started.ok().map(|_| tx))
    });
    let job = match slot.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        Some(tx) => match tx.send(job) {
            Ok(()) => return,
            Err(std::sync::mpsc::SendError(job)) => job,
        },
        None => job,
    };
    // This may be the window's thread, which never waits for the draft's turn.
    do_file_job(job, Turn::Never);
}

fn do_file_jobs(jobs: &Receiver<FileJob>) {
    for job in jobs {
        // One job going wrong (it's logged as a crash) mustn't lose the jobs after it.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| do_file_job(job, Turn::Wait)));
    }
}

/// Does `job`, waiting for the draft's turn as `turn` says.
fn do_file_job(job: FileJob, turn: Turn) {
    let folder = note::notes_dir();
    match job {
        FileJob::KeepDraft => keep_unsaved(&folder, turn),
        // A draft from before that wasn't shown yet wasn't seen, so it isn't thrown away.
        FileJob::ClearDraft if FILLED.load(Ordering::Acquire) => {
            // When it isn't its turn here, the draft stays: offered again next time, rather than lost.
            if let Some(_mine) = draft_turn(turn) {
                note::clear_draft(&folder);
            }
        }
        FileJob::ClearDraft => {}
        FileJob::Lock => {
            let busy = OPEN.load(Ordering::Acquire) || SAVING.load(Ordering::Acquire) > 0;
            note::writing(&folder, busy);
        }
        FileJob::KeepFailed(text) => keep_failed_words(&folder, &text),
        FileJob::Fill(weak, opening) => {
            // In its turn, so it never reads a draft that's half written.
            let text = {
                let _mine = draft_turn(turn);
                starting_text(&folder)
            };
            if text.is_empty() {
                FILLED.store(true, Ordering::Release);
                return;
            }
            let _ = weak.upgrade_in_event_loop(move |app| {
                if !app.get_note_open() || OPENINGS.load(Ordering::Acquire) != opening {
                    // Put aside before it arrived: it's still on disk, for next time.
                    return;
                }
                let typed = app.get_note_text();
                // Typed straight away: the draft goes first.
                let whole = if typed.is_empty() { text } else { format!("{text}\n\n{typed}") };
                app.set_note_text(whole.as_str().into());
                FILLED.store(true, Ordering::Release);
                *UNSAVED.lock().unwrap_or_else(|e| e.into_inner()) = Some(whole);
                on_files(FileJob::KeepDraft);
                on_files(FileJob::ForgetFailed);
            });
        }
        FileJob::ForgetFailed => {
            let _ = std::fs::remove_file(folder.join(FAILED_FILE));
        }
        FileJob::Finish(done) => {
            // The app is closing and its window has gone, so even done here it may wait a moment for the turn.
            keep_unsaved(&folder, if turn == Turn::Never { Turn::Moment } else { turn });
            note::writing(&folder, false);
            let _ = done.send(());
        }
    }
}

thread_local! {
    static PICTURE: RefCell<Option<Picture>> = const { RefCell::new(None) };
    static DRAFT_TIMER: Timer = Timer::default();
}

/// Opens the note, with a picture of the window as it is now and any draft left from before.
pub fn open_note(app: &AppWindow) {
    if app.get_note_open() {
        return;
    }
    let picture = match app.window().take_snapshot() {
        Ok(buf) => Some(Picture { width: buf.width(), height: buf.height(), rgba: buf.as_bytes().to_vec() }),
        Err(e) => {
            log::warning("Note", &format!("Couldn't take a picture of the window: {e}"));
            None
        }
    };
    app.set_note_has_picture(picture.is_some());
    app.set_note_include_picture(picture.is_some());
    PICTURE.with(|p| *p.borrow_mut() = picture);
    app.set_note_text("".into());
    FILLED.store(false, Ordering::Release);
    let opening = OPENINGS.fetch_add(1, Ordering::AcqRel) + 1;
    OPEN.store(true, Ordering::Release);
    update_lock();
    app.set_note_open(true);
    on_files(FileJob::Fill(app.as_weak(), opening));
}

pub fn connect(app: &AppWindow, feed: &Feed) {
    app.on_note_edited(|text| {
        *UNSAVED.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.to_string());
        DRAFT_TIMER.with(|t| t.start(slint::TimerMode::SingleShot, DRAFT_PAUSE, || on_files(FileJob::KeepDraft)));
    });

    let weak = app.as_weak();
    app.on_note_cancel(move || {
        if let Some(app) = weak.upgrade() {
            close(&app, true);
        }
    });

    let weak = app.as_weak();
    app.on_note_put_aside(move || {
        if let Some(app) = weak.upgrade() {
            close(&app, false);
        }
    });

    let weak = app.as_weak();
    let feed = feed.clone();
    app.on_note_save(move || {
        let Some(app) = weak.upgrade() else { return };
        let text = app.get_note_text().to_string();
        let picture = PICTURE.with(|p| p.borrow_mut().take()).filter(|_| app.get_note_include_picture());
        // The save counts before the note closes, so the lock never drops in between.
        SAVING.fetch_add(1, Ordering::AcqRel);
        close(&app, true);
        save_in_background(&feed, text, picture);
    });
}

/// Writes what's typed but not yet kept into the draft in `folder`, in the draft's turn, waiting for it as `turn` says
/// (see [`DRAFT_TURN`]). The words leave `UNSAVED` once they're on disk, unless newer ones came meanwhile. Safe from
/// any thread, and from a crash: it never waits longer than `turn` says.
fn keep_unsaved(folder: &Path, turn: Turn) {
    let mine = draft_turn(turn);
    if mine.is_none() && turn == Turn::Never {
        return;
    }
    // Only a look: the words stay in `UNSAVED` until they're on disk. The window's thread holds it only a moment.
    let Some(text) = lock_within(&UNSAVED, DRAFT_MOMENT).and_then(|unsaved| (*unsaved).clone()) else { return };
    if FILLED.load(Ordering::Acquire) {
        note::keep_draft(folder, &text);
    } else {
        // Typed before the text from before was shown: kept with it, not over it, and from now on it's all one draft.
        let before = starting_text(folder);
        let whole = match (before.is_empty(), text.is_empty()) {
            (true, _) => text.clone(),
            (false, true) => before,
            (false, false) => format!("{before}\n\n{text}"),
        };
        note::keep_draft(folder, &whole);
        let _ = std::fs::remove_file(folder.join(FAILED_FILE));
        FILLED.store(true, Ordering::Release);
    }
    if let Some(mut unsaved) = lock_within(&UNSAVED, DRAFT_MOMENT)
        && unsaved.as_ref() == Some(&text)
    {
        *unsaved = None;
    }
    // The turn ends once the words are on disk.
    drop(mine);
}

/// `writing.lock` is up while a note is open or still being saved.
fn update_lock() {
    on_files(FileJob::Lock);
}

/// For the app closing: waits up to a few seconds for notes still being saved. A save cut off after that has its
/// words on disk already (they're written before the picture).
pub fn wait_for_saves() {
    let until = Instant::now() + SAVE_WAIT;
    while SAVING.load(Ordering::Acquire) > 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    if SAVING.load(Ordering::Acquire) > 0 {
        log::warning("Note", "A note was still being saved when the app closed; its words were written first.");
    }
}

/// For the app closing: the words typed are kept as the draft for next time, and `writing.lock` comes down, so the
/// launcher isn't kept waiting. Waits a moment for the note's files to be done.
pub fn finish_up() {
    let (done, finished) = channel();
    on_files(FileJob::Finish(done));
    if finished.recv_timeout(Duration::from_secs(2)).is_err() {
        // The files' thread is stuck or gone: done here, so the draft and the lock are still right. If it's stuck in
        // the middle of the draft, this waits only a moment for it.
        let folder = note::notes_dir();
        keep_unsaved(&folder, Turn::Moment);
        note::writing(&folder, false);
    }
}

/// For a crash, from the crash handler. Whatever the crash, the words typed are kept straight away, on this thread,
/// before it returns: std may end the app straight after one the handler can't tell will (a crash in a thread's own
/// data as the thread ends). They're written in the draft's turn, waiting only a moment for it. When the crash ends the
/// app (`ends_the_app`), the lock comes down too. After any other crash the app goes on, perhaps with the note still
/// open: the "Note files" thread puts the lock as it should be, in order with its other jobs.
pub fn after_crash(ends_the_app: bool) {
    keep_after_crash(&note::notes_dir(), ends_the_app);
    if !ends_the_app {
        update_lock();
    }
}

/// What [`after_crash`] does in the notes folder, `folder`, before it returns.
fn keep_after_crash(folder: &Path, ends_the_app: bool) {
    keep_unsaved(folder, Turn::Moment);
    if ends_the_app {
        note::writing(folder, false);
    }
}

/// Closes the note. Saved or cancelled, the draft has done its job; put aside, it stays for next time.
fn close(app: &AppWindow, done_with: bool) {
    DRAFT_TIMER.with(Timer::stop);
    PICTURE.with(|p| p.borrow_mut().take());
    if done_with {
        UNSAVED.lock().unwrap_or_else(|e| e.into_inner()).take();
        on_files(FileJob::ClearDraft);
    } else {
        on_files(FileJob::KeepDraft);
    }
    OPEN.store(false, Ordering::Release);
    update_lock();
    app.set_note_open(false);
    app.set_note_text("".into());
}

/// Ends a save however the save thread ends, a panic included, so the lock can't stay up for good.
struct SaveDone;

impl Drop for SaveDone {
    fn drop(&mut self) {
        SAVING.fetch_sub(1, Ordering::AcqRel);
        update_lock();
    }
}

/// Words from saves that failed, waiting for the next F12. A file of their own, so a note open meanwhile (whose draft
/// is written and cleared as usual) can't overwrite them.
const FAILED_FILE: &str = "unsaved.txt";

/// A save that fails keeps its words for the next F12, in `folder` (and they're in the log too).
fn keep_failed_words(folder: &Path, text: &str) {
    use std::io::Write;
    let kept = std::fs::create_dir_all(folder).and_then(|()| {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(folder.join(FAILED_FILE))?;
        writeln!(f, "{}\n", text.trim_end())
    });
    if let Err(e) = kept {
        log::warning("Note", &format!("Couldn't keep the words of a note that failed to save: {e}"));
    }
}

/// The note's starting text in `folder`: words from saves that failed, then the draft. Only read: the files change
/// once it's shown.
fn starting_text(folder: &Path) -> String {
    let failed = std::fs::read_to_string(folder.join(FAILED_FILE)).unwrap_or_default();
    let draft = note::read_draft(folder).unwrap_or_default();
    match (failed.trim().is_empty(), draft.is_empty()) {
        (true, _) => draft,
        (false, true) => failed.trim_end().to_owned(),
        (false, false) => format!("{}\n\n{draft}", failed.trim_end()),
    }
}

/// Saves the note on a thread of its own. `SAVING` already counts it.
fn save_in_background(feed: &Feed, text: String, picture: Option<Picture>) {
    let page = crate::page_name();
    let job = feed.start("Saving your note", "");
    // If the saving can't even start, the words still reach the draft and the log.
    let words = text.clone();
    let spawned = mo_app::background::spawn("Note", move || {
        let _done = SaveDone;
        let png = picture.and_then(|p| match encode_png(&p) {
            Ok(png) => Some(png),
            Err(e) => {
                log::warning("Note", &format!("Couldn't make the picture: {e}"));
                None
            }
        });
        let recent = log::current().map(|l| l.recent()).unwrap_or_default();
        let version = mo_app::version::version();
        let n = note::Note {
            text: &text,
            page,
            version: &version,
            log_path: log::current().and_then(|l| l.path()),
            recent_log: &recent,
            picture: png.as_deref(),
        };
        match note::save(&note::notes_dir(), &n, chrono::Local::now()) {
            Ok(path) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                log::info("Note", &format!("Saved {name}"));
                job.done(&format!("Saved as {name}"));
            }
            Err(e) => {
                on_files(FileJob::KeepFailed(text.clone()));
                log::error("Note", &format!("Couldn't save the note: {e}\n{text}"));
                job.failed(&format!("Couldn't save it: {e}. Your words are kept for the next F12."));
            }
        }
    });
    if let Err(e) = spawned {
        // The closure (and its SaveDone) never ran, so the save is ended here.
        drop(SaveDone);
        on_files(FileJob::KeepFailed(words.clone()));
        log::error("Note", &format!("Couldn't start saving the note: {e}\n{words}"));
    }
}

fn encode_png(p: &Picture) -> image::ImageResult<Vec<u8>> {
    use image::ImageEncoder;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(
        &p.rgba,
        p.width,
        p.height,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// The words typed and the draft's turn are the app's own, one of each, so these tests take turns too.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    /// A notes folder of the test's own: tests never touch the app's data folder.
    fn temp_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-ui-note-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// As the window's thread does when the words change.
    fn typed(words: &str) {
        FILLED.store(true, Ordering::Release);
        *UNSAVED.lock().unwrap_or_else(PoisonError::into_inner) = Some(words.to_owned());
    }

    #[test]
    fn a_crash_keeps_the_words_before_it_returns_waiting_only_a_moment_for_its_turn() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = temp_folder("crash");
        typed("half a thought");
        // The "Note files" thread is part way through the draft, stuck on a disk that's asleep: it's its turn.
        let stuck = DRAFT_TURN.lock().unwrap_or_else(PoisonError::into_inner);
        // A crash the handler can't tell will end the app (std may end it straight after).
        let (done, finished) = channel();
        let folder = dir.clone();
        std::thread::spawn(move || {
            keep_after_crash(&folder, false);
            let _ = done.send(());
        });
        let returned = finished.recv_timeout(Duration::from_secs(30));
        let kept = note::read_draft(&dir);
        drop(stuck);
        assert!(returned.is_ok(), "it waits only a moment for its turn");
        assert_eq!(kept.as_deref(), Some("half a thought"), "the words are on disk before it returns");
        assert_eq!(*UNSAVED.lock().unwrap_or_else(PoisonError::into_inner), None, "and kept, so they're done with");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_writers_take_turns_so_the_newest_words_stay() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = temp_folder("turns");
        typed("the newest words");
        // The "Note files" thread has its turn, and is writing the words from before.
        let turn = DRAFT_TURN.lock().unwrap_or_else(PoisonError::into_inner);
        // Meanwhile another thread crashes, and keeps the newest words.
        let folder = dir.clone();
        let crash = std::thread::spawn(move || keep_after_crash(&folder, false));
        std::thread::sleep(Duration::from_millis(50));
        note::keep_draft(&dir, "the words from before");
        drop(turn);
        crash.join().expect("no crash in keeping the words");
        assert_eq!(note::read_draft(&dir).as_deref(), Some("the newest words"), "written in its turn, last");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_job_that_cant_wait_for_its_turn_leaves_the_words_for_the_next_write() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = temp_folder("never");
        typed("first words");
        keep_unsaved(&dir, Turn::Wait);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("first words"));
        assert_eq!(*UNSAVED.lock().unwrap_or_else(PoisonError::into_inner), None, "on disk, so done with");
        // When it isn't its turn, a job done on the window's thread leaves the words for the next try.
        typed("more words");
        let turn = DRAFT_TURN.lock().unwrap_or_else(PoisonError::into_inner);
        keep_unsaved(&dir, Turn::Never);
        drop(turn);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("first words"));
        assert_eq!(UNSAVED.lock().unwrap_or_else(PoisonError::into_inner).as_deref(), Some("more words"));
        keep_unsaved(&dir, Turn::Wait);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("more words"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
