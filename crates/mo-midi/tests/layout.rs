//! `midi-index.db` brought up to this app's layout (the notes in a table of their own) by copy, check, rename, and the
//! corrections made in the C# app taken in from its `midi.db` once, so it isn't opened again.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_formats::{NoteAttribute, NoteEvent};
use mo_midi::indexer::{self, Wanted};
use mo_midi::store::{self, MidiStore, bring_up_to_date};
use mo_midi::{Role, SourceKind};
use rusqlite::{Connection, OpenFlags, params};

/// The layout the Rust app made before this one (1): the notes in the last column of `notes`.
const LAYOUT_1: &str = "
    PRAGMA journal_mode = WAL;
    CREATE TABLE files (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE COLLATE NOCASE, kind INTEGER NOT NULL,
        mtime INTEGER NOT NULL, size INTEGER NOT NULL, version INTEGER NOT NULL, error TEXT);
    CREATE TABLE notes (id INTEGER PRIMARY KEY, sound TEXT NOT NULL UNIQUE, hash TEXT NOT NULL,
        count INTEGER NOT NULL, bars INTEGER NOT NULL, lowest INTEGER NOT NULL, highest INTEGER NOT NULL,
        chroma BLOB NOT NULL, features BLOB NOT NULL, data BLOB NOT NULL);
    CREATE INDEX ix_notes_hash ON notes(hash);
    CREATE TABLE parts (id INTEGER PRIMARY KEY, file INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
        notes INTEGER NOT NULL REFERENCES notes(id), part TEXT NOT NULL, clip TEXT NOT NULL, role INTEGER NOT NULL,
        why TEXT NOT NULL, drums INTEGER NOT NULL, program INTEGER, track_name TEXT NOT NULL, members TEXT);
    CREATE INDEX ix_parts_file ON parts(file);
    CREATE INDEX ix_parts_notes ON parts(notes);
    CREATE TABLE keys (notes INTEGER PRIMARY KEY REFERENCES notes(id) ON DELETE CASCADE, version INTEGER NOT NULL,
        role INTEGER NOT NULL, cut INTEGER NOT NULL, fact BLOB);
    CREATE INDEX ix_keys_version ON keys(version);
    CREATE TABLE your_roles (hash TEXT PRIMARY KEY, role INTEGER) WITHOUT ROWID;
    CREATE TABLE your_octaves (hash TEXT PRIMARY KEY, octaves INTEGER) WITHOUT ROWID;
    PRAGMA application_id = 1297042761;
    PRAGMA user_version = 1;";

/// The first layout (0): no `keys`, and a `harmony` table that was never filled.
const LAYOUT_0: &str = "
    PRAGMA journal_mode = WAL;
    CREATE TABLE files (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE COLLATE NOCASE, kind INTEGER NOT NULL,
        mtime INTEGER NOT NULL, size INTEGER NOT NULL, version INTEGER NOT NULL, error TEXT);
    CREATE TABLE notes (id INTEGER PRIMARY KEY, sound TEXT NOT NULL UNIQUE, hash TEXT NOT NULL,
        count INTEGER NOT NULL, bars INTEGER NOT NULL, lowest INTEGER NOT NULL, highest INTEGER NOT NULL,
        chroma BLOB NOT NULL, features BLOB NOT NULL, data BLOB NOT NULL);
    CREATE TABLE parts (id INTEGER PRIMARY KEY, file INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
        notes INTEGER NOT NULL REFERENCES notes(id), part TEXT NOT NULL, clip TEXT NOT NULL, role INTEGER NOT NULL,
        why TEXT NOT NULL, drums INTEGER NOT NULL, program INTEGER, track_name TEXT NOT NULL, members TEXT);
    CREATE TABLE harmony (hash TEXT PRIMARY KEY, version INTEGER NOT NULL, key_tonic INTEGER, key_mode INTEGER,
        confidence REAL, chords BLOB) WITHOUT ROWID;
    CREATE TABLE your_roles (hash TEXT PRIMARY KEY, role INTEGER) WITHOUT ROWID;
    CREATE TABLE your_octaves (hash TEXT PRIMARY KEY, octaves INTEGER) WITHOUT ROWID;
    PRAGMA application_id = 1297042761;";

