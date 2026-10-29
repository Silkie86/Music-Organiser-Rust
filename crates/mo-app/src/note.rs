//! F12 anywhere in the app: a note to Claude about what's on screen, saved with a picture of the window and the
//! app's last log lines in `notes\` in the app's data folder, the same as the C# app. Claude reads that folder from
//! Solomon's PC, so a test at home needs no copying out.
//!
//! Rule 17: what's typed is kept in `draft.txt` as it's typed, so if the app closes with a note open, the next F12
//! offers it back. While a note is open, `writing.lock` says so, and the launcher waits before restarting the app.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};

pub const DRAFT_FILE: &str = "draft.txt";
pub const WRITING_FILE: &str = "writing.lock";

/// `notes\` in the app's data folder.
pub fn notes_dir() -> PathBuf {
    crate::paths::data_dir().join("notes")
}

/// What a saved note is made of.
pub struct Note<'a> {
    pub text: &'a str,
    pub page: &'a str,
    pub version: &'a str,
    pub log_path: Option<&'a Path>,
    pub recent_log: &'a [String],
    /// The window as a PNG, if it could be drawn and the picture is wanted.
    pub picture: Option<&'a [u8]>,
}

/// Writes `note-<time>.md`, and a `.png` beside it when there's a picture. Returns the note's path.
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
    // The words go down first, so a save cut short (the app closing, a full disk) still leaves them, not an empty file.
    std::io::Write::write_all(&mut file, md.as_bytes())?;
    file.sync_all()?;
    if let Some(png) = note.picture {
        std::fs::write(folder.join(format!("{name}.png")), png)?;
    }
    Ok(folder.join(format!("{name}.md")))
}

/// The note typed so far that was never saved, if any.
pub fn read_draft(folder: &Path) -> Option<String> {
    std::fs::read_to_string(folder.join(DRAFT_FILE)).ok().filter(|t| !t.trim().is_empty())
}

/// Keeps what's typed so far. Failing to keep it is not worth stopping the typing for.
pub fn keep_draft(folder: &Path, text: &str) {
    if std::fs::create_dir_all(folder).is_ok() {
        let _ = std::fs::write(folder.join(DRAFT_FILE), text);
    }
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
            picture: Some(b"png"),
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

    #[test]
    fn a_draft_survives_until_saved_or_cancelled() {
        let dir = temp_folder("draft");
        assert_eq!(read_draft(&dir), None);
        keep_draft(&dir, "half a tho");
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
