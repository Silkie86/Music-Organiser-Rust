//! What the FL Studio reader finds in a project: the C# `FlStudioTests`, `FlStudio25Tests` and `FlStudio9Tests` that
//! cover reading a project's facts, on the same made-up files.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeMap;

use common::*;
use mo_formats::fl::{self, events};
use mo_formats::{Confidence, PluginKind, PluginRef, ProjectInfo};

fn read(bytes: &[u8]) -> ProjectInfo {
    fl::read_bytes(bytes)
}

fn by_name(info: &ProjectInfo) -> BTreeMap<String, PluginRef> {
    info.plugins.iter().map(|p| (p.name.clone(), p.clone())).collect()
}

#[test]
fn reads_tempo_version_and_channels() {
    let info = read(&fl_studio_project());
    assert_eq!(info.tempo, Some(128.0));
    assert_eq!(info.version, "21.2.3.4004");
    assert_eq!(info.title, "Night Drive (title)");
    assert_eq!(info.tracks, ["Lead", "Kick", "Bass"]);
    assert_eq!(info.details["Notes"], "4");
    assert_eq!(info.details["Patterns"], "2");
    assert_eq!(info.details["Channels"], "3");
    assert_eq!(info.details["PPQ"], "96");
    assert_eq!(info.confidence, Confidence::Full);
    assert!(info.warnings.is_empty());
}

#[test]
fn finds_plugins_and_their_roles() {
    let plugins = by_name(&read(&fl_studio_project()));
    assert_eq!(plugins.len(), 4);
    let serum = &plugins["Serum"];
    assert_eq!(
        (serum.kind, serum.vendor.as_str(), serum.format.as_str()),
        (PluginKind::Instrument, "Xfer Records", "VST2")
    );
    assert_eq!(serum.path, r"C:\Program Files\VstPlugins\Serum_x64.dll");
    assert_eq!((plugins["3x Osc"].kind, plugins["3x Osc"].format.as_str()), (PluginKind::Instrument, "Native"));
    assert_eq!(plugins["Fruity Limiter"].kind, PluginKind::Effect);
    assert_eq!((plugins["Pro-Q 3"].kind, plugins["Pro-Q 3"].format.as_str()), (PluginKind::Effect, "VST2"));
}

#[test]
fn finds_samples() {
    assert_eq!(read(&fl_studio_project()).samples, [r"C:\Samples\Drums\Kick 01.wav"]);
}

#[test]
fn lists_empty_and_damaged_files_with_whats_wrong() {
    let mut junk = b"FLhd".to_vec();
    junk.extend([6, 0, 0, 0, 0, 0, 3, 0, 96, 0]);
    junk.extend([0; 8]);
    junk.extend([0xff, 0xff, 0xff, 0xff, 0x7f, 1, 2, 3]);
    let mut index = b"INDX".to_vec();
    index.extend([0x28, 0, 0x09, 0]);
    index.extend([0; 40]);
    let files: [(&str, Vec<u8>); 4] = [
        ("empty", vec![]),
        ("text", b"not a project".to_vec()),
        // What disk damage left in one: an NTFS directory index record.
        ("index", index),
        // FL's header, then neither an FLdt chunk nor events with FL's version in them.
        ("junk", junk),
    ];
    for (name, bytes) in files {
        let info = read(&bytes);
        assert_eq!(info.confidence, Confidence::Catalogue, "{name}");
        assert_eq!(info.warnings, [format!("{}.", events::DAMAGED)], "{name}");
        assert!(!info.can_extract_midi, "{name}");
        assert!(events::load(&bytes).is_err(), "{name}");
    }
}

#[test]
fn reads_old_saves_with_a_garbled_data_header() {
    // Some FL Studio 4 to 7 saves have what looks like a memory address and a size where "FLdt" and its length belong.
    let good = Flp::new()
        .ascii(199, "7.0.2")
        .word(66, 140)
        .ascii(194, "Side")
        .word(64, 0)
        .byte(21, 0)
        .ascii(192, "Kick")
        .ascii(196, r"C:\Samples\Kick.wav")
        .word(65, 1)
        .data(224, &note(0, 0, 96, 60, 100)[..20]) // FL 7's notes are 20-byte records (rule 9)
        .data(236, &[0; 12]) // its mixer: FL got to the end
        .build();
    let mut garbled = good.clone();
    garbled[14..22].copy_from_slice(&[0x60, 0xfb, 0x12, 0x00, 0xd9, 0x4a, 0x06, 0x01]);

    let (a, b) = (read(&good), read(&garbled));
    assert_eq!(
        (b.version.as_str(), b.tempo, b.title.as_str(), b.confidence),
        ("7.0.2", Some(140.0), "Side", Confidence::Full)
    );
    assert_eq!(b.tracks, ["Kick"]);
    assert_eq!((&a.tracks, &a.samples, &a.details), (&b.tracks, &b.samples, &b.details));
    assert!(a.warnings.is_empty());
    assert_eq!(b.warnings, [events::GARBLED_HEADER]);
    assert_eq!(b.details["Notes"], "1");
}

