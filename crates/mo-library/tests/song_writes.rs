//! The app keeping what it reads in its own songs.db: saves read, read again, moved and gone missing, each batch all
//! or nothing, only ever in the app's own songs.db, and no save's row ever removed.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use mo_library::song_fields::{self, Change, Kept, SaveRow};
use mo_library::song_writes::{self, Batch, Missing, Put};
use mo_library::songs_db::{self, State};
use mo_library::{Library, PluginKind, ProjectPlugin, ProjectSample, Save, SaveDetail};
use rusqlite::Connection;

/// A data folder holding a library.db with one save (id 1), and the songs.db made from it, the app's own.
fn library(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-song-writes").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Connection::open(dir.join("library.db"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE roots (path TEXT PRIMARY KEY);
             CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                 name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT,
                 status TEXT DEFAULT 'Idea', rating INTEGER DEFAULT 0, notes TEXT DEFAULT '', last_opened INTEGER);
             CREATE TABLE tags (project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE, tag TEXT,
                 UNIQUE(project_id, tag));
             INSERT INTO projects (id, path, name, daw, folder, size, mtime, status, rating, notes, last_opened)
                 VALUES (1, 'C:\\Music\\Night\\Night_11.flp', 'Night_11', 'FL Studio', 'C:\\Music\\Night', 10, 100,
                     'Mixing', 4, 'louder kick', 1700000000);
             INSERT INTO tags VALUES (1, 'dark');",
        )
        .unwrap();
    let songs = dir.join(songs_db::FILE_NAME);
    songs_db::make(&dir.join("library.db"), &songs, &mut |_| {}).unwrap();
    assert_eq!(songs_db::at_start(&dir, "test").state, State::Switched);
    songs
}

/// A save as the reader gives it, with two plugins, three missing samples, its tracks and details.
fn read(id: i64, path: &str) -> SaveDetail {
    let (folder, name) = path.rsplit_once('\\').unwrap();
    let save = Save {
        id,
        path: path.to_owned(),
        name: name.trim_end_matches(".flp").to_owned(),
        daw: "FL Studio".into(),
        folder: folder.into(),
        size: 2_000,
        mtime: 1_700_000_500,
        tempo: Some(128.0),
        version: "24.1.1".into(),
        hardware: vec!["Digitakt".into()],
        tags: vec!["digitakt".into()],
        track_count: 2,
        ..Save::default()
    };
    SaveDetail {
        save,
        title: "Night".into(),
        plugins: vec![
            ProjectPlugin {
                name: "Serum".into(),
                vendor: "Xfer".into(),
                kind: PluginKind::Instrument,
                format: "VST3".into(),
                path: "C:\\VST3\\Serum.vst3".into(),
                installed: true,
            },
            ProjectPlugin {
                name: "Pro-Q 3".into(),
                vendor: "FabFilter".into(),
                kind: PluginKind::Effect,
                format: "VST3".into(),
                path: String::new(),
                installed: false,
            },
        ],
        // One with all its notes muted, one with none, one played: each kept as it is (the muted one isn't played).
        samples: vec![
            ProjectSample { path: "C:\\Samples\\hat.wav".into(), found: false, unplayed: false, muted: true },
            ProjectSample { path: "C:\\Samples\\kick \"1\".wav".into(), found: false, unplayed: true, muted: false },
            ProjectSample { path: "C:\\Samples\\snare.wav".into(), found: false, unplayed: false, muted: false },
        ],
        tracks: vec!["Lead".into(), "Drums".into()],
        details: vec![("Time signature".into(), "4/4".into()), ("Bars".into(), "64".into())],
        warnings: vec!["A plugin's state couldn't be read".into()],
        can_extract_midi: true,
        can_extract_presets: false,
    }
}

/// The save in `detail`, known to the library by the path `known_as` (`None`: never kept).
fn put(detail: SaveDetail, known_as: Option<&str>) -> Batch {
    Batch { put: vec![Put { detail, known_as: known_as.map(str::to_owned) }], ..Batch::default() }
}

/// The path the library made for these tests has its save 1 at.
const ONE: &str = "C:\\Music\\Night\\Night_11.flp";

