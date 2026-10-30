//! Plugin states taken out of FL projects as preset files: the C# `ExtractPresets` tests and the FL 9 and FL 25 ones
//! on plugin states, on the same made-up files.

mod common;

use common::*;
use mo_formats::PluginKind;
use mo_formats::fl::{events, presets, song};
use mo_formats::presets::PresetBlob;

fn presets_of(bytes: &[u8]) -> Vec<PresetBlob> {
    presets::extract_presets(&events::load(bytes).expect("an FL project"))
}

fn be_i32(d: &[u8], at: usize) -> i32 {
    i32::from_be_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

fn be_floats(d: &[u8], at: usize, n: usize) -> Vec<f32> {
    (0..n).map(|i| f32::from_be_bytes(d[at + 4 * i..at + 4 * i + 4].try_into().expect("4 bytes"))).collect()
}

fn text(d: &[u8], at: usize, len: usize) -> String {
    String::from_utf8_lossy(&d[at..at + len]).trim_end_matches('\0').to_owned()
}

#[test]
fn extracts_plugin_states() {
    let presets = presets_of(&fl_studio_project());
    assert_eq!(
        presets.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["Lead", "Bass", "Fruity Limiter", "Pro-Q 3"]
    );
    let serum = &presets[0];
    assert_eq!(serum.extension, ".fxp");
    assert_eq!((text(&serum.data, 0, 4), text(&serum.data, 8, 4)), ("CcnK".into(), "FPCh".into()));
    assert_eq!(serum.data[serum.data.len() - 4..], [1, 2, 3, 4]);
    assert_eq!(presets[1].extension, ".flstate");
    assert_eq!(
        presets.iter().map(|p| (p.kind, p.native)).collect::<Vec<_>>(),
        [
            (PluginKind::Instrument, false),
            (PluginKind::Instrument, true),
            (PluginKind::Effect, true),
            (PluginKind::Effect, false)
        ]
    );
    assert_eq!(presets.iter().map(|p| p.place.as_str()).collect::<Vec<_>>(), ["", "", "Master", "Master"]);
    assert_eq!(presets.iter().map(|p| p.channel).collect::<Vec<_>>(), [Some(0), Some(2), None, None]);
}

#[test]
fn exports_vst2_as_fxp_with_the_plugin_id() {
    let presets = presets_of(&fl_studio_25_project());
    let serum = presets.iter().find(|p| p.plugin == "Serum").expect("Serum's state");
    assert_eq!(serum.extension, ".fxp");
    assert_eq!(text(&serum.data, 16, 4), "XfsX", "fxID, big-endian");
    assert_eq!(serum.data[serum.data.len() - 3..], [1, 2, 3], "FL's header taken off");
    assert_eq!(be_i32(&serum.data, 56), 3);
}

#[test]
fn exports_parameter_plugins_as_parameter_presets() {
    let presets = presets_of(&fl_studio_25_project());
    let d = &presets.iter().find(|p| p.plugin == "Predator 2").expect("Predator's state").data;
    assert_eq!(text(d, 8, 4), "FxCk");
    assert_eq!(text(d, 16, 4), "Prd2");
    assert_eq!(be_i32(d, 24), 3);
    assert_eq!(text(d, 28, 28), "arp- sine beat");
    assert_eq!(be_i32(d, 4), d.len() as i32 - 8);
    assert_eq!(be_floats(d, 56, 3), [0.25, 1.0, 0.5]);
}

#[test]
fn a_parameter_plugin_that_saved_every_program_gives_the_one_its_channel_had_picked() {
    // Solomon's "broken homes_29" (2026-10-26): FL saved 32-bit FireBird's 128 programs of 50 values, 6,400 in all,
    // and the whole lot went to Bitwig as one program, so FireBird came up at its default sound there.
    let firebird = Wrapper {
        version: 12,
        kind: Some(0),
        four_cc: Some("BF2T"),
        ..Wrapper::new(
            "Tone2FireBird",
            r"C:\Program Files (x86)\VSTPlugIns\FireBird.dll",
            "Tone2",
            &fl_vst2_param_bank(
                &[&[0.1, 0.2, 0.3], &[0.4, 0.5, 0.6], &[0.7, 0.8, 0.9]],
                &["SYN Funky", "SYN live wire", "SYN FM Agro"],
                2,
            ),
        )
    };
    let bytes = Flp::new()
        .ascii(199, "21.2.2.3914")
        .dword(156, 140_000)
        .word(64, 0)
        .byte(21, 2)
        .text(201, "Fruity Wrapper")
        .text(203, "SYN FM Agro")
        .data(213, &firebird.build())
        .build();
    let song = song::read_song(&events::load(&bytes).expect("an FL project"));
    let state = song.channels[0].instrument.as_ref().and_then(|i| i.state.as_ref()).expect("its settings");
    let d = &state.data;
    assert_eq!(text(d, 8, 4), "FxCk");
    assert_eq!(be_i32(d, 24), 3);
    assert_eq!(text(d, 28, 28), "SYN FM Agro");
    assert_eq!(be_floats(d, 56, 3), [0.7, 0.8, 0.9]);
    assert_eq!(be_i32(d, 4), d.len() as i32 - 8);
    assert_eq!(state.program, 2);
}

#[test]
fn exports_vst3_as_vstpreset() {
    let presets = presets_of(&fl_studio_25_project());
    let d = &presets.iter().find(|p| p.plugin == "Arcade").expect("Arcade's state").data;
    assert_eq!(text(d, 0, 4), "VST3");
    // The same class id Bitwig writes for Arcade.
    assert_eq!(text(d, 8, 32), "56535441524344617263616465000000");
    assert_eq!(d[48..51], [7, 7, 7]);
    assert_eq!(d[51..52], [8]);
    let list = i64::from_le_bytes(d[40..48].try_into().expect("8 bytes")) as usize;
    assert_eq!(text(d, list, 4), "List");
    assert_eq!(i32::from_le_bytes(d[list + 4..list + 8].try_into().expect("4 bytes")), 2);
    assert_eq!(text(d, list + 8, 4), "Comp");

    // The song keeps the VST3's parameter ids, for automation.
    let song = song::read_song(&events::load(&fl_studio_25_project()).expect("an FL project"));
    let arcade = song.channels.iter().find(|c| c.name == "Keys").and_then(|c| c.instrument.clone()).expect("Arcade");
    assert_eq!(arcade.parameter_ids, Some(vec![10, 20, 2000003]));
    assert_eq!(arcade.state.as_ref().map(|s| &*s.extension), Some(".vstpreset"));
}

#[test]
fn keeps_an_fl_9_plugins_whole_saved_sound() {
    let chunk: Vec<u8> = (0..300).map(|i| (i * 7) as u8).collect();
    let bytes = fl_studio_9_project(&chunk);
    let presets = presets_of(&bytes);
    let cell = presets.iter().find(|p| p.plugin == "LegacyCell").expect("LegacyCell's state");
    assert_eq!(text(&cell.data, 8, 4), "FPCh");
    assert_eq!(text(&cell.data, 16, 4), "KLCV");
    assert_eq!(be_i32(&cell.data, 56), chunk.len() as i32);
    assert_eq!(cell.data[60..], chunk);
    // The program it had selected, which FL selects again after loading the chunk.
    assert_eq!(cell.program, 42);

    let song = song::read_song(&events::load(&bytes).expect("an FL project"));
    let state = song.channels[0].instrument.as_ref().and_then(|i| i.state.as_ref()).expect("its settings");
    assert_eq!(state.data[60..], chunk);
}

#[test]
fn reads_an_fl_9_plugins_settings_after_the_shorter_header() {
    let presets = presets_of(&fl_studio_9_project(&[1; 10]));
    let d = &presets.iter().find(|p| p.plugin == "Fruity Compressor").expect("the compressor").data;
    assert_eq!(text(d, 8, 4), "FxCk");
    assert_eq!(text(d, 28, 28), "Full Reset");
    assert_eq!(be_floats(d, 56, 3), [0.5, 0.02, 0.5]);
}

#[test]
fn a_plugin_that_gave_fl_nothing_carries_no_empty_sound() {
    let bytes = fl_studio_25_nothing_saved();
    assert!(presets_of(&bytes).is_empty());
    let song = song::read_song(&events::load(&bytes).expect("an FL project"));
    let cell = song.channels[0].instrument.clone().expect("LegacyCell");
    assert_eq!((cell.state, cell.nothing_saved), (None, true));
}

#[test]
fn a_quick_look_leaves_the_sounds_out() {
    let bytes = fl_studio_25_project();
    let song = song::read_song_with(&events::load(&bytes).expect("an FL project"), false);
    assert!(song.channels.iter().filter_map(|c| c.instrument.as_ref()).all(|i| i.state.is_none()));
}
