//! F12: the note to Claude. The picture is taken before the dialog opens, so it shows the page, not the dialog. What's
//! typed is kept in the draft file a moment after each pause in typing (and straight away if the app closes or
//! crashes first), and saving happens off the window's thread, ending in an activity card that says where it went.
//!
//! Only the Cancel button throws the words away. Esc or a click beside the dialog just puts the note aside: the next
//! F12 offers it back. After Save, the draft and `writing.lock` stay until the note is really on disk, and the app
//! waits a few seconds for a save still going before it closes.

use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::thread::JoinHandle;
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

/// Typed but not yet in the draft file.
static UNSAVED: Mutex<Option<String>> = Mutex::new(None);
const DRAFT_PAUSE: Duration = Duration::from_millis(300);
/// Counts the notes opened, so a save that ends after a new note was opened leaves that note's draft and lock alone.
static OPENED: AtomicU64 = AtomicU64::new(0);
/// Saves still going.
static SAVING: AtomicUsize = AtomicUsize::new(0);
static SAVES: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

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
    let folder = note::notes_dir();
    OPENED.fetch_add(1, Ordering::AcqRel);
    // While a save is still going, the draft is the note being saved, not one to offer back.
    let draft = if SAVING.load(Ordering::Acquire) > 0 { None } else { note::read_draft(&folder) };
    app.set_note_text(draft.unwrap_or_default().into());
    note::writing(&folder, true);
    app.set_note_open(true);
}

pub fn connect(app: &AppWindow, feed: &Feed) {
    app.on_note_edited(|text| {
        *UNSAVED.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.to_string());
        DRAFT_TIMER.with(|t| t.start(slint::TimerMode::SingleShot, DRAFT_PAUSE, keep_unsaved));
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
        // The words stay in the draft, and the lock stays up, until the save has finished.
        DRAFT_TIMER.with(Timer::stop);
        UNSAVED.lock().unwrap_or_else(|e| e.into_inner()).take();
        note::keep_draft(&note::notes_dir(), &text);
        app.set_note_open(false);
        app.set_note_text("".into());
        save_in_background(&feed, text, picture);
    });
}

/// Writes what's typed but not yet kept. Safe from any thread, and from a crash: it never waits for the lock.
fn keep_unsaved() {
    let text = match UNSAVED.try_lock() {
        Ok(mut t) => t.take(),
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner().take(),
        Err(std::sync::TryLockError::WouldBlock) => None,
    };
    if let Some(text) = text {
        note::keep_draft(&note::notes_dir(), &text);
    }
}

/// For the app closing: waits up to a few seconds for notes still being saved.
pub fn wait_for_saves() {
    let until = Instant::now() + Duration::from_secs(5);
    let saves = std::mem::take(&mut *SAVES.lock().unwrap_or_else(|e| e.into_inner()));
    for save in saves {
        while !save.is_finished() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        if save.is_finished() {
            let _ = save.join();
        } else {
            log::warning("Note", "A note was still being saved when the app closed; its words are kept as the draft.");
        }
    }
}

/// For the app closing or crashing: keeps a note that's open as a draft, and takes down this app's `writing.lock` so
/// the launcher doesn't wait for it.
pub fn finish_up() {
    keep_unsaved();
    note::writing(&note::notes_dir(), false);
}

/// Closes the note. Saved or cancelled, the draft has done its job; put aside, it stays for next time.
fn close(app: &AppWindow, done_with: bool) {
    DRAFT_TIMER.with(Timer::stop);
    PICTURE.with(|p| p.borrow_mut().take());
    let folder = note::notes_dir();
    if done_with {
        UNSAVED.lock().unwrap_or_else(|e| e.into_inner()).take();
        note::clear_draft(&folder);
    } else {
        keep_unsaved();
    }
    note::writing(&folder, false);
    app.set_note_open(false);
    app.set_note_text("".into());
}

fn save_in_background(feed: &Feed, text: String, picture: Option<Picture>) {
    let page = crate::page_name();
    let opened = OPENED.load(Ordering::Acquire);
    let job = feed.start("Saving your note", "");
    // If the saving can't even start, the words still reach the log.
    let words = text.clone();
    SAVING.fetch_add(1, Ordering::AcqRel);
    let spawned = std::thread::Builder::new().name("Note".into()).spawn(move || {
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
        let folder = note::notes_dir();
        let saved = note::save(&folder, &n, chrono::Local::now());
        // Only when no new note was opened meanwhile: that one owns the draft and the lock now.
        let still_ours = OPENED.load(Ordering::Acquire) == opened;
        match saved {
            Ok(path) => {
                if still_ours {
                    note::clear_draft(&folder);
                }
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                log::info("Note", &format!("Saved {name}"));
                job.done(&format!("Saved as {name}"));
            }
            Err(e) => {
                // The words stay in the draft and go into the log, so they're not lost when the folder can't be written.
                log::error("Note", &format!("Couldn't save the note: {e}\n{text}"));
                job.failed(&format!("Couldn't save it: {e}. Your words are kept for the next F12."));
            }
        }
        if still_ours {
            note::writing(&folder, false);
        }
        SAVING.fetch_sub(1, Ordering::AcqRel);
    });
    match spawned {
        Ok(handle) => SAVES.lock().unwrap_or_else(|e| e.into_inner()).push(handle),
        Err(e) => {
            SAVING.fetch_sub(1, Ordering::AcqRel);
            note::writing(&note::notes_dir(), false);
            log::error("Note", &format!("Couldn't start saving the note: {e}\n{words}"));
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
