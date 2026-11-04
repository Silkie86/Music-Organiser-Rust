//! The Space page's Bitwig copies (phase 4, part 2). Bitwig keeps a copy of each Library multisample a project uses in
//! the project's own `multi-samples` folder, under a new name whenever the name is taken ("Kicks 1 Select-3-2-2
//! .multisample"), so one sound can sit in many projects. This finds the copies that are the same as a Library file
//! byte for byte (`mo_space::bitwig`) and says how much room they take, project by project. Nothing here changes a
//! file. What comes next, a part at a time: a "(fixed)" project beside each one that uses the Library's own files
//! instead, then the copies it no longer needs go to the Recycle Bin once you've checked it plays (Solomon's choice,
//! 31 Oct 2026). The original project is never changed (rule 3).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_formats::paths;
use mo_space::bitwig::{self, LibraryCopy, LibraryIndex};
use mo_space::size::size_text;
use mo_space::{Stopped, key};

use crate::songs::{plural, thousands};
use crate::space::{Card, Line};

/// The Bitwig copies line's id.
pub const BITWIG_COPIES: &str = "bitwig-copies";

/// The activity card's title while a look is on.
pub const LOOKING: &str = "Looking for Bitwig copies";

/// What the card says comes next, under the projects.
pub const NEXT: &str = "Nothing is changed yet. Next, the app writes a \"(fixed)\" project beside each one that uses \
    the Library's own files instead. Once you've opened it and heard it play, its copies can go to the Recycle Bin. \
    The original project is never changed.";

/// A Bitwig project folder with copies of Library multisamples in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub folder: PathBuf,
    /// The copies in its `multi-samples` folder that are the same as a Library file, in the order of their paths.
    pub copies: Vec<LibraryCopy>,
}