#[test]
fn a_save_read_is_kept_and_reads_back_as_it_was_read() {
    let songs = library("new");
    let mut new = read(2, "C:\\Music\\Night\\Night_12.flp");
    new.save.status = "Mixing".into();
    let written = song_writes::write(&songs, &put(new.clone(), None)).unwrap();
    assert_eq!((written.put, written.ids.clone()), (1, Vec::new()));
    let lib = Library::open_read_only(&songs).unwrap();
    let back = lib.detail(2).unwrap().unwrap();
    assert_eq!(back.save.path, new.save.path);
    assert_eq!((back.save.size, back.save.mtime, back.save.tempo), (2_000, 1_700_000_500, Some(128.0)));
    assert_eq!((back.save.status.as_str(), back.save.hardware.clone()), ("Mixing", vec!["Digitakt".to_owned()]));
    assert_eq!((back.save.plugin_count, back.save.missing_plugins), (2, 1));
    assert_eq!((back.save.sample_count, back.save.missing_samples), (3, 3));
    assert_eq!(back.save.tags, vec!["digitakt".to_owned()]);
    assert_eq!((back.save.track_count, back.save.note_count), (2, -1));
    assert_eq!(back.title, "Night");
    assert_eq!(back.plugins, new.plugins);
    assert_eq!(back.samples, new.samples);
    // As the C# app keeps them: 1 has no notes, 2 has all its notes muted, 0 plays.
    let conn = Connection::open(&songs).unwrap();
    let mut st = conn.prepare("SELECT path, unplayed FROM samples WHERE project_id = 2 ORDER BY path").unwrap();
    let uses: Vec<(String, i64)> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    let uses: Vec<(&str, i64)> = uses.iter().map(|(p, u)| (p.rsplit('\\').next().unwrap(), *u)).collect();
    assert_eq!(uses, [("hat.wav", 2), ("kick \"1\".wav", 1), ("snare.wav", 0)]);
    assert_eq!(back.tracks, new.tracks);
    assert_eq!(back.details, new.details, "the details keep their order");
    assert_eq!(back.warnings, new.warnings);
    assert!(back.can_extract_midi && !back.can_extract_presets);
    assert_eq!(lib.saves().unwrap().len(), 2);
}

#[test]
fn a_save_read_again_is_the_same_row_with_what_you_gave_it_and_when_you_opened_it() {
    let songs = library("again");
    let mut again = read(1, "C:\\Music\\Night\\Night_11.flp");
    // As the watch hands it over: your fields kept from the save as it was.
    again.save.status = "Mixing".into();
    again.save.rating = 4;
    again.save.notes = "louder kick".into();
    again.save.tags = vec!["dark".into(), "digitakt".into()];
    again.plugins.truncate(1);
    song_writes::write(&songs, &put(again, Some(ONE))).unwrap();
    let lib = Library::open_read_only(&songs).unwrap();
    let saves = lib.saves().unwrap();
    assert_eq!(saves.len(), 1);
    let s = &saves[0];
    assert_eq!((s.id, s.rating, s.notes.as_str(), s.plugin_count, s.size), (1, 4, "louder kick", 1, 2_000));
    assert_eq!(s.tags, vec!["dark".to_owned(), "digitakt".to_owned()]);
    assert!(s.last_opened.is_some(), "when you last opened it isn't the file's to change");
}

