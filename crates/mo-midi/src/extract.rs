//! The parts in one file, as the C# `MidiIndexer` finds them: each pattern (FL Studio) or clip (Bitwig, Studio One)
//! as one part per channel or track with notes, and each track (or channel) of a `.mid` file. Long parts are cut into
//! phrases ([`analyser::split`]), and the same notes twice in one file are kept once.
//!
//! Files are only read. A damaged file gives an error for the failed list, never a crash: every reader runs inside
//! `catch_unwind`, as the app must with overflow checks on.
//!
//! Not yet: phrases joined from clips that play back to back (the C# `PhraseJoiner`); it needs each DAW's
//! arrangement, which comes with the arrangement preview.

use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use mo_formats::{MidiClip, NoteEvent, bitwig, case, fl, midi, paths, studio_one};

use crate::analyser::{self, Analysis};
use crate::instruments;
use crate::member::PhraseMember;

/// The analysis's own version; raised when what it finds changes, so every file is read again once. It follows the C#
/// `MidiIndexer.Version` (9) it was ported from.
pub const VERSION: u32 = 9;
/// What a `.mid` file was read with: the analysis and the MIDI file reader.
pub const FILE_VERSION: u32 = VERSION * 100 + midi::smf::VERSION;

/// Shorter than a MIDI file's header (14 bytes) and one empty track (8): an empty or never-finished download, skipped
/// as empty rather than failed.
pub const MIN_MIDI_BYTES: u64 = 22;

/// What a project was read with: the analysis and its DAW's reader, so a better reader reads its MIDI again too.
pub fn project_version(path: &str) -> Option<u32> {
    let reader = match case::to_lower(paths::extension(paths::file_name(path))).as_str() {
        ".flp" => fl::VERSION,
        ".bwproject" => bitwig::reader::VERSION,
        ".song" => studio_one::VERSION,
        _ => return None,
    };
    Some(VERSION * 100 + reader)
}

/// One part found in a file, ready to save.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The track, channel or `.mid` track name.
    pub part: String,
    /// The pattern or clip name (or the `.mid` file's), with " · bars 9–16" for a piece of a longer part.
    pub clip: String,
    /// At [`crate::PPQ`], starting in bar 1.
    pub notes: Vec<NoteEvent>,
    pub analysis: Analysis,
    pub drums: bool,
    /// For a phrase joined from clips.
    pub members: Vec<PhraseMember>,
    pub program: Option<u8>,
    /// A `.mid` track's own name where `part` leaves it out.
    pub track_name: String,
}

/// Why a file couldn't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unread {
    /// Another program has it open (a backup, a virus check): what was read from it before is kept, and it's read
    /// again next time.
    InUse(String),
    /// Gone, not allowed, or damaged: noted once, and only tried again when asked.
    Failed(String),
}

/// Plugins whose notes pick a slice or a pad, not a pitch (the C# `ProjectKey.NotPitched`).
const NOT_PITCHED: &[&str] = &[
    "Slicex",
    "Fruity Slicer",
    "FPC",
    "Drum Machine",
    "Battery",
    "Battery 4",
    "Drumaxx",
    "Fruity DrumSynth Live",
    "Drumpad",
    "BeepMap",
    "Autogun",
];

/// Whether a track named after this plugin plays slices or pads rather than pitches.
pub fn is_unpitched_plugin(name: &str) -> bool {
    NOT_PITCHED.iter().any(|p| case::eq_ignore_case(p, name))
}

/// Reads a file from disk and finds its parts. `midi_folders` are the MIDI library folders, whose own names aren't
/// hints for a `.mid` file inside them.
pub fn read(path: &Path, midi_folders: &[String]) -> Result<Vec<Found>, Unread> {
    let raw = std::fs::read(path).map_err(|e| unread(&e))?;
    from_bytes(path, &raw, midi_folders)
}

/// Finds the parts in a file's bytes, already read from `path`.
pub fn from_bytes(path: &Path, raw: &[u8], midi_folders: &[String]) -> Result<Vec<Found>, Unread> {
    let text = path.to_string_lossy();
    let found = catch_unwind(AssertUnwindSafe(|| {
        if midi::is_midi_path(&text) {
            from_midi_file(&text, raw, path, midi_folders)
        } else {
            from_project(&text, raw)
        }
    }));
    match found {
        Ok(result) => result,
        Err(_) => Err(Unread::Failed(
            "The reader stopped on it: it looks damaged in a way the reader doesn't know yet".into(),
        )),
    }
}

