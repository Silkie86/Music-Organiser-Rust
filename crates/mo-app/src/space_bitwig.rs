//! The Space page's Bitwig copies (phase 4, part 2). Bitwig keeps a copy of each Library multisample a project uses in
//! the project's own `multi-samples` folder, under a new name whenever the name is taken ("Kicks 1 Select-3-2-2
//! .multisample"), so one sound can sit in many projects. This finds the copies that are the same as a Library file
//! byte for byte (`mo_space::bitwig`) and says how much room they take, project by project.
//!
//! Fix (part 2b) writes a "(fixed)" project beside a save that uses the Library's own files instead of the copies
//! (`mo_convert::relink`). Each project's newest save is ticked for it out of the box, and any other save in its folder
//! can be ticked too (Solomon, 4 Nov 2026). Nothing else changes: the saves and their copies stay as they are (rule 3).
//! Next, once you've checked a fixed project plays, the copies it no longer needs go to the Recycle Bin (Solomon's
//! choice, 31 Oct 2026).
//!
//! While Fix is being tried, every "(fixed)" project it wrote can go to the Recycle Bin in one go, so the saves can be
//! fixed again (Solomon, 11 Nov 2026: "if i need to delete all the fixed ones as im testing i should be able to do
//! that easily"). Put back on the Recycle Bin line brings them back. One you've saved in since Fix wrote it stays, as
//! the app's note of what Fix wrote says ([`crate::space_fix_note`]; Solomon, 15 Nov 2026).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

use mo_convert::relink::{self, Relink};
use mo_formats::paths;
use mo_library::song_writes::BitwigReread;
use mo_library::songs::SongChoices;
use mo_space::bitwig::{self, LibraryCopy, LibraryIndex, Save};
use mo_space::size::size_text;
use mo_space::{Stopped, key};

use crate::songs::{plural, thousands};
use crate::space::{Card, Line, Opens, Part, and_list};
use crate::space_bin::BIN;
use crate::space_fix_note::Stays;

/// The Bitwig copies line's id.
pub const BITWIG_COPIES: &str = "bitwig-copies";

/// The activity card's headline while a look is on, in words that say what it's for (Solomon, 4 Nov 2026: "what
/// does looking for bitwig copies mean?"), and its line until it can say how far it is.
pub const LOOKING: &str = "Finding Bitwig's copies of your Library sounds";
pub const LOOKING_WHY: &str = "Bitwig copies a Library multisample or a sample into each project that uses it. The \
    Space page shows the room those copies take.";

/// The activity card's headline and line when a look stops because a newer one took its place.
pub const LOOK_STOPPED: (&str, &str) = ("Stopped finding Bitwig's copies", "A newer look took its place.");

/// What the card says about Fix, under the projects: the Recycle Bin on Windows, the Trash elsewhere, as its buttons
/// say (the whole-app review of 19 Nov, FS-16).
pub fn next() -> String {
    format!(
        "Fix writes a \"(fixed)\" project beside each ticked save, using the Library's own files \
    instead of the copies. The saves and their copies stay as they are. Once you've opened a fixed project in Bitwig \
    and heard it play, tick it: Free the space then sends the copies it no longer uses to the {BIN}. That \
    song's older saves, its older fixed projects and Bitwig's backups won't find those copies any more, so use the \
    fixed project from then on; a copy another song in the folder still uses stays. Put back brings them all back."
    )
}

/// The kind of clean-up the Bitwig copies Free the space sends are noted as, so Put back finds them.
pub const BITWIG_COPIES_FREED: &str = "bitwig-copies";

/// The activity card's title while Free the space sends copies.
pub const FREEING: &str =
    if cfg!(windows) { "Sending Bitwig copies to the Recycle Bin" } else { "Sending Bitwig copies to the Trash" };

/// The Bitwig card's Free the space button: its words and whether it can be pressed now. None when no fixed save is
/// ticked as playing.
pub fn free_button(found: Option<&Copies>, fixes: &Fixes, wait: bool) -> Option<(String, bool)> {
    if fixes.heard.is_empty() {
        return None;
    }
    let songs: usize = found?.projects.iter().map(|p| fixes.heard_in(p).len()).sum();
    (songs > 0).then(|| (format!("Free the space: copies of {} to the {BIN}", plural(songs, "checked song")), !wait))
}

/// The activity card's title while Fix writes.
pub const FIXING: &str = "Fixing Bitwig saves";

/// The kind the app's list of what it put in the Recycle Bin keeps the fixed projects it sent there under.
pub const BITWIG_FIXED: &str = "bitwig-fixed";

/// The activity card's title while the fixed projects go to the Recycle Bin.
pub const REMOVING_FIXED: &str = if cfg!(windows) {
    "Sending fixed Bitwig projects to the Recycle Bin"
} else {
    "Sending fixed Bitwig projects to the Trash"
};

/// A Bitwig project folder with copies of Library multisamples in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub folder: PathBuf,
    /// The copies in its `multi-samples` and `samples` folders that are the same as a Library multisample or a sound the
    /// Sounds library knows, in the order of their paths.
    pub copies: Vec<LibraryCopy>,
    /// Its saves: yours beside the project newest first, then Bitwig's auto-backups below it
    /// (`mo_space::bitwig::saves_in`).
    pub saves: Vec<Save>,
}

/// One song's saves beside a project, as its card lists them.
#[derive(Clone, Debug)]
pub struct SongSaves<'a> {
    /// Its newest save: the one Fix is ticked for out of the box.
    pub newest: &'a Save,
    /// Its older saves, newest first.
    pub older: Vec<&'a Save>,
}

impl Project {
    /// The songs beside the project, each with its saves, the song with the newest save first. A folder can hold more
    /// than one song (a remix, or another track made from its sounds: Solomon, 17 Nov 2026), told apart by name as the
    /// Songs page tells them (`mo_library::songs::key`: "Song v2" and "Song final" are one song, "Song remix"
    /// another). Songs you joined by hand on the Songs page aren't known here, so their saves may show as two songs.
    pub fn songs(&self) -> Vec<SongSaves<'_>> {
        let folder = self.folder.to_string_lossy();
        let mut keys: Vec<String> = Vec::new();
        let mut songs: Vec<SongSaves<'_>> = Vec::new();
        for save in self.saves.iter().filter(|s| !s.below) {
            let name = paths::stem(&save.path.to_string_lossy()).to_owned();
            let key = mo_library::songs::key(&folder, "Bitwig", &name);
            match keys.iter().position(|k| *k == key) {
                Some(at) => songs[at].older.push(save),
                None => {
                    keys.push(key);
                    songs.push(SongSaves { newest: save, older: Vec::new() });
                }
            }
        }
        songs
    }

    /// Bitwig's backups of it, in a folder below.
    pub fn backups(&self) -> impl Iterator<Item = &Save> {
        self.saves.iter().filter(|s| s.below)
    }

    /// The room its copies take.
    pub fn bytes(&self) -> u64 {
        self.copies.iter().fold(0u64, |sum, c| sum.saturating_add(c.bytes))
    }

    /// The name it's shown by: its folder's.
    pub fn name(&self) -> String {
        let text = self.folder.to_string_lossy();
        paths::file_name(text.trim_end_matches(SEPARATORS)).to_owned()
    }

    /// Where it is: the folder its folder is in, "K:\Bitwig Studio\Projects".
    pub fn place(&self) -> String {
        let text = self.folder.to_string_lossy();
        let text = text.trim_end_matches(SEPARATORS);
        text.rfind(SEPARATORS).map(|at| text[..at].to_owned()).unwrap_or_default()
    }
}

/// A path's separators, either way round: a path written on Windows is split the same way anywhere (the screenshots
/// are drawn on Linux).
const SEPARATORS: [char; 2] = ['\\', '/'];

/// What a look found: the Library folders the copies were compared with, how many project folders keep copies of
/// multisamples, and each project with copies that are the same as a Library file, biggest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Copies {
    pub library: Vec<PathBuf>,
    pub folders: usize,
    pub projects: Vec<Project>,
}

impl Copies {
    /// The room all the copies take.
    pub fn bytes(&self) -> u64 {
        self.projects.iter().fold(0u64, |sum, p| sum.saturating_add(p.bytes()))
    }

    /// How many copies there are.
    pub fn count(&self) -> usize {
        self.projects.iter().map(|p| p.copies.len()).sum()
    }
}

/// How often a look writes the fingerprints read so far to the store.
const KEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// Looks through the Bitwig projects in `roots` (your project folders) for copies of Library multisamples: the project
/// folders with a `multi-samples` folder first, then Bitwig's Library ([`library_folders`], from `studios`: your Bitwig
/// Studio folders, Settings' first), then each project's
/// copies. `index` is the last look's Library, kept so a file already read isn't read again while it's unchanged
/// (rule 12); the first look makes it, taking in the fingerprints kept in `prints` (the store in the app's data
/// folder, `mo_space::prints::FILE_NAME`) so a restart doesn't read them again either. What was read is written there
/// as it goes and at the end, also when stopped. `named` gives the Sounds library's sounds of a name, when it could be
/// read: each project's `samples` folder is then looked through too, for single sample copies of those sounds
/// (`mo_space::bitwig::sample_copies_in`). `progress` hears how many project folders have been looked through, of how
/// many. Reads the disks, so call it off the window's thread, inside the low-priority pool (rule 21).
#[allow(clippy::too_many_arguments)]
pub fn look(
    roots: &[PathBuf],
    studios: &[PathBuf],
    extra: &[PathBuf],
    index: &mut Option<LibraryIndex>,
    prints: Option<&Path>,
    named: Option<&bitwig::Named<'_>>,
    stop: &AtomicBool,
    progress: impl FnMut(usize, usize),
) -> Result<Copies, Stopped> {
    let started = std::time::Instant::now();
    let found = look_through(roots, studios, extra, index, prints, named, stop, progress);
    if let Some(index) = index.as_ref() {
        // A look that stopped forgets nothing; a whole one forgets the files it didn't ask about. One that found no
        // Library, or with a project folder away (a drive asleep or unplugged), isn't whole: what's there is kept for
        // when it's back.
        let whole = found.as_ref().is_ok_and(|f| !f.library.is_empty()) && roots.iter().all(|r| r.is_dir());
        if let Err(e) = index.keep(whole) {
            crate::log::warning("Space", &format!("Couldn't keep the Bitwig copies' fingerprints for next time: {e}"));
        }
    }
    if let Ok(found) = &found {
        crate::log::info(
            "Space",
            &format!(
                "Bitwig copies: {} project folders looked through in {:.1} s",
                found.folders,
                started.elapsed().as_secs_f64()
            ),
        );
    }
    found
}

#[allow(clippy::too_many_arguments)]
fn look_through(
    roots: &[PathBuf],
    studios: &[PathBuf],
    extra: &[PathBuf],
    index: &mut Option<LibraryIndex>,
    prints: Option<&Path>,
    named: Option<&bitwig::Named<'_>>,
    stop: &AtomicBool,
    mut progress: impl FnMut(usize, usize),
) -> Result<Copies, Stopped> {
    let folders = bitwig::project_folders(roots, stop)?;
    progress(0, folders.len());
    let library = if folders.is_empty() { Vec::new() } else { library_folders(&folders, studios, extra) };
    let mut found = Copies { library, folders: folders.len(), projects: Vec::new() };
    // Without a Library there's nothing to compare the multisamples with; the samples are still compared with the
    // Sounds library's.
    if found.library.is_empty() && named.is_none() {
        return Ok(found);
    }
    let index = match index {
        Some(known) => {
            known.list_again(&found.library, stop)?;
            known
        }
        None => index.insert(LibraryIndex::new(&found.library, stop)?),
    };
    // Tried at each look until it works (a store busy or unreadable once is tried again).
    if let Some(store) = prints.filter(|_| !index.remembers()) {
        match index.remember_in(store) {
            Ok(n) => crate::log::info("Space", &format!("Bitwig copies: {n} fingerprints kept from before")),
            Err(e) => crate::log::warning(
                "Space",
                &format!("The Bitwig copies' fingerprints are kept while the app runs only, for now: {e}"),
            ),
        }
    }
    // What's read is written to the store as it goes, not only at the end: a look takes minutes when much is new, and
    // one cut short by the app closing would otherwise read it all again at the next start.
    let mut kept_at = std::time::Instant::now();
    for (n, folder) in folders.iter().enumerate() {
        if index.remembers() && kept_at.elapsed() >= KEEP_EVERY {
            kept_at = std::time::Instant::now();
            if let Err(e) = index.keep(false) {
                crate::log::warning("Space", &format!("Couldn't keep the Bitwig copies' fingerprints so far: {e}"));
            }
        }
        let mut copies = index.copies_in(folder, stop)?;
        let samples = match named {
            Some(named) => bitwig::sample_copies_in(index, folder, named, stop)?,
            None => Vec::new(),
        };
        let has_samples = !samples.is_empty();
        copies.extend(samples);
        copies.sort_by(|a, b| a.in_project.cmp(&b.in_project));
        if !copies.is_empty() {
            let mut saves = bitwig::saves_in(folder, stop)?;
            if has_samples {
                // A Fix from before it pointed samples at their originals left them all in use.
                bitwig::fixed_before_samples(&mut saves);
            }
            found.projects.push(Project { folder: folder.clone(), copies, saves });
        }
        progress(n + 1, folders.len());
    }
    found.projects.sort_by(|a, b| b.bytes().cmp(&a.bytes()).then_with(|| a.folder.cmp(&b.folder)));
    Ok(found)
}

