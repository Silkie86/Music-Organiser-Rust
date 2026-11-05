//! A song's room on disk and its Compact list, on folders made in the test build's own fixtures folder under fixed
//! names, never a random temp one and never a real folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeDelta};
use mo_space::key;
use mo_space::song::{CompactKind, CompactSettings, Elsewhere, SongSave, measure, thin};

mod common;

use common::folder_link;

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn day() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 3, 14).unwrap().and_hms_opt(12, 0, 0).unwrap()
}

/// Writes a file with these bytes, last written `hours` after noon on 14 March 2026.
fn write(dir: &Path, rel: &str, bytes: &[u8], hours: i64) -> String {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, bytes).unwrap();
    let when = day() + TimeDelta::hours(hours);
    let local = when.and_local_timezone(Local).single().unwrap();
    fs::File::options().write(true).open(&p).unwrap().set_modified(SystemTime::from(local)).unwrap();
    p.to_string_lossy().into_owned()
}

fn save(path: &str, hours: i64) -> SongSave {
    SongSave {
        path: path.to_owned(),
        size: fs::metadata(path).map_or(0, |m| m.len()),
        modified: day() + TimeDelta::hours(hours),
    }
}

fn s(dir: &Path) -> String {
    dir.to_string_lossy().into_owned()
}

#[test]
fn a_song_with_its_own_folder_lists_copies_old_autosaves_and_unused_audio() {
    let root = fixtures("mo-space-own-folder");
    let song = root.join("Glass Harbor");
    let v1 = write(&song, "Glass Harbor v1.bwproject", b"same notes", 0);
    let v2 = write(&song, "Glass Harbor v2.bwproject", b"same notes", 1);
    let v3 = write(&song, "Glass Harbor v3.bwproject", b"new notes!!", 2);
    for n in 0..7 {
        write(&song, &format!("auto-backups/Glass Harbor {n}.bwproject"), b"auto", n);
    }
    // Audio in an autosave folder is yours, not an autosave.
    write(&song, "auto-backups/Vocal take.wav", &[0; 40], 0);
    write(&song, "Samples/Kick.wav", &[0; 10], 0);
    write(&song, "Bounce.wav", &[0; 100], 0);
    write(&song, "Notes.txt", &[0; 3], 0);
    let saves = [save(&v1, 0), save(&v2, 1), save(&v3, 2)];
    let used = vec![r"C:\Old PC\Samples\KICK.wav".to_owned()];

    let report =
        measure(&saves, &[s(&root)], &used, CompactSettings::default(), None, &AtomicBool::new(false)).unwrap();
    assert!(report.own_folder);
    assert_eq!(report.folder, s(&song));
    assert_eq!(report.saves, 10 + 10 + 11);
    assert_eq!(report.backups, 7 * 4);
    assert_eq!(report.audio, 40 + 10 + 100);
    assert_eq!(report.other, 3);
    assert_eq!(report.bytes, 31 + 28 + 150 + 3);

    let kinds: Vec<(CompactKind, String)> = report
        .items
        .iter()
        .map(|i| (i.kind, Path::new(&i.path).file_name().unwrap().to_string_lossy().into()))
        .collect();
    assert_eq!(
        kinds,
        [
            // v2 is a copy of v1: v1 stays, as the song's first save, which thinning keeps too (so a later Compact
            // can't thin the copy that's left); v3 (the newest) is never listed.
            (CompactKind::SameSave, "Glass Harbor v2.bwproject".into()),
            // The newest five autosaves stay.
            (CompactKind::Backup, "Glass Harbor 1.bwproject".into()),
            (CompactKind::Backup, "Glass Harbor 0.bwproject".into()),
            (CompactKind::UnusedAudio, "Bounce.wav".into()),
            (CompactKind::UnusedAudio, "Vocal take.wav".into()),
        ]
    );
    let copy = &report.items[0];
    assert_eq!(copy.same_as.as_deref(), Some(v1.as_str()));
    assert_eq!(copy.note(), "Same as Glass Harbor v1.bwproject");
    assert_eq!(report.items[1].note(), "Autosave from 14 Mar 2026");
    // Copies, old autosaves and caches free room straight away; unused audio only when you pick it.
    assert_eq!(report.freeable(), 10 + 4 + 4);
    assert!(copy.ticked_at_first() && !report.items[3].ticked_at_first());
}

