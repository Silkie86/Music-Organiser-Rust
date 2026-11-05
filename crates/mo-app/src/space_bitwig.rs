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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

use mo_convert::relink::{self, Relink};
use mo_formats::paths;
use mo_space::bitwig::{self, LibraryCopy, LibraryIndex, Save};
use mo_space::size::size_text;
use mo_space::{Stopped, key};

use crate::songs::{plural, thousands};
use crate::space::{Card, Line, Opens, Part, and_list};

/// The Bitwig copies line's id.
pub const BITWIG_COPIES: &str = "bitwig-copies";

/// The activity card's headline while a look is on, in words that say what it's for (Solomon, 4 Nov 2026: "what
/// does looking for bitwig copies mean?"), and its line until it can say how far it is.
pub const LOOKING: &str = "Finding Bitwig's copies of your Library sounds";
pub const LOOKING_WHY: &str = "Bitwig copies a Library multisample into each project that uses it. The Space page \
    shows the room those copies take.";

/// The activity card's headline and line when a look stops because a newer one took its place.
pub const LOOK_STOPPED: (&str, &str) = ("Stopped finding Bitwig's copies", "A newer look took its place.");

/// What the card says about Fix, under the projects.
pub const NEXT: &str = "Fix writes a \"(fixed)\" project beside each ticked save, using the Library's own files \
    instead of the copies. The saves and their copies stay as they are. Once you've opened a fixed project in Bitwig \
    and heard it play, its copies can go to the Recycle Bin: that part comes next.";

/// The activity card's title while Fix writes.
pub const FIXING: &str = "Fixing Bitwig saves";

/// A Bitwig project folder with copies of Library multisamples in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub folder: PathBuf,
    /// The copies in its `multi-samples` folder that are the same as a Library file, in the order of their paths.
    pub copies: Vec<LibraryCopy>,
    /// Its saves: yours beside the project newest first, then Bitwig's auto-backups below it
    /// (`mo_space::bitwig::saves_in`).
    pub saves: Vec<Save>,
}

impl Project {
    /// Its newest save beside the project: the one Fix is ticked for out of the box.
    pub fn newest(&self) -> Option<&Save> {
        self.saves.iter().find(|s| !s.below)
    }

