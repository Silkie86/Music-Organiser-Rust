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
        b"CcnK bank from Bitwig",
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
    Templates::from([(send::key(&t.plugin, t.format()), t)])
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

    let doc = bitwig::decode(send::build(&t, &sound, "Big Bass").unwrap()).unwrap();
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
    let doc = bitwig::decode(send::build(&t, &serum_sound("Pad", "Serum"), "Pad").unwrap()).unwrap();
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
    let doc = bitwig::decode(send::build(&t, &serum_sound("Pad", "Serum"), "Pad").unwrap()).unwrap();
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
}

#[test]
fn export_writes_a_bitwig_preset_beside_the_plugin_preset() {
    let dir = folder("export");
    let t = send::inspect(&saved_in_bitwig(&dir, "Init", "Serum", "Init", "a.fxb")).unwrap();
    let presets = [serum_sound("Big Bass", "Serum"), serum_sound("Wide", "Diva")];
    let out = export::export(&presets, "Song.flp", &dir.join("out"), Some(&one(t))).unwrap();
    assert_eq!(out.files, ["Serum - Big Bass.fxp", "Serum - Big Bass.bwpreset", "Diva - Wide.fxp"]);
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
fn a_bitwig_name_is_kept_for_each_sound_whether_or_not_one_is_made() {
    // Two plugins sharing a sound's name would share its Bitwig preset's name.
    let pad = |ext: &'static str| PresetBlob::new("Pad", "Pad", ext, vec![1]);
    assert_eq!(export::file_names(&[pad(".fxp"), pad(".vstpreset")]), ["Pad.fxp", "Pad 2.vstpreset"]);
}
