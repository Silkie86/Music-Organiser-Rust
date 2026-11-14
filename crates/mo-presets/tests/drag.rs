//! Dragging a preset out of the Presets page: what it carries, in folders of the test's own (never Solomon's Bitwig
//! folder or presets).

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use mo_formats::PluginKind;
use mo_formats::bitwig;
use mo_formats::presets::{PresetBlob, fxp, vstpreset};
use mo_presets::bitwig::{self as send, Templates};
use mo_presets::drag::{self, Carried, File};

mod common;
use common::{bwpreset_full, text, with_fields};

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-presets-drag").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const SERUM: [u8; 4] = *b"XfsX";

/// A preset Bitwig saved for Serum, its settings in the zip as a bank.
fn serum_template(dir: &Path) -> send::Template {
    let raw = bwpreset_full(
        &[("preset_name", "Init")],
        &[],
        "Serum",
        "Init",
        "0f1e2d.fxb",
        "0f1e2d.fxb",
        &send::to_bank(&fxp::wrap_chunk(&[9], "Init", i32::from_be_bytes(SERUM)), 0).unwrap(),
        &[],
    );
    let path = dir.join("Init.bwpreset");
    std::fs::write(&path, raw).unwrap();
    send::inspect(&path).unwrap()
}

fn one(t: send::Template) -> Templates {
    Templates::from([(send::key(&t.plugin, t.format()).unwrap(), t)])
}

fn zip_file(doc: &bitwig::Document, name: &str) -> Option<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(doc.zip()?)).ok()?;
    let mut f = zip.by_name(name).ok()?;
    let mut out = Vec::new();
    f.read_to_end(&mut out).unwrap();
    Some(out)
}

/// The made file's name and bytes.
fn made(c: &Carried) -> (&str, &[u8]) {
    match &c.file {
        File::Made { name, bytes } => (name, bytes),
        File::There(p) => panic!("{} went as it is ({:?})", p.display(), c.why_not),
    }
}

#[test]
fn a_vst_preset_file_goes_as_a_bitwig_preset_made_from_it() {
    let dir = folder("file");
    let t = serum_template(&dir);
    assert_eq!((t.plugin.as_str(), t.plugin_id.as_str()), ("Serum", "XfsX"));
    let program = fxp::wrap_chunk(&[7, 7], "Wobble", i32::from_be_bytes(SERUM));
    let path = dir.join("Wobble.fxp");
    std::fs::write(&path, &program).unwrap();

    // A VST2 file names its plugin only by its id, which is what the page shows: its template is found by the id.
    let c = drag::from_file(&path, "XfsX", PluginKind::Unknown, Some(&one(t.clone()))).unwrap();
    assert!(c.bitwig);
    assert_eq!(c.why_not, None);
    let (name, bytes) = made(&c);
    assert_eq!(name, "Wobble.bwpreset");
    let doc = bitwig::decode(bytes.to_vec()).unwrap();
    assert_eq!(doc.body_error, None);
    assert_eq!(doc.meta_str("preset_name"), "Wobble");
    let device = doc.objects.iter().find(|o| !o.str(0xbe5).is_empty()).unwrap();
    // Serum's own template, so its device stays Serum's, holding the program as a one-program bank (under a name of
    // its own: see `bitwig::fresh_state_name`).
    assert_eq!((device.str(0x9a), device.str(0x12de)), ("Serum", "Wobble"));
    let got = send::inspect_doc(&doc).unwrap();
    assert_ne!(got.state_file, "plugin-states/0f1e2d.fxb");
    assert_eq!(zip_file(&doc, &got.state_file), send::to_bank(&program, 0));
    // Nothing was written: not the file dragged, not the template.
    assert_eq!(std::fs::read(&path).unwrap(), program);
    let mut names: Vec<String> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["Init.bwpreset", "Wobble.fxp"]);
}

