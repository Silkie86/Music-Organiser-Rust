//! The indexer filling `midi-index.db` from `.mid` files: only what changed is read, each set of notes is stored once,
//! gone files are forgotten, failed ones listed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_midi::indexer::{self, Wanted};
use mo_midi::store::MidiStore;
use mo_midi::{Catalogue, Role, SourceKind};

/// A one-track Standard MIDI File at 96 PPQ playing `keys` as sixteenths... quarter notes, one after another.
fn smf(keys: &[u8]) -> Vec<u8> {
    let mut track = Vec::new();
    for &key in keys {
        track.extend([0x00, 0x90, key, 100, 0x60, 0x80, key, 0]);
    }
    track.extend([0x00, 0xFF, 0x2F, 0x00]);
    let mut out = b"MThd".to_vec();
    out.extend([0, 0, 0, 6, 0, 0, 0, 1, 0, 96]);
    out.extend(b"MTrk");
    out.extend((track.len() as u32).to_be_bytes());
    out.extend(track);
    out
}

const BASS: &[u8] = &[36, 36, 43, 41, 36, 36, 43, 41];
const TUNE: &[u8] = &[72, 74, 76, 79, 76, 74, 72, 71];

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Pack").join("Basslines")).unwrap();
    dir
}

/// Every .mid file under `dir`, as the library's folder walk would give them.
fn wanted(dir: &Path) -> Vec<Wanted> {
    let mut list = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap() {
            let e = e.unwrap();
            let meta = e.metadata().unwrap();
            if meta.is_dir() {
                stack.push(e.path());
            } else if mo_formats::midi::is_midi_path(&e.path().to_string_lossy()) {
                list.push(Wanted {
                    path: e.path().to_string_lossy().into_owned(),
                    kind: SourceKind::File,
                    mtime: mo_library::scan::mtime(&meta),
                    size: meta.len() as i64,
                });
            }
        }
    }
    list
}

fn run(store: &mut MidiStore, dir: &Path, retry: bool) -> indexer::Summary {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let plan = indexer::plan(&wanted(dir), &store.stamps().unwrap(), &[], retry);
    let folders = vec![dir.to_string_lossy().into_owned()];
    indexer::run(store, &plan, &folders, &pool, &AtomicBool::new(false), &|_, _, _| {}).unwrap()
}

fn count(db: &Path, table: &str) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap()
}

#[test]
fn reads_only_what_changed_and_stores_notes_once() {
    let dir = folder("mo-midi-indexer");
    let pack = dir.join("Pack").join("Basslines");
    std::fs::write(pack.join("Low 01.mid"), smf(BASS)).unwrap();
    // The same notes in a second file: one set of notes, two parts.
    std::fs::write(pack.join("Low 01 copy.mid"), smf(BASS)).unwrap();
    std::fs::write(dir.join("Tune.mid"), smf(TUNE)).unwrap();
    std::fs::write(dir.join("Broken.mid"), b"not a MIDI file at all, just some text").unwrap();
    let db = dir.join("midi-index.db");
    let mut store = MidiStore::open(&db).unwrap();

    let first = run(&mut store, &dir, false);
    assert_eq!((first.read, first.failed, first.parts), (4, 1, 3));
    assert_eq!(count(&db, "notes"), 2);
    assert_eq!(count(&db, "parts"), 3);
    let failures = store.failures().unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].name(), "Broken.mid");

    // Nothing changed: nothing to read. A failed file is only tried again when asked.
    assert_eq!(run(&mut store, &dir, false).read, 0);
    assert_eq!(run(&mut store, &dir, true).read, 1);

    // The page's list: newest first, one entry per set of notes, with the folders' names as hints.
    let all = store.all(&HashMap::new(), &HashMap::new()).unwrap();
    assert_eq!(all.len(), 3);
    let bass = all.iter().find(|p| p.clip == "Low 01").unwrap();
    assert_eq!(bass.role, Role::Bass);
    assert_eq!(bass.role_why, "Single notes, low down, and its name agrees", "the Basslines folder says so");
    assert_eq!(bass.source.daw, "MIDI");
    assert_eq!(store.notes(&bass.hash).unwrap().len(), 8);
    assert_eq!(store.notes(&bass.hash).unwrap()[0].velocity7(), 100);
    let catalogue = Catalogue::new(all);
    assert_eq!(catalogue.entries.len(), 2);
    assert_eq!(catalogue.entries.iter().map(|e| e.copies()).max(), Some(2));

    // A file gone: forgotten, and its notes too once no file has them.
    std::fs::remove_file(dir.join("Tune.mid")).unwrap();
    let after = run(&mut store, &dir, false);
    assert_eq!((after.read, after.forgotten), (0, 1));
    assert_eq!(count(&db, "notes"), 1);

    // A copy gone: the notes stay, the other file still has them.
    std::fs::remove_file(pack.join("Low 01 copy.mid")).unwrap();
    run(&mut store, &dir, false);
    assert_eq!((count(&db, "notes"), count(&db, "parts")), (1, 1));
}