#[test]
fn audio_another_project_uses_or_in_a_sample_folder_is_never_offered() {
    let root = fixtures("mo-space-elsewhere");
    let song = root.join("Projects/Slow Orbit");
    let v = write(&song, "Slow Orbit.bwproject", b"x", 0);
    write(&song, "Pad.wav", &[0; 5], 0);
    write(&song, "Shared Kit/Snare.WAV", &[0; 6], 0);
    write(&song, "Stem.wav", &[0; 7], 0);
    let samples = vec![s(&song.join("Shared Kit"))];
    let asked = std::sync::Mutex::new(Vec::new());
    let used_by_any = |names: &[String]| {
        asked.lock().unwrap().extend(names.iter().cloned());
        names.iter().filter(|n| n.eq_ignore_ascii_case("pad.wav")).map(|n| mo_formats::case::key(n)).collect()
    };
    let shared = HashSet::new();
    let elsewhere = Elsewhere { sample_folders: &samples, used_by_any: &used_by_any, shared_names: &shared };
    let report = measure(
        &[save(&v, 0)],
        &[s(&root.join("Projects"))],
        &[],
        CompactSettings::default(),
        Some(&elsewhere),
        &AtomicBool::new(false),
    )
    .unwrap();
    let offered: Vec<String> = report.items.iter().map(|i| i.note()).collect();
    assert_eq!(offered, ["Stem.wav"]);
    // The sample folder's sound isn't even asked about.
    let mut asked = asked.into_inner().unwrap();
    asked.sort();
    assert_eq!(asked, ["Pad.wav", "Stem.wav"]);
}

#[test]
fn fl_songs_sharing_a_folder_count_only_their_saves_and_their_autosaves() {
    let root = fixtures("mo-space-fl-shared");
    let projects = root.join("Projects");
    let a = write(&projects, "Beats/Night Drive_2.flp", b"aa", 0);
    let b = write(&projects, "Beats/Night Drive_14.flp", b"bbbb", 5);
    write(&projects, "Beats/Other Song.flp", b"o", 0);
    write(&projects, "Beats/loop.wav", &[0; 9], 0);
    write(&projects, "Backup/Night Drive_2 (autosave).flp", b"123", 1);
    write(&projects, "Backup/NIGHT DRIVE_2 (overwritten at 2213h05).flp", b"12", 2);
    // FL Studio names an autosave after the save's file, so this one is another song's.
    write(&projects, "Backup/Night Drive (autosave).flp", b"12345", 1);
    write(&projects, "Backup/Night Drive_14 (autosave).flp", b"1", 3);
    write(&projects, "Backup/Other Song (autosave).flp", b"1234", 4);
    write(&projects, "Backup/Night Drive.wav", b"1234", 4);
    let saves = [save(&a, 0), save(&b, 5)];
    let roots = [s(&projects)];
    let settings = CompactSettings { keep_backups: 0, sitting_hours: 0 };
    let stop = AtomicBool::new(false);

    let report = measure(&saves, &roots, &[], settings, None, &stop).unwrap();
    assert!(!report.own_folder);
    assert_eq!(report.saves, 6);
    assert_eq!(report.audio + report.other, 0);
    let mut backups: Vec<String> =
        report.items.iter().map(|i| Path::new(&i.path).file_name().unwrap().to_string_lossy().into()).collect();
    backups.sort();
    assert_eq!(
        backups,
        ["NIGHT DRIVE_2 (overwritten at 2213h05).flp", "Night Drive_14 (autosave).flp", "Night Drive_2 (autosave).flp"]
    );
    assert_eq!(report.backups, 3 + 2 + 1);

    // A name another song has too: its autosaves could be that song's, so they're left alone.
    let shared: HashSet<String> = [mo_formats::case::key("Night Drive_2")].into();
    let none = |_: &[String]| HashSet::new();
    let elsewhere = Elsewhere { sample_folders: &[], used_by_any: &none, shared_names: &shared };
    let report = measure(&saves, &roots, &[], settings, Some(&elsewhere), &stop).unwrap();
    assert_eq!(report.items.len(), 1);
    assert!(report.items[0].path.ends_with("Night Drive_14 (autosave).flp"));
}

