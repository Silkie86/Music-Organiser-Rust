//! Send to Bitwig: Bitwig presets made from a template, in folders of the test's own (never Solomon's Bitwig folder).

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use mo_formats::bitwig;
use mo_formats::presets::{PresetBlob, fxp};
use mo_presets::bitwig::{self as send, Templates};
use mo_presets::{export, library};

mod common;
use common::{bwpreset_field, bwpreset_full};

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-presets-bitwig").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A preset Bitwig saved for one plugin, its settings in the zip as `entry`, and a preview picture after.
fn saved_in_bitwig(dir: &Path, file: &str, plugin: &str, preset: &str, entry: &str) -> PathBuf {
    let raw = bwpreset_full(
        &[("preset_name", file), ("creator", "silkie86")],
        &[],
        plugin,
        preset,
        entry,
        entry,
        &send::to_bank(&fxp::wrap_chunk(&[9], "Init", i32::from_be_bytes(*b"XfsX")), 0).unwrap(),
        &[("preview.png", b"png")],
    );
    let path = dir.join(format!("{file}.bwpreset"));
    std::fs::write(&path, raw).unwrap();
    path
}

fn serum_sound(name: &str, plugin: &str) -> PresetBlob {
    PresetBlob::new(name, plugin, ".fxp", fxp::wrap_chunk(&[1, 2, 3], name, i32::from_be_bytes(*b"XfsX")))
}

fn zip_file(doc: &bitwig::Document, name: &str) -> Option<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(doc.zip()?)).ok()?;
    let mut f = zip.by_name(name).ok()?;
    let mut out = Vec::new();
    f.read_to_end(&mut out).unwrap();
    Some(out)
}

fn one(t: send::Template) -> Templates {
    Templates::from([(send::key(&t.plugin, t.format()).unwrap(), t)])
}

/// A Bitwig preset of `sound` from the template, read from its file.
fn build(t: &send::Template, sound: &PresetBlob, name: &str) -> Result<Vec<u8>, String> {
    send::build(&std::fs::read(&t.path).unwrap(), &t.path, sound, name)
}

#[test]
fn builds_a_bitwig_preset_from_one_saved_for_the_same_plugin() {
    let dir = folder("build");
    let t = send::inspect(&saved_in_bitwig(&dir, "Init Lead", "Serum", "Init Lead", "0f1e2d.fxb")).unwrap();
    assert_eq!((t.plugin.as_str(), t.format(), t.preset_name.as_str()), ("Serum", "VST2", "Init Lead"));

    // FL calls it "Serum_x64"; the template was saved for "Serum".
    let mut sound = serum_sound("Big Bass", "Serum_x64");
    sound.program = 2;
    let templates = one(t.clone());
    assert_eq!(send::template_for(&sound, &templates), Some(&t));

    let doc = bitwig::decode(build(&t, &sound, "Big Bass").unwrap()).unwrap();
    assert_eq!(doc.body_error, None);
    assert_eq!(doc.meta_str("preset_name"), "Big Bass");
    assert_eq!(doc.meta_str("creator"), "silkie86");
    let device = doc.objects.iter().find(|o| !o.str(0xbe5).is_empty()).unwrap();
    // FL's program goes in as a one-program bank, as Bitwig keeps VST2 settings.
    assert_eq!((device.str(0x9a), device.str(0x12de), device.str(0xbe5)), ("Serum", "Big Bass", "0f1e2d.fxb"));
    assert_eq!(zip_file(&doc, "plugin-states/0f1e2d.fxb"), send::to_bank(&sound.data, 2));
    assert_eq!(zip_file(&doc, "preview.png").as_deref(), Some(&b"png"[..]));
    // The template itself is only read.
    assert_eq!(send::inspect(&t.path).unwrap().preset_name, "Init Lead");
}

#[test]
fn the_settings_file_is_renamed_when_its_type_changes() {
    let dir = folder("rename");
    // A template that keeps its plugin's settings as a program; the new sound goes in as a bank.
    let t = send::inspect(&saved_in_bitwig(&dir, "Init", "Serum", "Init", "c3.fxp")).unwrap();
    let doc = bitwig::decode(build(&t, &serum_sound("Pad", "Serum"), "Pad").unwrap()).unwrap();
    let device = doc.objects.iter().find(|o| !o.str(0xbe5).is_empty()).unwrap();
    assert_eq!(device.str(0xbe5), "c3.fxb");
    assert!(zip_file(&doc, "plugin-states/c3.fxp").is_none());
    assert!(zip_file(&doc, "plugin-states/c3.fxb").is_some());
}

