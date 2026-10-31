//! Reading a `midi.db` made as the C# app makes it (its `MidiIndex` schema), without changing it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use mo_midi::{Catalogue, MidiError, MidiIndex, PhraseChord, Role, SourceKind};
use rusqlite::{Connection, params};

/// The C# app's schema, as `MidiIndex.Schema` writes it today.
const SCHEMA: &str = "
    CREATE TABLE sources (
        path TEXT PRIMARY KEY COLLATE NOCASE, kind TEXT NOT NULL, daw TEXT, mtime INTEGER, size INTEGER,
        tempo REAL, version INTEGER, error TEXT);
    CREATE TABLE phrases (
        id INTEGER PRIMARY KEY,
        source TEXT NOT NULL COLLATE NOCASE REFERENCES sources(path) ON DELETE CASCADE,
        part TEXT, clip TEXT, role TEXT, role_why TEXT, drums INTEGER,
        bars INTEGER, notes INTEGER, lowest INTEGER, highest INTEGER,
        key_tonic INTEGER, key_minor INTEGER, key_conf REAL,
        chroma BLOB, chords TEXT, features BLOB, hash TEXT, data BLOB, members TEXT, program INTEGER, track_name TEXT);
    CREATE TABLE your_roles (hash TEXT PRIMARY KEY, role TEXT NOT NULL);
    CREATE TABLE clamp (hash TEXT PRIMARY KEY, model TEXT NOT NULL, vec BLOB NOT NULL);
    CREATE TABLE your_octaves (hash TEXT PRIMARY KEY, octaves INTEGER NOT NULL);";

/// The older schema, before `members`, `program`, `track_name` and the octaves table.
const OLD_SCHEMA: &str = "
    CREATE TABLE sources (path TEXT PRIMARY KEY COLLATE NOCASE, kind TEXT NOT NULL, daw TEXT, mtime INTEGER,
        size INTEGER, tempo REAL, version INTEGER, error TEXT);
    CREATE TABLE phrases (id INTEGER PRIMARY KEY, source TEXT NOT NULL COLLATE NOCASE, part TEXT, clip TEXT,
        role TEXT, role_why TEXT, drums INTEGER, bars INTEGER, notes INTEGER, lowest INTEGER, highest INTEGER,
        key_tonic INTEGER, key_minor INTEGER, key_conf REAL, chroma BLOB, chords TEXT, features BLOB, hash TEXT,
        data BLOB);";

fn notes(list: &[(i32, i32, u8, u8)]) -> Vec<u8> {
    let mut data = Vec::new();
    for (start, length, key, vel) in list {
        data.extend(start.to_le_bytes());
        data.extend(length.to_le_bytes());
        data.extend([*key, *vel]);
    }
    data
}

fn make(name: &str, schema: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-midi");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    let db = Connection::open(&path).unwrap();
    db.execute_batch(schema).unwrap();
    path
}

fn full(name: &str) -> PathBuf {
    let path = make(name, SCHEMA);
    let db = Connection::open(&path).unwrap();
    let source = "INSERT INTO sources (path, kind, daw, mtime, size, tempo, version, error) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";
    db.execute(source, params![r"C:\P\Song\Song_2.flp", "project", "FL Studio", 200, 10, 140.0, 901, None::<String>])
        .unwrap();
    db.execute(source, params![r"C:\P\Song\Song_1.flp", "project", "FL Studio", 100, 10, 140.0, 901, None::<String>])
        .unwrap();
    db.execute(source, params![r"D:\Packs\Keys\Dark.mid", "file", "MIDI", 150, 10, None::<f64>, 10, None::<String>])
        .unwrap();
    db.execute(source, params![r"D:\Packs\zz broken.mid", "file", "MIDI", 1, 1, None::<f64>, 10, "It looks cut short"])
        .unwrap();
    db.execute(source, params![r"D:\Packs\A locked.mid", "file", "MIDI", -1, -1, None::<f64>, 10, "In use"]).unwrap();
    let phrase = "INSERT INTO phrases (id, source, part, clip, role, role_why, drums, bars, notes, lowest, highest,
                  key_tonic, key_minor, key_conf, chroma, chords, features, hash, data, members, program, track_name)
                  VALUES (?1, ?2, ?3, ?4, ?5, 'why', ?6, 4, 3, 36, 48, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)";
    let chroma = vec![255u8, 0, 0, 0, 0, 0, 0, 128, 0, 0, 0, 0];
    let data = notes(&[(0, 96, 36, 100), (96, 96, 43, 127), (192, 96, 48, 1)]);
    // The newest save's bassline, and the same notes in the older save.
    for (id, src) in [(1, r"C:\P\Song\Song_2.flp"), (3, r"C:\P\Song\Song_1.flp")] {
        db.execute(
            phrase,
            params![
                id,
                src,
                "Bass",
                "Pattern 1",
                "Bass",
                0,
                0,
                1,
                0.3,
                chroma,
                "",
                vec![10u8; 48],
                "AAAA",
                data,
                None::<String>,
                None::<i64>,
                ""
            ],
        )
        .unwrap();
    }
    db.execute(
        phrase,
        params![
            2,
            r"D:\Packs\Keys\Dark.mid",
            "",
            "Dark",
            "Chords",
            0,
            None::<i64>,
            None::<i64>,
            None::<f64>,
            chroma,
            "9:211 0:091/4",
            vec![0u8; 48],
            "BBBB",
            notes(&[(0, 384, 57, 90)]),
            None::<String>,
            4,
            "Keyscape"
        ],
    )
    .unwrap();
    db.execute("INSERT INTO your_roles (hash, role) VALUES ('BBBB', 'Arp')", []).unwrap();
    db.execute("INSERT INTO your_octaves (hash, octaves) VALUES ('AAAA', -1)", []).unwrap();
    path
}