#[test]
fn a_garbled_save_fl_stopped_writing_before_the_mixer_says_so() {
    let mut cut = Flp::new().ascii(199, "7.0.2").word(64, 0).ascii(192, "Kick").build();
    cut[14..22].copy_from_slice(&[0x60, 0xfb, 0x12, 0x00, 0xd9, 0x4a, 0x06, 0x01]);
    let info = read(&cut);
    assert_eq!(info.warnings, [events::STOPPED_PART_WAY]);
    assert_eq!(info.tracks, ["Kick"]);
}

#[test]
fn survives_truncated_files_and_says_so() {
    let bytes = fl_studio_project();
    let info = read(&bytes[..bytes.len() - 30]);
    assert_eq!(info.tempo, Some(128.0));
    assert_eq!(info.confidence, Confidence::Full);
    assert_eq!(info.warnings, [events::CUT_SHORT]);

    // An event whose size never ends is damage too.
    let damaged =
        Flp::new().ascii(199, "21.0.0").dword(156, 120_000).raw(&[213, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]).build();
    let info = read(&damaged);
    assert_eq!(info.tempo, Some(120.0));
    assert_eq!(info.warnings, [events::CUT_SHORT]);
}

#[test]
fn a_zero_filled_stretch_ends_the_reading() {
    // An interrupted copy: the rest of the data is zeros, which would read as millions of 2-byte events.
    let bytes = Flp::new().ascii(199, "21.0.0").dword(156, 120_000).raw(&vec![0; 100_000]).dword(156, 90_000).build();
    let flp = events::load(&bytes).expect("an FL project");
    assert!(flp.events.len() < 10, "{} events", flp.events.len());
    assert_eq!(flp.warning, Some(events::CUT_SHORT));
    assert_eq!(read(&bytes).tempo, Some(120.0));
    // A few zero events in a row are FL's own.
    let few = Flp::new().ascii(199, "21.0.0").raw(&[0; 40]).dword(156, 90_000).build();
    assert_eq!((read(&few).tempo, read(&few).warnings.len()), (Some(90.0), 0));
}

#[test]
fn text_before_fl_11_5_is_in_windows_code_page_1252() {
    let bytes = Flp::new()
        .ascii(199, "9.0.0")
        .data(194, b"Don\x92t \x96 \x80\xe9\0")
        .word(64, 0)
        .data(196, b"C:\\Samples\\Don\x92t Stop.wav\0")
        .build();
    let info = read(&bytes);
    assert_eq!(info.title, "Don\u{2019}t \u{2013} \u{20AC}\u{e9}");
    assert_eq!(info.samples, ["C:\\Samples\\Don\u{2019}t Stop.wav"]);
}

#[test]
fn patterns_named_again_in_each_channel_count_once() {
    // FL 9 names the pattern in every channel's events, before its steps.
    let mut b = Flp::new().ascii(199, "9.0.0");
    for channel in 0..3 {
        b = b.word(64, channel).word(65, 1).word(65, 2);
    }
    assert_eq!(read(&b.build()).details["Patterns"], "2");
}

#[test]
fn a_garbled_save_that_is_also_cut_short_says_both() {
    let bytes =
        Flp::new().ascii(199, "7.0.2").word(64, 0).ascii(192, "Kick").data(236, &[0; 12]).data(233, &[0; 40]).build();
    let mut garbled = bytes[..bytes.len() - 10].to_vec();
    garbled[14..22].copy_from_slice(&[0x60, 0xfb, 0x12, 0x00, 0xd9, 0x4a, 0x06, 0x01]);
    assert_eq!(read(&garbled).warnings, [events::GARBLED_HEADER, events::CUT_SHORT]);
}

#[test]
fn a_save_cut_short_lists_no_sample_as_unplayed() {
    let song = |cut: usize| {
        let bytes = Flp::new()
            .ascii(199, "7.0.2")
            .word(64, 0)
            .ascii(192, "Vocal")
            .ascii(196, r"C:\Samples\Vocal.wav")
            .data(233, &[0; 40])
            .build();
        read(&bytes[..bytes.len() - cut])
    };
    assert_eq!(song(0).unplayed.len(), 1, "a whole save with no notes for it");
    let cut = song(10);
    assert_eq!(cut.warnings, [events::CUT_SHORT]);
    assert!(cut.unplayed.is_empty(), "{:?}", cut.unplayed);
}