/// Bitwig's Library folders for these project folders: first the Library in each of `studios` (your Bitwig Studio
/// folder as Settings › DAWs names it, then the one in Documents, where Bitwig keeps it unless told otherwise:
/// `mo_space::bitwig::documents`), then the Library folder in each "Bitwig Studio" folder above a project (Solomon
/// keeps his Bitwig Studio folder on K:). Settings' comes first so Fix points copies at the Library Bitwig uses now
/// (Solomon, 11 Nov 2026). The Library folders added on Settings › DAWs (`extra`) come straight after the first of
/// `studios`. Only the ones that are there, each once.
pub fn library_folders(projects: &[PathBuf], studios: &[PathBuf], extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut add_library = |library: PathBuf| {
        let known = |f: &PathBuf| key(&f.to_string_lossy()) == key(&library.to_string_lossy());
        if library.is_dir() && !found.iter().any(known) {
            found.push(library);
        }
    };
    if let Some((first, _)) = studios.split_first() {
        add_library(first.join("Library"));
    }
    // The Library folders added on Settings › DAWs, after your Bitwig Studio folder's own.
    for library in extra {
        add_library(library.clone());
    }
    let mut add = |studio: &Path| add_library(studio.join("Library"));
    for studio in studios.iter().skip(1) {
        add(studio);
    }
    let is_studio = |folder: &&Path| folder.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Bitwig Studio"));
    for project in projects {
        project.ancestors().skip(1).filter(is_studio).for_each(&mut add);
    }
    found
}

/// "1 copy", "1,204 copies".
fn copies_words(n: usize) -> String {
    if n == 1 { "1 copy".to_owned() } else { format!("{} copies", thousands(n)) }
}

/// How far a look is: `done` of `of` project folders looked through (`of` is 0 while they're still being found).
pub fn looking_words((done, of): (usize, usize)) -> String {
    if of == 0 {
        "Looking for your Bitwig projects…".to_owned()
    } else {
        format!("Looking through your Bitwig projects: {} of {}", thousands(done), thousands(of))
    }
}

/// The activity card's line while a look goes on: "1,204 of 5,000 projects".
pub fn progress_words(done: usize, of: usize) -> String {
    format!("{} of {}", thousands(done), plural(of, "project"))
}

/// The Bitwig copies line under Biggest wins first, with the room the copies take (what the wins are sorted by), once
/// a look has found copies; or, while the first look is on (`looking`), how far it is. None when there are none.
pub fn line(found: Option<&Copies>, looking: Option<(usize, usize)>) -> Option<(u64, Line)> {
    let line = |why: String, size: String| Line { id: BITWIG_COPIES, title: "Bitwig sound copies", why, size };
    match (found, looking) {
        (Some(c), _) if !c.projects.is_empty() => Some((
            c.bytes(),
            line(
                format!("Copies of your multisamples and samples in {}", plural(c.projects.len(), "project")),
                size_text(c.bytes()),
            ),
        )),
        (None, Some(at)) => Some((0, line(looking_words(at), "Measuring…".to_owned()))),
        _ => None,
    }
}

/// What the page says under Biggest wins first about Bitwig copies: why there's no Bitwig line when a look found
/// none, or nothing.
pub fn none_note(found: Option<&Copies>) -> &'static str {
    match found {
        Some(c) if c.folders > 0 && c.library.is_empty() => {
            "Bitwig's Library folder wasn't found (the Library folder in Bitwig Studio), so its copies in your projects \
             can't be compared with it. "
        }
        Some(c) if c.projects.is_empty() => "No Bitwig project has copies of your multisamples or samples. ",
        _ => "",
    }
}

/// The Library folders, as a sentence says them: "K:\Bitwig Studio\Library", "A and B", "A, B and C".
fn library_words(library: &[PathBuf]) -> String {
    let names: Vec<String> = library.iter().map(|l| l.to_string_lossy().into_owned()).collect();
    if names.is_empty() { "your Bitwig Library".to_owned() } else { and_list(&names) }
}

/// What Fix did to a save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fixing {
    /// Waiting its turn, or being written now.
    Writing,
    /// Its "(fixed)" project was written.
    Written(PathBuf),
    /// Nothing was written, and why.
    Failed(String),
}

/// A save for Fix, with its project's copies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToFix {
    pub save: PathBuf,
    pub copies: Vec<LibraryCopy>,
    /// Bitwig's Library folders, as the look found them (Settings' first): where a file whose folder moved is looked
    /// for.
    pub libraries: Vec<PathBuf>,
    /// The save's own fixed project, when Fix writes the next one from it: it still uses copies a later look found
    /// ([`Fixes::still_uses`]). None writes from the save.
    pub from: Option<PathBuf>,
}

/// A project whose copies may go, once you've heard its fixed saves play.
#[derive(Clone, Debug)]
pub struct ToFree {
    pub project: Project,
    /// Each save you ticked as playing, with its fixed project.
    pub checked: Vec<(PathBuf, PathBuf)>,
    /// Bitwig's Library folders, as the look found them: a multisample's Library file must be in one.
    pub libraries: Vec<PathBuf>,
    /// Your say on the Songs page on which saves are one song: a save you split off, or a song you said is another,
    /// isn't left behind with a checked song's saves. Read from the library with [`ToFree::elsewhere`], off the
    /// window's thread.
    pub choices: SongChoices,
    /// The saves the library knows that use one of the project's copies by its path, with the copy's path as stored
    /// (`mo_library::Library::saves_using`): one in another folder keeps the copy. Err (and why) when the library
    /// couldn't be read: every copy stays then. Nothing goes until it's been read.
    pub elsewhere: Result<Vec<(String, String)>, String>,
    /// How far the library is with reading every Bitwig save again for the multisamples it finds outside its folder by
    /// their full path (`mo_library::song_writes::bitwig_reread`): until it's done, [`ToFree::elsewhere`] can't name a
    /// save in another folder that uses a multisample copy, so every multisample copy stays (the whole-app review of
    /// 19 Nov, FS-5). Read with it, off the window's thread.
    pub reread: BitwigReread,
    /// Each checked save's older fixed projects beside it that are still as Fix wrote them, with when each last
    /// changed: they're left behind with the song's older saves ([`older_fixed`]). Read with it, off the window's
    /// thread; none until then.
    pub older_fixed: Vec<(PathBuf, Option<SystemTime>)>,
}

impl ToFree {
    /// The copies' paths, as [`ToFree::elsewhere`] asks the library about them.
    pub fn copy_paths(&self) -> Vec<String> {
        self.project.copies.iter().map(|c| c.in_project.clone()).collect()
    }
}

/// The rows of one `saves_using` asked for every project's copies at once that name one of this project's copies,
/// matched as the library matches them (the whole-app review of 19 Nov, FS-12: the library is read once for them all).
pub fn rows_for(job: &ToFree, rows: &[(String, String)]) -> Vec<(String, String)> {
    let mine: HashSet<String> =
        job.project.copies.iter().map(|c| mo_library::plugins::sample_key(&c.in_project)).collect();
    rows.iter().filter(|(sample, _)| mine.contains(&mo_library::plugins::sample_key(sample))).cloned().collect()
}

/// The saves whose copies stop being found when a project's copies go (Solomon, 17 Nov 2026: he chose that they go
/// once each song using them is fixed and checked): every save of each checked song (the save itself and its older
/// ones), Bitwig's backups, and the song's older fixed projects still as Fix wrote them ([`older_fixed`]). Every other Bitwig file in the folder (another song's saves, a fixed project, a template
/// or clip) still keeps a copy it names. Newest first.
pub fn left_behind(job: &ToFree) -> Vec<PathBuf> {
    let checked: HashSet<&Path> = job.checked.iter().map(|(save, _)| save.as_path()).collect();
    let mut out = Vec::new();
    let mut songs: HashSet<String> = HashSet::new();
    // By name, as the card shows songs; then your say on the Songs page: an older save you split off or put under
    // another song isn't this song's, so it still keeps its copies.
    let chosen = |s: &Save| {
        let path = s.path.to_string_lossy();
        job.choices.key_of(&path, &job.project.folder.to_string_lossy(), "Bitwig", paths::stem(&path))
    };
    for song in job.project.songs() {
        if checked.contains(song.newest.path.as_path()) {
            let key = chosen(song.newest);
            songs.insert(song_key(&job.project, song.newest));
            songs.insert(key.clone());
            out.push(song.newest.path.clone());
            out.extend(song.older.iter().filter(|s| chosen(s) == key).map(|s| s.path.clone()));
        }
    }
    // Only Bitwig's own backups, in its auto-backups folder, and only a checked song's (by name, with the number
    // Bitwig adds taken off): any other project in a folder below, or another song's backup, still keeps its copies.
    out.extend(
        job.project
            .backups()
            .filter(|s| {
                in_auto_backups(&job.project, s) && {
                    let k = backup_song_key(&job.project, s);
                    songs.contains(&k) && songs.contains(&job.choices.follow(&k))
                }
            })
            .map(|s| s.path.clone()),
    );
    out.extend(job.older_fixed.iter().map(|(f, _)| f.clone()));
    out
}

/// Each checked save's older fixed projects beside it ("X (fixed)" once "X (fixed 2)" is the one you heard), newest
/// first, with when each last changed: only ones still as Fix wrote them (on the app's note in `note`, unchanged since).
/// They're older saves of the song too, left behind with it: Fix writes the next one from a fixed project that still
/// uses copies a later look found (the whole-app review of 19 Nov, FS-3), and the one before would keep those for
/// ever. One you've saved in since is yours, and keeps the copies it names. Reads the disk: call it off the window's
/// thread.
pub fn older_fixed(job: &ToFree, note: &Path) -> Vec<(PathBuf, Option<SystemTime>)> {
    let mut out = Vec::new();
    for (save, fixed) in &job.checked {
        let (Some(dir), name) = (save.parent(), paths::stem(&save.to_string_lossy()).to_owned()) else { continue };
        let Ok(listing) = std::fs::read_dir(dir) else { continue };
        let mut here: Vec<(PathBuf, Option<SystemTime>)> = listing
            .flatten()
            .map(|e| e.path())
            .filter(|path| {
                let project = path.extension().is_some_and(|x| x.eq_ignore_ascii_case("bwproject"));
                let of_it = bitwig::fixed_from(paths::stem(&path.to_string_lossy()))
                    .is_some_and(|from| from.eq_ignore_ascii_case(&name));
                project && of_it && key(&path.to_string_lossy()) != key(&fixed.to_string_lossy())
            })
            .filter(|path| crate::space_fix_note::as_noted(note, path))
            .map(|path| {
                let at = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                (path, at)
            })
            .collect();
        here.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out.extend(here);
    }
    out
}

/// Bitwig's folder for its automatic backups, in a project's folder.
const AUTO_BACKUPS: &str = "auto-backups";

