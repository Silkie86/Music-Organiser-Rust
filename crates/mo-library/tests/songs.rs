//! The C# app's song tests (LibraryTests, SongVersions), case for case, so saves group into songs exactly as there.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use chrono::{Days, NaiveDate, NaiveDateTime};
use mo_library::songs::{self, SongChoices, split_key};
use mo_library::{Save, Song};

fn day(y: i32, m: u32, d: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(0, 0, 0).unwrap()
}

/// A save in `folder`, written on `saved`.
fn at(name: &str, folder: &str, ext: &str, daw: &str, saved: NaiveDateTime) -> Save {
    Save {
        name: name.into(),
        path: format!("{}\\{name}.{ext}", folder.trim_end_matches('\\')),
        daw: daw.into(),
        folder: folder.into(),
        modified: saved,
        ..Save::default()
    }
}

fn fl(name: &str, folder: &str, saved: NaiveDateTime) -> Save {
    at(name, folder, "flp", "FL Studio", saved)
}

fn bitwig(name: &str, folder: &str, saved: NaiveDateTime) -> Save {
    at(name, folder, "bwproject", "Bitwig Studio", saved)
}

fn group(saves: &[Save]) -> Vec<Song> {
    songs::group_songs(saves, &SongChoices::default())
}

fn names<'a>(saves: impl IntoIterator<Item = &'a Save>) -> Vec<&'a str> {
    saves.into_iter().map(|s| s.name.as_str()).collect()
}

#[test]
fn splits_numbered_saves() {
    let cases: &[(&str, &str, i32)] = &[
        ("Didums_70", "Didums", 70),
        ("Didums", "Didums", 1),
        ("Beauty bitch (orch)_9", "Beauty bitch (orch)", 9),
        ("a_2_3", "a_2", 3),
        ("130", "130", 1),
        ("_5", "_5", 1),
        ("civil war_5 (fixed)", "civil war", 5),
        ("Didums (Fixed 2)", "Didums", 1),
        ("Didums_70 (fixed) (fixed)", "Didums", 70),
        ("Proj_5[1]", "Proj_5[1]", 1),
        ("Track 2", "Track", 2),
        ("track2", "track", 2),
        ("track nd 1", "track nd", 1),
        ("song-3", "song", 3),
        ("song - 3", "song", 3),
        ("analog full set nb 36", "analog full set nb", 36),
        ("MO Live Test 1", "MO Live Test", 1),
        ("Swu 10 year oct 2026", "Swu 10 year oct 2026", 1),
        ("Untitled 3", "Untitled 3", 1),
        ("Untitled_2", "Untitled", 2),
        ("fleck athens FLP19", "fleck athens FLP", 19),
        ("fleck athens FLP", "fleck athens FLP", 1),
        ("fleck athens flp_15", "fleck athens flp", 15),
        ("Night Drive v2", "Night Drive", 2),
        ("Night Drive_V3", "Night Drive", 3),
        ("Night Drive v2.1", "Night Drive", 2),
        ("Night DriveV4", "Night Drive", 4),
        ("Night Drive take 4", "Night Drive", 4),
        ("Night Drive - mix 2", "Night Drive", 2),
        ("Night Drive rev7", "Night Drive", 7),
        ("Night Drive final", "Night Drive", 1_000_000),
        ("Night Drive FINAL FINAL", "Night Drive", 1_000_000),
        ("Night Drive final2", "Night Drive", 1_000_002),
        ("Night Drive v2 final v3", "Night Drive", 1_000_003),
        ("Night Drive - Copy", "Night Drive", 1),
        ("Night Drive_12 - Copy (2)", "Night Drive", 12),
        ("Night Drive (2)", "Night Drive", 1),
        ("Night Drive wip", "Night Drive", 1),
        ("Night Drive 2024-03-01", "Night Drive", 1),
        ("Night Drive_20240301", "Night Drive", 1),
        ("Track final", "Track", 1_000_000),
        ("Untitled final", "Untitled final", 1),
        ("Swu oct 2026", "Swu oct 2026", 1),
    ];
    for &(name, song, number) in cases {
        assert_eq!(songs::split(name), (song.to_owned(), number), "{name}");
    }
}