#[test]
fn never_renames_the_plugin_itself() {
    // A preset simply called "Serum" must not turn the device into a different plugin.
    let dir = folder("plugin-name");
    let t = send::inspect(&saved_in_bitwig(&dir, "Serum", "Serum", "Serum", "a.fxb")).unwrap();
    let doc = bitwig::decode(build(&t, &serum_sound("Pad", "Serum"), "Pad").unwrap()).unwrap();
    let device = doc.objects.iter().find(|o| !o.str(0xbe5).is_empty()).unwrap();
    assert_eq!(device.str(0x9a), "Serum");
}

#[test]
fn only_a_preset_with_its_plugins_settings_is_a_template() {
    let state = b"CcnK";
    // Its settings file isn't in the zip.
    assert!(send::inspect_bytes(&bwpreset_field(&[], &[], "Serum", "x", "a.fxb", "b.fxb", state)).is_none());
    // Settings of a kind Bitwig doesn't load as VST2 or VST3.
    assert!(send::inspect_bytes(&bwpreset_field(&[], &[], "Serum", "x", "a.bin", "a.bin", state)).is_none());
    // One of Bitwig's own devices, with no plugin in it.
    assert!(send::inspect_bytes(&bwpreset_field(&[], &[], "Polymer", "x", "", "a.fxb", state)).is_none());
    assert!(send::inspect_bytes(b"not a Bitwig file, not at all, not at all, not at all").is_none());
    assert!(send::inspect_bytes(&bwpreset_field(&[], &[], "Serum", "x", "a.fxb", "a.fxb", state)).is_some());
}

#[test]
fn finds_templates_among_found_presets_and_matches_by_format() {
    let dir = folder("find");
    std::fs::create_dir_all(dir.join("Serum")).unwrap();
    let old = saved_in_bitwig(&dir.join("Serum"), "Old", "Serum", "Old", "a.fxb");
    let new = saved_in_bitwig(&dir.join("Serum"), "New", "Serum", "New", "b.fxb");
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    std::fs::write(dir.join("broken.bwpreset"), "not a preset").unwrap();

    let templates = send::templates(&library::find_preset_files(std::slice::from_ref(&dir)));
    assert_eq!(templates.len(), 1);
    // The newest one saved for the plugin wins.
    let serum = send::template_for(&serum_sound("A", "Serum"), &templates).unwrap();
    assert_eq!(serum.path, new);
    // A VST3 state needs a VST3 template.
    let vst3 = PresetBlob::new("A", "Serum", ".vstpreset", vec![1]);
    assert!(send::template_for(&vst3, &templates).is_none());
    assert!(send::template_for(&serum_sound("A", "Diva"), &templates).is_none());

    // The list kept between runs gives the same files and templates, read from their files and then kept.
    let store = dir.join("data").join(mo_presets::file_store::FILE_NAME);
    let roots = std::slice::from_ref(&dir);
    for run in 0..2 {
        let (found, why) = mo_presets::file_store::find(roots, &store);
        assert_eq!(why, None);
        assert_eq!(found.read, if run == 0 { 3 } else { 0 });
        assert_eq!(found.files, library::find_preset_files(roots));
        assert_eq!(found.templates, templates);
    }
}

#[test]
fn export_writes_a_bitwig_preset_beside_the_plugin_preset() {
    let dir = folder("export");
    let t = send::inspect(&saved_in_bitwig(&dir, "Init", "Serum", "Init", "a.fxb")).unwrap();
    let presets = [serum_sound("Big Bass", "Serum"), serum_sound("Wide", "Diva")];
    let out = export::export(&presets, "Song.flp", &dir.join("out"), Some(&one(t))).unwrap();
    // Diva has no template of its own: Serum's stands in for it (a VST2 too).
    assert_eq!(
        out.files,
        ["Serum - Big Bass.fxp", "Serum - Big Bass.bwpreset", "Diva - Wide.fxp", "Diva - Wide.bwpreset"]
    );
    assert!(out.failed.is_empty());
    let made = std::fs::read(out.folder.join("Serum - Big Bass.bwpreset")).unwrap();
    assert_eq!(bitwig::decode(made).unwrap().meta_str("preset_name"), "Big Bass");
}