/// The C# app's `midi.db`, as far as its corrections go.
const CS_MIDI_DB: &str = "
    CREATE TABLE sources (path TEXT PRIMARY KEY COLLATE NOCASE, kind TEXT NOT NULL, daw TEXT, mtime INTEGER,
        size INTEGER, tempo REAL, version INTEGER, error TEXT);
    CREATE TABLE phrases (id INTEGER PRIMARY KEY, source TEXT NOT NULL COLLATE NOCASE, part TEXT, clip TEXT,
        role TEXT, role_why TEXT, drums INTEGER, bars INTEGER, notes INTEGER, lowest INTEGER, highest INTEGER,
        key_tonic INTEGER, key_minor INTEGER, key_conf REAL, chroma BLOB, chords TEXT, features BLOB, hash TEXT,
        data BLOB, members TEXT, program INTEGER, track_name TEXT);
    CREATE TABLE your_roles (hash TEXT PRIMARY KEY, role TEXT NOT NULL);
    CREATE TABLE your_octaves (hash TEXT PRIMARY KEY, octaves INTEGER NOT NULL);";

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-midi-layout").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `count` notes going up from `key`, a beat each, at a 16-bit velocity.
fn notes(key: u8, count: usize) -> Vec<NoteEvent> {
    (0..count)
        .map(|i| NoteEvent {
            start: i as i64 * 96,
            length: 90,
            key: key + i as u8,
            velocity: 40_000 + i as u16,
            muted: false,
            step: false,
            attribute: NoteAttribute::NONE,
        })
        .collect()
}

/// An index of an older layout (`schema`), as the app made it: two files (one that couldn't be read), two sets of
/// notes, three parts (two copies of the first notes), a key read for the first notes when `keys`, and a kind and an
/// octave move you gave.
fn old_index(dir: &Path, schema: &str, keys: bool) -> PathBuf {
    let path = dir.join(store::FILE_NAME);
    let db = Connection::open(&path).unwrap();
    db.execute_batch(schema).unwrap();
    db.execute_batch(
        "INSERT INTO files VALUES (1, 'L:\\New Midi\\Keys\\Up.mid', 1, 1700000000, 120, 9, NULL);
         INSERT INTO files VALUES (2, 'C:\\Music\\Night\\Night_11.flp', 0, 1700000500, 4000, 30, NULL);
         INSERT INTO files VALUES (3, 'L:\\New Midi\\Broken.mid', 1, 1700000001, 10, 9, 'Not a MIDI file');",
    )
    .unwrap();
    let note = "INSERT INTO notes VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8, ?9)";
    for (id, sound, hash, list) in [(1, "S1", "HASH1", notes(60, 8)), (2, "S2", "HASH2", notes(36, 3))] {
        let (lowest, highest) = (list[0].key, list[list.len() - 1].key);
        let data = store::encode_notes(&list);
        db.execute(note, params![id, sound, hash, list.len() as i64, lowest, highest, [7u8; 12], [9u8; 48], data])
            .unwrap();
    }
    db.execute_batch(
        "INSERT INTO parts VALUES (1, 1, 1, 'Up', 'Up', 1, 'Single notes', 0, NULL, 'Keys', NULL);
         INSERT INTO parts VALUES (2, 2, 1, 'Lead', 'Lead #2', 1, 'Single notes', 0, 81, 'Lead', 'HASH1@0+768');
         INSERT INTO parts VALUES (3, 2, 2, 'Bass', 'Bass', 2, 'Low down', 0, 33, 'Bass', NULL);
         INSERT INTO your_roles VALUES ('HASH2', 0);
         INSERT INTO your_octaves VALUES ('HASH1', -1);",
    )
    .unwrap();
    if keys {
        db.execute("INSERT INTO keys VALUES (1, 503, 1, 0, ?1)", [[1u8, 2, 3, 4, 5, 6, 7, 8]]).unwrap();
    }
    path
}

/// Every row of `table` as text, in the order of its ids.
fn rows(path: &Path, table: &str, columns: &str) -> Vec<String> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut st = db
        .prepare(&format!("SELECT quote({}) FROM {table} ORDER BY 1", columns.replace(',', ") || ',' || quote(")))
        .unwrap();
    st.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
}

