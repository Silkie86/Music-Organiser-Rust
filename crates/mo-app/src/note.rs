//! F12 anywhere in the app: a note to Claude about what's on screen, saved with a picture of the window and the
//! app's last log lines in `notes\` in the app's data folder, the same as the C# app. Claude reads that folder from
//! Solomon's PC, so a test at home needs no copying out.
//!
//! Rule 17: what's typed is kept in `draft.txt` as it's typed, so if the app closes with a note open, the next F12
//! offers it back. While a note is open, `writing.lock` says so, and the launcher waits before restarting the app.

use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Local};

pub const DRAFT_FILE: &str = "draft.txt";
/// What the draft is written as first ([`keep_draft`]): made new beside `draft.txt`, written whole, then renamed over
/// it, so a write cut short (a crash that ends the app, a full disk, a power cut) leaves the draft from before whole,
/// not an empty file. A crash between the writing and the renaming leaves this behind; the next draft written removes
/// it first, so at most one small file stays. The start-up sweep never touches it (rule 17: it may hold the newest
/// words), and it's never read back as the draft: it may be cut short.
pub const DRAFT_PART: &str = "draft.txt.mo-part";
/// The same, for a draft written without its turn ([`keep_draft_alone`]): a crash, or the app closing, while the
/// writer whose turn it is is stuck part way through [`DRAFT_PART`]. A name of its own, so neither writer ever removes
/// or renames the other's half-written file. The next draft written without its turn removes one left behind, so at
/// most one small file stays; the start-up sweep never touches it either.
pub const DRAFT_PART_ALONE: &str = "draft.txt.mo-part-2";
/// How many times a draft's renaming is tried on Windows while another program holds the file a moment
/// ([`tried_while_held`]), and how long apart: about 100 ms at most, so a crash's write stays quick.
const RENAME_TRIES: u32 = 6;
const RENAME_GAP: Duration = Duration::from_millis(20);
pub const WRITING_FILE: &str = "writing.lock";
/// The notes' folder in the app's data folder (the C# app's notes are there too).
pub const FOLDER: &str = "notes";

/// `notes\` in the app's data folder.
pub fn notes_dir() -> PathBuf {
    crate::paths::data_dir().join(FOLDER)
}

/// What a saved note is made of.
pub struct Note<'a> {
    pub text: &'a str,
    pub page: &'a str,
    pub version: &'a str,
    pub log_path: Option<&'a Path>,
    pub recent_log: &'a [String],
    /// What makes the window's picture into a PNG, if it could be drawn and the picture is wanted. It's called only
    /// once the words are on disk, as a big window takes a moment; it says why when the picture can't be made.
    pub picture: Option<&'a dyn Fn() -> Result<Vec<u8>, String>>,
}

/// Writes `note-<time>.md`, and then a `.png` beside it when there's a picture: the words are on disk before the
/// picture is made. Returns the note's path.
pub fn save(folder: &Path, note: &Note, now: DateTime<Local>) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(folder)?;
    let stamp = now.format("%Y-%m-%d_%H-%M-%S");
    // Claims the name by creating the file, so two notes saved in the same second can't take the same one.
    let (name, mut file) = (1..1000)
        .find_map(|n| {
            let name = if n == 1 { format!("note-{stamp}") } else { format!("note-{stamp}-{n}") };
            match std::fs::OpenOptions::new().write(true).create_new(true).open(folder.join(format!("{name}.md"))) {
                Ok(f) => Some(Ok((name, f))),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(e) => Some(Err(e)),
            }
        })
        .unwrap_or_else(|| Err(std::io::Error::other("no free note name")))?;
    let words = note.text.trim();
    let words = if !words.is_empty() {
        words
    } else if note.picture.is_some() {
        "(no words, just the picture)"
    } else {
        "(no words)"
    };
    let mut md = format!("# Note for Claude, {}\n\n{words}\n\n", now.format("%Y-%m-%d %H:%M:%S"));
    md += &format!("- Page: {}\n", if note.page.is_empty() { "?" } else { note.page });
    md += &format!("- Version: {}\n", note.version);
    if let Some(p) = note.log_path {
        md += &format!("- Log: {}\n", p.display());
    }
    if note.picture.is_some() {
        md += &format!("- Picture: {name}.png\n");
    }
    if !note.recent_log.is_empty() {
        md += "\n## The app's last log lines\n\n```\n";
        let skip = note.recent_log.len().saturating_sub(40);
        for line in &note.recent_log[skip..] {
            md += line;
            md.push('\n');
        }
        md += "```\n";
    }
    // The words go down first, so a save cut short (the app closing, a full disk, a crash while the picture is made)
    // still leaves them, not an empty file.
    file.write_all(md.as_bytes())?;
    file.sync_all()?;
    if let Some(make_png) = note.picture {
        let picture = folder.join(format!("{name}.png"));
        let made = make_png().and_then(|png| std::fs::write(&picture, png).map_err(|e| e.to_string()));
        if let Err(e) = made {
            // The note stands without its picture, and says so at its end: added to what's on disk, never written
            // again from the start, so its words stay whole whatever happens now. It's saved either way.
            let _ = std::fs::remove_file(&picture);
            let said = format!("\nThe picture ({name}.png) couldn't be saved: {e}\n");
            let told = file.write_all(said.as_bytes()).and_then(|()| file.sync_all());
            let also = told.err().map_or_else(String::new, |t| format!(" (nor could the note say so: {t})"));
            crate::log::warning("Note", &format!("Saved {name}.md without its picture: {e}{also}"));
        }
    }
    Ok(folder.join(format!("{name}.md")))
}

