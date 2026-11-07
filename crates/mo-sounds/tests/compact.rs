//! The Rust app's own sound library (`sounds-rust.db`): made from a sounds.db built the way the C# app builds it, it
//! must give every answer the C# app's file gives, to the last bit (CLAP's fingerprints as CLAP's index holds them),
//! and a copy of the format before must come out of being laid out anew holding every fact it held.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use mo_sounds::compact::backup::{self, BackedUp, When};
use mo_sounds::compact::did::{self, Did};
use mo_sounds::compact::grow::{self, Kept, Listened};
use mo_sounds::compact::{self, LaidOut, PageRead, Synced};
use mo_sounds::index::{Gathered, SoundIndex, held, unheld};
use mo_sounds::{ChordSpan, Everything, SoundAnalysis, SoundsDb, SoundsError, event};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, params};

/// The C# app's tables and indexes, as it makes them (`SoundLibrary.Schema`, `NewIndexes` and `PrintSchema`).
const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS sounds (
    id INTEGER PRIMARY KEY,
    path TEXT UNIQUE NOT NULL COLLATE NOCASE,
    size INTEGER, mtime INTEGER, error TEXT,
    analysis TEXT, features BLOB, clap BLOB,
    name_type TEXT, user_type TEXT, type TEXT, type_source TEXT, type_conf REAL,
    favourite INTEGER DEFAULT 0, rating INTEGER DEFAULT 0, plays INTEGER DEFAULT 0, last_played INTEGER
);
CREATE TABLE IF NOT EXISTS sound_tags (
    sound_id INTEGER REFERENCES sounds(id) ON DELETE CASCADE,
    tag TEXT NOT NULL, source TEXT NOT NULL DEFAULT 'you',
    UNIQUE(sound_id, tag)
);
CREATE INDEX IF NOT EXISTS ix_sound_tags_tag ON sound_tags(tag);
CREATE TABLE IF NOT EXISTS sound_events (
    sound_id INTEGER REFERENCES sounds(id) ON DELETE CASCADE,
    kind TEXT NOT NULL, at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_sound_events ON sound_events(kind, at);
CREATE TABLE IF NOT EXISTS sound_maps (
    space TEXT NOT NULL,
    sound_id INTEGER NOT NULL REFERENCES sounds(id) ON DELETE CASCADE,
    x REAL NOT NULL, y REAL NOT NULL,
    PRIMARY KEY (space, sound_id)
);
CREATE INDEX IF NOT EXISTS ix_sound_maps_sound ON sound_maps(sound_id);
CREATE INDEX IF NOT EXISTS ix_sound_events_sound ON sound_events(sound_id);
CREATE TABLE IF NOT EXISTS sound_prints (
    sound_id INTEGER PRIMARY KEY REFERENCES sounds(id) ON DELETE CASCADE,
    mtime INTEGER, size INTEGER, version INTEGER,
    exact BLOB, frames INTEGER, bands BLOB, keys BLOB, sketch BLOB, seconds REAL, sfp BLOB,
    compared INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS ix_sound_prints_exact ON sound_prints(exact);
CREATE TABLE IF NOT EXISTS sound_matches (
    a INTEGER NOT NULL REFERENCES sounds(id) ON DELETE CASCADE,
    b INTEGER NOT NULL REFERENCES sounds(id) ON DELETE CASCADE,
    kind TEXT NOT NULL, score REAL, offset REAL, apart REAL, typical REAL,
    PRIMARY KEY (a, b)
);
CREATE INDEX IF NOT EXISTS ix_sound_matches_b ON sound_matches(b);
CREATE TABLE IF NOT EXISTS sound_hidden (
    sound_id INTEGER PRIMARY KEY REFERENCES sounds(id) ON DELETE CASCADE,
    batch TEXT NOT NULL, at INTEGER NOT NULL
);
";

/// "Now" for the tests: 1 June 2026, noon, local time.
fn now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 6, 1).unwrap().and_hms_opt(12, 0, 0).unwrap()
}

fn unix(y: i32, m: u32, d: u32) -> i64 {
    Local
        .from_local_datetime(&NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(12, 0, 0).unwrap())
        .unwrap()
        .timestamp()
}

/// The same numbers every run (xorshift64*).
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    /// 0 to 1.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[self.below(from.len() as u64) as usize]
    }
}

/// As the C# analyser rounds (`Math.Round(x, digits)`).
fn round(x: f64, digits: i32) -> f64 {
    let p = 10f64.powi(digits);
    (x * p).round_ties_even() / p
}

fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// A folder of its own for each test, emptied first.
fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-sounds-compact").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The C# app's connection: foreign keys on, so forgetting a sound forgets its tags, plays and places too.
fn cs(db: &Path) -> Connection {
    let c = Connection::open(db).unwrap();
    c.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    c
}

/// An analysis as the C# app stores it: its own fields, the features again, and the two names it works out.
fn analysis_json(dice: &mut Dice) -> String {
    let rounded = dice.chance(90);
    let r = |x: f64, d: i32| if rounded { round(x, d) } else { x };
    let waveform: Vec<f32> = (0..if dice.chance(5) { 0 } else { 160 })
        .map(|_| {
            let x = dice.unit();
            if rounded { round(x, 3) as f32 } else { x as f32 }
        })
        .collect();
    let chroma: Vec<f64> = (0..12).map(|_| r(dice.unit(), 3)).collect();
    let chords = if dice.chance(20) {
        vec![
            ChordSpan { start: 0.0, end: r(dice.unit() * 2.0, 3), name: "Cm".into() },
            ChordSpan { start: 1.0, end: 2.5, name: "Ab\"maj7\"".into() },
            // A time written out to 17 digits, which text doesn't always bring back to the last bit.
            ChordSpan { start: 2.5, end: 2.5 + (0.1 + 0.2) * dice.unit(), name: "G7".into() },
        ]
    } else {
        Vec::new()
    };
    let words = ["punchy", "soft", "bright", "dark", "wide", "tonal", "noisy", "mono"];
    let character = (0..dice.below(4)).map(|_| dice.pick(&words).into()).collect();
    let tonic = dice.chance(60).then(|| dice.below(12) as i32);
    // A peak just under 0 dB rounds to -0.0, as Math.Round does.
    let peak_db = if dice.chance(10) { -0.0 } else { r(-dice.unit() * 30.0, 1) };
    let a = SoundAnalysis {
        seconds: r(dice.unit() * 20.0, 3),
        sample_rate: *[44_100, 48_000, 96_000].get(dice.below(3) as usize).unwrap(),
        channel_count: 1 + dice.below(2) as i32,
        peak_db,
        rms_db: r(-dice.unit() * 40.0, 1),
        attack_ms: r(dice.unit() * 100.0, 1),
        decay_ms: r(dice.unit() * 2000.0, 1),
        brightness: r(dice.unit() * 8000.0, 0),
        noisiness: r(dice.unit(), 3),
        width: r(dice.unit(), 3),
        is_loop: dice.chance(30),
        bpm: dice.chance(40).then(|| r(80.0 + dice.unit() * 100.0, 2)),
        bpm_source: if dice.chance(40) { "name".into() } else { Default::default() },
        pitch: dice.chance(50).then(|| r(24.0 + dice.unit() * 60.0, 2)),
        pitch_confidence: r(dice.unit(), 3),
        key_tonic: tonic,
        key_minor: dice.chance(50),
        key_confidence: r(dice.unit(), 3),
        key_source: if tonic.is_some() { "chroma".into() } else { Default::default() },
        chord: dice.chance(10).then(|| "Ebm".to_owned()),
        chords,
        chroma,
        character,
        waveform: waveform.into(),
    };
    let mut json = serde_json::to_value(&a).unwrap();
    let features: Vec<f32> = (0..73).map(|_| dice.unit() as f32).collect();
    json["Features"] = serde_json::json!(features);
    json["KeyName"] = serde_json::json!(a.key_name());
    json["RootNoteName"] = serde_json::json!(a.root_note_name());
    if dice.chance(5) {
        // An older analysis, without some of today's fields.
        json.as_object_mut().unwrap().retain(|k, _| !matches!(k.as_str(), "Chroma" | "Width" | "KeySource"));
    }
    json.to_string()
}

/// A library like Solomon's in all but size: `n` sounds, with every kind of value the C# app writes and some it
/// doesn't mean to (damaged ones), and every table filled.
fn library(dir: &Path, n: i64) -> PathBuf {
    let db = dir.join("sounds.db");
    let c = cs(&db);
    c.execute_batch(SCHEMA).unwrap();
    let mut dice = Dice(0x9e37_79b9_7f4a_7c15);
    let packs = ["Neptunian Drumz", "Lo-Fi Keys", "VENGEANCE", "kits"];
    let folders = ["Kicks", "Snares", "Pads", "Loops", "One Shots", "FX"];
    let types = ["Kick", "Snare", "Pad", "Loop", "FX", "Hat"];
    let tx = c.unchecked_transaction().unwrap();
    // Ids with gaps (sounds forgotten since), paths out of id order.
    let mut id = 0;
    for i in 0..n {
        id += 1 + dice.below(3) as i64;
        let path = format!(
            "{}:\\Samples\\{}\\{}\\{} {:05}.wav",
            if dice.chance(50) { "D" } else { "e" },
            dice.pick(&packs),
            dice.pick(&folders),
            dice.pick(&types),
            (i * 7919) % 100_000
        );
        let broken = dice.chance(4);
        let analysis = if broken { None } else { Some(analysis_json(&mut dice)) };
        let features =
            (!broken && dice.chance(95)).then(|| floats(&(0..73).map(|_| dice.unit() as f32).collect::<Vec<_>>()));
        let clap = (!broken && dice.chance(80))
            .then(|| floats(&(0..512).map(|_| dice.unit() as f32 - 0.5).collect::<Vec<_>>()));
        let name_type = dice.chance(60).then(|| dice.pick(&types));
        let user_type = dice.chance(5).then(|| dice.pick(&types));
        let (shown, source) = match (user_type, name_type) {
            (Some(t), _) => (Some(t), "you"),
            (None, Some(t)) => (Some(t), "name"),
            (None, None) if dice.chance(70) => (Some(dice.pick(&types)), "learned"),
            _ => (None, ""),
        };
        c.execute(
            "INSERT INTO sounds (id, path, size, mtime, error, analysis, features, clap, name_type, user_type, type,
                 type_source, type_conf, favourite, rating, plays, last_played)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                id,
                path,
                dice.chance(97).then(|| 1000 + dice.below(9_000_000) as i64),
                dice.chance(98).then(|| unix(2020, 1, 1) + dice.below(200_000_000) as i64),
                broken.then_some("Couldn't read the file: unsupported format"),
                analysis,
                features,
                clap,
                name_type,
                user_type,
                shown,
                source,
                round(dice.unit(), 3),
                dice.chance(10),
                dice.below(6) as i64,
                dice.below(20) as i64,
                dice.chance(20).then(|| unix(2025, 1, 1) + dice.below(40_000_000) as i64),
            ],
        )
        .unwrap();
    }
    let last = id;
    // Values the C# app doesn't mean to write, as a damaged file might hold them: each is read the same either way.
    c.execute_batch(&format!(
        "INSERT INTO sounds (id, path, analysis, features, clap, type, rating, plays, favourite, error, type_conf)
             VALUES ({a}, 'D:\\Damaged\\one.wav', 'not json', x'0102030405', 'text where a fingerprint goes', NULL,
                     'five', 2.5, 2, CAST(x'48ff49' AS TEXT), -0.0);
         INSERT INTO sounds (id, path, analysis, mtime, size)
             VALUES ({b}, 'D:\\Damaged\\two.wav', x'7b7d', 'yesterday', 'big');
         INSERT INTO sounds (id, path, analysis) VALUES ({c}, 'D:\\Damaged\\three.wav', '{{}}');",
        a = last + 1,
        b = last + 2,
        c = last + 3
    ))
    .unwrap();
    let ids: Vec<i64> = {
        let mut st = c.prepare("SELECT id FROM sounds").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    c.execute_batch(
        "INSERT INTO settings VALUES ('galaxy_basis', 'clap'), ('empty', NULL), ('classifier', '{\"w\":[1,2]}');",
    )
    .unwrap();
    let tags = ["warm", "big", "dusty", "808", "vocal"];
    for &id in &ids {
        if dice.chance(30) {
            c.execute(
                "INSERT OR IGNORE INTO sound_tags VALUES (?1, ?2, ?3)",
                params![id, dice.pick(&tags), if dice.chance(70) { "you" } else { "learned" }],
            )
            .unwrap();
        }
        for _ in 0..dice.below(4) {
            let at = unix(2024, 1, 1) + dice.below(80_000_000) as i64;
            c.execute(
                "INSERT INTO sound_events VALUES (?1, ?2, ?3)",
                params![id, dice.pick(&["play", "drag", "send"]), at],
            )
            .unwrap();
        }
        for space in ["features", "clap"] {
            if dice.chance(80) {
                let (x, y) = (dice.unit() * 100.0 - 50.0, dice.unit() * 100.0 - 50.0);
                c.execute("INSERT INTO sound_maps VALUES (?1, ?2, ?3, ?4)", params![space, id, x, y]).unwrap();
            }
        }
        if dice.chance(70) {
            // Every eleventh shares its hash with another: an exact copy. The rest of a print is big, and not copied.
            let exact = if dice.chance(9) { vec![7u8, 7, 7] } else { id.to_le_bytes().to_vec() };
            let big: Vec<u8> = (0..1500).map(|i| (i % 251) as u8).collect();
            c.execute(
                "INSERT INTO sound_prints (sound_id, mtime, size, version, exact, frames, bands, keys, sketch, seconds)
                 VALUES (?1, 1, 2, 3, ?2, 4, ?3, ?3, ?3, 1.5)",
                params![id, exact, big],
            )
            .unwrap();
        }
        if dice.chance(5) {
            c.execute("INSERT INTO sound_hidden VALUES (?1, 'batch 1', ?2)", params![id, unix(2026, 1, 1)]).unwrap();
        }
    }
    // An event the C# app made before it had sound ids for them.
    c.execute_batch("INSERT INTO sound_events VALUES (NULL, 'play', 5);").unwrap();
    for pair in ids.windows(2).step_by(37) {
        let kind = *["Exact", "Converted", "Trimmed", "Mystery"].get(dice.below(4) as usize).unwrap();
        c.execute(
            "INSERT INTO sound_matches VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
            params![pair[0], pair[1], kind, round(dice.unit(), 3), 0.25, dice.chance(50).then_some(1.5)],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    // Rows left behind by a sound that's gone, from a time foreign keys weren't checked.
    c.execute_batch(
        "PRAGMA foreign_keys = OFF;
         INSERT INTO sound_tags VALUES (999999, 'lost', 'you');
         INSERT INTO sound_events VALUES (999999, 'play', 7);
         INSERT INTO sound_maps VALUES ('features', 999999, 1.5, -2.5);
         PRAGMA foreign_keys = ON;",
    )
    .unwrap();
    db
}

/// The half-made copies (and their SQLite files) in `dir`.
fn half_made(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".part"))
        .collect();
    names.sort();
    names
}

