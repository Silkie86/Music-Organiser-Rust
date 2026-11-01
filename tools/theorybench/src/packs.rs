//! `theorybench packs`: Solomon's MIDI packs as labelled music for the bench, made on his computer and kept there.
//!
//! Every MIDI file under the folders given is read as the MIDI page reads it (`mo_midi::extract`: a part for each
//! track or channel, long parts cut into phrases, each part's role guessed from its notes and names), and each part
//! that isn't drums becomes one piece, labelled with the key the file's name gives ([`read_name`]). Only a name that
//! gives exactly one key and no other note or chord name labels a file: "Loop 3 - C minor", "Keys_90_Ebmaj", "F#m".
//! A name with chord names in it ("Fm Bb Eb", "Dm7 G7 Cmaj7") names chords, not the key, and a lone letter ("A
//! Section") could be either, so those files are left out. A file whose name says nothing about notes takes the key of
//! the nearest folder above it (up to three up, inside the one given) whose name gives exactly one.
//!
//! Each piece is heard as the MIDI page hears it: whole, as a loop, or as a window when the page cut it from a part
//! longer than 16 bars ([`Whole`]).
//!
//! Each piece keeps its file's path below the folder given as its id, and its pack (the first folder below it) as its
//! group, so a pack's files share a fold. Its licence is [`item::OWN`]: `keyfit` only measures these pieces, and
//! `keyfit --rescale` fits only the numbers Solomon allowed (rule 5). The file written holds his notes, so it must
//! stay on his computer: it can't be written inside a copy of this repository, and `*.jsonl` is in `.gitignore`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use mo_harmony::names::parse_note;
use mo_harmony::{Chord, Key};
use mo_midi::Role;
use mo_midi::extract::{self, Found};
use rayon::prelude::*;

use crate::item::{self, Item, Part, Whole};

/// The source the pieces are given.
pub const SOURCE: &str = "packs";
/// How many folders up a file may take its key from.
const FOLDERS_UP: usize = 3;

/// What a name says about the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Named {
    /// No note or chord name at all.
    Nothing,
    /// Exactly one key, and no other note or chord name.
    Key(Key),
    /// Note or chord names that don't settle one key: two keys, a key among chords, a lone note.
    Unclear,
}

/// What one word of a name is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Word {
    /// A key: "Cm", "Ebmaj", "F#minor"; or, with the next word, "F# Minor"; or a lone note after "key", "of" or "in".
    Key(Key),
    /// A note or chord name that isn't a key: a lone "Eb", "Dm7", "Bb13", "Cmaj9", "C/E", "Gsus4".
    Music,
    /// Anything else: "Loop", "120bpm", "Bass", "Gospel".
    Other,
}

/// The words a key's quality may be written as, after its tonic. A lone small "m" is minor; a capital "M" alone could
/// be either (chord names use it for major), so it isn't one.
fn quality(text: &str) -> Option<bool> {
    if text == "m" {
        return Some(true);
    }
    match text.to_ascii_lowercase().as_str() {
        "min" | "minor" => Some(true),
        "maj" | "major" => Some(false),
        _ => None,
    }
}

/// One word of a name, given the words either side ("F#" then "Minor"; "key" then "Eb"). Gives what it is and how
/// many words it took.
fn word_at(words: &[&str], i: usize) -> (Word, usize) {
    let Some(&word) = words.get(i) else { return (Word::Other, 1) };
    // A note's letter must be a capital: "a" and "be" are words, "A" and "Bb" may be notes.
    if !word.starts_with(|c: char| ('A'..='G').contains(&c)) {
        return (Word::Other, 1);
    }
    let Some((tonic, used)) = parse_note(word) else { return (Word::Other, 1) };
    let rest = word.get(used..).unwrap_or_default();
    let key = |minor: bool| if minor { Key::minor(tonic) } else { Key::major(tonic) };
    if rest.is_empty() {
        if let Some(minor) = words.get(i + 1).and_then(|next| quality(next)) {
            return (Word::Key(key(minor)), 2);
        }
        let after_key = i
            .checked_sub(1)
            .and_then(|j| words.get(j))
            .is_some_and(|before| matches!(before.to_ascii_lowercase().as_str(), "key" | "of" | "in"));
        return (if after_key { Word::Key(key(false)) } else { Word::Music }, 1);
    }
    if let Some(minor) = quality(rest) {
        return (Word::Key(key(minor)), 1);
    }
    (if Chord::parse(word).is_some() { Word::Music } else { Word::Other }, 1)
}

