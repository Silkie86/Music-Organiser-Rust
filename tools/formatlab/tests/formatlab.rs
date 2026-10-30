//! Format Lab's tests, ported from the C# app's FormatLabTests, FormatRegistryTests, FormatCensusTests and
//! FormatLabCommandTests, on the same made-up files (mo-formats' test fixtures).

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../crates/mo-formats/tests/common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};

use common::Flp;
use common::bitwig::{bitwig_project, bitwig_project_bytes};
use mo_formatlab::census::Census;
use mo_formatlab::command;
use mo_formatlab::lab::{Data, Format, LabFile};
use mo_formatlab::ops::{self, ChangeKind};
use mo_formatlab::registry::{self, LayoutField};

fn fl(raw: &[u8]) -> LabFile {
    LabFile::read(raw, "test.flp").unwrap()
}

fn write(dir: &Path, name: &str, raw: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, raw).unwrap();
    path
}

/// `raw` with the first `from` in it changed to `to`.
fn patched(raw: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let at = raw.windows(from.len()).position(|w| w == from).expect("the bytes to change");
    let mut out = raw.to_vec();
    out[at..at + to.len()].copy_from_slice(to);
    out
}

fn dword(id: u8, v: u32) -> Vec<u8> {
    [&[id][..], &v.to_le_bytes()].concat()
}

/// An FL project's file with more events at its end.
fn with_events(raw: &[u8], more: &[u8]) -> Vec<u8> {
    let mut out = [raw, more].concat();
    let len = i32::from_le_bytes(out[18..22].try_into().unwrap()) + more.len() as i32;
    out[18..22].copy_from_slice(&len.to_le_bytes());
    out
}

/// The FL Studio 25 project with a pattern of two notes after its channels, as the C# tests' `withNotes`.
fn fl_studio_25_with_notes() -> Vec<u8> {
    let notes = [common::note(0, 1, 96, 60, 100), common::note(0, 2, 96, 64, 100)].concat();
    let more = Flp::new().word(65, 1).data(224, &notes).build();
    // The builder's events start after its 22-byte header.
    with_events(&common::fl_studio_25_project(), &more[22..])
}

// ---- The registry ----

#[test]
fn both_registries_load_with_their_names() {
    let (fl, bw) = (registry::fl_studio(), registry::bitwig());
    assert_eq!(fl.event(156).map(|e| e.name.as_str()), Some("tempo"));
    assert_eq!(fl.event(199).and_then(|e| e.kind.as_deref()), Some("ascii"));
    assert_eq!(fl.wrapper_record(54).map(|e| e.name.as_str()), Some("plugin name"));
    assert_eq!(fl.vst3_record(4).map(|e| e.name.as_str()), Some("setting IDs"));
    assert_eq!(bw.class(0x728).map(|e| e.name.as_str()), Some("VST3 plugin"));
    assert_eq!(bw.field(0x9a).map(|e| e.name.as_str()), Some("device name"));
    assert!(bw.meta_key("application_version_name").is_some());
    assert!(fl.event(151).is_none());
}

#[test]
fn ids_are_written_once_and_every_layout_reads() {
    // Parsing checks that each id is a number and is written once.
    for json in [include_str!("../registry/fl-studio.json"), include_str!("../registry/bitwig.json")] {
        let r = registry::Registry::parse(json).unwrap();
        for e in r.events.iter().chain(&r.wrapper).chain(&r.vst3).chain(&r.classes).chain(&r.fields) {
            assert!(!e.name.trim().is_empty(), "{} has no name", e.id);
        }
    }
    for e in registry::fl_studio().events.iter().filter(|e| e.layout.is_some() || e.record_layout.is_some()) {
        LayoutField::parse(e.layout.as_deref()).unwrap();
        let smallest = e.record_by_version.as_ref().and_then(|v| v.values().min().copied()).or(e.record).unwrap_or(0);
        for f in LayoutField::parse(e.record_layout.as_deref()).unwrap() {
            assert!(f.offset + f.size() <= smallest, "{}: {} runs past its {smallest}-byte record", e.id, f.name);
        }
    }
    assert!(LayoutField::parse(Some("pan i32")).is_err());
    assert!(LayoutField::parse(Some("pan x7@0")).is_err());
}

