//! Preset files and a song's sounds, on made-up files and songs (never Solomon's: rule 4).

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::io::Write;

use mo_formats::fl::song::{FlChannel, FlPlugin, FlSong, FlStretch};
use mo_formats::presets::{PresetBlob, fxp, vstpreset};
use mo_formats::{MidiClip, NoteAttribute, NoteEvent, PluginKind};
use mo_presets::files::{self, FileFormat};
use mo_presets::list::{self, SongPreset};
use mo_presets::role::Basis;
use mo_presets::{Role, Thresholds};

fn four_cc(s: &[u8; 4]) -> i32 {
    i32::from_be_bytes(*s)
}

#[test]
fn reads_a_vst2_program_and_bank() {
    let program = fxp::wrap_chunk(&[1, 2, 3], "Wide Saw", four_cc(b"XfsX"));
    let f = files::read_bytes(&program, "Serum - Wide Saw.fxp").unwrap();
    assert_eq!((f.format, f.name.as_str(), f.plugin_id.as_str()), (Some(FileFormat::Fxp), "Wide Saw", "XfsX"));
    assert_eq!((f.plugin.as_str(), f.programs), ("", 1));

    let params = fxp::wrap_params(&[0.5; 4], "", four_cc(b"Abcd"));
    assert_eq!(files::read_bytes(&params, "Init.fxp").unwrap().name, "Init", "no program name: the file's");

    // A bank of chunks: "FBCh", 3 programs.
    let mut bank = b"CcnK".to_vec();
    bank.extend(0i32.to_be_bytes());
    bank.extend(b"FBCh");
    bank.extend(1i32.to_be_bytes());
    bank.extend(four_cc(b"Syl1").to_be_bytes());
    bank.extend(1i32.to_be_bytes());
    bank.extend(3i32.to_be_bytes());
    bank.extend([0u8; 128]);
    let f = files::read_bytes(&bank, "My bank.FXB").unwrap();
    assert_eq!((f.format, f.programs, f.name.as_str()), (Some(FileFormat::Fxb), 3, "My bank"));

    assert!(files::read_bytes(b"RIFF....", "x.fxp").is_err());
    assert!(files::read_bytes(b"CcnK\0\0\0\0Oops", "x.fxp").is_err());
    assert!(files::read_bytes(b"CcnK", "x.fxp").is_err(), "cut short");
}

/// A .vstpreset with a component state and an "Info" chunk, laid out as the VST3 SDK writes it.
fn vstpreset_with_info(class_id: &str, info: &str) -> Vec<u8> {
    let component = [9u8; 10];
    let header = 48usize;
    let info_at = header + component.len();
    let list = info_at + info.len();
    let mut out = b"VST3".to_vec();
    out.extend(1i32.to_le_bytes());
    out.extend(class_id.as_bytes());
    out.extend((list as i64).to_le_bytes());
    out.extend(component);
    out.extend(info.as_bytes());
    out.extend(b"List");
    out.extend(2i32.to_le_bytes());
    for (id, at, size) in [(b"Comp", header, component.len()), (b"Info", info_at, info.len())] {
        out.extend(id);
        out.extend((at as i64).to_le_bytes());
        out.extend((size as i64).to_le_bytes());
    }
    out
}

const CLASS: &str = "ABCDEF0123456789ABCDEF0123456789";

