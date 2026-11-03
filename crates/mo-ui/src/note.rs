//! F12: the note to Claude. The picture is taken before the dialog opens, so it shows the page, not the dialog. What's
//! typed is kept in the draft file a moment after each pause in typing (and straight away if the app closes or
//! crashes first), and saving happens off the window's thread, ending in an activity card that says where it went.
//!
//! Only the Cancel button throws the words away. Esc or a click beside the dialog just puts the note aside: the next
//! F12 offers it back (straight from memory, as the same note, while its words aren't on disk yet). Save hands the
//! words to the save: the note's words are written to disk first, before its picture is made (a big window takes a
//! moment), a save that fails keeps them for the next F12, and `writing.lock` stays up while a note is open or still
//! being saved, so the launcher never restarts the app in the middle. The app waits a few seconds for a save still
//! going before it closes, and puts the words of one that isn't on disk yet in the session log.
//!
//! The window's thread never touches the note's files (rule 12): the draft, the lock and the words of failed saves are
//! read and written in turn by one "Note files" thread, which always writes the newest draft and the lock as it is
//! now. A crash, and the app closing with that thread stuck, write the draft themselves, straight away, in the
//! draft's turn ([`DRAFT_TURN`]) when it comes within a moment, and without it when it doesn't: then through a part
//! file of their own, so they never touch one that thread is writing. At the end of a run, words that aren't surely in
//! the draft (written without the turn, or a draft that couldn't be written) go with the words of failed saves too,
//! and in the session log, as nothing comes after to put the draft right.

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

/// Words typed but not yet in the draft file. They stay here until they're written in the draft's turn, so a crash
/// meanwhile still finds them.
static UNSAVED: Mutex<Option<Typed>> = Mutex::new(None);
const DRAFT_PAUSE: Duration = Duration::from_millis(300);

/// Words typed in a note, with how that note stood when they were typed.
#[derive(Clone, PartialEq, Eq)]
struct Typed {
    words: String,
    /// The note they were typed in (by `OPENINGS`).
    opening: usize,
    /// That note showed the text from before by then, or there was none ([`FILLED`] as it was): the words hold that
    /// text already, so they go over the draft. Words typed before it showed go before it (as the note shows them).
    filled: bool,
}

/// The draft's turn: taken while the draft is written or cleared, from reading the words in `UNSAVED` until they're on
/// disk, and while the words of failed saves are kept or let go, so one thread at a time does that. The "Note files"
/// thread waits for its turn. A crash, the app closing with that thread stuck, and a save that failed wait only a
/// moment ([`DRAFT_MOMENT`]) and then go ahead anyway (the thread whose turn it is may be the one that crashed, or stuck
/// on a slow disk). A draft written then leaves the words in `UNSAVED`, for a write in its turn; words added to those of
/// failed saves then are in the session log too, as the thread whose turn it is may let that file go meanwhile. The
/// window's thread never waits for it: it only touches `UNSAVED`.
///
/// It also keeps the text from before as it was read for one note ([`Before`]): read once, by the first of the note's
/// Fill job and a write of words typed before the note showed it, so the two always use the same text, and nothing in
/// the note shows twice.
static DRAFT_TURN: Mutex<Option<Before>> = Mutex::new(None);
/// How long a crash, or the app closing, waits for the draft's turn (longer in tests, so a slow test machine can't run
/// out of time).
const DRAFT_MOMENT: Duration = Duration::from_secs(if cfg!(test) { 5 } else { 1 });

/// The text from before (the words of failed saves, then the draft), as it was read for one note.
struct Before {
    /// The note it was read for (by `OPENINGS`).
    opening: usize,
    text: String,
    /// What the file of failed saves' words held when it was read, so only those words are let go once the draft
    /// holds them ([`forget_failed`]): words of a save that failed since stay, to be shown rather than lost.
    failed: String,
    /// Those words are let go already (or there were none), so their file isn't looked at again.
    forgotten: bool,
}

/// A note is open in the window.
static OPEN: AtomicBool = AtomicBool::new(false);
/// Saves still going.
static SAVING: AtomicUsize = AtomicUsize::new(0);
/// How many times a note has been opened, so the text from before reaches only the note it was read for.
static OPENINGS: AtomicUsize = AtomicUsize::new(0);
/// The newest note (by `OPENINGS`) whose words were written into the draft in its turn; read and changed only in the
/// draft's turn. A note saved or cancelled clears the draft only while it holds no newer note's words: a write can go
/// ahead of the jobs still waiting for an earlier note (a crash's, the close's, or one asked for before that note
/// closed, which writes the newest words), and the earlier note's clearing mustn't take them.
static IN_THE_DRAFT: AtomicUsize = AtomicUsize::new(0);
/// The open note shows the text from before (the draft and the words of failed saves), or there was none. Until then,
/// what's typed is kept before that text rather than over it. Only the window's thread changes it or reads it, and it
/// goes with the words typed ([`Typed`]), so the words and it always match.
static FILLED: AtomicBool = AtomicBool::new(true);
/// The app is ending (closing, or a crash that ends it): from now on a Lock job only takes `writing.lock` down, so a
/// save still going, or a crash as the app closes, doesn't put it back up after the app took it down. (A Lock job
/// already past this check may still put it up a last time; the launcher passes over a lock whose app has gone.)
static ENDING: AtomicBool = AtomicBool::new(false);
/// How long the app waits for them when it closes.
const SAVE_WAIT: Duration = Duration::from_secs(5);

/// What the "Note files" thread does, in the order it's asked.
enum FileJob {
    /// Writes what's typed but not yet kept.
    KeepDraft,
    /// The note opened that time (`opening`, by `OPENINGS`) was saved or cancelled: its draft has done its job if the
    /// note showed it (`filled`, [`FILLED`] as it was then), and so have the words of failed saves it showed.
    ClearDraft { filled: bool, opening: usize },
    /// Puts `writing.lock` up or takes it down, by whether a note is open or being saved now.
    Lock,
    /// Keeps the words of a note that failed to save, for the next F12. `shown`: the note they're from (by
    /// `OPENINGS`), when it showed the text from before: the words of failed saves it showed have done their job, as its
    /// ClearDraft says too, so they're let go first, and don't show twice whichever of the two jobs comes first.
    KeepFailed { words: String, shown: Option<usize> },
    /// Reads the note's starting text and puts it in the note opened that time (by `OPENINGS`).
    Fill(slint::Weak<AppWindow>, usize),
    /// The app is closing: keeps the draft, takes the lock down and says so.
    Finish(Sender<()>),
}

/// Where jobs go to the "Note files" thread (`None` when it couldn't start). Set once, the first time a job is asked
/// for, and never locked: a crash can always hand it a job.
static FILES: OnceLock<Option<Sender<FileJob>>> = OnceLock::new();

/// How long a job waits for the draft's turn ([`DRAFT_TURN`]).
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
fn draft_turn(turn: Turn) -> Option<MutexGuard<'static, Option<Before>>> {
    match turn {
        Turn::Wait => Some(DRAFT_TURN.lock().unwrap_or_else(PoisonError::into_inner)),
        Turn::Moment => lock_within(&DRAFT_TURN, DRAFT_MOMENT),
        Turn::Never => lock_within(&DRAFT_TURN, Duration::ZERO),
    }
}

/// How long a job waits for `UNSAVED`, which the window's thread holds only a moment: a moment, or not at all for one
/// that mustn't wait (`Turn::Never`).
fn unsaved_wait(turn: Turn) -> Duration {
    if turn == Turn::Never { Duration::ZERO } else { DRAFT_MOMENT }
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
    let files = FILES.get_or_init(|| {
        let (tx, rx) = channel();
        let started = mo_app::background::spawn("Note files", move || do_file_jobs(&rx));
        if let Err(e) = &started {
            log::warning("Note", &format!("Couldn't start the thread for the note's files: {e}"));
        }
        started.ok().map(|_| tx)
    });
    let job = match files {
        Some(tx) => match tx.send(job) {
            Ok(()) => return,
            Err(std::sync::mpsc::SendError(job)) => job,
        },
        None => job,
    };
    // This may be the window's thread, which never waits for the draft's turn.
    do_file_job(&note::notes_dir(), job, Turn::Never);
}