#[test]
fn a_template_that_cant_be_used_still_leaves_the_plugin_preset() {
    let dir = folder("export-broken");
    let path = saved_in_bitwig(&dir, "Init", "Serum", "Init", "a.fxb");
    let t = send::inspect(&path).unwrap();
    // It was damaged after the page found it.
    std::fs::write(&path, b"damaged").unwrap();
    let out = export::export(&[serum_sound("Lead", "Serum")], "Song.flp", &dir.join("out"), Some(&one(t))).unwrap();
    assert_eq!(out.files, ["Serum - Lead.fxp"]);
    assert_eq!(out.failed.len(), 1);
    assert!(out.failed[0].starts_with("Serum - Lead.bwpreset: "), "{:?}", out.failed);
    assert!(!out.folder.join("Serum - Lead.bwpreset").exists());
}

#[test]
fn a_sound_for_another_plugin_is_refused() {
    // Named "Serum", but its settings are Diva's.
    let dir = folder("other-plugin");
    let t = send::inspect(&saved_in_bitwig(&dir, "Init", "Serum", "Init", "a.fxb")).unwrap();
    assert_eq!(t.plugin_id, "XfsX");
    let diva = PresetBlob::new("Pad", "Serum", ".fxp", fxp::wrap_chunk(&[1], "Pad", i32::from_be_bytes(*b"DiVa")));
    let e = build(&t, &diva, "Pad").unwrap_err();
    assert!(e.contains("DiVa"), "{e}");
}

#[test]
fn a_bitwig_name_is_kept_for_each_sound_whether_or_not_one_is_made() {
    // Two plugins sharing a sound's name would share its Bitwig preset's name.
    let pad = |ext: &'static str| PresetBlob::new("Pad", "Pad", ext, vec![1]);
    assert_eq!(export::file_names(&[pad(".fxp"), pad(".vstpreset")]), ["Pad.fxp", "Pad 2.vstpreset"]);
    // Only the ending is taken off: "Lead 1.1" and "Lead 1.2" with no ending don't share "Lead 1.bwpreset".
    let lead = |name: &str| PresetBlob::new(name, "", "", vec![1]);
    assert_eq!(export::file_names(&[lead("Lead 1.1"), lead("Lead 1.2")]), ["Lead 1.1", "Lead 1.2"]);
}

#[test]
fn an_old_bitwig_preset_left_from_an_earlier_export_is_told() {
    let dir = folder("stale");
    let t = send::inspect(&saved_in_bitwig(&dir, "Init", "Serum", "Init", "a.fxb")).unwrap();
    let out = dir.join("out");
    export::export(&[serum_sound("Lead", "Serum")], "Song.flp", &out, Some(&one(t))).unwrap();
    // Exported again with no template for Serum now.
    let again = export::export(&[serum_sound("Lead", "Serum")], "Song.flp", &out, Some(&Templates::new())).unwrap();
    assert_eq!(again.files, ["Serum - Lead.fxp"]);
    assert_eq!(again.failed.len(), 1);
    assert!(again.failed[0].contains("earlier export is still there"), "{:?}", again.failed);
}

