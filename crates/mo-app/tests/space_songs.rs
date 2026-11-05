//! The Space page's songs, from the library's saves to what each song could free, then Remove and Put back for their
//! old autosaves and caches: songs made in the test build's own fixtures folder under fixed names, never the
//! computer's real project folders, nor its Recycle Bin (a folder there stands in for it). The saves are made-up bytes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use chrono::{NaiveDate, NaiveDateTime};
use mo_app::binned;
use mo_app::space_bin::{self, PutBacks};
use mo_app::space_removing::Removable;
use mo_app::space_songs::{
    COMPACT, FromLibrary, Removing, Shown, SongItem, SongRoom, compact_card, lines, look, remove, tick,
};
use mo_library::Save;
use mo_space::clean::{Bin, Problem, Refused, Removed, What, undo};
use mo_space::song::{CompactKind, CompactSettings};

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// A save as the library lists it, written on disk with `bytes`.
fn save(id: i64, path: &Path, bytes: &[u8], modified: &str) -> Save {
    write(path, bytes);
    let text = path.to_string_lossy().into_owned();
    Save {
        id,
        name: path.file_stem().unwrap().to_string_lossy().into_owned(),
        daw: "Bitwig Studio".to_owned(),
        folder: path.parent().unwrap().to_string_lossy().into_owned(),
        size: i64::try_from(bytes.len()).unwrap(),
        modified: NaiveDateTime::parse_from_str(modified, "%Y-%m-%d %H:%M").unwrap(),
        path: text,
        ..Save::default()
    }
}

/// What a song could free, as (kind, file name) pairs.
fn items(song: &SongRoom) -> Vec<(CompactKind, String)> {
    let name = |p: &str| Path::new(p).file_name().unwrap().to_string_lossy().into_owned();
    song.report.items.iter().map(|i| (i.kind, name(&i.path))).collect()
}

