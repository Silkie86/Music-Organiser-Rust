//! Scanning the sample folders for new, changed, moved and gone sounds, and keeping what's found in the app's own sound
//! library (stage 2 of "the sound library grows again", review finding X1-own1). Since the C# app retired, nothing
//! did: new samples never showed on the Sounds page.
//!
//! As the C# app's `SoundOrganiser.Scan`: the folders are walked ([`scan::find_sounds`]); only files new or changed by
//! their size and time are listened to; renamed and moved ones keep everything you gave them; a folder on an unplugged
//! drive keeps its sounds ([`scan::plan`]). It all runs off the window's thread, on the bulk pool at low priority
//! (rule 21), a few sounds at a time, each few kept in one short write ([`grow`]).

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use mo_library::scan::FoundFile;
use mo_sounds::compact::{self, grow};
use mo_sounds::scan::{self, ScanPlan};
use mo_sounds::{KnownSound, SoundsDb, analyse};
use rayon::prelude::*;

use crate::background;
use crate::backup::thousands;

/// How many sounds are listened to before what was found is kept: each write stays short, and a stop (the app closed)
/// loses no more than this.
const AT_A_TIME: usize = 64;

/// The most of the library a scan forgets at once: past it (and past [`FEW_GONE`] sounds), the sounds that look gone
/// are left, as something is more likely wrong with the folders than half the samples deleted.
const MOST_GONE: f64 = 0.5;

/// So many sounds gone are forgotten however few the library has.
const FEW_GONE: usize = 100;

/// What a scan did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scanned {
    /// How many sound files are in the sample folders.
    pub found: usize,
    /// What changed in the library.
    pub kept: grow::Kept,
    /// Of the files listened to, how many couldn't be (kept with why, as the C# app kept them).
    pub unreadable: usize,
    /// Of those, how many were new files: they aren't counted as new sounds found.
    pub unheard_new: usize,
    /// How many sounds looked gone but weren't forgotten: too many at once ([`MOST_GONE`]).
    pub not_forgotten: usize,
    /// How long it took.
    pub took: Duration,
    /// Why it stopped part way, when it did: what was kept before stays kept, and the rest is tried again at the next
    /// scan.
    pub stopped: Option<String>,
    /// The files listened to and kept (new and changed sounds), for the page to read only their fingerprints, not
    /// every one again (review finding S-5).
    pub listened: Vec<String>,
}

/// Scans `folders` and keeps what's new, changed, moved or gone in the app's own sound library beside `sounds_db`.
/// `None` when there's nothing to scan: no folders, or no library that's the app's own (one still following
/// `sounds.db` is made from it instead). `progress` is told how many files have been listened to, of how many, before
/// the first and after each few; it's called on this thread. Waits for the work, so never on the window's thread.
///
/// An error when nothing could be done (the library couldn't be read); a write that fails once some are kept stops the
/// scan with what was kept ([`Scanned::stopped`]), for the page to take it.
pub fn scan(sounds_db: &Path, folders: &[String], progress: &dyn Fn(usize, usize)) -> Result<Option<Scanned>, String> {
    let began = Instant::now();
    let copy = compact::copy_of(sounds_db);
    if folders.is_empty() || compact::own_since(&copy).is_none() {
        return Ok(None);
    }
    let found = background::on_bulk_pool(|| scan::find_sounds(folders));
    let known = SoundsDb::open_read_only(&copy).and_then(|db| db.known_files()).map_err(|e| e.to_string())?;
    let ScanPlan { listen, moves, forget } = scan::plan(&found, &known, folders);
    let mut scanned = Scanned { found: found.files.len(), ..Scanned::default() };
    let forget = if forget.len() > FEW_GONE && forget.len() as f64 > known.len() as f64 * MOST_GONE {
        scanned.not_forgotten = forget.len();
        Vec::new()
    } else {
        forget
    };
    if let Err(why) = keep_all(sounds_db, &known, &listen, &moves, &forget, &mut scanned, progress) {
        scanned.stopped = Some(why);
    }
    scanned.took = began.elapsed();
    Ok(Some(scanned))
}

