//! The MIDI twin test: every part the C# app saved in `midi.db` is worked out again from its notes by the Rust
//! analyser (`mo_midi::analyser`), and every difference is listed: the note hash, bars, lowest and highest note, the
//! pitch-class shares, the shape numbers and the kind of part. Only reads: `midi.db` and `library.db` are opened
//! read-only.
//!
//! The names a part was guessed from aren't kept in `midi.db`, so they're put back together as the C# indexer made
//! them (a `.mid` file's folders come from the library's MIDI folders). Keys and chords wait for `mo-harmony`, so the
//! key the C# app saved is given to the analyser.
//!
//! `midi-twin [--midi <midi.db>] [--library <library.db>] [--summary <file>] [--report <file>]`
//! The summary has counts and field names only; the report lists each difference with its file and part name, so it
//! stays on the PC it ran on.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::LazyLock;

use mo_formats::{NoteEvent, case, paths};
use mo_midi::analyser::{self, Analysis};
use mo_midi::index::{SourceKind, StoredPhrase};
use mo_midi::{MidiIndex, instruments};
use rayon::prelude::*;
use regex::Regex;

/// The C# `MidiIndexer.Version` and `FileRevision` this port follows.
const CS_VERSION: i64 = 9;
const CS_FILE_REVISION: i64 = 1;
/// Parts worked out at a time, so a big index's notes never all sit in memory at once.
const CHUNK: usize = 5000;
/// The most differences of one kind listed in the report.
const EXAMPLES: usize = 200;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

struct Args {
    midi: PathBuf,
    library: Option<PathBuf>,
    summary: Option<PathBuf>,
    report: Option<PathBuf>,
}

fn args() -> Result<Args, String> {
    let data = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("MusicOrganiser"));
    let mut a = Args {
        midi: data.as_ref().map(|d| d.join("midi.db")).unwrap_or_default(),
        library: data.as_ref().map(|d| d.join("library.db")),
        summary: None,
        report: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().map(PathBuf::from).ok_or(format!("{flag} needs a file after it"))?;
        match flag.as_str() {
            "--midi" => a.midi = value,
            "--library" => a.library = Some(value),
            "--summary" => a.summary = Some(value),
            "--report" => a.report = Some(value),
            other => return Err(format!("I don't know {other}. Use --midi, --library, --summary or --report.")),
        }
    }
    if a.midi.as_os_str().is_empty() {
        return Err("Say where midi.db is: midi-twin --midi <file>".into());
    }
    Ok(a)
}

/// One difference: which field, and the two values.
struct Diff {
    field: &'static str,
    detail: String,
}

fn run() -> Result<(), String> {
    let a = args()?;
    let index = MidiIndex::open_read_only(&a.midi).map_err(|e| e.to_string())?;
    // The MIDI folders, to put a .mid file's folder names back as the C# indexer saw them.
    let folders = match &a.library {
        Some(lib) if lib.exists() => mo_library::Library::open_read_only(lib)
            .and_then(|l| l.list_setting("midi_folders"))
            .map_err(|e| e.to_string())?,
        _ => Vec::new(),
    };
    let all = index.all().map_err(|e| e.to_string())?;
    // Only what the C# analyser this port follows saved (version 9; .mid files 10): a file read by an older one, or
    // kept from before because another program had it open, follows older rules.
    let versions = index.sources().map_err(|e| e.to_string())?;
    let current = |p: &StoredPhrase| {
        let version = versions.get(&case::key(&p.source.path)).map_or(0, |v| v.2);
        match p.source.kind {
            SourceKind::Project => version / 100 == CS_VERSION,
            SourceKind::File => version == CS_VERSION + CS_FILE_REVISION,
        }
    };
    let total = all.len();
    let phrases: Vec<StoredPhrase> = all.into_iter().filter(|p| current(p)).collect();
    let older = total - phrases.len();
    eprintln!("MIDI twin test: {total} parts in {}.", a.midi.display());

    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).max(1));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .start_handler(|_| {
            let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
        })
        .build()
        .map_err(|e| format!("Couldn't start the threads: {e}"))?;

    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut roles: BTreeMap<String, usize> = BTreeMap::new();
    let mut examples: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let (mut same, mut done, mut no_notes) = (0usize, 0usize, 0usize);
    for chunk in phrases.chunks(CHUNK) {
        eprint!("\r  {done} of {} parts...", phrases.len());
        // Read here, one after another (one connection); worked out on every core.
        let mut with_notes = Vec::with_capacity(chunk.len());
        for p in chunk {
            let notes = index.notes(p.id, None).map_err(|e| e.to_string())?;
            with_notes.push((p, notes));
        }
        let found: Vec<Option<Vec<Diff>>> = pool.install(|| {
            with_notes.par_iter().map(|(p, notes)| (!notes.is_empty()).then(|| compare(p, notes, &folders))).collect()
        });
        for ((p, _), diffs) in with_notes.iter().zip(found) {
            done += 1;
            let Some(diffs) = diffs else {
                no_notes += 1;
                continue;
            };
            if diffs.is_empty() {
                same += 1;
            }
            for d in diffs {
                *counts.entry(d.field).or_default() += 1;
                if d.field == "role" {
                    *roles.entry(d.detail.clone()).or_default() += 1;
                }
                let list = examples.entry(d.field).or_default();
                if list.len() < EXAMPLES {
                    list.push(format!("  {} | {} | {}: {}", p.source.path, p.part, p.clip, d.detail));
                }
            }
        }
    }
    eprintln!("\r  {done} of {} parts worked out.", phrases.len());

    let mut summary = String::new();
    let _ = writeln!(
        summary,
        "MIDI twin test: {} parts, {same} the same, {} differ.",
        phrases.len(),
        done - same - no_notes
    );
    if older > 0 {
        let _ = writeln!(summary, "  {older} more read by an older C# app (left out)");
    }
    if no_notes > 0 {
        let _ = writeln!(summary, "  {no_notes} with no notes saved (left out)");
    }
    for (field, n) in &counts {
        let _ = writeln!(summary, "  {field}: {n}");
    }
    if !roles.is_empty() {
        let _ = writeln!(summary, "Kinds (C# → Rust):");
        for (pair, n) in &roles {
            let _ = writeln!(summary, "  {pair}: {n}");
        }
    }
    let mut report = String::new();
    for (field, list) in &examples {
        let _ = writeln!(report, "{field} ({} listed of {}):", list.len(), counts.get(field).copied().unwrap_or(0));
        for line in list {
            let _ = writeln!(report, "{line}");
        }
    }
    write_or_print(a.summary.as_ref(), &summary)?;
    match &a.report {
        Some(path) => std::fs::write(path, report).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?,
        None if !examples.is_empty() => println!("{report}"),
        None => {}
    }
    Ok(())
}

