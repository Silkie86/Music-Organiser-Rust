//! Bitwig's file format: decoding ends at the zip, and encoding writes a decoded file back byte for byte, edited or
//! not. The C# `BitwigWriterTests`, on the same made-up files.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::bitwig::*;
use mo_formats::bitwig::{self, Document, ObjId, Slot, Value};

fn decode(raw: &[u8]) -> Document {
    bitwig::decode(raw.to_vec()).expect("a Bitwig file")
}

fn root(doc: &Document) -> ObjId {
    doc.root.expect("decoded")
}

/// The project group (field 0x4dd of the root).
fn group(doc: &Document) -> ObjId {
    doc.field_obj(root(doc), 0x4dd).expect("the project group")
}

fn names(doc: &Document, of: ObjId, field: u32) -> Vec<String> {
    doc.field_list(of, field).iter().map(|&d| doc.obj(d).str(0x9a).to_owned()).collect()
}

fn list_mut(doc: &mut Document, of: ObjId, field: u32) -> &mut Vec<Slot> {
    match doc.obj_mut(of).fields.iter_mut().find(|f| f.id == field).map(|f| &mut f.value) {
        Some(Value::List(l)) => l,
        _ => panic!("no list in field {field:x}"),
    }
}

#[test]
fn an_untouched_project_writes_back_byte_for_byte() {
    for raw in [bitwig_project_bytes(), bitwig_template(Template::default())] {
        let doc = decode(&raw);
        assert_eq!(doc.body_error, None);
        assert_eq!(bitwig::encode(&doc, None).expect("written"), raw);
    }
}

#[test]
fn every_template_variant_writes_back_byte_for_byte() {
    for (a, b, c, d) in [(false, false, false, false), (true, false, true, true), (false, true, false, true)] {
        let t = Template { automation_lane: a, audio_track: b, device_power: c, master_plugin: d };
        let raw = bitwig_template(t);
        let doc = decode(&raw);
        assert_eq!(doc.body_error, None);
        assert_eq!(bitwig::encode(&doc, None).expect("written"), raw);
    }
}

#[test]
fn decoding_ends_at_the_zip_and_reads_the_metadata() {
    let doc = decode(&bitwig_project_bytes());
    assert_eq!(doc.meta_str("application_version_name"), "5.2.4");
    assert_eq!(doc.meta_strings("used_files"), ["samples/Kick 01.wav", "multi-samples/Pad.multisample"]);
    assert!(doc.zip().is_some_and(|z| z.starts_with(b"PK")));
    let r = root(&doc);
    assert_eq!(doc.obj(r).str(0x44), "Loop Idea");
    assert_eq!(doc.field_obj(r, 0x29a6).and_then(|t| doc.obj(t).num(0x2c8)), Some(128.5));
    // A UTF-16 string and an int array, as Delay+ has them.
    let track = doc.field_list(group(&doc), 0x4de)[0];
    let delay = doc.field_list(track, 0x4ba)[1];
    assert_eq!(doc.obj(delay).str(0x133b), "Delay » Feedback");
    assert_eq!(doc.obj(delay).get(0x14e1), Some(&Value::Ints(vec![0, 1])));
}

#[test]
fn the_structure_summary_is_a_document_of_its_own() {
    let doc = decode(&bitwig_project_bytes());
    let Some(Value::Blob(b)) = doc.meta("structure") else { panic!("no structure") };
    let inner = decode(b.bytes());
    assert_eq!(inner.body_error, None);
    let project = inner.field_obj(root(&inner), 0xad2).expect("the project");
    let tracks = inner.field_list(project, 0xace);
    assert_eq!(inner.obj(tracks[0]).str(0xad1), "Keys Display");
    assert_eq!(bitwig::encode(&inner, None).expect("written"), b.bytes());
}

#[test]
fn a_file_cut_short_says_where_it_stopped() {
    let raw = bitwig_project_bytes();
    let doc = decode(&raw[..raw.len() / 2]);
    assert!(doc.body_error.is_some());
    assert!(bitwig::encode(&doc, None).is_err());
    assert_eq!(
        bitwig::decode(b"not a Bitwig file at all, but long enough to be".to_vec()).err(),
        Some(bitwig::NotBitwig)
    );
}

#[test]
fn a_body_that_ends_before_the_zip_isnt_trusted() {
    // Rule 8: one byte of padding between the body and the zip, and the decode no longer ends where it should.
    let raw = bitwig_project_bytes();
    let doc = decode(&raw);
    let zip = doc.zip_offset;
    let mut longer = raw[..zip].to_vec();
    longer.push(0);
    longer.extend_from_slice(&raw[zip..]);
    let hex = format!("{:016x}", zip + 1);
    longer[4 + 20..4 + 36].copy_from_slice(hex.as_bytes());
    let doc = decode(&longer);
    assert_eq!(doc.body_error, Some(format!("body ended at 0x{zip:x}, expected 0x{:x}", zip + 1)));
}

