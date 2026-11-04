//! The Rust app's copy of the sound library (`sounds-rust.db`): made from a sounds.db built the way the C# app builds
//! it, it must give every answer the C# app's file gives, to the last bit, and keep doing so as the C# app saves.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use mo_sounds::compact::{self, Synced};
use mo_sounds::{ChordSpan, SoundAnalysis, SoundsDb};
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
    let character = (0..dice.below(4)).map(|_| dice.pick(&words).to_owned()).collect();
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
        bpm_source: if dice.chance(40) { "name".into() } else { String::new() },
        pitch: dice.chance(50).then(|| r(24.0 + dice.unit() * 60.0, 2)),
        pitch_confidence: r(dice.unit(), 3),
        key_tonic: tonic,
        key_minor: dice.chance(50),
        key_confidence: r(dice.unit(), 3),
        key_source: if tonic.is_some() { "chroma".into() } else { String::new() },
        chord: dice.chance(10).then(|| "Ebm".to_owned()),
        chords,
        chroma,
        character,
        waveform,
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

/// Both read the same, every way they can be read, to the last bit (a -0.0 isn't a 0.0).
fn same_answers(old: &Path, copy: &Path) {
    let a = SoundsDb::open_read_only(old).unwrap();
    let b = SoundsDb::open_read_only(copy).unwrap();
    assert!(!a.is_copy());
    assert!(b.is_copy());
    let (ea, eb) = (a.everything(now()).unwrap(), b.everything(now()).unwrap());
    assert_eq!(ea.sounds.len(), eb.sounds.len());
    for (x, y) in ea.sounds.iter().zip(&eb.sounds) {
        assert_eq!(format!("{x:?}"), format!("{y:?}"));
    }
    assert_eq!(ea.features, eb.features);
    assert_eq!(ea.clap, eb.clap);
    assert_eq!(ea.analysed(), eb.analysed());
    assert_eq!(format!("{:?}", a.all(now()).unwrap()), format!("{:?}", b.all(now()).unwrap()));
    assert_eq!(a.activity(now()).unwrap(), b.activity(now()).unwrap());
    assert_eq!(a.history(500).unwrap(), b.history(500).unwrap());
    for space in ["features", "clap"] {
        assert_eq!(a.map(space).unwrap(), b.map(space).unwrap(), "{space}");
        assert_eq!(a.vectors(space).unwrap(), b.vectors(space).unwrap(), "{space}");
    }
    assert_eq!(a.tag_counts().unwrap(), b.tag_counts().unwrap());
    for tag in ["warm", "big", "dusty", "808", "vocal", "none"] {
        for source in ["you", "learned"] {
            assert_eq!(a.tagged(tag, source).unwrap(), b.tagged(tag, source).unwrap(), "{tag} {source}");
        }
    }
    assert_eq!(a.hidden().unwrap(), b.hidden().unwrap());
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

#[test]
fn the_copy_gives_every_answer_sounds_db_gives() {
    let dir = folder("every");
    let old = library(&dir, 1500);
    let copy = compact::copy_of(&old);
    assert!(!compact::in_step(&old, &copy));
    let synced = compact::sync(&old, &copy).unwrap();
    assert!(synced.made);
    assert_eq!((synced.sounds, synced.added, synced.changed, synced.removed), (1503, 1503, 0, 0));
    assert!(compact::in_step(&old, &copy));
    same_answers(&old, &copy);
    // Nothing changed since: nothing to do.
    let again = compact::sync(&old, &copy).unwrap();
    assert_eq!(again, Synced { sounds: 1503, ..Synced::default() });
    assert!(!again.changed_anything());
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
    assert!(copy_size * 100 < old_size * 45, "the copy is {copy_size} bytes, sounds.db {old_size}");
    let c = Connection::open(&copy).unwrap();
    assert_eq!(c.query_row("PRAGMA page_size", [], |r| r.get::<_, i64>(0)).unwrap(), 32_768);
    assert_eq!(c.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0)).unwrap(), "wal");
    // Nothing left beside it from making it.
    assert_eq!(half_made(&dir), Vec::<String>::new());
}