#[test]
fn a_moved_save_keeps_its_row_and_a_file_the_library_has_keeps_its_id() {
    let songs = library("moved");
    // Moved to another folder: the same save, in its new place.
    let moved = read(1, "D:\\Music\\Night_11.flp");
    let written = song_writes::write(&songs, &put(moved, Some(ONE))).unwrap();
    assert!(written.ids.is_empty());
    let lib = Library::open_read_only(&songs).unwrap();
    assert_eq!(lib.saves().unwrap()[0].path, "D:\\Music\\Night_11.flp");
    drop(lib);
    // Given a new id, but the library has its file already (in other capitals): it's that save.
    let known = read(9, "d:\\music\\NIGHT_11.flp");
    let written = song_writes::write(&songs, &put(known, None)).unwrap();
    assert_eq!(written.ids, vec![(9, 1)]);
    // Given an id another save has meanwhile (another copy of the app): the next free one.
    let other = read(1, "D:\\Music\\Day_1.flp");
    let written = song_writes::write(&songs, &put(other, None)).unwrap();
    assert_eq!(written.ids, vec![(1, 2)]);
    let lib = Library::open_read_only(&songs).unwrap();
    assert_eq!(lib.saves().unwrap().len(), 2);
    drop(lib);
    // Moved, as another copy of the app knew it (by a path its row no longer has, as id 2 is another save's here):
    // kept as a save of its own, and the other save is left as it was.
    let stale = read(2, "E:\\Music\\Dusk_1.flp");
    let written = song_writes::write(&songs, &put(stale, Some("C:\\Music\\Dusk_1.flp"))).unwrap();
    assert_eq!(written.ids, vec![(2, 3)]);
    let lib = Library::open_read_only(&songs).unwrap();
    let paths: Vec<String> = lib.saves().unwrap().into_iter().map(|s| s.path).collect();
    assert!(paths.contains(&"D:\\Music\\Day_1.flp".to_owned()) && paths.len() == 3, "{paths:?}");
}

#[test]
fn a_gone_save_is_marked_missing_and_keeps_all_it_had() {
    let songs = library("gone");
    song_writes::write(&songs, &put(read(2, "C:\\Music\\Night\\Night_12.flp"), None)).unwrap();
    let missing = |id: i64, path: &str| Missing { id, path: path.to_owned() };
    // Only the row with both its id and its path: one that's another save's by now is left alone.
    let batch = Batch {
        missing: vec![missing(2, "c:\\music\\night\\NIGHT_12.flp"), missing(1, "C:\\Somewhere\\Else.flp")],
        ..Batch::default()
    };
    let written = song_writes::write(&songs, &batch).unwrap();
    assert_eq!(written.missing, 1);
    let lib = Library::open_read_only(&songs).unwrap();
    let saves = lib.saves().unwrap();
    assert_eq!(saves.len(), 2);
    let night_12 = saves.iter().find(|s| s.id == 2).unwrap();
    assert!(night_12.file_missing);
    assert_eq!((night_12.plugin_count, night_12.sample_count), (2, 3), "a missing save keeps what it had");
    assert!(!saves.iter().find(|s| s.id == 1).unwrap().file_missing);
}

#[test]
fn only_the_apps_own_songs_db_is_written_and_a_busy_one_is_left_as_it_was() {
    let songs = library("refused");
    let dir = songs.parent().unwrap();
    // library.db, and a songs.db not yet the app's own: never written.
    let why = song_writes::write(&dir.join("library.db"), &put(read(2, "C:\\a.flp"), None)).unwrap_err();
    assert!(why.to_string().contains("isn't the app's own"), "{why}");
    let not_own = dir.join("other").join(songs_db::FILE_NAME);
    std::fs::create_dir_all(not_own.parent().unwrap()).unwrap();
    songs_db::make(&dir.join("library.db"), &not_own, &mut |_| {}).unwrap();
    assert!(song_writes::write(&not_own, &put(read(2, "C:\\a.flp"), None)).is_err());
    assert_eq!(Library::open_read_only(&not_own).unwrap().saves().unwrap().len(), 1);
    // Another copy of the app writing meanwhile: busy, nothing changed, and it can be tried again.
    let holder = Connection::open(&songs).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let busy = song_writes::write(&songs, &put(read(2, "C:\\a.flp"), None)).unwrap_err();
    assert!(busy.is_busy(), "{busy}");
    holder.execute_batch("ROLLBACK;").unwrap();
    drop(holder);
    assert_eq!(Library::open_read_only(&songs).unwrap().saves().unwrap().len(), 1);
    song_writes::write(&songs, &put(read(2, "C:\\a.flp"), None)).unwrap();
    assert_eq!(Library::open_read_only(&songs).unwrap().saves().unwrap().len(), 2);
}