#[test]
fn a_record_size_follows_fl_s_version() {
    let notes = registry::fl_studio().event(224).unwrap();
    assert_eq!(notes.record_size(25), Some(24));
}

/// The FL reader's event and wrapper record ids (`EV_…` and `VST_…` constants in mo-formats' `fl` folder) are each in
/// the registry, under the C# name for them.
#[test]
fn every_id_the_fl_reader_uses_is_in_the_registry() {
    let fl = registry::fl_studio();
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/mo-formats/src/fl");
    let mut checked = 0;
    for file in std::fs::read_dir(folder).unwrap() {
        let code = std::fs::read_to_string(file.unwrap().path()).unwrap();
        for line in code.lines() {
            // Private or public alike.
            let line = line.trim_start();
            let Some(rest) = ["const ", "pub const ", "pub(crate) const ", "pub(super) const "]
                .iter()
                .find_map(|p| line.strip_prefix(p))
                .filter(|r| r.starts_with("EV_") || r.starts_with("VST_"))
            else {
                continue;
            };
            let (name, value) = rest.split_once(':').unwrap();
            let value = value.split('=').nth(1).map(|v| v.trim().trim_end_matches(';')).unwrap();
            let value = match value.strip_prefix("0x") {
                Some(hex) => u32::from_str_radix(&hex.replace('_', ""), 16),
                None => value.replace('_', "").parse::<u32>(),
            }
            .unwrap_or_else(|_| panic!("{name}'s value {value} isn't a number this test reads"));
            let camel: String =
                name.split('_').map(|p| p[..1].to_owned() + &p[1..].to_lowercase()).collect::<Vec<_>>().concat();
            let entry = if name.starts_with("EV_") {
                fl.event(value)
            } else if name.starts_with("VST_") {
                fl.wrapper_record(value)
            } else {
                continue;
            };
            assert_eq!(entry.and_then(|e| e.code.as_deref()), Some(camel.as_str()), "{name} ({value})");
            checked += 1;
        }
    }
    assert!(checked > 20, "only {checked} ids found");
}

// ---- FL Studio ----

#[test]
fn tree_names_fl_events_and_takes_their_data_apart() {
    let f = fl(&fl_studio_25_with_notes());
    assert_eq!(f.format, Format::FlStudio);
    assert_eq!(f.version, "25.1.0.4868");
    let tree = ops::tree(&f, usize::MAX);
    assert!(tree.contains("  156 tempo = 126000"), "{tree}");
    assert!(tree.contains("channel 2\n"));
    assert!(tree.contains("    wrapper 51 VST2 ID = 1483109208 (\"XfsX\")"));
    assert!(tree.contains("    wrapper 54 plugin name = \"Arcade\""));
    assert!(tree.contains("      vst3 4 setting IDs = <16 bytes>"));
    assert!(tree.contains("    [1]: <24 bytes>"));
    assert!(tree.contains("      key = 64"));

    let notes = f.items.iter().find(|i| i.key == "pattern 1/224/[1]").unwrap();
    assert_eq!(notes.span, Some((24, 24)));
    assert_eq!(f.items.iter().find(|i| i.key == "pattern 1/224/[1]/channel").unwrap().data, Data::Int(2));
    assert!(!ops::tree(&f, 2).contains("velocity"));
}

#[test]
fn fl_scopes_follow_channels_patterns_and_mixer_slots() {
    let f = fl(&common::fl_studio_project());
    let has = |k: &str| f.items.iter().any(|i| i.key == k);
    assert!(has("channel 1/196"));
    assert!(has("pattern 2/193"));
    assert!(has("insert 0, slot 0/201"));
    assert!(has("insert 0, slot 1/213/wrapper 54"));
    assert_eq!(f.items.iter().find(|i| i.key == "insert 0, slot 1/213/wrapper 54").unwrap().value, "\"Pro-Q 3\"");
}