#[test]
fn reads_a_vst3_preset_and_its_info() {
    let info = r#"<?xml version="1.0" encoding="utf-8"?>
<MetaInfo>
	<Attr id="MediaType" value="VstPreset" type="string" flags="writeProtected"/>
	<Attr id="PlugInName" value="Pigments" type="string" flags="writeProtected"/>
	<Attr id="PlugInCategory" value="Instrument|Synth" type="string" flags="writeProtected"/>
	<Attr id="MusicalCategory" value="Bass" type="string"/>
	<Attr id="MusicalCharacter" value="Dark|Wide" type="string"/>
	<Attr id="Name" value="Deep &amp; Dark" type="string"/>
</MetaInfo>"#;
    let raw = vstpreset_with_info(CLASS, info);
    let f = files::read_bytes(&raw, "deep.vstpreset").unwrap();
    assert_eq!(f.format, Some(FileFormat::VstPreset));
    assert_eq!((f.plugin.as_str(), f.name.as_str(), f.kind), ("Pigments", "Deep & Dark", PluginKind::Instrument));
    assert_eq!((f.category.as_str(), f.tags.clone()), ("Bass", vec!["Dark".to_owned(), "Wide".to_owned()]));
    assert_eq!(f.plugin_id, mo_formats::plugins::class_id(CLASS));

    // Bitwig's own .vstpreset (no Info chunk): just the plugin's id and the file's name.
    let plain = vstpreset::build(CLASS, &[1, 2], &[]);
    let f = files::read_bytes(&plain, "Track - Pigments.vstpreset").unwrap();
    assert_eq!((f.name.as_str(), f.plugin.as_str(), f.kind), ("Track - Pigments", "", PluginKind::Unknown));
    assert!(files::read_bytes(b"VST3\x01\0\0\0not hex", "x.vstpreset").is_err());
}

#[test]
fn reads_a_vst3_preset_from_disk_when_its_list_is_past_the_first_part() {
    // A sampler's preset: megabytes of state before the Info chunk and the list.
    let info = r#"<MetaInfo><Attr id="PlugInName" value="Kontakt" type="string"/></MetaInfo>"#;
    let component = vec![7u8; 200 * 1024];
    let header = 48usize;
    let info_at = header + component.len();
    let list = info_at + info.len();
    let mut raw = b"VST3".to_vec();
    raw.extend(1i32.to_le_bytes());
    raw.extend(CLASS.as_bytes());
    raw.extend((list as i64).to_le_bytes());
    raw.extend(&component);
    raw.extend(info.as_bytes());
    raw.extend(b"List");
    raw.extend(1i32.to_le_bytes());
    raw.extend(b"Info");
    raw.extend((info_at as i64).to_le_bytes());
    raw.extend((info.len() as i64).to_le_bytes());
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("big sampler.vstpreset");
    std::fs::write(&path, &raw).unwrap();
    let f = files::read(&path).unwrap().unwrap();
    assert_eq!((f.plugin.as_str(), f.name.as_str()), ("Kontakt", "big sampler"));
}

/// Bitwig's object stream, enough for a preset: big-endian, typed fields (as mo-formats' test writer).
struct Bw(Vec<u8>);

impl Bw {
    fn u32(&mut self, v: u32) {
        self.0.extend(v.to_be_bytes());
    }
    fn text(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend(s.as_bytes());
    }
    fn str_field(&mut self, field: u32, s: &str) {
        self.u32(field);
        self.0.push(0x08);
        self.text(s);
    }
}

/// A .bwpreset: metadata, one device object, and a zip holding `state` as `plugin-states/<entry>`.
fn bwpreset(meta: &[(&str, &str)], tags: &[&str], device: &str, preset: &str, entry: &str, state: &[u8]) -> Vec<u8> {
    bwpreset_field(meta, tags, device, preset, entry, entry, state)
}

/// [`bwpreset`], with the device's state field written as `field` rather than the zip file's name.
fn bwpreset_field(
    meta: &[(&str, &str)],
    tags: &[&str],
    device: &str,
    preset: &str,
    field: &str,
    entry: &str,
    state: &[u8],
) -> Vec<u8> {
    let mut m = Bw(Vec::new());
    m.u32(4);
    m.text("meta");
    for (k, v) in meta {
        m.u32(1);
        m.text(k);
        m.0.push(0x08);
        m.text(v);
    }
    if !tags.is_empty() {
        m.u32(1);
        m.text("tags");
        m.0.push(0x19);
        m.u32(tags.len() as u32);
        for t in tags {
            m.text(t);
        }
    }
    m.u32(0);
    let mut b = Bw(Vec::new());
    b.u32(0x728); // a VST3 plugin device
    b.str_field(0x9a, device);
    b.str_field(0x12de, preset);
    if !field.is_empty() {
        b.str_field(0xbe5, field);
    }
    b.u32(0);

    let mut zip_bytes = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_bytes));
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file(format!("plugin-states/{entry}"), options).unwrap();
        zip.write_all(state).unwrap();
        zip.finish().unwrap();
    }
    let body_offset = 42 + m.0.len() + 8;
    let zip_offset = body_offset + b.0.len();
    let mut out = format!("BtWg000300020000{body_offset:08x}{zip_offset:016x}00").into_bytes();
    out.extend(&m.0);
    out.extend(b"       \n");
    out.extend(&b.0);
    out.extend(&zip_bytes);
    out
}

