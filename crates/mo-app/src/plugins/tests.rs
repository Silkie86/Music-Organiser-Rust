#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use chrono::NaiveDate;
use mo_library::plugins::StoredFolder;

fn at(y: i32, m: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, m, 1).unwrap().and_hms_opt(12, 0, 0).unwrap()
}

fn now() -> NaiveDateTime {
    at(2026, 11)
}

fn save(id: i64, name: &str, daw: &str, when: NaiveDateTime) -> SaveBrief {
    SaveBrief { id, name: name.into(), path: format!(r"C:\beats\{name}.flp"), daw: daw.into(), modified: when }
}

fn uses(save: i64, name: &str, format: &str, kind: PluginKind, installed: bool, daw: &str) -> PluginUse {
    PluginUse {
        save,
        name: name.into(),
        key: mo_library::read::name_key(name),
        vendor: String::new(),
        kind,
        format: format.into(),
        installed,
        daw: daw.into(),
    }
}

fn space(name: &str, folders: &[(&str, u8, u64)]) -> StoredSpace {
    StoredSpace {
        key: mo_library::read::name_key(name),
        name: name.into(),
        vendor: String::new(),
        folders: folders
            .iter()
            .map(|(p, k, b)| StoredFolder {
                path: (*p).into(),
                kind: *k,
                bytes: *b,
                shared_with: Vec::new(),
                added: false,
            })
            .collect(),
        measured: Some(at(2026, 10)),
    }
}

/// Two songs: Didums (saves 1, 2, 3; 3 is newest) and Odd (save 4), plus Late (save 5, Bitwig).
fn library() -> Read {
    use PluginKind::*;
    let saves = vec![
        save(5, "Late", "Bitwig", at(2026, 9)),
        save(3, "Didums_3", "FL Studio", at(2025, 6)),
        save(2, "Didums_2", "FL Studio", at(2023, 1)),
        save(4, "Odd", "FL Studio", at(2022, 3)),
        save(1, "Didums", "FL Studio", at(2019, 5)),
    ];
    let song_of = HashMap::from([(1, 0), (2, 0), (3, 0), (4, 1), (5, 2)]);
    let songs = vec![
        SongBrief { name: "Didums".into(), latest: 3 },
        SongBrief { name: "Odd".into(), latest: 4 },
        SongBrief { name: "Late".into(), latest: 5 },
    ];
    let uses = vec![
        // Serum everywhere, installed.
        uses(1, "Serum", "VST3", Instrument, true, "FL Studio"),
        uses(2, "Serum", "VST3", Instrument, true, "FL Studio"),
        uses(3, "Serum", "VST3", Instrument, true, "FL Studio"),
        uses(5, "Serum", "VST3", Instrument, true, "Bitwig"),
        // Sylenth1: only in Didums' oldest save, missing. Didums' newest doesn't use it, so it opens nothing.
        uses(1, "Sylenth1", "VST2", Instrument, false, "FL Studio"),
        // Massive: the one thing Odd's newest save misses, so it opens Odd.
        uses(4, "Massive", "VST2", Instrument, false, "FL Studio"),
        // Didums' newest misses two: Nexus and Ozone 9 (another Ozone is installed).
        uses(3, "Nexus", "VST2", Instrument, false, "FL Studio"),
        uses(3, "Ozone 9", "VST3", Effect, false, "FL Studio"),
        // A DAW's own devices: one row per DAW.
        uses(3, "Delay", "Native", Effect, true, "FL Studio"),
        uses(5, "Delay", "Native", Effect, true, "Bitwig"),
        // Pro-Q 3, last used in 2019 only: installed, unused for over 2 years.
        uses(1, "Pro-Q 3", "VST3", Effect, true, "FL Studio"),
    ];
    Read {
        uses,
        saves,
        song_of,
        songs,
        spaces: vec![
            space("Serum", &[(r"C:\VST3\Serum.vst3", 0, 30_000_000), (r"C:\Users\me\Documents\Xfer", 1, 5_000_000)]),
            // The file's name has the maker in front: matched by family.
            space("FabFilter Pro-Q 3", &[(r"C:\VST3\FabFilter Pro-Q 3.vst3", 0, 40_000_000)]),
            // No song uses it at all.
            space("Kontakt 7", &[(r"C:\VST3\Kontakt 7.vst3", 0, 100_000_000), (r"D:\Libraries", 2, 50_000_000_000)]),
        ],
        installed_names: vec!["Serum".into(), "Ozone 11".into(), "FabFilter Pro-Q 3".into(), "Kontakt 7".into()],
        scans: ScanCounts { files: 120, plugins: 140, failed: 2 },
        looked_for_installed: true,
        unused_years: 2,
        needed_count: 6,
    }
}