fn do_file_jobs(jobs: &Receiver<FileJob>) {
    let folder = note::notes_dir();
    for job in jobs {
        // One job going wrong (it's logged as a crash) mustn't lose the jobs after it.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| do_file_job(&folder, job, Turn::Wait)));
    }
}

/// Does `job` in the notes folder, `folder`, waiting for the draft's turn as `turn` says.
fn do_file_job(folder: &Path, job: FileJob, turn: Turn) {
    match job {
        FileJob::KeepDraft => {
            keep_unsaved(folder, turn);
        }
        // A draft from before that wasn't shown yet wasn't seen, so it isn't thrown away.
        FileJob::ClearDraft { filled: true, opening } => {
            // When it isn't its turn here, the draft stays: offered again next time, rather than lost. So does a draft
            // that holds a newer note's words (`IN_THE_DRAFT`), and the words of failed saves in it with them.
            if let Some(mut mine) = draft_turn(turn)
                && IN_THE_DRAFT.load(Ordering::Acquire) <= opening
            {
                note::clear_draft(folder);
                if let Some(before) = mine.as_mut().filter(|b| b.opening == opening) {
                    forget_failed(folder, before);
                }
            }
        }
        FileJob::ClearDraft { filled: false, .. } => {}
        FileJob::Lock => {
            // Once the app is ending, it only comes down.
            let open = OPEN.load(Ordering::Acquire) || SAVING.load(Ordering::Acquire) > 0;
            note::writing(folder, open && !ENDING.load(Ordering::Acquire));
        }
        FileJob::KeepFailed { words, shown } => {
            // In its turn, so the words of failed saves a note showed are let go only while the file holds just those
            // (see `forget_failed`), never these unseen. When it isn't its turn here, they're added all the same; the
            // file may then go with them, but whoever asked put them in the session log too.
            let mut mine = draft_turn(turn);
            if let Some(before) = mine.as_deref_mut().and_then(Option::as_mut).filter(|b| Some(b.opening) == shown) {
                forget_failed(folder, before);
            }
            keep_failed_words(folder, &words);
        }
        FileJob::Fill(weak, opening) => {
            let text = text_for_note(folder, opening, turn);
            let _ = weak.upgrade_in_event_loop(move |app| {
                if !app.get_note_open() {
                    // Put aside before it arrived: it's still on disk, for next time.
                    return;
                }
                if let Some(whole) = fill_lands(opening, &app.get_note_text(), text) {
                    app.set_note_text(whole.as_str().into());
                    on_files(FileJob::KeepDraft);
                }
            });
        }
        FileJob::Finish(done) => {
            // The app is closing and its window has gone, so even done here it may wait a moment for the turn.
            keep_at_the_end(folder, if turn == Turn::Never { Turn::Moment } else { turn });
            note::writing(folder, false);
            let _ = done.send(());
        }
    }
}

/// The note's Fill job: the text from before for the note opened that time (`opening`), read in the draft's turn
/// (waiting as `turn` says), so it never reads a draft that's half written, and reads what a write before it read.
/// Nothing for a note that's no longer the newest: its Fill never lands ([`fill_lands`]), and what was read for the
/// newer note must stay as it was read.
fn text_for_note(folder: &Path, opening: usize, turn: Turn) -> String {
    let mut mine = draft_turn(turn);
    if OPENINGS.load(Ordering::Acquire) != opening {
        return String::new();
    }
    text_before(mine.as_deref_mut(), folder, opening)
}

/// The text from before (`text`, read for the note opened that time, `opening`) reaches the open note, in which
/// `typed` is typed so far: gives what the note shows now, and keeps that as typed in it. `None` when it's for an
/// earlier note, or the note shows it already (it was opened again with its words), or there's no text from before.
/// Words typed before it came stay first, where the cursor is, and the text from before goes after them: what's typed
/// always goes before it (setting the text leaves the cursor where it was).
fn fill_lands(opening: usize, typed: &str, text: String) -> Option<String> {
    if OPENINGS.load(Ordering::Acquire) != opening || FILLED.load(Ordering::Acquire) {
        return None;
    }
    FILLED.store(true, Ordering::Release);
    if text.is_empty() {
        return None;
    }
    let whole = if typed.is_empty() { text } else { format!("{typed}\n\n{text}") };
    *UNSAVED.lock().unwrap_or_else(PoisonError::into_inner) =
        Some(Typed { words: whole.clone(), opening, filled: true });
    Some(whole)
}

/// The text from before for the note opened that time (`opening`), read from `folder` in the draft's turn (`turn`,
/// what the turn keeps): read the first time, and the same text after that, so the note's Fill job and a write of words
/// typed before the note showed it always agree. Without the turn, it's read again.
fn text_before(turn: Option<&mut Option<Before>>, folder: &Path, opening: usize) -> String {
    let Some(kept) = turn else { return starting_text(folder).1 };
    if let Some(before) = kept.as_ref().filter(|b| b.opening == opening) {
        return before.text.clone();
    }
    let (failed, text) = starting_text(folder);
    *kept = Some(Before { opening, text: text.clone(), forgotten: failed.is_empty(), failed });
    text
}