#[test]
fn reads_a_bitwig_preset_of_a_plugin() {
    let state = vstpreset::build(CLASS, &[1, 2, 3], &[]);
    let raw = bwpreset(
        &[
            ("device_name", "Pigments"),
            ("device_type", "instrument"),
            ("creator", "Solomon"),
            ("preset_category", "Bass"),
        ],
        &["dark", "mono"],
        "Pigments",
        "Night Bass",
        "a1b2.vstpreset",
        &state,
    );
    let f = files::read_bytes(&raw, "Night Bass.bwpreset").unwrap();
    assert_eq!(f.format, Some(FileFormat::BwPreset));
    assert_eq!((f.plugin.as_str(), f.name.as_str(), f.kind), ("Pigments", "Night Bass", PluginKind::Instrument));
    assert_eq!((f.creator.as_str(), f.category.as_str()), ("Solomon", "Bass"));
    assert_eq!(f.tags, ["dark", "mono"]);
    assert!(f.wraps_plugin);
    assert_eq!(f.plugin_id, mo_formats::plugins::class_id(CLASS), "the id from the state in its zip");

    // No metadata: the device object still names the plugin; the preset's name is the device's.
    let raw = bwpreset(&[], &[], "Serum", "Serum", "c3.fxb", &fxp::wrap_chunk(&[0], "x", four_cc(b"XfsX")));
    let f = files::read_bytes(&raw, "Lost.bwpreset").unwrap();
    assert_eq!((f.plugin.as_str(), f.name.as_str(), f.plugin_id.as_str()), ("Serum", "Lost", "XfsX"));
    assert!(files::read_bytes(b"not bitwig at all, not at all, not at all..", "x.bwpreset").is_err());

    // A state field without the file's extension still finds the file, as the Bitwig reader finds it.
    let state = fxp::wrap_chunk(&[0], "x", four_cc(b"XfsX"));
    let raw = bwpreset_field(&[], &[], "Serum", "Serum", "C3", "c3.fxp", &state);
    assert_eq!(files::read_bytes(&raw, "Stem.bwpreset").unwrap().plugin_id, "XfsX");
}

fn note(start: i64, length: i64, key: u8) -> NoteEvent {
    NoteEvent { start, length, key, velocity: 40000, muted: false, step: false, attribute: NoteAttribute::NONE }
}

fn channel(index: u32, name: &str, plugin: &str, insert: u32, root_key: u8, notes: Vec<NoteEvent>) -> FlChannel {
    FlChannel {
        index,
        name: name.into(),
        instrument: Some(FlPlugin {
            name: plugin.into(),
            vendor: String::new(),
            format: "VST2",
            device_id: String::new(),
            path: String::new(),
            nothing_saved: false,
            state: None,
            parameter_ids: None,
            slot: None,
            enabled: true,
        }),
        sample: String::new(),
        insert,
        notes,
        root_key,
        pitch_cents: 0,
        volume: 0.78,
        pan: 0.5,
        clips: Vec::new(),
        volume_envelope: None,
        audio_clips: Vec::new(),
        stretch: FlStretch::default(),
        enabled: true,
        switched_by: Vec::new(),
    }
}

fn blob(name: &str, plugin: &str, kind: PluginKind, channel: Option<u32>, place: &str, native: bool) -> PresetBlob {
    PresetBlob { kind, channel, place: place.into(), native, ..PresetBlob::new(name, plugin, ".fxp", vec![1, 2, 3]) }
}