fn plugins() -> Plugins {
    Plugins::new(library(), now())
}

fn row<'a>(p: &'a Plugins, name: &str) -> &'a PluginRow {
    p.rows.iter().find(|r| r.name == name).unwrap_or_else(|| panic!("no row {name}"))
}

#[test]
fn plugins_are_counted_by_song_not_by_save() {
    let p = plugins();
    let serum = row(&p, "Serum");
    assert_eq!(serum.saves, vec![5, 3, 2, 1]);
    assert_eq!((serum.songs, serum.songs_now), (2, 2));
    assert_eq!(serum.daws, ["Bitwig", "FL Studio"]);
    assert_eq!(serum.last_used, Some(at(2026, 9)));
    assert_eq!(serum.years, [2026, 2025, 2023, 2019]);
    // The most saves first, as the C# page.
    assert_eq!(p.rows[0].name, "Serum");
}

#[test]
fn a_daws_own_device_is_one_row_per_daw() {
    let p = plugins();
    let delays: Vec<&PluginRow> = p.rows.iter().filter(|r| r.name == "Delay").collect();
    assert_eq!(delays.len(), 2);
    assert!(delays.iter().any(|r| r.own == "Bitwig" && r.format_text() == "Bitwig native"));
}

#[test]
fn a_missing_plugin_opens_the_songs_whose_newest_save_misses_only_it() {
    let p = plugins();
    assert_eq!(row(&p, "Massive").opens, 1);
    assert_eq!(row(&p, "Nexus").opens, 0);
    // Only in an old save: no song needs it now.
    let sylenth = row(&p, "Sylenth1");
    assert_eq!((sylenth.songs, sylenth.songs_now, sylenth.opens), (1, 0, 0));

    let cards = p.needed();
    let names: Vec<&str> = cards.iter().map(|c| p.rows[c.row].name.as_str()).collect();
    // Massive opens a song, so it's first; Sylenth1 isn't needed by any newest save, so it isn't a card.
    assert_eq!(names, ["Massive", "Nexus", "Ozone 9"]);
    assert_eq!((cards[0].count, cards[0].label), (1, "song it opens"));
    assert_eq!((cards[1].count, cards[1].label), (1, "song needs it"));
    assert_eq!(cards[2].other_version, "You have Ozone 11, which may open them");
    assert_eq!(
        p.needed_summary(&cards),
        "2 of your songs are missing a plugin. Install these 3 and 2 songs open with nothing missing."
    );
    // Fewer cards: only Massive's song opens.
    let mut few = plugins();
    few.needed_count = 1;
    let cards = few.needed();
    assert_eq!(
        few.needed_summary(&cards),
        "2 of your songs are missing a plugin. Install this one and 1 song open with nothing missing."
    );
    few.needed_count = 0;
    assert!(few.needed().is_empty());
    assert!(!few.all_installed());
}

#[test]
fn sizes_go_to_their_rows_and_unused_ones_can_free_room() {
    let p = plugins();
    assert_eq!(row(&p, "Serum").bytes(), 35_000_000);
    assert!(!row(&p, "Serum").unused);
    // Named "FabFilter Pro-Q 3" on disk, "Pro-Q 3" in the songs.
    let proq = row(&p, "Pro-Q 3");
    assert_eq!(proq.bytes(), 40_000_000);
    assert!(proq.unused, "last used 2019, over 2 years ago");
    // Installed with no song using it: a row of its own.
    let kontakt = row(&p, "Kontakt 7");
    assert!(kontakt.never_used() && kontakt.unused);
    assert_eq!(kontakt.format, "VST3");
    assert_eq!(p.freeable(), "46.7 GB in 2 plugins none of your songs have used in over 2 years.");
    assert!(p.has_sizes() && p.any_missing());
    assert_eq!(p.sizes_note(), "Sizes measured 1 Oct 2026.");
    assert_eq!(p.summary(), "8 plugins used, 4 not installed on this computer, 1 installed that no song uses");

    // Five years: Pro-Q 3 (May 2019) is still unused; a year leaves Serum (Sep 2026) used.
    let mut p = plugins();
    p.set_unused_years(5);
    assert!(row(&p, "Pro-Q 3").unused && row(&p, "Kontakt 7").unused);
    p.set_unused_years(1);
    assert!(!row(&p, "Serum").unused);
    // Seen from January 2024, May 2019 is within five years: used.
    let mut read = library();
    read.unused_years = 5;
    let p = Plugins::new(read, at(2024, 1));
    assert!(!row(&p, "Pro-Q 3").unused);
}