/// What the C# app does to sounds.db, one save at a time (`SoundLibrary.cs`), and what the copy should make of it.
#[test]
fn the_copy_follows_each_kind_of_save() {
    let dir = folder("follows");
    let old = library(&dir, 600);
    let copy = compact::copy_of(&old);
    compact::sync(&old, &copy).unwrap();
    let c = cs(&old);
    let some = |n: usize| -> Vec<i64> {
        let mut st = c.prepare("SELECT id FROM sounds WHERE analysis IS NOT NULL AND clap IS NULL LIMIT ?1").unwrap();
        st.query_map([n as i64], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    let one = some(1)[0];
    let step = |what: &str, sql: &str, expect: (usize, usize, usize)| {
        c.execute_batch(sql).unwrap();
        assert!(!compact::in_step(&old, &copy), "{what}: the copy should be behind");
        let s = compact::sync(&old, &copy).unwrap();
        assert_eq!((s.added, s.changed, s.removed), expect, "{what}: {s:?}");
        assert!(compact::in_step(&old, &copy), "{what}");
        same_answers(&old, &copy);
        s
    };
    // MarkPlayed: one more play, and when.
    let s = step(
        "a play",
        &format!(
            "UPDATE sounds SET plays = plays + 1, last_played = 1780000000 WHERE id = {one};
             INSERT INTO sound_events SELECT {one}, 'play', 1780000000 WHERE EXISTS (SELECT 1 FROM sounds WHERE id = {one});"
        ),
        (0, 1, 0),
    );
    assert_eq!(s.tables, 1, "only the plays' table changed");
    step("stars", &format!("UPDATE sounds SET rating = 5 WHERE id = {one}"), (0, 1, 0));
    step("a favourite", &format!("UPDATE sounds SET favourite = 1 WHERE id = {one}"), (0, 1, 0));
    step(
        "your own type",
        &format!(
            "UPDATE sounds SET user_type = 'Clap', type = COALESCE('Clap', name_type, 'Unknown'), type_source = 'you',
                 type_conf = 1 WHERE id = {one}"
        ),
        (0, 1, 0),
    );
    let fifty: Vec<String> = some(50).iter().map(i64::to_string).collect();
    step(
        "the classifier",
        &format!(
            "UPDATE sounds SET type = 'Perc', type_source = 'learned', type_conf = 0.75 WHERE id IN ({})",
            fifty.join(",")
        ),
        (0, 50, 0),
    );
    // SetClap: CLAP listened to one more.
    let clap = floats(&[0.25; 512]);
    c.execute("UPDATE sounds SET clap = ?1 WHERE id = ?2", params![clap, one]).unwrap();
    let s = compact::sync(&old, &copy).unwrap();
    assert_eq!((s.added, s.changed, s.removed, s.tables), (0, 1, 0, 0));
    same_answers(&old, &copy);
    // Save: a scan finds a new file, and a changed one (its CLAP kept, as the C# app's upsert keeps it).
    let save = "INSERT INTO sounds (path, size, mtime, error, analysis, features, clap, name_type, type, type_source, type_conf)
        VALUES (?1, ?2, ?3, NULL, ?4, ?5, NULL, 'Kick', COALESCE('Kick', 'Unknown'), 'name', 1)
        ON CONFLICT(path) DO UPDATE SET size = excluded.size, mtime = excluded.mtime, error = excluded.error,
            analysis = excluded.analysis, features = excluded.features, clap = COALESCE(excluded.clap, sounds.clap),
            name_type = excluded.name_type";
    let mut dice = Dice(42);
    let one_path: String = c.query_row("SELECT path FROM sounds WHERE id = ?1", [one], |r| r.get(0)).unwrap();
    for path in ["Z:\\New\\Kicks\\Kick new.wav", one_path.as_str()] {
        let features = floats(&[0.5; 73]);
        c.execute(save, params![path, 1234, 1_780_000_000, analysis_json(&mut dice), features]).unwrap();
    }
    let s = compact::sync(&old, &copy).unwrap();
    assert_eq!((s.added, s.changed, s.removed), (1, 1, 0));
    same_answers(&old, &copy);
    // Move: a file found under a new name.
    step(
        "a move",
        &format!(
            "UPDATE sounds SET path = 'Z:\\Moved\\here.wav', name_type = NULL WHERE path = '{}'",
            one_path.replace('\'', "''")
        ),
        (0, 1, 0),
    );
    // Remove: a file gone, and with it its tags, plays, places, print, copies and hidden mark.
    let gone: i64 = c.query_row("SELECT sound_id FROM sound_hidden LIMIT 1", [], |r| r.get(0)).unwrap();
    let s = step("a file gone", &format!("DELETE FROM sounds WHERE id = {gone}"), (0, 0, 1));
    assert!(s.tables >= 1);
    // Your tags and the app's suggestions.
    let s = step(
        "tags",
        &format!(
            "INSERT INTO sound_tags VALUES ({one}, 'fresh', 'you') ON CONFLICT(sound_id, tag) DO UPDATE SET source = excluded.source;
             DELETE FROM sound_tags WHERE tag = 'dusty' AND source = 'learned';"
        ),
        (0, 0, 0),
    );
    assert_eq!(s.tables, 1);
    // A galaxy laid out again, copies hidden, the duplicate finder's prints and its copies, and a setting.
    step("galaxy", "INSERT OR REPLACE INTO sound_maps SELECT 'clap', id, 1.0, 2.0 FROM sounds LIMIT 10;", (0, 0, 0));
    step("hidden", &format!("INSERT OR REPLACE INTO sound_hidden VALUES ({one}, 'batch 2', 99)"), (0, 0, 0));
    step(
        "prints",
        "UPDATE sound_prints SET exact = x'070707' WHERE sound_id IN (SELECT sound_id FROM sound_prints LIMIT 3)",
        (0, 0, 0),
    );
    step("copies", "UPDATE sound_matches SET apart = 2.5, typical = 1.25;", (0, 0, 0));
    step(
        "a setting",
        "INSERT INTO settings VALUES ('galaxy_basis', 'features') ON CONFLICT(key) DO UPDATE SET value = excluded.value;",
        (0, 0, 0),
    );
    // A table the C# app adds later (an older library had no duplicate finder yet), and one it drops.
    step("no prints", "DROP TABLE sound_prints;", (0, 0, 0));
    let copied = Connection::open(&copy).unwrap();
    let tables: i64 =
        copied.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'sound_prints'", [], |r| r.get(0)).unwrap();
    assert_eq!(tables, 0, "gone from the copy too");
    // With the damaged rows gone, every file the library knows reads the same from both, CLAP or not.
    let s = step("the damaged rows gone", "DELETE FROM sounds WHERE path LIKE 'D:\\Damaged\\%'", (0, 0, 3));
    // The 600 sounds made at the start, with the one the scan added and without the one gone.
    assert_eq!(s.sounds, 600);
    let known = SoundsDb::open_read_only(&copy).unwrap().known_files().unwrap();
    assert_eq!(known.len(), s.sounds);
    assert!(known.iter().any(|k| k.has_clap) && known.iter().any(|k| !k.has_clap));
}

#[test]
fn a_copy_of_another_format_or_half_made_is_made_again() {
    let dir = folder("again");
    let old = library(&dir, 50);
    let copy = compact::copy_of(&old);
    // Left from copies that stopped half way (this app's and others', by an older version too), and a log beside
    // where the copy goes that isn't its own: none may end up in the new copy, or be left behind.
    for name in [
        "sounds-rust.db.part".to_owned(),
        "sounds-rust.db.part-99999".to_owned(),
        "sounds-rust.db.part-99999-wal".to_owned(),
        format!("sounds-rust.db.part-{}", std::process::id()),
    ] {
        std::fs::write(dir.join(name), b"half a copy").unwrap();
    }
    std::fs::write(dir.join("sounds-rust.db-wal"), vec![0x37u8; 4096]).unwrap();
    let synced = compact::sync(&old, &copy).unwrap();
    assert!(synced.made);
    assert_eq!(half_made(&dir), Vec::<String>::new());
    same_answers(&old, &copy);
    // A copy an older version made, in another format: made again from the start.
    Connection::open(&copy).unwrap().execute("UPDATE copy_info SET value = 0 WHERE key = 'format'", []).unwrap();
    assert!(!compact::in_step(&old, &copy));
    let synced = compact::sync(&old, &copy).unwrap();
    assert!(synced.made);
    assert_eq!(synced.added, 53);
    same_answers(&old, &copy);
    // Something else where the copy goes: made again too.
    std::fs::write(&copy, b"not a database").unwrap();
    assert!(compact::sync(&old, &copy).unwrap().made);
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
    drop(holder);
    assert!(compact::in_step(&old, &copy), "still the copy it was");
    assert_eq!(compact::sync(&old, &copy).unwrap(), Synced { sounds: 33, ..Synced::default() });
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