/// Eighth notes cycling through `keys` for two bars, at 96 ticks a beat.
fn line(keys: &[u8]) -> Vec<NoteEvent> {
    (0..16).map(|i| note(i * 48, 40, keys[i as usize % keys.len()])).collect()
}

#[test]
fn an_fl_songs_sounds_get_their_channels_parts_and_effects_their_inserts() {
    let song = FlSong {
        title: "Test".into(),
        ppq: 96,
        tempo: 140.0,
        channels: vec![
            // Written high, but the root key two octaves up makes it a bass.
            channel(0, "Wobble", "Serum", 2, 84, line(&[60, 62, 67])),
            channel(1, "Melody", "Sylenth1", 2, 60, line(&[72, 74, 76, 79])),
            channel(2, "Spare", "3x Osc", 3, 60, Vec::new()),
            channel(3, "Hook", "Spire", 2, 60, line(&[72, 74, 76, 79]).into_iter().take(10).collect()),
        ],
        inserts: Vec::new(),
        automation: Vec::new(),
        muted_tracks: Vec::new(),
        curves: BTreeMap::new(),
        warning: None,
    };
    let presets = vec![
        blob("Wobble", "Serum", PluginKind::Instrument, Some(0), "", false),
        blob("Melody", "Sylenth1", PluginKind::Instrument, Some(1), "", false),
        blob("Spare", "3x Osc", PluginKind::Instrument, Some(2), "", true),
        blob("Hook", "Spire", PluginKind::Instrument, Some(3), "", false),
        blob("Pro-Q 3", "Pro-Q 3", PluginKind::Effect, None, "Insert 2 · Synths", false),
        blob("Fruity Limiter", "Fruity Limiter", PluginKind::Effect, None, "Master", true),
        blob("Fruity Delay", "Fruity Delay", PluginKind::Effect, None, "Insert 3", true),
    ];
    let mut song = song;
    // Switched off in the Channel Rack: it plays nothing, though its clips hold notes.
    song.channels.push(FlChannel { enabled: false, ..channel(4, "Off", "Serum", 4, 60, line(&[72, 74, 76])) });
    let mut presets = presets;
    presets.push(blob("Serum", "Serum", PluginKind::Instrument, Some(4), "", false));
    let mut out = list::from_fl(&song, presets, &Thresholds::default());
    let role = |p: &SongPreset| p.guess.role;
    assert_eq!(role(&out[0]), Some(Role::Bass), "{}", out[0].guess.reason);
    assert!(out[0].guess.reason.contains("octave wasn't checked"));
    assert_eq!(role(&out[1]), Some(Role::Lead));
    assert!(out[2].plays_nothing && role(&out[2]).is_none(), "an unused synth with no clue in its name");
    assert!(!out[1].plays_nothing);

    // Insert 2 hears Wobble (16 bass notes), Melody and Hook (26 lead notes): lead wins.
    let eq = &out[4];
    assert_eq!((role(eq), eq.guess.basis), (Some(Role::Lead), Basis::Track));
    assert!(eq.guess.reason.contains("Insert 2") && eq.guess.reason.contains("Wobble"), "{}", eq.guess.reason);
    assert!(eq.guess.weak, "more than one part feeds it");
    assert_eq!(eq.track, "Insert 2 · Synths");
    assert_eq!(role(&out[5]), None);
    assert!(out[5].guess.reason.contains("master"));
    assert_eq!(role(&out[6]), None, "only a channel with no notes feeds insert 3");

    assert!(out[7].plays_nothing && out[7].guess.role.is_none(), "{}", out[7].guess.reason);
    assert_ne!(out[0].place_key(), out[7].place_key(), "two Serum channels are told apart in their save");

    // A fix wins, and is kept by the plugin, its settings and the notes it plays. These made-up presets all have
    // the same settings, so only the plugin and the notes tell them apart.
    assert_ne!(out[0].hash, out[2].hash, "another plugin");
    assert_ne!(out[1].hash, out[3].hash, "other notes");
    assert_ne!(out[4].hash, out[6].hash, "the same effect on inserts fed by other notes");
    let fixes = std::collections::HashMap::from([(out[2].hash.clone(), Role::Pad)]);
    list::apply_fixes(&mut out, &fixes);
    assert_eq!(
        (out[2].role(), out[2].fixed, role(&out[2])),
        (Some(Role::Pad), Some(Role::Pad), None),
        "the guess stays"
    );
    assert_eq!(out[0].role(), Some(Role::Bass));
    // The same sound playing the same notes in another copy of the save takes the fix.
    let again = list::from_fl(
        &song,
        vec![blob("Spare", "3x Osc", PluginKind::Instrument, Some(2), "", true)],
        &Thresholds::default(),
    );
    assert_eq!(again[0].hash, out[2].hash);
    // Taken away: the guess shows again.
    list::apply_fixes(&mut out, &std::collections::HashMap::new());
    assert_eq!(out[2].role(), None);

    out[0].forget_data();
    assert_eq!((out[0].blob.data.len(), out[0].size), (0, 3));
}