/// Both read the same, every way they can be read, to the last bit (a -0.0 isn't a 0.0): CLAP's fingerprints as
/// CLAP's index holds them ([`held`]) from a copy of this version's format, so its index is the same to the last bit;
/// as they are from one of the format before.
fn same_answers(old: &Path, copy: &Path) {
    let a = SoundsDb::open_read_only(old).unwrap();
    let b = SoundsDb::open_read_only(copy).unwrap();
    assert!(!a.is_copy());
    assert!(b.is_copy());
    let as_kept = |clap: Gathered| if b.is_formerly() { clap } else { clap.into_held() };
    let (ea, mut eb) = (a.everything(now()).unwrap(), b.everything(now()).unwrap());
    eb.sounds = waved(&b, eb.sounds);
    assert_eq!(ea.sounds.len(), eb.sounds.len());
    for (x, y) in ea.sounds.iter().zip(&eb.sounds) {
        assert_eq!(format!("{x:?}"), format!("{y:?}"));
    }
    assert_eq!(ea.features, eb.features);
    assert_eq!(as_kept(ea.clap.clone()), eb.clap);
    assert_eq!(ea.analysed(), eb.analysed());
    // "Sounds like this" from either: the same index, to the last bit.
    assert_eq!(
        SoundIndex::from_gathered(ea.features.clone(), ea.clap.clone(), ea.analysed()),
        SoundIndex::from_gathered(eb.features.clone(), eb.clap.clone(), eb.analysed())
    );
    assert_eq!(format!("{:?}", a.all(now()).unwrap()), format!("{:?}", all_of(&b)));
    assert_eq!(a.activity(now()).unwrap(), b.activity(now()).unwrap());
    assert_eq!(a.history(500).unwrap(), b.history(500).unwrap());
    for space in ["features", "clap"] {
        assert_eq!(a.map(space).unwrap(), b.map(space).unwrap(), "{space}");
        let mut wanted = a.vectors(space).unwrap();
        if space == "clap" && !b.is_formerly() {
            wanted.values_mut().for_each(|v| *v = unheld(&held(v)));
        }
        assert_eq!(wanted, b.vectors(space).unwrap(), "{space}");
    }
    assert_eq!(a.tag_counts().unwrap(), b.tag_counts().unwrap());
    for tag in ["warm", "big", "dusty", "808", "vocal", "none"] {
        for source in ["you", "learned"] {
            assert_eq!(a.tagged(tag, source).unwrap(), b.tagged(tag, source).unwrap(), "{tag} {source}");
        }
    }
    assert_eq!(a.hidden().unwrap(), b.hidden().unwrap());
    // The hidden sounds alone: the same ones every read marks hidden.
    let hidden = a.hidden_sounds().unwrap();
    let ids = |sounds: &mut dyn Iterator<Item = &mo_sounds::Sound>| {
        let mut ids: Vec<i64> = sounds.map(|s| s.id).collect();
        ids.sort_unstable();
        ids
    };
    assert_eq!(ids(&mut hidden.iter()), ids(&mut ea.sounds.iter().filter(|s| s.hidden)));
    assert!(hidden.iter().all(|s| s.hidden));
    assert_eq!(format!("{hidden:?}"), format!("{:?}", waved(&b, b.hidden_sounds().unwrap())));
    // Read at one moment, the same in both (the sizes compared as maps: their printed order is random).
    let (fa, fb) = (a.finds().unwrap(), b.finds().unwrap());
    assert_eq!((&fa.matches, &fa.exact, &fa.sizes), (&fb.matches, &fb.exact, &fb.sizes));
    assert_eq!(format!("{:?}", fa.hidden), format!("{:?}", waved(&b, fb.hidden.clone())));
    assert_eq!(a.matches().unwrap(), b.matches().unwrap());
    assert_eq!(a.exact_copies().unwrap(), b.exact_copies().unwrap());
    assert_eq!(a.duplicate_finds().unwrap(), b.duplicate_finds().unwrap());
    assert_eq!(a.sizes().unwrap(), b.sizes().unwrap());
    assert_eq!(a.count().unwrap(), b.count().unwrap());
    assert_eq!(a.count_with_clap().unwrap(), b.count_with_clap().unwrap());
    // The damaged size ('big') stops this read in both, as it does in the C# app.
    assert_eq!(format!("{:?}", a.known_files()), format!("{:?}", b.known_files()));
    for key in ["galaxy_basis", "empty", "classifier", "missing"] {
        assert_eq!(a.setting(key).unwrap(), b.setting(key).unwrap(), "{key}");
    }
}

/// The same sounds and fingerprints, to the last bit.
fn same_everything(a: &Everything, b: &Everything) {
    assert_eq!(a.sounds.len(), b.sounds.len());
    for (x, y) in a.sounds.iter().zip(&b.sounds) {
        assert_eq!(format!("{x:?}"), format!("{y:?}"));
    }
    assert_eq!(a.features, b.features);
    assert_eq!(a.clap, b.clap);
}

/// A sound's row's columns in the copy but for how many numbers its CLAP has: the same in both formats.
const ROW_COLUMNS: &str = "id, path, size, mtime, error, name_type, user_type, type, type_source, type_conf, favourite, \
     rating, plays, last_played, seconds, sample_rate, channels, peak_db, rms_db, attack_ms, decay_ms, brightness, \
     noisiness, width, is_loop, bpm, bpm_source, pitch, pitch_conf, key_tonic, key_minor, key_conf, key_source, chord, \
     chords, chroma, character";

/// The tables copied whole from `sounds.db`, in both formats.
const WHOLE: [&str; 7] =
    ["settings", "sound_tags", "sound_events", "sound_maps", "sound_matches", "sound_hidden", "sound_prints"];

/// Removes a database and SQLite's files beside it.
fn remove_db(path: &Path) {
    for end in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{end}", path.display()));
    }
}

/// Makes `copy` a copy of `old` as the version before this one made it and kept it (format 1, until 2026-11-05): the
/// waveform in each sound's row, CLAP's fingerprints as `sounds.db` holds them, how long each is beside a hash of each
/// sound's row (`sound_hashes`), and a hash of each table copied whole in `copy_info`. Up to date with `old`, and the
/// app's own sound library since `own_since` when that's given. Made with SQL from a copy of this version's format and
/// `old` itself, as that version wrote it (checked once against a copy that version made: the same rows, the same
/// tables).
fn formerly(old: &Path, copy: &Path, own_since: Option<i64>) {
    let made = copy.with_file_name("made-now.db");
    remove_db(&made);
    assert!(compact::sync(old, &made).unwrap().made);
    remove_db(copy);
    let c = Connection::open(copy).unwrap();
    c.execute_batch("PRAGMA page_size = 32768; PRAGMA foreign_keys = OFF;").unwrap();
    c.execute("ATTACH DATABASE ?1 AS made", [made.to_str().unwrap()]).unwrap();
    c.execute("ATTACH DATABASE ?1 AS lib", [old.to_str().unwrap()]).unwrap();
    c.execute_batch(&format!(
        "BEGIN;
         CREATE TABLE copy_info (key TEXT PRIMARY KEY, value);
         CREATE TABLE sounds (
             id INTEGER PRIMARY KEY,
             path TEXT NOT NULL COLLATE NOCASE,
             size INTEGER, mtime INTEGER, error TEXT,
             name_type TEXT, user_type TEXT, type TEXT, type_source TEXT, type_conf REAL,
             favourite INTEGER DEFAULT 0, rating INTEGER DEFAULT 0, plays INTEGER DEFAULT 0, last_played INTEGER,
             seconds, sample_rate, channels, peak_db, rms_db, attack_ms, decay_ms, brightness, noisiness, width,
             is_loop, bpm, bpm_source, pitch, pitch_conf, key_tonic, key_minor, key_conf, key_source, chord, chords,
             chroma, character, waveform
         );
         CREATE TABLE sound_features (sound_id INTEGER PRIMARY KEY, v);
         CREATE TABLE sound_clap (sound_id INTEGER PRIMARY KEY, v);
         CREATE TABLE sound_hashes (sound_id INTEGER PRIMARY KEY, row INTEGER NOT NULL, features INTEGER NOT NULL,
             clap INTEGER NOT NULL, clap_len INTEGER);
         INSERT INTO main.sounds SELECT {row}, w.v FROM made.sounds s LEFT JOIN made.sound_waves w ON w.sound_id = s.id;
         INSERT INTO main.sound_features SELECT sound_id, v FROM made.sound_features;
         INSERT INTO main.sound_clap SELECT id, clap FROM lib.sounds WHERE clap IS NOT NULL;
         INSERT INTO main.sound_hashes SELECT id, id * 7919, id * 31, id * 17,
             CASE WHEN clap IS NULL THEN NULL WHEN typeof(clap) = 'blob' THEN length(clap) ELSE 0 END FROM lib.sounds;",
        row = ROW_COLUMNS.split(", ").map(|c| format!("s.{c}")).collect::<Vec<_>>().join(", ")
    ))
    .unwrap();
    for table in WHOLE {
        let sql: Option<String> = c
            .query_row("SELECT sql FROM made.sqlite_master WHERE type = 'table' AND name = ?1", [table], |r| r.get(0))
            .ok();
        let Some(sql) = sql else { continue };
        c.execute_batch(&sql).unwrap();
        let columns: Vec<String> = {
            let mut st = c.prepare("SELECT name FROM pragma_table_info(?1, 'made')").unwrap();
            st.query_map([table], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        let columns = columns.join(", ");
        c.execute_batch(&format!(
            "INSERT INTO main.{table} (rowid, {columns}) SELECT rowid, {columns} FROM made.{table};"
        ))
        .unwrap();
        let indexes: Vec<String> = {
            let mut st = c
                .prepare(
                    "SELECT sql FROM made.sqlite_master WHERE type = 'index' AND tbl_name = ?1 AND sql IS NOT NULL",
                )
                .unwrap();
            st.query_map([table], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        for sql in indexes {
            c.execute_batch(&sql).unwrap();
        }
        c.execute("INSERT INTO main.copy_info VALUES (?1, ?2)", params![format!("hash:{table}"), 0x5eed_i64 * 3])
            .unwrap();
    }
    c.execute_batch(
        "INSERT INTO main.copy_info SELECT key, value FROM made.copy_info WHERE key <> 'format';
         INSERT INTO main.copy_info VALUES ('format', 1);
         CREATE INDEX sounds_path ON sounds(path);",
    )
    .unwrap();
    if let Some(since) = own_since {
        c.execute("INSERT INTO main.copy_info VALUES ('own_since', ?1)", [since]).unwrap();
    }
    c.execute_batch("COMMIT; DETACH DATABASE made; DETACH DATABASE lib; PRAGMA journal_mode = WAL;").unwrap();
    drop(c);
    remove_db(&made);
    assert_eq!(format_of(copy), 1);
}

/// [`compact::lay_out`], and whether it said it was starting the work.
fn lay_out(old: &Path) -> (Option<LaidOut>, bool) {
    let mut started = false;
    let laid_out = compact::lay_out(old, || started = true);
    (laid_out, started)
}

/// The format the copy at `copy` notes it's laid out in.
fn format_of(copy: &Path) -> i64 {
    Connection::open(copy)
        .unwrap()
        .query_row("SELECT value FROM copy_info WHERE key = 'format'", [], |r| r.get(0))
        .unwrap()
}

/// Every row `sql` gives from the database at `db`, each value as it reads (`{:?}`: a -0.0 isn't a 0.0).
fn rows(db: &Path, sql: &str) -> Vec<String> {
    let c = Connection::open(db).unwrap();
    let mut st = c.prepare(sql).unwrap();
    let n = st.column_count();
    st.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get_ref(i).unwrap())).collect::<Vec<_>>().join(" | ")))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Whether `a` and `b` hold the same tables and indexes, made the same way, and the same rows in each, in the same
/// order, every value to the last bit: but for `copy_info`, compared apart.
fn same_files(a: &Path, b: &Path) {
    let made = "SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY name";
    assert_eq!(rows(a, made), rows(b, made));
    let tables: Vec<String> = {
        let c = Connection::open(a).unwrap();
        let mut st = c
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name <> 'copy_info' ORDER BY name")
            .unwrap();
        st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    for table in tables {
        let sql = format!("SELECT rowid, * FROM {table} ORDER BY rowid");
        assert_eq!(rows(a, &sql), rows(b, &sql), "{table}");
    }
}

/// Every sound as [`SoundsDb::all`] reads it from `db`, each with its waveform: read apart from the app's own copy
/// ([`SoundsDb::waves`]), with the sounds from the others.
fn all_of(db: &SoundsDb) -> Vec<mo_sounds::Sound> {
    waved(db, db.all(now()).unwrap())
}

/// `sounds` read from `db`, each given its waveform when `db` keeps them apart.
fn waved(db: &SoundsDb, mut sounds: Vec<mo_sounds::Sound>) -> Vec<mo_sounds::Sound> {
    if !db.waves_apart() {
        return sounds;
    }
    let ids: Vec<i64> = sounds.iter().map(|s| s.id).collect();
    let mut waves: std::collections::HashMap<i64, mo_sounds::Peaks> = db.waves(&ids).unwrap().into_iter().collect();
    for s in &mut sounds {
        let a = s.analysis.as_mut();
        assert!(a.as_ref().is_none_or(|a| a.waveform.is_empty()), "read without its waveform");
        if let (Some(a), Some(wave)) = (a, waves.remove(&s.id)) {
            a.waveform = wave;
        }
    }
    assert!(waves.is_empty(), "a waveform for each sound with one");
    sounds
}

/// What the page's own read of `sounds.db` gives.
fn read_for_page(old: &Path) -> Everything {
    SoundsDb::open_read_only(old).unwrap().everything(now()).unwrap()
}

#[test]
fn the_copy_gives_every_answer_sounds_db_gives() {
    let dir = folder("every");
    let old = library(&dir, 1500);
    let copy = compact::copy_of(&old);
    assert!(!compact::in_step(&old, &copy));
    let synced = compact::sync(&old, &copy).unwrap();
    assert_eq!(synced, Synced { made: true, sounds: 1503, adopted: false, ..Synced::default() });
    assert!(compact::in_step(&old, &copy));
    same_answers(&old, &copy);
    // Nothing changed since: nothing to do.
    let again = compact::sync(&old, &copy).unwrap();
    assert_eq!(again, Synced::default());
    assert!(!again.changed_the_list());
    same_answers(&old, &copy);
}

#[test]
fn the_copy_is_much_smaller() {
    let dir = folder("small");
    let old = library(&dir, 1500);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    // Folded into the file itself, as the C# app's would be when it closes.
    cs(&old).query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).unwrap();
    let (old_size, copy_size) = (std::fs::metadata(&old).unwrap().len(), std::fs::metadata(&copy).unwrap().len());
    assert!(copy_size * 100 < old_size * 25, "the copy is {copy_size} bytes, sounds.db {old_size}");
    let c = Connection::open(&copy).unwrap();
    assert_eq!(c.query_row("PRAGMA page_size", [], |r| r.get::<_, i64>(0)).unwrap(), 32_768);
    assert_eq!(c.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0)).unwrap(), "wal");
    // CLAP's fingerprints at 2 bytes a number, and nothing kept to follow sounds.db's saves by.
    let clap: i64 = c.query_row("SELECT SUM(length(v)) FROM sound_clap", [], |r| r.get(0)).unwrap();
    let heard: i64 = c.query_row("SELECT SUM(clap_numbers) FROM sounds", [], |r| r.get(0)).unwrap();
    assert_eq!(clap, heard * 2);
    let hashes: i64 = c
        .query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'sound_hashes' OR sql LIKE '%waveform%'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(hashes, 0);
    // Nothing left beside it from making it.
    assert_eq!(half_made(&dir), Vec::<String>::new());
}