#[test]
fn every_song_is_measured_and_the_save_it_opens_always_stays() {
    let dir = fixtures("mo-app-space-songs");
    let projects = dir.join("Projects");
    let night = projects.join("Night Drive");
    let bounce = projects.join("Bounce");
    // Night Drive, saved five times in one sitting. Its highest-numbered save, the one it opens, was saved before its
    // fourth: by time alone it's a save from the middle of the sitting. Its second and third saves are the same.
    let mut saves = vec![
        save(1, &night.join("Night Drive_1.bwproject"), b"first", "2026-10-10 10:00"),
        save(2, &night.join("Night Drive_2.bwproject"), b"second", "2026-10-10 10:20"),
        save(3, &night.join("Night Drive_3.bwproject"), b"second", "2026-10-10 10:30"),
        save(5, &night.join("Night Drive_5.bwproject"), b"the one it opens", "2026-10-10 10:50"),
        save(4, &night.join("Night Drive_4.bwproject"), b"fourth!", "2026-10-10 11:30"),
    ];
    // Seven auto-backups: the newest five stay.
    for n in 0..7u64 {
        let path = night.join("auto-backups").join(format!("Night Drive_4 ({n}).bwproject"));
        write(&path, &[b'a'; 10]);
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000 + n * 60);
        fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
    }
    // An old bounce no save names, a vocal its saves use, and a loop only another song's save uses.
    write(&night.join("Old bounce.wav"), &[0; 1000]);
    write(&night.join("Vox.wav"), &[0; 500]);
    write(&night.join("Shared loop.wav"), &[0; 300]);
    // Bounce: three saves in a morning, then one the next evening. Its second save is from the middle of a sitting.
    saves.extend([
        save(11, &bounce.join("Bounce_1.bwproject"), &[1; 200], "2026-10-11 09:00"),
        save(12, &bounce.join("Bounce_2.bwproject"), &[2; 210], "2026-10-11 09:10"),
        save(13, &bounce.join("Bounce_3.bwproject"), &[3; 220], "2026-10-11 09:20"),
        save(14, &bounce.join("Bounce_4.bwproject"), &[4; 230], "2026-10-12 18:00"),
    ]);
    // Gone: the library says its only save has gone, so it isn't measured.
    let gone = projects.join("Gone").join("Gone_1.bwproject");
    saves.push(Save { file_missing: true, ..save(21, &gone, &[5; 50], "2026-10-01 12:00") });
    fs::remove_file(&gone).unwrap();
    let from = FromLibrary {
        saves,
        samples: HashMap::from([
            (4, vec![r"C:\Old PC\Night Drive\Vox.wav".to_owned()]),
            (14, vec!["Shared loop.wav".to_owned()]),
        ]),
        roots: vec![projects.to_string_lossy().into_owned()],
        ..FromLibrary::default()
    };

    let found = look(&from, CompactSettings::default(), &AtomicBool::new(false)).unwrap();
    let mut songs: Vec<&SongRoom> = found.songs.iter().collect();
    songs.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<&str> = songs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Bounce", "Night Drive"]);
    let (bounce_room, night_room) = (songs[0], songs[1]);

    // The copy of the third save goes; the save it opens doesn't, though by time it's from the middle of the sitting.
    assert!(night_room.opens.ends_with("Night Drive_5.bwproject"));
    let mut night_items = items(night_room);
    night_items.sort_by(|a, b| a.1.cmp(&b.1));
    assert_eq!(
        night_items,
        [
            (CompactKind::SameSave, "Night Drive_2.bwproject".to_owned()),
            (CompactKind::Backup, "Night Drive_4 (0).bwproject".to_owned()),
            (CompactKind::Backup, "Night Drive_4 (1).bwproject".to_owned()),
            (CompactKind::UnusedAudio, "Old bounce.wav".to_owned()),
        ]
    );
    assert_eq!(night_room.saves, 5);
    assert_eq!(items(bounce_room), [(CompactKind::ThinnedSave, "Bounce_2.bwproject".to_owned())]);
    // Compact frees the copy and the old autosaves; the bounce goes only if you pick it.
    assert_eq!(found.compact_bytes(), 6 + 20);
    assert_eq!(found.thin_bytes(), 210);

    // Stopped, it says so.
    assert!(look(&from, CompactSettings::default(), &AtomicBool::new(true)).is_err());
}

/// A folder standing in for the Recycle Bin (as in the spare copies test): what's sent is moved in, under a number, and
/// can be put back from there.
struct FolderBin {
    dir: PathBuf,
    held: RefCell<Vec<(String, PathBuf)>>,
}

impl FolderBin {
    fn new(dir: PathBuf) -> FolderBin {
        fs::create_dir_all(&dir).unwrap();
        FolderBin { dir, held: RefCell::new(Vec::new()) }
    }

    fn at(&self, path: &str) -> Option<PathBuf> {
        self.held.borrow().iter().find(|(p, _)| p == path).map(|(_, at)| at.clone())
    }
}

impl Bin for FolderBin {
    fn send(&self, path: &Path) -> Result<(), String> {
        let mut held = self.held.borrow_mut();
        let to = self.dir.join(held.len().to_string());
        fs::rename(path, &to).map_err(|e| e.to_string())?;
        held.push((path.to_string_lossy().into_owned(), to));
        Ok(())
    }
    fn find(&self, removed: &mut [Removed]) -> Vec<Problem> {
        for r in removed {
            r.bin_id = self.at(&r.real).map(|at| at.to_string_lossy().into_owned());
        }
        Vec::new()
    }
    fn put_back(&self, removed: &[Removed]) -> Vec<Problem> {
        let mut problems = Vec::new();
        for r in removed {
            match r.bin_id.as_deref().map(PathBuf::from).filter(|at| at.exists()) {
                Some(at) => fs::rename(at, &r.path).unwrap(),
                None => {
                    problems.push(Problem { path: r.path.clone(), why: "It isn't in the bin any more.".to_owned() })
                }
            }
        }
        problems
    }
    fn holds(&self, removed: &[Removed]) -> Option<Vec<bool>> {
        Some(removed.iter().map(|r| r.bin_id.as_deref().is_some_and(|at| Path::new(at).exists())).collect())
    }
}