#[test]
fn a_folder_with_another_songs_save_isnt_the_songs_own() {
    let root = fixtures("mo-space-not-own");
    let song = root.join("Projects/Two Songs");
    let v = write(&song, "One.bwproject", b"1", 0);
    write(&song, "Inside/Two.bwproject", b"2", 0);
    write(&song, "Big.wav", &[0; 50], 0);
    let report = measure(
        &[save(&v, 0)],
        &[s(&root.join("Projects"))],
        &[],
        CompactSettings::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!report.own_folder);
    assert_eq!(report.bytes, 1);
    assert!(report.items.is_empty());

    // A project folder itself is never a song's own, even holding one song.
    let alone = root.join("Alone");
    let one = write(&alone, "Alone.bwproject", b"1", 0);
    let report =
        measure(&[save(&one, 0)], &[s(&alone)], &[], CompactSettings::default(), None, &AtomicBool::new(false))
            .unwrap();
    assert!(!report.own_folder);
}

#[test]
fn studio_ones_cache_folder_is_one_line() {
    let root = fixtures("mo-space-studio-one");
    let song = root.join("Slow Orbit");
    let v = write(&song, "Slow Orbit.song", b"song", 0);
    write(&song, "Cache/a.peak", &[0; 20], 0);
    write(&song, "Cache/deep/b.peak", &[0; 30], 0);
    write(&song, "History/Slow Orbit (1).song", b"old", 0);
    let report = measure(
        &[save(&v, 0)],
        &[s(&root)],
        &[],
        CompactSettings { keep_backups: 1, sitting_hours: 0 },
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(report.caches, 50);
    assert_eq!(report.backups, 3);
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].kind, CompactKind::Cache);
    assert_eq!(report.items[0].bytes, 50);
    assert_eq!(key(&report.items[0].path), key(&s(&song.join("Cache"))));
}

#[test]
fn thinning_keeps_the_first_save_the_newest_and_the_last_of_each_sitting() {
    let saves: Vec<SongSave> = [0, 1, 2, 10, 11, 20]
        .iter()
        .map(|&h| SongSave { path: format!("C:/Song/S{h}.flp"), size: 1, modified: day() + TimeDelta::hours(h) })
        .collect();
    let thinned: Vec<&str> = thin(&saves, 3).iter().map(|s| s.path.as_str()).collect();
    assert_eq!(thinned, ["C:/Song/S1.flp", "C:/Song/S10.flp"]);
    assert!(thin(&saves, 0).is_empty());
    assert!(thin(&saves[..2], 3).is_empty());
}

#[test]
fn saves_a_copy_wrote_all_at_once_are_not_thinned() {
    let at = |hour: i64, seconds: i64| day() + TimeDelta::hours(hour) + TimeDelta::seconds(seconds);
    let one = |name: &str, modified| SongSave { path: format!("C:/Song/{name}.flp"), size: 1, modified };
    let thinned = |saves: &[SongSave]| -> Vec<String> { thin(saves, 3).iter().map(|s| s.path.clone()).collect() };
    // A copied folder: the copy wrote its six saves within a few seconds, so by time alone four would be thinned.
    let copied: Vec<SongSave> = (1..=6).map(|n| one(&format!("S{n}"), at(9, n))).collect();
    assert_eq!(thinned(&copied), Vec::<String>::new());
    // Saved twice more that morning: the first of those is from the middle of a sitting; the last save the copy wrote
    // stays, as its time is the copy's.
    let mut more = copied.clone();
    more.extend([one("S7", at(10, 0)), one("S8", at(10, 1800))]);
    assert_eq!(thinned(&more), ["C:/Song/S7.flp"]);
    // Three saves in a sitting, then the last two fixed at once: the last save you made stays, and so do the fixed ones.
    let fixed = [
        one("A", at(10, 0)),
        one("B", at(10, 1800)),
        one("C", at(11, 0)),
        one("B (fixed)", at(11, 1200)),
        one("C (fixed)", at(11, 1201)),
    ];
    assert_eq!(thinned(&fixed), ["C:/Song/B.flp"]);
    // A minute apart is you saving again.
    let quick: Vec<SongSave> = (0..4).map(|n| one(&format!("Q{n}"), at(9, n * mo_space::song::COPY_SECONDS))).collect();
    assert_eq!(thinned(&quick), ["C:/Song/Q1.flp", "C:/Song/Q2.flp"]);
}