fn layout(path: &Path) -> i64 {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    db.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap()
}

fn columns(path: &Path, table: &str) -> Vec<String> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut st = db.prepare(&format!("SELECT name FROM pragma_table_info('{table}')")).unwrap();
    st.query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
}

/// Files in `dir` whose name starts with `start`.
fn named(dir: &Path, start: &str) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(start))
        .collect()
}

/// A `midi.db` beside the index, as the C# app made it, holding `roles` (by name) and `octaves`.
fn cs_midi_db(dir: &Path, roles: &[(&str, &str)], octaves: &[(&str, i64)]) -> PathBuf {
    let path = dir.join("midi.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch(CS_MIDI_DB).unwrap();
    for (hash, role) in roles {
        db.execute("INSERT INTO your_roles VALUES (?1, ?2)", params![hash, role]).unwrap();
    }
    for (hash, o) in octaves {
        db.execute("INSERT INTO your_octaves VALUES (?1, ?2)", params![hash, o]).unwrap();
    }
    path
}

#[test]
fn an_index_of_the_older_layout_is_copied_into_this_one_with_every_row_kept() {
    let dir = folder("copied");
    let path = old_index(&dir, LAYOUT_1, true);
    let files = rows(&path, "files", "id,path,kind,mtime,size,version,error");
    let parts = rows(&path, "parts", "id,file,notes,part,clip,role,why,drums,program,track_name,members");
    let facts = rows(&path, "notes", "id,sound,hash,count,bars,lowest,highest,chroma,features");
    let data = rows(&path, "notes", "id,data");
    let kept = (rows(&path, "your_roles", "hash,role"), rows(&path, "your_octaves", "hash,octaves"));

    let up = bring_up_to_date(&path);
    assert!(up.later.is_empty(), "{up:?}");
    assert!(
        up.done[0].starts_with("Copied the MIDI index into its new layout")
            && up.done[0].contains("2 sets of notes, 3 parts"),
        "{up:?}"
    );
    // No midi.db: nothing to take in yet.
    assert_eq!(up.done.len(), 1, "{up:?}");
    assert!(up.old_roles.is_empty() && up.old_octaves.is_empty());

    assert_eq!(layout(&path), store::LAYOUT);
    assert!(!columns(&path, "notes").contains(&"data".to_owned()), "the notes have a table of their own");
    assert_eq!(rows(&path, "files", "id,path,kind,mtime,size,version,error"), files);
    assert_eq!(rows(&path, "parts", "id,file,notes,part,clip,role,why,drums,program,track_name,members"), parts);
    assert_eq!(rows(&path, "notes", "id,sound,hash,count,bars,lowest,highest,chroma,features"), facts);
    assert_eq!(rows(&path, "note_data", "notes,data"), data);
    assert_eq!((rows(&path, "your_roles", "hash,role"), rows(&path, "your_octaves", "hash,octaves")), kept);
    // The key read is kept; the notes without one get "not read yet".
    assert_eq!(rows(&path, "keys", "notes,version"), ["1,503", "2,0"]);
    assert!(named(&dir, "midi-index.db.new-").is_empty(), "the copy has the index's name");

    // The page lists it as before, and the roll reads the notes from their own table.
    let page = MidiStore::open_read_only(&path).unwrap();
    let list = page.all(&HashMap::new(), &HashMap::new()).unwrap();
    assert_eq!(list.len(), 3);
    let lead = list.iter().find(|p| p.clip == "Lead #2").unwrap();
    assert_eq!((lead.your_octave, lead.program, lead.members.as_str()), (Some(-1), Some(81), "HASH1@0+768"));
    assert_eq!(page.notes(lead.id, &lead.hash).unwrap(), notes(60, 8), "16-bit velocities and all");
    let bass = list.iter().find(|p| p.clip == "Bass").unwrap();
    assert_eq!(bass.role(), Role::Chords, "your kind for it");
    drop(page);
    // The indexer goes on writing it: the notes with no key read are read.
    let store = MidiStore::open(&path).unwrap();
    let stale = store.stale_keys(10).unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!((stale[0].notes_id, stale[0].notes.clone()), (2, notes(36, 3)));
    drop(store);

    // Once done, nothing more to do.
    let again = bring_up_to_date(&path);
    assert!(again.done.is_empty() && again.later.is_empty(), "{again:?}");
    // A midi.db that turns up (the data folder put back from a backup) is taken in then, once.
    cs_midi_db(&dir, &[], &[]);
    let late = bring_up_to_date(&path);
    assert_eq!(late.done, ["The old app's midi.db has no corrections to take in; it isn't opened again"]);
    assert!(bring_up_to_date(&path).done.is_empty());
}

#[test]
fn an_index_from_before_keys_were_read_gets_them_read() {
    let dir = folder("layout-0");
    let path = old_index(&dir, LAYOUT_0, false);
    let up = bring_up_to_date(&path);
    assert!(up.later.is_empty(), "{up:?}");
    assert_eq!(layout(&path), store::LAYOUT);
    assert_eq!(rows(&path, "keys", "notes,version"), ["1,0", "2,0"]);
    assert_eq!(rows(&path, "note_data", "notes").len(), 2);
}

#[test]
fn an_index_open_elsewhere_is_read_as_it_is_and_copied_the_next_time() {
    let dir = folder("open-elsewhere");
    let path = old_index(&dir, LAYOUT_1, true);
    cs_midi_db(&dir, &[("HASH1", "Arp")], &[]);
    // Another copy of the app reading it.
    let other = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let n: i64 = other.query_row("SELECT COUNT(*) FROM parts", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 3);

    let up = bring_up_to_date(&path);
    assert!(up.done.is_empty(), "{up:?}");
    assert!(up.later.iter().any(|l| l.contains("another copy of the app has it open")), "{up:?}");
    assert_eq!(layout(&path), 1, "left as it was");
    assert!(named(&dir, "midi-index.db.new-").is_empty(), "the copy is thrown away");
    // Your corrections from the old app still count, read as they were.
    assert_eq!(up.old_roles.get("HASH1"), Some(&Role::Arp));

    // The page reads it as it is meanwhile, notes and all.
    let page = MidiStore::open_read_only(&path).unwrap();
    let list = page.all(&up.old_roles, &up.old_octaves).unwrap();
    let lead = list.iter().find(|p| p.clip == "Lead #2").unwrap();
    assert_eq!(lead.role(), Role::Arp);
    assert_eq!(page.notes(lead.id, &lead.hash).unwrap(), notes(60, 8));
    drop(page);
    // Reading your MIDI waits for the copy.
    let why = MidiStore::open(&path).err().unwrap().to_string();
    assert!(why.contains("tried again the next time"), "{why}");

    drop(other);
    let up = bring_up_to_date(&path);
    assert!(up.later.is_empty(), "{up:?}");
    assert_eq!(up.done.len(), 2, "copied, and the old app's corrections taken in: {up:?}");
    assert_eq!(layout(&path), store::LAYOUT);
    assert!(MidiStore::open(&path).is_ok());
}

/// Runs the indexer over every `.mid` file in `dir`.
fn index(store: &mut MidiStore, dir: &Path) {
    let mut wanted = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let meta = e.metadata().unwrap();
        if e.path().extension().is_some_and(|x| x == "mid") {
            wanted.push(Wanted {
                path: e.path().to_string_lossy().into_owned(),
                kind: SourceKind::File,
                mtime: mo_library::scan::mtime(&meta),
                size: meta.len() as i64,
            });
        }
    }
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let plan = indexer::plan(&wanted, &store.stamps().unwrap(), &[], false);
    let folders = vec![dir.to_string_lossy().into_owned()];
    indexer::run(store, &plan, &folders, &pool, 2, &AtomicBool::new(false), &|_, _, _| {}).unwrap();
}