#[test]
fn chips_search_and_years_filter_the_list() {
    let mut p = plugins();
    let shown = |p: &Plugins| p.shown.iter().map(|&i| p.rows[i].name.clone()).collect::<Vec<_>>();
    p.filters.pick(Chip::Missing);
    p.refilter();
    assert_eq!(shown(&p), ["Massive", "Nexus", "Ozone 9", "Sylenth1"]);
    // Two in one filter: either. Counts are with the other filters as they are.
    p.filters.pick(Chip::Effects);
    p.refilter();
    assert_eq!(shown(&p), ["Ozone 9"]);
    p.filters.pick(Chip::Instruments);
    p.refilter();
    assert_eq!(shown(&p), ["Massive", "Nexus", "Ozone 9", "Sylenth1"]);
    let show = p.filter(Group::Show);
    assert_eq!(
        show.chips.iter().map(|c| (c.text.as_str(), c.on)).collect::<Vec<_>>()[..2],
        [("Missing", true), ("Installed", false)]
    );
    assert_eq!(show.chips[0].count, "4");
    let kind = p.filter(Group::Kind);
    assert_eq!((kind.chips[0].count.as_str(), kind.chips[1].count.as_str()), ("3", "1"));
    // Picking it again unpicks it.
    p.filters.pick(Chip::Missing);
    assert!(!p.filters.chips.contains(&Chip::Missing));
    p.filters = Filters::default();

    // The DAW filter: Bitwig's songs only.
    p.filters.pick_daw("Bitwig");
    p.refilter();
    assert!(p.shown.iter().all(|&i| p.rows[i].daws.contains(&"Bitwig".to_owned())));
    let daws = p.daw_filter();
    assert_eq!(daws.chips.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["Bitwig", "FL Studio"]);
    assert!(daws.chips[0].on && !daws.chips[1].on);
    p.filters = Filters::default();

    p.filters.pick(Chip::BigUnused);
    p.refilter();
    // Biggest first.
    assert_eq!(p.filters.sort, Sort::Biggest);
    assert_eq!(shown(&p), ["Kontakt 7", "Pro-Q 3"]);

    p.filters = Filters { search: "  ser ".into(), ..Filters::default() };
    p.refilter();
    assert_eq!(shown(&p), ["Serum"]);
    assert_eq!(p.count_text(), format!("Showing 1 of {}", p.rows.len()));

    p.filters = Filters { years: Some((2022, 2022)), ..Filters::default() };
    p.refilter();
    assert_eq!(shown(&p), ["Massive"]);

    p.filters = Filters { chips: vec![Chip::Effects], sort: Sort::Name, ..Filters::default() };
    p.refilter();
    assert_eq!(shown(&p), ["Delay", "Delay", "Ozone 9", "Pro-Q 3"]);
    assert!(p.filters.any());
}