/// The note typed so far that was never saved, if any.
pub fn read_draft(folder: &Path) -> Option<String> {
    std::fs::read_to_string(folder.join(DRAFT_FILE)).ok().filter(|t| !t.trim().is_empty())
}

/// Keeps what's typed so far, through [`DRAFT_PART`], so the draft from before stays whole until the new one is. Says
/// whether it was kept. Failing to keep it is not worth stopping the typing for: the caller keeps the words for
/// another try.
pub fn keep_draft(folder: &Path, text: &str) -> bool {
    write_draft(folder, text, DRAFT_PART)
}

/// [`keep_draft`] for a draft written without its turn, through [`DRAFT_PART_ALONE`], so it never touches the part
/// file the writer whose turn it is may be writing.
pub fn keep_draft_alone(folder: &Path, text: &str) -> bool {
    write_draft(folder, text, DRAFT_PART_ALONE)
}

/// Writes `text` as the draft in `folder`: made new as `part`, written whole and synced, then renamed over the draft.
fn write_draft(folder: &Path, text: &str, part: &str) -> bool {
    let part = folder.join(part);
    // One a crash left goes first. Made new, so nothing there (a link included) is ever written through.
    let _ = std::fs::remove_file(&part);
    let written = std::fs::create_dir_all(folder).and_then(|()| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&part)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()
    });
    let kept = written.and_then(|()| tried_while_held(|| std::fs::rename(&part, folder.join(DRAFT_FILE))));
    if kept.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    kept.is_ok()
}

/// Does `step` (a renaming), trying it again a few times, a moment apart ([`RENAME_TRIES`], [`RENAME_GAP`]), while
/// another program holds the file a moment without letting it be replaced (on Windows, a virus scanner or a backup
/// tool looking at a file just written). A crash's write and the app closing write once, so a moment's hold mustn't
/// lose their words.
fn tried_while_held(mut step: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    let mut tries = 1;
    loop {
        match step() {
            Err(e) if tries < RENAME_TRIES && held_a_moment(&e) => {
                tries += 1;
                std::thread::sleep(RENAME_GAP);
            }
            done => return done,
        }
    }
}

/// On Windows, what another program holding the file says: access denied, or the file in use (error 32). Elsewhere a
/// renaming isn't held up like that, so it isn't tried again.
fn held_a_moment(e: &std::io::Error) -> bool {
    cfg!(windows) && (e.kind() == ErrorKind::PermissionDenied || e.raw_os_error() == Some(32))
}

/// Saved or cancelled on purpose: either way the draft has done its job.
pub fn clear_draft(folder: &Path) {
    let _ = std::fs::remove_file(folder.join(DRAFT_FILE));
}