#[test]
fn diff_names_each_changed_value() {
    let raw = common::fl_studio_project();
    let before = fl(&raw);
    let after = fl(&patched(&raw, &dword(156, 128_000), &dword(156, 121_500)));
    let changes = ops::diff(&before, &after);
    let [change] = changes.as_slice() else { panic!("{changes:?}") };
    assert_eq!(change.kind, ChangeKind::Changed);
    assert_eq!(
        (change.key.as_str(), change.before.as_deref(), change.after.as_deref()),
        ("project/156", Some("128000"), Some("121500"))
    );
    assert!(ops::diff_text(&before, &after, &changes).contains("changed  project › 156 tempo"));

    let utf16 = |s: &str| s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
    let renamed = fl(&patched(&raw, &utf16("Kick"), &utf16("Kock")));
    let keys: Vec<String> = ops::diff(&before, &renamed).into_iter().map(|c| c.key).collect();
    assert_eq!(keys, ["channel 1/203"]);
    assert!(ops::diff(&before, &fl(&raw)).is_empty());
}

#[test]
fn diff_lists_an_added_part_once_with_what_it_holds() {
    let plain = fl(&common::fl_studio_project());
    let playlist = [common::playlist_item(0, 1, 384, -1), common::playlist_item(384, 2, 384, -1)].concat();
    let arranged = fl(&common::fl_studio_project_with(Some(&playlist)));
    let changes = ops::diff(&plain, &arranged);
    let [change] = changes.as_slice() else { panic!("{changes:?}") };
    assert_eq!((change.kind, change.key.as_str()), (ChangeKind::Added, "arrangement"));
    // The playlist event, its records and their numbers.
    assert!(change.inside >= 3);
    let back = ops::diff(&arranged, &plain);
    let [back] = back.as_slice() else { panic!("{back:?}") };
    assert_eq!((back.kind, back.key.as_str()), (ChangeKind::Removed, "arrangement"));
}

#[test]
fn diff_says_where_data_differs() {
    let raw = common::fl_studio_25_project();
    let changes = ops::diff(&fl(&raw), &fl(&patched(&raw, b"XsfX", b"XsfY")));
    let [change] = changes.as_slice() else { panic!("{changes:?}") };
    assert_eq!(change.key, "channel 1/213/wrapper 51");
}

#[test]
fn diff_of_bytes_says_which_bytes() {
    let raw = common::fl_studio_project();
    // Serum's own state, 1 2 3 4, changed in its second and third bytes.
    let changes = ops::diff(&fl(&raw), &fl(&patched(&raw, &[1, 2, 3, 4], &[1, 9, 9, 4])));
    let [change] = changes.as_slice() else { panic!("{changes:?}") };
    assert_eq!(change.key, "channel 0/213/wrapper 53");
    assert_eq!(change.detail.as_deref(), Some("2 bytes between 1 and 2: 02 03 04 → 09 09 04"));
}

#[test]
fn find_looks_for_numbers_in_fields_and_raw_bytes() {
    let f = fl(&common::fl_studio_25_project());
    let tempo = ops::find(&f, "126000", false);
    let [tempo] = tempo.as_slice() else { panic!("{tempo:?}") };
    assert_eq!((tempo.path.as_str(), tempo.how.as_str()), ("project › 156 tempo", "number"));

    // Predator's first setting, 0.25, is a float inside the plugin's own state.
    let setting = ops::find(&f, "0.25", false);
    let [setting] = setting.as_slice() else { panic!("{setting:?}") };
    assert_eq!(setting.path, "channel 3 › 213 plugin state › wrapper 53 plugin's own state");
    assert!(setting.how.starts_with("32-bit float at byte "), "{}", setting.how);

    let text = ops::find(&f, "arcade", false);
    assert!(text.iter().any(|h| h.path.ends_with("wrapper 54 plugin name") && h.how == "text"));
    assert!(ops::find(&f, "126000", true).is_empty());
    assert!(ops::find_text(&f, "Nope", &ops::find(&f, "Nope", false), 200).contains("isn't in it"));
}