#[test]
fn groups_numbered_saves_under_the_newest() {
    let p = |name: &str, folder: &str, d: u32| fl(name, folder, day(2020, 1, d));
    let groups = group(&[
        p("Didums", r"C:\dub", 9),
        p("Didums_10", r"C:\dub", 2),
        p("Didums_2", r"C:\dub", 3),
        p("Night Drive", r"C:\dub", 1),
        p("Didums_3", r"C:\other", 1),
    ]);
    assert_eq!(groups.len(), 3);
    let didums: Vec<_> = groups.iter().filter(|g| g.name == "Didums" && g.latest.folder == r"C:\dub").collect();
    assert_eq!(didums.len(), 1);
    assert_eq!(didums[0].latest.name, "Didums_10");
    assert_eq!(names(&didums[0].older), ["Didums_2", "Didums"]);
}

#[test]
fn bitwig_saves_each_in_a_folder_of_its_own_name_are_one_song() {
    let projects = r"K:\Bitwig Studio\Projects";
    let p = |name: &str, folder: String| bitwig(name, &folder, day(2020, 1, 1));
    let groups = group(&[
        p("bored_57", format!(r"{projects}\bored_57")),
        p("bored_58", format!(r"{projects}\bored_58")),
        p("bored", format!(r"{projects}\bored")),
        p("bored_59", format!(r"{projects}\bored")),
        p("MO Live Test 1", format!(r"{projects}\MO Live Test 1")),
        p("bored_2", format!(r"{projects}\Old\bored stuff")),
    ]);
    assert_eq!(groups.len(), 3);
    let bored = groups.iter().find(|g| g.latest.name == "bored_59").unwrap();
    assert_eq!(names(&bored.older), ["bored_58", "bored_57", "bored"]);
    // A folder named otherwise is a song of its own.
    assert!(groups.iter().any(|g| g.latest.name == "bored_2"));
}

#[test]
fn saves_numbered_flp1_to_flp20_join_the_unnumbered_flp_save_as_one_song() {
    let folder = r"K:\Bitwig Studio\Projects\fleck athens";
    let p = |name: String, d: u64, own: Option<String>| {
        bitwig(&name, own.as_deref().unwrap_or(folder), day(2025, 3, 1).checked_add_days(Days::new(d)).unwrap())
    };
    let mut saves: Vec<Save> = (1..=19).map(|n| p(format!("fleck athens FLP{n}"), n, None)).collect();
    saves.push(p("fleck athens FLP".into(), 0, None));
    saves.push(p("fleck athens FLP20".into(), 20, Some(format!(r"{folder}\fleck athens FLP20"))));
    let groups = group(&saves);
    assert_eq!(groups.len(), 1);
    let song = &groups[0];
    assert_eq!(song.name, "fleck athens FLP");
    assert_eq!(song.latest.name, "fleck athens FLP20");
    assert_eq!(song.older.len(), 20);
    assert_eq!(song.older.last().unwrap().name, "fleck athens FLP");
}

#[test]
fn saves_named_alike_apart_from_their_number_are_one_song_only_in_the_same_folder() {
    let groups = group(&[
        fl("track1", r"C:\beats", day(2025, 3, 1)),
        fl("track2", r"C:\beats", day(2025, 3, 2)),
        fl("track nd 1", r"C:\beats", day(2025, 3, 3)),
        fl("track3", r"C:\other", day(2025, 3, 4)),
    ]);
    assert_eq!(groups.len(), 3);
    let track = groups.iter().find(|g| g.latest.folder == r"C:\beats" && g.name == "track").unwrap();
    assert_eq!(track.latest.name, "track2");
    assert_eq!(names(&track.older), ["track1"]);
}

fn song(name: &str, folder: &str, d: u32) -> Save {
    fl(name, folder, day(2025, 3, d))
}