/// Whether a save below a project is in its own auto-backups folder.
fn in_auto_backups(project: &Project, save: &Save) -> bool {
    save.path.parent().is_some_and(|p| {
        p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(AUTO_BACKUPS))
            && p.parent().is_some_and(|f| key(&f.to_string_lossy()) == key(&project.folder.to_string_lossy()))
    })
}

/// The song a save beside a project is of, as [`Project::songs`] tells them.
fn song_key(project: &Project, save: &Save) -> String {
    let name = paths::stem(&save.path.to_string_lossy()).to_owned();
    mo_library::songs::key(&project.folder.to_string_lossy(), "Bitwig", &name)
}

/// The song one of Bitwig's backups is of: its name with the "-3" Bitwig adds taken off, as for a save beside it.
fn backup_song_key(project: &Project, save: &Save) -> String {
    let stem = paths::stem(&save.path.to_string_lossy()).to_owned();
    let name = match stem.rsplit_once('-') {
        Some((before, n)) if !before.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => before,
        _ => stem.as_str(),
    };
    mo_library::songs::key(&project.folder.to_string_lossy(), "Bitwig", name)
}

/// Why a project's copies can't be freed now, from the disk just before they'd go: one of its saves has been saved
/// again since the look (an older one too: your newest work may be in it now, and it would lose them), or a checked
/// save's fixed project is gone or older than it. A time within [`crate::space_fix_note::SAME_TIME`] of the look's is
/// the same save (a folder's listing and the file itself can give a time a little apart).
fn changed_since(job: &ToFree) -> Option<String> {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let near = |a: SystemTime, b: SystemTime| {
        a.duration_since(b).or_else(|_| b.duration_since(a)).is_ok_and(|d| d <= crate::space_fix_note::SAME_TIME)
    };
    for save in job.project.saves.iter().filter(|s| !s.below) {
        let name = paths::file_name(&save.path.to_string_lossy()).to_owned();
        let same = match (modified(&save.path), save.modified) {
            (Some(now), Some(looked)) => near(now, looked),
            _ => false,
        };
        if !same {
            return Some(format!(
                "\"{name}\" was saved again since the page looked, so the copies stay: measure again."
            ));
        }
    }
    // A song's older fixed project left behind with it, saved in since it was read: your work may be in it now.
    for (older, looked) in &job.older_fixed {
        if modified(older) != *looked {
            let name = paths::file_name(&older.to_string_lossy()).to_owned();
            return Some(format!(
                "\"{name}\" was saved again since the page looked, so the copies stay: measure again."
            ));
        }
    }
    for (save, fixed) in &job.checked {
        let name = paths::file_name(&save.to_string_lossy()).to_owned();
        let now = modified(save);
        if modified(fixed).is_none_or(|f| now.is_some_and(|n| f < n)) {
            return Some(format!("\"{name}\"'s fixed project is gone or older than it now, so its copies stay."));
        }
    }
    None
}

/// Sends the copies a project's checked fixed saves no longer use to the Recycle Bin (`mo_space::bitwig::
/// remove_copies`), each checked again just before it goes: still the same as what's used instead, named by no Bitwig
/// file in the folder but those [`left_behind`] lists, and used by no save in another folder the library knows of. The
/// saves themselves stay. Reads the disks, so call it off the window's thread.
pub fn free(job: &ToFree, bin: &dyn mo_space::clean::Bin, stop: &AtomicBool) -> mo_space::clean::Done {
    let mut done = mo_space::clean::Done::default();
    let Some((original, fixed)) = job.checked.first() else { return done };
    let everyone = |why: &str| -> Vec<mo_space::clean::Problem> {
        job.project
            .copies
            .iter()
            .map(|c| mo_space::clean::Problem { path: c.in_project.clone(), why: why.to_owned() })
            .collect()
    };
    if let Some(why) = changed_since(job) {
        done.problems.extend(everyone(&why));
        return done;
    }
    let elsewhere = match &job.elsewhere {
        Ok(e) => e,
        Err(why) => {
            done.problems.extend(everyone(&format!("The library couldn't be read to see who else uses it ({why}).")));
            return done;
        }
    };
    // Without the drive letter: the library keeps a save under the letter its drive had when it was read.
    let folder = without_drive(key(&job.project.folder.to_string_lossy()));
    let inside = |save: &str| {
        let k = without_drive(key(save));
        k.strip_prefix(&folder).is_some_and(|rest| rest.starts_with('/'))
    };
    // The same words for every copy, so the card can give them when nothing went (FS-15).
    let not_yet = match job.reread {
        BitwigReread::Done => None,
        BitwigReread::Waiting => Some(
            "The app is still reading your Bitwig saves again to see which use a multisample copy from another \
             folder, so the multisample copies stay. Try again once that's done (the Songs page's look through \
             your folders)."
                .to_owned(),
        ),
        BitwigReread::Owed(n) => Some(format!(
            "{} couldn't be read again (outside your project folders now, or on a drive that's away), so whether \
             {} a multisample copy from another folder isn't known and the multisample copies stay. {} tried \
             again at each start.",
            plural(n, "Bitwig save"),
            if n == 1 { "it uses" } else { "they use" },
            if n == 1 { "It's" } else { "They're" }
        )),
        BitwigReread::NotOwn => Some(
            "The app doesn't keep its own song library yet, so which saves in other folders use a multisample copy \
             can't be told, and the multisample copies stay."
                .to_owned(),
        ),
        BitwigReread::Unknown => Some(
            "The song library couldn't be read just now, so the multisample copies stay. Try again in a moment."
                .to_owned(),
        ),
    };
    // Worked out once for the project, not for each copy (FS-10): the copies a save the library knows still uses,
    // with that save. A save in another folder that names a copy by its full path (saved without copying its samples)
    // keeps it; so does a save in the folder that the folder's own check below doesn't read, one an FL Studio or
    // Ableton save made there, say (FS-17). A save whose file isn't there keeps it too while its drive is away (or has
    // another letter now): that isn't a save that's gone. On a drive that's here, it's gone, even with its folder.
    let bitwig_file = |save: &str| {
        Path::new(save).extension().is_some_and(|x| relink::BITWIG_FILES.iter().any(|b| x.eq_ignore_ascii_case(b)))
    };
    let mut kept_by: HashMap<String, (&str, bool)> = HashMap::new();
    for (sample, save) in elsewhere {
        if inside(save) && bitwig_file(save) {
            continue;
        }
        let there = Path::new(save).is_file();
        if !there && !drive_away(Path::new(save)) {
            continue;
        }
        kept_by.entry(without_drive(key(sample))).or_insert((save.as_str(), there));
    }
    let used_elsewhere = |copy: &Path| -> Option<String> {
        let name = paths::file_name(&copy.to_string_lossy()).to_owned();
        let multisample = copy
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(relink::COPY_FOLDERS[0]));
        if multisample && let Some(why) = &not_yet {
            return Some(why.clone());
        }
        kept_by.get(&without_drive(key(&copy.to_string_lossy()))).map(|(save, there)| {
            if *there {
                format!("{save} uses {name} too, so it stays.")
            } else {
                format!(
                    "{save} uses {name} too, and its folder can't be opened now (a drive that's away?), so it stays."
                )
            }
        })
    };
    let left: HashSet<String> = left_behind(job).iter().map(|p| key(&p.to_string_lossy())).collect();
    let left_out = |p: &Path| {
        let full = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        left.contains(&key(&full.to_string_lossy())) || left.contains(&key(&p.to_string_lossy()))
    };
    let changed: Vec<(PathBuf, PathBuf)> =
        job.project.copies.iter().map(|c| (PathBuf::from(&c.in_project), PathBuf::from(&c.library_file))).collect();
    let relinks: Vec<Relink> = changed.iter().map(|(c, l)| Relink { copy: c.clone(), library: l.clone() }).collect();
    // Every other Bitwig file in the folder is read once for all the copies, and read again only when one of them has
    // changed since (a size or a time, or a file come or gone): each copy is asked just before it goes, and again
    // before Windows asks to send what it guards, so a save made meanwhile is still seen.
    let ask = || -> (Result<relink::FilesNow, String>, HashMap<String, Option<String>>) {
        let files = relink::bitwig_files_now(original);
        // And each checked song's fixed project must use the file the copy is the same as now (FS-2). When the file
        // that still names the copy is a checked fixed project itself, that's said first: Fix is offered again from it
        // (FS-3).
        let uses = relink::fixed_use_originals(&job.checked, &relinks);
        let by_fixed: Vec<String> = job
            .checked
            .iter()
            .filter_map(|(_, f)| f.file_name().map(|n| format!("{} still uses ", n.to_string_lossy())))
            .collect();
        let first = |n: Option<String>, u: Option<String>| match n {
            Some(n) if by_fixed.iter().any(|b| n.starts_with(b.as_str())) => Some(n),
            n => u.or(n),
        };
        let named = relink::still_named_except(original, &relinks, &left_out);
        let why = relinks
            .iter()
            .map(|r| key(&r.copy.to_string_lossy()))
            .zip(named.into_iter().zip(uses).map(|(n, u)| first(n, u)))
            .collect();
        (files, why)
    };
    let asked = std::sync::Mutex::new(ask());
    let still_named = |copy: &Path, _: &Path| {
        if let Some(why) = used_elsewhere(copy) {
            return Some(why);
        }
        // A save of the checked songs made while Free runs (one Ctrl+S in Bitwig) stops the rest going (FS-18): it's
        // one look at each save's time, not a read.
        if let Some(why) = changed_since(job) {
            return Some(why);
        }
        // The folder is listed again for each copy (it reads no file): a Bitwig file made or saved a moment ago is
        // still seen. Only what the library knows is worked out once (FS-10).
        let mut asked = asked.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = relink::bitwig_files_now(original);
        if now.is_err() || now != asked.0 {
            *asked = ask();
        }
        if let Err(why) = &asked.0 {
            return Some(format!("{why}, so it stays."));
        }
        asked.1.get(&key(&copy.to_string_lossy())).cloned().unwrap_or_else(|| Some("It wasn't asked about.".to_owned()))
    };
    let fixed = bitwig::FixedProject {
        file: fixed,
        changed: &changed,
        library_folders: &job.libraries,
        still_named: &still_named,
    };
    bitwig::remove_copies(&fixed, bin, stop)
}

/// Each checked save whose fixed project still uses some of the project's copies that Fix could point at their
/// originals now, with that fixed project and how many: copies a look found after Fix ran (a sample the Sounds library
/// learned since, a Library folder added in Settings). Free the space keeps them while the fixed project names them,
/// so Fix is offered again ([`Fixes::still_uses`]), writing from that fixed project (the whole-app review of 19 Nov,
/// FS-3). Reads each checked fixed project once, so call it off the window's thread.
pub fn fixed_still_using(job: &ToFree) -> Vec<(PathBuf, PathBuf, usize)> {
    let relinks: Vec<Relink> = job
        .project
        .copies
        .iter()
        .map(|c| Relink { copy: PathBuf::from(&c.in_project), library: PathBuf::from(&c.library_file) })
        .collect();
    job.checked
        .iter()
        .filter_map(|(save, fixed)| {
            let n = relink::could_point(fixed, &relinks);
            (n > 0).then(|| (save.clone(), fixed.clone(), n))
        })
        .collect()
}

/// Whether the drive a path is on isn't here now (unplugged, or another letter now): on Windows its drive's root isn't
/// there; elsewhere, nothing on the path is there below a folder drives are put in (`/Volumes`, `/media`, `/mnt`,
/// `/run/media`, or the top).
fn drive_away(path: &Path) -> bool {
    if cfg!(windows) {
        return path.ancestors().last().is_none_or(|root| !root.is_dir());
    }
    const MOUNTS: [&str; 5] = ["/", "/Volumes", "/media", "/mnt", "/run/media"];
    let here = path.ancestors().skip(1).find(|a| a.is_dir());
    here.is_none_or(|h| {
        MOUNTS.iter().any(|m| h == Path::new(m))
            || h.parent().is_some_and(|p| p == Path::new("/media") || p == Path::new("/run/media"))
    })
}