#[test]
fn a_bitwig_tracks_presets_share_its_part() {
    let mut first = MidiClip { name: "Bass 2 1".into(), ppq: 960, ..MidiClip::default() };
    first.track("Bass 2").extend((0..16).map(|i| note(i * 480, 400, [33, 36, 40][i as usize % 3])));
    let mut second = MidiClip { name: "Bass 1".into(), ppq: 960, ..MidiClip::default() };
    second.track("Bass").extend((0..16).map(|i| note(i * 480, 400, [84, 86][i as usize % 2])));
    let presets = vec![
        PresetBlob::new("Bass 2 - Deep", "Pigments", ".vstpreset", vec![1]),
        PresetBlob::new("Bass 2 - Pro-Q 3", "Pro-Q 3", ".vstpreset", vec![1]),
        PresetBlob::new("Bass - Squeal", "Serum", ".fxb", vec![1]),
        PresetBlob::new("Pads - Omnisphere", "Omnisphere", ".vstpreset", vec![1]),
    ];
    let out = list::from_bitwig_parts(presets, &[first, second], &Thresholds::default());
    assert_eq!(out.iter().map(|p| p.track.as_str()).collect::<Vec<_>>(), ["Bass 2", "Bass 2", "Bass", "Pads"]);
    assert_eq!((out[0].guess.role, out[1].guess.role), (Some(Role::Bass), Some(Role::Bass)));
    assert!(out[0].guess.reason.contains("transpose"), "{}", out[0].guess.reason);
    // "Bass" plays high notes: the notes win over the name only when they're sure, and they are.
    assert_eq!(out[2].guess.role, Some(Role::Lead));
    // No clips: from its track's name.
    assert_eq!((out[3].guess.role, out[3].guess.basis, out[3].plays_nothing), (Some(Role::Pad), Basis::Name, false));
}

#[test]
fn reads_a_vst3_preset_from_disk_when_its_list_runs_past_the_first_part() {
    // The list starts just inside the first 64 KiB and its entries run past it.
    let info = r#"<MetaInfo><Attr id="PlugInName" value="Pigments" type="string"/></MetaInfo>"#;
    let header = 48usize;
    let list = 64 * 1024 - 20;
    let component = vec![7u8; list - header - info.len()];
    let info_at = header + component.len();
    let mut raw = b"VST3".to_vec();
    raw.extend(1i32.to_le_bytes());
    raw.extend(CLASS.as_bytes());
    raw.extend((list as i64).to_le_bytes());
    raw.extend(&component);
    raw.extend(info.as_bytes());
    raw.extend(b"List");
    raw.extend(1i32.to_le_bytes());
    raw.extend(b"Info");
    raw.extend((info_at as i64).to_le_bytes());
    raw.extend((info.len() as i64).to_le_bytes());
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("edge.vstpreset");
    std::fs::write(&path, &raw).unwrap();
    assert_eq!(files::read(&path).unwrap().unwrap().plugin, "Pigments");
}