/// What the C# app does to sounds.db, one save at a time (`SoundLibrary.cs`): a copy made before the C# app was retired
/// that's behind is made afresh, and gives every answer `sounds.db` gives after each.
#[test]
fn a_copy_behind_is_made_afresh_after_each_kind_of_save() {
    let dir = folder("follows");
    let old = library(&dir, 600);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    let c = cs(&old);
    let one: i64 = c
        .query_row("SELECT id FROM sounds WHERE analysis IS NOT NULL AND clap IS NULL LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let one_path: String = c.query_row("SELECT path FROM sounds WHERE id = ?1", [one], |r| r.get(0)).unwrap();
    let step = |what: &str, sql: &str| {
        c.execute_batch(sql).unwrap();
        assert!(!compact::in_step(&old, &copy), "{what}: the copy should be behind");
        let count: i64 = c.query_row("SELECT COUNT(*) FROM sounds", [], |r| r.get(0)).unwrap();
        let s = compact::sync(&old, &copy).unwrap();
        assert_eq!(s, Synced { made: true, sounds: count as usize, adopted: false, ..Synced::default() }, "{what}");
        assert!(s.changed_the_list());
        assert!(compact::in_step(&old, &copy), "{what}");
        same_answers(&old, &copy);
    };
    // MarkPlayed: one more play, and when.
    step(
        "a play",
        &format!(
            "UPDATE sounds SET plays = plays + 1, last_played = 1780000000 WHERE id = {one};
             INSERT INTO sound_events VALUES ({one}, 'play', 1780000000);"
        ),
    );
    step("stars and a favourite", &format!("UPDATE sounds SET rating = 5, favourite = 1 WHERE id = {one}"));
    step(
        "your own type, and the classifier's",
        &format!(
            "UPDATE sounds SET user_type = 'Clap', type = 'Clap', type_source = 'you', type_conf = 1 WHERE id = {one};
             UPDATE sounds SET type = 'Perc', type_source = 'learned', type_conf = 0.75
                 WHERE id IN (SELECT id FROM sounds WHERE user_type IS NULL LIMIT 50);"
        ),
    );
    // SetClap: CLAP listened to one more.
    let clap = floats(&[0.25; 512]);
    c.execute("UPDATE sounds SET clap = ?1 WHERE id = ?2", params![clap, one]).unwrap();
    // Save: a scan finds a new file, and a changed one (its CLAP kept, as the C# app's upsert keeps it).
    let save = "INSERT INTO sounds (path, size, mtime, error, analysis, features, clap, name_type, type, type_source, type_conf)
        VALUES (?1, ?2, ?3, NULL, ?4, ?5, NULL, 'Kick', COALESCE('Kick', 'Unknown'), 'name', 1)
        ON CONFLICT(path) DO UPDATE SET size = excluded.size, mtime = excluded.mtime, error = excluded.error,
            analysis = excluded.analysis, features = excluded.features, clap = COALESCE(excluded.clap, sounds.clap),
            name_type = excluded.name_type";
    let mut dice = Dice(42);
    for path in ["Z:\\New\\Kicks\\Kick new.wav", one_path.as_str()] {
        c.execute(save, params![path, 1234, 1_780_000_000, analysis_json(&mut dice), floats(&[0.5; 73])]).unwrap();
    }
    step("a move", &format!("UPDATE sounds SET path = 'Z:\\Moved\\here.wav', name_type = NULL WHERE id = {one}"));
    // Remove: a file gone, and with it its tags, plays, places, print, copies and hidden mark.
    let gone: i64 = c.query_row("SELECT sound_id FROM sound_hidden LIMIT 1", [], |r| r.get(0)).unwrap();
    step("a file gone", &format!("DELETE FROM sounds WHERE id = {gone}"));
    step(
        "tags, galaxy places, hidden copies, prints, copies found and a setting",
        &format!(
            "INSERT INTO sound_tags VALUES ({one}, 'fresh', 'you') ON CONFLICT(sound_id, tag) DO UPDATE SET source = excluded.source;
             DELETE FROM sound_tags WHERE tag = 'dusty' AND source = 'learned';
             INSERT OR REPLACE INTO sound_maps SELECT 'clap', id, 1.0, 2.0 FROM sounds LIMIT 10;
             INSERT OR REPLACE INTO sound_hidden VALUES ({one}, 'batch 2', 99);
             UPDATE sound_prints SET exact = x'070707' WHERE sound_id IN (SELECT sound_id FROM sound_prints LIMIT 3);
             UPDATE sound_matches SET apart = 2.5, typical = 1.25;
             INSERT INTO settings VALUES ('galaxy_basis', 'features') ON CONFLICT(key) DO UPDATE SET value = excluded.value;"
        ),
    );
    // A table the C# app adds later (an older library had no duplicate finder yet), and one it drops.
    step("no prints", "DROP TABLE sound_prints;");
    let tables: i64 = Connection::open(&copy)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'sound_prints'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tables, 0, "gone from the copy too");
    // With the damaged rows gone, every file the library knows reads the same from both, CLAP or not.
    step("the damaged rows gone", "DELETE FROM sounds WHERE path LIKE 'D:\\Damaged\\%'");
    let known = SoundsDb::open_read_only(&copy).unwrap().known_files().unwrap();
    // The 600 sounds made at the start, with the one the scan added and without the one gone.
    assert_eq!(known.len(), 600);
    assert!(known.iter().any(|k| k.has_clap) && known.iter().any(|k| !k.has_clap));
}

/// A copy left half made by an app closed while it made one (minutes on Solomon's disk) goes at the next start, even
/// though no copy is made then: it's most of a copy's size.
#[test]
fn a_copy_left_half_made_goes_at_the_next_start() {
    let dir = folder("left");
    let old = library(&dir, 30);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    let an_hour_ago = SystemTime::now() - Duration::from_secs(3600);
    let left_long_ago = |path: &Path| {
        std::fs::File::options().write(true).open(path).unwrap().set_modified(an_hour_ago).unwrap();
    };
    // Left by an app closed while it made a copy, an hour ago; and one another app is making just now.
    let (left, making) = (dir.join("sounds-rust.db.part-77777"), dir.join("sounds-rust.db.part-66666"));
    for path in [&left, &making] {
        std::fs::write(path, b"half a copy").unwrap();
    }
    left_long_ago(&left);
    // Bringing the copy up to date clears what was left, and only that.
    cs(&old)
        .execute_batch("UPDATE sounds SET plays = COALESCE(plays, 0) + 1 WHERE id = (SELECT MIN(id) FROM sounds);")
        .unwrap();
    assert!(compact::sync(&old, &copy).unwrap().made);
    assert_eq!(half_made(&dir), vec!["sounds-rust.db.part-66666".to_owned()]);
    same_answers(&old, &copy);
    // So does the page's read of the copy, once the other has been left long enough too.
    left_long_ago(&making);
    let Ok(PageRead::Copy { .. }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read");
    };
    assert_eq!(half_made(&dir), Vec::<String>::new());
}

/// Where SQLite keeps `table` in the file at `db`: the first byte of its first page.
fn page_of(db: &Path, table: &str) -> u64 {
    let c = Connection::open(db).unwrap();
    let root: i64 = c.query_row("SELECT rootpage FROM sqlite_master WHERE name = ?1", [table], |r| r.get(0)).unwrap();
    let size: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
    u64::try_from((root - 1) * size).unwrap()
}

/// Spoils the first page of `table` in the file at `db`, as a bad sector would.
fn damage(db: &Path, table: &str) {
    use std::io::{Seek, SeekFrom, Write};
    let at = page_of(db, table);
    let mut f = std::fs::File::options().write(true).open(db).unwrap();
    f.seek(SeekFrom::Start(at)).unwrap();
    f.write_all(&[0xEE; 64]).unwrap();
}

/// A copy damaged inside (a bad sector) is made again, not brought up to date and left damaged: where its list is,
/// on the page's read; where its fingerprints are, once they're found damaged.
#[test]
fn a_copy_damaged_inside_is_made_again() {
    let dir = folder("damaged");
    let old = library(&dir, 80);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    damage(&copy, "sounds");
    let listed = SoundsDb::open_read_only(&copy).unwrap().all(now()).unwrap_err();
    assert!(listed.damaged(), "{listed}");
    let Ok(PageRead::Library { everything, made }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("a damaged copy isn't read");
    };
    assert!(made.unwrap().made, "made again");
    same_everything(&everything, &read_for_page(&old));
    same_answers(&old, &copy);
    // Its fingerprints damaged: the list still reads, the fingerprints say they're damaged, and the copy is made again
    // on a pass that gives them.
    damage(&copy, "sound_features");
    let db = SoundsDb::open_read_only(&copy).unwrap();
    assert!(db.all(now()).is_ok());
    let prints = db.fingerprints(everything.analysed()).unwrap_err();
    assert!(prints.damaged(), "{prints}");
    drop(db);
    let (again, made) = compact::mend_reading(&old, &copy, now()).unwrap();
    assert!(made.unwrap().made);
    same_everything(&again, &read_for_page(&old));
    same_answers(&old, &copy);
    // What isn't damage isn't taken for it.
    assert!(!SoundsError::Copy { path: copy.clone(), why: "busy".into() }.damaged());
}

/// A copy that's behind is made afresh even when it's damaged where nothing reads it (its index of paths). Damage in
/// `sounds.db` stops the new copy being made, and leaves the copy there as it was, to the last byte: it's all the page
/// has then.
#[test]
fn damage_met_while_making_the_copy_afresh() {
    let dir = folder("damaged-update");
    let old = library(&dir, 300);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    damage(&copy, "sounds_path");
    // (A save that changes nothing leaves the file as it was: SQLite doesn't write a row that's the same.)
    let play = "UPDATE sounds SET plays = COALESCE(plays, 0) + 1 WHERE id = (SELECT MIN(id) FROM sounds);";
    cs(&old).execute_batch(play).unwrap();
    let listed = SoundsDb::open_read_only(&copy).unwrap().all(now());
    assert!(listed.is_ok(), "the copy still reads");
    let synced = compact::sync(&old, &copy).unwrap();
    assert!(synced.made, "made again: {synced:?}");
    same_answers(&old, &copy);
    // sounds.db damaged where making a copy reads it (its tags): the copy is left as it was, and says why it couldn't
    // be made afresh.
    cs(&old).execute_batch(play).unwrap();
    damage(&old, "sound_tags");
    let before = std::fs::read(&copy).unwrap();
    let failed = compact::sync(&old, &copy).unwrap_err();
    assert!(failed.to_string().contains("malformed"), "{failed}");
    assert!(std::fs::read(&copy).unwrap() == before, "the copy changed");
    assert_eq!(half_made(&dir), Vec::<String>::new());
    // Damaged where every sound is read: the same.
    damage(&old, "sounds");
    assert!(compact::sync(&old, &copy).is_err());
    assert!(std::fs::read(&copy).unwrap() == before, "the copy changed");
}

#[test]
fn a_copy_of_another_format_or_half_made_is_made_again() {
    let dir = folder("again");
    let old = library(&dir, 50);
    let copy = compact::copy_of(&old);
    // Left from copies that stopped half way (this app's, and others' long since), and a log beside where the copy
    // goes that isn't its own: none may end up in the new copy, or be left behind. One another copy of the app is
    // making just now stays.
    let an_hour_ago = SystemTime::now() - Duration::from_secs(3600);
    for name in [
        "sounds-rust.db.part".to_owned(),
        "sounds-rust.db.part-99999".to_owned(),
        "sounds-rust.db.part-99999-wal".to_owned(),
        format!("sounds-rust.db.part-{}", std::process::id()),
        format!("sounds-rust.db.part-{}0", std::process::id()),
        "sounds-rust.db.part-88888".to_owned(),
    ] {
        std::fs::write(dir.join(&name), b"half a copy").unwrap();
        if !name.ends_with("88888") && !name.ends_with('0') {
            std::fs::File::options().write(true).open(dir.join(&name)).unwrap().set_modified(an_hour_ago).unwrap();
        }
    }
    std::fs::write(dir.join("sounds-rust.db-wal"), vec![0x37u8; 4096]).unwrap();
    let synced = compact::sync(&old, &copy).unwrap();
    assert!(synced.made);
    let mut others = vec![format!("sounds-rust.db.part-{}0", std::process::id()), "sounds-rust.db.part-88888".into()];
    others.sort();
    assert_eq!(half_made(&dir), others, "only others' half-made copies being made just now are left");
    for name in &others {
        std::fs::remove_file(dir.join(name)).unwrap();
    }
    same_answers(&old, &copy);
    // A copy an older version made, in another format: made again from the start.
    Connection::open(&copy).unwrap().execute("UPDATE copy_info SET value = 0 WHERE key = 'format'", []).unwrap();
    assert!(!compact::in_step(&old, &copy));
    let synced = compact::sync(&old, &copy).unwrap();
    assert!(synced.made);
    assert_eq!(synced.sounds, 53);
    same_answers(&old, &copy);
    // One a newer version made is left as it is (that version would only have to make it again), and isn't read.
    Connection::open(&copy).unwrap().execute("UPDATE copy_info SET value = 99 WHERE key = 'format'", []).unwrap();
    let newer = std::fs::read(&copy).unwrap();
    let left = compact::sync(&old, &copy).unwrap_err().to_string();
    assert!(left.contains("a newer version of the app made it"), "{left}");
    assert!(!compact::in_step(&old, &copy));
    assert_eq!(std::fs::read(&copy).unwrap(), newer);
    let unread = SoundsDb::open_read_only(&copy).err().expect("a copy of another format isn't read").to_string();
    assert!(unread.contains("another version of the app"), "{unread}");
    // Something nothing can be read from where the copy goes: made again too.
    for junk in [&b"not a database"[..], &[]] {
        std::fs::write(&copy, junk).unwrap();
        assert!(compact::sync(&old, &copy).unwrap().made);
        same_answers(&old, &copy);
    }
}