#[test]
fn your_corrections_win_over_the_csharp_apps() {
    let dir = folder("mo-midi-corrections");
    std::fs::write(dir.join("Tune.mid"), smf(TUNE)).unwrap();
    let db = dir.join("midi-index.db");
    let mut store = MidiStore::open(&db).unwrap();
    run(&mut store, &dir, false);
    let hash = store.all(&HashMap::new(), &HashMap::new()).unwrap()[0].hash.clone();

    let cs_roles = HashMap::from([(hash.clone(), Role::Arp)]);
    let cs_octaves = HashMap::from([(hash.clone(), 1)]);
    let p = &store.all(&cs_roles, &cs_octaves).unwrap()[0];
    assert_eq!((p.role(), p.your_octave), (Role::Arp, Some(1)), "the C# app's, read live");

    store.set_your_role(&hash, Some(Role::Chords)).unwrap();
    store.set_your_octave(&hash, Some(-2)).unwrap();
    let p = &store.all(&cs_roles, &cs_octaves).unwrap()[0];
    assert_eq!((p.role(), p.your_octave), (Role::Chords, Some(-2)), "the Rust app's win");

    store.set_your_role(&hash, None).unwrap();
    assert_eq!(store.all(&HashMap::new(), &HashMap::new()).unwrap()[0].role(), Role::Melody);

    // Read-only, the page can't change it.
    let page = MidiStore::open_read_only(&db).unwrap();
    assert_eq!(page.all(&HashMap::new(), &HashMap::new()).unwrap().len(), 1);
}

#[test]
fn a_stopped_run_saves_what_it_read() {
    let dir = folder("mo-midi-stop");
    for i in 0..20u8 {
        std::fs::write(dir.join(format!("Tune {i}.mid")), smf(&[60 + i % 12, 62, 64, 65])).unwrap();
    }
    let mut store = MidiStore::open(&dir.join("midi-index.db")).unwrap();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let plan = indexer::plan(&wanted(&dir), &store.stamps().unwrap(), &[], false);
    let summary = indexer::run(&mut store, &plan, &[], &pool, &AtomicBool::new(true), &|_, _, _| {}).unwrap();
    assert!(summary.stopped);
    assert_eq!(summary.read, store.stamps().unwrap().len());
}

#[test]
fn away_folders_are_kept() {
    let stamps = HashMap::from([(
        mo_formats::case::key(r"E:\Packs\a.mid"),
        mo_midi::store::Stamp { path: r"E:\Packs\a.mid".into(), mtime: 1, size: 1, version: 1, failed: false },
    )]);
    assert!(indexer::plan(&[], &stamps, &[r"E:\Packs".into()], false).forget.is_empty());
    assert_eq!(indexer::plan(&[], &stamps, &[], false).forget, [r"E:\Packs\a.mid"]);
}