/// A save the app read again though its file hadn't changed (one marked missing that came back, say) is read again
/// too when its samples aren't as the C# app kept them: fewer muted, or some where the C# app couldn't read it (review
/// findings G-1, X1-4). Not one the app couldn't read itself: it would be tried at every start.
#[test]
fn saves_read_again_unchanged_are_mended_by_their_samples() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-song-writes").join("mend-samples");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let library_db = dir.join("library.db");
    // The C# app's layout: the details, then what it found wrong (only what comes before the details is read).
    Connection::open(&library_db)
        .unwrap()
        .execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE roots (path TEXT PRIMARY KEY);
             CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                 name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT,
                 error TEXT, file_missing INTEGER DEFAULT 0);
             CREATE TABLE samples (project_id INTEGER, path TEXT, found INTEGER, unplayed INTEGER);
             INSERT INTO projects (id, path, name, daw, folder, size, mtime, error) VALUES
                 (1, 'C:\\M\\Lost.flp', 'Lost', 'FL Studio', 'C:\\M', 10, 100, NULL),
                 (2, 'C:\\M\\Kept.flp', 'Kept', 'FL Studio', 'C:\\M', 10, 100, NULL),
                 (3, 'C:\\M\\Unread.flp', 'Unread', 'FL Studio', 'C:\\M', 10, 100, 'an FL 25 event'),
                 (4, 'C:\\M\\Broken.flp', 'Broken', 'FL Studio', 'C:\\M', 10, 100, NULL);
             INSERT INTO samples VALUES (1, 'C:\\s\\a.wav', 1, 2), (1, 'C:\\s\\b.wav', 1, 0),
                 (2, 'C:\\s\\a.wav', 1, 2), (4, 'C:\\s\\a.wav', 1, 2);",
        )
        .unwrap();
    let songs = dir.join(songs_db::FILE_NAME);
    songs_db::make(&library_db, &songs, &mut |_| {}).unwrap();
    assert_eq!(songs_db::at_start(&dir, "test").state, State::Switched);
    assert_eq!(song_writes::to_mend(&songs).unwrap(), Some(Vec::new()));
    // Read again unchanged: Lost by an earlier build (its muted mark lost), Kept by this one, Unread for the first
    // time (the C# app couldn't), and Broken couldn't be read (kept with what it had).
    Connection::open(&songs)
        .unwrap()
        .execute_batch(
            "UPDATE samples SET unplayed = 0 WHERE project_id = 1;
             UPDATE projects SET error = NULL WHERE id = 3;
             INSERT INTO samples (project_id, path, found, unplayed) VALUES (3, 'C:\\s\\c.wav', 1, 0);
             UPDATE projects SET error = 'damaged' WHERE id = 4;
             DELETE FROM samples WHERE project_id = 4;",
        )
        .unwrap();
    assert_eq!(song_writes::to_mend(&songs).unwrap(), Some(vec![1, 3]));
    // An older C# app that kept no muted marks: only how many samples each has.
    Connection::open(&library_db).unwrap().execute_batch("ALTER TABLE samples DROP COLUMN unplayed;").unwrap();
    assert_eq!(song_writes::to_mend(&songs).unwrap(), Some(vec![3]));
}