#[test]
fn reads_what_the_csharp_app_saved() {
    let path = full("reads.db");
    let before = std::fs::read(&path).unwrap();
    let index = MidiIndex::open_read_only(&path).unwrap();
    assert_eq!(index.phrase_count().unwrap(), 3);

    let all = index.all().unwrap();
    // Newest file first.
    assert_eq!(all.iter().map(|p| p.id).collect::<Vec<_>>(), [1, 2, 3]);
    let bass = &all[0];
    assert_eq!((bass.role, bass.your_role, bass.your_octave), (Role::Bass, None, Some(-1)));
    assert_eq!(bass.source.kind, SourceKind::Project);
    assert_eq!(bass.source.tempo, Some(140.0));
    assert_eq!(bass.key.unwrap().short(), "Cm");
    assert_eq!((bass.lowest, bass.highest, bass.bars, bass.notes), (36, 48, 4, 3));
    assert_eq!(bass.chroma[7], 128);
    let keys = &all[1];
    assert_eq!((keys.role, keys.your_role, keys.role()), (Role::Chords, Some(Role::Arp), Role::Arp));
    assert_eq!(keys.chords, vec![PhraseChord::new(9, 0x211), PhraseChord { root: 0, mask: 0x091, bass: 4 }]);
    assert_eq!((keys.program, keys.track_name.as_str(), keys.key), (Some(4), "Keyscape", None));

    // Notes by id, or by hash once the id is gone.
    let n = index.notes(1, None).unwrap();
    // An old id that's now another part's: found by its hash instead.
    assert_eq!(index.notes(1, Some("BBBB")).unwrap().len(), 1);
    assert_eq!(n.len(), 3);
    assert_eq!(n[2].velocity7(), 1);
    assert_eq!(index.notes(99, Some("BBBB")).unwrap().len(), 1);
    assert!(index.notes(99, None).unwrap().is_empty());
    assert!(index.notes(99, Some("none")).unwrap().is_empty());

    let failures = index.failures().unwrap();
    assert_eq!(failures.iter().map(|f| f.name()).collect::<Vec<_>>(), ["A locked.mid", "zz broken.mid"]);
    let sources = index.sources().unwrap();
    assert_eq!(sources.len(), 5);
    assert_eq!(sources[&r"C:\P\SONG\SONG_2.FLP".to_owned()], (200, 10, 901));

    let catalogue = Catalogue::new(all);
    assert_eq!(catalogue.entries.len(), 2);
    assert_eq!(catalogue.entries[0].copies(), 2);
    let keys = &catalogue.entries[1];
    assert_eq!(keys.instrument.as_ref().unwrap().why, "“Keyscape” is a piano plugin");

    drop(index);
    assert_eq!(std::fs::read(&path).unwrap(), before, "midi.db is never changed");
}

#[test]
fn never_writes() {
    let path = full("never.db");
    let index = MidiIndex::open_read_only(&path).unwrap();
    drop(index);
    // A second connection the same way can't write either.
    let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert!(conn.execute("DELETE FROM phrases", []).is_err());
}

#[test]
fn reads_an_older_midi_db() {
    let path = make("old-midi.db", OLD_SCHEMA);
    let db = Connection::open(&path).unwrap();
    db.execute("INSERT INTO sources VALUES ('a.mid', 'file', 'MIDI', 5, 5, NULL, 9, NULL)", []).unwrap();
    db.execute(
        "INSERT INTO phrases (id, source, part, clip, role, hash, chroma, features) VALUES (1, 'a.mid', 'x', 'y', 'Melody', 'H', x'00', x'')",
        [],
    )
    .unwrap();
    drop(db);
    let index = MidiIndex::open_read_only(&path).unwrap();
    let all = index.all().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(
        (all[0].role, all[0].members.as_str(), all[0].program, all[0].your_octave),
        (Role::Melody, "", None, None)
    );
    // No notes stored: none read, not a crash.
    assert!(index.notes(1, None).unwrap().is_empty());
}

#[test]
fn says_what_is_wrong() {
    let missing = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-midi").join("not-there.db");
    assert!(matches!(MidiIndex::open_read_only(&missing), Err(MidiError::Open { .. })));
    let other = make("not-midi.db", "CREATE TABLE projects (id INTEGER)");
    let err = MidiIndex::open_read_only(&other).err().unwrap();
    assert!(err.to_string().contains("isn't a MIDI index"), "{err}");
}