/// A path as Space compares them, without its drive letter and with `/` between folders: the library keeps the path a
/// save had when it was read, and the drive may have had another letter then.
fn without_drive(k: String) -> String {
    let k = k.replace('\\', "/");
    match k.as_bytes() {
        [d, b':', ..] if d.is_ascii_alphabetic() => k[2..].to_owned(),
        _ => k,
    }
}

/// [`free`], with the app's note in `note` kept as it goes, for the Free the space that runs at `when` (the time the
/// app's list of what it put in the Recycle Bin keeps it under; [`crate::space_fix_note::moved`] when that turns out
/// to be another): each checked song's fixed project is marked before the first copy goes, so a crash, a power cut or
/// a kill part way through leaves the mark on (the whole-app review of 19 Nov, FS-23), and the button for every fixed
/// project never sends it ([`crate::space_fix_note::freed`]): it's the one that plays now. Afterwards the mark comes
/// off one whose save named none of the copies that went. When the note can't be read, nothing goes.
pub fn free_noted(
    job: &ToFree,
    bin: &dyn mo_space::clean::Bin,
    stop: &AtomicBool,
    note: &Path,
    when: i64,
) -> mo_space::clean::Done {
    for (_, fixed) in &job.checked {
        if let Err(why) = crate::space_fix_note::freed(note, fixed, when) {
            let mut done = mo_space::clean::Done::default();
            let why = format!("The app's note of the fixed projects couldn't be kept ({why}), so the copies stay.");
            done.problems = job
                .project
                .copies
                .iter()
                .map(|c| mo_space::clean::Problem { path: c.in_project.clone(), why: why.clone() })
                .collect();
            let marked: Vec<PathBuf> = job.checked.iter().map(|(_, f)| f.clone()).collect();
            crate::space_fix_note::unfreed(note, &marked, when);
            return done;
        }
    }
    let done = free(job, bin, stop);
    let went: Vec<Relink> = done
        .removed
        .iter()
        .filter_map(|r| {
            let copy = PathBuf::from(&r.path);
            let c = job.project.copies.iter().find(|c| key(&c.in_project) == key(&r.path))?;
            Some(Relink { copy, library: PathBuf::from(&c.library_file) })
        })
        .collect();
    let none_went: Vec<PathBuf> = job
        .checked
        .iter()
        .filter(|(save, _)| went.is_empty() || !relink::names_any(save, &went))
        .map(|(_, fixed)| fixed.clone())
        .collect();
    if !none_went.is_empty() {
        crate::space_fix_note::unfreed(note, &none_went, when);
    }
    done
}

/// Writes a "(fixed)" project beside a save that uses the Library's own files instead of its project's copies
/// (`mo_convert::relink`): never over a file that's there, and only once it reads back as a Bitwig project. The save and
/// its copies aren't changed (rule 3). Every copy of the project goes in the one call: a copy the save doesn't use is
/// left alone. What it wrote goes on the app's note in `note` ([`crate::space_fix_note`]), so the button that sends
/// every fixed project to the Recycle Bin can tell it from one you've saved in since. Reads and writes the disk, so
/// call it off the window's thread, inside the low-priority pool (rule 21).
pub fn fix(job: &ToFix, note: &Path, stop: &AtomicBool) -> Fixing {
    let relinks: Vec<Relink> = job
        .copies
        .iter()
        .map(|c| Relink { copy: PathBuf::from(&c.in_project), library: PathBuf::from(&c.library_file) })
        .collect();
    // The app closing stops it before it writes (the whole-app review of 19 Nov, FS-9).
    let fixed = match &job.from {
        Some(from) => relink::fix_bitwig_project_from(&job.save, from, &relinks, &job.libraries, stop),
        None => relink::fix_bitwig_project_until(&job.save, &relinks, &job.libraries, stop),
    };
    match fixed {
        Ok(fixed) => {
            crate::space_fix_note::wrote(note, &fixed.file);
            for line in &fixed.found {
                crate::log::info("Space", &format!("Fix found again in {}: {line}", job.save.display()));
            }
            // The copies it left, and why (the whole-app review of 19 Nov, FS-3: never said); the folder's copies this
            // save doesn't use, only counted (each save of a folder is given them all).
            let unused = relink::UNUSED;
            let (not_used, said): (Vec<&String>, Vec<&String>) = fixed.left.iter().partition(|l| l.ends_with(unused));
            for line in said {
                crate::log::info("Space", &format!("Fix left in {}: {line}", job.save.display()));
            }
            if !not_used.is_empty() {
                crate::log::info(
                    "Space",
                    &match &job.from {
                        Some(from) => format!(
                            "Fix left in {}: {} copies {} doesn't use (already pointed at the Library, or not this \
                             song's)",
                            job.save.display(),
                            not_used.len(),
                            from.display()
                        ),
                        None => format!(
                            "Fix left in {}: {} copies this save doesn't use",
                            job.save.display(),
                            not_used.len()
                        ),
                    },
                );
            }
            Fixing::Written(fixed.file)
        }
        Err(why) => Fixing::Failed(why),
    }
}

/// What the card knows while the app runs: what Fix did to each save this time, the saves you ticked or unticked, and
/// the projects whose other saves are on show.
#[derive(Clone, Debug, Default)]
pub struct Fixes {
    /// By the save's path.
    pub fixing: HashMap<PathBuf, Fixing>,
    /// By the save's path. A save you haven't changed has the tick it has out of the box: its project's newest save is
    /// ticked, the others aren't.
    pub ticks: HashMap<PathBuf, bool>,
    /// By the project's folder.
    pub shown: HashSet<PathBuf>,
    /// The fixed saves you've ticked as heard playing, by the save's path, with the fixed project you heard: the tick
    /// counts only while that's still its fixed project.
    pub heard: HashMap<PathBuf, PathBuf>,
    /// The fixed saves whose fixed project still uses copies a later look found (a sample the Sounds library learned
    /// after Fix, a Library folder added since), as Free the space found them: by the save's path, that fixed project
    /// and how many it could point at their originals. Each counts only while it's still the save's fixed project; Fix
    /// can then write the next one from it (the whole-app review of 19 Nov, FS-3).
    still_uses: HashMap<PathBuf, (PathBuf, usize)>,
    /// From the moment Fix starts until it has finished, as planned or not.
    running: bool,
}