#[test]
fn a_plugin_with_no_template_of_its_own_gets_a_stand_in_of_its_format() {
    // Solomon has a Bitwig preset of Serum, none of Hive (5 Nov 2026: Hive never sent).
    let dir = folder("stand-in");
    let t = send::inspect(&saved_in_bitwig(&dir, "Init", "Serum", "Init", "a.fxb")).unwrap();
    let templates = one(t.clone());
    let hive_id = i32::from_be_bytes(*b"HiVe");
    let hive = PresetBlob::new("Hive - Hive", "Hive", ".fxp", fxp::wrap_chunk(&[4, 5, 6], "Bass", hive_id));
    assert!(send::template_for(&hive, &templates).is_none());
    assert_eq!(send::stand_in_for(&hive, &templates), Some(&t));
    // A VST3 sound has no VST2 stand-in.
    let vst3 = PresetBlob::new("Pad", "Diva", ".vstpreset", b"VST3".to_vec());
    assert!(send::stand_in_for(&vst3, &templates).is_none());

    let raw = std::fs::read(&t.path).unwrap();
    let made = send::build_for_other(&raw, &t.path, &hive, "Hive - Hive").unwrap();
    let doc = bitwig::decode(made).unwrap();
    let got = send::inspect_doc(&doc).unwrap();
    // The device is Hive's now, by its name and its VST2 id, and holds Hive's settings as a bank.
    assert_eq!((got.plugin.as_str(), got.plugin_id.as_str()), ("Hive", "HiVe"));
    let device = (0..doc.objects.len() as u32).map(bitwig::ObjId).find(|&o| doc.obj(o).str(0x9a) == "Hive").unwrap();
    assert_eq!(doc.obj(device).get(0xce4), Some(&bitwig::Value::I32(hive_id)));
    let state = zip_file(&doc, &got.state_file).unwrap();
    assert_eq!(&state[8..12], b"FBCh");
    // The stand-in's own name for its preset became the sound's.
    assert_eq!(doc.obj(device).str(0x12de), "Hive - Hive");
    // The template itself is only read.
    assert_eq!(std::fs::read(&t.path).unwrap(), raw);

    // Settings that don't say which plugin they're for aren't given a stand-in's device.
    let unknown = PresetBlob::new("X", "Hive", ".fxp", b"not a program".to_vec());
    assert!(send::build_for_other(&raw, &t.path, &unknown, "X").is_err());
}

#[test]
fn export_uses_a_stand_in_when_the_plugin_has_no_template() {
    let dir = folder("stand-in-export");
    let t = send::inspect(&saved_in_bitwig(&dir, "Init", "Serum", "Init", "a.fxb")).unwrap();
    let templates = one(t);
    let hive = PresetBlob::new("Lead", "Hive", ".fxp", fxp::wrap_chunk(&[7], "Lead", i32::from_be_bytes(*b"HiVe")));
    let out = dir.join("out");
    let e = export::export(&[hive], "/songs/Song.flp", &out, Some(&templates)).unwrap();
    assert!(e.files.iter().any(|f| f.ends_with(".bwpreset")), "{:?} {:?}", e.files, e.failed);
}

#[test]
fn a_plugins_own_preset_file_becomes_a_bitwig_preset_to_drag() {
    // Solomon, 13 Nov: Bitwig takes only its own presets dropped on a track.
    let dir = folder("from-file");
    let t = send::inspect(&saved_in_bitwig(&dir, "Init Lead", "Serum", "Init Lead", "0f1e2d.fxb")).unwrap();
    let templates = one(t);
    let file = dir.join("Big Bass.fxp");
    std::fs::write(&file, serum_sound("Big Bass", "Serum").data).unwrap();
    let to = dir.join("Big Bass.bwpreset");
    let made = export::bitwig_preset_from_file(&file, "Serum", mo_formats::PluginKind::Instrument, &templates, &to);
    assert_eq!(made.unwrap(), Some(()));
    let doc = bitwig::decode(std::fs::read(&to).unwrap()).unwrap();
    assert_eq!(doc.meta_str("preset_name"), "Big Bass");
    assert_eq!(std::fs::read(&file).unwrap(), serum_sound("Big Bass", "Serum").data, "the file is only read");
    // No template of its format: nothing made, nothing written.
    let none = dir.join("None.bwpreset");
    let made =
        export::bitwig_preset_from_file(&file, "Serum", mo_formats::PluginKind::Instrument, &Templates::new(), &none);
    assert_eq!(made.unwrap(), None);
    assert!(!none.exists());
    // Not a preset: an error, nothing written.
    let junk = dir.join("Junk.fxp");
    std::fs::write(&junk, b"not a preset").unwrap();
    assert!(
        export::bitwig_preset_from_file(&junk, "Serum", mo_formats::PluginKind::Instrument, &templates, &none).is_err()
    );
    assert!(!none.exists());
}