#[test]
fn a_thinned_save_that_is_a_copy_or_kept_for_one_is_not_thinned() {
    let root = fixtures("mo-space-thin-copies");
    let song = root.join("Song");
    let a = write(&song, "A.bwproject", b"a", 0);
    let b = write(&song, "B.bwproject", b"same", 1);
    let c = write(&song, "C.bwproject", b"same", 2);
    let d = write(&song, "D.bwproject", b"d", 3);
    let e = write(&song, "E.bwproject", b"e", 4);
    let saves = [save(&a, 0), save(&b, 1), save(&c, 2), save(&d, 3), save(&e, 4)];
    let settings = CompactSettings { keep_backups: 5, sitting_hours: 3 };
    let report = measure(&saves, &[s(&root)], &[], settings, None, &AtomicBool::new(false)).unwrap();
    let lines: Vec<(CompactKind, &str)> =
        report.items.iter().map(|i| (i.kind, Path::new(&i.path).file_name().unwrap().to_str().unwrap())).collect();
    // B is a copy of C; neither is thinned. D is from the middle of the sitting.
    assert_eq!(lines, [(CompactKind::SameSave, "B.bwproject"), (CompactKind::ThinnedSave, "D.bwproject")]);
    assert_eq!(report.thinnable(), 1);
    assert_eq!(report.items[1].note(), "Saved 14 Mar 2026 15:00");
}

#[test]
fn a_stopped_measure_says_so() {
    let root = fixtures("mo-space-stopped");
    let v = write(&root.join("Song"), "Song.bwproject", b"1", 0);
    let stopped = measure(&[save(&v, 0)], &[s(&root)], &[], CompactSettings::default(), None, &AtomicBool::new(true));
    assert_eq!(stopped, Err(mo_space::Stopped));
}

#[test]
fn file_times_read_back_as_written() {
    let root = fixtures("mo-space-times");
    let p = write(&root, "x.flp", b"1", 2);
    let t = DateTime::<Local>::from(fs::metadata(&p).unwrap().modified().unwrap()).naive_local();
    assert!((t - (day() + TimeDelta::hours(2))).abs() < TimeDelta::seconds(2), "{t}");
}

#[test]
fn the_newest_save_is_never_thinned_even_sharing_its_moment() {
    let at = |h: i64| day() + TimeDelta::hours(h);
    let saves = [
        SongSave { path: "C:/Song/X.flp".into(), size: 1, modified: at(10) },
        SongSave { path: "C:/Song/Y.flp".into(), size: 1, modified: at(10) },
        SongSave { path: "C:/Song/W.flp".into(), size: 1, modified: at(4) },
        SongSave { path: "C:/Song/V.flp".into(), size: 1, modified: at(6) },
        SongSave { path: "C:/Song/U.flp".into(), size: 1, modified: at(5) },
    ];
    // X is the newest (listed first of the two at 10:00); sorted, it comes before Y, as if from the middle of a sitting.
    let thinned: Vec<&str> = thin(&saves, 3).iter().map(|s| s.path.as_str()).collect();
    assert_eq!(thinned, ["C:/Song/U.flp"]);
}

#[test]
fn saves_outside_the_songs_folder_still_count() {
    let root = fixtures("mo-space-saves-elsewhere");
    let old = write(&root, "Old/Song v1.bwproject", b"old!", 0);
    let new = write(&root, "Song/Song v2.bwproject", b"new", 1);
    let report = measure(
        &[save(&old, 0), save(&new, 1)],
        &[s(&root)],
        &[],
        CompactSettings::default(),
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.own_folder);
    assert_eq!(report.saves, 7);
    assert_eq!(report.bytes, 7);
}