#[test]
fn reads_and_writes_lists_of_floats() {
    // FM-4 keeps its modulation matrix as a counted list of floats (type 0x17).
    let raw = BwWriter::file(
        |_| {},
        |b| {
            b.begin(0x2e);
            b.obj(0xa4);
            b.begin(0x3c);
            b.str(0x2b9, "MATRIX");
            b.floats(0xe75, &[0.43, 0.0, 0.35]);
            b.end();
            b.str(0x15b, "after");
            b.end();
        },
        None,
    );
    let doc = decode(&raw);
    assert_eq!(doc.body_error, None);
    let matrix = doc.field_obj(root(&doc), 0xa4).expect("matrix");
    assert_eq!(doc.obj(matrix).get(0xe75), Some(&Value::Floats(vec![0.43, 0.0, 0.35])));
    assert_eq!(doc.obj(root(&doc)).str(0x15b), "after");
    assert_eq!(bitwig::encode(&doc, None).expect("written"), raw);
}

#[test]
fn reads_and_writes_lists_of_doubles() {
    // Bitwig 6.1 keeps a Sampler sample's analysis as counted lists of doubles (type 0x18). Not knowing the type
    // stopped Solomon's "analog full set" from being read at all.
    let raw = BwWriter::file(
        |_| {},
        |b| {
            b.begin(0x2e);
            b.obj(0x40fe);
            b.begin(0x15df);
            b.doubles(0x40fd, &[28.074285715, 31.502857142, 34.931519273]);
            b.end();
            b.str(0x15b, "after");
            b.end();
        },
        None,
    );
    let doc = decode(&raw);
    assert_eq!(doc.body_error, None);
    let analysis = doc.field_obj(root(&doc), 0x40fe).expect("analysis");
    assert_eq!(doc.obj(analysis).get(0x40fd), Some(&Value::Doubles(vec![28.074285715, 31.502857142, 34.931519273])));
    assert_eq!(bitwig::encode(&doc, None).expect("written"), raw);
}

#[test]
fn text_that_isnt_valid_comes_back_as_it_was() {
    let raw = BwWriter::file(
        |_| {},
        |b| {
            b.begin(0x2e);
            // Bad UTF-8, then a lone UTF-16 surrogate.
            b.u32(0x15b);
            b.s.push(0x08);
            b.u32(2);
            b.s.extend_from_slice(&[0xff, 0xfe]);
            b.u32(0x15c);
            b.s.push(0x08);
            b.u32(0x8000_0001);
            b.s.extend_from_slice(&[0xd8, 0x00]);
            b.end();
        },
        None,
    );
    let doc = decode(&raw);
    assert_eq!(doc.body_error, None);
    assert_eq!(bitwig::encode(&doc, None).expect("written"), raw);
}

#[test]
fn edits_keep_references_pointing_at_the_right_objects() {
    let mut doc = decode(&bitwig_project_bytes());
    let g = group(&doc);
    let track = doc.field_list(g, 0x4de)[0];
    // Take Delay+ out of the first track. The master is written after it, so every reference to the master has to be
    // renumbered.
    let delay: Vec<ObjId> = doc.field_list(track, 0x4ba).into_iter().filter(|&d| doc.obj(d).class == 0x40).collect();
    list_mut(&mut doc, track, 0x4ba).retain(|s| !matches!(s, Slot::Obj(d) if delay.contains(d)));
    doc.obj_mut(track).set(0x15b, Value::Str(bitwig::Text::new("Keys from FL")));

    let back = decode(&bitwig::encode(&doc, None).expect("written"));
    assert_eq!(back.body_error, None);
    let bg = group(&back);
    let back_track = back.field_list(bg, 0x4de)[0];
    assert_eq!(back.obj(back_track).str(0x15b), "Keys from FL");
    assert_eq!(names(&back, back_track, 0x4ba), ["Keyscape"]);
    let master = back.field_obj(bg, 0x4e0).expect("the master");
    assert_eq!(names(&back, master, 0x4ba), ["FabFilter Pro-L 2"]);
    assert_eq!(back.objects.len(), doc.objects.len() - 1);
    assert_eq!(back.zip(), doc.zip());
}

#[test]
fn a_field_can_point_at_another_object_by_identity() {
    let mut doc = decode(&bitwig_project_bytes());
    let g = group(&doc);
    let fx = doc.field_list(g, 0x4df)[0];
    // Point the project's master slot at the effect track instead.
    doc.obj_mut(g).set(0x4e0, Value::Obj(Slot::Obj(fx)));

    let back = decode(&bitwig::encode(&doc, None).expect("written"));
    let bg = group(&back);
    assert_eq!(back.field_obj(bg, 0x4e0), Some(back.field_list(bg, 0x4df)[0]));
}