/// Where a save is, as its line says.
enum State<'a> {
    Writing,
    /// Its fixed project, as new as it or newer.
    Fixed(&'a Path),
    /// Its fixed project, as new as it, still uses this many copies a later look found: Fix writes the next one from
    /// it. With why, when Fix tried and couldn't.
    StillUses(&'a Path, usize, Option<&'a str>),
    Failed(&'a str),
    /// Its fixed project was written before it was saved again.
    Outdated(&'a Path),
    /// Not fixed yet.
    Open,
}

impl Fixes {
    /// Fix starts on `jobs`: each save is being written until Fix says how it went ([`done`](Self::done)), and Fix is
    /// busy until it has [`finished`](Self::finished).
    pub fn start(&mut self, jobs: &[ToFix]) {
        self.running = true;
        for job in jobs {
            self.fixing.insert(job.save.clone(), Fixing::Writing);
        }
    }

    /// Whether Fix is on: no other Fix can start, and a look that arrives meanwhile may have started before Fix wrote.
    pub fn busy(&self) -> bool {
        self.running || self.fixing.values().any(|f| *f == Fixing::Writing)
    }

    fn state<'a>(&'a self, save: &'a Save) -> State<'a> {
        let fixing = self.fixing.get(&save.path);
        let state = match (fixing, &save.fixed) {
            (Some(Fixing::Writing), _) => State::Writing,
            // Written this time: fixed, unless a look has found that very file older than the save (saved again
            // since). A look that found no fixed project, or an older one, started before Fix wrote.
            (Some(Fixing::Written(file)), Some((on_disk, false))) if on_disk == file => State::Outdated(file),
            (Some(Fixing::Written(file)), _) => State::Fixed(file),
            (_, Some((file, true))) => State::Fixed(file),
            (Some(Fixing::Failed(why)), _) => State::Failed(why),
            (_, Some((file, false))) => State::Outdated(file),
            _ => State::Open,
        };
        match (state, self.still_uses.get(&save.path)) {
            (State::Fixed(file), Some((still, n))) if still == file => {
                let failed = match fixing {
                    Some(Fixing::Failed(why)) => Some(why.as_str()),
                    _ => None,
                };
                State::StillUses(file, *n, failed)
            }
            (state, _) => state,
        }
    }

    /// A save's tick: None when it can't be fixed now (it's fixed or being written, or it's one of Bitwig's backups
    /// in a folder below, which the converter only fixes beside the project); else whether it's ticked. Out of the box
    /// that's each song's `newest` save, unless Fix has just failed on it: pressing Fix again wouldn't help until
    /// something changes, so it waits for you to tick it again.
    ///
    /// A fixed save's tick says you've opened its fixed project in Bitwig and heard it play: its copies may then go
    /// to the Recycle Bin ([`to_free`](Self::to_free)). It's never ticked out of the box.
    fn tick(&self, newest: bool, save: &Save) -> Option<bool> {
        let state = self.state(save);
        if let State::Fixed(file) = state {
            // Only a song's newest save frees its copies; while Fix writes, nothing can be ticked.
            return (newest && !save.below && !self.running).then(|| self.heard.get(&save.path) == Some(&file.into()));
        }
        if save.below || !matches!(state, State::Failed(_) | State::Outdated(_) | State::StillUses(..) | State::Open) {
            return None;
        }
        let out_of_the_box = newest && !matches!(state, State::Failed(_) | State::StillUses(_, _, Some(_)));
        Some(self.ticks.get(&save.path).copied().unwrap_or(out_of_the_box))
    }

    /// How a save went, from Fix: kept for its line. One that couldn't be fixed loses any tick you gave it.
    pub fn done(&mut self, save: PathBuf, fixing: Fixing) {
        // A tick for Fix isn't a tick for "it plays": a save fixed now starts unticked.
        if matches!(fixing, Fixing::Failed(_) | Fixing::Written(_)) {
            self.ticks.remove(&save);
            self.heard.remove(&save);
        }
        // Written from its fixed project: that one no longer counts. One Fix couldn't write from it still says why,
        // and Fix can be tried again.
        if matches!(fixing, Fixing::Written(_)) {
            self.still_uses.remove(&save);
        }
        self.fixing.insert(save, fixing);
    }

    /// Fix has finished, as planned or not: a save it didn't get to (it stopped with an error) says so.
    pub fn finished(&mut self) {
        self.running = false;
        for fixing in self.fixing.values_mut() {
            if *fixing == Fixing::Writing {
                *fixing = Fixing::Failed(STOPPED.to_owned());
            }
        }
    }

    /// A look arrived. Once Fix has finished (the page looks again then), the look saw everything Fix wrote, so what's
    /// on disk counts from now on: what Fix wrote this time is forgotten, and a fixed project deleted since shows as
    /// the look found it. Why a save couldn't be fixed stays. While Fix is on, the look may have started before Fix
    /// wrote, so what Fix wrote is kept.
    pub fn looked(&mut self) {
        if !self.busy() {
            self.fixing.retain(|_, fixing| !matches!(fixing, Fixing::Written(_)));
        }
    }

    /// Each save of a project with its tick ([`tick`](Self::tick)), in the order of its saves.
    fn ticks<'a>(&self, project: &'a Project) -> Vec<(&'a Save, Option<bool>)> {
        let newest: HashSet<&Path> = project.songs().iter().map(|s| s.newest.path.as_path()).collect();
        project.saves.iter().map(|s| (s, self.tick(newest.contains(s.path.as_path()), s))).collect()
    }

    /// Whether Fix can write for a save now (not fixed, or its fixed project is out of date or still uses copies a
    /// later look found, or Fix failed on it).
    fn fixable(&self, save: &Save) -> bool {
        matches!(self.state(save), State::Failed(_) | State::Outdated(_) | State::StillUses(..) | State::Open)
    }

    /// What Free the space found ([`fixed_still_using`]): each checked save whose fixed project still uses copies a later
    /// look found, with that fixed project and how many. Fix is offered for them again, ticked; a save not in the
    /// list loses an earlier one.
    pub fn still_uses(&mut self, found: Vec<(PathBuf, PathBuf, usize)>, checked: &[PathBuf]) {
        for save in checked {
            self.still_uses.remove(save);
        }
        for (save, fixed, n) in found {
            self.heard.remove(&save);
            self.ticks.remove(&save);
            self.still_uses.insert(save, (fixed, n));
        }
    }

    /// The ticked saves Fix can write for, project by project, each with its project's copies.
    pub fn to_fix(&self, found: &Copies) -> Vec<ToFix> {
        found
            .projects
            .iter()
            .flat_map(|p| {
                let ticked = self.ticks(p).into_iter().filter(|(s, t)| *t == Some(true) && self.fixable(s));
                ticked.map(|(s, _)| ToFix {
                    save: s.path.clone(),
                    copies: p.copies.clone(),
                    libraries: found.library.clone(),
                    from: match self.state(s) {
                        State::StillUses(f, ..) => Some(f.to_path_buf()),
                        _ => None,
                    },
                })
            })
            .collect()
    }

    /// Ticks or unticks a save, as its line's tick box asks: a fixed save's tick says you heard its fixed project play
    /// (kept with that fixed project), any other's whether Fix writes for it.
    pub fn set_tick(&mut self, found: Option<&Copies>, save: PathBuf, ticked: bool) {
        let fixed =
            found.into_iter().flat_map(|f| &f.projects).flat_map(|p| &p.saves).find(|s| s.path == save).and_then(|s| {
                match self.state(s) {
                    State::Fixed(file) => Some(file.to_path_buf()),
                    _ => None,
                }
            });
        match (fixed, ticked) {
            (Some(file), true) => {
                self.heard.insert(save, file);
            }
            (Some(_), false) => {
                self.heard.remove(&save);
            }
            (None, _) => {
                self.ticks.insert(save, ticked);
            }
        }
    }

    /// A project's fixed saves ticked as heard playing (each a song's newest save), with their fixed projects.
    fn heard_in(&self, project: &Project) -> Vec<(PathBuf, PathBuf)> {
        self.ticks(project)
            .into_iter()
            .filter(|(_, t)| *t == Some(true))
            .filter_map(|(s, _)| match self.state(s) {
                State::Fixed(f) => Some((s.path.clone(), f.to_path_buf())),
                _ => None,
            })
            .collect()
    }

    /// The projects whose copies can go, for the fixed saves you've ticked as heard playing: each project once, with
    /// every save of it ticked so.
    pub fn to_free(&self, found: &Copies) -> Vec<ToFree> {
        found
            .projects
            .iter()
            .filter_map(|p| {
                let checked = self.heard_in(p);
                (!checked.is_empty()).then(|| ToFree {
                    project: p.clone(),
                    checked,
                    libraries: found.library.clone(),
                    choices: SongChoices::default(),
                    elsewhere: Err("the library wasn't asked yet".to_owned()),
                    reread: BitwigReread::Waiting,
                    older_fixed: Vec::new(),
                })
            })
            .collect()
    }

    /// Fix's button: its words and whether it can be pressed now. None when no save can be fixed.
    pub fn button(&self, found: Option<&Copies>) -> Option<(String, bool)> {
        if self.busy() {
            return Some(("Fixing…".to_owned(), false));
        }
        let found = found?;
        let any = found.projects.iter().any(|p| self.ticks(p).iter().any(|(s, t)| t.is_some() && self.fixable(s)));
        let ticked = self.to_fix(found).len();
        any.then(|| {
            if ticked == 0 { ("Fix".to_owned(), false) } else { (format!("Fix {}", plural(ticked, "save")), true) }
        })
    }
}

/// A save's name on its project's card: its file name, or its place in the project's folder when it's in a folder
/// below ("auto-backups\GARAGE-3.bwproject").
fn save_name(project: &Project, save: &Save) -> String {
    let text = save.path.to_string_lossy();
    let folder = project.folder.to_string_lossy();
    match text.strip_prefix(folder.trim_end_matches(SEPARATORS)) {
        Some(inside) if save.below => inside.trim_start_matches(SEPARATORS).to_owned(),
        _ => paths::file_name(&text).to_owned(),
    }
}

/// "Saved 3 Nov 2026, 19:43" in this computer's time; "" when the time can't be read.
pub fn saved_words(modified: Option<SystemTime>) -> String {
    modified
        .map(|t| format!("Saved {}", chrono::DateTime::<chrono::Local>::from(t).format("%-d %b %Y, %H:%M")))
        .unwrap_or_default()
}

/// The most of why a save couldn't be fixed that its line shows; the log keeps all of it.
const WHY_SHOWN: usize = 280;

/// Why a save Fix didn't get to wasn't fixed: Fix stopped first, with an error the log has.
const STOPPED: &str = "Fix stopped before it was written; the log says why";

/// How a save's line speaks for it.
#[derive(Clone, Copy)]
enum Speaks<'a> {
    /// The project's own line, for the newest save of its one song; with how many other saves the project has.
    Project(usize),
    /// A song's own line, in a folder with more than one song.
    Song,
    /// An older save, of the song whose newest save this is (None for a backup, or in a folder with one song).
    Older(Option<&'a Save>),
}

/// What a save's line says about it, and whether that's a problem.
fn state_words(project: &Project, save: &Save, state: &State<'_>, speaks: Speaks<'_>) -> (String, bool) {
    let newest = !matches!(speaks, Speaks::Older(_));
    let file = |f: &Path| paths::file_name(&f.to_string_lossy()).to_owned();
    match state {
        State::Writing if newest => (format!("Writing a fixed project for \"{}\"…", save_name(project, save)), false),
        State::Writing => ("Writing its fixed project…".to_owned(), false),
        State::Fixed(f) => (
            format!("Fixed: \"{}\". Open it in Bitwig and check it plays, then tick it so its copies can go.", file(f)),
            false,
        ),
        State::Failed(why) => {
            let mut why = why.to_string();
            if why.chars().count() > WHY_SHOWN {
                why = format!("{}… (the log has it all)", why.chars().take(WHY_SHOWN).collect::<String>());
            }
            (format!("Not fixed: {why}"), true)
        }
        State::StillUses(f, _, Some(why)) => {
            let mut why = why.to_string();
            if why.chars().count() > WHY_SHOWN {
                why = format!("{}… (the log has it all)", why.chars().take(WHY_SHOWN).collect::<String>());
            }
            (
                format!(
                    "\"{}\" still uses copies found since it was fixed, but Fix couldn't write from it: {why}",
                    file(f)
                ),
                true,
            )
        }
        State::StillUses(f, n, None) => (
            format!(
                "\"{}\" still uses {} found since it was fixed, which can use their originals now: Fix writes a new \
                 fixed project from it.",
                file(f),
                copies_words(*n)
            ),
            false,
        ),
        State::Outdated(f) => {
            (format!("\"{}\" is older than this save or than the newest Fix: fix it again.", file(f)), false)
        }
        State::Open if matches!(speaks, Speaks::Song) => ("The newest save of this song.".to_owned(), false),
        State::Open if save.below => (BACKUP.to_owned(), false),
        State::Open if let Speaks::Older(Some(of)) = speaks => {
            (format!("An older save of \"{}\".", save_name(project, of)), false)
        }
        State::Open if let Speaks::Project(others) = speaks => {
            let also = if others == 0 {
                String::new()
            } else {
                format!(" Its {} may use these copies too.", plural(others, "other save"))
            };
            (format!("The tick is for its newest save, \"{}\".{also}", save_name(project, save)), false)
        }
        State::Open => (String::new(), false),
    }
}

/// What a backup's line says: Bitwig's auto-backups sit in a folder below the project, and the converter only fixes a
/// save beside the project's own `multi-samples` folder.
const BACKUP: &str = "Bitwig's own backup. Fix only writes beside the project's own saves, so it can't be fixed where \
    it is.";

/// What a project's line says when no save of its own is beside it (only "(fixed)" ones, or backups below).
const NO_SAVE: &str = "No save of its own is beside it, only fixed projects or backups, so there's nothing to fix.";

/// The Bitwig copies card: the room they take, the Library they're the same as, each project's copies biggest first
/// with its newest save's tick and how it's doing (or in a folder with more than one song, a line for each song's
/// newest save), then (when you show them) its other saves, and what Fix does. While
/// the first look is on (`looking`), how far it is.
pub fn card(found: Option<&Copies>, looking: Option<(usize, usize)>, fixes: &Fixes) -> Card {
    let projects = found.map(|c| c.projects.as_slice()).unwrap_or_default();
    let mut parts = Vec::new();
    for p in projects {
        let place = Some(p.place()).filter(|at| !at.is_empty()).map(|at| format!(", in {at}")).unwrap_or_default();
        let mut line = Part {
            name: p.name(),
            what: format!("{}{place}", copies_words(p.copies.len())),
            size: size_text(p.bytes()),
            ..Part::default()
        };
        let songs = p.songs();
        let backups: Vec<&Save> = p.backups().collect();
        let others = songs.iter().map(|s| s.older.len()).sum::<usize>() + backups.len();
        let ticks: HashMap<&Path, Option<bool>> =
            fixes.ticks(p).into_iter().map(|(s, t)| (s.path.as_path(), t)).collect();
        let tick_of =
            |save: &Save| ticks.get(save.path.as_path()).copied().flatten().map(|t| (vec![save.path.clone()], t));
        let save_line = |save: &Save, speaks: Speaks<'_>| {
            let state = fixes.state(save);
            let (status, problem) = state_words(p, save, &state, speaks);
            Part {
                name: save_name(p, save),
                what: saved_words(save.modified),
                under: true,
                tick: tick_of(save),
                status,
                problem,
                open: match state {
                    State::Fixed(f) | State::StillUses(f, ..) => Some((Opens::InBitwig, f.to_path_buf())),
                    _ => None,
                },
                ..Part::default()
            }
        };
        match songs.as_slice() {
            [] => line.status = NO_SAVE.to_owned(),
            [one] => {
                let state = fixes.state(one.newest);
                (line.status, line.problem) = state_words(p, one.newest, &state, Speaks::Project(others));
                line.tick = tick_of(one.newest);
                if let State::Fixed(f) | State::StillUses(f, ..) = state {
                    line.open = Some((Opens::InBitwig, f.to_path_buf()));
                }
            }
            many => {
                let failed = many.iter().filter(|s| matches!(fixes.state(s.newest), State::Failed(_))).count();
                line.status = format!("{} songs in this folder use these copies: each has its own line.", many.len());
                if failed > 0 {
                    line.status.push_str(&format!(" {} couldn't be fixed.", plural(failed, "song")));
                    line.problem = true;
                }
            }
        }
        let shown = fixes.shown.contains(&p.folder);
        if others > 0 {
            let words = match (shown, others) {
                (true, _) => "Hide its other saves".to_owned(),
                (false, 1) => "Show its other save".to_owned(),
                (false, n) => format!("Show its {} other saves", thousands(n)),
            };
            line.more = Some((words, p.folder.clone()));
        }
        parts.push(line);
        // In a folder with more than one song, each song has its own line, always shown (each is fixed on its own),
        // with its older saves under it when shown.
        let many = songs.len() > 1;
        for song in &songs {
            if many {
                parts.push(save_line(song.newest, Speaks::Song));
            }
            if shown {
                let of = many.then_some(song.newest);
                parts.extend(song.older.iter().map(|save| save_line(save, Speaks::Older(of))));
            }
        }
        if shown {
            parts.extend(backups.iter().map(|save| save_line(save, Speaks::Older(None))));
        }
    }
    Card {
        kind: "BITWIG COPIES",
        title: "Bitwig sound copies",
        big: found.map_or_else(|| "Measuring…".to_owned(), |c| size_text(c.bytes())),
        big_note: match (found, looking) {
            (Some(c), _) => format!("{} in {}", copies_words(c.count()), plural(c.projects.len(), "project")),
            (None, Some(at)) => looking_words(at),
            (None, None) => String::new(),
        },
        lead: format!(
            "Byte for byte the same as the multisamples in {}, or as a sample your Sounds library knows. Bitwig \
             copies a multisample or a sample into each project that uses it, under a new name each time, so one \
             sound can sit in many projects.",
            library_words(found.map(|c| c.library.as_slice()).unwrap_or_default())
        ),
        ticks: true,
        parts,
        note: if projects.is_empty() { String::new() } else { next() },
        room: String::new(),
        warning: String::new(),
    }
}

