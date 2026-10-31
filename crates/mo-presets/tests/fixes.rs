//! Your part fixes in presets.db, made and changed in a folder of the test's own (never the real file).

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;

use mo_presets::Role;
use mo_presets::fixes::{RoleFixes, sound_hash};

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-presets-fixes").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn fixes_are_kept_changed_and_taken_away() {
    let path = folder("keep").join("MusicOrganiser").join("presets.db");
    let wobble = sound_hash(b"wobble settings");
    {
        let fixes = RoleFixes::open(&path).unwrap();
        let left: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            // The open database's own write-ahead files.
            .filter(|n| !n.ends_with("-wal") && !n.ends_with("-shm"))
            .collect();
        assert_eq!(left, ["presets.db"], "made, then put in place, nothing else left");
        assert!(fixes.all().unwrap().is_empty());
        fixes.set(&wobble, Role::Lead).unwrap();
        fixes.set(&wobble, Role::Bass).unwrap();
        assert!(fixes.set("", Role::Bass).is_err());
    }
    // Opened again: still there, once.
    let fixes = RoleFixes::open(&path).unwrap();
    assert_eq!(fixes.get(&wobble).unwrap(), Some(Role::Bass));
    assert_eq!(fixes.all().unwrap().len(), 1);
    fixes.clear(&wobble).unwrap();
    assert_eq!(fixes.get(&wobble).unwrap(), None);
}

#[test]
fn a_part_a_newer_app_wrote_is_left_alone() {
    let path = folder("newer").join("presets.db");
    let fixes = RoleFixes::open(&path).unwrap();
    fixes.set("aa", Role::Pad).unwrap();
    drop(fixes);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("INSERT INTO your_preset_roles (hash, role) VALUES ('bb', 'vocal chop')", []).unwrap();
    // Rows this app would never write: no fingerprint (it would match every preset file), and a part that isn't text.
    conn.execute("INSERT INTO your_preset_roles (hash, role) VALUES ('', 'bass')", []).unwrap();
    conn.execute("INSERT INTO your_preset_roles (hash, role) VALUES ('dd', x'00')", []).unwrap();
    drop(conn);
    let fixes = RoleFixes::open(&path).unwrap();
    fixes.set("ee", Role::Arp).unwrap();
    fixes.clear("ee").unwrap();
    assert_eq!(fixes.all().unwrap().into_iter().collect::<Vec<_>>(), [("aa".to_owned(), Role::Pad)]);
    assert_eq!(fixes.get("bb").unwrap(), None);
    assert_eq!(fixes.get("").unwrap(), None);
    assert_eq!(fixes.get("dd").unwrap(), None, "passed over, as all() does, not an error");
    drop(fixes);
    // Left alone: still there for the newer app.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let kept: String =
        conn.query_row("SELECT role FROM your_preset_roles WHERE hash = 'bb'", [], |r| r.get(0)).unwrap();
    assert_eq!(kept, "vocal chop");
}

#[test]
fn a_file_that_isnt_a_database_is_never_replaced() {
    let path = folder("not-a-db").join("presets.db");
    let junk = vec![b'x'; 4096];
    std::fs::write(&path, &junk).unwrap();
    assert!(RoleFixes::open(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), junk, "your file is as it was");
}

#[test]
fn a_half_made_database_from_a_crash_is_removed_and_an_old_journal_not_replayed() {
    let dir = folder("crash");
    let path = dir.join("presets.db");
    let old = dir.join("presets.db.new-1");
    std::fs::write(&old, b"half made").unwrap();
    let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options().write(true).open(&old).unwrap().set_modified(hour_ago).unwrap();
    let fresh = dir.join("presets.db.new-2");
    std::fs::write(&fresh, b"another copy making it now").unwrap();
    std::fs::write(dir.join("presets.db-journal"), b"left by a deleted database").unwrap();
    RoleFixes::open(&path).unwrap().set("cc", Role::Drums).unwrap();
    assert!(!old.exists() && fresh.exists());
    assert!(!dir.join("presets.db-journal").exists());
    assert_eq!(RoleFixes::open(&path).unwrap().get("cc").unwrap(), Some(Role::Drums));
}