#[test]
fn songs_whose_names_are_close_are_asked_about_but_not_ones_you_said_are_different() {
    let groups = group(&[
        song("track nd 1", r"C:\beats", 1),
        song("track 2", r"C:\beats", 2),
        song("Sunrise", r"C:\beats", 3),
        song("Sunrize", r"C:\beats", 4),
        song("Sunrise", r"C:\other", 5),
        song("bass", r"C:\beats", 6),
        song("bossa", r"C:\beats", 7),
        song("Untitled", r"C:\beats", 8),
        song("Untitled 2", r"C:\beats", 9),
    ]);
    let none = SongChoices::default();
    let doubts = songs::doubts(&groups, &none, false, 200);
    assert_eq!(doubts.len(), 2);
    assert_eq!((doubts[0].a.name.as_str(), doubts[0].b.name.as_str()), ("Sunrize", "Sunrise"));
    assert_eq!(doubts[0].why, "The names are one letter apart");
    assert_eq!((doubts[1].a.name.as_str(), doubts[1].b.name.as_str()), ("track", "track nd"));
    assert_eq!(doubts[1].why, "The names differ only by \"nd\"");

    // Across folders only when asked for.
    assert!(songs::doubts(&groups, &none, true, 200).iter().any(|d| d.why == "Same name in different folders"));

    let apart = songs::pair(&doubts[1].a.key, &doubts[1].b.key);
    let choices = SongChoices::from_rows([("apart", apart.as_str(), "")]);
    let left: Vec<_> = songs::doubts(&groups, &choices, false, 200).into_iter().map(|d| d.a.name).collect();
    assert_eq!(left, ["Sunrize"]);
}

#[test]
fn an_old_final_save_doesnt_open_over_newer_saves_and_the_app_asks_about_it() {
    let p = |name: &str, saved| fl(name, r"C:\beats", saved);
    let mut saves = vec![p("Night Drive final", day(2023, 5, 1)), p("Night Drive_70 - Copy", day(2025, 6, 1))];
    saves.extend(
        (2..=14).map(|n| p(&format!("Night Drive_{n}"), day(2025, 1, 1).checked_add_days(Days::new(n)).unwrap())),
    );
    saves.push(p("Night Drive_70", day(2025, 6, 1)));
    let groups = group(&saves);
    assert_eq!(groups.len(), 1);
    let one = &groups[0];
    // The newest by date; its copy, saved with it, comes after.
    assert_eq!(one.latest.name, "Night Drive_70");

    let none = SongChoices::default();
    let asked = songs::doubts(std::slice::from_ref(one), &none, false, 200);
    let mut asked_names: Vec<_> = asked.iter().map(|d| d.b.name.as_str()).collect();
    asked_names.sort();
    assert_eq!(asked_names, ["Night Drive final", "Night Drive_70 - Copy"]);
    assert!(asked.iter().all(|d| d.one_save));
    assert!(asked.iter().any(|d| d.why == "It was put with Night Drive because its name adds \"final\""));

    // Answered: final is a save of the song (not asked again); the copy is its own song.
    let final_key = &asked.iter().find(|d| d.b.name == "Night Drive final").unwrap().b.key;
    let apart = songs::pair(&one.key, final_key);
    let copy_path = saves[1].path.clone();
    let choices = SongChoices::from_rows([("apart", apart.as_str(), ""), ("split", copy_path.as_str(), "")]);
    let regrouped = songs::group_songs(&saves, &choices);
    assert!(songs::doubts(&regrouped, &choices, false, 200).is_empty());
    assert_eq!(regrouped.len(), 2);

    // Saves that all carry the same mark have nothing to ask.
    let marked = group(&[p("Dub v2", day(2025, 1, 1)), p("Dub v3", day(2025, 2, 1))]);
    assert!(songs::doubts(&marked, &none, false, 200).is_empty());
}