/// The activity card's line while Fix writes: "2 of 5 saves".
pub fn fix_progress_words(done: usize, of: usize) -> String {
    format!("{} of {}", thousands(done), plural(of, "save"))
}

/// The activity card's line once Fix is done, from how many saves were fixed and how many weren't: Err when none was
/// (the card then fails).
pub fn fixed_words(written: usize, failed: usize) -> Result<String, String> {
    let why = |f: usize| {
        if f == 1 { "its line on the Space page says why" } else { "each one's line on the Space page says why" }
    };
    match (written, failed) {
        (0, 0) => Err("Nothing was fixed.".to_owned()),
        (0, 1) => Err(format!("It couldn't be fixed: {}.", why(1))),
        (0, f) => Err(format!("None of the {} could be fixed: {}.", plural(f, "save"), why(f))),
        (1, 0) => Ok("Wrote 1 fixed project. Open it in Bitwig and check it plays.".to_owned()),
        (n, 0) => Ok(format!("Wrote {}. Open each in Bitwig and check it plays.", plural(n, "fixed project"))),
        (n, f) => {
            Ok(format!("Wrote {}; {} couldn't be fixed ({}).", plural(n, "fixed project"), plural(f, "save"), why(f)))
        }
    }
}

/// How many saves have a "(fixed)" project beside them, from the last look.
pub fn fixed_saves(found: Option<&Copies>) -> usize {
    found.map_or(0, |f| f.projects.iter().flat_map(|p| &p.saves).filter(|s| s.fixed.is_some()).count())
}

/// The card's button that sends every fixed project to the Recycle Bin: its words and whether it can be pressed now
/// (not while Fix writes, a look is on, or something goes to the bin or comes back). None when no save has one.
pub fn remove_fixed_button(found: Option<&Copies>, wait: bool) -> Option<(String, bool)> {
    let n = fixed_saves(found);
    (n > 0).then(|| {
        // Every "(fixed)" project in the folders goes, not only the newest of each save, so no count here.
        (format!("Send every fixed Bitwig project to the {BIN}"), !wait)
    })
}

/// The activity card's line once the fixed projects went, from what went and why each one the app's note held back
/// stayed (`stayed`, by the path as Space compares paths: [`crate::space_fix_note::Check::stayed`]): Err when none went
/// and one stayed for a
/// reason of its own (the card then fails). Those the note held back stay on purpose, so they're named: the ones you've
/// saved in since Fix wrote them, and the ones the app has no record of Fix writing (the whole-app review of 14 Nov,
/// FX-5).
pub fn removed_fixed_words(done: &mo_space::clean::Done, stayed: &HashMap<String, Stays>) -> Result<String, String> {
    let went: HashSet<&str> = done.removed.iter().map(|r| r.path.as_str()).collect();
    let mut seen = HashSet::new();
    let (mut changed, mut not_noted, mut freed, mut other) = (Vec::new(), Vec::new(), Vec::new(), 0);
    for p in done.problems.iter().filter(|p| !went.contains(p.path.as_str()) && seen.insert(p.path.as_str())) {
        let name = paths::stem(&p.path).to_owned();
        match stayed.get(&key(&p.path)) {
            Some(Stays::Changed) => changed.push(name),
            Some(Stays::NotNoted) => not_noted.push(name),
            Some(Stays::Freed) => freed.push(name),
            Some(Stays::NoTime) | None => other += 1,
        }
    }
    let freed = match freed.len() {
        0 => String::new(),
        1 => format!("{} stayed: its song's copies have gone, so it's the one that plays now.", named(&freed)),
        _ => format!("{} stayed: their songs' copies have gone, so they're the ones that play now.", named(&freed)),
    };
    let held: Vec<String> =
        [changed_words(&changed), not_noted_words(&not_noted), freed].into_iter().filter(|w| !w.is_empty()).collect();
    let held = held.join(" ");
    match (done.removed.len(), other) {
        (0, 0) if held.is_empty() => Err("No fixed project was found, so nothing went.".to_owned()),
        (0, 0) => Ok(format!("Nothing went. {held}")),
        (0, m) => Err(format!("None could go ({} stayed): the log says why.{}", thousands(m), spaced(&held))),
        (n, m) => {
            let stayed = if m == 0 { String::new() } else { format!("; {} stayed (the log says why)", thousands(m)) };
            Ok(format!(
                "Sent {} ({}) to the {BIN}{stayed}. Fix can write them again; Put back on the {BIN} line brings them \
                 back.{}",
                plural(n, "fixed project"),
                size_text(done.bytes()),
                spaced(&held),
            ))
        }
    }
}

/// The activity card's line once Free the space has run: Err when nothing went and something stayed.
pub fn freed_words(done: &mo_space::clean::Done) -> Result<String, String> {
    let went: HashSet<&str> = done.removed.iter().map(|r| r.path.as_str()).collect();
    let stayed = done.problems.iter().filter(|p| !went.contains(p.path.as_str())).count();
    match (done.removed.len(), stayed) {
        (0, 0) => Ok("Nothing went: the checked songs use no copies now.".to_owned()),
        // One reason for them all (a save made again since the look, a library that couldn't be read): that's the
        // reason the card gives, not the general one (the whole-app review of 19 Nov, FS-15).
        (0, m)
            if m > 1
                && let Some(why) = one_why(done, &went) =>
        {
            Err(format!("None went ({} stayed): {why}", thousands(m)))
        }
        (0, 1) if let Some(why) = one_why(done, &went) => Err(format!("None went: {why}")),
        (0, m) => Err(format!(
            "None went ({} stayed): another song, fixed project or file in the folder still uses them, or they \
             changed. The log says why for each.",
            thousands(m)
        )),
        (n, m) => {
            let stayed = if m == 0 {
                String::new()
            } else {
                format!("; {} stayed, still used or changed (the log says why)", thousands(m))
            };
            Ok(format!(
                "Sent {} ({}) to the {BIN}{stayed}. Use the fixed projects from now on; Put back on the {BIN} line \
                 brings them back.",
                if n == 1 { "1 Bitwig copy".to_owned() } else { format!("{} Bitwig copies", thousands(n)) },
                size_text(done.bytes()),
            ))
        }
    }
}

/// The Bitwig card's words once Free the space was stopped: what went before it stopped; the rest stay.
pub fn stopped_words(done: &mo_space::clean::Done) -> String {
    match done.removed.len() {
        0 => "Stopped before any copy went: they all stay.".to_owned(),
        n => format!(
            "Stopped: {} ({}) went to the {BIN} before it stopped; the rest stay. Put back on the {BIN} line brings \
             them back.",
            if n == 1 { "1 Bitwig copy".to_owned() } else { format!("{} Bitwig copies", thousands(n)) },
            size_text(done.bytes()),
        ),
    }
}

/// The one reason every copy that stayed has, when they all have the same.
fn one_why<'a>(done: &'a mo_space::clean::Done, went: &HashSet<&str>) -> Option<&'a str> {
    let mut whys = done.problems.iter().filter(|p| !went.contains(p.path.as_str())).map(|p| p.why.as_str());
    let first = whys.next()?;
    whys.all(|w| w == first).then_some(first)
}

/// A sentence after another: a space before it, unless it's empty.
fn spaced(sentence: &str) -> String {
    if sentence.is_empty() { String::new() } else { format!(" {sentence}") }
}

/// Projects by name, in quotes: the first three, then how many more.
fn named(names: &[String]) -> String {
    const NAMED: usize = 3;
    let mut shown: Vec<String> = names.iter().take(NAMED).map(|n| format!("\"{n}\"")).collect();
    if names.len() > NAMED {
        shown.push(format!("{} more", thousands(names.len() - NAMED)));
    }
    and_list(&shown)
}

/// The "(fixed)" projects that stayed because they've changed since Fix wrote them, named; nothing when there are none.
fn changed_words(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => format!("{} stayed: it has changed since Fix wrote it, so it's yours now.", named(names)),
        _ => format!("{} stayed: they've changed since Fix wrote them, so they're yours now.", named(names)),
    }
}

/// The "(fixed)" projects that stayed because the app has no record of Fix writing them, so it can't tell if you've
/// saved in them, named, with what you can do; nothing when there are none.
fn not_noted_words(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => format!(
            "{} stayed: the app has no record of Fix writing it, so it can't tell if you've saved in it. If you \
             haven't, you can delete it yourself.",
            named(names)
        ),
        _ => format!(
            "{} stayed: the app has no record of Fix writing them, so it can't tell if you've saved in them. If you \
             haven't, you can delete them yourself.",
            named(names)
        ),
    }
}