#[test]
fn another_plugins_preset_file_gets_a_stand_in_of_its_format() {
    let dir = folder("stand-in");
    let templates = one(serum_template(&dir));
    let path = dir.join("Bass.fxp");
    std::fs::write(&path, fxp::wrap_chunk(&[4, 5], "Bass", i32::from_be_bytes(*b"HiVe"))).unwrap();
    let c = drag::from_file(&path, "Hive", PluginKind::Unknown, Some(&templates)).unwrap();
    let doc = bitwig::decode(made(&c).1.to_vec()).unwrap();
    let got = send::inspect_doc(&doc).unwrap();
    // Serum's template, made Hive's by the name the page shows and the id in the settings.
    assert_eq!((got.plugin.as_str(), got.plugin_id.as_str()), ("Hive", "HiVe"));
}

#[test]
fn a_plugin_id_matches_only_to_the_letter() {
    let dir = folder("id-case");
    let templates = one(serum_template(&dir));
    // Another plugin, whose id is Serum's in other capitals: a VST2's id is four bytes, so it's another plugin.
    let path = dir.join("Other.fxp");
    std::fs::write(&path, fxp::wrap_chunk(&[1, 2], "Other", i32::from_be_bytes(*b"xfsx"))).unwrap();
    let c = drag::from_file(&path, "xfsx", PluginKind::Unknown, Some(&templates)).unwrap();
    let got = send::inspect_doc(&bitwig::decode(made(&c).1.to_vec()).unwrap()).unwrap();
    // Serum's template stands in and is made the other plugin's: never Serum loaded with the other one's settings.
    assert_eq!((got.plugin.as_str(), got.plugin_id.as_str()), ("xfsx", "xfsx"));
}

#[test]
fn a_bitwig_preset_or_one_none_can_be_made_of_goes_as_it_is() {
    let dir = folder("as-it-is");
    let t = serum_template(&dir);
    let c = drag::from_file(&t.path, "Serum", PluginKind::Unknown, Some(&one(t.clone()))).unwrap();
    assert_eq!(c, Carried { file: File::There(t.path.clone()), bitwig: true, why_not: None });

    // No preset Bitwig saved to make one from: the file as it is, and why.
    let fxp_path = dir.join("Lead.fxp");
    std::fs::write(&fxp_path, fxp::wrap_chunk(&[1], "Lead", i32::from_be_bytes(SERUM))).unwrap();
    for templates in [None, Some(Templates::new())] {
        let c = drag::from_file(&fxp_path, "XfsX", PluginKind::Unknown, templates.as_ref()).unwrap();
        assert_eq!(c.file, File::There(fxp_path.clone()));
        assert!(!c.bitwig);
        assert!(c.why_not.as_deref().unwrap().contains("VST2"), "{:?}", c.why_not);
    }
    // A VST3 preset has no VST2 template to stand in.
    let vst3 = dir.join("Pad.vstpreset");
    std::fs::write(&vst3, b"VST3 not much of one").unwrap();
    let c = drag::from_file(&vst3, "Diva", PluginKind::Unknown, Some(&one(t))).unwrap();
    assert_eq!(c.file, File::There(vst3.clone()));
    assert!(c.why_not.is_some());

    // Gone since it was listed.
    assert!(drag::from_file(&dir.join("Gone.fxp"), "XfsX", PluginKind::Unknown, None).is_err());
    assert!(drag::from_file(&dir, "XfsX", PluginKind::Unknown, None).is_err(), "a folder isn't a preset");
}

#[test]
fn a_songs_sound_goes_as_a_bitwig_preset_or_as_its_plugin_preset() {
    let dir = folder("sound");
    let templates = one(serum_template(&dir));
    let lead = || PresetBlob::new("Lead", "Serum", ".fxp", fxp::wrap_chunk(&[3], "Lead", i32::from_be_bytes(SERUM)));

    let c = drag::from_sound(lead(), "Serum - Lead.fxp".into(), Some(&templates));
    assert!(c.bitwig);
    let (name, bytes) = made(&c);
    assert_eq!(name, "Serum - Lead.bwpreset");
    assert_eq!(bitwig::decode(bytes.to_vec()).unwrap().meta_str("preset_name"), "Lead");

    // With no template, its plugin preset as an export writes it.
    let c = drag::from_sound(lead(), "Serum - Lead.fxp".into(), None);
    assert_eq!(made(&c), ("Serum - Lead.fxp", lead().data.as_slice()));
    assert!(!c.bitwig && c.why_not.is_some());

    // An FL plugin's own settings can't be a Bitwig preset: as they are, with nothing to say.
    let limiter = PresetBlob::new("Fruity Limiter", "Fruity Limiter", ".flstate", vec![1, 2, 3]);
    let c = drag::from_sound(limiter, "Fruity Limiter.flstate".into(), Some(&templates));
    assert_eq!(
        c,
        Carried {
            file: File::Made { name: "Fruity Limiter.flstate".into(), bytes: vec![1, 2, 3] },
            bitwig: false,
            why_not: None
        }
    );
}