/// Nothing but a copy is ever removed: not a database of something else where the copy goes, nor the library itself.
#[test]
fn only_a_copy_is_ever_replaced() {
    let dir = folder("foreign");
    let old = library(&dir, 20);
    let copy = compact::copy_of(&old);
    let other = Connection::open(&copy).unwrap();
    other.execute_batch("CREATE TABLE songs (name TEXT); INSERT INTO songs VALUES ('mine');").unwrap();
    drop(other);
    let before = std::fs::read(&copy).unwrap();
    let left = compact::sync(&old, &copy).unwrap_err().to_string();
    assert!(left.contains("isn't a copy of the library"), "{left}");
    assert_eq!(std::fs::read(&copy).unwrap(), before);
    // The library itself, by its own name or another spelling of it.
    let library_before = std::fs::read(&old).unwrap();
    for same in [old.clone(), dir.join(".").join("sounds.db")] {
        let left = compact::sync(&old, &same).unwrap_err().to_string();
        assert!(left.contains("the library itself"), "{left}");
    }
    assert_eq!(std::fs::read(&old).unwrap(), library_before);
}

/// Two brought up to date at once in one app take turns: the second finds the first one's copy and has nothing to do.
#[test]
fn two_syncs_at_once_take_turns() {
    let dir = folder("turns");
    let old = library(&dir, 200);
    let copy = compact::copy_of(&old);
    let both: Vec<Synced> = std::thread::scope(|s| {
        let a = s.spawn(|| compact::sync(&old, &copy).unwrap());
        let b = s.spawn(|| compact::sync(&old, &copy).unwrap());
        vec![a.join().unwrap(), b.join().unwrap()]
    });
    assert_eq!(both.iter().filter(|s| s.made).count(), 1, "{both:?}");
    assert!(both.iter().any(|s| *s == Synced::default()), "{both:?}");
    assert_eq!(half_made(&dir), Vec::<String>::new());
    same_answers(&old, &copy);
}

/// Each save the C# app makes is noticed, even one that leaves `sounds.db` and its log the size they were with the
/// times they had: SQLite writes its log again from the start once it has copied it into the file, and Windows may
/// not move a file's time while the C# app holds it open.
#[test]
fn a_save_that_moves_no_size_or_time_is_noticed() {
    let dir = folder("unmoved");
    let old = library(&dir, 100);
    let copy = compact::copy_of(&old);
    let wal = dir.join("sounds.db-wal");
    // The C# app, open all along, as it is while it runs.
    let c = cs(&old);
    let rate = |stars: i64| {
        c.execute("UPDATE sounds SET rating = ?1 WHERE id = (SELECT MIN(id) FROM sounds)", [stars]).unwrap();
    };
    let checkpoint = || {
        let busy: i64 = c.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |r| r.get(0)).unwrap();
        assert_eq!(busy, 0, "nothing else holds the library");
    };
    c.execute_batch("PRAGMA wal_autocheckpoint = 0;").unwrap();
    // Two stars, then three: a 2 and a 3 take the same room in a row (a 0 or a 1 takes none), so from the second
    // save on the file keeps its size. The first may have moved the row.
    rate(2);
    checkpoint();
    rate(3);
    compact::sync(&old, &copy).unwrap();
    assert!(compact::in_step(&old, &copy));
    let looks = |p: &Path| {
        let m = std::fs::metadata(p).unwrap();
        (m.len(), m.modified().unwrap())
    };
    let (db_was, wal_was) = (looks(&old), looks(&wal));
    // Copied into the file, then the same save again: the log is written again from its start.
    checkpoint();
    rate(2);
    for (p, (_, at)) in [(&old, db_was), (&wal, wal_was)] {
        std::fs::File::options().write(true).open(p).unwrap().set_modified(at).unwrap();
    }
    assert_eq!((looks(&old), looks(&wal)), (db_was, wal_was), "the save left sizes and times as they were");
    assert!(!compact::in_step(&old, &copy), "the save is noticed all the same");
    assert!(compact::sync(&old, &copy).unwrap().made);
    assert!(compact::in_step(&old, &copy));
    same_answers(&old, &copy);
}

/// A copy another program holds to itself just then, beyond waiting for, is left as it is for the next time: it isn't
/// thrown away and made again (minutes on Solomon's disk).
#[test]
fn a_copy_busy_just_then_is_kept() {
    let dir = folder("busy");
    let old = library(&dir, 30);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    let holder = Connection::open(&copy).unwrap();
    holder
        .execute_batch(
            "PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE;
             UPDATE copy_info SET value = value WHERE key = 'format';",
        )
        .unwrap();
    let busy = compact::sync(&old, &copy).unwrap_err();
    assert!(busy.to_string().contains("couldn't be read just now"), "{busy}");
    // Asking whether it's up to date never waits.
    let asked = Instant::now();
    assert!(!compact::in_step(&old, &copy));
    assert!(asked.elapsed() < Duration::from_secs(2), "{:?}", asked.elapsed());
    drop(holder);
    assert!(compact::in_step(&old, &copy), "still the copy it was");
    assert_eq!(compact::sync(&old, &copy).unwrap(), Synced::default());
    same_answers(&old, &copy);
    // One something else has open (a page reading it) isn't replaced under it, nor waited for: it's made afresh the
    // next time.
    let page = SoundsDb::open_read_only(&copy).unwrap();
    let listed = format!("{:?}", page.all(now()).unwrap());
    cs(&old).execute_batch("UPDATE sounds SET plays = plays + 1 WHERE id = (SELECT MIN(id) FROM sounds);").unwrap();
    let before = std::fs::read(&copy).unwrap();
    let tried = Instant::now();
    let open = compact::sync(&old, &copy).unwrap_err().to_string();
    assert!(open.contains("has it open"), "{open}");
    assert!(tried.elapsed() < Duration::from_secs(3), "{:?}", tried.elapsed());
    assert!(std::fs::read(&copy).unwrap() == before, "the copy changed");
    assert_eq!(half_made(&dir), Vec::<String>::new());
    assert_eq!(format!("{:?}", page.all(now()).unwrap()), listed, "the page reads on");
    drop(page);
    assert!(compact::sync(&old, &copy).unwrap().made);
    same_answers(&old, &copy);
}

#[test]
fn a_library_that_isnt_one_makes_no_copy() {
    let dir = folder("not");
    let old = dir.join("sounds.db");
    Connection::open(&old).unwrap().execute_batch("CREATE TABLE other (x);").unwrap();
    let copy = compact::copy_of(&old);
    assert!(compact::sync(&old, &copy).is_err());
    assert!(!copy.exists());
    assert_eq!(half_made(&dir), Vec::<String>::new());
    // Nor is the copy taken for the library.
    let real = library(&folder("not-real"), 10);
    let real_copy = compact::copy_of(&real);
    compact::sync(&real, &real_copy).unwrap();
    assert!(compact::sync(&real_copy, &dir.join("copy of a copy.db")).is_err());
}

#[test]
fn sounds_db_is_never_changed() {
    let dir = folder("untouched");
    let old = library(&dir, 100);
    cs(&old).query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).unwrap();
    let before = std::fs::read(&old).unwrap();
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    cs(&old).execute("UPDATE sounds SET rating = 1", []).unwrap();
    cs(&old).query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).unwrap();
    let changed = std::fs::read(&old).unwrap();
    compact::sync(&old, &copy).unwrap();
    assert_ne!(before, changed);
    assert_eq!(std::fs::read(&old).unwrap(), changed, "reading it changed nothing");
}

/// With no copy yet, the page's read and the copy's are one pass over `sounds.db`: the page gets what its own read
/// gives, and the copy made on the same pass gives it too, and is the app's own library from then on. An update on
/// the same pass (of a copy made before the C# app was retired) likewise.
#[test]
fn the_page_reads_sounds_db_once_while_the_copy_is_made() {
    let dir = folder("one-pass");
    let old = library(&dir, 300);
    let copy = compact::copy_of(&old);
    let (e, synced) = compact::sync_reading(&old, &copy, now()).unwrap();
    let s = synced.unwrap();
    assert!(s.made && s.adopted);
    assert_eq!(s.sounds, e.sounds.len());
    same_everything(&e, &read_for_page(&old));
    assert!(compact::in_step(&old, &copy));
    same_answers(&old, &copy);
    // One made while the C# app still ran, which still follows sounds.db.
    for end in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{end}", copy.display()));
    }
    assert!(!compact::sync(&old, &copy).unwrap().adopted);
    cs(&old)
        .execute_batch(
            "UPDATE sounds SET plays = COALESCE(plays, 0) + 1
                 WHERE id IN (SELECT id FROM sounds WHERE id < (SELECT MAX(id) FROM sounds) LIMIT 7);
             DELETE FROM sounds WHERE id = (SELECT MAX(id) FROM sounds);",
        )
        .unwrap();
    let (e, synced) = compact::sync_reading(&old, &copy, now()).unwrap();
    let s = synced.unwrap();
    assert_eq!(s, Synced { made: true, sounds: 302, adopted: true, ..Synced::default() });
    same_everything(&e, &read_for_page(&old));
    assert!(compact::in_step(&old, &copy));
    same_answers(&old, &copy);
}

/// When no copy can be made (here another database is where it goes, which is never replaced), the page still gets
/// its sounds, and says why there's no copy. Without a library at all there's nothing to read.
#[test]
fn the_page_gets_its_sounds_when_no_copy_can_be_made() {
    let dir = folder("no-copy");
    let old = library(&dir, 40);
    let copy = compact::copy_of(&old);
    Connection::open(&copy).unwrap().execute_batch("CREATE TABLE notes (x); INSERT INTO notes VALUES (1);").unwrap();
    let (e, synced) = compact::sync_reading(&old, &copy, now()).unwrap();
    assert!(matches!(synced, Err(SoundsError::Copy { .. })), "{synced:?}");
    same_everything(&e, &read_for_page(&old));
    let notes: i64 =
        Connection::open(&copy).unwrap().query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0)).unwrap();
    assert_eq!(notes, 1, "the other database is left as it was");
    assert!(compact::sync_reading(&dir.join("none.db"), &dir.join("none-rust.db"), now()).is_err());
}

/// The page reads the copy's list first and its fingerprints after: what the likeness index keeps, the same from both
/// files. CLAP's are read only once deep listening has heard enough of the analysed sounds (the index would drop
/// them otherwise), counted as the index counts them: those of the most common length, a damaged one left out.
#[test]
fn the_fingerprints_come_apart_from_the_list() {
    let dir = folder("prints-apart");
    let old = library(&dir, 400);
    // A CLAP of another length (an older model's) and one that isn't whole floats: neither counts towards "enough".
    cs(&old)
        .execute_batch(&format!(
            "INSERT INTO sounds (path, analysis, clap) VALUES ('E:\\Old model.wav', '{{}}', x'{}');
             INSERT INTO sounds (path, analysis, clap) VALUES ('E:\\Torn.wav', '{{}}', x'0102030405');",
            "00".repeat(256 * 4)
        ))
        .unwrap();
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    let e = read_for_page(&old);
    let mut lengths = std::collections::HashMap::new();
    for (_, v) in e.clap.iter() {
        *lengths.entry(v.len()).or_insert(0usize) += 1;
    }
    let heard = lengths.values().copied().max().unwrap();
    assert!(heard < e.clap.len(), "the test needs CLAP of two lengths");
    // The most analysed sounds deep listening is "enough" for, and one more.
    let enough = (heard as f64 / 0.9).floor() as usize;
    assert!(mo_sounds::learn::heard_enough(heard, enough) && !mo_sounds::learn::heard_enough(heard, enough + 1));
    for path in [&old, &copy] {
        let db = SoundsDb::open_read_only(path).unwrap();
        // The copy keeps CLAP's as CLAP's index holds them.
        let kept = if db.is_copy() { e.clap.clone().into_held() } else { e.clap.clone() };
        for (analysed, deep) in [(enough, true), (enough + 1, false)] {
            let (features, clap) = db.fingerprints(analysed).unwrap();
            assert_eq!(features, e.features, "{analysed}");
            assert_eq!(clap, if deep { kept.clone() } else { Gathered::default() }, "{analysed}");
            let apart = SoundIndex::from_gathered(features, clap, analysed);
            let together = SoundIndex::from_gathered(e.features.clone(), e.clap.clone(), analysed);
            assert_eq!((apart.deep, apart.clap.is_some()), (deep, deep));
            assert_eq!(apart, together, "{analysed}");
        }
    }
}