/// Sets when a file was last written, to `secs` past 1970.
fn written_at(path: &Path, secs: u64) {
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
    fs::File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
}

/// A Studio One save, as the library lists it.
fn studio_one(id: i64, path: &Path, bytes: &[u8], modified: &str) -> Save {
    Save { daw: "Studio One".to_owned(), ..save(id, path, bytes, modified) }
}

fn name_of(path: &str) -> String {
    let path = Path::new(path);
    let folder = path.parent().and_then(Path::file_name).unwrap_or_default().to_string_lossy();
    format!("{folder}/{}", path.file_name().unwrap().to_string_lossy())
}

#[test]
fn ticked_autosaves_and_caches_go_to_the_bin_each_checked_again_and_come_back() {
    let dir = fixtures("mo-app-space-songs-remove");
    let projects = dir.join("Projects");
    let (night, tape, keys) = (projects.join("Night Drive"), projects.join("Tape Echo"), projects.join("Lo-Fi Keys"));
    // Night Drive, with eight auto-backups a minute apart: the newest five stay, so three are offered.
    let mut saves = vec![
        save(1, &night.join("Night Drive_1.bwproject"), b"first", "2026-10-10 10:00"),
        save(2, &night.join("Night Drive_2.bwproject"), b"second", "2026-10-12 18:00"),
    ];
    let backup = |n: u64| night.join("auto-backups").join(format!("Night Drive_2 ({n}).bwproject"));
    for n in 0..8 {
        write(&backup(n), &[b'a'; 10]);
        written_at(&backup(n), 1_790_000_000 + n * 60);
    }
    // Two Studio One songs, each in a folder of its own with a Cache folder the DAW makes again.
    saves.push(studio_one(11, &tape.join("Tape Echo.song"), b"tape", "2026-10-11 09:00"));
    write(&tape.join("Cache").join("peaks.dat"), &[0; 400]);
    write(&tape.join("Cache").join("Waves").join("wave.cache"), &[0; 100]);
    saves.push(studio_one(21, &keys.join("Lo-Fi Keys.song"), b"keys", "2026-10-13 20:00"));
    write(&keys.join("Cache").join("peaks.dat"), &[0; 300]);
    let roots = vec![projects.to_string_lossy().into_owned()];
    let from = FromLibrary { saves, roots, ..FromLibrary::default() };
    let stop = AtomicBool::new(false);
    let found = look(&from, CompactSettings::default(), &stop).unwrap();
    let shown = Shown::of(&found, NaiveDate::from_ymd_opt(2026, 11, 4).unwrap());

    // Every old autosave and cache is ticked out of the box, the caches first (they free more), each song's autosaves
    // newest first, as they're measured.
    let mut removing = Removing::default();
    let names = |items: &[SongItem]| items.iter().map(|i| name_of(i.path())).collect::<Vec<_>>();
    let picked = removing.ticked_copies(Some(&shown));
    assert_eq!(
        names(&picked),
        [
            "Tape Echo/Cache",
            "Lo-Fi Keys/Cache",
            "auto-backups/Night Drive_2 (2).bwproject",
            "auto-backups/Night Drive_2 (1).bwproject",
            "auto-backups/Night Drive_2 (0).bwproject",
        ]
    );
    // A song's tick on the card is for all of its: unticked, none of Night Drive's goes; ticked again, all do.
    let card = compact_card(Some(&shown), &removing, true);
    let night_line = card.parts.iter().find(|p| p.name == "Night Drive").unwrap();
    let (ticks, ticked) = night_line.tick.clone().unwrap();
    assert!(ticked);
    tick(&mut removing, Some(&shown), &ticks, false);
    assert_eq!(removing.ticked_copies(Some(&shown)).len(), 2);
    let card = compact_card(Some(&shown), &removing, true);
    assert!(card.parts.iter().find(|p| p.name == "Night Drive").unwrap().tick.as_ref().is_some_and(|t| !t.1));
    assert!(card.parts.iter().find(|p| p.name == "2 old autosaves").is_none());
    let kind_line = card.parts.iter().find(|p| p.name == "3 old autosaves").unwrap();
    assert!(!kind_line.tick.as_ref().unwrap().1 && !kind_line.partly);
    tick(&mut removing, Some(&shown), &ticks, true);
    assert_eq!(removing.ticked_copies(Some(&shown)), picked);

    // After the look and before Remove: Night Drive's oldest autosave is written again (same time, another size), its
    // newest goes (so the third oldest is among the newest five now), and another song is saved into Lo-Fi Keys'
    // folder (so the folder isn't the song's alone any more, and nor is its cache).
    write(&backup(0), &[b'b'; 11]);
    written_at(&backup(0), 1_790_000_000);
    fs::remove_file(backup(7)).unwrap();
    let mut now = from.clone();
    now.saves.push(studio_one(22, &keys.join("Rhodes Jam.song"), b"jam", "2026-11-01 12:00"));
    // And Studio One writes more to Tape Echo's cache: it frees what it holds now.
    write(&tape.join("Cache").join("more.dat"), &[0; 50]);
    removing.start(picked.clone());
    let bin = FolderBin::new(dir.join("Bin"));
    let when = binned::now();
    let done = remove(&picked, &now, CompactSettings::default(), &bin, &stop);

    // Tape Echo's cache and the second oldest autosave went, to the bin; the rest stayed, each saying why.
    let removed: Vec<(String, What)> = done.removed.iter().map(|r| (name_of(&r.path), r.what)).collect();
    assert_eq!(
        removed,
        [
            ("Tape Echo/Cache".to_owned(), What::Song(CompactKind::Cache)),
            ("auto-backups/Night Drive_2 (1).bwproject".to_owned(), What::Song(CompactKind::Backup)),
        ]
    );
    assert!(!tape.join("Cache").exists() && !backup(1).exists());
    assert!(keys.join("Cache").join("peaks.dat").exists() && backup(0).exists() && backup(2).exists());
    assert_eq!(done.bytes(), 550 + 10);
    let why = |name: &str| done.problems.iter().find(|p| name_of(&p.path) == name).map(|p| p.why.clone()).unwrap();
    assert_eq!(why("auto-backups/Night Drive_2 (0).bwproject"), Refused::Changed.text());
    assert_eq!(
        why("auto-backups/Night Drive_2 (2).bwproject"),
        "Measured again just now, it isn't one of its song's old autosaves any more, so it stays."
    );
    assert_eq!(
        why("Lo-Fi Keys/Cache"),
        "Measured again just now, it isn't a cache of its song's any more, so it stays."
    );
    assert_eq!(done.problems.len(), 3);

    // What went is kept in the app's folder, named by kind on the Recycle Bin card, so Put back has it after a restart
    // too.
    let app = dir.join("App");
    let (kept, kept_at) = binned::record(&app, &bin, COMPACT, when, &done);
    assert!(kept.noted && kept_at == when);
    assert_eq!((kept.in_bin.items, kept.in_bin.bytes), (2, 560));
    let bin_card = space_bin::card(Some(&kept), &PutBacks::default(), true, false);
    assert_eq!(bin_card.parts[0].name, "Songs that can be compacted");
    assert!(bin_card.parts[0].what.starts_with("1 old autosave and 1 cache, removed "), "{}", bin_card.parts[0].what);
    assert_eq!(bin_card.parts[0].put_back, Some((COMPACT.to_owned(), when)));

    // The card says how each song went, and the line what was freed.
    removing.sent(when, done);
    let card = compact_card(Some(&shown), &removing, true);
    let status =
        |name: &str| card.parts.iter().find(|p| p.name == name).map(|p| (p.status.clone(), p.problem)).unwrap();
    assert_eq!(status("Tape Echo"), (format!("In the {}.", space_bin::BIN), false));
    assert_eq!(
        status("Lo-Fi Keys").0,
        "Measured again just now, it isn't a cache of its song's any more, so it stays."
    );
    assert_eq!(status("Night Drive"), (format!("1 in the {}, and 2 stayed (the log says why).", space_bin::BIN), true));
    let [line, _] = lines(Some(&shown), false, &removing);
    assert_eq!(line.unwrap().1.size, "Freed 560 bytes");

    // Put back: both are where they were, the cache with all it held, and nothing of the app's is left in the bin.
    let sent = removing.to_put_back().unwrap();
    assert_eq!(sent.coming_back(), kept.list[0].done);
    let problems = undo(&sent.coming_back(), &bin);
    assert!(problems.is_empty(), "{problems:?}");
    assert!(tape.join("Cache").join("Waves").join("wave.cache").exists() && backup(1).exists());
    assert!(tape.join("Cache").join("more.dat").exists());
    removing.put_back(when, problems);
    let card = compact_card(Some(&shown), &removing, true);
    assert_eq!(card.parts.iter().find(|p| p.name == "Tape Echo").unwrap().status, "Back where it was.");
    let now_kept = binned::refresh(&app, &bin);
    assert!(now_kept.list.is_empty() && now_kept.in_bin.is_empty());
}