impl Project {
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
/// (rule 12); the first look makes it. `progress` hears how many project folders have been looked through, of how
/// many. Reads the disks, so call it off the window's thread, inside the low-priority pool (rule 21).
pub fn look(
    roots: &[PathBuf],
    documents: Option<&Path>,
    index: &mut Option<LibraryIndex>,
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
        None => index.insert(LibraryIndex::new(&found.library, stop)?),
    };
    for (n, folder) in folders.iter().enumerate() {
        let copies = index.copies_in(folder, stop)?;
        if !copies.is_empty() {
            found.projects.push(Project { folder: folder.clone(), copies });
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

/// The Bitwig copies line under Biggest wins first, once a look has found copies; or, while the first look is on
/// (`looking`), how far it is. None when there are none.
pub fn line(found: Option<&Copies>, looking: Option<(usize, usize)>) -> Option<Line> {
    let line = |why: String, size: String| Line { id: BITWIG_COPIES, title: "Bitwig multisample copies", why, size };
    match (found, looking) {
        (Some(c), _) if !c.projects.is_empty() => Some(line(
            format!("Copies of your Library's multisamples in {}", plural(c.projects.len(), "project")),
            size_text(c.bytes()),
        )),
        (None, Some(at)) => Some(line(looking_words(at), "Measuring…".to_owned())),
        _ => None,
    }
}

/// What can still be freed, as the page says it under Biggest wins first.
pub const STILL_TO_COME: &str = "Still to come: old copies left after moving folders and spare plugin copies, then \
    songs, then plugins you haven't used.";

/// What the page says under Biggest wins first: why there's no Bitwig line when a look found none, and what's still
/// to come.
pub fn wins_note(found: Option<&Copies>) -> String {
    let none = match found {
        Some(c) if c.folders > 0 && c.library.is_empty() => {
            "Bitwig's Library folder wasn't found (the Library folder in Bitwig Studio), so its copies in your projects \
             can't be compared with it. "
        }
        Some(c) if c.projects.is_empty() => "No Bitwig project has copies of your Library's multisamples. ",
        _ => "",
    };
    format!("{none}{STILL_TO_COME}")
}

/// The Library folders, as a sentence says them: "K:\Bitwig Studio\Library", "A and B", "A, B and C".
fn library_words(library: &[PathBuf]) -> String {
    let names: Vec<String> = library.iter().map(|l| l.to_string_lossy().into_owned()).collect();
    match names.as_slice() {
        [] => "your Bitwig Library".to_owned(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The Bitwig copies card: the room they take, the Library they're the same as, each project's copies biggest first,
/// and what comes next. While the first look is on (`looking`), how far it is.
pub fn card(found: Option<&Copies>, looking: Option<(usize, usize)>) -> Card {
    let projects = found.map(|c| c.projects.as_slice()).unwrap_or_default();
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
        parts: projects
            .iter()
            .map(|p| {
                let place =
                    Some(p.place()).filter(|at| !at.is_empty()).map(|at| format!(", in {at}")).unwrap_or_default();
                (p.name(), format!("{}{place}", copies_words(p.copies.len())), size_text(p.bytes()))
            })
            .collect(),
        note: NEXT.to_owned(),
        room: String::new(),
        warning: String::new(),
    }
}

/// The activity card's line once a look is done: what it found.
pub fn found_words(found: &Copies) -> String {
    if found.projects.is_empty() {
        "None found".to_owned()
    } else {
        format!("{} in {}", size_text(found.bytes()), plural(found.projects.len(), "project"))
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
        Project { folder: PathBuf::from(folder), copies }
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
        let line = line(Some(&found), None).unwrap();
        assert_eq!(line.id, BITWIG_COPIES);
        assert_eq!(
            (line.why.as_str(), line.size.as_str()),
            ("Copies of your Library's multisamples in 2 projects", "272 MB")
        );
        // Looking again keeps what was found on show.
        assert_eq!(super::line(Some(&found), Some((1, 3))).unwrap().size, "272 MB");
        let first = super::line(None, Some((1, 3))).unwrap();
        assert_eq!(
            (first.why.as_str(), first.size.as_str()),
            ("Looking through your Bitwig projects: 1 of 3", "Measuring…")
        );
        assert_eq!(super::line(None, Some((0, 0))).unwrap().why, "Looking for your Bitwig projects…");
        // None found, or not looked yet: no line.
        assert_eq!(super::line(Some(&Copies::default()), None), None);
        assert_eq!(super::line(None, None), None);
    }

    #[test]
    fn the_note_says_why_there_is_no_line() {
        assert_eq!(wins_note(None), STILL_TO_COME);
        assert_eq!(wins_note(Some(&sample())), STILL_TO_COME);
        let none = Copies { library: sample().library, folders: 3, projects: Vec::new() };
        assert!(wins_note(Some(&none)).starts_with("No Bitwig project has copies of your Library's multisamples. "));
        assert!(wins_note(Some(&Copies::default())).starts_with("No Bitwig project"));
        let no_library = Copies { library: Vec::new(), folders: 3, projects: Vec::new() };
        assert!(wins_note(Some(&no_library)).starts_with("Bitwig's Library folder wasn't found"));
    }

    #[test]
    fn the_card_lists_each_projects_copies() {
        let card = card(Some(&sample()), None);
        assert_eq!((card.big.as_str(), card.big_note.as_str()), ("272 MB", "3 copies in 2 projects"));
        assert!(card.lead.starts_with(r"Byte for byte the same as the multisamples in K:\Bitwig Studio\Library. "));
        assert_eq!(
            card.parts[0],
            ("GARAGE FOR BW 1".to_owned(), r"2 copies, in K:\Bitwig Studio\Projects".to_owned(), "242 MB".to_owned())
        );
        assert_eq!(card.parts[1].1, r"1 copy, in K:\Bitwig Studio\Projects");
        assert_eq!(card.note, NEXT);
        let first = super::card(None, Some((2, 9)));
        assert_eq!(
            (first.big.as_str(), first.big_note.as_str()),
            ("Measuring…", "Looking through your Bitwig projects: 2 of 9")
        );
        assert!(first.parts.is_empty());
    }

    #[test]
    fn a_project_is_named_after_its_folder_with_either_separator() {
        let at = |folder: &str| {
            let p = Project { folder: PathBuf::from(folder), copies: Vec::new() };
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
        assert_eq!(found_words(&sample()), "272 MB in 2 projects");
        assert_eq!(found_words(&Copies::default()), "None found");
    }
}