/// Follows the moved and gone sounds, then listens to the new and changed ones a few at a time, each few kept in one
/// write, counting what was kept in `scanned` as it goes.
fn keep_all(
    sounds_db: &Path,
    known: &[KnownSound],
    listen: &[FoundFile],
    moves: &[(String, String)],
    forget: &[String],
    scanned: &mut Scanned,
    progress: &dyn Fn(usize, usize),
) -> Result<(), String> {
    if !moves.is_empty() || !forget.is_empty() {
        let followed = grow::follow(sounds_db, moves, forget).map_err(|e| e.to_string())?;
        scanned.kept.moved = followed.moved;
        scanned.kept.forgotten = followed.forgotten;
    }
    if listen.is_empty() {
        return Ok(());
    }
    progress(0, listen.len());
    // The paths the library knows (letter case aside, as SQLite compares them), to tell a new file that couldn't be
    // listened to from a changed one: gathered only once there's one.
    let mut known_paths: Option<HashSet<String>> = None;
    let mut done = 0;
    for files in listen.chunks(AT_A_TIME) {
        let listened: Vec<grow::Listened> = background::on_bulk_pool(|| files.par_iter().map(listen_to).collect());
        let kept = grow::keep(sounds_db, &listened).map_err(|e| e.to_string())?;
        scanned.kept.added += kept.added;
        scanned.kept.updated += kept.updated;
        scanned.listened.extend(listened.iter().map(|l| l.path.clone()));
        for l in listened.iter().filter(|l| l.analysed.is_err()) {
            scanned.unreadable += 1;
            let known = known_paths.get_or_insert_with(|| {
                known
                    .iter()
                    .map(|k| &k.path)
                    .chain(moves.iter().map(|(_, to)| to))
                    .map(|p| p.to_ascii_lowercase())
                    .collect()
            });
            if !known.contains(&l.path.to_ascii_lowercase()) {
                scanned.unheard_new += 1;
            }
        }
        done += files.len();
        progress(done, listen.len());
    }
    Ok(())
}

/// Listens to one sound file. A file that can't be read, or that trips the reader up (a damaged file), is kept with
/// why, as the C# app kept it: it isn't listened to again until it changes.
fn listen_to(file: &FoundFile) -> grow::Listened {
    let path = Path::new(&file.path);
    let analysed = match mo_catch::catch(|| analyse::analyse_file(path)) {
        Ok(Ok(analysed)) => Ok(analysed),
        Ok(Err(e)) => Err(format!("Couldn't read the file: {e}")),
        Err(_) => Err("Couldn't read the file: it tripped the reader up (the file may be damaged)".to_owned()),
    };
    grow::Listened { path: file.path.clone(), size: file.size, mtime: file.mtime, analysed }
}

/// What the log says a scan did.
pub fn scanned_line(s: &Scanned) -> String {
    let k = &s.kept;
    let mut line = format!(
        "{} sound files in the sample folders: {} new, {} listened to again, {} moved, {} gone, in {:.1} s",
        thousands(s.found),
        thousands(k.added),
        thousands(k.updated),
        thousands(k.moved),
        thousands(k.forgotten),
        s.took.as_secs_f32()
    );
    if s.unreadable > 0 {
        line.push_str(&format!("; {} couldn't be listened to", thousands(s.unreadable)));
    }
    if s.not_forgotten > 0 {
        line.push_str(&format!(
            "; {} that look gone were left, as that's over half the library",
            thousands(s.not_forgotten)
        ));
    }
    if let Some(why) = &s.stopped {
        line.push_str(&format!("; it stopped part way: {why}"));
    }
    line
}