/// The words of failed saves that a note showed (`before`, read for it) are in the draft now, written in its turn, or
/// have done their job (the note was saved or cancelled): their file goes, but only while it holds just those words. Words of a save that failed since then stay, with those
/// (shown again rather than lost). Looked at until they're let go, or the file has changed.
fn forget_failed(folder: &Path, before: &mut Before) {
    if before.forgotten {
        return;
    }
    let file = folder.join(FAILED_FILE);
    match std::fs::read_to_string(&file) {
        Ok(now) if now == before.failed => before.forgotten = std::fs::remove_file(&file).is_ok(),
        Ok(_) => before.forgotten = true,
        Err(e) => before.forgotten = e.kind() == std::io::ErrorKind::NotFound,
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
    let note = opening_note(&on_files);
    app.set_note_text(note.words.as_str().into());
    OPEN.store(true, Ordering::Release);
    update_lock();
    app.set_note_open(true);
    if !note.filled {
        on_files(FileJob::Fill(app.as_weak(), note.opening));
    }
}

/// How a note opens ([`opening_note`]).
struct Opening {
    /// Its number (by `OPENINGS`).
    opening: usize,
    /// The words it opens with, before the text from before comes.
    words: String,
    /// It shows the text from before already ([`FILLED`]).
    filled: bool,
    /// It's the note put aside, opened again with its words.
    again: bool,
}

/// A note opens. While the words of the note put aside aren't on disk yet (their write is still to come, or failed),
/// that note opens again, with them, as it stood: so the next words typed can't take their place, and they're shown
/// straight away. Their write is asked for again through `send` (the one that should have kept them may have failed).
/// Otherwise a new note opens, empty until the text from before comes.
fn opening_note(send: &dyn Fn(FileJob)) -> Opening {
    let waiting = UNSAVED.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let note = match waiting {
        // Words in `UNSAVED` are always the newest note's: a new note opens only once they're gone.
        Some(typed) => Opening { opening: typed.opening, words: typed.words, filled: typed.filled, again: true },
        None => Opening {
            opening: OPENINGS.fetch_add(1, Ordering::AcqRel) + 1,
            words: String::new(),
            filled: false,
            again: false,
        },
    };
    FILLED.store(note.filled, Ordering::Release);
    if note.again {
        send(FileJob::KeepDraft);
    }
    note
}

/// What's typed (`words`) in the open note, as that note stands now.
fn typed_now(words: String) -> Typed {
    Typed { words, opening: OPENINGS.load(Ordering::Acquire), filled: FILLED.load(Ordering::Acquire) }
}

/// The note closes: saved or cancelled (`done_with`), its words have done their job, and so has its draft if it showed
/// it; put aside, what's typed is kept for next time. Gives the job for the "Note files" thread.
fn closing(done_with: bool) -> FileJob {
    if done_with {
        UNSAVED.lock().unwrap_or_else(PoisonError::into_inner).take();
        FileJob::ClearDraft { filled: FILLED.load(Ordering::Acquire), opening: OPENINGS.load(Ordering::Acquire) }
    } else {
        FileJob::KeepDraft
    }
}

pub fn connect(app: &AppWindow, feed: &Feed) {
    app.on_note_edited(|text| {
        *UNSAVED.lock().unwrap_or_else(PoisonError::into_inner) = Some(typed_now(text.to_string()));
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
        // The save counts before the note closes, so the lock never drops in between, and its words are kept for the
        // end of the run until they're on disk.
        SAVING.fetch_add(1, Ordering::AcqRel);
        let save = save_of(being_saved(&text));
        close(&app, true);
        save_in_background(&feed, save, text, picture);
    });
}

/// How [`keep_unsaved`] went.
#[derive(Debug, PartialEq, Eq)]
enum Kept {
    /// No words were waiting, or none could be written now (`Turn::Never`, and not its turn).
    Nothing,
    /// Written in the draft's turn, and on disk.
    OnDisk,
    /// Written in the draft's turn, but the draft couldn't be written (these words, as typed): they stay in `UNSAVED`
    /// for another try.
    NotOnDisk(String),
    /// Written without the draft's turn (these words, as typed), through a part file of its own: an older write still
    /// going may yet land over them, so they stay in `UNSAVED` for a write in its turn.
    Alone(String),
}

/// Writes what's typed but not yet kept into the draft in `folder`, in the draft's turn, waiting for it as `turn` says
/// (see [`DRAFT_TURN`]), and for `UNSAVED` a moment more (not at all with `Turn::Never`). The words leave `UNSAVED`
/// only once they're on disk, written in the draft's turn, unless newer ones came meanwhile. Safe from any thread, and
/// from a crash: with `Turn::Moment` or `Turn::Never` it never waits more than a moment.
fn keep_unsaved(folder: &Path, turn: Turn) -> Kept {
    let mut mine = draft_turn(turn);
    if mine.is_none() && turn == Turn::Never {
        return Kept::Nothing;
    }
    // Only a look: the words stay in `UNSAVED` until they're on disk. The window's thread holds it only a moment.
    let unsaved = lock_within(&UNSAVED, unsaved_wait(turn)).and_then(|unsaved| (*unsaved).clone());
    let Some(typed) = unsaved else { return Kept::Nothing };
    let whole = if typed.filled {
        typed.words.clone()
    } else {
        // Typed before the note showed the text from before: kept before it, not over it (as the note shows them
        // once it comes, `fill_lands`).
        let before = text_before(mine.as_deref_mut(), folder, typed.opening);
        match (before.is_empty(), typed.words.is_empty()) {
            (true, _) => typed.words.clone(),
            (false, true) => before,
            (false, false) => format!("{}\n\n{before}", typed.words),
        }
    };
    let Some(held) = mine.as_mut() else {
        note::keep_draft_alone(folder, &whole);
        return Kept::Alone(typed.words);
    };
    if !note::keep_draft(folder, &whole) {
        return Kept::NotOnDisk(typed.words);
    }
    // The draft holds this note's words now (see `IN_THE_DRAFT`), and the words of failed saves it showed.
    IN_THE_DRAFT.fetch_max(typed.opening, Ordering::AcqRel);
    if let Some(before) = held.as_mut().filter(|b| b.opening == typed.opening) {
        forget_failed(folder, before);
    }
    // Done with once they're on disk, unless newer ones came meanwhile.
    if let Some(mut unsaved) = lock_within(&UNSAVED, unsaved_wait(turn))
        && unsaved.as_ref() == Some(&typed)
    {
        *unsaved = None;
    }
    // The turn ends once the words are on disk.
    drop(mine);
    Kept::OnDisk
}

/// Keeps what's typed as the app ends (it closes, or a crash ends it), waiting for the draft's turn as `turn` says.
/// Words that aren't surely in the draft then (written without the turn, so an older write still going may yet land
/// over them, or a draft that couldn't be written) go with the words of failed saves too, and in the session log, as
/// nothing comes after to try again. Shown twice, then, rather than lost.
fn keep_at_the_end(folder: &Path, turn: Turn) {
    let words = match keep_unsaved(folder, turn) {
        Kept::Alone(words) | Kept::NotOnDisk(words) => words,
        Kept::Nothing | Kept::OnDisk => return,
    };
    if words.trim().is_empty() {
        return;
    }
    keep_failed_words(folder, &words);
    // A line of how the run ends: it shows even once the log is full.
    let said = format!(
        "As the app ended, a note's words couldn't be put in its draft for sure (it couldn't be written, or an older \
         write was still going), so they're kept with the words of failed saves too, for the next F12:\n{words}"
    );
    log::last(log::Level::Warning, "Note", &said);
}

/// `writing.lock` is up while a note is open or still being saved.
fn update_lock() {
    on_files(FileJob::Lock);
}

/// For the app closing: waits up to a few seconds for notes still being saved. A save cut off after that most likely
/// loses only its picture: its words go to disk first, before the picture is made. The words of one that aren't on
/// disk yet are kept for the next F12, as a save that failed, and go in the session log.
pub fn wait_for_saves() {
    wait_for_saves_in(&note::notes_dir(), SAVE_WAIT);
}

/// [`wait_for_saves`], in the notes folder `folder`, waiting up to `wait`.
fn wait_for_saves_in(folder: &Path, wait: Duration) {
    let until = Instant::now() + wait;
    while SAVING.load(Ordering::Acquire) > 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    if SAVING.load(Ordering::Acquire) > 0 && keep_saves_at_the_end(folder) == 0 {
        // A line of how the run ends: it shows even once the log is full. Its words were in a file before they stopped
        // being kept for the end of the run (in the note, or with the words of failed saves, and in this log).
        let said =
            "A note was still being saved when the app closed, after its words were kept; its picture may not be.";
        log::last(log::Level::Warning, "Note", said);
    }
}

/// As the app ends (it closes, or a crash ends it), the words of notes being saved that aren't on disk yet are kept
/// for the next F12, as saves that failed, and go in the session log: shown twice if a save ends after all, rather
/// than lost. Waits a moment at most. Gives how many there were.
fn keep_saves_at_the_end(folder: &Path) -> usize {
    let waiting = lock_within(&BEING_SAVED, DRAFT_MOMENT).map(|mut saves| std::mem::take(&mut *saves));
    let waiting = waiting.unwrap_or_default();
    for (_, words) in &waiting {
        keep_failed_words(folder, words);
        // A line of how the run ends: it shows even once the log is full.
        let said = format!(
            "A note was still being saved as the app ended, and its words weren't on disk yet. They're kept for the \
             next F12:\n{words}"
        );
        log::last(log::Level::Warning, "Note", &said);
    }
    waiting.len()
}

/// For the app closing: the words typed are kept as the draft for next time (or, when it can't be written, with the
/// words of failed saves: [`keep_at_the_end`]), and `writing.lock` comes down, so the launcher isn't kept waiting.
/// Waits a moment for the note's files to be done.
pub fn finish_up() {
    finish_up_in(&note::notes_dir(), &on_files);
}

/// [`finish_up`], in the notes folder `folder`, handing the job to the "Note files" thread through `send`.
fn finish_up_in(folder: &Path, send: &dyn Fn(FileJob)) {
    ENDING.store(true, Ordering::Release);
    let (done, finished) = channel();
    send(FileJob::Finish(done));
    if finished.recv_timeout(Duration::from_secs(2)).is_err() {
        // The files' thread is stuck or gone: done here, so the draft and the lock are still right. If it's stuck in
        // the middle of the draft, this waits only a moment for it.
        keep_at_the_end(folder, Turn::Moment);
        note::writing(folder, false);
    }
}

/// For a crash, from the crash handler. Whatever the crash, the words typed are kept straight away, on this thread,
/// before it returns: std may end the app straight after one the handler can't tell will (a crash in a thread's own
/// data as the thread ends). They're written in the draft's turn, waiting only a moment for it. When the crash ends the
/// app (`ends_the_app`), the lock comes down too (see [`ENDING`]), and words written without the turn go with the
/// words of failed saves too ([`keep_at_the_end`]). After any other crash the app goes on, perhaps with the note still
/// open: the "Note files" thread puts the lock as it should be, in order with its other jobs.
pub fn after_crash(ends_the_app: bool) {
    keep_after_crash(&note::notes_dir(), ends_the_app, &update_lock_after_crash);
}

/// All [`after_crash`] does, in the notes folder `folder`, with `update_lock` to put the lock as it should be after a
/// crash the app goes on after.
fn keep_after_crash(folder: &Path, ends_the_app: bool, update_lock: &dyn Fn()) {
    if ends_the_app {
        ENDING.store(true, Ordering::Release);
        keep_at_the_end(folder, Turn::Moment);
        keep_saves_at_the_end(folder);
        note::writing(folder, false);
    } else {
        keep_unsaved(folder, Turn::Moment);
        update_lock();
    }
}

/// [`update_lock`] for the crash handler: only once a job has been handed to the "Note files" thread (a note was
/// opened, so there may be a lock to put right), so a crash never starts that thread, nor waits on one being started.
fn update_lock_after_crash() {
    if FILES.get().is_some() {
        update_lock();
    }
}

/// Closes the note. Saved or cancelled, the draft has done its job; put aside, it stays for next time.
fn close(app: &AppWindow, done_with: bool) {
    DRAFT_TIMER.with(Timer::stop);
    PICTURE.with(|p| p.borrow_mut().take());
    on_files(closing(done_with));
    OPEN.store(false, Ordering::Release);
    update_lock();
    app.set_note_open(false);
    app.set_note_text("".into());
}

/// A save: its number (in [`BEING_SAVED`]), and, when its note showed the text from before, that note (by `OPENINGS`,
/// see [`FileJob::KeepFailed`]).
#[derive(Clone, Copy)]
struct SaveOf {
    number: usize,
    shown: Option<usize>,
}

/// The open note's save, as number `number`: if it fails, the words of failed saves the note showed go (when it showed
/// the text from before), as with its ClearDraft.
fn save_of(number: usize) -> SaveOf {
    SaveOf { number, shown: FILLED.load(Ordering::Acquire).then(|| OPENINGS.load(Ordering::Acquire)) }
}

/// Ends a save however the save thread ends, a panic included, so the lock can't stay up for good. A save that stopped
/// before its words were on disk (it crashed part way) keeps them for the next F12, as a save that failed.
struct SaveDone(SaveOf);

impl Drop for SaveDone {
    fn drop(&mut self) {
        // On the save's own thread, which may wait a moment for the draft's turn. (When the save couldn't start, it's
        // dropped on the window's thread, but only once its words were let go: then there's nothing to keep here.)
        save_ended(self.0, &|job| do_file_job(&note::notes_dir(), job, Turn::Moment));
        SAVING.fetch_sub(1, Ordering::AcqRel);
        update_lock();
    }
}

/// A save (`save`) is over: if its words weren't on disk yet (it stopped part way), they go in the session log and are
/// kept for the next F12, as a save that failed, through `keep`, before they stop being kept for the end of the run:
/// so they're always in one or the other.
fn save_ended(save: SaveOf, keep: &dyn Fn(FileJob)) {
    if let Some(words) = still_being_saved(save.number) {
        log::error("Note", &format!("A note's save stopped part way: its words are kept for the next F12.\n{words}"));
        keep(FileJob::KeepFailed { words, shown: save.shown });
        words_on_disk(save.number);
    }
}

/// Words from saves that failed, waiting for the next F12. A file of their own, so a note open meanwhile (whose draft
/// is written and cleared as usual) can't overwrite them.
const FAILED_FILE: &str = "unsaved.txt";

/// A save that fails keeps its words for the next F12, in `folder` (and they're in the log too). Synced: after a failed
/// save it's the one file that holds them.
fn keep_failed_words(folder: &Path, text: &str) {
    use std::io::Write;
    let kept = std::fs::create_dir_all(folder).and_then(|()| {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(folder.join(FAILED_FILE))?;
        f.write_all(format!("{}\n\n", text.trim_end()).as_bytes())?;
        f.sync_all()
    });
    if let Err(e) = kept {
        log::warning("Note", &format!("Couldn't keep the words of a note that failed to save: {e}"));
    }
}

/// The note's starting text in `folder`: what the file of failed saves' words held, and the text the note starts
/// with (those words, then the draft). Only read: the files change once it's shown.
fn starting_text(folder: &Path) -> (String, String) {
    let failed = std::fs::read_to_string(folder.join(FAILED_FILE)).unwrap_or_default();
    let draft = note::read_draft(folder).unwrap_or_default();
    let text = match (failed.trim().is_empty(), draft.is_empty()) {
        (true, _) => draft,
        (false, true) => failed.trim_end().to_owned(),
        (false, false) => format!("{}\n\n{draft}", failed.trim_end()),
    };
    (failed, text)
}

/// The words of notes being saved that aren't on disk yet, by a number for each save: if the app closes first, they're
/// kept for the next F12 ([`wait_for_saves`]).
static BEING_SAVED: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());
/// How many saves there have been, to number them.
static SAVES: AtomicUsize = AtomicUsize::new(0);