/// Why a file couldn't be read, in a few plain words, for the list of failed ones (the C# `MidiIndexer.Why`).
pub fn unread(e: &io::Error) -> Unread {
    // Windows' sharing and lock violations (32, 33); elsewhere the "would block" .NET gives for the same.
    let in_use = matches!(e.raw_os_error(), Some(32 | 33)) && cfg!(windows) || e.kind() == io::ErrorKind::WouldBlock;
    if in_use {
        return Unread::InUse(format!("It couldn't be opened, maybe because another program had it open ({e})"));
    }
    Unread::Failed(match e.kind() {
        io::ErrorKind::PermissionDenied => "Windows didn't allow reading it".into(),
        io::ErrorKind::NotFound => "It isn't there any more".into(),
        io::ErrorKind::UnexpectedEof | io::ErrorKind::InvalidData => {
            format!("It looks cut short or damaged, or saved in a way the reader doesn't know yet ({e})")
        }
        _ => format!("It couldn't be opened, maybe because another program had it open ({e})"),
    })
}

fn damaged(what: &str) -> Unread {
    Unread::Failed(format!("It looks cut short or damaged, or saved in a way the reader doesn't know yet ({what})"))
}

/// Each pattern or clip as one part per channel or track with notes.
fn from_project(path: &str, raw: &[u8]) -> Result<Vec<Found>, Unread> {
    let clips: Vec<MidiClip> = match case::to_lower(paths::extension(paths::file_name(path))).as_str() {
        ".flp" => {
            let flp = fl::events::load(raw).map_err(|_| damaged("not an FL Studio project, or damaged"))?;
            fl::song::extract_midi(&flp)
        }
        // A file that isn't Bitwig's has no clips, as in the C# reader.
        ".bwproject" => bitwig::reader::Project::open(raw.to_vec()).map(|p| p.midi()).unwrap_or_default(),
        ".song" => studio_one::Song::open(raw).map_err(|e| damaged(&e.to_string()))?.midi(),
        _ => Vec::new(),
    };
    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for clip in &clips {
        for (track, notes) in &clip.tracks {
            let drums = is_unpitched_plugin(track);
            let clip_name = if case::eq_ignore_case(&clip.name, track) { "" } else { clip.name.as_str() };
            let notes = analyser::normalise(notes, i64::from(clip.ppq), false);
            let hints = project_hints(track, &clip.name);
            add(&mut found, &mut seen, notes, track, clip_name, &hints, drums, None, String::new());
        }
    }
    Ok(found)
}

/// The names a project part is guessed from: its track and its clip's own name.
pub fn project_hints(track: &str, clip: &str) -> String {
    format!("{track} {clip}")
}

/// Each track of a `.mid` file (each channel, in a one-track file), with its file and folder names as hints.
fn from_midi_file(path: &str, raw: &[u8], on_disk: &Path, midi_folders: &[String]) -> Result<Vec<Found>, Unread> {
    let parts = midi::read_from(raw, on_disk).map_err(|e| unread(&e))?;
    let name = paths::stem(paths::file_name(path));
    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for part in parts {
        let hints = file_hints(path, &part.name, part.program, part.drums, midi_folders);
        let track_name = if part.name.is_empty() { part.track_name.trim().to_owned() } else { String::new() };
        let notes = analyser::normalise(&part.notes, i64::from(part.ppq), false);
        add(&mut found, &mut seen, notes, &part.name, name, &hints, part.drums, part.program, track_name);
    }
    Ok(found)
}

/// The names a `.mid` part is guessed from: the part, the file, the last three folders below the MIDI folder it's in,
/// and the General MIDI sound it asks for (not the default piano, and not on drums).
pub fn file_hints(path: &str, part: &str, program: Option<u8>, drums: bool, midi_folders: &[String]) -> String {
    let name = paths::stem(paths::file_name(path));
    let below = folders_below(path, midi_folders);
    let last = below.get(below.len().saturating_sub(3)..).unwrap_or(&[]).join(" ");
    let sound = match program {
        Some(p) if (1..128).contains(&p) && !drums => format!(" {}", instruments::general_midi(p)),
        _ => String::new(),
    };
    format!("{part} {name} {last}{sound}")
}