#[test]
fn the_fl_studio_saves_the_app_kept_are_read_again_once_for_their_muted_samples() {
    let songs = library("mend");
    let dir = songs.parent().unwrap();
    // Only the app's own songs.db has any to mend: not library.db, nor a songs.db that isn't its own yet.
    assert_eq!(song_writes::to_mend(&dir.join("library.db")).unwrap(), None);
    let not_own = dir.join("other").join(songs_db::FILE_NAME);
    std::fs::create_dir_all(not_own.parent().unwrap()).unwrap();
    songs_db::make(&dir.join("library.db"), &not_own, &mut |_| {}).unwrap();
    assert_eq!(song_writes::to_mend(&not_own).unwrap(), None);
    // Just made from library.db: none of its rows the app's.
    assert_eq!(song_writes::to_mend(&songs).unwrap(), Some(Vec::new()));
    // Read again (changed since), changed lately, two months ago, a Bitwig save, and one gone.
    let now = chrono::Local::now().timestamp();
    let day = 24 * 60 * 60;
    let mut again = read(1, "C:\\Music\\Night\\Night_11.flp");
    again.save.mtime = now - 90 * day;
    let mut lately = read(2, "C:\\Music\\Night\\Night_12.flp");
    lately.save.mtime = now - 20 * day;
    let mut long_ago = read(3, "C:\\Music\\Night\\Night_10.flp");
    long_ago.save.mtime = now - 60 * day;
    let mut bitwig = read(4, "C:\\Music\\Night\\Night.bwproject");
    bitwig.save.daw = "Bitwig Studio".into();
    bitwig.save.mtime = now - day;
    let mut gone = read(5, "C:\\Music\\Night\\Night_13.flp");
    gone.save.mtime = now - day;
    gone.save.file_missing = true;
    let put = [(again, Some(ONE)), (lately, None), (long_ago, None), (bitwig, None), (gone, None)]
        .into_iter()
        .map(|(detail, known_as)| Put { detail, known_as: known_as.map(str::to_owned) })
        .collect();
    song_writes::write(&songs, &Batch { put, ..Batch::default() }).unwrap();
    // The FL Studio ones that aren't in library.db as they are, whenever they changed.
    let mut ids = song_writes::to_mend(&songs).unwrap().unwrap();
    ids.sort_unstable();
    assert_eq!(ids, [1, 2, 3]);
    // With library.db gone: a guess, those changed since a month before the library became the app's own.
    std::fs::rename(dir.join("library.db"), dir.join("library.db.away")).unwrap();
    assert_eq!(song_writes::to_mend(&songs).unwrap(), Some(vec![2]));
    // Read again but for two that couldn't be: noted with them, and only those still FL Studio saves there are
    // given after that (one since gone missing, one no longer in the library).
    assert!(!song_writes::mend_noted(&songs), "library.db is still wanted for the mend");
    let noted = song_writes::write(&songs, &Batch { mended: true, owed: vec![2, 5, 99], ..Batch::default() }).unwrap();
    assert!(noted.mended);
    assert!(song_writes::mend_noted(&songs), "from here on library.db isn't looked at");
    assert!(!song_writes::mend_noted(&dir.join("no-songs.db")));
    assert_eq!(song_writes::to_mend(&songs).unwrap(), Some(vec![2]));
    // Once they're read again, a batch notes it (on its own too): none after that.
    let noted = song_writes::write(&songs, &Batch { mended: true, ..Batch::default() }).unwrap();
    assert!(noted.mended);
    assert_eq!(song_writes::to_mend(&songs).unwrap(), None);
}

/// The path the tests' second save is at.
const TWO: &str = "C:\\Music\\Night\\Night_12.flp";

/// Saves as [`song_fields::set`] is told of them: by id and the path the library keeps each under.
fn rows(saves: &[(i64, &str)]) -> Vec<SaveRow> {
    saves.iter().map(|(id, path)| SaveRow { id: *id, path: (*path).to_owned() }).collect()
}

/// What you gave the save `id`, as the library at `songs` reads it: status, stars, heart and tags (in order).
fn yours(songs: &std::path::Path, id: i64) -> (String, i32, bool, Vec<String>) {
    let lib = Library::open_read_only(songs).unwrap();
    let s = lib.saves().unwrap().into_iter().find(|s| s.id == id).unwrap();
    let mut tags = s.tags;
    tags.sort();
    (s.status, s.rating, s.favourite, tags)
}