/// What a file's or folder's name says about the key ([`Named`]).
pub fn read_name(name: &str) -> Named {
    let words: Vec<&str> = name
        .split([' ', '_', '-', '(', ')', '[', ']', '{', '}', '.', ',', '&', '+', '~', '='])
        .filter(|w| !w.is_empty())
        .collect();
    let (mut keys, mut music) = (Vec::new(), 0);
    let mut i = 0;
    while i < words.len() {
        let (word, took) = word_at(&words, i);
        match word {
            Word::Key(key) => keys.push(key),
            Word::Music => music += 1,
            Word::Other => {}
        }
        i += took.max(1);
    }
    match keys.as_slice() {
        [] if music == 0 => Named::Nothing,
        [key] if music == 0 => Named::Key(*key),
        _ => Named::Unclear,
    }
}

/// Where a file's key comes from: its own name, or a folder's (the nearest that names notes).
fn label(stem: &str, folders: &[String]) -> (Named, &'static str) {
    match read_name(stem) {
        Named::Nothing => {}
        named => return (named, "file name"),
    }
    for folder in folders.iter().rev().take(FOLDERS_UP) {
        match read_name(folder) {
            Named::Nothing => {}
            named => return (named, "folder name"),
        }
    }
    (Named::Nothing, "")
}

/// The role a piece is given, as the bench's datasets name them, so the key finder hears it as the MIDI page has it
/// heard (`mo_midi::keys`): an arp as chords, a part whose role isn't known as one not known ("", heard as
/// `mo_harmony::Role::Unknown`); drums aren't kept.
fn role_name(role: Role) -> Option<&'static str> {
    match role {
        Role::Chords | Role::Arp => Some("chords"),
        Role::Melody => Some("melody"),
        Role::Bass => Some("bass"),
        Role::Other => Some(""),
        Role::Drums => None,
    }
}

/// A key as the bench's files label it: "C:min", "Eb:maj".
fn label_of(key: Key) -> String {
    format!("{}:{}", key.tonic_name(), if key.is_minor() { "min" } else { "maj" })
}

/// How one file came out.
enum Outcome {
    /// Its name and folders name no key.
    NoKey,
    /// Its name (or folder) names notes that don't settle one key.
    Unclear,
    /// It couldn't be read.
    Unread,
    /// Its pieces, and how many of its parts were drums.
    Read { pieces: Vec<Item>, drums: usize, from_folder: bool },
}

/// A file's place below the folder it was found in: the folders on the way down, and its name without the ending.
fn place(path: &Path, root: &Path) -> (Vec<String>, String) {
    let below = path.strip_prefix(root).unwrap_or(path);
    let mut parts: Vec<String> = below
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let file = parts.pop().unwrap_or_default();
    let stem = Path::new(&file).file_stem().map_or(file.clone(), |s| s.to_string_lossy().into_owned());
    (parts, stem)
}

/// The pieces of one file's parts (as `mo_midi::extract` found them), labelled `key`, and how many were drums.
fn pieces_of(found: &[Found], id: &str, group: &str, key: Key, from_folder: bool) -> (Vec<Item>, usize) {
    let bar = 4 * mo_midi::PPQ;
    let several = found.len() > 1;
    let mut drums = 0;
    let mut pieces = Vec::new();
    for (i, f) in found.iter().enumerate() {
        let Some(role) = role_name(f.analysis.role).filter(|_| !f.drums) else {
            drums += 1;
            continue;
        };
        let length = f.analysis.bars.max(1) * bar;
        // Muted notes aren't heard, as on the MIDI page.
        let notes = f.notes.iter().filter(|n| !n.muted).map(|n| (n.start, n.length, n.key, n.velocity7())).collect();
        pieces.push(Item {
            source: SOURCE.to_owned(),
            id: if several { format!("{id} #{}", i + 1) } else { id.to_owned() },
            ppq: mo_midi::PPQ as u32,
            length,
            beats_per_bar: 4,
            first_downbeat: 0,
            aligned: true,
            parts: vec![Part { role: role.to_owned(), notes }],
            keys: vec![(0, length, label_of(key))],
            chords: Vec::new(),
            key_labels: if from_folder { "folder name" } else { "file name" }.to_owned(),
            chord_labels: String::new(),
            licence: item::OWN.to_owned(),
            group: group.to_owned(),
            whole: if f.cut { Whole::Window } else { Whole::Loop },
        });
    }
    (pieces, drums)
}