/// A Standard MIDI File of `keys`, a beat each.
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

#[test]
fn the_old_apps_corrections_are_taken_in_once_and_midi_db_isnt_opened_again() {
    let dir = folder("corrections");
    std::fs::write(dir.join("Tune.mid"), smf(&[72, 74, 76, 79, 76, 74, 72, 71])).unwrap();
    std::fs::write(dir.join("Low.mid"), smf(&[36, 36, 43, 41, 36, 36, 43, 41])).unwrap();
    let path = dir.join(store::FILE_NAME);
    let mut store = MidiStore::open(&path).unwrap();
    index(&mut store, &dir);
    let list = store.all(&HashMap::new(), &HashMap::new()).unwrap();
    let hash = |clip: &str| list.iter().find(|p| p.clip == clip).unwrap().hash.clone();
    let (tune, low) = (hash("Tune"), hash("Low"));
    // Said in this app since: it wins over the old app's.
    store.set_your_role(&tune, Some(Role::Chords)).unwrap();
    drop(store);
    cs_midi_db(&dir, &[(tune.as_str(), "Arp"), (low.as_str(), "Melody"), ("GONE", "Drums")], &[(low.as_str(), 2)]);

    let up = bring_up_to_date(&path);
    assert!(up.later.is_empty(), "{up:?}");
    assert_eq!(up.done.len(), 1, "{up:?}");
    assert_eq!(
        up.done[0],
        "Took in 2 kinds and 1 octave move you gave parts in the old app, from midi.db, which isn't opened again \
         (1 was given in this app since, which stays)"
    );
    let check = || {
        let page = MidiStore::open_read_only(&path).unwrap();
        let list = page.all(&HashMap::new(), &HashMap::new()).unwrap();
        let part = |clip: &str| list.iter().find(|p| p.clip == clip).unwrap().clone();
        assert_eq!(part("Tune").your_role, Some(Role::Chords));
        assert_eq!((part("Low").your_role, part("Low").your_octave), (Some(Role::Melody), Some(2)));
    };
    check();

    // midi.db isn't opened again: one that can't be read is no matter now.
    std::fs::write(dir.join("midi.db"), b"not a database").unwrap();
    let again = bring_up_to_date(&path);
    assert!(again.done.is_empty() && again.later.is_empty(), "{again:?}");
    check();
}