#[test]
fn a_songs_sound_is_read_from_its_save_again() {
    // A recipe save made by hand with FL's own instruments (rule 4's exception), so its sounds are FL's own settings.
    let save = concat!(env!("CARGO_MANIFEST_DIR"), "/../mo-formats/tests/fixtures/recipe/fl/MO Recipe 08 sampler.flp");
    let sounds = mo_presets::library::read_save(save, &mo_presets::Thresholds::default()).unwrap().sounds;
    let key = mo_presets::list::place_keys(&sounds).into_iter().next().expect("the recipe has a sound");
    let c = drag::from_save(save, &key, PluginKind::Unknown, None).unwrap();
    let (name, bytes) = made(&c);
    // Its settings as an export writes them, though the page's list let them go once it was read.
    assert!(name.ends_with(&*sounds[0].blob.extension), "{name}");
    assert!(!bytes.is_empty());
    assert!(!c.bitwig);
    let gone = drag::from_save(save, "no such sound", PluginKind::Unknown, None).unwrap_err();
    assert!(gone.to_string().contains("isn't in the save"), "{gone}");
}

#[test]
fn a_synths_preset_file_goes_as_an_instruments_bitwig_preset() {
    // Solomon, 14 Nov: a dragged Keyscape preset went onto an audio track. The only VST3 preset Bitwig saved is an
    // effect's; the page knows Keyscape is a synth (the installed plugin says), though its file doesn't.
    let dir = folder("synth-file");
    let pro_q = "72C4DB717A4D459AB97E51745D84B39D";
    let raw = bwpreset_full(
        &[],
        &[],
        "Pro-Q 3",
        "Init",
        "a.vstpreset",
        "a.vstpreset",
        &vstpreset::build(pro_q, b"", b""),
        &[],
    );
    let raw = with_fields(
        raw,
        &[("device_type", text("audio_to_audio")), ("device_name", text("Pro-Q 3"))],
        &[(0x9c, text("EQ")), (0x9d, mo_formats::bitwig::Value::U8(0)), (0xce4, text(pro_q))],
    );
    let path = dir.join("Pro-Q 3.bwpreset");
    std::fs::write(&path, raw).unwrap();
    let templates = one(send::inspect(&path).unwrap());
    let keys = dir.join("Grand.vstpreset");
    std::fs::write(&keys, vstpreset::build("ABCDEF0123456789ABCDEF0123456789", b"keys", b"")).unwrap();

    let c = drag::from_file(&keys, "Keyscape", PluginKind::Instrument, Some(&templates)).unwrap();
    let doc = bitwig::decode(made(&c).1.to_vec()).unwrap();
    assert_eq!((doc.meta_str("device_type"), doc.meta_str("device_name")), ("note_to_audio", "Keyscape"));
    let got = send::inspect_doc(&doc).unwrap();
    assert_eq!((got.plugin.as_str(), got.kind), ("Keyscape", PluginKind::Instrument));

    // When neither the page nor the file knows what it is, the template's kind stays.
    let c = drag::from_file(&keys, "Keyscape", PluginKind::Unknown, Some(&templates)).unwrap();
    let doc = bitwig::decode(made(&c).1.to_vec()).unwrap();
    assert_eq!(doc.meta_str("device_type"), "audio_to_audio");
}