#[test]
fn notes_above_midis_top_key_are_left_out_with_a_warning() {
    let notes: Vec<u8> = [60, 130].iter().flat_map(|&k| note(0, 0, 96, k, 100)).collect();
    let bytes = Flp::new().ascii(199, "21.0.0").word(64, 0).ascii(192, "Lead").word(65, 1).data(224, &notes).build();
    let info = read(&bytes);
    assert_eq!(info.details["Notes"], "1", "as many as are read");
    assert_eq!(info.warnings, ["1 note is above G10, the top of MIDI's keys, so they're left out."]);
    // A damaged note event says so too (rule 9): an FL 7 note and 8 bytes of damage.
    let bytes = Flp::new()
        .ascii(199, "7.0.2")
        .word(64, 0)
        .word(65, 1)
        .data(224, &[&note(0, 0, 96, 60, 100)[..20], &[0; 8]].concat())
        .build();
    let info = read(&bytes);
    assert_eq!(info.details["Notes"], "1");
    assert!(info.warnings[0].starts_with("A pattern's notes end part-way"), "{:?}", info.warnings);
}

#[test]
fn fl_25_writes_event_172_with_three_bytes() {
    // Read as four bytes, the 172 swallows the title's id and garbles everything after it.
    let info = read(&fl_studio_25_nothing_saved());
    assert_eq!(info.tempo, Some(98.0));
    assert_eq!(info.tracks, ["Cell"]);
    assert_eq!(info.details["Notes"], "1");
}

#[test]
fn finds_vst_plugins_in_newer_wrapper_states() {
    let plugins = by_name(&read(&fl_studio_25_project()));
    let serum = &plugins["Serum"];
    assert_eq!(
        (serum.vendor.as_str(), serum.format.as_str(), serum.kind),
        ("Xfer Records", "VST2", PluginKind::Instrument)
    );
    assert_eq!((plugins["Arcade"].vendor.as_str(), plugins["Arcade"].format.as_str()), ("Output", "VST3"));
    assert_eq!(plugins["Predator 2"].kind, PluginKind::Instrument);
}

