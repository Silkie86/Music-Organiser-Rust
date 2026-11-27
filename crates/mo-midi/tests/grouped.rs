//! Reading the index sorted into sets of the same notes ([`MidiStore::grouped`]) gives the same catalogue as reading
//! every part and sorting them after ([`MidiStore::all`] and [`Catalogue::new`]), read as one or in pieces.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mo_midi::store::MidiStore;
use mo_midi::{Catalogue, Entry};
use rusqlite::params;

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An index of `files` song saves and pack files with `per_file` parts each: most notes are in several files (older
/// saves of a song), two sets of notes differ only in how hard they're played (one hash), some files' dates aren't
/// real, and one set of notes has a kind you gave it.
fn index(name: &str, files: i64, per_file: i64) -> PathBuf {
    let path = folder(name).join("midi-index.db");
    drop(MidiStore::open(&path).unwrap());
    let mut conn = rusqlite::Connection::open(&path).unwrap();
    let tx = conn.transaction().unwrap();
    // A third as many different hashes as parts (at most 1,000); the last notes are hash 1's notes played softer.
    let sets = (files * per_file / 3).min(1_000);
    for n in 1..=sets + 1 {
        let hash = format!("H{:04}", if n == sets + 1 { 1 } else { n });
        tx.execute(
            "INSERT INTO notes (id, sound, hash, count, bars, lowest, highest, chroma, features)
             VALUES (?1, ?2, ?3, 8, ?4, ?5, 72, zeroblob(12), zeroblob(48))",
            params![n, format!("S{n}"), hash, 1 + n % 8, 36 + n % 24],
        )
        .unwrap();
    }
    let mut part = 0;
    for f in 1..=files {
        // Every 50th file's date isn't real (a clock set wrong); a few files share a date.
        let mtime = if f % 50 == 0 { 9_000_000_000 } else { 1_700_000_000 + (f / 3) * 60 };
        let (kind, ending) = if f % 4 == 0 { (1, "mid") } else { (0, "flp") };
        tx.execute(
            "INSERT INTO files (id, path, kind, mtime, size, version) VALUES (?1, ?2, ?3, ?4, 100, 1)",
            params![f, format!("C:/Music/Song {}/Save {f}.{ending}", f % 37), kind, mtime],
        )
        .unwrap();
        for p in 0..per_file {
            part += 1;
            let notes = 1 + (f * 7 + p * 13) % (sets + 1);
            tx.execute(
                "INSERT INTO parts (id, file, notes, part, clip, role, why, drums, program, track_name)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0, NULL, '')",
                params![part, f, notes, format!("Track {p}"), format!("Pattern {}", p % 5), p % 6],
            )
            .unwrap();
        }
    }
    tx.execute("INSERT INTO your_roles (hash, role) VALUES ('H0007', 2)", []).unwrap();
    tx.commit().unwrap();
    path
}

/// What a catalogue's entry says, for comparing.
fn said(e: &Entry) -> String {
    let p = &e.phrase;
    let others: Vec<(&str, i64)> = e.other_sources.iter().map(|s| (s.path.as_str(), s.mtime)).collect();
    format!(
        "{} {} {} {:?} {} {} {:?} {:?} {:?}",
        p.id,
        p.hash,
        p.source.path,
        e.role(),
        p.part,
        p.clip,
        others,
        e.key_group(),
        e.within
    )
}

fn same_catalogue(path: &Path) {
    let store = MidiStore::open_read_only(path).unwrap();
    let none = (HashMap::new(), HashMap::new());
    let every = Catalogue::new(store.all(&none.0, &none.1).unwrap());
    let grouped = store.grouped(&none.0, &none.1).unwrap();
    let kept = grouped.sets();
    let cat = Catalogue::from_grouped(grouped);
    assert_eq!(kept, every.entries.len(), "only the newest of each set read in full");
    assert_eq!((cat.phrase_count, cat.line_count), (every.phrase_count, every.line_count));
    let a: Vec<String> = cat.entries.iter().map(said).collect();
    let b: Vec<String> = every.entries.iter().map(said).collect();
    assert_eq!(a, b);
    // Copies there are, and the one you gave a kind keeps it.
    assert!(cat.entries.iter().any(|e| e.copies() > 2));
    let yours = cat.find("H0007").and_then(|at| cat.entry(at)).unwrap();
    assert_eq!(yours.role().code(), 2);
    for e in &cat.entries {
        assert_eq!(cat.find(&e.phrase.hash).and_then(|at| cat.entry(at)).map(|f| f.phrase.id), Some(e.phrase.id));
    }
}

#[test]
fn a_small_index_read_as_one_gives_the_same_catalogue() {
    let path = index("grouped-small", 120, 6);
    same_catalogue(&path);
}

#[test]
fn a_big_index_read_in_pieces_gives_the_same_catalogue() {
    // 45,000 parts: two pieces or more on four threads.
    let path = index("grouped-big", 1_500, 30);
    rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap().install(|| same_catalogue(&path));
}