#[test]
fn find_does_not_repeat_what_a_record_found_for_the_data_around_it() {
    let f = fl(&common::fl_studio_project());
    // The second note's velocity is 128: found as that number, not again as bytes of its record or event.
    let hits: Vec<_> = ops::find(&f, "128", false).into_iter().filter(|h| h.path.starts_with("pattern 1")).collect();
    let [hit] = hits.as_slice() else { panic!("{hits:?}") };
    assert_eq!((hit.path.as_str(), hit.how.as_str()), ("pattern 1 › 224 pattern notes[1] › velocity", "number"));
}

#[test]
fn shows_only_what_is_in_the_file_and_says_what_was_off() {
    // FL 9's step sequencer dots stay dots: the notes the reader makes of them aren't in the file.
    let f = fl(&common::fl_studio_9_steps());
    assert!(f.items.iter().any(|i| i.key.ends_with("/91")), "{}", ops::tree(&f, 1));
    assert!(!f.items.iter().any(|i| i.key.ends_with("/224")));

    // A save cut short is read up to the cut, and says so.
    let raw = common::fl_studio_project();
    let cut = fl(&raw[..raw.len() - 10]);
    assert!(cut.warning.is_some() && cut.problem.is_none());
    assert!(ops::tree(&cut, 0).lines().nth(1).unwrap().starts_with("Warning: "));
    assert!(ops::diff_text(&cut, &fl(&raw), &[]).contains("Warning about test.flp: "));
}

#[test]
fn an_arrangement_starts_with_its_own_event() {
    let playlist = common::playlist_item(0, 1, 384, -1);
    let raw = with_events(
        &common::fl_studio_project(),
        &Flp::new().word(99, 0).text(241, "Arrangement").data(233, &playlist).build()[22..],
    );
    let f = fl(&raw);
    for key in ["arrangement 0/99", "arrangement 0/241", "arrangement 0/233"] {
        assert!(f.items.iter().any(|i| i.key == key), "{key} isn't in\n{}", ops::tree(&f, 1));
    }
    // Without event 99 (before FL 12.9), the playlist starts it.
    assert!(fl(&common::fl_studio_project_with(Some(&playlist))).items.iter().any(|i| i.key == "arrangement/233"));
}

#[test]
fn find_looks_for_a_number_as_text_in_data_too() {
    // Data the registry doesn't know, holding "808 Kick" among bytes that aren't text.
    let more = Flp::new().data(250, b"\x01\x02808 Kick\x03").build();
    let f = fl(&with_events(&common::fl_studio_project(), &more[22..]));
    let hits = ops::find(&f, "808", false);
    assert!(hits.iter().any(|h| h.how == "UTF-8 text at byte 2" && h.path == "insert 0 › 250"), "{hits:?}");
}

#[test]
fn numbers_are_written_as_the_c_sharp_tool_wrote_them() {
    use mo_formatlab::lab::float;
    assert_eq!(
        [float(128.5), float(133.0), float(-0.0), float(0.0001), float(1e-5), float(1.5e-7), float(1e14), float(1e15)],
        ["128.5", "133", "-0", "0.0001", "1E-05", "1.5E-07", "100000000000000", "1E+15"]
    );
    assert_eq!(
        [float(f64::INFINITY), float(f64::NAN), float(f64::from(0.1f32))],
        ["Infinity", "NaN", "0.10000000149011612"]
    );
}

// ---- Bitwig ----

#[test]
fn tree_shows_bitwig_objects_nested_documents_and_zip_files() {
    let f = LabFile::read(&bitwig_project_bytes(), "Loop Idea.bwproject").unwrap();
    assert_eq!(f.format, Format::Bitwig);
    assert_eq!(f.version, "5.2.4");
    assert_eq!(f.problem, None);
    let tree = ops::tree(&f, usize::MAX);
    for line in [
        "meta structure (summary): <",
        "0xad1 shown name = \"Keys Display\"",
        "0x2c8 value = 128.5",
        "[0]: 0x728 VST3 plugin",
        "0x9a device name = \"Keyscape\"",
        "0x4e0 master track = → project group › tracks[0] › 0x2bb0 (0x90 track)",
        "plugin-states/aaaa-keys.vstpreset = <15 bytes> \"VST3 keys state\"",
    ] {
        assert!(tree.contains(line), "{line} isn't in\n{tree}");
    }
}