/// The page reads the copy whenever there's one to read, at once even when it's behind (with the sounds as they were
/// when it was last brought up to date), and `sounds.db` only when there's none, making it on the same pass. A copy
/// a newer version of the app made isn't read (or touched): `sounds.db` is, and the page still gets its sounds.
#[test]
fn the_page_reads_the_copy_whenever_there_is_one() {
    let dir = folder("page-read");
    let old = library(&dir, 120);
    let copy = compact::copy_of(&old);
    let listed = |path: &Path| format!("{:?}", all_of(&SoundsDb::open_read_only(path).unwrap()));
    let Ok(PageRead::Library { everything, made }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("no copy yet, so sounds.db is read");
    };
    let made = made.unwrap();
    assert!(made.made && made.adopted, "{made:?}");
    assert!(compact::own_since(&copy).is_some());
    same_everything(&everything, &read_for_page(&old));
    let Ok(PageRead::Copy { db, sounds, behind, adopted }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read once it's there");
    };
    assert!(!behind);
    assert!(!db.is_formerly());
    assert_eq!(lay_out(&old).0, None, "of this version's format already");
    assert!(!adopted, "it became the app's own when it was made, on the page's pass");
    assert!(db.is_copy());
    assert_eq!(format!("{:?}", waved(&db, sounds)), listed(&old));
    drop(db);
    // A newer version's copy: left alone, and sounds.db is read instead.
    Connection::open(&copy).unwrap().execute("UPDATE copy_info SET value = 99 WHERE key = 'format'", []).unwrap();
    let Ok(PageRead::Library { everything, made }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("a newer copy isn't read");
    };
    assert!(made.is_err());
    same_everything(&everything, &read_for_page(&old));
    let format: i64 = Connection::open(&copy)
        .unwrap()
        .query_row("SELECT value FROM copy_info WHERE key = 'format'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(format, 99, "the newer copy is left as it is");
}

/// The C# app is retired (Solomon, 2026-11-05): a copy that's behind `sounds.db` is shown at once, brought up to date
/// one last time, and is the app's own sound library from then on. `sounds.db` is never read into it again, and once
/// it's gone the page still reads the copy.
#[test]
fn a_copy_behind_is_brought_up_to_date_one_last_time_and_is_the_apps_own_after() {
    let dir = folder("last-sync");
    let old = library(&dir, 90);
    let copy = compact::copy_of(&old);
    let listed = |path: &Path| format!("{:?}", all_of(&SoundsDb::open_read_only(path).unwrap()));
    // Made by the app before the C# app was retired: it still follows sounds.db.
    assert!(!compact::sync(&old, &copy).unwrap().adopted);
    assert_eq!(compact::own_since(&copy), None);
    let before = listed(&old);
    cs(&old)
        .execute("UPDATE sounds SET rating = 5 - COALESCE(rating, 0) WHERE id IN (SELECT id FROM sounds LIMIT 4)", [])
        .unwrap();
    assert_ne!(listed(&old), before);
    let Ok(PageRead::Copy { sounds, behind, adopted, .. }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read even when it's behind");
    };
    assert!(behind && !adopted);
    assert_eq!(format!("{:?}", waved(&SoundsDb::open_read_only(&copy).unwrap(), sounds)), before);
    assert_eq!(compact::own_since(&copy), None, "not the app's own while it's behind");
    // One last time.
    let last = compact::last_sync(&old, &copy).unwrap();
    assert_eq!(last, Synced { made: true, sounds: 93, adopted: true, ..Synced::default() });
    let since = compact::own_since(&copy).expect("the app's own now");
    same_answers(&old, &copy);
    let Ok(PageRead::Copy { sounds, behind, adopted, .. }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read");
    };
    assert!(!behind && !adopted);
    assert_eq!(format!("{:?}", waved(&SoundsDb::open_read_only(&copy).unwrap(), sounds)), listed(&old));
    // A save in the C# app after that: never read into the copy.
    let owned = listed(&copy);
    cs(&old)
        .execute("UPDATE sounds SET rating = 5 - COALESCE(rating, 0) WHERE id IN (SELECT id FROM sounds)", [])
        .unwrap();
    assert!(compact::in_step(&old, &copy), "nothing more to take from sounds.db");
    let Ok(PageRead::Copy { sounds, behind, .. }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read");
    };
    assert!(!behind);
    assert_eq!(format!("{:?}", waved(&SoundsDb::open_read_only(&copy).unwrap(), sounds)), owned);
    let refused = compact::sync(&old, &copy).unwrap_err().to_string();
    assert!(refused.contains("own sound library"), "{refused}");
    assert_eq!(compact::last_sync(&old, &copy).unwrap(), Synced::default());
    assert_eq!(listed(&copy), owned);
    assert_eq!(compact::own_since(&copy), Some(since));
    // sounds.db archived: the page reads the copy all the same.
    for end in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{end}", old.display()));
    }
    let Ok(PageRead::Copy { sounds, behind, .. }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read without sounds.db");
    };
    assert!(!behind);
    assert_eq!(format!("{:?}", waved(&SoundsDb::open_read_only(&copy).unwrap(), sounds)), owned);
}

/// `sounds.db` gone before the copy became the app's own (archived, or left in the old data folder when it moved): the
/// copy is all there is, so the page reads it as it is, nothing behind, and it becomes the app's own library.
#[test]
fn a_copy_left_without_sounds_db_becomes_the_apps_own_as_it_is() {
    let dir = folder("library-gone");
    let old = library(&dir, 40);
    let copy = compact::copy_of(&old);
    assert!(!compact::sync(&old, &copy).unwrap().adopted);
    let listed = |path: &Path| format!("{:?}", all_of(&SoundsDb::open_read_only(path).unwrap()));
    let made = listed(&copy);
    // The C# app saved once more after that, then sounds.db went.
    cs(&old)
        .execute("UPDATE sounds SET rating = 5 - COALESCE(rating, 0) WHERE id IN (SELECT id FROM sounds)", [])
        .unwrap();
    assert!(!compact::in_step(&old, &copy));
    for end in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{end}", old.display()));
    }
    assert!(compact::in_step(&old, &copy), "nothing to bring it up to date with");
    let Ok(PageRead::Copy { sounds, behind, adopted, .. }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read without sounds.db");
    };
    assert!(!behind && adopted);
    assert_eq!(format!("{:?}", waved(&SoundsDb::open_read_only(&copy).unwrap(), sounds)), made);
    assert!(compact::own_since(&copy).is_some());
}

/// A copy that's up to date becomes the app's own on the page's first read, once; made again because it's damaged, it
/// keeps the time it became so. Without `sounds.db` to make it again from, the damage is said, not hidden.
#[test]
fn an_up_to_date_copy_becomes_the_apps_own_and_stays_so_when_made_again() {
    let dir = folder("adopt");
    let old = library(&dir, 60);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    let Ok(PageRead::Copy { behind, adopted, .. }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read");
    };
    assert!(!behind && adopted);
    let since = compact::own_since(&copy).expect("the app's own now");
    let Ok(PageRead::Copy { adopted, .. }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read");
    };
    assert!(!adopted, "only once");
    // Damaged where its list is: made again from sounds.db, still the app's own since the same time.
    damage(&copy, "sounds");
    let Ok(PageRead::Library { everything, made }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("a damaged copy isn't read");
    };
    let made = made.unwrap();
    assert!(made.made && !made.adopted, "{made:?}");
    assert_eq!(compact::own_since(&copy), Some(since));
    same_everything(&everything, &read_for_page(&old));
    // Damaged again, with sounds.db gone: the page says why it can't read its sounds.
    damage(&copy, "sounds");
    for end in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{end}", old.display()));
    }
    assert!(compact::read_for_page(&old, now(), |_| {}).is_err());
}

/// A library with CLAP fingerprints of every kind `sounds.db` holds: of the most common length, of another (an older
/// model's), one that isn't whole floats, an empty one and text (the damaged rows the library has already).
fn library_with_odd_claps(dir: &Path, n: i64) -> PathBuf {
    let old = library(dir, n);
    cs(&old)
        .execute_batch(&format!(
            "INSERT INTO sounds (path, analysis, clap) VALUES ('E:\\Old model.wav', '{{}}', x'{}');
             INSERT INTO sounds (path, analysis, clap) VALUES ('E:\\Torn.wav', '{{}}', x'0102030405');
             INSERT INTO sounds (path, analysis, clap) VALUES ('E:\\Empty.wav', '{{}}', x'');",
            "0000803f".repeat(256)
        ))
        .unwrap();
    old
}

/// A copy of the format before is read as it is, then laid out anew from itself once the page is done with it, even
/// with `sounds.db` gone: it comes out the very file this version makes from the same `sounds.db`, table for table and
/// row for row, but that it's still the app's own library since the time it was. Smaller, and nothing left beside it.
#[test]
fn a_copy_of_the_format_before_is_laid_out_anew_from_itself() {
    // A folder name SQLite's URIs must spell out (the copy is named to SQLite as one).
    let dir = folder("lay-out #1 % é");
    let old = library_with_odd_claps(&dir, 700);
    let copy = compact::copy_of(&old);
    formerly(&old, &copy, Some(1_780_000_000));
    // Read as it is, it gives every answer sounds.db gives, CLAP's fingerprints as CLAP gave them.
    assert!(SoundsDb::open_read_only(&copy).unwrap().is_formerly());
    same_answers(&old, &copy);
    // What this version makes from the same sounds.db, to compare with.
    let fresh = dir.join("fresh").join(compact::FILE_NAME);
    std::fs::create_dir_all(fresh.parent().unwrap()).unwrap();
    compact::sync(&old, &fresh).unwrap();
    let listed = format!("{:?}", all_of(&SoundsDb::open_read_only(&old).unwrap()));
    let before = std::fs::metadata(&copy).unwrap().len();
    // sounds.db archived: the copy is all there is.
    let archived = dir.join("archived.db");
    std::fs::rename(&old, &archived).unwrap();
    remove_db(&old);
    let Ok(PageRead::Copy { db, sounds, behind, adopted }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read");
    };
    assert!(!behind && !adopted);
    assert!(db.is_formerly());
    assert_eq!(format!("{sounds:?}"), listed);
    // Not while the page has it open: nor is the work started.
    let (Some(LaidOut::NotNow(why)), false) = lay_out(&old) else { panic!("not laid out while it's open") };
    assert!(why.contains("has it open"), "{why}");
    assert_eq!(format_of(&copy), 1);
    drop(db);
    let (laid_out, started) = lay_out(&old);
    assert!(started);
    let Some(LaidOut::Done { before: was, after }) = laid_out else { panic!("laid out anew: {laid_out:?}") };
    let db = SoundsDb::open_read_only(&copy).unwrap();
    assert!(db.is_copy() && !db.is_formerly());
    assert_eq!(format!("{:?}", all_of(&db)), listed);
    assert_eq!(was, before);
    assert!(after * 100 < before * 85, "{after} from {before}");
    assert_eq!(after, std::fs::metadata(&copy).unwrap().len());
    drop(db);
    same_files(&copy, &fresh);
    // Its own notes: what it was made from, kept, and since when it's the app's own; no hashes.
    let info = "SELECT key, value FROM copy_info ORDER BY key";
    let mut wanted = rows(&fresh, info);
    wanted.push(format!("{:?} | {:?}", ValueRef::Text(b"own_since"), ValueRef::Integer(1_780_000_000)));
    wanted.sort();
    let mut got = rows(&copy, info);
    got.sort();
    assert_eq!(got, wanted);
    assert_eq!(compact::own_since(&copy), Some(1_780_000_000));
    assert!(compact::in_step(&old, &copy));
    let c = Connection::open(&copy).unwrap();
    assert_eq!(c.query_row("PRAGMA page_size", [], |r| r.get::<_, i64>(0)).unwrap(), 32_768);
    assert_eq!(c.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0)).unwrap(), "wal");
    assert_eq!(c.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)).unwrap(), "ok");
    drop(c);
    assert_eq!(half_made(&dir), Vec::<String>::new());
    // Once is enough.
    assert_eq!(lay_out(&old), (None, false));
    // And it still gives every answer sounds.db gave.
    std::fs::rename(&archived, &old).unwrap();
    same_answers(&old, &copy);
}

/// A copy of the format before is read as it is, CLAP's fingerprints and all, and isn't laid out anew while another
/// program has it open: it is once that has let go of it. Up to date with `sounds.db`, it becomes the app's own on the
/// page's first read.
#[test]
fn a_copy_of_the_format_before_that_another_program_has_open_is_read_as_it_is() {
    let dir = folder("lay-out-open");
    let old = library_with_odd_claps(&dir, 200);
    let copy = compact::copy_of(&old);
    formerly(&old, &copy, None);
    let other = Connection::open(&copy).unwrap();
    other.query_row("SELECT COUNT(*) FROM sounds", [], |r| r.get::<_, i64>(0)).unwrap();
    let e = read_for_page(&old);
    let Ok(PageRead::Copy { db, sounds, behind, adopted }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read");
    };
    assert!(!behind && adopted);
    assert!(db.is_formerly());
    assert_eq!(format!("{sounds:?}"), format!("{:?}", e.sounds));
    // However few sounds are analysed: CLAP's are read.
    let (features, clap) = db.fingerprints(0).unwrap();
    assert_eq!((features, clap), (e.features.clone(), e.clap.clone()));
    assert_eq!(db.count_with_clap().unwrap(), SoundsDb::open_read_only(&old).unwrap().count_with_clap().unwrap());
    drop(db);
    let Some(LaidOut::NotNow(why)) = lay_out(&old).0 else { panic!("not laid out while it's open") };
    assert!(why.contains("has it open"), "{why}");
    assert_eq!(format_of(&copy), 1);
    assert_eq!(half_made(&dir), Vec::<String>::new());
    drop(other);
    let since = compact::own_since(&copy).expect("the app's own now");
    let laid_out = lay_out(&old).0;
    assert!(matches!(laid_out, Some(LaidOut::Done { .. })), "{laid_out:?}");
    let Ok(PageRead::Copy { db, adopted, .. }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read");
    };
    assert!(!adopted && !db.is_formerly());
    drop(db);
    assert_eq!(compact::own_since(&copy), Some(since));
    same_answers(&old, &copy);
}

/// A copy of the format before that's behind `sounds.db` isn't laid out anew: it's shown as it is, then made afresh
/// in this version's format by the last bringing up to date.
#[test]
fn a_copy_of_the_format_before_that_is_behind_is_made_afresh() {
    let dir = folder("lay-out-behind");
    let old = library(&dir, 120);
    let copy = compact::copy_of(&old);
    formerly(&old, &copy, None);
    let listed = format!("{:?}", all_of(&SoundsDb::open_read_only(&old).unwrap()));
    cs(&old)
        .execute("UPDATE sounds SET rating = 5 - COALESCE(rating, 0) WHERE id IN (SELECT id FROM sounds LIMIT 4)", [])
        .unwrap();
    let Ok(PageRead::Copy { db, sounds, behind, adopted }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read");
    };
    assert!(behind && !adopted);
    assert!(db.is_formerly());
    assert_eq!(format!("{sounds:?}"), listed);
    drop(db);
    assert_eq!(lay_out(&old).0, None, "one that's behind is made afresh instead");
    assert_eq!(format_of(&copy), 1);
    assert_eq!(
        compact::last_sync(&old, &copy).unwrap(),
        Synced { made: true, sounds: 123, adopted: true, ..Synced::default() }
    );
    assert_eq!(format_of(&copy), 2);
    same_answers(&old, &copy);
}