#[test]
fn resolves_the_user_data_folder_from_the_projects_location() {
    let dir = test_dir("user-data");
    let projects = dir.join("FL Studio").join("Projects");
    std::fs::create_dir_all(&projects).expect("make the folders");
    std::fs::write(projects.join("Snare 01.wav"), [0]).expect("write the sample");
    let path = projects.join("Shuffle.flp");
    std::fs::write(&path, fl_studio_25_project()).expect("write the project");

    let info = fl::read(&path).expect("the file is there");
    let snare = projects.join("Snare 01.wav").to_string_lossy().into_owned();
    assert_eq!(info.samples, [snare.as_str()]);
    // The snare has no notes: what's said of it is under the same full path.
    assert_eq!(info.unplayed.keys().collect::<Vec<_>>(), [&snare]);
    // Without the disk, it stays as FL wrote it.
    assert_eq!(read(&fl_studio_25_project()).samples, [r"%FLStudioUserData%\Projects\Snare 01.wav"]);

    // A folder no folder above the project has: left as written. (A made-up name, so no real folder can fit.)
    let mut user_data = fl::UserData::new(&path);
    let samples: Vec<String> = [
        r"%FLStudioUserData%\MO Test Nowhere 5c1e\Kick.wav",
        r"%flstudiouserdata%/Projects/Snare 01.wav",
        r"C:\Samples\Hat.wav",
    ]
    .iter()
    .map(|s| user_data.resolve(s))
    .collect();
    assert_eq!(
        samples,
        [
            r"%FLStudioUserData%\MO Test Nowhere 5c1e\Kick.wav".to_owned(),
            projects.join("Snare 01.wav").to_string_lossy().into_owned(),
            r"C:\Samples\Hat.wav".to_owned(),
        ]
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn one_sample_written_two_ways_plays_once() {
    let dir = test_dir("two-ways");
    let projects = dir.join("FL Studio").join("Projects");
    std::fs::create_dir_all(&projects).expect("make the folders");
    std::fs::write(projects.join("Snare 01.wav"), [0]).expect("write the sample");
    let snare = projects.join("Snare 01.wav").to_string_lossy().into_owned();
    let path = projects.join("Two ways.flp");
    std::fs::write(&path, fl_studio_25_one_sample_two_ways(&snare)).expect("write the project");

    let info = fl::read(&path).expect("the file is there");
    assert_eq!(info.samples, [snare.as_str()]);
    // Channel 0 plays it, so it isn't unplayed though channel 1 has no notes.
    assert!(info.unplayed.is_empty(), "{:?}", info.unplayed);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn reads_fl_9_step_sequencer_drums_as_notes() {
    let bytes = fl_studio_9_steps();
    let info = read(&bytes);
    assert_eq!(info.details["Notes"], "3");
    assert_eq!(info.tracks, ["Kick"]);
    assert_eq!(info.tempo, Some(140.0));

    // The dots become one note event right after them: steps 0, 5 and 7 at 96 PPQ, odd steps swung 3 ticks by
    // shuffle 36, step 7 also shifted 12, step 5 at volume 52.
    let flp = events::load(&bytes).expect("an FL project");
    let made: Vec<_> = flp.events.iter().filter(|e| e.offset.is_none()).collect();
    assert_eq!(made.len(), 1);
    let notes: Vec<(u32, u8)> = made[0]
        .data
        .as_chunks::<24>()
        .0
        .iter()
        .map(|n| (u32::from_le_bytes([n[0], n[1], n[2], n[3]]), n[21]))
        .collect();
    assert_eq!(notes, [(0, 100), (123, 52), (183, 100)]);
}

#[test]
fn lists_fls_own_effects_hosted_in_the_old_wrapper_as_native() {
    let plugins = by_name(&read(&fl_studio_9_project(&[3; 300])));
    let compressor = &plugins["Fruity Compressor"];
    assert_eq!((compressor.format.as_str(), compressor.vendor.as_str()), ("Native", "Image-Line"));
    let cell = &plugins["LegacyCell"];
    assert_eq!((cell.format.as_str(), cell.kind), ("VST", PluginKind::Instrument));
}

#[test]
fn says_when_a_plugin_gave_fl_nothing_to_save() {
    let info = read(&fl_studio_25_nothing_saved());
    assert_eq!(info.warnings.len(), 1);
    assert!(info.warnings[0].starts_with("LegacyCell has no saved sound in this file"), "{}", info.warnings[0]);
}

#[test]
fn the_day_a_project_was_made_is_cut_to_the_millisecond_as_the_c_sharp_app_cuts_it() {
    // 1.2346 seconds after midnight: the C# app (.NET's AddDays) shows .234, not .235.
    let mut time = (45_000.0f64 + 1_234.6 / 86_400_000.0).to_le_bytes().to_vec();
    time.extend(0.1f64.to_le_bytes());
    let info = read(&Flp::new().ascii(199, "21.0.0").data(237, &time).build());
    let shown = info.created.map(|c| c.format("%H:%M:%S%.3f").to_string());
    assert_eq!(shown.as_deref(), Some("00:00:01.234"));
}

#[test]
fn reads_when_the_project_was_made_and_how_long_it_was_open() {
    // 45,000 days after 30 Dec 1899 is 15 Mar 2023; 0.1 of a day open is 2 h 24 min.
    let mut time = 45_000.5f64.to_le_bytes().to_vec();
    time.extend(0.1f64.to_le_bytes());
    let info = read(&Flp::new().ascii(199, "21.0.0").data(237, &time).build());
    assert_eq!(info.created.map(|c| c.to_string()).as_deref(), Some("2023-03-15 12:00:00"));
    assert_eq!(info.details["Time spent"], "2 h 24 min");
    // 13 minutes is 0.00902777... days, which times 1440 is 12.999999...
    assert_eq!(
        fl::project_time(&[45_000f64.to_le_bytes(), (13.0f64 / 1440.0).to_le_bytes()].concat()).1.as_deref(),
        Some("13 min")
    );
    assert_eq!(fl::project_time(&[1, 2, 3]), (None, None), "too short to hold the two dates");

    // Out of range: left out.
    let mut odd = 1.0f64.to_le_bytes().to_vec();
    odd.extend(f64::NAN.to_le_bytes());
    let info = read(&Flp::new().ascii(199, "21.0.0").data(237, &odd).build());
    assert_eq!((info.created, info.details.get("Time spent")), (None, None));
}

#[test]
fn text_before_fl_11_5_is_one_byte_a_letter() {
    let old = Flp::new().ascii(199, "11.0.0").data(194, &[b'C', 0xe9, 0]).build();
    assert_eq!(read(&old).title, "Cé");
    let new = Flp::new().ascii(199, "11.5.0").text(194, "Café").build();
    assert_eq!(read(&new).title, "Café");
    // A version with no minor part: FL 9 is one byte a letter all the same.
    let bare = Flp::new().ascii(199, "9").ascii(194, "Kick").build();
    assert_eq!(read(&bare).title, "Kick");
}