/// A save of these words (`text`) starts: they're kept for the end of the run until they're on disk. Gives its number.
fn being_saved(text: &str) -> usize {
    let save = SAVES.fetch_add(1, Ordering::AcqRel);
    BEING_SAVED.lock().unwrap_or_else(PoisonError::into_inner).push((save, text.to_owned()));
    save
}

/// The words of save number `save` are on disk (or the save is over): they're no longer kept for the end of the run.
/// Gives them if they were still kept.
fn words_on_disk(save: usize) -> Option<String> {
    let mut saves = BEING_SAVED.lock().unwrap_or_else(PoisonError::into_inner);
    let at = saves.iter().position(|(n, _)| *n == save)?;
    Some(saves.swap_remove(at).1)
}

/// The words of save number `save`, while they're still kept for the end of the run.
fn still_being_saved(save: usize) -> Option<String> {
    let saves = BEING_SAVED.lock().unwrap_or_else(PoisonError::into_inner);
    saves.iter().find(|(n, _)| *n == save).map(|(_, words)| words.clone())
}

/// Saves the note on a thread of its own, as `save`. `SAVING` already counts it.
fn save_in_background(feed: &Feed, save: SaveOf, text: String, picture: Option<Picture>) {
    let page = crate::page_name();
    let job = feed.start("Saving your note", "");
    // If the saving can't even start, the words still reach the file of failed saves' words, and the log.
    let words = text.clone();
    let spawned = mo_app::background::spawn("Note", move || {
        let _done = SaveDone(save);
        let make_png = |p: &Picture| encode_png(p).map_err(|e| e.to_string());
        // A save that fails keeps its words itself, waiting a moment for the draft's turn at most: the "Note files"
        // thread may be stuck as the app closes.
        let keep = |job: FileJob| do_file_job(&note::notes_dir(), job, Turn::Moment);
        match save_note(&note::notes_dir(), save, &text, picture, page, &make_png, &keep) {
            Ok(name) => job.done(&format!("Saved as {name}")),
            Err(e) => job.failed(&format!("Couldn't save it: {e}. Your words are kept for the next F12.")),
        }
    });
    if let Err(e) = spawned {
        // The closure (and its SaveDone) never ran, so the save is ended here, on the window's thread: its words go to
        // the "Note files" thread and the log before they're let go, and only then does its SaveDone end it.
        on_files(FileJob::KeepFailed { words: words.clone(), shown: save.shown });
        log::error("Note", &format!("Couldn't start saving the note: {e}\n{words}"));
        words_on_disk(save.number);
        drop(SaveDone(save));
    }
}