/// A copy of the format before that holds a table this version doesn't know (one a later version added, say) is left
/// as it is, to the last byte, and read as it is: laying it out anew would lose what's in that table.
#[test]
fn a_copy_of_the_format_before_with_a_table_this_version_doesnt_know_is_left_as_it_is() {
    let dir = folder("lay-out-unknown");
    let old = library(&dir, 60);
    let copy = compact::copy_of(&old);
    formerly(&old, &copy, Some(1_780_000_000));
    Connection::open(&copy)
        .unwrap()
        .execute_batch("CREATE TABLE later_notes (x); INSERT INTO later_notes VALUES (1);")
        .unwrap();
    let before = std::fs::read(&copy).unwrap();
    let Ok(PageRead::Copy { db, .. }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read");
    };
    assert!(db.is_formerly());
    drop(db);
    let laid_out = lay_out(&old).0;
    let Some(LaidOut::NotNow(why)) = laid_out else { panic!("not laid out: {laid_out:?}") };
    assert!(why.contains("later_notes"), "{why}");
    assert!(std::fs::read(&copy).unwrap() == before, "the copy changed");
    assert_eq!(half_made(&dir), Vec::<String>::new());
    same_answers(&old, &copy);
}

/// A copy of the format before found damaged as it's laid out anew is left as it was, and says so: the page makes it
/// again from `sounds.db` while that's there and it has no backup copy ([`compact::mend_reading`]), still the app's own
/// since the same time.
#[test]
fn a_damaged_copy_of_the_format_before_is_left_as_it_was() {
    let dir = folder("lay-out-damaged");
    let old = library(&dir, 150);
    let copy = compact::copy_of(&old);
    formerly(&old, &copy, Some(1_780_000_000));
    damage(&copy, "sound_clap");
    let Ok(PageRead::Copy { db, sounds, .. }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read as it is: the list isn't damaged");
    };
    assert!(db.is_formerly());
    assert_eq!(sounds.len(), 153);
    // However few sounds are analysed: CLAP's are read.
    let prints = db.fingerprints(0).unwrap_err();
    assert!(prints.damaged(), "{prints}");
    drop(db);
    let before = std::fs::read(&copy).unwrap();
    let laid_out = lay_out(&old).0;
    let Some(LaidOut::Damaged(why)) = laid_out else { panic!("found damaged: {laid_out:?}") };
    assert!(why.contains("malformed"), "{why}");
    assert!(std::fs::read(&copy).unwrap() == before, "the copy changed");
    assert_eq!(half_made(&dir), Vec::<String>::new());
    let (everything, made) = compact::mend_reading(&old, &copy, now()).unwrap();
    // No backup copy: made again from sounds.db, which says why.
    let made = made.unwrap();
    assert_eq!((made.made, made.sounds, made.adopted, made.from_backup), (true, 153, false, None));
    assert_eq!(made.backup_unused.as_deref(), Some("there's no backup copy"));
    same_everything(&everything, &read_for_page(&old));
    assert_eq!(format_of(&copy), 2);
    assert_eq!(compact::own_since(&copy), Some(1_780_000_000));
    same_answers(&old, &copy);
    // Again, with sounds.db gone: still left as it was, the list read as it is.
    formerly(&old, &copy, Some(1_780_000_000));
    damage(&copy, "sound_clap");
    remove_db(&old);
    assert!(matches!(lay_out(&old).0, Some(LaidOut::Damaged(_))));
    let Ok(PageRead::Copy { db, sounds, .. }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("the copy is read as it is");
    };
    assert!(db.is_formerly());
    assert_eq!(sounds.len(), 153);
    // However few sounds are analysed: CLAP's are read.
    let prints = db.fingerprints(0).unwrap_err();
    assert!(prints.damaged(), "{prints}");
    drop(db);
    assert_eq!(format_of(&copy), 1);
    assert_eq!(half_made(&dir), Vec::<String>::new());
}

/// A table whose rows count up (AUTOINCREMENT) keeps SQLite's count of them as it was, past its last row, through
/// being laid out anew; and nothing another copy of the app writes meanwhile is lost.
#[test]
fn a_count_of_rows_and_a_save_meanwhile_are_kept() {
    let dir = folder("lay-out-count");
    let old = library(&dir, 80);
    let copy = compact::copy_of(&old);
    formerly(&old, &copy, Some(1_780_000_000));
    Connection::open(&copy)
        .unwrap()
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             CREATE TABLE counted (sound_id INTEGER PRIMARY KEY AUTOINCREMENT, batch TEXT NOT NULL, at INTEGER NOT NULL);
             INSERT INTO counted SELECT * FROM sound_hidden;
             DROP TABLE sound_hidden;
             ALTER TABLE counted RENAME TO sound_hidden;
             UPDATE sqlite_sequence SET seq = 9999999 WHERE name = 'sound_hidden';",
        )
        .unwrap();
    let hidden = SoundsDb::open_read_only(&copy).unwrap().hidden().unwrap();
    let count = "SELECT name, seq FROM sqlite_sequence ORDER BY name";
    let counted = rows(&copy, count);
    assert_eq!(counted.len(), 1);
    let (laid_out, _) = lay_out(&old);
    assert!(matches!(laid_out, Some(LaidOut::Done { .. })), "{laid_out:?}");
    assert_eq!(rows(&copy, count), counted);
    assert_eq!(SoundsDb::open_read_only(&copy).unwrap().hidden().unwrap(), hidden);
    // Another copy of the app saving into it while it's laid out anew: it's left as it was, save and all, and laid out
    // anew the next time.
    formerly(&old, &copy, Some(1_780_000_000));
    let mut saved = false;
    let laid_out = compact::lay_out(&old, || {
        Connection::open(&copy).unwrap().execute("INSERT INTO copy_info VALUES ('saved', 1)", []).unwrap();
        saved = true;
    });
    assert!(saved);
    let Some(LaidOut::NotNow(why)) = laid_out else { panic!("not laid out: {laid_out:?}") };
    assert!(why.contains("wrote to it meanwhile"), "{why}");
    assert_eq!(format_of(&copy), 1);
    assert_eq!(half_made(&dir), Vec::<String>::new());
    assert!(matches!(lay_out(&old).0, Some(LaidOut::Done { .. })));
    let saved: i64 = Connection::open(&copy)
        .unwrap()
        .query_row("SELECT value FROM copy_info WHERE key = 'saved'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(saved, 1);
}

#[test]
fn a_few_sounds_read_on_their_own_are_as_the_whole_list_gives_them() {
    let dir = folder("a few on their own");
    let old = library(&dir, 1_200);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    formerly(&old, &dir.join("formerly.db"), None);
    for path in [old.clone(), copy, dir.join("formerly.db")] {
        let db = SoundsDb::open_read_only(&path).unwrap();
        // With their waveforms, which the app's own library reads apart from the list.
        let all = waved(&db, db.all(now()).unwrap());
        let hidden = db.hidden().unwrap();
        assert!(!hidden.is_empty());
        // Every sound but the hidden copies.
        let mut ids = db.shown_ids().unwrap();
        ids.sort_unstable();
        let mut shown: Vec<i64> = all.iter().filter(|s| !s.hidden).map(|s| s.id).collect();
        shown.sort_unstable();
        assert_eq!(ids, shown, "{}", path.display());
        // Every other one (more than SQLite is asked for at once), one it doesn't have and a hidden one: each as the
        // whole list has it, in its order.
        let mut picked: Vec<i64> = ids.iter().step_by(2).copied().collect();
        assert!(picked.len() > 500);
        picked.extend([999_999, *hidden.iter().next().unwrap()]);
        picked.reverse();
        let some = db.some(&picked, now()).unwrap();
        let expected: Vec<&mo_sounds::Sound> = all.iter().filter(|s| picked.contains(&s.id)).collect();
        assert_eq!(some.len(), picked.len() - 1);
        assert_eq!(some.len(), expected.len());
        assert!(some.iter().any(|s| s.hidden) && some.iter().any(|s| !s.tags.is_empty()));
        assert!(some.iter().any(|s| s.activity != mo_sounds::SoundActivity::default()));
        for (a, b) in some.iter().zip(expected) {
            assert_eq!(format!("{a:?}"), format!("{b:?}"), "{}", path.display());
        }
    }
}

/// The app's own library keeps each sound's waveform in a table of its own, and its list comes without them: they're
/// read for the sounds on show, each as `sounds.db` has it. `sounds.db` and a copy of the format before give them with
/// the list, and the same ones on their own.
#[test]
fn only_the_apps_own_library_reads_the_waveforms_apart_from_the_list() {
    let dir = folder("waves apart");
    let old = library(&dir, 1_200);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    formerly(&old, &dir.join("formerly.db"), None);
    let listed = SoundsDb::open_read_only(&old).unwrap().all(now()).unwrap();
    let wanted: std::collections::HashMap<i64, mo_sounds::Peaks> = listed
        .iter()
        .filter_map(|s| Some((s.id, s.analysis.as_ref()?.waveform.clone())))
        .filter(|(_, wave)| !wave.is_empty())
        .collect();
    // More than SQLite is asked for at once, a sound never analysed and one that isn't there.
    assert!(wanted.len() > 500 && wanted.len() < listed.len());
    let mut ids: Vec<i64> = listed.iter().map(|s| s.id).collect();
    ids.push(999_999);
    for (path, apart) in [(old.clone(), false), (copy, true), (dir.join("formerly.db"), false)] {
        let db = SoundsDb::open_read_only(&path).unwrap();
        assert_eq!(db.waves_apart(), apart, "{}", path.display());
        let with_waves = |sounds: &[mo_sounds::Sound]| {
            sounds.iter().filter(|s| s.analysis.as_ref().is_some_and(|a| !a.waveform.is_empty())).count()
        };
        assert_eq!(with_waves(&db.all(now()).unwrap()), if apart { 0 } else { wanted.len() }, "{}", path.display());
        assert_eq!(with_waves(&db.everything(now()).unwrap().sounds), if apart { 0 } else { wanted.len() });
        let waves: std::collections::HashMap<i64, mo_sounds::Peaks> = db.waves(&ids).unwrap().into_iter().collect();
        assert_eq!(waves, wanted, "{}", path.display());
        assert!(db.waves(&[]).unwrap().is_empty());
    }
}

/// A change written to the library, as the app writes them: `id`'s stars, one more (after five, none). What they are
/// now. (A save that changes nothing leaves the file as it was.)
fn changed(copy: &Path, id: i64) -> i64 {
    let rating = (rating_of(copy, id) + 1) % 6;
    let c = Connection::open(copy).unwrap();
    c.execute("UPDATE sounds SET rating = ?1 WHERE id = ?2", params![rating, id]).unwrap();
    rating
}

/// Sets the time the file at `path` was last written.
fn set_time(path: &Path, at: SystemTime) {
    std::fs::File::options().write(true).open(path).unwrap().set_modified(at).unwrap();
}

fn age_of(path: &Path) -> Duration {
    SystemTime::now().duration_since(std::fs::metadata(path).unwrap().modified().unwrap()).unwrap_or_default()
}

/// What SQLite's `PRAGMA name` says of the database at `db`, as text.
fn pragma(db: &Path, name: &str) -> String {
    Connection::open(db).unwrap().query_row(&format!("PRAGMA {name}"), [], |r| r.get(0)).unwrap()
}

fn rating_of(db: &Path, id: i64) -> i64 {
    Connection::open(db).unwrap().query_row("SELECT rating FROM sounds WHERE id = ?1", [id], |r| r.get(0)).unwrap()
}

/// The app's own sound library has one backup copy, made when there's none or the library has changed since, and
/// checked whole; a library that isn't the app's own yet has none.
#[test]
fn the_apps_own_library_keeps_one_checked_backup_copy() {
    let dir = folder("backup-made");
    let old = library(&dir, 120);
    let copy = compact::copy_of(&old);
    let backup = backup::backup_of(&copy);
    compact::sync(&old, &copy).unwrap();
    // Still following sounds.db: sounds.db is its backup.
    assert_eq!(backup::back_up(&old), None);
    assert!(!backup.exists());
    compact::last_sync(&old, &copy).unwrap();
    assert!(compact::own_since(&copy).is_some());
    let Some(BackedUp::Made { bytes, .. }) = backup::back_up(&old) else { panic!("made") };
    assert_eq!(bytes, std::fs::metadata(&backup).unwrap().len());
    assert_eq!(half_made(&dir), Vec::<String>::new());
    same_answers(&old, &backup);
    // A file of its own (no log beside it), the same size of page.
    assert_eq!(pragma(&backup, "journal_mode"), "delete");
    assert_eq!(rows(&backup, "PRAGMA page_size"), rows(&copy, "PRAGMA page_size"));
    // Up to date: not made again.
    let before = std::fs::read(&backup).unwrap();
    assert_eq!(backup::back_up(&old), None);
    assert!(std::fs::read(&backup).unwrap() == before);
    // Changed: made again, with the change.
    let id: i64 = Connection::open(&copy).unwrap().query_row("SELECT MIN(id) FROM sounds", [], |r| r.get(0)).unwrap();
    let stars = changed(&copy, id);
    assert!(matches!(backup::back_up(&old), Some(BackedUp::Made { .. })));
    assert_eq!(rating_of(&backup, id), stars);
    assert_eq!(backup::back_up(&old), None);
    // Only reading the library changes nothing.
    let _ = read_for_page(&copy);
    assert_eq!(rows(&copy, "SELECT COUNT(*) FROM sounds"), rows(&backup, "SELECT COUNT(*) FROM sounds"));
    assert_eq!(backup::back_up(&old), None);
    // One not looked at for a week is checked again: left as it is while it's whole, and its time set to now.
    let a_week_ago = SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60);
    set_time(&backup, a_week_ago);
    assert_eq!(backup::back_up(&old), None);
    assert!(age_of(&backup) < Duration::from_secs(60));
    // Damaged: made again then, though the library hasn't changed (only then: it isn't checked every time).
    damage(&backup, "sounds");
    assert_eq!(backup::back_up(&old), None);
    set_time(&backup, a_week_ago);
    assert!(matches!(backup::back_up(&old), Some(BackedUp::Made { .. })));
    assert_eq!(rating_of(&backup, id), stars);
    assert_eq!(pragma(&backup, "integrity_check"), "ok");
    // Changed and damaged: made again.
    damage(&backup, "sounds");
    let stars = changed(&copy, id);
    assert!(matches!(backup::back_up(&old), Some(BackedUp::Made { .. })));
    assert_eq!(rating_of(&backup, id), stars);
    assert_eq!(pragma(&backup, "integrity_check"), "ok");
    assert_eq!(half_made(&dir), Vec::<String>::new());
}