#[test]
fn a_midi_db_that_cant_be_read_is_tried_again_and_the_index_still_lists() {
    let dir = folder("unreadable-midi-db");
    let path = dir.join(store::FILE_NAME);
    std::fs::write(dir.join("Tune.mid"), smf(&[72, 74, 76, 79])).unwrap();
    let mut store = MidiStore::open(&path).unwrap();
    index(&mut store, &dir);
    drop(store);
    std::fs::write(dir.join("midi.db"), b"not a database").unwrap();
    let up = bring_up_to_date(&path);
    assert!(up.done.is_empty(), "{up:?}");
    assert!(up.later.iter().any(|l| l.contains("couldn't be read from midi.db")), "{up:?}");
    assert_eq!(MidiStore::open_read_only(&path).unwrap().all(&HashMap::new(), &HashMap::new()).unwrap().len(), 1);
    // Readable again (put back): taken in then.
    std::fs::remove_file(dir.join("midi.db")).unwrap();
    cs_midi_db(&dir, &[], &[]);
    assert_eq!(bring_up_to_date(&path).done.len(), 1);
}

#[test]
fn an_index_of_a_newer_layout_is_left_as_it_is() {
    let dir = folder("newer");
    let path = dir.join(store::FILE_NAME);
    drop(MidiStore::open(&path).unwrap());
    Connection::open(&path).unwrap().pragma_update(None, "user_version", store::LAYOUT + 1).unwrap();
    let why = MidiStore::open(&path).err().unwrap().to_string();
    assert!(why.contains("a newer version of the app made it"), "{why}");
    assert_eq!(layout(&path), store::LAYOUT + 1);
}

#[test]
fn no_index_yet_is_nothing_to_do() {
    let dir = folder("none");
    cs_midi_db(&dir, &[("HASH1", "Arp")], &[]);
    let up = bring_up_to_date(&dir.join(store::FILE_NAME));
    assert!(up.done.is_empty() && up.later.is_empty() && up.old_roles.is_empty(), "{up:?}");
    assert!(!dir.join(store::FILE_NAME).exists(), "the first reading of your MIDI makes it");
}
