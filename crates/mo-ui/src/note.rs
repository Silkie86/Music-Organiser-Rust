//! F12: the note to Claude. The picture is taken before the dialog opens, so it shows the page, not the dialog. What's
//! typed is kept in the draft file half a second after each pause in typing, and saving happens off the window's
//! thread, ending in an activity card that says where the note went.

use std::cell::RefCell;
use std::time::Duration;

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
    app.set_note_text(note::read_draft(&folder).unwrap_or_default().into());
    note::writing(&folder, true);
    app.set_note_open(true);
}

pub fn connect(app: &AppWindow, feed: &Feed) {
    app.on_note_edited(|text| {
        let text = text.to_string();
        DRAFT_TIMER.with(|t| {
            t.start(slint::TimerMode::SingleShot, Duration::from_millis(500), move || {
                note::keep_draft(&note::notes_dir(), &text);
            })
        });
    });

    let weak = app.as_weak();
    app.on_note_cancel(move || {
        if let Some(app) = weak.upgrade() {
            close(&app);
        }
    });

    let weak = app.as_weak();
    let feed = feed.clone();
    app.on_note_save(move || {
        let Some(app) = weak.upgrade() else { return };
        let text = app.get_note_text().to_string();
        let picture = PICTURE.with(|p| p.borrow_mut().take()).filter(|_| app.get_note_include_picture());
        close(&app);
        save_in_background(&feed, text, picture);
    });
}

/// Closes the note, saved or cancelled. Either way the draft has done its job.
fn close(app: &AppWindow) {
    DRAFT_TIMER.with(Timer::stop);
    PICTURE.with(|p| p.borrow_mut().take());
    let folder = note::notes_dir();
    note::clear_draft(&folder);
    note::writing(&folder, false);
    app.set_note_open(false);
    app.set_note_text("".into());
}

fn save_in_background(feed: &Feed, text: String, picture: Option<Picture>) {
    let page = crate::page_name();
    let job = feed.start("Saving your note", "");
    // If the saving can't even start, the words still reach the log.
    let words = text.clone();
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
        match note::save(&note::notes_dir(), &n, chrono::Local::now()) {
            Ok(path) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                log::info("Note", &format!("Saved {name}"));
                job.done(&format!("Saved as {name}"));
            }
            Err(e) => {
                // The words go into the log, so they're not lost even when the folder can't be written.
                log::error("Note", &format!("Couldn't save the note: {e}\n{text}"));
                job.failed(&format!("Couldn't save it: {e}"));
            }
        }
    });
    if let Err(e) = spawned {
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