/// A library so damaged nothing can be read from it (its first page) is put back from its backup copy, not made again
/// from sounds.db, which would lose what the app wrote into it; nothing makes it again from sounds.db unasked.
#[test]
fn a_library_nothing_can_be_read_from_is_mended_from_its_backup_copy() {
    use std::io::{Seek, SeekFrom, Write};
    let dir = folder("backup-unreadable");
    let old = library(&dir, 90);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    compact::last_sync(&old, &copy).unwrap();
    let id: i64 = Connection::open(&copy).unwrap().query_row("SELECT MIN(id) FROM sounds", [], |r| r.get(0)).unwrap();
    let stars = changed(&copy, id);
    assert!(matches!(backup::back_up(&old), Some(BackedUp::Made { .. })));
    // Its first page overwritten.
    let mut f = std::fs::File::options().write(true).open(&copy).unwrap();
    f.seek(SeekFrom::Start(0)).unwrap();
    f.write_all(&[0xEE; 100]).unwrap();
    drop(f);
    let Err(e) = compact::sync(&old, &copy) else { panic!("not made again from sounds.db") };
    assert!(e.to_string().contains("backup copy"), "{e}");
    assert!(!backup::damaged_of(&copy).exists());
    let Ok(PageRead::Library { made, .. }) = compact::read_for_page(&old, now(), |_| {}) else { panic!("mended") };
    let made = made.unwrap();
    assert!(made.from_backup.is_some(), "{made:?}");
    assert_eq!(rating_of(&copy, id), stars);
    assert!(backup::damaged_of(&copy).exists());
}

/// A damaged library its backup copy can't be put back into just now (something else has it open) is left as it is,
/// for the next time: never made again from sounds.db meanwhile.
#[test]
fn a_damaged_library_held_open_is_left_for_the_next_time() {
    let dir = folder("backup-held");
    let old = library(&dir, 60);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    compact::last_sync(&old, &copy).unwrap();
    let id: i64 = Connection::open(&copy).unwrap().query_row("SELECT MIN(id) FROM sounds", [], |r| r.get(0)).unwrap();
    let stars = changed(&copy, id);
    assert!(matches!(backup::back_up(&old), Some(BackedUp::Made { .. })));
    damage(&copy, "sounds");
    let holder = Connection::open(&copy).unwrap();
    holder.query_row("SELECT COUNT(*) FROM sqlite_master", [], |_| Ok(())).unwrap();
    let Err(e) = compact::read_for_page(&old, now(), |_| {}) else { panic!("left as it is") };
    assert!(matches!(&e, SoundsError::Damaged { path, .. } if *path == copy), "{e}");
    assert!(e.to_string().contains("something else has it open"), "{e}");
    assert!(!backup::damaged_of(&copy).exists());
    assert!(compact::own_since(&copy).is_some());
    drop(holder);
    let Ok(PageRead::Library { made, .. }) = compact::read_for_page(&old, now(), |_| {}) else { panic!("mended") };
    assert!(made.unwrap().from_backup.is_some());
    assert_eq!(rating_of(&copy, id), stars);
    assert_eq!(half_made(&dir), Vec::<String>::new());
}

/// A library found damaged is put back from its backup copy, with what was written up to then, even with sounds.db
/// gone; the damaged one is set aside, not thrown away. Without a backup copy it's made again from sounds.db while
/// that's there, and with neither the page is told the library itself is damaged (review finding B-1).
#[test]
fn a_damaged_library_is_mended_from_its_backup_copy() {
    let dir = folder("backup-mend");
    let old = library(&dir, 140);
    let copy = compact::copy_of(&old);
    let backup = backup::backup_of(&copy);
    compact::sync(&old, &copy).unwrap();
    compact::last_sync(&old, &copy).unwrap();
    let since = compact::own_since(&copy).unwrap();
    let id: i64 = Connection::open(&copy).unwrap().query_row("SELECT MIN(id) FROM sounds", [], |r| r.get(0)).unwrap();
    let stars = changed(&copy, id);
    assert!(matches!(backup::back_up(&old), Some(BackedUp::Made { .. })));
    let kept = read_for_page(&copy);
    // sounds.db gone, and the library's list damaged: put back from the backup copy, the change and all.
    remove_db(&old);
    damage(&copy, "sounds");
    let Ok(PageRead::Library { everything, made }) = compact::read_for_page(&old, now(), |_| {}) else {
        panic!("put back from its backup copy");
    };
    let made = made.unwrap();
    assert!(made.made && made.from_backup.is_some(), "{made:?}");
    same_everything(&everything, &kept);
    assert_eq!(rating_of(&copy, id), stars);
    assert_eq!(compact::own_since(&copy), Some(since));
    assert_eq!(pragma(&copy, "journal_mode"), "wal");
    assert_eq!(pragma(&copy, "integrity_check"), "ok");
    // The damaged one is beside it, and the backup copy is as it was.
    let damaged = backup::damaged_of(&copy);
    assert!(damaged.exists());
    assert_ne!(pragma(&damaged, "quick_check"), "ok");
    assert_eq!(half_made(&dir), Vec::<String>::new());
    // Its fingerprints damaged: the same, on the page's pass that gives them.
    damage(&copy, "sound_features");
    let (again, made) = compact::mend_reading(&old, &copy, now()).unwrap();
    assert!(made.unwrap().from_backup.is_some());
    same_everything(&again, &kept);
    // No backup copy, and no sounds.db: the library itself is named, as damaged.
    damage(&copy, "sounds");
    remove_db(&backup);
    let Err(e) = compact::read_for_page(&old, now(), |_| {}) else { panic!("nothing to mend it from") };
    assert!(matches!(&e, SoundsError::Damaged { path, .. } if *path == copy), "{e}");
    assert!(e.damaged());
    let said = e.to_string();
    assert!(said.contains(compact::FILE_NAME) && said.contains("there's no backup copy"), "{said}");
}

/// What listening found for a test sound: `seconds` long, with a small waveform and features.
fn heard(seconds: f64) -> mo_sounds::analyse::Analysed {
    mo_sounds::analyse::Analysed {
        analysis: SoundAnalysis {
            seconds,
            sample_rate: 44_100,
            channel_count: 2,
            waveform: mo_sounds::Peaks::new(&[0.25, 0.5, 1.0, 0.125]),
            ..SoundAnalysis::default()
        },
        features: vec![0.5; 73],
    }
}

fn id_of(copy: &Path, path: &str) -> Option<i64> {
    use rusqlite::OptionalExtension;
    Connection::open(copy)
        .unwrap()
        .query_row("SELECT id FROM sounds WHERE path = ?1", [path], |r| r.get(0))
        .optional()
        .unwrap()
}

fn count(db: &Path, sql: &str, id: i64) -> i64 {
    Connection::open(db).unwrap().query_row(sql, [id], |r| r.get(0)).unwrap()
}

/// What a scan found is kept in the app's own sound library (stage 2, review finding X1-own1): new sounds added with
/// what listening found and the type their name says; changed ones listened to again, keeping your stars and tags;
/// moved ones followed, keeping theirs; gone ones forgotten with theirs. A forgotten sound's number is never given
/// again. A library that isn't the app's own yet isn't written.
#[test]
fn what_a_scan_found_is_kept_in_the_apps_own_library() {
    let dir = folder("grow");
    let old = library(&dir, 60);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    let new_kick = "D:\\Samples\\New Pack\\Kick 01.wav".to_owned();
    let listened =
        |path: &str, size, analysed| Listened { path: path.to_owned(), size, mtime: unix(2026, 11, 6), analysed };
    // Still following sounds.db: not written.
    let Err(e) = grow::keep(&old, &[listened(&new_kick, 1000, Ok(heard(0.5)))]) else {
        panic!("not the app's own yet")
    };
    assert!(e.to_string().contains("isn't the app's own"), "{e}");
    compact::last_sync(&old, &copy).unwrap();
    // Rows the fixture leaves pointing at no sound (as a damaged sounds.db might): none more after.
    let orphans = rows(&copy, "PRAGMA foreign_key_check");
    let ids: Vec<(i64, String)> = {
        let c = Connection::open(&copy).unwrap();
        let mut st = c.prepare("SELECT id, path FROM sounds ORDER BY id LIMIT 3").unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
    };
    let [(changed_id, changed), (moved_id, moved), (gone_id, gone)] = ids.as_slice() else { panic!("three sounds") };
    {
        let c = Connection::open(&copy).unwrap();
        for id in [changed_id, moved_id, gone_id] {
            c.execute("INSERT OR IGNORE INTO sound_tags (sound_id, tag, source) VALUES (?1, 'keep', 'you')", [id])
                .unwrap();
            c.execute("UPDATE sounds SET rating = 4 WHERE id = ?1", [id]).unwrap();
        }
    }
    let highest: i64 =
        Connection::open(&copy).unwrap().query_row("SELECT MAX(id) FROM sounds", [], |r| r.get(0)).unwrap();
    let before = backup::back_up(&old);
    assert!(matches!(before, Some(BackedUp::Made { .. })), "{before:?}");
    // New, unreadable and changed.
    let broken = "D:\\Samples\\New Pack\\broken.wav".to_owned();
    let kept = grow::keep(
        &old,
        &[
            listened(&new_kick, 1000, Ok(heard(0.5))),
            listened(&broken, 10, Err("Couldn't read the file: unsupported format".into())),
            listened(changed, 2000, Ok(heard(2.0))),
        ],
    )
    .unwrap();
    assert_eq!(kept, Kept { added: 2, updated: 1, ..Kept::default() });
    let sounds = SoundsDb::open_read_only(&copy).unwrap().all(now()).unwrap();
    let find = |path: &str| sounds.iter().find(|s| s.path == path).unwrap_or_else(|| panic!("{path}")).clone();
    let kick = find(&new_kick);
    assert!(kick.id > highest);
    assert_eq!(kick.analysis.as_ref().map(|a| a.seconds), Some(0.5));
    // Its waveform kept apart, read when it's shown.
    let waves = SoundsDb::open_read_only(&copy).unwrap().waves(&[kick.id]).unwrap();
    assert_eq!(waves, vec![(kick.id, heard(0.5).analysis.waveform)]);
    let named = mo_sounds::types::from_path(&new_kick);
    assert_eq!(kick.name_type.as_deref(), named);
    assert_eq!(kick.sound_type.as_str(), named.unwrap_or("Unknown"));
    assert_eq!((kick.rating, kick.plays, kick.favourite, kick.tags.len()), (0, 0, false, 0));
    let unreadable = find(&broken);
    assert_eq!(unreadable.error.as_deref(), Some("Couldn't read the file: unsupported format"));
    assert!(unreadable.analysis.is_none());
    let again = find(changed);
    assert_eq!(again.id, *changed_id);
    assert_eq!(again.analysis.as_ref().map(|a| a.seconds), Some(2.0));
    assert_eq!(again.rating, 4);
    assert!(again.tags.contains(&"keep".to_owned()), "{:?}", again.tags);
    // What the scan plans from, a damaged row too (its size and time aren't numbers: listened to again).
    let known = SoundsDb::open_read_only(&copy).unwrap().known_files().unwrap();
    assert!(known.iter().any(|k| k.path == *changed && k.size == 2000 && k.mtime == unix(2026, 11, 6)));
    assert!(known.iter().any(|k| k.path == "D:\\Damaged\\two.wav" && k.size == 0 && k.mtime == 0));
    assert_eq!(count(&copy, "SELECT size FROM sounds WHERE id = ?1", *changed_id), 2000);
    assert_eq!(count(&copy, "SELECT mtime FROM sounds WHERE id = ?1", *changed_id), unix(2026, 11, 6));
    assert_eq!(count(&copy, "SELECT COUNT(*) FROM sound_features WHERE sound_id = ?1", kick.id), 1);
    assert_eq!(count(&copy, "SELECT COUNT(*) FROM sound_features WHERE sound_id = ?1", unreadable.id), 0);
    // Moved and gone.
    let to = "E:\\Moved\\here.wav".to_owned();
    let tagged = count(&copy, "SELECT COUNT(*) FROM sound_tags WHERE sound_id = ?1", *moved_id);
    assert!(tagged > 0);
    let kept = grow::follow(&old, &[(moved.clone(), to.clone())], &[gone.clone(), kick.path.clone()]).unwrap();
    assert_eq!(kept, Kept { moved: 1, forgotten: 2, ..Kept::default() });
    assert_eq!(id_of(&copy, &to), Some(*moved_id));
    assert_eq!(id_of(&copy, moved), None);
    assert_eq!(count(&copy, "SELECT COUNT(*) FROM sound_tags WHERE sound_id = ?1", *moved_id), tagged);
    assert_eq!(id_of(&copy, gone), None);
    for table in ["sound_tags", "sound_waves", "sound_features"] {
        assert_eq!(count(&copy, &format!("SELECT COUNT(*) FROM {table} WHERE sound_id = ?1"), *gone_id), 0, "{table}");
    }
    // The kick forgotten and added again: a number of its own, not the one it had.
    grow::keep(&old, &[listened(&new_kick, 1000, Ok(heard(0.5)))]).unwrap();
    assert!(id_of(&copy, &new_kick).unwrap() > kick.id);
    assert_eq!(pragma(&copy, "integrity_check"), "ok");
    assert!(rows(&copy, "PRAGMA foreign_key_check").iter().all(|r| orphans.contains(r)));
    // The backup copy follows.
    assert!(matches!(backup::back_up(&old), Some(BackedUp::Made { .. })));
    assert!(id_of(&backup::backup_of(&copy), &new_kick).is_some());
}