#[test]
fn what_you_give_a_song_is_kept_for_every_save_of_it_and_reads_back() {
    let songs = library("fields");
    song_writes::write(&songs, &put(read(2, TWO), None)).unwrap();
    let both = rows(&[(1, ONE), (2, TWO)]);
    let set = |change: Change| song_fields::set(&songs, &both, &change).unwrap();
    assert_eq!(set(Change::Status("Finished".into())), Kept { saves: vec![1, 2], unknown: vec![] });
    set(Change::Rating(5));
    set(Change::Favourite(true));
    // A tag is kept trimmed, in small letters, once.
    set(Change::AddTag("  Techno ".into()));
    set(Change::AddTag("techno".into()));
    let tags = |list: &[&str]| list.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
    assert_eq!(yours(&songs, 1), ("Finished".into(), 5, true, tags(&["dark", "techno"])));
    assert_eq!(yours(&songs, 2), ("Finished".into(), 5, true, tags(&["digitakt", "techno"])));
    // Taken off whatever its letter case, also where an older library spelt it otherwise; one that isn't on is fine.
    Connection::open(&songs).unwrap().execute("UPDATE tags SET tag = 'Dark' WHERE tag = 'dark'", []).unwrap();
    set(Change::AddTag("dark".into()));
    assert_eq!(yours(&songs, 1).3, tags(&["Dark", "techno"]), "the same tag in other letters isn't put on twice");
    set(Change::RemoveTag("DARK".into()));
    set(Change::Favourite(false));
    set(Change::Rating(0));
    assert_eq!(yours(&songs, 1), ("Finished".into(), 0, false, tags(&["techno"])));
    assert_eq!(yours(&songs, 2), ("Finished".into(), 0, false, tags(&["digitakt", "techno"])));
    // A save the library doesn't have is said so, one asked twice counts once, and the rest is kept. An id the
    // library has under another path is another save's: it's left alone. Paths are the same in any letter case.
    let asked = rows(&[(2, "c:\\music\\night\\NIGHT_12.flp"), (77, "C:\\x.flp"), (2, TWO), (1, "C:\\other.flp")]);
    let kept = song_fields::set(&songs, &asked, &Change::Rating(3)).unwrap();
    assert_eq!(kept, Kept { saves: vec![2], unknown: vec![77, 1] });
    assert_eq!((yours(&songs, 1).1, yours(&songs, 2).1), (0, 3));
    // Your key and notes, as you write them (a key without the spaces around it); empty takes them off.
    set(Change::Key(" F# minor ".into()));
    set(Change::Notes("louder kick\nthen bounce".into()));
    let lib = Library::open_read_only(&songs).unwrap();
    for s in lib.saves().unwrap() {
        assert_eq!((s.musical_key.as_str(), s.notes.as_str()), ("F# minor", "louder kick\nthen bounce"));
    }
    drop(lib);
    set(Change::Key(String::new()));
    assert_eq!(Library::open_read_only(&songs).unwrap().saves().unwrap()[0].musical_key, "");
    // A tag an older library holds that couldn't be put on now (too long, spaces around it) can still be taken off.
    let long = "x".repeat(70);
    let c = Connection::open(&songs).unwrap();
    c.execute("INSERT INTO tags VALUES (1, ?1), (1, ' Loose ')", [&long]).unwrap();
    drop(c);
    set(Change::RemoveTag(long));
    set(Change::RemoveTag("loose".into()));
    assert_eq!(yours(&songs, 1).3, tags(&["techno"]));
    // Nothing else of the save is touched: its plugins, samples, details and when it was opened.
    let lib = Library::open_read_only(&songs).unwrap();
    let two = lib.detail(2).unwrap().unwrap();
    assert_eq!((two.plugins.len(), two.samples.len(), two.tracks.len()), (2, 3, 2));
    assert!(lib.saves().unwrap().iter().find(|s| s.id == 1).unwrap().last_opened.is_some());
}

#[test]
fn a_save_read_again_cant_undo_what_you_gave_it() {
    let songs = library("fields-read-again");
    for change in [
        Change::Status("Released".into()),
        Change::Rating(2),
        Change::Favourite(true),
        Change::AddTag("techno".into()),
        Change::RemoveTag("dark".into()),
    ] {
        song_fields::set(&songs, &rows(&[(1, ONE)]), &change).unwrap();
    }
    // An older library spelt the hardware's tag its own way: it isn't put on again beside it.
    Connection::open(&songs).unwrap().execute("INSERT INTO tags VALUES (1, 'Digitakt')", []).unwrap();
    // The folder watch hands the save over as it knew it a moment before: the old status, stars and tags.
    let mut again = read(1, ONE);
    again.save.status = "Mixing".into();
    again.save.rating = 4;
    again.save.favourite = false;
    again.save.notes = "something else".into();
    again.save.tags = vec!["dark".into(), "digitakt".into()];
    song_writes::write(&songs, &put(again, Some(ONE))).unwrap();
    // Yours as you left them; the file's as read; the tag for its hardware put on, as ever.
    assert_eq!(yours(&songs, 1), ("Released".into(), 2, true, vec!["Digitakt".to_owned(), "techno".to_owned()]));
    let lib = Library::open_read_only(&songs).unwrap();
    let one = lib.saves().unwrap().into_iter().find(|s| s.id == 1).unwrap();
    assert_eq!((one.size, one.notes.as_str(), one.plugin_count), (2_000, "louder kick", 2));
}