#[test]
fn saves_at_a_drives_root_group_with_ones_in_a_folder_of_their_own_name() {
    let groups = group(&[bitwig("beat", r"C:\beat", day(2025, 1, 1)), bitwig("beat_2", r"C:\", day(2025, 1, 1))]);
    assert_eq!(groups.len(), 1);
}

#[test]
fn a_name_ending_in_a_non_english_digit_is_a_name_not_a_crash() {
    assert_eq!(songs::split("beat_٢"), ("beat_٢".to_owned(), 1));
    assert_eq!(songs::split("曲１"), ("曲１".to_owned(), 1));
}

#[test]
fn you_can_join_songs_split_a_save_off_and_move_a_save_and_reset_undoes_it() {
    let saves = [
        song("track nd 1", r"C:\beats", 1),
        song("track 2", r"C:\beats", 2),
        song("track 3", r"C:\beats", 3),
        song("bossa", r"C:\beats", 4),
    ];
    let start = group(&saves);
    let key_of = |groups: &[Song], name: &str| groups.iter().find(|g| g.name == name).unwrap().key.clone();
    let (track, nd) = (key_of(&start, "track"), key_of(&start, "track nd"));

    // "track nd" goes under "track": one song, called track, newest track 3.
    let joined = SongChoices::from_rows([("join", nd.as_str(), track.as_str())]);
    let groups = songs::group_songs(&saves, &joined);
    assert_eq!(groups.len(), 2);
    let t = groups.iter().find(|g| g.name == "track").unwrap();
    assert_eq!(names(&t.older), ["track 2", "track nd 1"]);
    assert!(joined.changed(&t.key, t.saves().map(|s| s.path.as_str())));
    // Joining back would make a loop: following either key ends at track.
    assert_eq!(joined.follow(&nd), track);

    // track 2 split off into a song of its own, then moved under bossa.
    let split_path = saves[1].path.clone();
    let split = SongChoices::from_rows([("join", nd.as_str(), track.as_str()), ("split", split_path.as_str(), "")]);
    assert_eq!(songs::group_songs(&saves, &split).len(), 3);
    let bossa = key_of(&start, "bossa");
    let own = split_key(&saves[1].path);
    let moved = SongChoices::from_rows([
        ("join", nd.as_str(), track.as_str()),
        ("split", split_path.as_str(), ""),
        ("join", own.as_str(), bossa.as_str()),
    ]);
    let groups = songs::group_songs(&saves, &moved);
    let b = groups.iter().find(|g| g.name == "bossa").unwrap();
    // Saved last, whatever the numbers.
    assert_eq!((b.latest.name.as_str(), names(&b.older)), ("bossa", vec!["track 2"]));

    // With no choices it's the guess again.
    let groups = group(&saves);
    assert_eq!(groups.len(), 3);
    assert_eq!(names(&groups.iter().find(|g| g.name == "track").unwrap().older), ["track 2"]);
}

#[test]
fn bitwig_saves_numbered_with_a_space_are_one_song() {
    let p = |name: String| bitwig(&name, &format!(r"K:\Bitwig Studio\Projects\{name}"), day(2026, 10, 2));
    let mut saves: Vec<Save> = (26..=36).map(|n| p(format!("analog full set nb {n}"))).collect();
    saves.extend(["Swu 10 year oct 2026", "Untitled 1", "Untitled 2"].map(|n| p(n.into())));
    let groups = group(&saves);
    assert_eq!(groups.len(), 4);
    let s = groups.iter().find(|g| g.name == "analog full set nb").unwrap();
    assert_eq!(s.latest.name, "analog full set nb 36");
    assert_eq!(s.older.len(), 10);
}

/// A save of Didums with its counts, as the C# tests make them.
fn counted(name: &str, saved: NaiveDateTime) -> Save {
    Save {
        note_count: 100,
        length_bars: 32,
        track_count: 6,
        plugin_names: vec!["Serum".into(), "Fruity Reverb 2".into()],
        ..fl(name, r"C:\dub", saved)
    }
}

#[test]
fn saves_three_months_apart_start_a_new_run() {
    let saves = [
        Save { note_count: 400, length_bars: 96, ..counted("Didums_77", day(2024, 8, 3)) },
        Save { note_count: 20, ..counted("Didums", day(2019, 1, 10)) },
        counted("Didums_12", day(2019, 2, 2)),
        counted("Didums_13", day(2019, 4, 30)),
        Save { note_count: 380, length_bars: 90, ..counted("Didums_40", day(2024, 7, 20)) },
    ];
    let runs = songs::runs(&saves, songs::DEFAULT_RUN_GAP_MONTHS);
    let got: Vec<_> = runs.iter().map(|r| (r.label(), r.last.name.as_str(), r.saves)).collect();
    assert_eq!(got, [("2019 original".to_owned(), "Didums_13", 3), ("2024 rework".to_owned(), "Didums_77", 2)]);
    assert_eq!(runs[1].details(), "Didums_77 · 3 Aug 2024 · 2 saves");
}

#[test]
fn a_shorter_gap_from_settings_splits_saves_into_more_versions() {
    let saves = [
        counted("Didums", day(2019, 1, 10)),
        counted("Didums_12", day(2019, 2, 20)),
        counted("Didums_13", day(2019, 4, 30)),
    ];
    assert_eq!(songs::runs(&saves, 3).len(), 1);
    assert_eq!(songs::runs(&saves, 2).len(), 2);
    assert_eq!(songs::runs(&saves, 1).len(), 3);
}

#[test]
fn resaving_in_a_newer_daw_is_not_a_rework() {
    let original = counted("Didums_12", day(2019, 2, 2));
    let resaved = Save {
        note_count: 104,
        length_bars: 34,
        plugin_names: vec!["Serum 2".into(), "Fruity Reverb 2 (x64)".into()],
        ..counted("Didums_13", day(2024, 8, 3))
    };
    assert!(!songs::is_reworked(&original, &resaved));
    assert_eq!(songs::runs([&original, &resaved], 3)[1].label(), "2024 re-save");

    let x = || counted("x", NaiveDateTime::default());
    let three = vec!["Serum".into(), "Fruity Reverb 2".into(), "Vital".into()];
    assert!(songs::is_reworked(&original, &Save { plugin_names: three, ..x() }));
    assert!(songs::is_reworked(&original, &Save { track_count: 7, ..x() }));
    assert!(songs::is_reworked(&original, &Save { note_count: 121, ..x() }));
    assert!(!songs::is_reworked(&original, &Save { note_count: 120, ..x() }));
    assert!(songs::is_reworked(&original, &Save { length_bars: 41, ..x() }));
    assert!(!songs::is_reworked(&original, &Save { note_count: -1, length_bars: -1, ..x() }));
}

#[test]
fn plugin_versions_are_the_same_plugin() {
    for (name, family) in [
        ("Serum 2", "serum"),
        ("Serum (x64)", "serum"),
        ("Pro-Q 3", "pro-q"),
        ("Kontakt 7.10.1", "kontakt"),
        ("Sylenth1_x64", "sylenth"),
        ("Tau", "tau"),
        ("3x Osc", "3x osc"),
        ("808", "808"),
    ] {
        assert_eq!(songs::plugin_family(name), family, "{name}");
    }
}

#[test]
fn a_song_is_as_old_as_its_first_save_or_the_date_its_file_keeps() {
    let saves = [counted("Didums_77", day(2024, 8, 3)), counted("Didums", day(2019, 1, 10))];
    assert_eq!(songs::first_made(&saves), Some(day(2019, 1, 10)));
    // Every save was re-saved in 2024, but FL Studio kept the day it was made.
    let kept = Save { created: Some(day(2018, 11, 5)), ..counted("Didums", day(2024, 8, 3)) };
    assert_eq!(songs::first_made([&kept]), Some(day(2018, 11, 5)));
}

// Beyond the C# app (Solomon, 2026-10-31: improvements 2 and 3).

/// A save with its size and counts, so it can be told apart from other files.
fn sized(name: &str, folder: &str, size: i64, saved: NaiveDateTime) -> Save {
    Save { size, note_count: 50, track_count: 4, ..fl(name, folder, saved) }
}

#[test]
fn the_same_file_in_a_copied_folder_is_one_save_kept_aside_not_a_second_song() {
    let saves = [
        sized("Didums_3", r"D:\Backup\dub", 300, day(2025, 5, 1)),
        sized("Didums_3", r"C:\dub", 300, day(2025, 4, 1)),
        sized("Didums_2", r"C:\dub", 200, day(2025, 3, 1)),
        sized("Didums_2", r"D:\Backup\dub", 200, day(2025, 5, 1)),
        // Same name, but a different file: still a save of its own folder's song.
        sized("Other", r"C:\dub", 10, day(2025, 1, 1)),
        sized("Other", r"D:\Backup\dub", 11, day(2025, 1, 1)),
    ];
    let groups = group(&saves);
    let didums: Vec<_> = groups.iter().filter(|g| g.name == "Didums").collect();
    assert_eq!(didums.len(), 1);
    let d = didums[0];
    assert_eq!((d.save_count(), d.copies.len()), (2, 2));
    assert_eq!((d.latest.name.as_str(), d.latest.folder.as_str()), ("Didums_3", r"D:\Backup\dub"));
    assert_eq!(groups.iter().filter(|g| g.name == "Other").count(), 2);

    // The two "Other" songs sit in folders that are copies of each other, so the app asks about them.
    let asked = songs::doubts(&groups, &SongChoices::default(), false, 200);
    assert_eq!(asked.len(), 1);
    assert_eq!((asked[0].a.name.as_str(), asked[0].why.as_str()), ("Other", "Same name in a copied folder"));

    // A file that's gone, or empty, can't be told apart: no joining on it.
    let gone = [
        Save { file_missing: true, ..sized("Song", r"C:\a", 5, day(2025, 1, 1)) },
        sized("Song", r"C:\b", 5, day(2025, 1, 1)),
        sized("Tune", r"C:\a", 0, day(2025, 1, 1)),
        sized("Tune", r"C:\b", 0, day(2025, 1, 1)),
    ];
    assert_eq!(group(&gone).len(), 4);

    // Songs you kept apart stay apart.
    let keys: Vec<String> = group(&saves[..2]).iter().map(|g| g.key.clone()).collect();
    assert_eq!(keys.len(), 1);
    let (a, b) =
        (songs::key(r"D:\Backup\dub", "FL Studio", "Didums_3"), songs::key(r"C:\dub", "FL Studio", "Didums_3"));
    let apart = songs::pair(&a, &b);
    assert_eq!(songs::group_songs(&saves[..2], &SongChoices::from_rows([("apart", apart.as_str(), "")])).len(), 2);
}

#[test]
fn a_renamed_save_is_asked_about_when_its_plugins_channels_and_tempo_match() {
    let with = |name: &str, d: u32, tempo: f64, tracks: i32, plugins: &[&str]| Save {
        tempo: Some(tempo),
        track_count: tracks,
        plugin_names: plugins.iter().map(|p| (*p).to_owned()).collect(),
        ..song(name, r"C:\beats", d)
    };
    let kit = ["Serum", "Vital", "Pro-Q 3", "Fruity Reverb 2", "Kickstart"];
    let groups = group(&[
        with("Untitled_5", 1, 140.0, 8, &kit),
        with("Sunset", 2, 140.0, 9, &["Serum 2", "Vital", "Pro-Q 3", "Fruity Reverb 2", "Kickstart"]),
        // Another tempo, or other plugins: a different song.
        with("Rain", 3, 128.0, 8, &kit),
        with("Storm", 4, 140.0, 8, &["Massive", "Sylenth1", "Ozone 11"]),
    ]);
    let asked = songs::doubts(&groups, &SongChoices::default(), false, 200);
    assert_eq!(asked.len(), 1);
    assert_eq!((asked[0].a.name.as_str(), asked[0].b.name.as_str()), ("Sunset", "Untitled"));
    assert_eq!(asked[0].why, "They use nearly the same plugins, channels and tempo");
}