#[test]
fn a_field_pointing_at_an_object_written_earlier_is_a_bare_reference() {
    // Two automation lanes on one plugin both point at it; the second is written after the plugin (rule 8: never a
    // class 1 reference in a field that holds one object).
    let mut doc = decode(&bitwig_project_bytes());
    let g = group(&doc);
    let fx = doc.field_list(g, 0x4df)[0];
    doc.obj_mut(g).set(0x4e0, Value::Obj(Slot::Obj(fx)));
    let fields = &doc.obj(g).fields;
    assert!(fields.iter().position(|f| f.id == 0x4df) < fields.iter().position(|f| f.id == 0x4e0));

    let back = decode(&bitwig::encode(&doc, None).expect("written"));
    assert_eq!(back.body_error, None);
    let bg = group(&back);
    assert_eq!(back.field_obj(bg, 0x4e0), Some(back.field_list(bg, 0x4df)[0]));
    assert!(matches!(back.obj(bg).get(0x4e0), Some(Value::BareRef(_))));
}

#[test]
fn a_copied_track_shares_the_master_it_routes_to_and_gets_its_own_ids() {
    let mut doc = decode(&bitwig_project_bytes());
    let g = group(&doc);
    let first = doc.field_list(g, 0x4de)[0];
    let copy = bitwig::clone_object(&mut doc, first);
    doc.obj_mut(copy).set(0x15b, Value::Str(bitwig::Text::new("Copy")));
    list_mut(&mut doc, g, 0x4de).push(Slot::Obj(copy));

    let back = decode(&bitwig::encode(&doc, None).expect("written"));
    assert_eq!(back.body_error, None);
    let bg = group(&back);
    let tracks = back.field_list(bg, 0x4de);
    let (a, b) = (tracks[0], tracks[tracks.len() - 1]);
    assert_eq!(back.obj(b).str(0x15b), "Copy");
    assert_eq!(names(&back, b, 0x4ba), ["Keyscape", "Delay+"]);
    assert_ne!(back.obj(a).get(0xf), back.obj(b).get(0xf));
    assert_ne!(back.field_list(a, 0x4ba)[0], back.field_list(b, 0x4ba)[0]);

    // The master is written once, inside the first track, and both tracks point at it.
    let master = back.field_obj(bg, 0x4e0).expect("the master");
    assert_eq!(back.field_obj(a, 0x2bb0), Some(master));
    assert_eq!(back.field_obj(b, 0x2bb0), Some(master));
    let limiters = (0..back.objects.len() as u32)
        .filter(|&i| names(&back, ObjId(i), 0x4ba).contains(&"FabFilter Pro-L 2".to_owned()))
        .count();
    assert_eq!(limiters, 1);
}

#[test]
fn an_object_is_written_where_its_first_reached() {
    let mut doc = decode(&bitwig_project_bytes());
    let g = group(&doc);
    // Drop the first track, which is where the master was written: it moves to the project's master slot.
    list_mut(&mut doc, g, 0x4de).remove(0);

    let back = decode(&bitwig::encode(&doc, None).expect("written"));
    assert_eq!(back.body_error, None);
    let bg = group(&back);
    let master = back.field_obj(bg, 0x4e0).expect("the master");
    assert_eq!(names(&back, master, 0x4ba), ["FabFilter Pro-L 2"]);
    assert!(back.field_list(bg, 0x4de).is_empty());
}

#[test]
fn a_new_zip_replaces_the_old_one() {
    let doc = decode(&bitwig_project_bytes());
    let zip = stored_zip(&[("plugin-states/new.vstpreset", b"new")]);
    let back = decode(&bitwig::encode(&doc, Some(&zip)).expect("written"));
    assert_eq!(back.body_error, None);
    assert_eq!(back.zip(), Some(&zip[..]));
}

#[test]
fn damaged_files_never_stop_the_reader() {
    // Every byte of the template changed in turn (a cheap stand-in for a damaged save): each decodes to a document or
    // an error, never a crash, and one that decodes cleanly writes back to a file that decodes cleanly too.
    let raw = bitwig_template(Template::default());
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    for at in 4..raw.len() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let mut bad = raw.clone();
        bad[at] ^= (seed as u8) | 1;
        let Ok(doc) = bitwig::decode(bad) else { continue };
        if doc.body_error.is_none()
            && let Ok(again) = bitwig::encode(&doc, None)
        {
            assert_eq!(decode(&again).body_error, None, "byte {at}");
        }
    }
}

