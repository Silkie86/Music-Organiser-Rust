//! Reading Studio One songs: the C# app's `StudioOneTests`, on the same made-up files.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::HashMap;

use common::studio_one::*;
use common::test_dir;
use mo_formats::studio_one::{self, Song};
use mo_formats::{NoteEvent, PluginKind, velocity};

/// A note as Studio One writes it, its velocity 0 to 1.
fn note(start: i64, length: i64, key: u8, studio_one_velocity: f64) -> NoteEvent {
    NoteEvent {
        start,
        length,
        key,
        velocity: velocity::from_unit(studio_one_velocity),
        muted: false,
        step: false,
        attribute: None,
    }
}

#[test]
fn reads_metadata_plugins_and_samples() {
    let info = studio_one::read(&studio_one_song(&test_dir("s1-read"), "Sunday Song")).expect("read");
    assert_eq!(info.tempo, Some(92.0));
    assert_eq!(info.version, "Studio One/6.5.2");
    assert_eq!(info.title, "Sunday Song");
    assert_eq!(info.samples, [r"C:\Users\Sol\Samples\Vox Chop.wav", r"C:\Users\Sol\Samples\Rim.aif"]);
    let plugins: HashMap<&str, _> = info.plugins.iter().map(|p| (p.name.as_str(), p)).collect();
    assert_eq!((plugins["Diva"].kind, plugins["Diva"].format.as_str()), (PluginKind::Instrument, "VST3"));
    assert_eq!(plugins["Mai Tai"].format, "Native");
    assert_eq!(plugins["Pro-C 2"].kind, PluginKind::Effect);
    assert_eq!(info.tracks, ["Keys", "Vocals"]);
    assert_eq!((info.details["Notes"].as_str(), info.details["Stored plugin states"].as_str()), ("3", "1"));
}

#[test]
fn extracts_notes_and_presets() {
    let raw = studio_one_song_bytes();
    let mut song = Song::open(&raw).expect("a zip");
    let clips = song.midi();
    assert_eq!(clips.len(), 1);
    let mut clip = clips[0].clone();
    assert_eq!((clip.name.as_str(), clip.ppq), ("Verse Chords", 960));
    let notes = clip.track("Keys").clone();
    // The last as an older song writes it, 0 to 127: MIDI 2.0's own scaling keeps it 100 in MIDI 1.0.
    let older = NoteEvent { velocity: velocity::from_7(100), ..note(1440, 480, 67, 0.0) };
    assert_eq!(notes, [note(0, 960, 60, 0.8), note(960, 480, 64, 1.0), older]);
    // As MIDI 1.0 velocities, what the C# app gives.
    assert_eq!(notes.iter().map(NoteEvent::velocity7).collect::<Vec<_>>(), [102, 127, 100]);
    let presets = song.presets();
    assert_eq!(presets.len(), 1);
    let p = &presets[0];
    assert_eq!((p.name.as_str(), p.plugin.as_str(), &*p.extension), ("Inst 1", "Diva", ".vstpreset"));
    assert_eq!(p.data, b"VST3 fake preset");
}

#[test]
fn reads_each_insert_as_the_plugin_it_holds_with_its_id_rather_than_the_name_it_shows() {
    // Solomon's "Click Clack" (27 Oct 2026): Studio One numbers a plugin's second insert ("FabFilter Pro-Q 2 2") and
    // keeps the name an insert first had after Waves renamed the plugin ("TG12345 Stereo"). Read by those, and with
    // Studio One's own effects not known as its own, 12 of its 27 plugins showed as missing.
    let info = studio_one::read(&studio_one_inserts(&test_dir("s1-inserts"), "Click Clack")).expect("read");
    let mut found: Vec<_> =
        info.plugins.iter().map(|p| (p.name.as_str(), p.kind, p.format.as_str(), p.id.as_str())).collect();
    found.sort_by(|a, b| a.0.cmp(b.0));
    assert_eq!(
        found,
        [
            ("Abbey Road EMI TG12345 Ch Stereo", PluginKind::Effect, "VST3", "56535454473553746731323334352073"),
            ("Arpeggiator", PluginKind::NoteEffect, "Native", "BDD2D6E5381941B4BF917B832DAAFA0B"),
            // A VST2's four letters, as the plugin scanner gives them.
            ("FabFilter Pro-Q 2", PluginKind::Effect, "VST2", "FQ2p"),
            ("Gate", PluginKind::Effect, "Native", "132C0E1F06FD4C96BD65678C04FE1C92"),
            ("Impact", PluginKind::Instrument, "Native", "3713E26C2FCA40249F2517E9D2BE2B9B"),
            ("Kontakt 5 16out", PluginKind::Instrument, "VST2", "NiO7"),
            ("Kramer Tape Stereo", PluginKind::Effect, "VST3", "565354544150536B72616D6572207461"),
            ("Pro EQ", PluginKind::Effect, "Native", "D9AE9ACD69B44B43B8D5983E39C559A5"),
        ]
    );
    assert!(info.plugins.iter().all(|p| p.path.is_empty()));
}

#[test]
fn a_file_that_isnt_a_zip_says_it_cant_be_read() {
    let path = test_dir("s1-not").join("not.song");
    std::fs::write(&path, b"not a zip at all").expect("write");
    assert!(studio_one::read(&path).is_err());
}

#[test]
fn a_song_without_its_song_file_says_so() {
    let raw = common::bitwig::stored_zip(&[("metainfo.xml", br#"<M><Attribute id="Media:Tempo" value="120"/></M>"#)]);
    let info = Song::open(&raw).expect("a zip").info();
    assert_eq!(info.tempo, Some(120.0));
    assert_eq!(info.warnings, ["Song/song.xml is missing or unreadable."]);
    assert!(!info.details.contains_key("Notes"));
}