/// A library without the duplicate finder's prints is written all the same, and the highest sound forgotten before
/// anything was ever added keeps its number from being given again.
#[test]
fn a_scan_is_kept_without_prints_and_never_gives_a_number_again() {
    let dir = folder("grow-plain");
    let old = library(&dir, 20);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    compact::last_sync(&old, &copy).unwrap();
    Connection::open(&copy).unwrap().execute_batch("DROP TABLE IF EXISTS sound_prints;").unwrap();
    let (highest, path): (i64, String) = Connection::open(&copy)
        .unwrap()
        .query_row("SELECT id, path FROM sounds ORDER BY id DESC LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    let first: String =
        Connection::open(&copy).unwrap().query_row("SELECT MIN(path) FROM sounds", [], |r| r.get(0)).unwrap();
    assert_eq!(grow::follow(&old, &[], &[path]).unwrap(), Kept { forgotten: 1, ..Kept::default() });
    let new = "D:\\Samples\\Snare.wav".to_owned();
    let listened = |path: &str| Listened { path: path.to_owned(), size: 5, mtime: 6, analysed: Ok(heard(1.0)) };
    let kept = grow::keep(&old, &[listened(&new), listened(&first)]).unwrap();
    assert_eq!(kept, Kept { added: 1, updated: 1, ..Kept::default() });
    assert!(id_of(&copy, &new).unwrap() > highest);
    assert!(!grow::Kept::default().any() && kept.any());
}

/// What you do with sounds is kept in the app's own library (stage 3, review finding X1-own1), as the C# app's
/// `MarkPlayed` and `Record` kept it: a play counts on its sound and goes into History; drags and sends count for Most
/// liked; a sound forgotten since is skipped, and its events go with it. A library without the events table gets the
/// C# app's. The backup copy follows them once a day at most.
#[test]
fn plays_drags_and_sends_are_kept_for_history_and_most_liked() {
    let dir = folder("did");
    let old = library(&dir, 20);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    // After every play the fixture has, so History lists it first.
    let latest: i64 =
        Connection::open(&copy).unwrap().query_row("SELECT MAX(at) FROM sound_events", [], |r| r.get(0)).unwrap();
    let at = latest.max(unix(2026, 5, 20)) + 1;
    let did = |id, kind, at| Did { id, kind, at };
    let ids: Vec<(i64, String)> = {
        let c = Connection::open(&copy).unwrap();
        let mut st = c.prepare("SELECT id, path FROM sounds ORDER BY id LIMIT 2").unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
    };
    let [(a, _), (b, b_path)] = ids.as_slice() else { panic!("two sounds") };
    let (a, b) = (*a, *b);
    // Still following sounds.db: not written.
    let Err(e) = did::keep(&old, &[did(a, event::PLAY, at)]) else { panic!("not the app's own yet") };
    assert!(e.to_string().contains("isn't the app's own"), "{e}");
    compact::last_sync(&old, &copy).unwrap();
    assert!(matches!(backup::back_up(&old), Some(BackedUp::Made { .. })));
    let plays = |id| count(&copy, "SELECT COALESCE(plays, 0) FROM sounds WHERE id = ?1", id);
    let events = |id| count(&copy, "SELECT COUNT(*) FROM sounds JOIN sound_events ON sound_id = id WHERE id = ?1", id);
    let activity =
        |id| SoundsDb::open_read_only(&copy).unwrap().activity(now()).unwrap().get(&id).copied().unwrap_or_default();
    let (plays_a, events_b, was_a, was_b) = (plays(a), events(b), activity(a), activity(b));
    // Two plays, a drag and a send; one of a sound that isn't there is skipped.
    let kept = did::keep(
        &old,
        &[
            did(a, event::PLAY, at),
            did(a, event::PLAY, at + 60),
            did(b, event::DRAG, at),
            did(b, event::SEND, at),
            did(999_999, event::PLAY, at),
        ],
    )
    .unwrap();
    assert_eq!(kept, 4);
    assert_eq!(did::keep(&old, &[]).unwrap(), 0);
    assert_eq!(plays(a), plays_a + 2);
    assert_eq!(count(&copy, "SELECT last_played FROM sounds WHERE id = ?1", a), at + 60);
    assert_eq!(events(b), events_b + 2);
    let db = SoundsDb::open_read_only(&copy).unwrap();
    let history = db.history(10).unwrap();
    assert_eq!(history.first().map(|h| h.0), Some(a));
    drop(db);
    let (now_a, now_b) = (activity(a), activity(b));
    assert_eq!(now_a.plays, was_a.plays + 2);
    assert_eq!((now_b.drags, now_b.sends), (was_b.drags + 1, was_b.sends + 1));
    assert_eq!(pragma(&copy, "integrity_check"), "ok");
    // A sound forgotten at a scan takes its events with it.
    assert_eq!(grow::follow(&old, &[], std::slice::from_ref(b_path)).unwrap().forgotten, 1);
    assert_eq!(count(&copy, "SELECT COUNT(*) FROM sound_events WHERE sound_id = ?1", b), 0);
    // A library made from a sounds.db without the events table gets the C# app's.
    Connection::open(&copy).unwrap().execute_batch("DROP TABLE sound_events;").unwrap();
    assert_eq!(did::keep(&old, &[did(a, event::SEND, at)]).unwrap(), 1);
    assert_eq!(count(&copy, "SELECT COUNT(*) FROM sound_events WHERE sound_id = ?1", a), 1);
    let indexes: Vec<String> = {
        let c = Connection::open(&copy).unwrap();
        let mut st = c
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'sound_events' ORDER BY name")
            .unwrap();
        st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    assert_eq!(indexes, ["ix_sound_events", "ix_sound_events_sound"]);
    // The backup copy: behind by a sound forgotten at a scan, it's made again at once, even once a day.
    assert!(matches!(backup::back_up_when(&old, When::Daily), Some(BackedUp::Made { .. })));
    // Behind only by plays, drags and sends since it was made: followed at once when asked, else once it's a day old.
    did::keep(&old, &[did(a, event::PLAY, at)]).unwrap();
    assert_eq!(backup::back_up_when(&old, When::Daily), None);
    assert!(matches!(backup::back_up_when(&old, When::Now), Some(BackedUp::Made { .. })));
    did::keep(&old, &[did(a, event::PLAY, at)]).unwrap();
    assert_eq!(backup::back_up_when(&old, When::Daily), None);
    let day_old = backup::backup_of(&copy);
    Connection::open(&day_old)
        .unwrap()
        .execute("UPDATE copy_info SET value = value - 90000 WHERE key = 'backed_up_at'", [])
        .unwrap();
    assert!(matches!(backup::back_up_when(&old, When::Daily), Some(BackedUp::Made { .. })));
    assert_eq!(backup::back_up_when(&old, When::Daily), None);
    assert_eq!(backup::back_up(&old), None);
    // Any other change, in the day after: made again at once.
    did::keep(&old, &[did(a, event::PLAY, at)]).unwrap();
    assert_eq!(grow::follow(&old, &[], &[]).unwrap(), grow::Kept::default());
    assert!(matches!(backup::back_up_when(&old, When::Daily), Some(BackedUp::Made { .. })));
}

#[test]
fn the_galaxys_maps_are_kept_for_the_next_start() {
    use mo_sounds::StarPoint;
    use mo_sounds::compact::maps;
    let dir = folder("maps");
    let old = library(&dir, 20);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    let ids: Vec<i64> = {
        let c = Connection::open(&copy).unwrap();
        let mut st = c.prepare("SELECT id FROM sounds ORDER BY id LIMIT 3").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    let [a, b, c] = ids[..] else { panic!("three sounds") };
    let at = |x: f32, y: f32| StarPoint { x, y };
    let map = |space: &str| SoundsDb::open_read_only(&copy).unwrap().map(space).unwrap();
    // Still following sounds.db: not written.
    let points = HashMap::from([(a, at(0.25, 0.5))]);
    let Err(e) = maps::keep(&old, "features", &points, false) else { panic!("not the app's own yet") };
    assert!(e.to_string().contains("isn't the app's own"), "{e}");
    compact::last_sync(&old, &copy).unwrap();
    assert!(matches!(backup::back_up(&old), Some(BackedUp::Made { .. })));
    let clap_before = map("clap");
    // New places go over the old ones; a sound that isn't there is skipped.
    let was = map("features");
    let points = HashMap::from([(a, at(0.25, 0.5)), (b, at(0.75, 0.125)), (999_999, at(0.5, 0.5))]);
    assert_eq!(maps::keep(&old, "features", &points, false).unwrap(), 2);
    let now = map("features");
    assert_eq!((now.get(&a), now.get(&b)), (Some(&at(0.25, 0.5)), Some(&at(0.75, 0.125))));
    assert_eq!(now.len(), was.len() + [a, b].iter().filter(|id| !was.contains_key(id)).count());
    assert_eq!(maps::keep(&old, "features", &HashMap::new(), false).unwrap(), 0);
    // Laid out afresh: the whole map in place of the old one, the other ways of placing as they were.
    let fresh = HashMap::from([(b, at(0.5, 0.25)), (c, at(0.125, 0.875))]);
    assert_eq!(maps::keep(&old, "features", &fresh, true).unwrap(), 2);
    assert_eq!(map("features"), fresh);
    assert_eq!(map("clap"), clap_before);
    assert_eq!(pragma(&copy, "integrity_check"), "ok");
    // A sound forgotten at a scan takes its places with it.
    let c_path: String =
        Connection::open(&copy).unwrap().query_row("SELECT path FROM sounds WHERE id = ?1", [c], |r| r.get(0)).unwrap();
    assert_eq!(grow::follow(&old, &[], &[c_path]).unwrap().forgotten, 1);
    assert_eq!(map("features").len(), 1);
    assert!(matches!(backup::back_up(&old), Some(BackedUp::Made { .. })));
    // Maps change the library at nearly every start: the backup copy follows them once a day.
    assert_eq!(maps::keep(&old, "clap+features", &fresh, true).unwrap(), 1);
    assert_eq!(backup::back_up_when(&old, When::Daily), None);
    // A library made from a sounds.db without the maps table gets the C# app's.
    Connection::open(&copy).unwrap().execute_batch("DROP TABLE sound_maps;").unwrap();
    assert_eq!(maps::keep(&old, "features", &fresh, false).unwrap(), 1);
    assert_eq!(map("features").get(&b), Some(&at(0.5, 0.25)));
    let index: i64 = Connection::open(&copy)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'ix_sound_maps_sound'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(index, 1);
}

#[test]
fn your_favourites_stars_types_and_tags_are_kept() {
    use mo_sounds::compact::yours::{self, Change, Yours};
    let dir = folder("yours");
    let old = library(&dir, 20);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    // Two sounds with a name's type, and one with none.
    let pick = |sql: &str| -> Vec<(i64, String)> {
        let c = Connection::open(&copy).unwrap();
        let mut st = c.prepare(sql).unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
    };
    let named = pick("SELECT id, name_type FROM sounds WHERE name_type IS NOT NULL ORDER BY id LIMIT 2");
    let [(a, a_named), (b, _)] = named.as_slice() else { panic!("two sounds with a name's type") };
    let (a, b) = (*a, *b);
    let unnamed = pick("SELECT id, path FROM sounds WHERE name_type IS NULL ORDER BY id LIMIT 1");
    let [(c, c_path)] = unnamed.as_slice() else { panic!("a sound with no name's type") };
    let c = *c;
    let y = |id, change| Yours { id, change };
    let read = |id: i64| SoundsDb::open_read_only(&copy).unwrap().some(&[id], now()).unwrap().remove(0);
    // Still following sounds.db: not written.
    let Err(e) = yours::keep(&old, &[y(a, Change::Favourite(true))]) else { panic!("not the app's own yet") };
    assert!(e.to_string().contains("isn't the app's own"), "{e}");
    compact::last_sync(&old, &copy).unwrap();
    assert!(matches!(backup::back_up(&old), Some(BackedUp::Made { .. })));
    // A sound learned "dusty" from the C# app's model: yours now, kept in its capitals; "Warm" added in any capitals
    // once.
    Connection::open(&copy)
        .unwrap()
        .execute_batch(&format!(
            "DELETE FROM sound_tags WHERE sound_id IN ({a}, {b}, {c});
             INSERT INTO sound_tags VALUES ({a}, 'dusty', 'learned');"
        ))
        .unwrap();
    let kept = yours::keep(
        &old,
        &[
            y(a, Change::Favourite(true)),
            y(a, Change::Stars(9)),
            y(a, Change::Type(Some("  Perc ".into()))),
            y(a, Change::Tag("Dusty".into())),
            y(a, Change::Tag(" Warm ".into())),
            y(a, Change::Tag("warm".into())),
            y(a, Change::Tag("  ".into())),
            y(b, Change::Favourite(true)),
            y(b, Change::Favourite(false)),
            y(b, Change::Stars(3)),
            y(b, Change::Tag("gone".into())),
            y(b, Change::Untag("GONE".into())),
            y(c, Change::Type(Some("Kick".into()))),
            y(999_999, Change::Favourite(true)),
        ],
    )
    .unwrap();
    assert_eq!(kept, 12);
    assert_eq!(yours::keep(&old, &[]).unwrap(), 0);
    let now_a = read(a);
    assert!(now_a.favourite);
    assert_eq!(now_a.rating, yours::MOST_STARS);
    assert_eq!(now_a.user_type.as_deref(), Some("Perc"));
    assert_eq!((now_a.sound_type.as_str(), now_a.type_source.as_str()), ("Perc", "you"));
    assert_eq!(now_a.tags, ["Warm", "dusty"]);
    let source: String = Connection::open(&copy)
        .unwrap()
        .query_row("SELECT source FROM sound_tags WHERE sound_id = ?1 AND tag = 'dusty'", [a], |r| r.get(0))
        .unwrap();
    assert_eq!(source, "you");
    let now_b = read(b);
    assert!(!now_b.favourite);
    assert_eq!(now_b.rating, 3);
    assert!(now_b.tags.is_empty(), "{:?}", now_b.tags);
    assert_eq!((read(c).sound_type.as_str(), read(c).type_source.as_str()), ("Kick", "you"));
    assert_eq!(pragma(&copy, "integrity_check"), "ok");
    // Your own type taken away: the name's type shows again, or Unknown when the name gives none.
    yours::keep(&old, &[y(a, Change::Type(None)), y(c, Change::Type(Some(" ".into())))]).unwrap();
    let (now_a, now_c) = (read(a), read(c));
    assert_eq!((now_a.user_type, now_a.sound_type.as_str(), now_a.type_source.as_str()), (None, &a_named[..], "name"));
    assert_eq!((now_c.user_type, now_c.sound_type.as_str(), now_c.type_source.as_str()), (None, "Unknown", ""));
    // As sure as a new sound's would be: the name's for certain, nothing said of Unknown.
    assert_eq!((now_a.type_confidence, now_c.type_confidence), (1.0, 0.0));
    // They change the library at nearly every start: the backup copy follows them once a day.
    assert_eq!(backup::back_up_when(&old, When::Daily), None);
    assert!(matches!(backup::back_up_when(&old, When::Now), Some(BackedUp::Made { .. })));
    // A sound forgotten at a scan takes its tags with it.
    yours::keep(&old, &[y(c, Change::Tag("lost".into()))]).unwrap();
    assert_eq!(grow::follow(&old, &[], std::slice::from_ref(c_path)).unwrap().forgotten, 1);
    assert_eq!(count(&copy, "SELECT COUNT(*) FROM sound_tags WHERE sound_id = ?1", c), 0);
    // A library made from a sounds.db without the tags table gets the C# app's.
    Connection::open(&copy).unwrap().execute_batch("DROP TABLE sound_tags;").unwrap();
    assert_eq!(yours::keep(&old, &[y(a, Change::Tag("big".into()))]).unwrap(), 1);
    assert_eq!(read(a).tags, ["big"]);
    let index: i64 = Connection::open(&copy)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'ix_sound_tags_tag'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(index, 1);
}