    /// Its other saves, as its card lists them under it.
    pub fn others(&self) -> impl Iterator<Item = &Save> {
        let newest = self.newest().map(|s| s.path.as_path());
        self.saves.iter().filter(move |s| Some(s.path.as_path()) != newest)
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

/// Looks through the Bitwig projects in `roots` (your project folders) for copies of Library multisamples: the project
/// folders with a `multi-samples` folder first, then Bitwig's Library ([`library_folders`]), then each project's
/// copies. `index` is the last look's Library, kept so a file already read isn't read again while it's unchanged
/// (rule 12); the first look makes it, taking in the fingerprints kept in `prints` (the store in the app's data
/// folder, `mo_space::prints::FILE_NAME`) so a restart doesn't read them again either. What was read is written there
/// at the end, also when stopped. `progress` hears how many project folders have been looked through, of how many.
/// Reads the disks, so call it off the window's thread, inside the low-priority pool (rule 21).
pub fn look(
    roots: &[PathBuf],
    documents: Option<&Path>,
    index: &mut Option<LibraryIndex>,
    prints: Option<&Path>,
    stop: &AtomicBool,
    progress: impl FnMut(usize, usize),
) -> Result<Copies, Stopped> {
    let started = std::time::Instant::now();
    let found = look_through(roots, documents, index, prints, stop, progress);
    if let Some(index) = index.as_ref() {
        // A look that stopped forgets nothing; a whole one forgets the files it didn't ask about.
        if let Err(e) = index.keep(found.is_ok()) {
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

fn look_through(
    roots: &[PathBuf],
    documents: Option<&Path>,
    index: &mut Option<LibraryIndex>,
    prints: Option<&Path>,
    stop: &AtomicBool,
    mut progress: impl FnMut(usize, usize),
) -> Result<Copies, Stopped> {
    let folders = bitwig::project_folders(roots, stop)?;
    progress(0, folders.len());
    let library = if folders.is_empty() { Vec::new() } else { library_folders(&folders, documents) };
    let mut found = Copies { library, folders: folders.len(), projects: Vec::new() };
    if found.library.is_empty() {
        return Ok(found);
    }
    let index = match index {
        Some(known) => {
            known.list_again(&found.library, stop)?;
            known
        }
        None => {
            let mut made = LibraryIndex::new(&found.library, stop)?;
            if let Some(store) = prints {
                match made.remember_in(store) {
                    Ok(n) => crate::log::info("Space", &format!("Bitwig copies: {n} fingerprints kept from before")),
                    Err(e) => crate::log::warning(
                        "Space",
                        &format!("The Bitwig copies' fingerprints are kept while the app runs only: {e}"),
                    ),
                }
            }
            index.insert(made)
        }
    };
    for (n, folder) in folders.iter().enumerate() {
        let copies = index.copies_in(folder, stop)?;
        if !copies.is_empty() {
            let saves = bitwig::saves_in(folder, stop)?;
            found.projects.push(Project { folder: folder.clone(), copies, saves });
        }
        progress(n + 1, folders.len());
    }
    found.projects.sort_by(|a, b| b.bytes().cmp(&a.bytes()).then_with(|| a.folder.cmp(&b.folder)));
    Ok(found)
}

/// Bitwig's Library folders for these project folders: the Library folder in each "Bitwig Studio" folder above one
/// (Solomon keeps his Bitwig Studio folder on K:), and the one in `documents`' Bitwig Studio folder, where Bitwig
/// keeps it unless told otherwise (`mo_space::bitwig::documents`). Only the ones that are there, each once.
pub fn library_folders(projects: &[PathBuf], documents: Option<&Path>) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut add = |studio: &Path| {
        let library = studio.join("Library");
        let known = |f: &PathBuf| key(&f.to_string_lossy()) == key(&library.to_string_lossy());
        if library.is_dir() && !found.iter().any(known) {
            found.push(library);
        }
    };
    let is_studio = |folder: &&Path| folder.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Bitwig Studio"));
    for project in projects {
        project.ancestors().skip(1).filter(is_studio).for_each(&mut add);
    }
    if let Some(documents) = documents {
        add(&documents.join("Bitwig Studio"));
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
    let line = |why: String, size: String| Line { id: BITWIG_COPIES, title: "Bitwig multisample copies", why, size };
    match (found, looking) {
        (Some(c), _) if !c.projects.is_empty() => Some((
            c.bytes(),
            line(
                format!("Copies of your Library's multisamples in {}", plural(c.projects.len(), "project")),
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
        Some(c) if c.projects.is_empty() => "No Bitwig project has copies of your Library's multisamples. ",
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
}

/// Writes a "(fixed)" project beside a save that uses the Library's own files instead of its project's copies
/// (`mo_convert::relink`): never over a file that's there, and only once it reads back as a Bitwig project. The save and
/// its copies aren't changed (rule 3). Every copy of the project goes in the one call: a copy the save doesn't use is
/// left alone. Reads and writes the disk, so call it off the window's thread, inside the low-priority pool (rule 21).
pub fn fix(job: &ToFix) -> Fixing {
    let relinks: Vec<Relink> = job
        .copies
        .iter()
        .map(|c| Relink { copy: PathBuf::from(&c.in_project), library: PathBuf::from(&c.library_file) })
        .collect();
    match relink::fix_bitwig_project(&job.save, &relinks) {
        Ok(fixed) => Fixing::Written(fixed.file),
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
    /// From the moment Fix starts until it has finished, as planned or not.
    running: bool,
}

/// Where a save is, as its line says.
enum State<'a> {
    Writing,
    /// Its fixed project, as new as it or newer.
    Fixed(&'a Path),
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
        match (fixing, &save.fixed) {
            (Some(Fixing::Writing), _) => State::Writing,
            // Written this time: fixed, unless a look has found that very file older than the save (saved again
            // since). A look that found no fixed project, or an older one, started before Fix wrote.
            (Some(Fixing::Written(file)), Some((on_disk, false))) if on_disk == file => State::Outdated(file),
            (Some(Fixing::Written(file)), _) => State::Fixed(file),
            (_, Some((file, true))) => State::Fixed(file),
            (Some(Fixing::Failed(why)), _) => State::Failed(why),
            (_, Some((file, false))) => State::Outdated(file),
            _ => State::Open,
        }
    }

    /// A save's tick: None when it can't be fixed now (it's fixed or being written, or it's one of Bitwig's backups
    /// in a folder below, which the converter only fixes beside the project); else whether it's ticked. Out of the box
    /// that's its project's newest save, unless Fix has just failed on it: pressing Fix again wouldn't help until
    /// something changes, so it waits for you to tick it again.
    fn tick(&self, project: &Project, save: &Save) -> Option<bool> {
        let state = self.state(save);
        if save.below || !matches!(state, State::Failed(_) | State::Outdated(_) | State::Open) {
            return None;
        }
        let newest = project.newest().is_some_and(|n| n.path == save.path);
        let out_of_the_box = newest && !matches!(state, State::Failed(_));
        Some(self.ticks.get(&save.path).copied().unwrap_or(out_of_the_box))
    }

    /// How a save went, from Fix: kept for its line. One that couldn't be fixed loses any tick you gave it.
    pub fn done(&mut self, save: PathBuf, fixing: Fixing) {
        if matches!(fixing, Fixing::Failed(_)) {
            self.ticks.remove(&save);
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

    /// The ticked saves, project by project, each with its project's copies.
    pub fn to_fix(&self, found: &Copies) -> Vec<ToFix> {
        let ticked = |p: &Project, s: &Save| self.tick(p, s) == Some(true);
        found
            .projects
            .iter()
            .flat_map(|p| {
                p.saves
                    .iter()
                    .filter(move |s| ticked(p, s))
                    .map(|s| ToFix { save: s.path.clone(), copies: p.copies.clone() })
            })
            .collect()
    }

    /// Fix's button: its words and whether it can be pressed now. None when no save can be fixed.
    pub fn button(&self, found: Option<&Copies>) -> Option<(String, bool)> {
        if self.busy() {
            return Some(("Fixing…".to_owned(), false));
        }
        let found = found?;
        let any = found.projects.iter().any(|p| p.saves.iter().any(|s| self.tick(p, s).is_some()));
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

/// What a save's line says about it, and whether that's a problem. `newest` is for a project's own line, which speaks
/// for its newest save.
fn state_words(project: &Project, save: &Save, state: &State<'_>, newest: bool) -> (String, bool) {
    let file = |f: &Path| paths::file_name(&f.to_string_lossy()).to_owned();
    match state {
        State::Writing if newest => (format!("Writing a fixed project for \"{}\"…", save_name(project, save)), false),
        State::Writing => ("Writing its fixed project…".to_owned(), false),
        State::Fixed(f) => (format!("Fixed: \"{}\". Open it in Bitwig and check it plays.", file(f)), false),
        State::Failed(why) => {
            let mut why = why.to_string();
            if why.chars().count() > WHY_SHOWN {
                why = format!("{}… (the log has it all)", why.chars().take(WHY_SHOWN).collect::<String>());
            }
            (format!("Not fixed: {why}"), true)
        }
        State::Outdated(f) => {
            (format!("Saved again since \"{}\" was written: fix it again for this save.", file(f)), false)
        }
        State::Open if newest => {
            let others = project.others().count();
            let also = if others == 0 {
                String::new()
            } else {
                format!(" Its {} may use these copies too.", plural(others, "other save"))
            };
            (format!("The tick is for its newest save, \"{}\".{also}", save_name(project, save)), false)
        }
        State::Open if save.below => (BACKUP.to_owned(), false),
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
/// with its newest save's tick and how it's doing, then (when you show them) its other saves, and what Fix does. While
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
        if let Some(newest) = p.newest() {
            let state = fixes.state(newest);
            (line.status, line.problem) = state_words(p, newest, &state, true);
            line.tick = fixes.tick(p, newest).map(|t| (vec![newest.path.clone()], t));
            if let State::Fixed(f) = state {
                line.open = Some((Opens::InBitwig, f.to_path_buf()));
            }
        } else {
            line.status = NO_SAVE.to_owned();
        }
        let others = p.others().count();
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
        if !shown {
            continue;
        }
        for save in p.others() {
            let state = fixes.state(save);
            let (status, problem) = state_words(p, save, &state, false);
            parts.push(Part {
                name: save_name(p, save),
                what: saved_words(save.modified),
                under: true,
                tick: fixes.tick(p, save).map(|t| (vec![save.path.clone()], t)),
                status,
                problem,
                open: if let State::Fixed(f) = state { Some((Opens::InBitwig, f.to_path_buf())) } else { None },
                ..Part::default()
            });
        }
    }
    Card {
        kind: "BITWIG COPIES",
        title: "Bitwig multisample copies",
        big: found.map_or_else(|| "Measuring…".to_owned(), |c| size_text(c.bytes())),
        big_note: match (found, looking) {
            (Some(c), _) => format!("{} in {}", copies_words(c.count()), plural(c.projects.len(), "project")),
            (None, Some(at)) => looking_words(at),
            (None, None) => String::new(),
        },
        lead: format!(
            "Byte for byte the same as the multisamples in {}. Bitwig copies a multisample into each project that \
             uses it, under a new name each time, so one sound can sit in many projects.",
            library_words(found.map(|c| c.library.as_slice()).unwrap_or_default())
        ),
        ticks: true,
        parts,
        note: if projects.is_empty() { String::new() } else { NEXT.to_owned() },
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
            ("Copies of your Library's multisamples in 2 projects", "272 MB")
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
        assert_eq!(none_note(Some(&none)), "No Bitwig project has copies of your Library's multisamples. ");
        assert!(none_note(Some(&Copies::default())).starts_with("No Bitwig project"));
        let no_library = Copies { library: Vec::new(), folders: 3, projects: Vec::new() };
        assert!(none_note(Some(&no_library)).starts_with("Bitwig's Library folder wasn't found"));
    }

    #[test]
    fn the_card_lists_each_projects_copies() {
        let card = card(Some(&sample()), None, &Fixes::default());
        assert_eq!((card.big.as_str(), card.big_note.as_str()), ("272 MB", "3 copies in 2 projects"));
        assert!(card.lead.starts_with(r"Byte for byte the same as the multisamples in K:\Bitwig Studio\Library. "));
        let garage = &card.parts[0];
        assert_eq!(
            (garage.name.as_str(), garage.what.as_str(), garage.size.as_str()),
            ("GARAGE FOR BW 1", r"2 copies, in K:\Bitwig Studio\Projects", "242 MB")
        );
        assert_eq!(card.parts[1].what, r"1 copy, in K:\Bitwig Studio\Projects");
        assert!(card.ticks);
        assert_eq!(card.note, NEXT);
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
        let mut older = save(&format!(r"{folder}\GARAGE v1.bwproject"));
        older.fixed = Some((PathBuf::from(format!(r"{folder}\GARAGE v1 (fixed).bwproject")), false));
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
        assert_eq!(names, ["GARAGE FOR BW 1", "GARAGE v1.bwproject", r"auto-backups\GARAGE FOR BW 1-3.bwproject"]);
        assert_eq!(card.parts[0].more.as_ref().map(|m| m.0.as_str()), Some("Hide its other saves"));
        assert!(card.parts[1].under && card.parts[1].tick == Some((vec![p.saves[1].path.clone()], false)));
        assert_eq!(
            card.parts[1].status,
            "Saved again since \"GARAGE v1 (fixed).bwproject\" was written: fix it again for this save."
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
            "Fixed: \"GARAGE FOR BW 1 (fixed 2).bwproject\". Open it in Bitwig and check it plays."
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
            "Saved again since \"GARAGE FOR BW 1 (fixed 2).bwproject\" was written: fix it again for this save."
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
        assert!(state_words(&p, &p.saves[0], &State::Failed(&long), true).0.ends_with("… (the log has it all)"));
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