/// A save (`save`): the words `text`, typed on the page `page`, saved as a note in `folder`, with `picture` made into a
/// PNG by `make_png` only once the words are on disk (a big window takes a moment). Gives the note's file name, or why
/// it couldn't be saved: then its words go in the session log and are kept for the next F12, as a save that failed,
/// through `keep`. Either way they're in a file before they stop being kept for the end of the run ([`BEING_SAVED`]),
/// so they're always in one or the other.
fn save_note(
    folder: &Path,
    save: SaveOf,
    text: &str,
    picture: Option<Picture>,
    page: &str,
    make_png: &dyn Fn(&Picture) -> Result<Vec<u8>, String>,
    keep: &dyn Fn(FileJob),
) -> Result<String, String> {
    // `note::save` calls it once the words are on disk.
    let picture_after = picture.map(|p| {
        move || {
            words_on_disk(save.number);
            make_png(&p)
        }
    });
    let recent = log::current().map(|l| l.recent()).unwrap_or_default();
    let version = mo_app::version::version();
    let n = note::Note {
        text,
        page,
        version: &version,
        log_path: log::current().and_then(|l| l.path()),
        recent_log: &recent,
        picture: picture_after.as_ref().map(|make| make as &dyn Fn() -> Result<Vec<u8>, String>),
    };
    match note::save(folder, &n, chrono::Local::now()) {
        Ok(path) => {
            words_on_disk(save.number);
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            log::info("Note", &format!("Saved {name}"));
            Ok(name)
        }
        Err(e) => {
            log::error("Note", &format!("Couldn't save the note: {e}\n{text}"));
            keep(FileJob::KeepFailed { words: text.to_owned(), shown: save.shown });
            words_on_disk(save.number);
            Err(e.to_string())
        }
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

    /// The words typed, the draft's turn and the note's state are the app's own, one of each, so these tests take
    /// turns too. Each starts as the app does: no words waiting, nothing read, no note opened or written yet, none being
    /// saved, the app not ending.
    fn one_at_a_time() -> MutexGuard<'static, ()> {
        static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
        let one = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        *UNSAVED.lock().unwrap_or_else(PoisonError::into_inner) = None;
        *DRAFT_TURN.lock().unwrap_or_else(PoisonError::into_inner) = None;
        BEING_SAVED.lock().unwrap_or_else(PoisonError::into_inner).clear();
        FILLED.store(true, Ordering::Release);
        OPEN.store(false, Ordering::Release);
        ENDING.store(false, Ordering::Release);
        OPENINGS.store(0, Ordering::Release);
        IN_THE_DRAFT.store(0, Ordering::Release);
        SAVING.store(0, Ordering::Release);
        one
    }

    /// A notes folder of the test's own: tests never touch the app's data folder.
    fn temp_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-ui-note-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// As the window's thread does when the words change, in note number `opening`, which showed the text from before
    /// by then (`filled`) or not.
    fn typed_in(words: &str, opening: usize, filled: bool) {
        *UNSAVED.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(Typed { words: words.to_owned(), opening, filled });
    }

    /// As the window's thread does when the words change, in a note that shows the text from before.
    fn typed(words: &str) {
        typed_in(words, 1, true);
    }

    /// The words waiting in `UNSAVED`.
    fn waiting() -> Option<String> {
        UNSAVED.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|t| t.words.clone())
    }

    #[test]
    fn a_crash_keeps_the_words_before_it_returns_waiting_only_a_moment_for_its_turn() {
        let _one = one_at_a_time();
        let dir = temp_folder("crash");
        typed("half a thought");
        // The "Note files" thread is part way through the draft, stuck on a disk that's asleep: it's its turn.
        let stuck = DRAFT_TURN.lock().unwrap_or_else(PoisonError::into_inner);
        // A crash the handler can't tell will end the app (std may end it straight after).
        let (done, finished) = channel();
        let folder = dir.clone();
        std::thread::spawn(move || {
            keep_after_crash(&folder, false, &|| {});
            let _ = done.send(());
        });
        let returned = finished.recv_timeout(Duration::from_secs(30));
        let kept = note::read_draft(&dir);
        drop(stuck);
        assert!(returned.is_ok(), "it waits only a moment for its turn");
        assert_eq!(kept.as_deref(), Some("half a thought"), "the words are on disk before it returns");
        // Written without its turn: the stuck write may yet land older words over them, so they stay for the next
        // write in its turn.
        assert_eq!(waiting().as_deref(), Some("half a thought"), "they wait for a write in its turn");
        keep_unsaved(&dir, Turn::Wait);
        assert_eq!(waiting(), None, "written in its turn, they're done with");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_writers_take_turns_so_the_newest_words_stay() {
        let _one = one_at_a_time();
        let dir = temp_folder("turns");
        typed("the newest words");
        // The "Note files" thread has its turn, and is writing the words from before.
        let turn = DRAFT_TURN.lock().unwrap_or_else(PoisonError::into_inner);
        // Meanwhile another thread crashes, and keeps the newest words.
        let folder = dir.clone();
        let crash = std::thread::spawn(move || keep_after_crash(&folder, false, &|| {}));
        std::thread::sleep(Duration::from_millis(50));
        assert!(note::keep_draft(&dir, "the words from before"));
        drop(turn);
        crash.join().expect("no crash in keeping the words");
        assert_eq!(note::read_draft(&dir).as_deref(), Some("the newest words"), "written in its turn, last");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_job_that_cant_wait_for_its_turn_leaves_the_words_for_the_next_write() {
        let _one = one_at_a_time();
        let dir = temp_folder("never");
        typed("first words");
        keep_unsaved(&dir, Turn::Wait);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("first words"));
        assert_eq!(waiting(), None, "on disk, so done with");
        // When it isn't its turn, a job done on the window's thread leaves the words for the next try.
        typed("more words");
        let turn = DRAFT_TURN.lock().unwrap_or_else(PoisonError::into_inner);
        keep_unsaved(&dir, Turn::Never);
        drop(turn);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("first words"));
        assert_eq!(waiting().as_deref(), Some("more words"));
        keep_unsaved(&dir, Turn::Wait);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("more words"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn words_that_couldnt_be_written_wait_for_the_next_try() {
        let _one = one_at_a_time();
        let dir = temp_folder("unwritable");
        // Something in the way of the draft that isn't a file (as bad as a full disk): it can't be written.
        let in_the_way = dir.join(note::DRAFT_PART);
        std::fs::create_dir_all(in_the_way.join("in the way")).expect("made");
        typed("words to keep");
        keep_unsaved(&dir, Turn::Wait);
        assert_eq!(note::read_draft(&dir), None);
        assert_eq!(waiting().as_deref(), Some("words to keep"), "not on disk, so still kept here");
        std::fs::remove_dir_all(&in_the_way).expect("removable");
        keep_unsaved(&dir, Turn::Wait);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("words to keep"));
        assert_eq!(waiting(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn words_go_with_the_draft_as_their_note_stood_when_they_were_typed() {
        let _one = one_at_a_time();
        let dir = temp_folder("as-typed");
        assert!(note::keep_draft(&dir, "from before"));
        // Typed before their note showed the text from before, and only then written, after it had shown it: they go
        // before the text from before (where the note shows them), not over it.
        typed_in("hi", 1, false);
        FILLED.store(true, Ordering::Release);
        keep_unsaved(&dir, Turn::Wait);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("hi\n\nfrom before"));
        // Typed in a note that showed it (so they hold it), and only written once the next note had opened: they go
        // over the draft, not before it again.
        typed_in("hi there\n\nfrom before", 1, true);
        FILLED.store(false, Ordering::Release);
        keep_unsaved(&dir, Turn::Wait);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("hi there\n\nfrom before"));
        // Saved or cancelled before it showed the draft from before: that draft stays, though the next note shows it.
        FILLED.store(true, Ordering::Release);
        do_file_job(&dir, FileJob::ClearDraft { filled: false, opening: 1 }, Turn::Wait);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("hi there\n\nfrom before"));
        do_file_job(&dir, FileJob::ClearDraft { filled: true, opening: 1 }, Turn::Wait);
        assert_eq!(note::read_draft(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn words_kept_before_their_note_shows_the_text_from_before_show_once() {
        let _one = one_at_a_time();
        let dir = temp_folder("shown-once");
        assert!(note::keep_draft(&dir, "from before"));
        // F12 while the "Note files" thread is busy: "hi" is typed before the note shows the text from before, and a
        // crash keeps it, before that text, before the note's Fill job has read it.
        OPENINGS.store(7, Ordering::Release);
        FILLED.store(false, Ordering::Release);
        *UNSAVED.lock().unwrap_or_else(PoisonError::into_inner) = Some(typed_now("hi".to_owned()));
        keep_after_crash(&dir, false, &|| {});
        assert_eq!(note::read_draft(&dir).as_deref(), Some("hi\n\nfrom before"));
        // The Fill job then gets the text the crash read, not the draft with "hi" in it already, and the note shows
        // "hi" once, with that text after it.
        let text = text_for_note(&dir, 7, Turn::Wait);
        assert_eq!(text, "from before");
        assert_eq!(fill_lands(7, "hi", text).as_deref(), Some("hi\n\nfrom before"));
        assert!(FILLED.load(Ordering::Acquire));
        assert_eq!(waiting().as_deref(), Some("hi\n\nfrom before"), "what it shows now is kept as typed in it");
        // Words typed before it showed, written once it had: still the same text, once.
        typed_in("hi there", 7, false);
        keep_unsaved(&dir, Turn::Wait);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("hi there\n\nfrom before"));
        // The next note reads the text from before afresh.
        OPENINGS.store(8, Ordering::Release);
        assert_eq!(text_for_note(&dir, 8, Turn::Wait), "hi there\n\nfrom before");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_words_of_failed_saves_are_kept_and_let_go_in_the_drafts_turn() {
        let _one = one_at_a_time();
        let dir = temp_folder("failed");
        std::fs::create_dir_all(&dir).expect("made");
        let failed = dir.join(FAILED_FILE);
        let turn = DRAFT_TURN.lock().unwrap_or_else(PoisonError::into_inner);
        // The "Note files" thread waits for its turn to keep them.
        let folder = dir.clone();
        let keep = std::thread::spawn(move || {
            do_file_job(&folder, FileJob::KeepFailed { words: "lost?".into(), shown: None }, Turn::Wait)
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(!failed.exists(), "it waits for its turn to keep them");
        drop(turn);
        keep.join().expect("kept");
        assert_eq!(std::fs::read_to_string(&failed).ok().as_deref(), Some("lost?\n\n"));
        // The next note shows them; once the draft holds them, written in its turn, their file goes.
        OPENINGS.store(3, Ordering::Release);
        FILLED.store(false, Ordering::Release);
        let text = text_for_note(&dir, 3, Turn::Wait);
        assert_eq!(text, "lost?");
        assert_eq!(fill_lands(3, "", text).as_deref(), Some("lost?"));
        assert!(failed.exists(), "not until the draft holds them");
        keep_unsaved(&dir, Turn::Wait);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("lost?"));
        assert!(!failed.exists(), "the draft holds them now");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_saves_words_go_only_once_the_draft_holds_them_and_never_newer_ones() {
        let _one = one_at_a_time();
        let dir = temp_folder("failed-newer");
        std::fs::create_dir_all(&dir).expect("made");
        let failed = dir.join(FAILED_FILE);
        keep_failed_words(&dir, "first note");
        // The note shows them, but the draft can't be written (as bad as a full disk): they stay.
        OPENINGS.store(1, Ordering::Release);
        FILLED.store(false, Ordering::Release);
        let text = text_for_note(&dir, 1, Turn::Wait);
        assert!(fill_lands(1, "", text).is_some());
        let in_the_way = dir.join(note::DRAFT_PART).join("in the way");
        std::fs::create_dir_all(&in_the_way).expect("made");
        assert_eq!(keep_unsaved(&dir, Turn::Wait), Kept::NotOnDisk("first note".into()));
        assert!(failed.exists(), "the draft doesn't hold them, so they stay");
        std::fs::remove_dir_all(dir.join(note::DRAFT_PART)).expect("removable");
        // Another save fails meanwhile: its words are added after the note read the file.
        keep_failed_words(&dir, "second note");
        assert_eq!(keep_unsaved(&dir, Turn::Wait), Kept::OnDisk);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("first note"));
        let left = std::fs::read_to_string(&failed).expect("still there");
        assert!(left.contains("second note"), "words the note never showed stay: {left}");
        // Typed before the next note shows its text from before, and written then: the same, they go only while the
        // file holds just what was read for the note.
        OPENINGS.store(2, Ordering::Release);
        FILLED.store(false, Ordering::Release);
        *UNSAVED.lock().unwrap_or_else(PoisonError::into_inner) = Some(typed_now("hi".to_owned()));
        // Read for the note (by its Fill job), then a third save fails before the words are written.
        let read = text_for_note(&dir, 2, Turn::Wait);
        assert!(read.contains("second note") && read.contains("first note"), "{read}");
        keep_failed_words(&dir, "third note");
        assert_eq!(keep_unsaved(&dir, Turn::Wait), Kept::OnDisk);
        let left = std::fs::read_to_string(&failed).expect("still there");
        assert!(left.contains("third note"), "{left}");
        // Saved or cancelled once it showed them: they've done their job, unless newer ones came.
        let _ = std::fs::remove_file(&failed);
        keep_failed_words(&dir, "fourth note");
        OPENINGS.store(4, Ordering::Release);
        FILLED.store(false, Ordering::Release);
        let text = text_for_note(&dir, 4, Turn::Wait);
        assert!(fill_lands(4, "", text).is_some());
        do_file_job(&dir, FileJob::ClearDraft { filled: true, opening: 4 }, Turn::Wait);
        assert!(!failed.exists(), "shown in the note, which was saved or cancelled");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crash_the_app_goes_on_after_keeps_the_words_and_puts_the_lock_right() {
        let _one = one_at_a_time();
        let dir = temp_folder("goes-on");
        typed("words");
        let lock_updates = std::cell::Cell::new(0);
        keep_after_crash(&dir, false, &|| lock_updates.set(lock_updates.get() + 1));
        assert_eq!(note::read_draft(&dir).as_deref(), Some("words"), "on disk before it returns");
        assert_eq!(lock_updates.get(), 1, "the lock is put as it should be");
        assert!(!ENDING.load(Ordering::Acquire), "the app goes on");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn once_the_app_is_ending_the_lock_only_comes_down() {
        let _one = one_at_a_time();
        let dir = temp_folder("ending");
        // A note still open as the app ends.
        OPEN.store(true, Ordering::Release);
        do_file_job(&dir, FileJob::Lock, Turn::Wait);
        assert!(dir.join(note::WRITING_FILE).exists(), "up while a note is open");
        typed("last words");
        let lock_updates = std::cell::Cell::new(0);
        keep_after_crash(&dir, true, &|| lock_updates.set(lock_updates.get() + 1));
        assert_eq!(note::read_draft(&dir).as_deref(), Some("last words"));
        assert!(!dir.join(FAILED_FILE).exists(), "written in its turn: kept once");
        assert!(!dir.join(note::WRITING_FILE).exists(), "down as the app ends");
        assert_eq!(lock_updates.get(), 0);
        // A save still going, or a crash as the app ends, then puts the lock as it should be: it stays down.
        do_file_job(&dir, FileJob::Lock, Turn::Wait);
        assert!(!dir.join(note::WRITING_FILE).exists(), "it stays down once the app is ending");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_note_put_aside_opens_again_with_its_words_while_they_arent_on_disk() {
        let _one = one_at_a_time();
        let dir = temp_folder("again");
        assert!(note::keep_draft(&dir, "from before"));
        // The writes each opening asks for.
        let writes = std::cell::Cell::new(0);
        let send = |job: FileJob| {
            if matches!(job, FileJob::KeepDraft) {
                writes.set(writes.get() + 1);
            }
        };
        // F12: a new note, empty until the text from before comes.
        let first = opening_note(&send);
        assert_eq!((first.opening, first.words.as_str(), first.filled, first.again), (1, "", false, false));
        assert_eq!(writes.get(), 0, "a new note has nothing to write yet");
        assert!(!FILLED.load(Ordering::Acquire));
        // Typed, then Esc straight after: the "Note files" thread is behind, so the words aren't on disk yet.
        *UNSAVED.lock().unwrap_or_else(PoisonError::into_inner) = Some(typed_now("the rail jumps".to_owned()));
        assert!(matches!(closing(false), FileJob::KeepDraft));
        assert_eq!(waiting().as_deref(), Some("the rail jumps"), "put aside: kept");
        // F12 again before that write: the same note opens again, with its words, so the next ones typed can't take
        // their place.
        let again = opening_note(&send);
        assert_eq!(
            (again.opening, again.words.as_str(), again.filled, again.again),
            (1, "the rail jumps", false, true)
        );
        assert_eq!(OPENINGS.load(Ordering::Acquire), 1, "no new note");
        assert_eq!(writes.get(), 1, "its words' write is asked for again: the one that should have kept them may fail");
        *UNSAVED.lock().unwrap_or_else(PoisonError::into_inner) = Some(typed_now("the rail jumps x".to_owned()));
        // The write put aside, then the text from before reaching the note: everything once, nothing lost.
        assert_eq!(keep_unsaved(&dir, Turn::Wait), Kept::OnDisk);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("the rail jumps x\n\nfrom before"));
        let text = text_for_note(&dir, 1, Turn::Wait);
        assert_eq!(text, "from before", "the text read for that note, not the draft with its words in it");
        assert_eq!(fill_lands(1, "the rail jumps x", text.clone()).as_deref(), Some("the rail jumps x\n\nfrom before"));
        // A second Fill for it (the first was still on its way) adds nothing.
        assert_eq!(fill_lands(1, "the rail jumps x\n\nfrom before", text), None);
        // Once its words are on disk, the next F12 is a new note.
        assert_eq!(keep_unsaved(&dir, Turn::Wait), Kept::OnDisk);
        assert_eq!(waiting(), None);
        let next = opening_note(&send);
        assert_eq!((next.opening, next.again), (2, false));
        assert_eq!(writes.get(), 1);
        // A note put aside after it showed the text from before opens again showing it: no second Fill.
        FILLED.store(true, Ordering::Release);
        *UNSAVED.lock().unwrap_or_else(PoisonError::into_inner) = Some(typed_now("all of it".to_owned()));
        let shown = opening_note(&send);
        assert_eq!((shown.opening, shown.words.as_str(), shown.filled), (2, "all of it", true));
        assert!(FILLED.load(Ordering::Acquire));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_is_typed_and_the_close_carry_how_the_note_stood() {
        let _one = one_at_a_time();
        OPENINGS.store(5, Ordering::Release);
        FILLED.store(false, Ordering::Release);
        let early = typed_now("a".to_owned());
        assert_eq!((early.opening, early.filled), (5, false));
        FILLED.store(true, Ordering::Release);
        assert!(typed_now("a".to_owned()).filled);
        // Saved or cancelled before it showed the draft from before: that draft isn't thrown away.
        FILLED.store(false, Ordering::Release);
        *UNSAVED.lock().unwrap_or_else(PoisonError::into_inner) = Some(early);
        assert!(matches!(closing(true), FileJob::ClearDraft { filled: false, opening: 5 }));
        assert_eq!(waiting(), None, "saved or cancelled: its words have done their job");
        FILLED.store(true, Ordering::Release);
        assert!(matches!(closing(true), FileJob::ClearDraft { filled: true, opening: 5 }));
        // A save says the same, for its words if it fails.
        assert_eq!(save_of(9).shown, Some(5));
        FILLED.store(false, Ordering::Release);
        assert_eq!(save_of(9).shown, None);
        FILLED.store(true, Ordering::Release);
        // A late Fill for an earlier note changes nothing.
        FILLED.store(false, Ordering::Release);
        assert_eq!(fill_lands(4, "", "old".to_owned()), None);
        assert!(!FILLED.load(Ordering::Acquire));
        // No text from before: it shows, as nothing.
        assert_eq!(fill_lands(5, "x", String::new()), None);
        assert!(FILLED.load(Ordering::Acquire));
    }

    #[test]
    fn the_close_says_the_app_is_ending_so_the_lock_stays_down() {
        let _one = one_at_a_time();
        let dir = temp_folder("close");
        OPEN.store(true, Ordering::Release);
        do_file_job(&dir, FileJob::Lock, Turn::Wait);
        assert!(dir.join(note::WRITING_FILE).exists(), "up while a note is open");
        typed("last words");
        finish_up_in(&dir, &|job| do_file_job(&dir, job, Turn::Wait));
        assert!(ENDING.load(Ordering::Acquire));
        assert_eq!(note::read_draft(&dir).as_deref(), Some("last words"));
        assert!(!dir.join(FAILED_FILE).exists(), "written in its turn: kept once");
        assert!(!dir.join(note::WRITING_FILE).exists(), "down as the app closes");
        // A save still going then asks for the lock as it should be: it stays down.
        do_file_job(&dir, FileJob::Lock, Turn::Wait);
        assert!(!dir.join(note::WRITING_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crash_never_starts_the_note_files_thread() {
        let _one = one_at_a_time();
        // No test hands a job to the real "Note files" thread (it works in the app's own notes folder), so it was
        // never started here: a crash mustn't start it.
        update_lock_after_crash();
        assert!(FILES.get().is_none());
    }

    #[test]
    fn words_written_without_their_turn_as_the_app_ends_go_with_the_failed_saves_words_too() {
        let _one = one_at_a_time();
        let dir = temp_folder("end-alone");
        std::fs::create_dir_all(&dir).expect("made");
        typed("the newest words");
        // The "Note files" thread is stuck part way through its part file, in its turn.
        std::fs::write(dir.join(note::DRAFT_PART), "half wri").expect("writable");
        let stuck = DRAFT_TURN.lock().unwrap_or_else(PoisonError::into_inner);
        let folder = dir.clone();
        let crash = std::thread::spawn(move || keep_after_crash(&folder, true, &|| {}));
        crash.join().expect("no crash in keeping the words");
        drop(stuck);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("the newest words"), "written through a part of its own");
        assert_eq!(std::fs::read_to_string(dir.join(note::DRAFT_PART)).ok().as_deref(), Some("half wri"));
        // That stuck write may yet land over them, and nothing comes after: they're with the failed saves' words too.
        let failed = std::fs::read_to_string(dir.join(FAILED_FILE)).expect("there");
        assert_eq!(failed, "the newest words\n\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_words_of_a_save_not_on_disk_yet_are_kept_as_the_app_ends() {
        let _one = one_at_a_time();
        let dir = temp_folder("being-saved");
        std::fs::create_dir_all(&dir).expect("made");
        let failed = dir.join(FAILED_FILE);
        // The app closes while a save is stuck before its words are on disk.
        let stuck = being_saved("stuck words");
        let done = being_saved("done words");
        assert_eq!(words_on_disk(done).as_deref(), Some("done words"));
        SAVING.store(2, Ordering::Release);
        wait_for_saves_in(&dir, Duration::ZERO);
        assert_eq!(std::fs::read_to_string(&failed).ok().as_deref(), Some("stuck words\n\n"));
        assert_eq!(words_on_disk(stuck), None, "kept once");
        // A crash that ends the app does the same.
        let _ = std::fs::remove_file(&failed);
        being_saved("crash words");
        keep_after_crash(&dir, true, &|| {});
        assert_eq!(std::fs::read_to_string(&failed).ok().as_deref(), Some("crash words\n\n"));
        // A save that stops part way (a crash on its thread) keeps its words too, while they're still kept for the end
        // of the run, so they're always in one or the other; and one that ended doesn't.
        let _ = std::fs::remove_file(&failed);
        let part_way = SaveOf { number: being_saved("part way"), shown: None };
        let keep = |job: FileJob| {
            assert_eq!(still_being_saved(part_way.number).as_deref(), Some("part way"), "still kept for the end");
            do_file_job(&dir, job, Turn::Wait);
        };
        save_ended(part_way, &keep);
        assert_eq!(std::fs::read_to_string(&failed).ok().as_deref(), Some("part way\n\n"));
        assert_eq!(still_being_saved(part_way.number), None, "kept once");
        let ended = SaveOf { number: being_saved("ended"), shown: None };
        words_on_disk(ended.number);
        save_ended(ended, &|job| do_file_job(&dir, job, Turn::Wait));
        assert_eq!(std::fs::read_to_string(&failed).ok().as_deref(), Some("part way\n\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_last_draft_that_cant_be_written_keeps_its_words_with_the_failed_saves() {
        let _one = one_at_a_time();
        let dir = temp_folder("end-unwritable");
        // Something in the way of the draft that isn't a file (as bad as a full disk, or a scanner holding it): the
        // last write, in its turn, can't be done, and nothing comes after to try again.
        std::fs::create_dir_all(dir.join(note::DRAFT_PART).join("in the way")).expect("made");
        typed("last words");
        finish_up_in(&dir, &|job| do_file_job(&dir, job, Turn::Wait));
        assert_eq!(note::read_draft(&dir), None);
        assert_eq!(std::fs::read_to_string(dir.join(FAILED_FILE)).ok().as_deref(), Some("last words\n\n"));
        // A crash that ends the app does the same.
        let _ = std::fs::remove_file(dir.join(FAILED_FILE));
        ENDING.store(false, Ordering::Release);
        typed("crash words");
        keep_after_crash(&dir, true, &|| {});
        assert_eq!(std::fs::read_to_string(dir.join(FAILED_FILE)).ok().as_deref(), Some("crash words\n\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_earlier_notes_clearing_leaves_a_newer_notes_words_written_ahead_of_it() {
        let _one = one_at_a_time();
        let dir = temp_folder("clear-newer");
        assert!(note::keep_draft(&dir, "note one"));
        // Note 1 showed its draft and was cancelled, but its ClearDraft is still waiting: the "Note files" thread is
        // behind. Note 2 opens, "the rail jumps" is typed, and the note is put aside before its Fill lands.
        OPENINGS.store(2, Ordering::Release);
        FILLED.store(false, Ordering::Release);
        *UNSAVED.lock().unwrap_or_else(PoisonError::into_inner) = Some(typed_now("the rail jumps".to_owned()));
        // A crash the app goes on after writes them first, in its turn, so they're done with in memory.
        keep_after_crash(&dir, false, &|| {});
        assert_eq!(waiting(), None);
        // Then note 1's clearing comes: the draft holds a newer note's words, so it stays.
        do_file_job(&dir, FileJob::ClearDraft { filled: true, opening: 1 }, Turn::Wait);
        let left = note::read_draft(&dir).unwrap_or_default();
        assert!(left.starts_with("the rail jumps"), "not lost: {left:?}");
        // A write asked for before note 1 closed, done only now, writes the newest words too: the same.
        let _ = std::fs::remove_dir_all(&dir);
        IN_THE_DRAFT.store(0, Ordering::Release);
        *DRAFT_TURN.lock().unwrap_or_else(PoisonError::into_inner) = None;
        assert!(note::keep_draft(&dir, "note one"));
        *UNSAVED.lock().unwrap_or_else(PoisonError::into_inner) = Some(typed_now("more words".to_owned()));
        do_file_job(&dir, FileJob::KeepDraft, Turn::Wait);
        do_file_job(&dir, FileJob::ClearDraft { filled: true, opening: 1 }, Turn::Wait);
        assert_eq!(note::read_draft(&dir).as_deref(), Some("more words\n\nnote one"), "shown again, never lost");
        // The newer note's own clearing still clears it.
        do_file_job(&dir, FileJob::ClearDraft { filled: true, opening: 2 }, Turn::Wait);
        assert_eq!(note::read_draft(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_late_fill_for_an_earlier_note_leaves_what_was_read_for_the_newer_one() {
        let _one = one_at_a_time();
        let dir = temp_folder("late-fill");
        assert!(note::keep_draft(&dir, "from before"));
        // Note 1's Fill is still on its way; note 2 is open, "hi" is typed, and a crash keeps it before the text from
        // before, in its turn.
        OPENINGS.store(2, Ordering::Release);
        FILLED.store(false, Ordering::Release);
        *UNSAVED.lock().unwrap_or_else(PoisonError::into_inner) = Some(typed_now("hi".to_owned()));
        keep_after_crash(&dir, false, &|| {});
        assert_eq!(note::read_draft(&dir).as_deref(), Some("hi\n\nfrom before"));
        // Note 1's Fill comes late: nothing for it, and nothing read again.
        assert_eq!(text_for_note(&dir, 1, Turn::Wait), "");
        // Note 2's Fill still gets the text read for it, so "hi" shows once.
        let text = text_for_note(&dir, 2, Turn::Wait);
        assert_eq!(fill_lands(2, "hi", text).as_deref(), Some("hi\n\nfrom before"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_save_lets_go_of_the_failed_saves_words_it_showed_whichever_job_comes_first() {
        for clear_first in [false, true] {
            let _one = one_at_a_time();
            let dir = temp_folder(&format!("failed-shown-{clear_first}"));
            std::fs::create_dir_all(&dir).expect("made");
            keep_failed_words(&dir, "lost before");
            // Note 1 shows them; its save fails, with them in its words.
            OPENINGS.store(1, Ordering::Release);
            FILLED.store(false, Ordering::Release);
            let text = text_for_note(&dir, 1, Turn::Wait);
            let shown = fill_lands(1, "", text).expect("shown");
            let words = format!("{shown}\n\nand more");
            let clear = || do_file_job(&dir, FileJob::ClearDraft { filled: true, opening: 1 }, Turn::Wait);
            if clear_first {
                clear();
            }
            do_file_job(&dir, FileJob::KeepFailed { words, shown: Some(1) }, Turn::Wait);
            if !clear_first {
                clear();
            }
            let failed = std::fs::read_to_string(dir.join(FAILED_FILE)).expect("there");
            assert_eq!(failed, "lost before\n\nand more\n\n", "once, clearing first: {clear_first}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn a_saves_words_are_in_a_file_before_they_stop_being_kept_for_the_end_of_the_run() {
        let _one = one_at_a_time();
        let dir = temp_folder("save");
        let kept = std::cell::RefCell::new(Vec::new());
        // Saved: the picture is made only once the words are on disk, and they're let go then.
        let save = SaveOf { number: being_saved("the rail jumps"), shown: None };
        let made = std::cell::Cell::new(false);
        let make_png = |_: &Picture| {
            made.set(true);
            let mds = std::fs::read_dir(&dir).expect("there").flatten();
            let on_disk = mds
                .filter(|e| e.file_name().to_string_lossy().ends_with(".md"))
                .any(|e| std::fs::read_to_string(e.path()).is_ok_and(|md| md.contains("the rail jumps")));
            assert!(on_disk, "the words are on disk before the picture is made");
            assert_eq!(still_being_saved(save.number), None, "and no longer kept for the end of the run");
            Err("no picture today".to_owned())
        };
        let keep = |job: FileJob| {
            if let FileJob::KeepFailed { words, .. } = job {
                kept.borrow_mut().push(words);
            }
        };
        let picture = Picture { width: 1, height: 1, rgba: vec![0; 4] };
        let name = save_note(&dir, save, "the rail jumps", Some(picture), "Songs", &make_png, &keep).expect("saved");
        assert!(made.get());
        assert!(name.starts_with("note-") && name.ends_with(".md"), "{name}");
        assert!(kept.borrow().is_empty());
        // It can't be saved (a file where the folder should be): its words are kept for the next F12 while they're
        // still kept for the end of the run, and let go only then.
        let blocked = dir.join("not a folder");
        std::fs::write(&blocked, "a file").expect("writable");
        let save = SaveOf { number: being_saved("half a thought"), shown: Some(4) };
        let keep = |job: FileJob| {
            if let FileJob::KeepFailed { words, shown } = job {
                assert_eq!(still_being_saved(save.number).as_deref(), Some("half a thought"), "still kept for the end");
                assert_eq!(shown, Some(4));
                kept.borrow_mut().push(words);
            }
        };
        assert!(save_note(&blocked, save, "half a thought", None, "Songs", &make_png, &keep).is_err());
        assert_eq!(*kept.borrow(), ["half a thought"]);
        assert_eq!(still_being_saved(save.number), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
