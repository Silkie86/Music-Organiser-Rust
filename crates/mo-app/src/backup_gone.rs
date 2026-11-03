//! The Backup page's warning about the files the backup keeps that are gone from your folders (Solomon, 3 Nov 2026: a
//! file deleted from your music stays on the backup, with a warning). `mo_backup::gone` finds them; this says so in
//! words: how many, the room they take, and each one (where it was, when it was backed up, where the backup keeps
//! it). The autosaves your DAWs clear away themselves are counted apart, so they never bury the files you deleted.
//!
//! Words only, and the finding itself: `mo-ui` calls [`find`] on its low-priority pool, never the window's thread.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use chrono::{Local, NaiveDateTime, TimeZone};
use mo_backup::gone::{self, Gone, Kept};
use mo_formats::paths::file_name;
use mo_library::scan::{is_autosave_dir, is_autosave_file};
use mo_space::size::size_text;

use crate::backup::Prepared;
use crate::songs::{plural, thousands};

/// The files the backup at `backup` keeps that are gone from the folders this plan walked. Reads the backup drive.
pub fn find(prepared: &Prepared, backup: &Path, stop: &AtomicBool) -> io::Result<Vec<Gone>> {
    let present: Vec<&Path> = prepared
        .plan
        .copy
        .iter()
        .map(|f| f.path.as_path())
        .chain(prepared.plan.left_out.iter().map(|s| s.file.path.as_path()))
        .collect();
    gone::find(backup, &present, stop)
}

/// Whether one of your DAWs made the file, and clears such files away itself: an autosave, or a file straight in an
/// autosave folder (FL Studio's Backup, Ableton's Backup, Studio One's History…). Only the folder it's in counts: a
/// folder of yours called "Backups" further up doesn't make your songs in it autosaves.
pub fn daw_made(path: &Path) -> bool {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    is_autosave_file(&name(path)) || path.parent().is_some_and(|folder| is_autosave_dir(&name(folder)))
}

/// One gone file's line in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoneLine {
    /// Where it was.
    pub was: String,
    /// Its size, when it was backed up and where the backup keeps it.
    pub note: String,
    /// What Show on the backup shows: the copy, or the zip it's in.
    pub show: PathBuf,
}

/// The warning, in words.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Warning {
    /// The strip's line ("12 files you deleted or moved are still on the backup · 230 MB"); empty when there's
    /// nothing to warn about, and the strip isn't shown.
    pub line: String,
    /// Only autosaves your DAWs cleared away: news, not a warning, so the strip is in the page's own colours.
    pub quiet: bool,
    /// Under it, when opened: what it means, and the DAWs' autosaves counted apart.
    pub note: String,
    /// The files you deleted or moved, by where they were (the autosaves aren't listed).
    pub lines: Vec<GoneLine>,
}

/// A time from the backup's record or a zip's list (nanoseconds since 1970) as the date it was, here.
fn date(time: i128) -> Option<NaiveDateTime> {
    let secs = i64::try_from(time.div_euclid(1_000_000_000)).ok()?;
    Local.timestamp_opt(secs, 0).earliest().map(|t| t.naive_local())
}

/// Where the backup keeps a file, in words.
fn kept_where(kept: &[Kept]) -> String {
    let zips: Vec<&Path> = kept
        .iter()
        .filter_map(|k| match k {
            Kept::InZip(zip) => Some(zip.as_path()),
            Kept::Copy(_) => None,
        })
        .collect();
    let copy = kept.iter().any(|k| matches!(k, Kept::Copy(_)));
    let in_zips = match zips.as_slice() {
        [] => String::new(),
        [zip] => format!("in {}", file_name(&zip.to_string_lossy())),
        many => format!("in {} song zips", thousands(many.len())),
    };
    match (copy, in_zips.is_empty()) {
        (true, true) => "in the backup's folders".to_owned(),
        (true, false) => format!("in the backup's folders and {in_zips}"),
        (false, _) => in_zips,
    }
}

