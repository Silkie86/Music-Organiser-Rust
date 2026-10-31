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
    smf_at(keys, 100)
}

/// A MIDI file of `keys`, one after another, played at `velocity`.
fn smf_at(keys: &[u8], velocity: u8) -> Vec<u8> {
    let mut track = Vec::new();
    for &key in keys {
        track.extend([0x00, 0x90, key, velocity, 0x60, 0x80, key, 0]);
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
    indexer::run(store, &plan, &folders, &pool, 2, &AtomicBool::new(false), &|_, _, _| {}).unwrap()
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
    assert_eq!(store.notes(bass.id, &bass.hash).unwrap().len(), 8);
    assert_eq!(store.notes(bass.id, &bass.hash).unwrap()[0].velocity7(), 100);
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

    // Back to the guess: it wins over the C# app's correction too.
    store.set_your_role(&hash, None).unwrap();
    store.set_your_octave(&hash, None).unwrap();
    let p = &store.all(&cs_roles, &cs_octaves).unwrap()[0];
    assert_eq!((p.role(), p.your_octave), (Role::Melody, None));

    // Read-only, the page can't change it.
    let page = MidiStore::open_read_only(&db).unwrap();
    assert_eq!(page.all(&HashMap::new(), &HashMap::new()).unwrap().len(), 1);
}

#[test]
fn a_stopped_run_saves_what_it_read() {
    let dir = folder("mo-midi-stop");
    for i in 0..250u32 {
        let keys = [60 + (i % 12) as u8, 62 + (i / 12 % 12) as u8, 64, 65 + (i / 144) as u8];
        std::fs::write(dir.join(format!("Tune {i}.mid")), smf(&keys)).unwrap();
    }
    let mut store = MidiStore::open(&dir.join("midi-index.db")).unwrap();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let plan = indexer::plan(&wanted(&dir), &store.stamps().unwrap(), &[], false);
    assert_eq!(plan.todo.len(), 250);
    // A newer run wanted as soon as the first batch is saved.
    let stop = AtomicBool::new(false);
    let summary = indexer::run(&mut store, &plan, &[], &pool, 2, &stop, &|done, _, _| {
        if done > 0 {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    })
    .unwrap();
    assert!(summary.stopped);
    assert!(summary.read >= 100 && summary.read < 250, "{summary:?}");
    assert_eq!(summary.read, store.stamps().unwrap().len(), "everything read was saved");
    // The next run reads only the rest.
    let rest = indexer::plan(&wanted(&dir), &store.stamps().unwrap(), &[], false);
    assert_eq!(rest.todo.len(), 250 - summary.read);
}

#[test]
fn not_on_one_of_its_own_threads() {
    let dir = folder("mo-midi-own-thread");
    let mut store = MidiStore::open(&dir.join("midi-index.db")).unwrap();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let plan = indexer::Plan::default();
    let r = pool.install(|| indexer::run(&mut store, &plan, &[], &pool, 1, &AtomicBool::new(false), &|_, _, _| {}));
    assert!(r.is_err());
}

#[test]
fn plan_reads_each_file_once_and_skips_empty_ones() {
    let w = |path: &str, size: i64| Wanted { path: path.into(), kind: SourceKind::File, mtime: 5, size };
    let wanted =
        [w(r"L:\Midi\a.mid", 100), w(r"L:\MIDI\A.MID", 100), w(r"L:\Midi\empty.mid", 10), w(r"L:\Midi\._a.mid", 100)];
    let plan = indexer::plan(&wanted, &[], &[], false);
    assert_eq!(plan.todo.len(), 1);
    // Two rows for one file (letters whose case SQLite doesn't fold): the one spelled as wanted stays.
    let stamp = |path: &str| mo_midi::store::Stamp {
        path: path.into(),
        mtime: 5,
        size: 100,
        version: mo_midi::extract::FILE_VERSION,
        failed: false,
    };
    let wanted = [w(r"L:\Midi\CAFÉ.mid", 100)];
    let plan = indexer::plan(&wanted, &[stamp(r"L:\Midi\Café.mid"), stamp(r"L:\Midi\CAFÉ.mid")], &[], false);
    assert_eq!(plan.todo.len(), 0);
    assert_eq!(plan.forget, [r"L:\Midi\Café.mid"]);
}

#[test]
fn never_opens_the_csharp_midi_db_for_writing() {
    let dir = folder("mo-midi-guard");
    let cs = dir.join("midi.db");
    let conn = rusqlite::Connection::open(&cs).unwrap();
    conn.execute_batch("CREATE TABLE sources (path TEXT); CREATE TABLE phrases (id INTEGER);").unwrap();
    drop(conn);
    let before = std::fs::read(&cs).unwrap();
    assert!(MidiStore::open(&cs).is_err());
    assert!(MidiStore::open_read_only(&cs).is_err());
    assert_eq!(std::fs::read(&cs).unwrap(), before);
}

#[test]
fn louder_copies_keep_their_own_velocities() {
    let dir = folder("mo-midi-velocity");
    std::fs::write(dir.join("Soft.mid"), smf(TUNE)).unwrap();
    std::fs::write(dir.join("Loud.mid"), smf_at(TUNE, 127)).unwrap();
    let db = dir.join("midi-index.db");
    let mut store = MidiStore::open(&db).unwrap();
    run(&mut store, &dir, false);
    assert_eq!(count(&db, "notes"), 2, "two sounds");
    let all = store.all(&HashMap::new(), &HashMap::new()).unwrap();
    assert_eq!(all[0].hash, all[1].hash, "one set of notes for the page");
    for p in &all {
        let want = if p.clip == "Loud" { 127 } else { 100 };
        assert_eq!(store.notes(p.id, &p.hash).unwrap()[0].velocity7(), want);
    }
    assert_eq!(Catalogue::new(all).entries.len(), 1);
}

#[test]
fn away_folders_are_kept() {
    let stamps =
        vec![mo_midi::store::Stamp { path: r"E:\Packs\a.mid".into(), mtime: 1, size: 1, version: 1, failed: false }];
    assert!(indexer::plan(&[], &stamps, &[r"E:\Packs".into()], false).forget.is_empty());
    assert_eq!(indexer::plan(&[], &stamps, &[], false).forget, [r"E:\Packs\a.mid"]);
}