/// Puts up or takes down `writing.lock` (holding the app's process id) while a note is open. Only this app's own lock
/// is taken down: the folder is shared with the C# app, which may have a note open too.
pub fn writing(folder: &Path, open: bool) {
    let lock = folder.join(WRITING_FILE);
    let me = std::process::id().to_string();
    if open {
        if std::fs::create_dir_all(folder).is_ok() {
            let _ = std::fs::write(lock, me);
        }
    } else if std::fs::read_to_string(&lock).is_ok_and(|held| held.trim() == me) {
        let _ = std::fs::remove_file(lock);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-note-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn saves_words_page_version_picture_and_log() {
        let dir = temp_folder("save");
        let recent: Vec<String> = (0..50).map(|i| format!("line {i}")).collect();
        let note = Note {
            text: "  The rail jumps when I click Space  ",
            page: "Space",
            version: "0.1.0 (abc1234)",
            log_path: Some(Path::new("C:/logs/session.log")),
            recent_log: &recent,
            picture: Some(&|| Ok(b"png".to_vec())),
        };
        let now = Local::now();
        let path = save(&dir, &note, now).expect("saved");
        let md = std::fs::read_to_string(&path).expect("readable");
        assert!(md.contains("\nThe rail jumps when I click Space\n"));
        assert!(md.contains("- Page: Space\n- Version: 0.1.0 (abc1234)\n"));
        assert!(md.contains("- Picture: note-"));
        assert!(md.contains("line 49") && md.contains("line 10") && !md.contains("line 9\n"), "the last 40 lines");
        assert!(path.with_extension("png").exists());
        // A second note in the same second gets its own name.
        let again = save(&dir, &note, now).expect("saved");
        assert_ne!(path, again);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A note with these words and this picture.
    fn note_with<'a>(text: &'a str, picture: &'a dyn Fn() -> Result<Vec<u8>, String>) -> Note<'a> {
        Note { text, page: "Songs", version: "1", log_path: None, recent_log: &[], picture: Some(picture) }
    }

    #[test]
    fn the_words_are_on_disk_before_the_picture_is_made() {
        let dir = temp_folder("words-first");
        // What a crash while the picture is made would leave behind.
        let on_disk_then = std::cell::RefCell::new(String::new());
        let make_png = || {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                *on_disk_then.borrow_mut() += &std::fs::read_to_string(entry.path()).unwrap_or_default();
            }
            Ok(b"png".to_vec())
        };
        let path = save(&dir, &note_with("Words first", &make_png), Local::now()).expect("saved");
        assert!(on_disk_then.borrow().contains("\nWords first\n"), "{}", on_disk_then.borrow());
        assert!(path.with_extension("png").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_picture_that_cant_be_made_leaves_the_note_with_its_words() {
        let dir = temp_folder("no-picture");
        let make_png = || Err("the window was too big".to_owned());
        let path = save(&dir, &note_with("Still here", &make_png), Local::now()).expect("saved");
        let md = std::fs::read_to_string(&path).expect("readable");
        let name = path.file_stem().expect("named").to_string_lossy().into_owned();
        // Said at its end, added to what was on disk: the words before it are as they were written.
        assert!(md.starts_with("# Note for Claude, ") && md.contains("\nStill here\n"), "{md}");
        let end =
            format!("- Picture: {name}.png\n\nThe picture ({name}.png) couldn't be saved: the window was too big\n");
        assert!(md.ends_with(&end), "{md}");
        assert!(!path.with_extension("png").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_draft_written_without_its_turn_never_touches_the_part_file_being_written() {
        let dir = temp_folder("alone");
        assert!(keep_draft(&dir, "the words from before"));
        // The writer whose turn it is, stuck part way through its part file.
        std::fs::write(dir.join(DRAFT_PART), "half wri").expect("writable");
        // What a crash written without its turn left once before.
        std::fs::write(dir.join(DRAFT_PART_ALONE), "cut sh").expect("writable");
        assert!(keep_draft_alone(&dir, "the newest words"));
        assert_eq!(read_draft(&dir).as_deref(), Some("the newest words"));
        assert_eq!(std::fs::read_to_string(dir.join(DRAFT_PART)).ok().as_deref(), Some("half wri"), "left alone");
        assert!(!dir.join(DRAFT_PART_ALONE).exists(), "nothing of its own is left beside it");
        // And the writer whose turn it is never touches what a write without its turn is writing.
        std::fs::write(dir.join(DRAFT_PART_ALONE), "being writ").expect("writable");
        assert!(keep_draft(&dir, "in its turn"));
        assert_eq!(read_draft(&dir).as_deref(), Some("in its turn"));
        assert_eq!(std::fs::read_to_string(dir.join(DRAFT_PART_ALONE)).ok().as_deref(), Some("being writ"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_renaming_another_program_holds_a_moment_is_tried_again_on_windows() {
        let held = || std::io::Error::from(ErrorKind::PermissionDenied);
        let tries = std::cell::Cell::new(0_u32);
        // Held for the first two tries.
        let done = tried_while_held(|| {
            tries.set(tries.get() + 1);
            if tries.get() <= 2 { Err(held()) } else { Ok(()) }
        });
        if cfg!(windows) {
            assert!(done.is_ok());
            assert_eq!(tries.get(), 3);
        } else {
            assert!(done.is_err(), "elsewhere a renaming isn't held up like that");
            assert_eq!(tries.get(), 1);
        }
        // Held for good: tried a few times only, then it says so.
        tries.set(0);
        assert!(
            tried_while_held(|| {
                tries.set(tries.get() + 1);
                Err(held())
            })
            .is_err()
        );
        assert_eq!(tries.get(), if cfg!(windows) { RENAME_TRIES } else { 1 });
        // Any other trouble isn't tried again.
        tries.set(0);
        assert!(
            tried_while_held(|| {
                tries.set(tries.get() + 1);
                Err(std::io::Error::from(ErrorKind::NotFound))
            })
            .is_err()
        );
        assert_eq!(tries.get(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn the_draft_from_before_stays_whole_until_the_new_one_is_written() {
        use std::io::Read;
        let dir = temp_folder("whole");
        assert!(keep_draft(&dir, "the words from before"));
        // Someone reading the draft as the new one is written (the next F12, say).
        let mut reading = std::fs::File::open(dir.join(DRAFT_FILE)).expect("there");
        assert!(keep_draft(&dir, "newer words"));
        let mut read = String::new();
        reading.read_to_string(&mut read).expect("readable");
        // Written beside it and then put in its place: the draft from before was never emptied to make room.
        assert_eq!(read, "the words from before");
        assert_eq!(read_draft(&dir).as_deref(), Some("newer words"));
        assert!(!dir.join(DRAFT_PART).exists(), "nothing is left beside it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_draft_that_cant_be_written_says_so_and_the_one_from_before_stays() {
        let dir = temp_folder("unwritable");
        assert!(keep_draft(&dir, "the words from before"));
        // Something in the way of the new draft that isn't a file (as bad as a full disk): it can't be written.
        std::fs::create_dir_all(dir.join(DRAFT_PART).join("in the way")).expect("made");
        assert!(!keep_draft(&dir, "newer words"), "it says the words weren't kept");
        assert_eq!(read_draft(&dir).as_deref(), Some("the words from before"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_a_crash_left_beside_the_draft_goes_with_the_next_draft() {
        let dir = temp_folder("leftover");
        std::fs::create_dir_all(&dir).expect("made");
        // A crash between writing and renaming: the words in it may be cut short, so they're never offered back.
        std::fs::write(dir.join(DRAFT_PART), "cut sh").expect("writable");
        assert_eq!(read_draft(&dir), None);
        assert!(keep_draft(&dir, "the next words"));
        assert_eq!(read_draft(&dir).as_deref(), Some("the next words"));
        assert!(!dir.join(DRAFT_PART).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_draft_survives_until_saved_or_cancelled() {
        let dir = temp_folder("draft");
        assert_eq!(read_draft(&dir), None);
        assert!(keep_draft(&dir, "half a tho"));
        assert_eq!(read_draft(&dir).as_deref(), Some("half a tho"));
        clear_draft(&dir);
        assert_eq!(read_draft(&dir), None);
        writing(&dir, true);
        assert!(dir.join(WRITING_FILE).exists());
        writing(&dir, false);
        assert!(!dir.join(WRITING_FILE).exists());
        // Another app's lock is left alone.
        std::fs::write(dir.join(WRITING_FILE), "1").expect("writable");
        writing(&dir, false);
        assert!(dir.join(WRITING_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