#[test]
fn bitwig_diff_finds_a_changed_tempo() {
    let raw = bitwig_project_bytes();
    let before = LabFile::read(&raw, "Loop Idea.bwproject").unwrap();
    let after =
        LabFile::read(&patched(&raw, &128.5f64.to_be_bytes(), &133f64.to_be_bytes()), "Loop Idea.bwproject").unwrap();
    let changes = ops::diff(&before, &after);
    let [change] = changes.as_slice() else { panic!("{changes:?}") };
    assert_eq!((change.before.as_deref(), change.after.as_deref()), (Some("128.5"), Some("133")));
    assert!(change.path.ends_with("value"), "{}", change.path);

    assert!(ops::find(&after, "133", false).iter().any(|h| h.how == "number" && h.path.ends_with("value")));
    // The device and its file.
    assert_eq!(ops::find(&after, "keyscape", false).len(), 2);
}

#[test]
fn diff_sees_a_change_past_what_is_shown() {
    let file = |last: f64| {
        let values: Vec<f64> = (0..40).map(|i| if i == 39 { last } else { i as f64 }).collect();
        common::bitwig::BwWriter::file(
            |m| {
                m.meta_entry("application_version_name", 0x08);
                m.str_val("6.1.3");
            },
            |b| {
                b.begin(0x2e);
                b.doubles(0x123, &values);
                b.end();
            },
            None,
        )
    };
    let (before, after) =
        (LabFile::read(&file(1.0), "a.bwproject").unwrap(), LabFile::read(&file(2.0), "b.bwproject").unwrap());
    let changes = ops::diff(&before, &after);
    // Only the first 32 of the 40 numbers are shown, the same in both.
    let [change] = changes.as_slice() else { panic!("{changes:?}") };
    assert_eq!(change.before, change.after);
    assert!(ops::diff(&before, &LabFile::read(&file(1.0), "c.bwproject").unwrap()).is_empty());
}

#[test]
fn refuses_files_from_other_apps() {
    assert!(LabFile::read(b"RIFF....WAVE", "kick.wav").is_err());
    assert!(LabFile::read(b"FLhd", "short.flp").is_err());
}

// ---- The census ----

#[test]
fn census_counts_what_the_registry_does_not_know() {
    let dir = common::test_dir("formatlab-census");
    write(&dir, "Night Drive.flp", &common::fl_studio_project());
    write(&dir, "Shuffle.flp", &common::fl_studio_25_project());
    write(&dir, "Odd.flp", &Flp::new().ascii(199, "24.1.1.4234").dword(151, 7).dword(151, 8).build());
    bitwig_project(&dir, "Loop Idea");
    write(&dir, "Broken.flp", &[1, 2, 3]);
    write(&dir, "notes.txt", b"not a project");
    // Left out, as the C# app's library scan leaves them out.
    write(&dir, "._Night Drive.flp", &[0, 5, 22, 7]);
    write(&dir, "Night Drive (autosave).flp", &common::fl_studio_project());
    for folder in ["Backup", ".hidden", "Templates"] {
        std::fs::create_dir_all(dir.join(folder)).unwrap();
        write(&dir.join(folder), "Night Drive.flp", &common::fl_studio_project());
    }

    let files = mo_formatlab::census::find_projects(&[dir]);
    let census = Census::run(&files, |_| {});
    assert_eq!((census.fl_projects, census.bitwig_projects, census.unreadable_count), (3, 1, 1));
    assert_eq!(census.fl_versions.keys().collect::<Vec<_>>(), ["21", "24", "25"]);
    let unknown = census.unknown_fl_events();
    let [(id, tally)] = unknown.as_slice() else { panic!("{unknown:?}") };
    assert_eq!((*id, tally.projects, tally.times), (151, 1, 2));
    assert!(census.unknown_bitwig_classes().iter().any(|c| c.0 == 0x2e));
    assert!(!census.unknown_bitwig_classes().iter().any(|c| c.0 == 0x728));
    assert!(!census.unknown_bitwig_fields().iter().any(|c| c.0 == 0x9a));

    let report = census.report(40);
    assert!(report.contains("Format census of 5 projects"), "{report}");
    assert!(report.contains("event 151  in 1 (33.3%)"), "{report}");
    assert!(report.contains("Broken.flp"), "{report}");
}