#[test]
fn a_song_folder_that_is_a_link_isnt_its_own() {
    let root = fixtures("mo-space-linked-folder");
    write(&root, "Moved/Song.bwproject", b"1", 0);
    folder_link(&root.join("Moved"), &root.join("Projects"));
    let through = root.join("Projects/Song.bwproject").to_string_lossy().into_owned();
    let report =
        measure(&[save(&through, 0)], &[s(&root)], &[], CompactSettings::default(), None, &AtomicBool::new(false))
            .unwrap();
    assert!(!report.own_folder);
    assert_eq!(report.bytes, 1);
}

#[test]
fn of_two_identical_saves_the_one_ending_a_sitting_stays() {
    // X ends the first sitting; Y, identical, is in the middle of the second. Keeping Y (the newer) would let the next
    // Compact thin it, and the first sitting's last state would be only in the Recycle Bin.
    let root = fixtures("mo-space-twin-sitting");
    let song = root.join("Song");
    let a = write(&song, "A.bwproject", b"a", 0);
    let x = write(&song, "X.bwproject", b"same", 1);
    let y = write(&song, "Y.bwproject", b"same", 6);
    let z = write(&song, "Z.bwproject", b"z", 7);
    let saves = [save(&a, 0), save(&x, 1), save(&y, 6), save(&z, 7)];
    let settings = CompactSettings { keep_backups: 5, sitting_hours: 3 };
    let report = measure(&saves, &[s(&root)], &[], settings, None, &AtomicBool::new(false)).unwrap();
    let lines: Vec<(CompactKind, &str)> =
        report.items.iter().map(|i| (i.kind, Path::new(&i.path).file_name().unwrap().to_str().unwrap())).collect();
    assert_eq!(lines, [(CompactKind::SameSave, "Y.bwproject")]);
    assert_eq!(report.items[0].same_as.as_deref(), Some(x.as_str()));
}

#[test]
fn only_the_daws_own_autosaves_are_autosaves() {
    let root = fixtures("mo-space-own-backups");
    let song = root.join("MySong");
    let newest = write(&song, "MySong.flp", b"newest", 20);
    // FL Studio's own copies, Studio One's History and Bitwig's auto-backups are.
    write(&song, "Backup/MySong (overwritten at 2213h05).flp", b"fl", 8);
    write(&song, "History/MySong (Autosaved).song", b"s1", 9);
    write(&song, "auto-backups/MySong 3.bwproject", b"bw", 10);
    let settings = CompactSettings { keep_backups: 0, sitting_hours: 3 };
    let measure_it = || measure(&[save(&newest, 20)], &[s(&root)], &[], settings, None, &AtomicBool::new(false));
    let report = measure_it().unwrap();
    assert!(report.own_folder);
    let mut backups: Vec<&str> = report
        .items
        .iter()
        .filter(|i| i.kind == CompactKind::Backup)
        .map(|i| Path::new(&i.path).file_name().unwrap().to_str().unwrap())
        .collect();
    backups.sort_unstable();
    assert_eq!(backups, ["MySong (Autosaved).song", "MySong (overwritten at 2213h05).flp", "MySong 3.bwproject"]);

    // Your own versions in a Backups folder are yours: they aren't autosaves, and the folder isn't the song's alone
    // any more (they may use audio its saves don't), so nothing in it is offered: only FL Studio's autosaves of the
    // song, which are looked for in a Backup folder beside it either way.
    for n in 1..=3 {
        write(&song, &format!("Backups/MySong v{n}.flp"), b"mine", n);
    }
    write(&song, "Backups/take.wav", &[0; 10], 0);
    let report = measure_it().unwrap();
    assert!(!report.own_folder);
    let lines: Vec<(CompactKind, &str)> =
        report.items.iter().map(|i| (i.kind, Path::new(&i.path).file_name().unwrap().to_str().unwrap())).collect();
    assert_eq!(lines, [(CompactKind::Backup, "MySong (overwritten at 2213h05).flp")]);
}
