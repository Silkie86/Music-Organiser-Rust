//! The indexer filling `midi-index.db` from `.mid` files: only what changed is read, each set of notes is stored once,
//! gone files are forgotten, failed ones listed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_midi::indexer::{self, Wanted};
use mo_midi::store::MidiStore;
use mo_midi::{Catalogue, InKeys, Role, SourceKind};

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
    assert_eq!(count(&db, "note_data"), 2, "each set of notes once, in a table of its own");
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
    assert_eq!((count(&db, "note_data"), count(&db, "keys")), (1, 1), "the notes themselves and their key go too");

    // A copy gone: the notes stay, the other file still has them.
    std::fs::remove_file(pack.join("Low 01 copy.mid")).unwrap();
    run(&mut store, &dir, false);
    assert_eq!((count(&db, "notes"), count(&db, "parts")), (1, 1));
}

/// Sets the time a file was last changed, in seconds after 1970.
fn date(path: &Path, seconds: u64) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds)).unwrap();
}

#[test]
fn a_date_still_to_come_goes_after_the_real_ones() {
    let dir = folder("mo-midi-future-dates");
    let pack = dir.join("Pack");
    // Two files saved in 2019 and 2024; a pack whose dates are still to come (as one of Solomon's says 31 Dec 2027),
    // one with the 2019 file's notes; and a file with no date, which reads as 1970.
    let files: [(PathBuf, &[u8], u64); 5] = [
        (dir.join("Old.mid"), BASS, 1_559_390_400),
        (dir.join("Newer.mid"), TUNE, 1_709_294_400),
        (pack.join("Old again.mid"), BASS, 4_102_401_600),
        (pack.join("Fill.mid"), &[60, 64, 67, 72, 67, 64, 60, 55], 4_328_424_000),
        (pack.join("No date.mid"), &[40, 40, 40, 52, 40, 40, 40, 52], 0),
    ];
    for (path, keys, seconds) in &files {
        std::fs::write(path, smf(keys)).unwrap();
        date(path, *seconds);
    }
    let name = |path: &str| mo_formats::paths::file_name(path).to_owned();
    let in_order = ["Newer.mid", "Old.mid", "Fill.mid", "Old again.mid", "No date.mid"];
    let mut store = MidiStore::open(&dir.join("midi-index.db")).unwrap();

    // Read newest first, as the list shows them: the dates that aren't real after the rest.
    let plan = indexer::plan(&wanted(&dir), &store.stamps().unwrap(), &[], false);
    assert_eq!(plan.todo.iter().map(|w| name(&w.path)).collect::<Vec<_>>(), in_order);
    run(&mut store, &dir, false);
    let all = store.all(&HashMap::new(), &HashMap::new()).unwrap();
    assert_eq!(all.iter().map(|p| name(&p.source.path)).collect::<Vec<_>>(), in_order);

    // The copy with a real date stands for its notes, and the pack's copy is one of its other files.
    let cat = Catalogue::new(all);
    let lines: Vec<String> = cat.entries.iter().map(|e| name(&e.phrase.source.path)).collect();
    assert_eq!(lines, ["Newer.mid", "Old.mid", "Fill.mid", "No date.mid"]);
    assert_eq!(cat.entries[1].other_sources.iter().map(|s| name(&s.path)).collect::<Vec<_>>(), ["Old again.mid"]);
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
fn octaves_chosen_on_the_page_are_kept_in_an_index_thats_there_and_never_make_one() {
    let dir = folder("mo-midi-octaves-kept");
    std::fs::write(dir.join("Tune.mid"), smf(TUNE)).unwrap();
    let db = dir.join("midi-index.db");
    let mut store = MidiStore::open(&db).unwrap();
    run(&mut store, &dir, false);
    let hash = store.all(&HashMap::new(), &HashMap::new()).unwrap()[0].hash.clone();
    let kept = |store: &MidiStore| store.all(&HashMap::new(), &HashMap::new()).unwrap()[0].your_octave;

    // Several in one go, while the indexer has the index open too: the last said for the same notes stands, and
    // notes the index doesn't have are kept as well (they may be read later).
    MidiStore::keep_your_octaves(&db, &[(&hash, Some(1)), ("other notes", Some(3)), (&hash, Some(-2))]).unwrap();
    assert_eq!(kept(&store), Some(-2));
    // None: the app picks again.
    MidiStore::keep_your_octaves(&db, &[(&hash, None)]).unwrap();
    assert_eq!(kept(&store), None);
    MidiStore::keep_your_octaves(&db, &[]).unwrap();

    // An index that's gone isn't made again, nor its folder.
    let gone = dir.join("Gone").join("midi-index.db");
    assert!(MidiStore::keep_your_octaves(&gone, &[(&hash, Some(1))]).is_err());
    assert!(!gone.exists() && !dir.join("Gone").exists());
    let away = dir.join("midi-index-away.db");
    assert!(MidiStore::keep_your_octaves(&away, &[(&hash, Some(1))]).is_err());
    assert!(!away.exists());
    // Another program's database is left as it is.
    let other = dir.join("other.db");
    let conn = rusqlite::Connection::open(&other).unwrap();
    conn.execute_batch("CREATE TABLE your_octaves (hash TEXT PRIMARY KEY, octaves INTEGER);").unwrap();
    drop(conn);
    assert!(MidiStore::keep_your_octaves(&other, &[(&hash, Some(1))]).is_err());
    let conn = rusqlite::Connection::open(&other).unwrap();
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM your_octaves", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    // One made by a newer version of the app too.
    drop(store);
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.pragma_update(None, "user_version", mo_midi::store::LAYOUT + 1).unwrap();
    drop(conn);
    assert!(MidiStore::keep_your_octaves(&db, &[(&hash, Some(2))]).is_err());
    let conn = rusqlite::Connection::open(&db).unwrap();
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM your_octaves WHERE octaves = 2", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 0);
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

#[test]
fn keeps_the_key_and_reads_it_again_when_the_finder_gets_better() {
    let dir = folder("mo-midi-keys");
    // A tune around C major, twice through.
    let tune: Vec<u8> = [60, 64, 67, 72, 71, 67, 65, 64, 62, 59, 60, 67].repeat(2);
    std::fs::write(dir.join("Tune.mid"), smf(&tune)).unwrap();
    std::fs::write(dir.join("Pack").join("Basslines").join("Bass.mid"), smf(BASS)).unwrap();
    let db = dir.join("midi-index.db");
    let mut store = MidiStore::open(&db).unwrap();
    let first = run(&mut store, &dir, false);
    assert_eq!(first.rekeyed, 0, "read with this finder already");
    let all = store.all(&HashMap::new(), &HashMap::new()).unwrap();
    let tune_part = all.iter().find(|p| p.clip == "Tune").unwrap();
    let fact = tune_part.key_fact.expect("the reading is kept");
    // It leans to C major, whether or not the finder is sure enough to give it as the key (a tune alone seldom is).
    let leaning = fact.answer().and_then(|answer| answer.leaning_key());
    assert_eq!(leaning.map(|k| (k.tonic.number(), k.mode)), Some((0, mo_harmony::Mode::Major)), "{fact:?}");
    assert!(all.iter().all(|p| p.key_fact.is_some()));
    // Whole parts, heard as loops, as their keys were read: the card's chart reads them the same way.
    assert!(all.iter().all(|p| p.cut == Some(false)));

    // An older finder's readings: read again from the stored notes, to the same answer, without opening the files.
    let before: Vec<_> = all.iter().map(|p| (p.key_fact, p.features.clone())).collect();
    rusqlite::Connection::open(&db).unwrap().execute("UPDATE keys SET version = 1, fact = NULL", []).unwrap();
    std::fs::remove_file(dir.join("Tune.mid")).unwrap();
    std::fs::write(dir.join("Tune.mid"), b"gone").unwrap();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let again =
        indexer::run(&mut store, &indexer::Plan::default(), &[], &pool, 2, &AtomicBool::new(false), &|_, _, _| {})
            .unwrap();
    assert_eq!(again.rekeyed, 2);
    let after: Vec<_> =
        store.all(&HashMap::new(), &HashMap::new()).unwrap().iter().map(|p| (p.key_fact, p.features.clone())).collect();
    assert_eq!(after, before);
}

#[test]
fn an_index_from_before_keys_gets_them() {
    let dir = folder("mo-midi-upgrade");
    std::fs::write(dir.join("Tune.mid"), smf(TUNE)).unwrap();
    let db = dir.join("midi-index.db");
    let mut store = MidiStore::open(&db).unwrap();
    run(&mut store, &dir, false);
    drop(store);
    // As the first indexes were made: no keys table, and the notes in the last column of `notes`.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "ALTER TABLE notes ADD COLUMN data BLOB NOT NULL DEFAULT x'';
         UPDATE notes SET data = (SELECT d.data FROM note_data d WHERE d.notes = notes.id);
         DROP TABLE note_data; DROP TABLE keys; DROP TABLE about; PRAGMA user_version = 0;",
    )
    .unwrap();
    drop(conn);
    // The page can list it before the indexer has opened it, and show its notes.
    let page = MidiStore::open_read_only(&db).unwrap();
    let listed = page.all(&HashMap::new(), &HashMap::new()).unwrap();
    assert_eq!(listed[0].key_fact, None);
    assert_eq!(page.notes(listed[0].id, &listed[0].hash).unwrap().len(), TUNE.len());
    drop(page);
    // The indexer copies it into this app's layout first.
    let mut store = MidiStore::open(&db).unwrap();
    assert_eq!(count(&db, "note_data"), 1);
    assert_eq!(store.all(&HashMap::new(), &HashMap::new()).unwrap()[0].key_fact, None);
    assert_eq!(run(&mut store, &dir, false).rekeyed, 1);
    assert!(store.all(&HashMap::new(), &HashMap::new()).unwrap()[0].key_fact.is_some());
}

#[test]
fn the_same_loop_in_other_keys_is_one_set_and_old_notes_get_their_shape() {
    let dir = folder("mo-midi-indexer-keys");
    let pack = dir.join("Pack").join("Basslines");
    let up: Vec<u8> = BASS.iter().map(|k| k + 5).collect();
    let octave: Vec<u8> = BASS.iter().map(|k| k + 12).collect();
    std::fs::write(pack.join("Low C.mid"), smf(BASS)).unwrap();
    std::fs::write(pack.join("Low F.mid"), smf(&up)).unwrap();
    std::fs::write(dir.join("Tune.mid"), smf(TUNE)).unwrap();
    // The same shape in another folder: not the same pack's loop.
    let g: Vec<u8> = BASS.iter().map(|k| k + 7).collect();
    std::fs::write(dir.join("Low G.mid"), smf(&g)).unwrap();
    let db = dir.join("midi-index.db");
    let mut store = MidiStore::open(&db).unwrap();
    assert_eq!(run(&mut store, &dir, false).shaped, 0, "new notes get their shape as they're saved");
    assert_eq!(count(&db, "shapes"), 4);

    let catalogue = Catalogue::new(store.all(&HashMap::new(), &HashMap::new()).unwrap());
    assert_eq!(catalogue.entries.len(), 4, "different notes, so four parts");
    assert_eq!(catalogue.line_count, 3, "the pack's loop in two keys is one line");
    let low: Vec<_> =
        catalogue.entries.iter().filter(|e| e.phrase.clip == "Low C" || e.phrase.clip == "Low F").collect();
    assert_eq!(low.len(), 2);
    assert!(low.iter().all(|e| e.in_keys == Some(InKeys::Keys(2)) && e.key_group.is_some()));
    assert_eq!(low[0].key_group, low[1].key_group);
    for clip in ["Tune", "Low G"] {
        let e = catalogue.entries.iter().find(|e| e.phrase.clip == clip).unwrap();
        assert!(e.in_keys.is_none() && e.key_group.is_none(), "{clip}");
    }

    // Notes saved before shapes were kept get theirs at the next read, from the notes in the index.
    rusqlite::Connection::open(&db).unwrap().execute("DELETE FROM shapes", []).unwrap();
    let again = run(&mut store, &dir, false);
    assert_eq!((again.read, again.shaped), (0, 4));
    assert_eq!(run(&mut store, &dir, false).shaped, 0, "worked out once");

    // Only an octave up: the same key, so "in 2 octaves".
    std::fs::remove_file(pack.join("Low F.mid")).unwrap();
    std::fs::write(pack.join("Low C high.mid"), smf(&octave)).unwrap();
    run(&mut store, &dir, false);
    let catalogue = Catalogue::new(store.all(&HashMap::new(), &HashMap::new()).unwrap());
    let low = catalogue.entries.iter().find(|e| e.phrase.clip == "Low C").unwrap();
    assert_eq!(low.in_keys, Some(InKeys::Octaves(2)));
}

#[test]
fn a_file_away_mid_reading_keeps_its_parts_and_is_read_again() {
    let dir = folder("mo-midi-indexer-away");
    let file = dir.join("Pack").join("Basslines").join("Low 01.mid");
    std::fs::write(&file, smf(BASS)).unwrap();
    let db = dir.join("midi-index.db");
    let mut store = MidiStore::open(&db).unwrap();
    run(&mut store, &dir, false);
    assert_eq!(count(&db, "parts"), 1);

    // It changes, the reading is planned, and then the drive drops before it's read.
    // Longer, so its size says it changed even within the same second.
    std::fs::write(&file, smf(&[TUNE, TUNE].concat())).unwrap();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let plan = indexer::plan(&wanted(&dir), &store.stamps().unwrap(), &[], false);
    assert_eq!(plan.todo.len(), 1);
    std::fs::rename(&file, dir.join("away.tmp")).unwrap();
    let folders = vec![dir.to_string_lossy().into_owned()];
    indexer::run(&mut store, &plan, &folders, &pool, 2, &AtomicBool::new(false), &|_, _, _| {}).unwrap();
    assert_eq!(count(&db, "parts"), 1, "what was read before is kept");

    // Back again: a plain reading (not Read again) reads it.
    std::fs::rename(dir.join("away.tmp"), &file).unwrap();
    let again = run(&mut store, &dir, false);
    assert_eq!(again.read, 1);
    assert!(store.failures().unwrap().is_empty());
}

#[test]
fn every_set_of_notes_gets_its_slots_for_a_search() {
    let dir = folder("mo-midi-indexer-slots");
    std::fs::write(dir.join("Pack").join("Basslines").join("Low 01.mid"), smf(BASS)).unwrap();
    std::fs::write(dir.join("Tune.mid"), smf(TUNE)).unwrap();
    // The same notes played softer: another set of notes, the same slots.
    std::fs::write(dir.join("Tune soft.mid"), smf_at(TUNE, 40)).unwrap();
    let db = dir.join("midi-index.db");
    let mut store = MidiStore::open(&db).unwrap();
    assert_eq!(run(&mut store, &dir, false).slots, 0, "new notes get their slots as they're saved");
    assert_eq!(count(&db, "slots"), 3);

    // A search's book: each part's slots by its note hash. Eight quarter notes are two bars, four slots, each with
    // two notes half a slot long.
    let all = store.all(&HashMap::new(), &HashMap::new()).unwrap();
    let book = store.slots().unwrap();
    assert_eq!(book.len(), 2, "the soft copy shares the tune's hash");
    let tune = all.iter().find(|p| p.clip == "Tune").unwrap();
    let slots = book.of(&tune.hash).unwrap();
    assert_eq!(slots.len(), 4);
    let half = mo_harmony::SLOT_FULL / 2;
    // C and D, then E and G.
    assert_eq!((slots[0][0], slots[0][2], slots[0][4]), (half, half, 0));
    assert_eq!((slots[1][4], slots[1][7], slots[1][0]), (half, half, 0));
    assert_eq!(slots, mo_midi::fit::slots_of(&store.notes(tune.id, &tune.hash).unwrap()).as_slice());
    // The page's own connection reads the same.
    assert_eq!(MidiStore::open_read_only(&db).unwrap().slots().unwrap().len(), 2);

    // Notes saved before slots were kept get theirs at the next read, from the notes in the index.
    let raw = rusqlite::Connection::open(&db).unwrap();
    raw.execute("DELETE FROM slots", []).unwrap();
    assert!(store.slots().unwrap().is_empty());
    let again = run(&mut store, &dir, false);
    assert_eq!((again.read, again.slots), (0, 3));
    assert_eq!(run(&mut store, &dir, false).slots, 0, "worked out once");
    assert_eq!(store.slots().unwrap().of(&tune.hash), Some(slots));

    // Slots an older version made aren't searched by, and are made again; a damaged row is left out.
    raw.execute("UPDATE slots SET version = 0, data = x'0100FF'", []).unwrap();
    assert!(store.slots().unwrap().is_empty());
    assert_eq!(run(&mut store, &dir, false).slots, 3);
    assert_eq!(store.slots().unwrap().of(&tune.hash), Some(slots));
    raw.execute("UPDATE slots SET data = x'0100'", []).unwrap();
    assert!(store.slots().unwrap().is_empty());
    // So is a row that isn't bytes at all: it's never taken for a part with no notes in it.
    raw.execute("UPDATE slots SET data = 'words'", []).unwrap();
    assert!(store.slots().unwrap().is_empty());
    // Reading a file again puts its damaged row right; the others' stay as they are until theirs is read.
    date(&dir.join("Tune.mid"), 1_600_000_000);
    assert_eq!(run(&mut store, &dir, false).read, 1);
    let book = store.slots().unwrap();
    assert_eq!((book.len(), book.of(&tune.hash)), (1, Some(slots)));
    raw.execute("UPDATE slots SET version = 0", []).unwrap();
    assert_eq!(run(&mut store, &dir, false).slots, 3);

    // Notes no file has any more take their slots with them.
    std::fs::remove_file(dir.join("Pack").join("Basslines").join("Low 01.mid")).unwrap();
    run(&mut store, &dir, false);
    assert_eq!(count(&db, "slots"), 2);

    // An index from before slots were kept, read by the page before the indexer has opened it: an empty book.
    drop(store);
    raw.execute("DROP TABLE slots", []).unwrap();
    let page = MidiStore::open_read_only(&db).unwrap();
    assert!(page.slots().unwrap().is_empty());
    // And the indexer gives it the table and fills it: the page's index, open since before, has them then.
    let mut store = MidiStore::open(&db).unwrap();
    assert_eq!(run(&mut store, &dir, false).slots, 2);
    assert_eq!(page.slots().unwrap().len(), 1);
}

#[test]
fn a_search_finds_the_part_that_fits_through_the_index() {
    use mo_formats::{NoteAttribute, NoteEvent};
    use mo_midi::fit::{Candidate, Target};

    let dir = folder("mo-midi-indexer-search");
    // Over A minor, D minor, G and C, a bar each: a tune on each chord's notes, a note a beat.
    let fits: &[u8] = &[69, 72, 76, 72, 74, 77, 81, 77, 67, 71, 74, 71, 72, 76, 79, 76];
    let fourth_up: Vec<u8> = fits.iter().map(|k| k + 5).collect();
    let climbing: Vec<u8> = (60..76).collect();
    std::fs::write(dir.join("Fits.mid"), smf(fits)).unwrap();
    std::fs::write(dir.join("Fits in F.mid"), smf(&fourth_up)).unwrap();
    std::fs::write(dir.join("Climbing.mid"), smf(&climbing)).unwrap();
    std::fs::write(dir.join("Pack").join("Basslines").join("Low 01.mid"), smf(BASS)).unwrap();
    let db = dir.join("midi-index.db");
    let mut store = MidiStore::open(&db).unwrap();
    run(&mut store, &dir, false);

    let chords: Vec<NoteEvent> = [[57u8, 60, 64], [50, 62, 65], [55, 59, 62], [48, 60, 64]]
        .iter()
        .zip(0i64..)
        .flat_map(|(keys, bar)| {
            keys.iter().map(move |&key| NoteEvent {
                start: bar * mo_midi::BAR,
                length: mo_midi::BAR,
                key,
                velocity: 40_000,
                muted: false,
                step: false,
                attribute: NoteAttribute::default(),
            })
        })
        .collect();
    let grid = mo_harmony::Grid::four_four(mo_midi::PPQ as u32);
    let target = Target::chords(&[(Role::Chords, &chords)], grid, None).unwrap();

    // As the page would: its list, the index's slots, and notes read from the index for the close look alone.
    let all = store.all(&HashMap::new(), &HashMap::new()).unwrap();
    let candidates: Vec<Candidate> = all.iter().map(Candidate::of).collect();
    let book = store.slots().unwrap();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let mut read = 0;
    let mut notes = |id: i64, hash: &str| {
        read += 1;
        store.notes(id, hash).ok()
    };
    let search = target.search(&candidates, &book, &mut notes, 10, &pool, &AtomicBool::new(false));
    // The tune and its copy in F are one loop: one of them is read.
    assert_eq!((search.looked, search.close, search.not_ready, search.stopped), (4, 3, 0, false));
    assert_eq!((read, search.unread), (3, 0));
    // The tune on the chords' notes first, as it is; the same tune in F isn't listed beside it.
    let clips: Vec<&str> = search.found.iter().map(|f| all[f.at].clip.as_str()).collect();
    assert_eq!(clips[0], "Fits");
    assert_eq!(search.found[0].sits.best().0, 0);
    assert!(!clips.contains(&"Fits in F"), "{clips:?}");
    // The one climbing by half steps sits well below it.
    let climbing = search.found.iter().find(|f| all[f.at].clip == "Climbing").unwrap();
    assert!(climbing.sits.best().1 < search.found[0].sits.best().1 - 0.3);
}