/// What the card says once a scan that listened to sounds is done: a headline, and a line that's never empty.
pub fn scanned_card(s: &Scanned) -> (String, String) {
    let k = &s.kept;
    // A file that couldn't be listened to isn't a sound found.
    let title = match (k.added.saturating_sub(s.unheard_new), k.updated) {
        (0, 0) => "Couldn't listen to your new sound files".to_owned(),
        (0, _) => "Listened to your changed sounds".to_owned(),
        (1, _) => "Found a new sound".to_owned(),
        (n, _) => format!("Found {} new sounds", thousands(n)),
    };
    let mut detail = Vec::new();
    if k.updated > 0 {
        detail.push(format!("{} changed, listened to again", thousands(k.updated)));
    }
    if k.moved > 0 {
        detail.push(format!("{} moved", thousands(k.moved)));
    }
    if k.forgotten > 0 {
        detail.push(format!("{} gone", thousands(k.forgotten)));
    }
    if s.unreadable > 0 {
        detail.push(format!("{} couldn't be listened to", thousands(s.unreadable)));
    }
    // Never empty: the card would keep saying how far listening got.
    if detail.is_empty() {
        return (title, "They're in your sound library now.".to_owned());
    }
    (title, format!("{}.", detail.join(", ")))
}

/// What the card says when a scan that listened to sounds stopped part way ([`Scanned::stopped`]).
pub fn stopped_card(s: &Scanned) -> (String, String) {
    let why = s.stopped.as_deref().unwrap_or_default().trim_end_matches('.');
    let k = &s.kept;
    if k.added + k.updated == 0 {
        return ("Couldn't keep your new sounds".to_owned(), format!("{why}. They're tried again at the next start."));
    }
    (
        "Couldn't keep all your new sounds".to_owned(),
        format!(
            "{} were kept, then: {why}. The rest are tried again at the next start.",
            thousands(k.added + k.updated)
        ),
    )
}

/// What the card says when sounds that look gone were left ([`MOST_GONE`]).
pub fn not_forgotten_line(s: &Scanned) -> String {
    format!(
        "{} sounds look gone from your sample folders, over half your library, so they weren't forgotten. If a drive \
         or folder moved, check the sample folders in Settings.",
        thousands(s.not_forgotten)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_a_scan_did() {
        let s = Scanned {
            found: 453_885,
            kept: grow::Kept { added: 1_204, updated: 3, moved: 12, forgotten: 2 },
            unreadable: 2,
            unheard_new: 1,
            not_forgotten: 0,
            took: Duration::from_millis(61_250),
            stopped: None,
            listened: Vec::new(),
        };
        assert_eq!(
            scanned_line(&s),
            "453,885 sound files in the sample folders: 1,204 new, 3 listened to again, 12 moved, 2 gone, in 61.2 s; \
             2 couldn't be listened to"
        );
        assert_eq!(
            scanned_card(&s),
            (
                "Found 1,203 new sounds".to_owned(),
                "3 changed, listened to again, 12 moved, 2 gone, 2 couldn't be listened to.".to_owned()
            )
        );
        let one = Scanned { kept: grow::Kept { added: 1, ..grow::Kept::default() }, ..Scanned::default() };
        assert_eq!(
            scanned_card(&one),
            ("Found a new sound".to_owned(), "They're in your sound library now.".to_owned())
        );
        let left = Scanned { not_forgotten: 230_000, ..Scanned::default() };
        assert!(not_forgotten_line(&left).starts_with("230,000 sounds look gone"));
        assert!(scanned_line(&left).ends_with("230,000 that look gone were left, as that's over half the library"));
        // Only new files that couldn't be listened to.
        let unheard = Scanned {
            kept: grow::Kept { added: 2, ..grow::Kept::default() },
            unreadable: 2,
            unheard_new: 2,
            ..Scanned::default()
        };
        assert_eq!(scanned_card(&unheard).0, "Couldn't listen to your new sound files");
        // Stopped part way, and before anything was kept.
        let stopped = Scanned {
            kept: grow::Kept { added: 128, ..grow::Kept::default() },
            stopped: Some("There isn't room on the disk.".to_owned()),
            ..Scanned::default()
        };
        assert_eq!(
            stopped_card(&stopped),
            (
                "Couldn't keep all your new sounds".to_owned(),
                "128 were kept, then: There isn't room on the disk. The rest are tried again at the next start."
                    .to_owned()
            )
        );
        assert!(scanned_line(&stopped).ends_with("; it stopped part way: There isn't room on the disk."));
        let none = Scanned { stopped: Some("busy".to_owned()), ..Scanned::default() };
        assert_eq!(stopped_card(&none).0, "Couldn't keep your new sounds");
    }
}