#[test]
fn remove_takes_only_old_autosaves_and_caches_and_stop_leaves_them_all() {
    let dir = fixtures("mo-app-space-songs-remove-stop");
    let projects = dir.join("Projects");
    let night = projects.join("Night Drive");
    // Two saves the same, and seven auto-backups: the copy and the two oldest autosaves are offered.
    let saves = vec![
        save(1, &night.join("Night Drive_1.bwproject"), b"same", "2026-10-10 10:00"),
        save(2, &night.join("Night Drive_2.bwproject"), b"same", "2026-10-12 18:00"),
    ];
    let backup = |n: u64| night.join("auto-backups").join(format!("Night Drive_2 ({n}).bwproject"));
    for n in 0..7 {
        write(&backup(n), &[b'a'; 10]);
        written_at(&backup(n), 1_790_000_000 + n * 60);
    }
    let from = FromLibrary { saves, roots: vec![projects.to_string_lossy().into_owned()], ..FromLibrary::default() };
    let found = look(&from, CompactSettings::default(), &AtomicBool::new(false)).unwrap();
    let shown = Shown::of(&found, NaiveDate::from_ymd_opt(2026, 11, 4).unwrap());
    let items: Vec<SongItem> = mo_app::space_removing::Looked::items(&shown).to_vec();
    let kinds: Vec<CompactKind> = items.iter().map(|i| i.item.kind).collect();
    assert_eq!(kinds, [CompactKind::Backup, CompactKind::Backup, CompactKind::SameSave]);
    // The copy of a save has no tick (Remove doesn't take saves yet), and if it's sent all the same, it stays.
    let removing = Removing::default();
    assert_eq!(removing.ticked_copies(Some(&shown)), items[..2]);
    let bin = FolderBin::new(dir.join("Bin"));
    let done = remove(&items[2..], &from, CompactSettings::default(), &bin, &AtomicBool::new(false));
    assert!(done.removed.is_empty() && night.join("Night Drive_1.bwproject").exists());
    assert_eq!(done.problems.len(), 1);
    assert_eq!(done.problems[0].why, "Remove doesn't take these from here yet, so it stays.");
    // Stopped before it starts, everything picked stays where it is, saying so.
    let done = remove(&items[..2], &from, CompactSettings::default(), &bin, &AtomicBool::new(true));
    assert!(done.removed.is_empty() && backup(0).exists() && backup(1).exists());
    assert_eq!(done.problems.len(), 2);
    assert!(done.problems.iter().all(|p| p.why == Refused::Stopped.text()));
}