#[test]
fn a_change_that_cant_be_kept_changes_nothing() {
    let songs = library("fields-refused");
    let dir = songs.parent().unwrap();
    let before = yours(&songs, 1);
    let one = rows(&[(1, ONE)]);
    for (change, why) in [
        (Change::Status("Done".into()), "isn't one of the app's statuses"),
        (Change::Status("finished".into()), "isn't one of the app's statuses"),
        (Change::Rating(6), "isn't between 0 and 5"),
        (Change::Rating(-1), "isn't between 0 and 5"),
        (Change::AddTag("   ".into()), "a tag needs a name"),
        (Change::AddTag("a\u{1f}b".into()), "can't be shown"),
        (Change::AddTag("x".repeat(65)), "64 characters at most"),
        (Change::RemoveTag(String::new()), "a tag needs a name"),
        (Change::Key("x".repeat(33)), "32 characters at most"),
        (Change::Key("A\tminor".into()), "can't be shown"),
        (Change::Notes("x".repeat(20_001)), "20000 characters at most"),
    ] {
        let refused = song_fields::set(&songs, &one, &change).unwrap_err().to_string();
        assert!(refused.contains(why), "{change:?}: {refused}");
        assert!(!change.apply(&mut Save::default()), "{change:?}");
    }
    assert_eq!(yours(&songs, 1), before);
    // library.db, and a songs.db not yet the app's own: never written.
    let why = song_fields::set(&dir.join("library.db"), &one, &Change::Rating(1)).unwrap_err().to_string();
    assert!(why.contains("isn't the app's own"), "{why}");
    let not_own = dir.join("other").join(songs_db::FILE_NAME);
    std::fs::create_dir_all(not_own.parent().unwrap()).unwrap();
    songs_db::make(&dir.join("library.db"), &not_own, &mut |_| {}).unwrap();
    assert!(song_fields::set(&not_own, &one, &Change::Rating(1)).is_err());
    assert_eq!(yours(&not_own, 1).1, 4);
    assert!(song_fields::set(&dir.join("none.db"), &one, &Change::Rating(1)).is_err());
    assert!(!dir.join("none.db").exists());
    // Another write holding the library: busy, nothing changed, and it can be tried again.
    let holder = Connection::open(&songs).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let busy = song_fields::set(&songs, &one, &Change::Rating(1)).unwrap_err();
    assert!(busy.is_busy(), "{busy}");
    holder.execute_batch("ROLLBACK;").unwrap();
    drop(holder);
    assert_eq!(yours(&songs, 1), before);
    song_fields::set(&songs, &one, &Change::Rating(1)).unwrap();
    assert_eq!(yours(&songs, 1).1, 1);
}

#[test]
fn the_same_change_made_to_a_save_in_memory_is_what_the_library_keeps() {
    let songs = library("fields-memory");
    let mut held = Library::open_read_only(&songs).unwrap().saves().unwrap().remove(0);
    for change in [
        Change::Status("Arranging".into()),
        Change::Rating(1),
        Change::Favourite(true),
        Change::AddTag(" Techno".into()),
        Change::RemoveTag("Dark".into()),
    ] {
        song_fields::set(&songs, &rows(&[(1, ONE)]), &change).unwrap();
        assert!(change.apply(&mut held), "{change:?}");
        assert!(!change.apply(&mut held), "{change:?}: the same change again changes nothing");
        held.tags.sort();
        assert_eq!(yours(&songs, 1), (held.status.clone(), held.rating, held.favourite, held.tags.clone()));
    }
    assert_eq!(held.tags, vec!["techno".to_owned()]);
}

#[test]
fn a_new_save_doesnt_take_on_tags_an_older_library_left_under_its_id() {
    let songs = library("leftover-tags");
    // Tags of a save the C# app removed, copied over with the rest: no save has id 2 yet.
    let c = Connection::open(&songs).unwrap();
    c.execute_batch("PRAGMA foreign_keys = OFF; INSERT INTO tags VALUES (2, 'someone elses');").unwrap();
    drop(c);
    song_writes::write(&songs, &put(read(2, TWO), None)).unwrap();
    assert_eq!(yours(&songs, 2).3, vec!["digitakt".to_owned()]);
}