/// The warning's words for what [`find`] found.
pub fn warning(gone: &[Gone]) -> Warning {
    if gone.is_empty() {
        return Warning::default();
    }
    let (daw, yours): (Vec<&Gone>, Vec<&Gone>) = gone.iter().partition(|g| daw_made(&g.was));
    let bytes = |list: &[&Gone]| list.iter().map(|g| g.size).fold(0, u64::saturating_add);
    let autosaves = match daw.len() {
        0 => String::new(),
        1 => "1 autosave your DAW cleared away is kept too.".to_owned(),
        n => format!("{} autosaves your DAWs cleared away are kept too ({}).", thousands(n), size_text(bytes(&daw))),
    };
    let line = if yours.is_empty() {
        match daw.len() {
            1 => "1 autosave your DAW cleared away is still on the backup".to_owned(),
            n => format!("{} autosaves your DAWs cleared away are still on the backup", thousands(n)),
        }
    } else {
        let files = plural(yours.len(), "file");
        let verb = if yours.len() == 1 { "is" } else { "are" };
        format!("{files} you deleted or moved {verb} still on the backup · {}", size_text(bytes(&yours)))
    };
    let mut note = if yours.is_empty() {
        "The backup keeps them, as it keeps every file it has. Nothing on it is removed when your files are.".to_owned()
    } else {
        "They're gone from your folders, but the backup keeps them: copy back any you didn't mean to lose. Nothing on \
         the backup is removed when your files are."
            .to_owned()
    };
    if !yours.is_empty() && !autosaves.is_empty() {
        note.push(' ');
        note.push_str(&autosaves);
    }
    let lines = yours
        .iter()
        .map(|g| {
            let when = date(g.time).map(|d| format!(" · saved {}", d.format("%-d %b %Y"))).unwrap_or_default();
            let show = g
                .kept
                .iter()
                .map(|k| match k {
                    Kept::Copy(p) | Kept::InZip(p) => p.clone(),
                })
                .next()
                .unwrap_or_default();
            GoneLine {
                was: g.was.display().to_string(),
                note: format!("{}{when} · {}", size_text(g.size), kept_where(&g.kept)),
                show,
            }
        })
        .collect();
    Warning { line, quiet: yours.is_empty(), note, lines }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gone(was: &str, size: u64, kept: Vec<Kept>) -> Gone {
        // 1 Nov 2026, noon here.
        let noon = Local.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap();
        Gone { was: PathBuf::from(was), size, time: i128::from(noon.timestamp()) * 1_000_000_000, kept }
    }

    #[test]
    fn autosaves_are_told_apart_from_files_you_deleted() {
        assert!(daw_made(Path::new("/m/Song/Backup/Song (overwritten at 2213h05).flp")));
        assert!(daw_made(Path::new("/m/Song/History/Song v3.song")));
        assert!(daw_made(Path::new("/m/Song (autosave).flp")));
        assert!(!daw_made(Path::new("/m/Song/Song.flp")));
        // Only the folder a file is in counts: not one further up, and not the file's own name.
        assert!(!daw_made(Path::new("/m/Backups/Old songs/Song.flp")));
        assert!(!daw_made(Path::new("/m/Songs/backup")));
    }

    #[test]
    fn the_warning_counts_your_files_and_puts_the_autosaves_apart() {
        let copy = |p: &str| Kept::Copy(PathBuf::from(p));
        let zip = |p: &str| Kept::InZip(PathBuf::from(p));
        let found = vec![
            gone("/m/Samples/kick.wav", 2 << 20, vec![copy("/b/m/Samples/kick.wav"), zip("/b/Songs/Beat.zip")]),
            gone("/m/Song/Song.flp", 1 << 20, vec![zip("/b/Songs/Beat.zip"), zip("/b/Songs/Other.zip")]),
            gone("/m/Song/Backup/Song (overwritten at 1h05).flp", 500, vec![copy("/b/m/Song/Backup/x.flp")]),
            gone("/m/Song/Backup/Song (overwritten at 1h09).flp", 500, vec![copy("/b/m/Song/Backup/y.flp")]),
        ];
        let w = warning(&found);
        assert_eq!(w.line, "2 files you deleted or moved are still on the backup · 3 MB");
        assert!(!w.quiet);
        assert!(w.note.starts_with("They're gone from your folders, but the backup keeps them"), "{}", w.note);
        assert!(w.note.ends_with("2 autosaves your DAWs cleared away are kept too (0.98 KB)."), "{}", w.note);
        assert_eq!(w.lines.len(), 2);
        assert_eq!(w.lines[0].was, "/m/Samples/kick.wav");
        assert_eq!(w.lines[0].note, "2 MB · saved 1 Nov 2026 · in the backup's folders and in Beat.zip");
        assert_eq!(w.lines[0].show, PathBuf::from("/b/m/Samples/kick.wav"));
        assert!(w.lines[1].note.ends_with("· in 2 song zips"), "{}", w.lines[1].note);
        assert_eq!(w.lines[1].show, PathBuf::from("/b/Songs/Beat.zip"));

        // One file, nothing else.
        let one = warning(&found[..1]);
        assert_eq!(one.line, "1 file you deleted or moved is still on the backup · 2 MB");
        assert!(!one.note.contains("autosave"));
        // Only autosaves: a quieter line, and no list.
        let daw = warning(&found[2..]);
        assert_eq!(daw.line, "2 autosaves your DAWs cleared away are still on the backup");
        assert!(daw.quiet && daw.lines.is_empty());
        assert_eq!(warning(&[]), Warning::default());
    }
}