/// Reads one file found under `root` (named `root_name` in the pieces' ids).
fn read_file(path: &Path, root: &Path, root_name: &str) -> Outcome {
    let (folders, stem) = place(path, root);
    let key = match label(&stem, &folders) {
        (Named::Key(key), from) => (key, from == "folder name"),
        (Named::Unclear, _) => return Outcome::Unclear,
        (Named::Nothing, _) => return Outcome::NoKey,
    };
    let folder_names = [root.to_string_lossy().into_owned()];
    let Ok(found) = extract::read(path, &folder_names) else { return Outcome::Unread };
    let mut id = root_name.to_owned();
    for part in folders.iter().chain(std::iter::once(&stem)) {
        id.push('/');
        id.push_str(part);
    }
    let group = match folders.first() {
        Some(pack) => format!("{root_name}/{pack}"),
        None => root_name.to_owned(),
    };
    let (pieces, drums) = pieces_of(&found, &id, &group, key.0, key.1);
    Outcome::Read { pieces, drums, from_folder: key.1 }
}

/// Every MIDI file under a folder, in order of path. A folder inside it that can't be read is counted and passed by;
/// links aren't followed.
fn midi_files(root: &Path) -> Result<(Vec<PathBuf>, usize), String> {
    let mut files = Vec::new();
    let mut unreadable = 0;
    let mut stack = vec![root.to_path_buf()];
    let mut first = true;
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if first => return Err(format!("{}: {e}", dir.display())),
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };
        first = false;
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() && mo_formats::midi::is_midi_path(&path.to_string_lossy()) {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok((files, unreadable))
}

/// Refuses to write inside a copy of this repository: the file holds Solomon's notes (rule 4).
fn check_out(out: &Path) -> Result<(), String> {
    if out.extension().is_none_or(|e| e != "jsonl") {
        return Err(format!("--out {}: give a file ending in .jsonl", out.display()));
    }
    outside_checkout("--out", out, "your notes")
}

/// Refuses a file `option` would write inside a copy of this repository, when it holds `what` of Solomon's music
/// (rule 4: it must never be committed).
pub fn outside_checkout(option: &str, path: &Path, what: &str) -> Result<(), String> {
    let full = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(repo) = full.ancestors().skip(1).find(|dir| dir.join("crates").join("mo-harmony").is_dir()) {
        return Err(format!(
            "{option} {}: that's inside {}, a copy of this repository. Write it somewhere else, so {what} can never \
             be committed (rule 4).",
            path.display(),
            repo.display()
        ));
    }
    Ok(())
}