/// A file of `depth` objects, each in a field of the one before.
fn nested(depth: usize) -> Vec<u8> {
    BwWriter::file(
        |_| {},
        |b| {
            for _ in 1..depth {
                b.begin(0x2e);
                b.obj(0x10);
            }
            b.begin(0x2e);
            for _ in 0..depth {
                b.end();
            }
        },
        None,
    )
}

#[test]
fn deeply_nested_files_never_run_out_of_stack() {
    // On a thread with a quarter of the Windows main thread's 1 MB of stack, in a debug build: as deep as the reader
    // goes reads, copies and writes back, and deeper is an error, not a crash. (Reading, copying and writing keep
    // their place on the heap, so the stack a thread has doesn't matter; macOS's frames are bigger than Linux's.)
    let worker = std::thread::Builder::new().stack_size(1 << 18).spawn(|| {
        let raw = nested(512);
        let mut doc = decode(&raw);
        assert_eq!(doc.body_error, None);
        assert_eq!(bitwig::encode(&doc, None).expect("written"), raw);
        let root = doc.root.expect("a root");
        let Value::Obj(inside) = doc.obj(root).fields[0].value else { panic!("an object inside") };
        let inside = doc.resolve(inside).expect("there");
        let copy = bitwig::clone_object(&mut doc, inside);
        let mut depth = 1;
        let mut at = copy;
        while let Some(Value::Obj(s)) = doc.obj(at).fields.first().map(|f| f.value.clone()) {
            at = doc.resolve(s).expect("copied");
            depth += 1;
        }
        assert_eq!(depth, 511);
        assert_eq!(decode(&nested(513)).body_error.as_deref(), Some("objects nested too deeply"));
        assert_eq!(decode(&nested(100_000)).body_error.as_deref(), Some("objects nested too deeply"));
    });
    worker.expect("a thread").join().expect("no stack overflow");
}

#[test]
fn a_reference_to_an_object_not_read_yet_isnt_trusted() {
    let bare = BwWriter::file(
        |_| {},
        |b| {
            b.begin(0x2e);
            b.bare_ref(0x10, 99);
            b.end();
        },
        None,
    );
    assert!(decode(&bare).body_error.is_some_and(|e| e.contains("to object 99")));
    let zero = BwWriter::file(
        |_| {},
        |b| {
            b.begin(0x2e);
            b.list(0x10);
            b.list_ref(0);
            b.end_list();
            b.end();
        },
        None,
    );
    assert!(decode(&zero).body_error.is_some_and(|e| e.contains("to object 0")));
}

#[test]
fn metadata_read_before_a_damaged_entry_stays() {
    let raw = BwWriter::file(
        |m| {
            m.meta_entry("application_version_name", 0x08);
            m.str_val("5.2.4");
            m.meta_entry("broken", 0x99);
        },
        |b| {
            b.begin(0x2e);
            b.end();
        },
        None,
    );
    let doc = decode(&raw);
    assert!(doc.body_error.as_ref().is_some_and(|e| e.starts_with("metadata: unknown value type 0x99")));
    assert_eq!(doc.meta_str("application_version_name"), "5.2.4");
}

#[test]
fn a_changed_number_keeps_the_kind_bitwig_wrote() {
    let mut doc = decode(&bitwig_template(Template::default()));
    let r = root(&doc);
    let tempo = doc.field_obj(r, 0x29a6).expect("tempo");
    doc.obj_mut(tempo).set(0x2c8, Value::I32(140));
    assert_eq!(doc.obj(tempo).get(0x2c8), Some(&Value::F64(140.0)));
    let back = decode(&bitwig::encode(&doc, None).expect("written"));
    let tempo = back.field_obj(root(&back), 0x29a6).expect("tempo");
    assert_eq!(back.obj(tempo).num(0x2c8), Some(140.0));
}

#[test]
fn a_copy_knows_what_its_parts_are_inside() {
    let mut doc = decode(&bitwig_project_bytes());
    let track = doc.field_list(group(&doc), 0x4de)[0];
    let copy = bitwig::clone_object(&mut doc, track);
    let device = doc.field_list(copy, 0x4ba)[0];
    assert_eq!(doc.obj(device).str(0x9a), "Keyscape");
    assert_eq!(doc.ancestor(device, |_, o| o.class == 0x90), Some(copy));
}

#[test]
#[should_panic(expected = "copying an object the rest of the document still reaches")]
fn copying_a_track_still_in_the_tree_needs_it_named() {
    let mut doc = decode(&bitwig_project_bytes());
    let track = doc.field_list(group(&doc), 0x4de)[0];
    bitwig::Copier::new(&doc, None).copy(&mut doc, track);
}