/// The activity card's headline and line once a look is done: what it found, and where it's shown.
pub fn found_words(found: &Copies) -> (&'static str, String) {
    if found.projects.is_empty() {
        ("No Bitwig copies of your Library sounds", "None of your Bitwig projects keeps a copy of one.".to_owned())
    } else {
        (
            "Found Bitwig's copies of your Library sounds",
            format!(
                "{} in {}. The Space page lists them under Biggest wins first.",
                size_text(found.bytes()),
                plural(found.projects.len(), "project")
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1 << 20;

    fn project(name: &str, sizes: &[u64]) -> Project {
        let folder = format!(r"K:\Bitwig Studio\Projects\{name}");
        let copies = sizes
            .iter()
            .enumerate()
            .map(|(n, &bytes)| LibraryCopy {
                in_project: format!(r"{folder}\multi-samples\Kick-{}.multisample", n + 2),
                library_file: format!(r"K:\Bitwig Studio\Library\Kick {n}.multisample"),
                bytes,
            })
            .collect();
        let saves = vec![save(&format!(r"{folder}\{name}.bwproject"))];
        Project { folder: PathBuf::from(folder), copies, saves }
    }

    /// A save beside its project, not fixed.
    fn save(path: &str) -> Save {
        Save { path: PathBuf::from(path), modified: None, below: false, fixed: None }
    }

    fn sample() -> Copies {
        Copies {
            library: vec![PathBuf::from(r"K:\Bitwig Studio\Library")],
            folders: 3,
            projects: vec![project("GARAGE FOR BW 1", &[200 * MB, 42 * MB]), project("Night Drive", &[30 * MB])],
        }
    }

    #[test]
    fn the_line_shows_once_copies_are_found_and_while_the_first_look_is_on() {
        let found = sample();
        let (bytes, line) = line(Some(&found), None).unwrap();
        assert_eq!((bytes, line.id), (found.bytes(), BITWIG_COPIES));
        assert_eq!(
            (line.why.as_str(), line.size.as_str()),
            ("Copies of your multisamples and samples in 2 projects", "272 MB")
        );
        // Looking again keeps what was found on show.
        assert_eq!(super::line(Some(&found), Some((1, 3))).unwrap().1.size, "272 MB");
        // While the first look is on it frees nothing yet, so it sorts below any line that does.
        let (bytes, first) = super::line(None, Some((1, 3))).unwrap();
        assert_eq!(bytes, 0);
        assert_eq!(
            (first.why.as_str(), first.size.as_str()),
            ("Looking through your Bitwig projects: 1 of 3", "Measuring…")
        );
        assert_eq!(super::line(None, Some((0, 0))).unwrap().1.why, "Looking for your Bitwig projects…");
        // None found, or not looked yet: no line.
        assert_eq!(super::line(Some(&Copies::default()), None), None);
        assert_eq!(super::line(None, None), None);
    }

    #[test]
    fn the_note_says_why_there_is_no_line() {
        assert_eq!(none_note(None), "");
        assert_eq!(none_note(Some(&sample())), "");
        let none = Copies { library: sample().library, folders: 3, projects: Vec::new() };
        assert_eq!(none_note(Some(&none)), "No Bitwig project has copies of your multisamples or samples. ");
        assert!(none_note(Some(&Copies::default())).starts_with("No Bitwig project"));
        let no_library = Copies { library: Vec::new(), folders: 3, projects: Vec::new() };
        assert!(none_note(Some(&no_library)).starts_with("Bitwig's Library folder wasn't found"));
    }

    #[test]
    fn the_card_lists_each_projects_copies() {
        let card = card(Some(&sample()), None, &Fixes::default());
        assert_eq!((card.big.as_str(), card.big_note.as_str()), ("272 MB", "3 copies in 2 projects"));
        assert!(
            card.lead.starts_with(
                r"Byte for byte the same as the multisamples in K:\Bitwig Studio\Library, or as a sample "
            )
        );
        let garage = &card.parts[0];
        assert_eq!(
            (garage.name.as_str(), garage.what.as_str(), garage.size.as_str()),
            ("GARAGE FOR BW 1", r"2 copies, in K:\Bitwig Studio\Projects", "242 MB")
        );
        assert_eq!(card.parts[1].what, r"1 copy, in K:\Bitwig Studio\Projects");
        assert!(card.ticks);
        assert_eq!(card.note, next());
        assert!(card.note.contains(BIN));
        let first = super::card(None, Some((2, 9)), &Fixes::default());
        assert_eq!(
            (first.big.as_str(), first.big_note.as_str()),
            ("Measuring…", "Looking through your Bitwig projects: 2 of 9")
        );
        assert!(first.parts.is_empty() && first.note.is_empty());
    }

    /// A project with its newest save, an older one fixed before it was saved again, and an auto-backup below it.
    fn with_saves() -> Project {
        let mut p = project("GARAGE FOR BW 1", &[200 * MB, 42 * MB]);
        let folder = r"K:\Bitwig Studio\Projects\GARAGE FOR BW 1";
        let mut older = save(&format!(r"{folder}\GARAGE FOR BW 1 v1.bwproject"));
        older.fixed = Some((PathBuf::from(format!(r"{folder}\GARAGE FOR BW 1 v1 (fixed).bwproject")), false));
        let mut backup = save(&format!(r"{folder}\auto-backups\GARAGE FOR BW 1-3.bwproject"));
        backup.below = true;
        p.saves.extend([older, backup]);
        p
    }

    #[test]
    fn the_newest_save_is_ticked_out_of_the_box_and_any_other_can_be() {
        let p = with_saves();
        let found = Copies { library: sample().library, folders: 1, projects: vec![p.clone()] };
        let mut fixes = Fixes::default();
        let card = card(Some(&found), None, &fixes);
        // The project's own line speaks for its newest save; its other saves are a link away.
        assert_eq!(card.parts.len(), 1);
        let line = &card.parts[0];
        assert_eq!(line.tick, Some((vec![p.saves[0].path.clone()], true)));
        assert_eq!(
            line.status,
            "The tick is for its newest save, \"GARAGE FOR BW 1.bwproject\". Its 2 other saves may use these copies too."
        );
        assert_eq!(line.more, Some(("Show its 2 other saves".to_owned(), p.folder.clone())));
        assert_eq!(fixes.button(Some(&found)), Some(("Fix 1 save".to_owned(), true)));

        // Shown, the other saves are listed under it, not ticked; ticking one adds it. Bitwig's own backup, in a
        // folder below, can't be fixed where it is, so it has no tick.
        fixes.shown.insert(p.folder.clone());
        let card = super::card(Some(&found), None, &fixes);
        let names: Vec<&str> = card.parts.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            ["GARAGE FOR BW 1", "GARAGE FOR BW 1 v1.bwproject", r"auto-backups\GARAGE FOR BW 1-3.bwproject"]
        );
        assert_eq!(card.parts[0].more.as_ref().map(|m| m.0.as_str()), Some("Hide its other saves"));
        assert!(card.parts[1].under && card.parts[1].tick == Some((vec![p.saves[1].path.clone()], false)));
        assert_eq!(
            card.parts[1].status,
            "\"GARAGE FOR BW 1 v1 (fixed).bwproject\" is older than this save or than the newest Fix: fix it again."
        );
        assert!(card.parts[2].under);
        assert_eq!((card.parts[2].tick.clone(), card.parts[2].status.as_str()), (None, BACKUP));
        fixes.ticks.insert(p.saves[1].path.clone(), true);
        fixes.ticks.insert(p.saves[0].path.clone(), false);
        fixes.ticks.insert(p.saves[2].path.clone(), true);
        let wanted: Vec<PathBuf> = fixes.to_fix(&found).into_iter().map(|t| t.save).collect();
        assert_eq!(wanted, [p.saves[1].path.clone()]);
        assert_eq!(fixes.button(Some(&found)), Some(("Fix 1 save".to_owned(), true)));
        fixes.ticks.insert(p.saves[1].path.clone(), false);
        assert_eq!(fixes.button(Some(&found)), Some(("Fix".to_owned(), false)));

        // A project with no save of its own beside it (only backups) has nothing to fix.
        let mut only_backups = p.clone();
        only_backups.saves.retain(|s| s.below);
        let found = Copies { library: Vec::new(), folders: 1, projects: vec![only_backups] };
        let card = super::card(Some(&found), None, &Fixes::default());
        assert_eq!((card.parts[0].tick.clone(), card.parts[0].status.as_str()), (None, NO_SAVE));
        assert_eq!(Fixes::default().button(Some(&found)), None);
    }

    #[test]
    fn each_song_in_a_folder_gets_its_own_tick() {
        // A folder with a song, a later save of it, and another track made from the same sounds.
        let mut p = project("the moonlight garage", &[200 * MB]);
        let folder = r"K:\Bitwig Studio\Projects\the moonlight garage";
        p.saves = vec![
            save(&format!(r"{folder}\the moonlight TECH.bwproject")),
            save(&format!(r"{folder}\the moonlight garage v3.bwproject")),
            save(&format!(r"{folder}\the moonlight garage v2.bwproject")),
        ];
        let found = Copies { library: sample().library, folders: 1, projects: vec![p.clone()] };
        let fixes = Fixes::default();
        assert_eq!(p.songs().len(), 2);
        let card = card(Some(&found), None, &fixes);
        let names: Vec<&str> = card.parts.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            ["the moonlight garage", "the moonlight TECH.bwproject", "the moonlight garage v3.bwproject"]
        );
        assert_eq!(card.parts[0].tick, None);
        assert_eq!(card.parts[0].status, "2 songs in this folder use these copies: each has its own line.");
        assert!(card.parts[1..].iter().all(|l| l.under && l.tick.as_ref().is_some_and(|t| t.1)));
        assert_eq!(card.parts[1].status, "The newest save of this song.");
        // The older save of the garage song is a link away, unticked.
        assert_eq!(card.parts[0].more, Some(("Show its other save".to_owned(), p.folder.clone())));
        let wanted: Vec<PathBuf> = fixes.to_fix(&found).into_iter().map(|t| t.save).collect();
        assert_eq!(wanted, [p.saves[0].path.clone(), p.saves[1].path.clone()]);
        assert_eq!(fixes.button(Some(&found)), Some(("Fix 2 saves".to_owned(), true)));

        // Shown, a song's older save sits under its song and says whose it is. A song Fix failed on shows on the
        // folder's own line too.
        let mut fixes = Fixes::default();
        fixes.shown.insert(p.folder.clone());
        fixes.done(p.saves[0].path.clone(), Fixing::Failed("it broke".to_owned()));
        let card = super::card(Some(&found), None, &fixes);
        let names: Vec<&str> = card.parts.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "the moonlight garage",
                "the moonlight TECH.bwproject",
                "the moonlight garage v3.bwproject",
                "the moonlight garage v2.bwproject"
            ]
        );
        assert_eq!(card.parts[3].status, "An older save of \"the moonlight garage v3.bwproject\".");
        assert!(card.parts[0].problem && card.parts[0].status.ends_with(" 1 song couldn't be fixed."));
    }

    #[test]
    fn a_save_being_fixed_then_fixed_then_saved_again_says_so() {
        let mut p = with_saves();
        let newest = p.saves[0].path.clone();
        let folder = r"K:\Bitwig Studio\Projects\GARAGE FOR BW 1";
        let older_fixed = PathBuf::from(format!(r"{folder}\GARAGE FOR BW 1 (fixed).bwproject"));
        let fixed = PathBuf::from(format!(r"{folder}\GARAGE FOR BW 1 (fixed 2).bwproject"));
        let found = |p: &Project| Copies { library: Vec::new(), folders: 1, projects: vec![p.clone()] };
        let status = |p: &Project, fixes: &Fixes| super::card(Some(&found(p)), None, fixes).parts.remove(0);
        // It was fixed once, before it was saved again.
        p.saves[0].fixed = Some((older_fixed.clone(), false));
        let mut fixes = Fixes::default();
        let jobs = fixes.to_fix(&found(&p));
        assert_eq!(jobs.iter().map(|j| &j.save).collect::<Vec<_>>(), [&newest]);
        assert_eq!(jobs[0].copies, p.copies);
        fixes.start(&jobs);
        let line = status(&p, &fixes);
        assert_eq!(line.status, "Writing a fixed project for \"GARAGE FOR BW 1.bwproject\"…");
        assert_eq!(line.tick, None);
        assert_eq!(fixes.button(Some(&found(&p))), Some(("Fixing…".to_owned(), false)));

        // Written: it opens in Bitwig. Fix is on until it has finished, and a look that arrives meanwhile, which may
        // have started before Fix wrote (it found only the older fixed project), doesn't change that.
        fixes.done(newest.clone(), Fixing::Written(fixed.clone()));
        assert!(fixes.busy());
        fixes.looked();
        let line = status(&p, &fixes);
        assert_eq!(
            line.status,
            "Fixed: \"GARAGE FOR BW 1 (fixed 2).bwproject\". Open it in Bitwig and check it plays, then tick it so its copies can go."
        );
        assert_eq!((line.tick, line.open), (None, Some((Opens::InBitwig, fixed.clone()))));
        assert_eq!(fixes.button(Some(&found(&p))), Some(("Fixing…".to_owned(), false)));
        // Unless it found that very file older than the save: saved again meanwhile.
        p.saves[0].fixed = Some((fixed.clone(), false));
        assert_eq!(status(&p, &fixes).tick, Some((vec![newest.clone()], true)));

        // Finished, the look after it finds the fixed project on disk: that counts from now on.
        fixes.finished();
        p.saves[0].fixed = Some((fixed.clone(), true));
        fixes.looked();
        assert!(!fixes.busy() && fixes.fixing.is_empty());
        assert_eq!(status(&p, &fixes).open, Some((Opens::InBitwig, fixed.clone())));
        // The older save is the only one left to fix, and it isn't ticked.
        assert_eq!(fixes.button(Some(&found(&p))), Some(("Fix".to_owned(), false)));

        // Saved again since: it can be fixed again, and is ticked again.
        p.saves[0].fixed = Some((fixed.clone(), false));
        let line = status(&p, &fixes);
        assert_eq!((line.tick, line.open), (Some((vec![newest.clone()], true)), None));
        assert_eq!(
            line.status,
            "\"GARAGE FOR BW 1 (fixed 2).bwproject\" is older than this save or than the newest Fix: fix it again."
        );

        // One that failed says why, and loses its tick: pressing Fix again wouldn't help until something changes. It
        // can be ticked again.
        p.saves[0].fixed = None;
        fixes.ticks.insert(newest.clone(), true);
        fixes.start(&fixes.to_fix(&found(&p)));
        fixes.done(newest.clone(), Fixing::Failed("every copy is still needed, so nothing was written".into()));
        fixes.finished();
        let line = status(&p, &fixes);
        assert_eq!(line.status, "Not fixed: every copy is still needed, so nothing was written");
        assert!(line.problem && line.tick == Some((vec![newest.clone()], false)));
        fixes.looked();
        assert!(status(&p, &fixes).problem, "a look keeps why it couldn't be fixed");
        fixes.ticks.insert(newest.clone(), true);
        assert_eq!(status(&p, &fixes).tick, Some((vec![newest.clone()], true)));

        // A save Fix didn't get to, because it stopped with an error, says so.
        fixes.start(&fixes.to_fix(&found(&p)));
        fixes.finished();
        assert_eq!(status(&p, &fixes).status, format!("Not fixed: {STOPPED}"));
        // A long why is cut short on the card.
        let long = "x".repeat(400);
        assert!(
            state_words(&p, &p.saves[0], &State::Failed(&long), Speaks::Project(0))
                .0
                .ends_with("… (the log has it all)")
        );
    }

    #[test]
    fn a_fixed_save_whose_fixed_project_still_uses_copies_found_later_is_fixed_again_from_it() {
        // The whole-app review of 19 Nov, FS-3.
        let mut p = with_saves();
        let newest = p.saves[0].path.clone();
        let folder = r"K:\Bitwig Studio\Projects\GARAGE FOR BW 1";
        let fixed = PathBuf::from(format!(r"{folder}\GARAGE FOR BW 1 (fixed).bwproject"));
        p.saves[0].fixed = Some((fixed.clone(), true));
        let found = Copies { library: Vec::new(), folders: 1, projects: vec![p.clone()] };
        let mut fixes = Fixes::default();
        fixes.set_tick(Some(&found), newest.clone(), true);
        assert_eq!(fixes.to_free(&found).len(), 1, "heard, so its copies may go");
        assert!(fixes.to_fix(&found).is_empty());

        // Free the space found its fixed project still uses 2 copies: Fix is offered again, ticked, from that fixed
        // project; it isn't heard any more.
        fixes.still_uses(vec![(newest.clone(), fixed.clone(), 2)], std::slice::from_ref(&newest));
        let line = super::card(Some(&found), None, &fixes).parts.remove(0);
        assert_eq!(
            line.status,
            "\"GARAGE FOR BW 1 (fixed).bwproject\" still uses 2 copies found since it was fixed, which can use their \
             originals now: Fix writes a new fixed project from it."
        );
        assert_eq!(line.tick, Some((vec![newest.clone()], true)));
        assert_eq!(line.open, Some((Opens::InBitwig, fixed.clone())));
        assert!(fixes.to_free(&found).is_empty());
        let jobs = fixes.to_fix(&found);
        assert_eq!(jobs.iter().map(|j| (&j.save, &j.from)).collect::<Vec<_>>(), [(&newest, &Some(fixed.clone()))]);

        // A look that finds another fixed project for it: that one hasn't been asked about.
        let mut later = p.clone();
        later.saves[0].fixed = Some((PathBuf::from(format!(r"{folder}\GARAGE FOR BW 1 (fixed 2).bwproject")), true));
        let other = Copies { library: Vec::new(), folders: 1, projects: vec![later] };
        assert!(fixes.to_fix(&other).is_empty());

        // Fix couldn't write from it: the line says why, the tick waits for you, and Fix still writes from it.
        fixes.start(&jobs);
        fixes.done(newest.clone(), Fixing::Failed("Couldn't read it".into()));
        fixes.finished();
        let line = super::card(Some(&found), None, &fixes).parts.remove(0);
        assert_eq!(
            line.status,
            "\"GARAGE FOR BW 1 (fixed).bwproject\" still uses copies found since it was fixed, but Fix couldn't write \
             from it: Couldn't read it"
        );
        assert!(line.problem);
        assert_eq!(line.tick, Some((vec![newest.clone()], false)));
        fixes.set_tick(Some(&found), newest.clone(), true);
        assert_eq!(fixes.to_fix(&found)[0].from, Some(fixed.clone()));

        // Fix wrote the next one: it's fixed, as any.
        fixes.start(&jobs);
        fixes.done(newest.clone(), Fixing::Written(fixed.with_file_name("GARAGE FOR BW 1 (fixed 2).bwproject")));
        fixes.finished();
        assert!(fixes.still_uses.is_empty());

        // A later Free that finds none for a save it checked takes an earlier finding off.
        fixes.still_uses(vec![(newest.clone(), fixed.clone(), 1)], &[]);
        fixes.still_uses(Vec::new(), std::slice::from_ref(&newest));
        assert!(fixes.still_uses.is_empty());
    }

    #[test]
    fn free_the_space_names_one_reason_for_them_all_and_a_stop() {
        use mo_space::clean::{Done, Problem};
        let stayed = |whys: &[&str]| Done {
            problems: whys
                .iter()
                .enumerate()
                .map(|(i, w)| Problem { path: format!("K:/Song/multi-samples/{i}.multisample"), why: (*w).to_owned() })
                .collect(),
            ..Done::default()
        };
        let one = "The song library couldn't be read just now, so the multisample copies stay. Try again in a moment.";
        assert_eq!(freed_words(&stayed(&[one, one, one])), Err(format!("None went (3 stayed): {one}")));
        assert_eq!(freed_words(&stayed(&[one])), Err(format!("None went: {one}")));
        let mixed = freed_words(&stayed(&[one, "K:/Other.bwproject uses 1.multisample too, so it stays."]));
        assert!(mixed.unwrap_err().contains("The log says why"));
        assert_eq!(stopped_words(&Done::default()), "Stopped before any copy went: they all stay.");
    }

    #[test]
    fn fix_says_how_far_it_is_and_how_it_went() {
        assert_eq!(fix_progress_words(1, 3), "1 of 3 saves");
        assert_eq!(fixed_words(1, 0).unwrap(), "Wrote 1 fixed project. Open it in Bitwig and check it plays.");
        assert_eq!(fixed_words(3, 0).unwrap(), "Wrote 3 fixed projects. Open each in Bitwig and check it plays.");
        assert_eq!(
            fixed_words(2, 1).unwrap(),
            "Wrote 2 fixed projects; 1 save couldn't be fixed (its line on the Space page says why)."
        );
        assert_eq!(
            fixed_words(1, 3).unwrap(),
            "Wrote 1 fixed project; 3 saves couldn't be fixed (each one's line on the Space page says why)."
        );
        assert_eq!(fixed_words(0, 1).unwrap_err(), "It couldn't be fixed: its line on the Space page says why.");
        assert_eq!(
            fixed_words(0, 2).unwrap_err(),
            "None of the 2 saves could be fixed: each one's line on the Space page says why."
        );
        assert_eq!(saved_words(None), "");
        let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let local = chrono::DateTime::<chrono::Local>::from(at).format("%-d %b %Y, %H:%M").to_string();
        assert_eq!(saved_words(Some(at)), format!("Saved {local}"));
    }

    /// The ones the app's note of what Fix wrote held back stay on purpose, so the card names them, apart from the
    /// ones that stayed for a reason of their own (the whole-app review of 14 Nov, FX-5).
    #[test]
    fn the_bin_card_names_the_fixed_projects_the_note_held_back() {
        use mo_space::clean::{Done, Problem, Removed, What};
        let path = |name: &str| format!("K:/Bitwig Studio/Projects/Garage/{name}.bwproject");
        let went = |name: &str| Removed {
            path: path(name),
            real: path(name),
            bytes: 2 * MB,
            what: What::FixedProject,
            when: 0,
            bin_id: None,
            binned: None,
            binned_zone: None,
            stamp: None,
        };
        let stayed = |name: &str, stays: Option<Stays>| {
            let why = stays.map_or("It isn't there any more.", Stays::why);
            (Problem { path: path(name), why: why.to_owned() }, stays.map(|s| (key(&path(name)), s)))
        };
        let words = |removed: Vec<Removed>, problems: Vec<(Problem, Option<(String, Stays)>)>| {
            let held: HashMap<String, Stays> = problems.iter().filter_map(|(_, h)| h.clone()).collect();
            let problems = problems.into_iter().map(|(p, _)| p).collect();
            removed_fixed_words(&Done { removed, problems, went_with: Vec::new() }, &held)
        };
        let back = format!("Fix can write them again; Put back on the {BIN} line brings them back.");
        assert_eq!(
            words(vec![went("Garage 2 (fixed)")], Vec::new()).unwrap(),
            format!("Sent 1 fixed project (2 MB) to the {BIN}. {back}")
        );
        // One saved in since: named, and it stays on purpose, so the card doesn't fail.
        let saved_in = "\"Garage 3 (fixed)\" stayed: it has changed since Fix wrote it, so it's yours now.";
        assert_eq!(
            words(vec![went("Garage 2 (fixed)")], vec![stayed("Garage 3 (fixed)", Some(Stays::Changed))]).unwrap(),
            format!("Sent 1 fixed project (2 MB) to the {BIN}. {back} {saved_in}")
        );
        assert_eq!(
            words(Vec::new(), vec![stayed("Garage 3 (fixed)", Some(Stays::Changed))]).unwrap(),
            format!("Nothing went. {saved_in}")
        );
        // One the app has no record of: named, with what you can do.
        assert_eq!(
            words(Vec::new(), vec![stayed("Old (fixed)", Some(Stays::NotNoted))]).unwrap(),
            "Nothing went. \"Old (fixed)\" stayed: the app has no record of Fix writing it, so it can't tell if you've \
             saved in it. If you haven't, you can delete it yourself."
        );
        // Named apart from the ones that stayed for a reason of their own (a time that can't be read is one), the first
        // three, once each.
        let held: Vec<_> = (1..=5)
            .map(|n| stayed(&format!("Song {n} (fixed)"), Some(Stays::Changed)))
            .chain([
                stayed("Song 1 (fixed)", Some(Stays::Changed)),
                stayed("Old 1 (fixed)", Some(Stays::NotNoted)),
                stayed("Old 2 (fixed)", Some(Stays::NotNoted)),
                stayed("Gone (fixed)", None),
                stayed("Timeless (fixed)", Some(Stays::NoTime)),
            ])
            .collect();
        let named = "\"Song 1 (fixed)\", \"Song 2 (fixed)\", \"Song 3 (fixed)\" and 2 more stayed: they've changed since \
                     Fix wrote them, so they're yours now. \"Old 1 (fixed)\" and \"Old 2 (fixed)\" stayed: the app has \
                     no record of Fix writing them, so it can't tell if you've saved in them. If you haven't, you can \
                     delete them yourself.";
        assert_eq!(
            words(vec![went("Garage 2 (fixed)")], held.clone()).unwrap(),
            format!("Sent 1 fixed project (2 MB) to the {BIN}; 2 stayed (the log says why). {back} {named}")
        );
        // None went and one stayed for a reason of its own: the card fails, and still names the ones held back.
        assert_eq!(
            words(Vec::new(), held).unwrap_err(),
            format!("None could go (2 stayed): the log says why. {named}")
        );
        assert_eq!(
            words(Vec::new(), vec![stayed("Gone (fixed)", None)]).unwrap_err(),
            "None could go (1 stayed): the log says why."
        );
        assert_eq!(words(Vec::new(), Vec::new()).unwrap_err(), "No fixed project was found, so nothing went.");
    }

    #[test]
    fn a_project_is_named_after_its_folder_with_either_separator() {
        let at = |folder: &str| {
            let p = Project { folder: PathBuf::from(folder), copies: Vec::new(), saves: Vec::new() };
            (p.name(), p.place())
        };
        assert_eq!(
            at(r"K:\Bitwig Studio\Projects\Night Drive\"),
            ("Night Drive".to_owned(), r"K:\Bitwig Studio\Projects".to_owned())
        );
        assert_eq!(at("/Volumes/K/Projects/Bounce 2"), ("Bounce 2".to_owned(), "/Volumes/K/Projects".to_owned()));
        assert_eq!(at("Loose"), ("Loose".to_owned(), String::new()));
    }

    #[test]
    fn more_than_one_library_is_named_in_a_sentence() {
        let three = ["A", "B", "C"].map(PathBuf::from);
        assert_eq!(library_words(&three), "A, B and C");
        assert_eq!(library_words(&three[..2]), "A and B");
        assert_eq!(library_words(&[]), "your Bitwig Library");
    }

    #[test]
    fn the_activity_card_says_how_far_it_is_and_what_was_found() {
        assert_eq!(progress_words(1204, 5000), "1,204 of 5,000 projects");
        assert_eq!(progress_words(0, 1), "0 of 1 project");
        assert_eq!(
            found_words(&sample()),
            (
                "Found Bitwig's copies of your Library sounds",
                "272 MB in 2 projects. The Space page lists them under Biggest wins first.".to_owned()
            )
        );
        assert_eq!(found_words(&Copies::default()).0, "No Bitwig copies of your Library sounds");
    }
}