/// The folders a file sits in below the MIDI folder that holds it (the longest), from the top down. Letter case aside
/// as .NET's `OrdinalIgnoreCase` (by capitals, a letter for a letter), so the folder is cut off by letters.
pub fn folders_below(path: &str, midi_folders: &[String]) -> Vec<String> {
    let dir = path.rfind(['\\', '/']).and_then(|c| path.get(..c)).unwrap_or("");
    let upper = case::key(path);
    let root = midi_folders
        .iter()
        .map(|r| case::key(r))
        .filter(|r| upper.starts_with(r.as_str()))
        .map(|r| r.chars().count())
        .max()
        .unwrap_or(0);
    let rest: String = dir.chars().skip(root).collect();
    rest.split(['\\', '/']).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

/// A part's phrases, each analysed, the same notes twice in one file kept once (the C# `MidiIndexer.Add`).
#[allow(clippy::too_many_arguments)]
fn add(
    found: &mut Vec<Found>,
    seen: &mut std::collections::HashSet<String>,
    notes: Vec<NoteEvent>,
    part: &str,
    clip: &str,
    hints: &str,
    drums: bool,
    program: Option<u8>,
    track_name: String,
) {
    if notes.len() < 3 {
        return;
    }
    for piece in analyser::split(&notes, None) {
        // Keys and chords come with mo-harmony.
        let analysis = analyser::analyse(&piece.notes, hints, drums, None, Vec::new());
        if !seen.insert(analysis.hash.clone()) {
            continue;
        }
        let label = match (piece.place.is_empty(), clip.is_empty()) {
            (true, _) => clip.to_owned(),
            (false, true) => piece.place.clone(),
            (false, false) => format!("{clip} · {}", piece.place),
        };
        found.push(Found {
            part: part.to_owned(),
            clip: label,
            notes: piece.notes,
            analysis,
            drums,
            members: Vec::new(),
            program,
            track_name: track_name.clone(),
        });
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::roles::Role;

    /// A one-track Standard MIDI File at 96 PPQ: a bassline of `count` notes on channel 1 (or 10).
    fn smf(count: u8, channel: u8, name: &str) -> Vec<u8> {
        let mut track = Vec::new();
        track.extend([0x00, 0xFF, 0x03, name.len() as u8]);
        track.extend(name.as_bytes());
        for i in 0..count {
            let key = [36u8, 43, 41, 38][i as usize % 4];
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
    fn a_midi_file_part() {
        let raw = smf(8, 0, "Groove");
        let folders = vec![r"D:\Packs".to_owned()];
        let found = from_bytes(Path::new(r"D:\Packs\Deep House\Basslines\Low 01.mid"), &raw, &folders).unwrap();
        assert_eq!(found.len(), 1);
        let f = &found[0];
        assert_eq!(f.clip, "Low 01");
        assert_eq!(f.analysis.role, Role::Bass);
        assert_eq!(f.notes.len(), 8);
        assert!(!f.drums);
        // Channel 10 is drums.
        let drums = from_bytes(Path::new("x.mid"), &smf(8, 9, "Groove"), &[]).unwrap();
        assert!(drums[0].drums);
        assert_eq!(drums[0].analysis.role, Role::Drums);
        // Too few notes: nothing.
        assert!(from_bytes(Path::new("x.mid"), &smf(2, 0, "A"), &[]).unwrap().is_empty());
    }

    #[test]
    fn damaged_files_are_failures_not_crashes() {
        let raw = smf(8, 0, "Groove");
        assert!(matches!(from_bytes(Path::new("x.mid"), &raw[..30], &[]), Err(Unread::Failed(_)) | Ok(_)));
        assert!(matches!(from_bytes(Path::new("x.flp"), b"not a project", &[]), Err(Unread::Failed(_))));
        assert_eq!(from_bytes(Path::new("x.bwproject"), b"not a project", &[]), Ok(Vec::new()));
        assert!(
            matches!(read(Path::new("/no/such/file.mid"), &[]), Err(Unread::Failed(w)) if w == "It isn't there any more")
        );
    }

    #[test]
    fn hints_as_the_csharp_indexer() {
        let folders = vec![r"d:\packs".to_owned()];
        assert_eq!(folders_below(r"D:\Packs\A\B\C\D\x.mid", &folders), ["A", "B", "C", "D"]);
        assert_eq!(
            file_hints(r"D:\Packs\A\B\C\D\x.mid", "Piano", Some(33), false, &folders),
            "Piano x B C D Electric Bass (finger)"
        );
        assert_eq!(file_hints("x.mid", "", Some(0), false, &[]), " x ");
        assert_eq!(project_hints("Bass", "Pattern 1"), "Bass Pattern 1");
        assert!(is_unpitched_plugin("fpc") && !is_unpitched_plugin("Sytrus"));
    }

    #[test]
    fn versions() {
        assert_eq!(project_version("a.FLP"), Some(900 + fl::VERSION));
        assert_eq!(project_version("a.mid"), None);
    }
}