#[test]
fn the_card_lists_songs_newest_first_each_once() {
    let p = plugins();
    let serum = p.find("serum", "").unwrap();
    let s = p.songs_using(serum, false, "");
    assert_eq!(s.title, "Used in 2 songs");
    assert_eq!(s.note, "4 saves in all. Each song shows its newest save that uses it.");
    let names: Vec<(&str, usize)> = s.lines.iter().map(|l| (l.name.as_str(), l.saves)).collect();
    assert_eq!(names, [("Late", 1), ("Didums", 3)]);
    assert_eq!(s.lines[1].saves_text(), "3 saves");
    assert_eq!(s.daws, ["Bitwig", "FL Studio"]);

    let every = p.songs_using(serum, true, "FL Studio");
    assert_eq!(every.title, "Used in 1 song in FL Studio");
    assert_eq!(every.lines.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Didums_3", "Didums_2", "Didums"]);
    assert_eq!(every.note, "Every one of its 3 saves, newest first.");
    assert_eq!(p.saves_using(serum, "Bitwig"), HashSet::from([5]));

    let card = p.card(serum).unwrap();
    assert_eq!(card.state, State::Installed);
    assert_eq!(card.facts, "VST3 · Instrument · last used September 2026");
    assert_eq!(card.room.summary, "33.4 MB on disk · last used Sep 2026 in Late");
    assert_eq!(card.room.parts.iter().map(|r| r.name).collect::<Vec<_>>(), ["Plugin files", "Presets and settings"]);
    assert_eq!(card.room.folders[0].kind, "Plugin file");

    let ozone = p.card(p.find(&mo_library::read::name_key("Ozone 9"), "").unwrap()).unwrap();
    assert_eq!(ozone.state, State::Missing);
    assert_eq!(ozone.other_version, "You have Ozone 11, which may open these songs.");
    assert_eq!(ozone.room.summary, "Not installed, so it takes no room.");
}

#[test]
fn formats_used_are_for_the_spare_copies() {
    let used = plugins().formats_used();
    assert_eq!(used["Serum"], HashMap::from([("VST3".to_owned(), 4)]));
    assert!(!used.contains_key("Delay"), "a DAW's own device has no files");
    assert!(!used.contains_key("Kontakt 7"), "no song uses it");
}

#[test]
fn year_counts_run_newest_first() {
    let p = plugins();
    let years = p.year_counts();
    assert_eq!(years.first().map(|y| y.0), Some(2026));
    assert_eq!(years.last().map(|y| y.0), Some(2019));
}

#[test]
fn copies_read_as_card_lines() {
    let lines = copy_lines(&[ScannedPlugin {
        path: r"C:\VST3\Pro-Q 3.vst3".into(),
        format: "VST3".into(),
        id: "ABCD".into(),
        name: "Pro-Q 3".into(),
        vendor: "FabFilter".into(),
        version: "3.24".into(),
        kind: "note-effect".into(),
        shell: String::new(),
        parameters: Some(312),
        error: String::new(),
    }]);
    assert_eq!(lines[0].details, "Note effect · FabFilter · version 3.24");
    assert_eq!(lines[0].id, "ID ABCD");
    assert_eq!(lines[0].note, "312 parameters");
}

#[test]
fn search_online_asks_for_the_maker_and_name() {
    assert_eq!(
        search_url("Pro-Q 3", "FabFilter"),
        "https://www.google.com/search?q=FabFilter%20Pro-Q%203%20plugin%20download"
    );
    assert_eq!(search_url("Serum", ""), "https://www.google.com/search?q=Serum%20plugin%20download");
}

#[test]
fn nothing_read_shows_nothing() {
    let p = Plugins::new(Read::default(), now());
    assert!(p.rows.is_empty() && p.summary().is_empty() && p.needed().is_empty() && p.freeable().is_empty());
    assert!(!p.has_sizes() && !p.any_missing());
    assert!(!p.all_installed());
}

#[test]
fn review_fixes_hold() {
    use PluginKind::*;
    let mut read = library();
    // Many saves of one song don't beat two songs: Arp is in Didums' three saves, Serum in two songs.
    for id in [1, 2, 3] {
        read.uses.push(uses(id, "Arp", "VST3", NoteEffect, true, "FL Studio"));
    }
    // Late (Bitwig) uses Nexus installed, Didums' newest misses it: one song needs it, not two.
    read.uses.push(uses(5, "Nexus", "VST2", Instrument, true, "Bitwig"));
    // A measure for a plugin the library says is missing gives it no size, and adds no "never used" row.
    read.spaces.push(space("Massive", &[(r"C:\VST\Massive.dll", 0, 9_000_000)]));
    let p = Plugins::new(read, now());

    let serum = p.rows.iter().position(|r| r.name == "Serum").unwrap();
    let arp = p.rows.iter().position(|r| r.name == "Arp").unwrap();
    assert!(serum < arp, "two songs come before one song's three saves");

    let nexus = row(&p, "Nexus");
    assert_eq!((nexus.songs_now, nexus.missing_now), (2, 1));

    let massive = row(&p, "Massive");
    assert!(massive.room.is_none() && !massive.unused);
    assert_eq!(p.rows.iter().filter(|r| r.name == "Massive").count(), 1);

    // "Show them" turns Big and unused on, and keeps it on.
    let mut f = Filters::default();
    f.pick(Chip::Missing);
    f.show(Chip::BigUnused);
    f.show(Chip::BigUnused);
    assert_eq!((f.chips.as_slice(), f.sort), ([Chip::BigUnused].as_slice(), Sort::Biggest));
}

#[test]
fn another_page_can_show_one_plugin_or_every_unused_one() {
    let shown = |p: &Plugins| p.shown.iter().map(|&i| p.rows[i].name.clone()).collect::<Vec<_>>();
    let mut p = plugins();
    let kontakt = row(&p, "Kontakt 7").clone();
    let at = p.find(&kontakt.key, &kontakt.own).unwrap();
    // A search hides Kontakt 7: showing it takes the filters off, and keeps the sort.
    p.filters = Filters { search: "serum".into(), chips: vec![Chip::Missing], sort: Sort::Name, ..Filters::default() };
    p.refilter();
    assert!(!p.shown.contains(&at));
    assert_eq!(p.show_one(&kontakt.key, &kontakt.own, &kontakt.name), Some(at));
    assert!(p.shown.contains(&at));
    assert_eq!(p.filters, Filters { sort: Sort::Name, ..Filters::default() });
    // Shown already: the filters stay.
    p.filters = Filters { chips: vec![Chip::Installed], ..Filters::default() };
    p.refilter();
    assert_eq!(p.show_one(&kontakt.key, &kontakt.own, &kontakt.name), Some(at));
    assert_eq!(p.filters.chips, [Chip::Installed]);
    // One that isn't in the list any more: nothing to pick, and the list shows what has its name.
    assert_eq!(p.show_one("kontakt 6", "", "Kontakt"), None);
    assert_eq!((p.filters.search.as_str(), p.filters.chips.as_slice()), ("Kontakt", [].as_slice()));
    assert_eq!(shown(&p), ["Kontakt 7"]);

    // Every unused one, whatever else was on: the ones Space's line counts.
    p.filters = Filters { search: "serum".into(), chips: vec![Chip::Missing], ..Filters::default() };
    p.show_only_unused();
    assert_eq!(shown(&p), ["Kontakt 7", "Pro-Q 3"]);
    let space = crate::space_plugins::look(library(), now(), &[]);
    assert_eq!(space.plugins.iter().map(|u| u.name.clone()).collect::<Vec<_>>(), shown(&p));
    // And the same when the years change on the page.
    p.set_unused_years(10);
    assert_eq!(space.as_of(10, space.now).plugins.iter().map(|u| u.name.clone()).collect::<Vec<_>>(), shown(&p));
    // And when files went since the old app measured: both leave them out the same way.
    p.set_unused_years(2);
    let gone = [r"D:\Libraries".to_owned()];
    p.forget_paths(&gone);
    let sizes: Vec<(String, u64)> = p.shown.iter().map(|&i| (p.rows[i].name.clone(), p.rows[i].bytes())).collect();
    let left = space.without(&gone);
    assert_eq!(left.plugins.iter().map(|u| (u.name.clone(), u.bytes())).collect::<Vec<_>>(), sizes);
    assert_eq!(sizes, [("Kontakt 7".to_owned(), 100_000_000), ("Pro-Q 3".to_owned(), 40_000_000)]);
    // And when you rule out a folder on the page ("Not its"): Space's look counts it out the same way.
    let mut p = plugins();
    p.show_only_unused();
    let kontakt = mo_library::read::name_key("Kontakt 7");
    let fixes = [folder_fixes::FolderFix::Removed {
        key: kontakt,
        path: r"D:\Libraries".into(),
        measured: Some(self::at(2026, 10)),
    }];
    assert_eq!(p.apply_folder_fixes(&fixes), fixes);
    let sizes: Vec<(String, u64)> = p.shown.iter().map(|&i| (p.rows[i].name.clone(), p.rows[i].bytes())).collect();
    let space = crate::space_plugins::look(library(), now(), &fixes);
    assert_eq!(space.plugins.iter().map(|u| (u.name.clone(), u.bytes())).collect::<Vec<_>>(), sizes);
    assert_eq!(sizes, [("Kontakt 7".to_owned(), 100_000_000), ("Pro-Q 3".to_owned(), 40_000_000)]);
}