// ---- The command ----

fn run(args: &[&str]) -> (i32, String, String) {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = command::run(&args, &mut out, &mut err);
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

#[test]
fn runs_each_command() {
    let dir = common::test_dir("formatlab-command");
    let flp = write(&dir, "Night Drive.flp", &common::fl_studio_project());
    let bw = bitwig_project(&dir, "Loop Idea");
    let (flp, bw) = (flp.to_str().unwrap(), bw.to_str().unwrap());

    assert_eq!(run(&[]).0, 2);
    assert!(run(&["help"]).1.contains("formatlab tree"));

    let (code, tree, _) = run(&["tree", flp, "--depth", "1"]);
    assert_eq!(code, 0);
    assert!(tree.contains("156 tempo = 128000"), "{tree}");
    assert!(run(&["find", flp, "128000"]).1.contains("128000"));
    assert!(run(&["diff", flp, flp]).1.contains("No differences."));
    assert_eq!(run(&["diff", flp, bw]).0, 1);

    let (code, census, counting) = run(&["census", dir.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(census.contains("Format census of 2 projects"), "{census}");
    assert!(counting.contains("Counting 2 projects"), "{counting}");

    let fields = run(&["fields", "fl", "156"]).1;
    assert!(fields.contains("tempo") && fields.contains("how we know:"), "{fields}");
    assert!(run(&["fields", "bitwig"]).1.contains("VST3 plugin"));
}

#[test]
fn says_what_went_wrong() {
    let dir = common::test_dir("formatlab-wrong");
    let (code, _, err) = run(&["tree", dir.join("gone.flp").to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(err.starts_with("Format Lab:"), "{err}");
    assert_eq!(run(&["tree"]).0, 2);
    assert_eq!(run(&["fields", "studio one"]).0, 2);
    let (code, _, err) = run(&["explode"]);
    assert_eq!(code, 2);
    assert!(err.contains("there's no command \"explode\""), "{err}");
}

/// A Bitwig file whose metadata holds `meta` and a version.
fn bitwig_with_meta(meta: impl FnOnce(&mut common::bitwig::BwWriter)) -> Vec<u8> {
    common::bitwig::BwWriter::file(
        |m| {
            m.meta_entry("application_version_name", 0x08);
            m.str_val("6.1.3");
            meta(m);
        },
        |b| {
            b.begin(0x2e);
            b.end();
        },
        None,
    )
}

#[test]
fn documents_nested_deep_in_metadata_are_shown_as_bytes() {
    // Each document is a metadata blob inside the one before: 50 deep.
    let mut raw = bitwig_with_meta(|_| {});
    for _ in 0..50 {
        let inner = raw.clone();
        raw = bitwig_with_meta(|m| {
            m.meta_entry("structure", 0x0d);
            m.u32(inner.len() as u32);
            m.s.extend_from_slice(&inner);
        });
    }
    let f = LabFile::read(&raw, "nested.bwproject").unwrap();
    let documents = f.items.iter().filter(|i| i.value.ends_with("a Bitwig document>")).count();
    assert_eq!(documents, 4);
    assert!(f.items.iter().any(|i| matches!(i.data, Data::Bytes(_)) && i.label.starts_with("meta structure")));
}

#[test]
fn diff_sees_a_change_in_a_metadata_key_written_twice() {
    let file = |second: &str| {
        bitwig_with_meta(|m| {
            m.meta_entry("creator", 0x08);
            m.str_val("Solomon");
            m.meta_entry("creator", 0x08);
            m.str_val(second);
        })
    };
    let (before, after) =
        (LabFile::read(&file("Sol"), "a.bwproject").unwrap(), LabFile::read(&file("Sal"), "b.bwproject").unwrap());
    let changes = ops::diff(&before, &after);
    let [change] = changes.as_slice() else { panic!("{changes:?}") };
    assert_eq!((change.before.as_deref(), change.after.as_deref()), (Some("\"Sol\""), Some("\"Sal\"")));
    assert!(change.path.ends_with("#2"), "{}", change.path);
}