/// What the pieces and files came to, for the summary.
#[derive(Default)]
struct Counts {
    files: usize,
    no_key: usize,
    unclear: usize,
    unread: usize,
    from_file: usize,
    from_folder: usize,
    drums: usize,
    cut: usize,
    folders_unread: usize,
    roles: BTreeMap<String, usize>,
    packs: BTreeMap<String, usize>,
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut out: Option<PathBuf> = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(rest.next().ok_or("--out needs a file")?)),
            flag if flag.starts_with("--") => return Err(crate::USAGE.to_owned()),
            folder => roots.push(PathBuf::from(folder)),
        }
    }
    let out = out.ok_or("packs needs --out <file.jsonl>, outside this repository")?;
    if roots.is_empty() {
        return Err("packs needs the folders the packs are in".into());
    }
    check_out(&out)?;
    let mut counts = Counts::default();
    let mut pieces: Vec<Item> = Vec::new();
    for root in &roots {
        let (files, folders_unread) = midi_files(root)?;
        counts.files += files.len();
        counts.folders_unread += folders_unread;
        let root_name =
            root.file_name().map_or_else(|| root.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
        let outcomes: Vec<Outcome> = files.par_iter().map(|path| read_file(path, root, &root_name)).collect();
        for outcome in outcomes {
            match outcome {
                Outcome::NoKey => counts.no_key += 1,
                Outcome::Unclear => counts.unclear += 1,
                Outcome::Unread => counts.unread += 1,
                Outcome::Read { pieces: found, drums, from_folder } => {
                    if from_folder {
                        counts.from_folder += 1;
                    } else {
                        counts.from_file += 1;
                    }
                    counts.drums += drums;
                    for piece in found {
                        let role = piece.parts.first().map_or("", |p| p.role.as_str());
                        *counts
                            .roles
                            .entry(if role.is_empty() { "not known" } else { role }.to_owned())
                            .or_default() += 1;
                        *counts.packs.entry(piece.group.clone()).or_default() += 1;
                        counts.cut += usize::from(piece.whole == Whole::Window);
                        pieces.push(piece);
                    }
                }
            }
        }
    }
    let mut text = String::new();
    for piece in &pieces {
        text.push_str(&serde_json::to_string(piece).map_err(|e| e.to_string())?);
        text.push('\n');
    }
    std::fs::write(&out, text).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("{} MIDI files.", counts.files);
    println!("  Key from the file's name: {}; from a folder's name: {}.", counts.from_file, counts.from_folder);
    println!(
        "  Left out: {} name no key, {} name notes that don't settle one (chord names, two keys, a lone note), {} \
         couldn't be read.",
        counts.no_key, counts.unclear, counts.unread
    );
    if counts.folders_unread > 0 {
        println!("  {} folders couldn't be opened and were passed by.", counts.folders_unread);
    }
    let roles: Vec<String> = counts.roles.iter().map(|(r, n)| format!("{r} {n}")).collect();
    println!(
        "{} pieces (one a part) from {} packs: {}; {} drum parts left out.",
        pieces.len(),
        counts.packs.len(),
        roles.join(", "),
        counts.drums
    );
    if counts.cut > 0 {
        println!(
            "  {} of the pieces were cut from parts longer than 16 bars, as the MIDI page cuts them: each is heard \
             as a window, as the page hears it.",
            counts.cut
        );
    }
    println!("Written to {}. It holds your notes: keep it on this computer.", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_harmony::PitchClass;

    fn key(name: &str) -> Named {
        Named::Key(Key::parse(name).unwrap())
    }

    #[test]
    fn a_name_labels_a_file_only_when_it_gives_exactly_one_key() {
        for (name, want) in [
            ("Loop 3 - C minor", key("Cm")),
            ("Keys_90_Ebmaj", key("Eb")),
            ("F#m", key("F#m")),
            ("Neo Soul 01 F# Minor 85bpm", key("F#m")),
            ("Gospel Keys (Bb Major)", key("Bb")),
            ("Pad_Am_120", key("Am")),
            ("Chords in Db", key("Db")),
            ("Key of E", key("E")),
            ("Bbmin Groove", key("Bbm")),
            ("ABmin", Named::Nothing),
            ("Bass Line 1", Named::Nothing),
            ("Deep Gospel Chords 120bpm", Named::Nothing),
            ("Ebony", Named::Nothing),
            // Chord names: the key isn't named.
            ("Fm Bb Eb", Named::Unclear),
            ("Fm9 Bb13 Ebmaj9", Named::Unclear),
            ("Dm7 - G7 - Cmaj7", Named::Unclear),
            ("Cm Gospel Cm7", Named::Unclear),
            ("C/E walk", Named::Unclear),
            // A lone note: a key or not?
            ("Stab (Eb)", Named::Unclear),
            ("E Piano C minor", Named::Unclear),
            // Two keys.
            ("Am to C major", Named::Unclear),
        ] {
            assert_eq!(read_name(name), want, "{name}");
        }
        // The same key twice is still two keys named: left out, as a chord name would be.
        assert_eq!(read_name("Cm Cm"), Named::Unclear);
        assert_eq!(read_name(""), Named::Nothing);
    }

    #[test]
    fn a_file_whose_name_says_nothing_takes_the_nearest_folder_that_does() {
        let folders = |f: &[&str]| f.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let c_minor = Key::minor(PitchClass::C);
        assert_eq!(label("Loop 01", &folders(&["Pack", "C Minor", "Keys"])), (Named::Key(c_minor), "folder name"));
        assert_eq!(label("Loop Am", &folders(&["Pack", "C Minor"])), (key("Am"), "file name"));
        // The nearest folder naming notes decides, even when it can't settle one key.
        assert_eq!(label("Loop 01", &folders(&["C Minor", "Fm Bb Eb"])).0, Named::Unclear);
        // Only three folders up.
        assert_eq!(label("Loop", &folders(&["C Minor", "a", "b", "c"])).0, Named::Nothing);
    }

    /// A one-track MIDI file at 96 ticks a beat, its notes struck one after another for a beat each on a channel.
    fn smf(keys: &[u8], channel: u8, name: &str) -> Vec<u8> {
        let mut track = Vec::new();
        track.extend([0x00, 0xFF, 0x03, name.len() as u8]);
        track.extend(name.as_bytes());
        for &key in keys {
            track.extend([0x00, 0x90 | channel, key, 100, 0x60, 0x80 | channel, key, 0]);
        }
        track.extend([0x00, 0xFF, 0x2F, 0x00]);
        let mut out = b"MThd".to_vec();
        out.extend([0, 0, 0, 6, 0, 0, 0, 1, 0, 96]);
        out.extend(b"MTrk");
        out.extend((track.len() as u32).to_be_bytes());
        out.extend(track);
        out
    }

    #[test]
    fn each_part_becomes_a_piece_with_its_files_key_and_its_pack() {
        let root = Path::new("/packs");
        let path = root.join("Gospel Pack").join("Melodies").join("Lead C minor.mid");
        let (folders, stem) = place(&path, root);
        assert_eq!(
            (folders.as_slice(), stem.as_str()),
            (["Gospel Pack".to_owned(), "Melodies".to_owned()].as_slice(), "Lead C minor")
        );
        let tune = smf(&[60, 62, 63, 65, 67, 68, 70, 72], 0, "Lead");
        let found = extract::from_bytes(&path, &tune, &["/packs".to_owned()]).unwrap();
        let c_minor = Key::minor(PitchClass::C);
        let (pieces, drums) =
            pieces_of(&found, "packs/Gospel Pack/Melodies/Lead C minor", "packs/Gospel Pack", c_minor, false);
        assert_eq!((pieces.len(), drums), (1, 0));
        let p = &pieces[0];
        assert_eq!((p.source.as_str(), p.group.as_str(), p.licence.as_str()), (SOURCE, "packs/Gospel Pack", item::OWN));
        assert_eq!(p.keys, [(0, 768, "C:min".to_owned())]);
        assert_eq!((p.ppq, p.length, p.key_labels.as_str()), (96, 768, "file name"));
        assert_eq!(p.parts[0].notes.len(), 8);
        assert_eq!(p.parts[0].notes[0], (0, 96, 60, 100));
        assert!(!p.may_learn() && p.is_own() && p.whole == Whole::Loop);
        // The piece reads back as the bench reads its files.
        let back: Item = serde_json::from_str(&serde_json::to_string(p).unwrap()).unwrap();
        assert_eq!((back.group, back.keys, back.parts[0].notes.len()), (p.group.clone(), p.keys.clone(), 8));
        // Drums aren't kept.
        let kit = extract::from_bytes(&path, &smf(&[36, 38, 42, 36, 38, 42], 9, "Kit"), &[]).unwrap();
        assert_eq!(pieces_of(&kit, "x", "g", c_minor, false).1, 1);
    }

    #[test]
    fn the_file_written_stays_out_of_the_repository() {
        let here = std::env::current_dir().unwrap();
        let inside = here.join("packs.jsonl");
        // The tests run in a copy of the repository: writing in it is refused.
        assert!(check_out(&inside).unwrap_err().contains("rule 4"));
        assert!(check_out(Path::new("/elsewhere/packs.txt")).unwrap_err().contains(".jsonl"));
        assert!(check_out(Path::new("/elsewhere/packs.jsonl")).is_ok());
        // So is keyfit's dump of how each of his pieces came out.
        assert!(outside_checkout("--dump", &here.join("runs.tsv"), "x").unwrap_err().starts_with("--dump "));
    }
}