fn write_or_print(path: Option<&PathBuf>, text: &str) -> Result<(), String> {
    match path {
        Some(p) => std::fs::write(p, text).map_err(|e| format!("Couldn't write {}: {e}", p.display())),
        None => {
            print!("{text}");
            Ok(())
        }
    }
}

/// The part worked out again, and every field that differs from what the C# app saved.
fn compare(p: &StoredPhrase, notes: &[NoteEvent], folders: &[String]) -> Vec<Diff> {
    let a: Analysis = analyser::analyse(notes, &hints(p, folders), p.drums, p.key, Vec::new());
    let mut diffs = Vec::new();
    let check = |diffs: &mut Vec<Diff>, field: &'static str, cs: String, rust: String| {
        if cs != rust {
            diffs.push(Diff { field, detail: format!("{cs} → {rust}") });
        }
    };
    check(&mut diffs, "hash", p.hash.clone(), a.hash.clone());
    check(&mut diffs, "notes", p.notes.to_string(), notes.len().to_string());
    check(&mut diffs, "bars", p.bars.to_string(), a.bars.to_string());
    check(&mut diffs, "lowest", p.lowest.to_string(), a.lowest.to_string());
    check(&mut diffs, "highest", p.highest.to_string(), a.highest.to_string());
    check(&mut diffs, "chroma", format!("{:?}", p.chroma), format!("{:?}", analyser::bytes(&a.chroma, true)));
    let features = analyser::bytes(&a.features, false);
    if p.features != features {
        // Off by one somewhere is a float rounding the other way; more is a real difference.
        let worst = p.features.iter().zip(&features).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(u8::MAX);
        let field = if worst <= 1 && p.features.len() == features.len() { "features (off by 1)" } else { "features" };
        diffs.push(Diff { field, detail: format!("{:?} → {features:?}", p.features) });
    }
    check(&mut diffs, "role", p.role.name().to_owned(), a.role.name().to_owned());
    if p.role == a.role {
        check(&mut diffs, "role reason", p.role_why.clone(), a.role_why.to_owned());
    }
    diffs
}

/// A place a piece of a longer part sits: "bar 3", "bars 9–16".
static PLACE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new("^bars? [0-9]+(–[0-9]+)?$").ok());

/// The names the C# indexer guessed the part from (`MidiIndexer.FromProject`, `FromFile` and `Add`).
fn hints(p: &StoredPhrase, folders: &[String]) -> String {
    let is_place = |s: &str| PLACE.as_ref().is_some_and(|re| re.is_match(s));
    match p.source.kind {
        // A phrase joined from clips: its track and its own name, place and all.
        SourceKind::Project if !p.members.is_empty() => format!("{} {}", p.part, p.clip),
        SourceKind::Project => {
            // The clip's own name, without where the piece sits; none when it was named as the track.
            let clip = if is_place(&p.clip) {
                ""
            } else {
                match p.clip.rsplit_once(" · ") {
                    Some((name, place)) if is_place(place) => name,
                    _ => p.clip.as_str(),
                }
            };
            format!("{} {}", p.part, if clip.is_empty() { p.part.as_str() } else { clip })
        }
        SourceKind::File => {
            let path = p.source.path.as_str();
            let name = paths::stem(paths::file_name(path));
            let dir = path.rfind(['\\', '/']).and_then(|c| path.get(..c)).unwrap_or("");
            // The folders below the MIDI folder it's in (the longest that holds it), the last three. Letter case
            // aside as .NET's OrdinalIgnoreCase (by capitals, a letter for a letter), so the root is cut off by letters.
            let upper = case::key(dir);
            let root = folders
                .iter()
                .map(|r| case::key(r))
                .filter(|r| case::key(path).starts_with(r.as_str()))
                .max_by_key(|r| r.chars().count())
                .map_or(0, |r| r.chars().count());
            let rest: String = dir.chars().skip(root.min(upper.chars().count())).collect();
            let below: Vec<&str> = rest.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
            let last = below.get(below.len().saturating_sub(3)..).unwrap_or(&[]).join(" ");
            let sound = match p.program {
                Some(program) if (1..128).contains(&program) && !p.drums => {
                    format!(" {}", instruments::general_midi(program))
                }
                _ => String::new(),
            };
            format!("{} {name} {last}{sound}", p.part)
        }
    }
}
