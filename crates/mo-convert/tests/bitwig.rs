//! Writing a plan as a Bitwig project: the C# `BitwigProjectTests` that check Bitwig's objects, on the same made-up
//! template (built in mo-formats' tests, shared from there rather than copied).

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
#[path = "../../mo-formats/tests/common/bitwig.rs"]
mod fixtures;

use std::io::Read;

use fixtures::{Template, bitwig_template};
use mo_convert::bitwig::{template_problem, write};
use mo_convert::{Options, SampleFile, from_fl};
use mo_formats::bitwig::{Document, ObjId, Value, decode};
use mo_formats::fl::automation::{FlAutomation, FlControl, FlPoint};
use mo_formats::fl::song::{FlAudioClip, FlChannel, FlClip, FlInsert, FlPlugin, FlSong, FlStretch};
use mo_formats::presets::PresetBlob;
use mo_formats::{NoteAttribute, NoteEvent, PluginKind};

const MD5: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

fn snare(path: &str) -> Option<SampleFile> {
    path.ends_with("Snare 01.wav").then_some(SampleFile {
        bytes: 1234,
        md5: MD5,
        rate: 48000,
        channels: 2,
        seconds: 1.5,
    })
}

fn note(start: i64, length: i64, key: u8) -> NoteEvent {
    let velocity = mo_formats::velocity::from_unit(100.0 / 128.0);
    NoteEvent { start, length, key, velocity, muted: false, step: false, attribute: NoteAttribute::NONE }
}

/// A VST2 chunk program (.fxp) holding `chunk`.
fn fxp(chunk: &[u8]) -> Vec<u8> {
    let mut f = b"CcnK\0\0\0\0FPCh\0\0\0\x01XfsX\0\0\0\x01".to_vec();
    f.resize(56, 0);
    f.extend_from_slice(&(chunk.len() as u32).to_be_bytes());
    f.extend_from_slice(chunk);
    f
}

fn serum() -> FlPlugin {
    FlPlugin {
        name: "Serum".into(),
        vendor: "Xfer Records".into(),
        format: "VST2",
        device_id: "1483109208".into(),
        path: r"C:\VstPlugins\Serum_x64.dll".into(),
        nothing_saved: false,
        state: Some(PresetBlob {
            name: "Serum".into(),
            plugin: "Serum".into(),
            extension: ".fxp".into(),
            data: fxp(b"serum sound"),
            kind: PluginKind::Unknown,
            native: false,
            place: String::new(),
            channel: None,
            program: 0,
        }),
        parameter_ids: None,
        slot: None,
        enabled: true,
    }
}

fn channel(index: u32, name: &str, instrument: Option<FlPlugin>, sample: &str) -> FlChannel {
    FlChannel {
        index,
        name: name.into(),
        instrument,
        sample: sample.into(),
        insert: 0,
        notes: vec![note(0, 96, 60)],
        root_key: 60,
        pitch_cents: 0,
        volume: 0.78125,
        pan: 0.5,
        clips: vec![FlClip {
            name: "Pattern 1".into(),
            start: 384,
            length: 384,
            notes: vec![note(0, 96, 60)],
            muted: false,
            track: 1,
            on_muted_track: false,
        }],
        volume_envelope: None,
        audio_clips: Vec::new(),
        stretch: FlStretch::default(),
        enabled: true,
        switched_by: Vec::new(),
    }
}

fn automation(name: &str, channel: u32, control: FlControl, parameter: u32, points: &[(f64, f64)]) -> FlAutomation {
    FlAutomation {
        name: name.into(),
        channel: Some(channel),
        control,
        parameter,
        points: points.iter().map(|&(beat, value)| FlPoint { beat, value }).collect(),
        insert: None,
        slot: None,
        starts: vec![0.0],
    }
}

/// The C# test's song: a snare on a Sampler (root A4, pitch -2, volume 100%, pan 25%, its volume automated) and Serum
/// with its root key on E5 and its cutoff automated.
fn song() -> FlSong {
    let mut snare = channel(0, "Snare", None, r"C:\Samples\Snare 01.wav");
    (snare.root_key, snare.pitch_cents, snare.volume, snare.pan) = (57, -200, 1.0, 0.25);
    let mut synth = channel(1, "Serum", Some(serum()), "");
    synth.root_key = 64;
    FlSong {
        title: "Test".into(),
        ppq: 96,
        tempo: 128.0,
        channels: vec![snare, synth],
        inserts: Vec::new(),
        automation: vec![
            automation("Snare volume", 0, FlControl::Volume, 0, &[(0.0, 0.5), (4.0, 1.0)]),
            automation("Serum - Cutoff", 1, FlControl::PluginParameter, 205, &[(0.0, 0.2), (2.0, 1.0), (4.0, 0.5)]),
        ],
        muted_tracks: Vec::new(),
        curves: Default::default(),
        warning: None,
    }
}

fn convert(song: &FlSong, template: &[u8]) -> (Document, Vec<String>, Vec<u8>) {
    let plan = from_fl(song, Options::default(), &mut snare);
    let made = write(&plan, template).expect("written");
    let doc = decode(made.bytes.clone()).expect("a Bitwig file");
    assert_eq!(doc.body_error, None);
    (doc, made.notes, made.bytes)
}

fn obj(doc: &Document, o: ObjId, path: &[u32]) -> ObjId {
    path.iter().fold(o, |o, &f| doc.field_obj(o, f).unwrap_or_else(|| panic!("no field {f:#x}")))
}

fn chain(doc: &Document, track: ObjId) -> Vec<ObjId> {
    doc.field_list(obj(doc, track, &[0x164]), 0x144)
}

fn def(doc: &Document, device: ObjId) -> ObjId {
    obj(doc, device, &[0x197])
}

fn param(doc: &Document, device: ObjId, name: &str) -> ObjId {
    let contents = obj(doc, def(doc, device), &[0xa4]);
    doc.field_list(contents, 0x20c).into_iter().find(|&p| doc.obj(p).str(0x2b9) == name).expect("the setting")
}

fn tracks(doc: &Document) -> (ObjId, Vec<ObjId>) {
    let group = obj(doc, doc.root.unwrap(), &[0x4dd]);
    (group, doc.field_list(group, 0x4de))
}

fn reachable(doc: &Document) -> Vec<ObjId> {
    let mut seen = std::collections::HashSet::new();
    let mut todo = vec![doc.root.unwrap()];
    while let Some(o) = todo.pop() {
        if seen.insert(o) {
            todo.extend(doc.obj(o).fields.iter().flat_map(|f| f.value.targets()).filter_map(|s| doc.resolve(s)));
        }
    }
    seen.into_iter().collect()
}

fn zip_names(doc: &Document) -> Vec<String> {
    let Some(raw) = doc.zip() else { return Vec::new() };
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(raw)).unwrap();
    (0..z.len()).map(|i| z.by_index(i).unwrap().name().unwrap().to_string()).collect()
}

#[test]
fn builds_tracks_from_the_template_with_sampler_root_keys_and_transposed_plugins() {
    let (doc, notes, _) = convert(&song(), &bitwig_template(Template::default()));
    let (group, tracks) = tracks(&doc);
    assert_eq!(tracks.iter().map(|&t| doc.obj(t).str(0x15b)).collect::<Vec<_>>(), ["Snare", "Serum"]);
    assert!(doc.field_list(group, 0x4df).is_empty());

    // The Sampler: its sample (with what Bitwig checks it by), root key A4 (57) and FL's -2 semitone pitch.
    let sampler = chain(&doc, tracks[0]);
    assert_eq!(sampler.len(), 1, "the Amp is gone");
    let sampler = sampler[0];
    assert_eq!(doc.obj(def(&doc, sampler)).str(0x12de), "Snare 01");
    assert_eq!(doc.obj(param(&doc, sampler, "TRANSPOSE")).num(0x136), Some(-2.0));
    let zone = obj(&doc, param(&doc, sampler, "SAMPLE"), &[0x74c]);
    assert_eq!((doc.obj(zone).num(0x75c), doc.obj(zone).num(0x761)), (Some(57.0), Some(1.5)));
    let sample = obj(&doc, zone, &[0x4112, 0x748]);
    let s = doc.obj(sample);
    assert_eq!(
        (s.str(0x129f), s.num(0x94), s.num(0x95), s.num(0x96)),
        ("Snare 01.wav", Some(48000.0), Some(1.5), Some(2.0))
    );
    let file = obj(&doc, sample, &[0x129e]);
    assert_eq!(doc.obj(obj(&doc, file, &[0xd3b])).str(0x3b), r"C:\Samples\Snare 01.wav");
    assert_eq!(doc.obj(file).num(0x17), Some(1234.0));
    match doc.obj(obj(&doc, file, &[0x513])).get(0x16) {
        Some(Value::Blob(b)) => assert_eq!(b.bytes(), MD5),
        v => panic!("an MD5, not {v:?}"),
    }

    // Volume 100% and pan 25% on the mixer: Bitwig's fader holds the cube root of the gain, pan runs -1..1.
    let mixer = obj(&doc, tracks[0], &[0x165]);
    let volume = doc.obj(obj(&doc, mixer, &[0x1a4])).num(0x2c8).unwrap();
    assert!((volume - mo_convert::plan::gain(1.0).cbrt()).abs() < 1e-9);
    assert_eq!(doc.obj(obj(&doc, mixer, &[0x1a5])).num(0x2c8), Some(-0.5));

    // Serum (VST2), root key E5: a Note Transpose of -4 semitones in front of it (octaves 0, stored as 3).
    let serum_chain = chain(&doc, tracks[1]);
    let (transpose, serum) = (serum_chain[0], *serum_chain.last().unwrap());
    assert_eq!(doc.obj(def(&doc, transpose)).str(0x9a), "Note Transpose");
    assert_eq!(doc.obj(param(&doc, transpose, "OCTAVES")).num(0x273), Some(3.0));
    assert_eq!(doc.obj(param(&doc, transpose, "COARSE")).num(0x330), Some(252.0), "-4 as a signed byte");
    let serum_def = def(&doc, serum);
    let d = doc.obj(serum_def);
    assert_eq!((d.str(0x9a), d.str(0x9b), d.num(0xce4)), ("Serum", "Xfer Records", Some(1_483_109_208.0)));
    let location = obj(&doc, serum_def, &[0xc5a]);
    let files = doc.field_list(location, 0xc5b);
    assert_eq!(files.len(), 1);
    assert_eq!(doc.obj(files[0]).str(0x3b), r"C:\VstPlugins\Serum_x64.dll");
    assert!(doc.field_list(obj(&doc, serum_def, &[0x137c]), 0x8e0).is_empty(), "Drumazon's outputs don't come along");
    let state = format!("plugin-states/{}", d.str(0xbe5));
    assert_eq!(zip_names(&doc), std::slice::from_ref(&state));
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(doc.zip().unwrap())).unwrap();
    let mut bank = Vec::new();
    z.by_name(&state).unwrap().read_to_end(&mut bank).unwrap();
    assert_eq!(&bank[8..12], b"FBCh");
    assert!(bank.ends_with(b"serum sound"));

    // A clip where the pattern sits in the playlist (bar 2, one bar long), its notes timed from the clip's start.
    let arrangement = |t: ObjId| doc.field_list(obj(&doc, t, &[0x2bf8, 0x2d8e]), 0x21f);
    let clip = arrangement(tracks[1]);
    assert_eq!(clip.len(), 1);
    let clip = clip[0];
    assert_eq!((doc.obj(clip).num(0x2af), doc.obj(clip).num(0x26)), (Some(4.0), Some(4.0)));
    let lanes = doc.field_list(obj(&doc, clip, &[0x2ae9, 0x10e9, 0x49c]), 0x18cb);
    assert_eq!(lanes.len(), 1);
    assert_eq!(doc.obj(lanes[0]).num(0xee), Some(60.0));
    let n = doc.field_list(lanes[0], 0x21f);
    let n = doc.obj(n[0]);
    assert_eq!((n.num(0x2af), n.num(0x26)), (Some(0.0), Some(1.0)));
    assert!((n.num(0xef).unwrap() - 100.0 / 128.0).abs() < 1e-4);
    let content = obj(&doc, clip, &[0x2ae9, 0x10e9]);
    assert_eq!(doc.field_list(obj(&doc, tracks[1], &[0x2aed]), 0x10e6), [content]);

    // The template's own tracks, the Amp, the effect track and its sends are gone; the master stays.
    let all = reachable(&doc);
    assert!(!all.iter().any(|&o| {
        let o = doc.obj(o);
        o.str(0x15b) == "FX 1" || ["Drumazon", "Amp", "Serum 2"].contains(&o.str(0x9a))
    }));
    assert!(
        !all.iter().any(|&o| doc.obj(o).str(0x2b9).starts_with("TRACK_SEND(") || doc.obj(o).str(0x2bd) == "AMOUNT")
    );
    let master = obj(&doc, group, &[0x4e0]);
    assert_eq!(doc.obj(master).str(0x15b), "Master");
    for &t in &tracks {
        assert_eq!(doc.field_obj(obj(&doc, t, &[0x897]), 0x89b), Some(master));
    }
    let tempo: Vec<_> = all.iter().filter(|&&o| doc.obj(o).str(0x2bd) == "TEMPO").collect();
    assert_eq!(tempo.len(), 1);
    assert_eq!(doc.obj(*tempo[0]).num(0x2c8), Some(128.0));

    // The snare's volume automation as a lane of points on its fader, in the fader's units; the template's
    // automation clips and points don't come along.
    let lanes = doc.field_list(obj(&doc, tracks[0], &[0x2dc9]), 0x4ba);
    assert_eq!(lanes.len(), 1);
    let target = obj(&doc, lanes[0], &[0x2a3]);
    assert_eq!(doc.field_obj(target, 0x41), Some(obj(&doc, mixer, &[0x1a4])));
    assert_eq!((doc.obj(target).str(0x133b), doc.obj(target).str(0x133a)), ("Mixer", "Volume"));
    assert!(doc.field_list(obj(&doc, lanes[0], &[0x2d8c]), 0x21f).is_empty());
    let points: Vec<_> = doc
        .field_list(obj(&doc, lanes[0], &[0x2d8c, 0x35ac]), 0x21f)
        .iter()
        .map(|&p| {
            (
                doc.obj(p).num(0x2af).unwrap(),
                doc.obj(p).num(0x28f).unwrap(),
                doc.obj(p).num(0x359e),
                doc.obj(p).num(0xba6),
            )
        })
        .collect();
    let gain = |v: f64| mo_convert::plan::gain(v).cbrt();
    assert_eq!(points, [(0.0, gain(0.5), Some(1.0), Some(0.0)), (4.0, gain(1.0), Some(1.0), Some(0.0))]);

    // Serum's cutoff (VST2 setting 205) on a lane for that setting of the plugin, which lists it as automated.
    let lanes = doc.field_list(obj(&doc, tracks[1], &[0x2dc9]), 0x4ba);
    assert_eq!(lanes.len(), 1);
    let target = obj(&doc, lanes[0], &[0x2a3]);
    assert_eq!(doc.field_obj(target, 0xec), Some(serum));
    let t = doc.obj(target);
    assert_eq!(
        (t.str(0xed), t.str(0x133b), t.str(0x133a)),
        ("CONTENTS/ROOT_GENERIC_MODULE/PARAM205", "Serum", "Cutoff")
    );
    let points: Vec<_> = doc
        .field_list(obj(&doc, lanes[0], &[0x2d8c, 0x35ac]), 0x21f)
        .iter()
        .map(|&p| (doc.obj(p).num(0x2af).unwrap(), doc.obj(p).num(0x28f).unwrap()))
        .collect();
    assert_eq!(points, [(0.0, 0.2), (2.0, 1.0), (4.0, 0.5)]);
    let listed = doc.field_list(serum_def, 0x1318);
    assert_eq!(listed.len(), 1);
    assert_eq!((doc.obj(listed[0]).num(0x1319), doc.obj(listed[0]).num(0x131a)), (Some(205.0), Some(0.2)));
    assert!(!notes.iter().any(|n| n.contains("automation")), "{notes:?}");

    // The browser's summary lists the new tracks.
    let Some(Value::Blob(summary)) = doc.meta("structure") else { panic!("a summary") };
    let summary = decode(summary.bytes()).unwrap();
    let project = obj(&summary, summary.root.unwrap(), &[0xad2]);
    let names: Vec<_> =
        summary.field_list(project, 0xace).iter().map(|&e| summary.obj(e).str(0xad1).to_owned()).collect();
    assert_eq!(names, ["Snare", "Serum"]);
}

#[test]
fn a_plugin_on_the_templates_master_keeps_its_settings() {
    let master_plugin = Template { master_plugin: true, ..Template::default() };
    let (doc, _, _) = convert(&song(), &bitwig_template(master_plugin));
    let names = zip_names(&doc);
    assert!(names.contains(&"plugin-states/limiter.vstpreset".to_owned()), "{names:?}");
    assert_eq!(names.len(), 2);
}

#[test]
fn an_inserts_effects_go_after_the_instrument_and_the_master_inserts_on_the_master() {
    let effect = |name: &str, format: &'static str, id: &str, slot| FlPlugin {
        name: name.into(),
        device_id: id.into(),
        format,
        slot: Some(slot),
        ..serum()
    };
    let mut s = song();
    s.channels.remove(0);
    s.automation.clear();
    s.channels[0].insert = 3;
    let off = FlPlugin { enabled: false, ..effect("Pro-L 2", "VST2", "1181700674", 0) };
    s.inserts = vec![
        FlInsert { index: 0, name: String::new(), effects: vec![off], fader: None, pan: None },
        FlInsert {
            index: 3,
            name: "Keys bus".into(),
            effects: vec![effect("Pro-Q 3", "VST3", "72C4DB717A4D459AB97E51745D84B39D", 0)],
            fader: None,
            pan: None,
        },
    ];
    let (doc, _, _) = convert(&s, &bitwig_template(Template::default()));
    let (group, tracks) = tracks(&doc);
    let names: Vec<_> = chain(&doc, tracks[0]).iter().map(|&d| doc.obj(def(&doc, d)).str(0x9a).to_owned()).collect();
    assert_eq!(names, ["Note Transpose", "Serum", "Pro-Q 3"]);
    let eq = doc.obj(def(&doc, *chain(&doc, tracks[0]).last().unwrap()));
    assert_eq!((eq.num(0x9d), eq.str(0x9c), eq.num(0x1809)), (Some(0.0), "Audio FX", Some(0.0)));
    assert_eq!((eq.str(0xce4), eq.str(0xc5e)), ("72C4DB717A4D459AB97E51745D84B39D", ""), "not Serum 2's version");
    let master = chain(&doc, obj(&doc, group, &[0x4e0]));
    assert_eq!(master.len(), 1);
    let limiter = doc.obj(def(&doc, master[0]));
    assert_eq!((limiter.str(0x9a), limiter.num(0xce4)), ("Pro-L 2", Some(1_181_700_674.0)));
    assert_eq!(limiter.num(0xa3), Some(0.0), "switched off, as in FL");
}

#[test]
fn an_effect_slot_switched_on_and_off_is_an_on_off_lane_of_switch_points() {
    let mut s = song();
    s.channels.remove(0);
    s.channels[0].insert = 3;
    let eq = FlPlugin { name: "Pro-Q 3".into(), device_id: "1".into(), slot: Some(2), ..serum() };
    s.inserts = vec![FlInsert { index: 3, name: "Keys bus".into(), effects: vec![eq], fader: None, pan: None }];
    s.automation = vec![FlAutomation {
        channel: None,
        insert: Some(3),
        slot: Some(2),
        ..automation(
            "EQ on",
            0,
            FlControl::InsertSlotOnOff,
            0,
            &[
                (0.0, 1.0),
                (2.0, 0.0),
                (3.0, 0.0),
                (4.0, 1.0),
                // Off and on again on one beat, where two placements of a clip meet: no click.
                (6.0, 1.0),
                (6.0, 0.0),
                (6.0, 1.0),
                (8.0, 1.0),
                (8.0, 0.0),
            ],
        )
    }];
    let (doc, notes, _) = convert(&s, &bitwig_template(Template::default()));
    assert!(!notes.iter().any(|n| n.contains("automation")), "{notes:?}");
    let (_, tracks) = tracks(&doc);
    let eq = *chain(&doc, tracks[0]).last().unwrap();
    let lanes = doc.field_list(obj(&doc, tracks[0], &[0x2dc9]), 0x4ba);
    assert_eq!(lanes.len(), 1);
    // As Bitwig 6.1.6 saves a device's On/Off lane.
    let target = obj(&doc, lanes[0], &[0x2a3]);
    assert_eq!(doc.field_obj(target, 0xec), Some(eq));
    let t = doc.obj(target);
    assert_eq!((t.str(0xed), t.str(0x133b), t.str(0x133a)), ("On", "Pro-Q 3", "On/Off"));
    let switch = doc.obj(obj(&doc, target, &[0xe92]));
    assert_eq!((switch.class, switch.get(0x1b2d)), (0xc6, Some(&Value::Bool(1))));
    let list = obj(&doc, lanes[0], &[0x2d8c, 0x35ac]);
    assert_eq!(doc.obj(list).class, 0xa4);
    // One point at each change.
    let points: Vec<_> = doc
        .field_list(list, 0x21f)
        .iter()
        .map(|&p| (doc.obj(p).class, doc.obj(p).num(0x2af).unwrap(), doc.obj(p).get(0x12e).cloned()))
        .collect();
    assert_eq!(
        points,
        [
            (0x7d, 0.0, Some(Value::Bool(1))),
            (0x7d, 2.0, Some(Value::Bool(0))),
            (0x7d, 4.0, Some(Value::Bool(1))),
            (0x7d, 8.0, Some(Value::Bool(0)))
        ]
    );
    // The setting isn't listed on the plugin as a knob of its own.
    assert!(doc.field_list(def(&doc, eq), 0x1318).is_empty());
}

#[test]
fn an_insert_fader_is_a_volume_lane_and_the_master_inserts_is_on_the_master() {
    let mut s = song();
    s.channels.remove(0);
    s.channels[0].insert = 3;
    s.inserts = vec![FlInsert { index: 3, name: "Keys bus".into(), effects: vec![], fader: None, pan: None }];
    let fader = |name: &str, insert| FlAutomation {
        channel: None,
        insert: Some(insert),
        ..automation(name, 0, FlControl::InsertVolume, 0, &[(0.0, 0.8), (4.0, 0.5)])
    };
    s.automation = vec![fader("Bus fader", 3), fader("Master fader", 0)];
    let (doc, notes, _) = convert(&s, &bitwig_template(Template::default()));
    let (group, tracks) = tracks(&doc);
    let gains = |track: ObjId| -> Vec<f64> {
        let lanes = doc.field_list(obj(&doc, track, &[0x2dc9]), 0x4ba);
        assert_eq!(lanes.len(), 1);
        let target = obj(&doc, lanes[0], &[0x2a3]);
        assert_eq!(doc.field_obj(target, 0x41), Some(obj(&doc, track, &[0x165, 0x1a4])));
        doc.field_list(obj(&doc, lanes[0], &[0x2d8c, 0x35ac]), 0x21f)
            .iter()
            .map(|&p| doc.obj(p).num(0x28f).unwrap())
            .collect()
    };
    // From 0.8 to 0.5 in steps of 0.05 on FL's curve, as Bitwig draws a straight line on its own scale.
    let want: Vec<f64> = (0..=6).map(|i| mo_convert::plan::insert_gain(0.8 - 0.05 * i as f64).cbrt()).collect();
    assert!((want[0] - 1.0).abs() < 1e-12);
    for (got, track) in [(gains(tracks[0]), "Serum"), (gains(obj(&doc, group, &[0x4e0])), "master")] {
        assert_eq!(got.len(), want.len(), "{track}: {got:?}");
        assert!(got.iter().zip(&want).all(|(g, w)| (g - w).abs() < 1e-12), "{track}: {got:?}");
    }
    assert!(!notes.iter().any(|n| n.contains("automation")), "{notes:?}");
}

#[test]
fn tempo_automation_is_the_transports_tempo_lane_in_bpm() {
    let mut s = song();
    s.automation = vec![FlAutomation {
        channel: None,
        ..automation("Tempo", 0, FlControl::Tempo, 0, &[(0.0, 0.2), (4.0, 0.25), (8.0, 0.0)])
    }];
    let (doc, notes, _) = convert(&s, &bitwig_template(Template::default()));
    let transport = obj(&doc, doc.root.unwrap(), &[0x190]);
    let tempo = obj(&doc, transport, &[0x211]);
    let lanes = doc.field_list(obj(&doc, transport, &[0x2dc9]), 0x4ba);
    assert_eq!(lanes.len(), 1);
    assert_eq!(doc.obj(obj(&doc, transport, &[0x2dc9])).class, 0xfa3);
    let target = obj(&doc, lanes[0], &[0x2a3]);
    assert_eq!(doc.field_obj(target, 0x41), Some(tempo));
    assert_eq!(doc.obj(target).str(0x133b), "Transport");
    assert_eq!(doc.obj(target).str(0x133a), "Tempo");
    let bpm: Vec<(f64, f64)> = doc
        .field_list(obj(&doc, lanes[0], &[0x2d8c, 0x35ac]), 0x21f)
        .iter()
        .map(|&p| (doc.obj(p).num(0x2af).unwrap(), doc.obj(p).num(0x28f).unwrap()))
        .collect();
    // 10 + 512 x the FL value; below Bitwig's 20 BPM stays at 20, from where FL's line from 138 to 10 crosses it.
    assert_eq!(bpm, [(0.0, 112.4), (4.0, 138.0), (7.6875, 20.0), (8.0, 20.0)]);
    assert!(notes.iter().any(|n| n.starts_with("Tempo: Bitwig's tempo goes down to 20 BPM")), "{notes:?}");
    // The lanes list comes before the tempo it points at, so the tempo is defined in the lane, as Bitwig saves it.
    let fields: Vec<u32> = doc.obj(transport).fields.iter().map(|f| f.id).collect();
    assert!(fields.iter().position(|&f| f == 0x2dc9) < fields.iter().position(|&f| f == 0x211));
}

#[test]
fn a_master_fader_lane_is_left_out_where_the_template_master_already_has_one() {
    use mo_formats::bitwig::{Field, Object, Slot, Value, clone_object, encode};
    // The template's Sampler track volume lane, copied onto its master and pointed at the master's fader.
    let template = bitwig_template(Template::default());
    let mut t = decode(template.clone()).unwrap();
    let (group, sampler) = tracks(&t);
    let master = obj(&t, group, &[0x4e0]);
    let sampler_lane = t.field_list(obj(&t, sampler[0], &[0x2dc9]), 0x4ba)[0];
    let lane = clone_object(&mut t, sampler_lane);
    let fader = obj(&t, master, &[0x165, 0x1a4]);
    let target = obj(&t, lane, &[0x2a3]);
    t.obj_mut(target).set(0x41, Value::Obj(Slot::Obj(fader)));
    let mut list = Object::new(0xfa2);
    list.set(0x1832, Value::Null);
    list.set(0x4ba, Value::List(vec![Slot::Obj(lane)]));
    list.set(0x3f83, Value::List(Vec::new()));
    let list = t.add(list);
    t.obj_mut(master).fields.insert(0, Field { id: 0x2dc9, value: Value::Obj(Slot::Obj(list)) });
    let zip = t.has_zip.then(|| template[t.zip_offset..].to_vec());
    let template = encode(&t, zip.as_deref()).unwrap();

    let mut s = song();
    s.automation = vec![FlAutomation {
        channel: None,
        insert: Some(0),
        ..automation("Master fader", 0, FlControl::InsertVolume, 0, &[(0.0, 0.8)])
    }];
    let (doc, notes, _) = convert(&s, &template);
    let master = obj(&doc, tracks(&doc).0, &[0x4e0]);
    assert_eq!(doc.field_list(obj(&doc, master, &[0x2dc9]), 0x4ba).len(), 1);
    assert!(
        notes
            .contains(&"Master fader: automation left out (the Bitwig template already automates it there)".to_owned()),
        "{notes:?}"
    );
}

#[test]
fn audio_clips_come_across_stretched_and_pitched_on_a_copy_of_the_audio_track() {
    let mut vox = channel(0, "Vox", None, r"C:\Samples\Snare 01.wav");
    vox.notes.clear();
    vox.clips.clear();
    vox.stretch = FlStretch { pitch_cents: 1200, ..FlStretch::default() };
    vox.audio_clips = vec![FlAudioClip {
        start: 192,
        length: 96,
        from: 0.5,
        muted: false,
        track: 2,
        on_muted_track: false,
        partly_off: false,
    }];
    let mut s = song();
    s.tempo = 120.0;
    s.channels = vec![vox];
    s.automation.clear();
    let (doc, _, _) = convert(&s, &bitwig_template(Template::default()));
    let (_, tracks) = tracks(&doc);
    assert_eq!(tracks.len(), 1);
    let clips = doc.field_list(obj(&doc, tracks[0], &[0x2bf8, 0x2d8e]), 0x21f);
    assert_eq!(clips.len(), 1);
    let c = doc.obj(clips[0]);
    assert_eq!((c.class, c.num(0x2af), c.num(0x26)), (0xee, Some(2.0), Some(1.0)));
    // 1.5 seconds an octave up plays 0.75 seconds: 1.5 beats at 120 BPM. It starts a third of the way in.
    let lane = doc.field_list(obj(&doc, clips[0], &[0x2ae9, 0x10e9, 0x49c]), 0x18cb);
    let audio = doc.field_list(lane[0], 0x21f);
    assert_eq!(audio.len(), 1, "only the audio filled in");
    assert_eq!(doc.obj(audio[0]).num(0x26), Some(1.5));
    assert_eq!(doc.obj(obj(&doc, clips[0], &[0x2adb, 0x98c])).num(0x2af), Some(0.5));
    let play = obj(&doc, audio[0], &[0x288]);
    let transposed = obj(&doc, play, &[0xc33]);
    let point = doc.field_list(transposed, 0x21f);
    assert!((doc.obj(point[0]).num(0x28f).unwrap() - 12.0).abs() < 1e-9);
    let markers: Vec<_> = doc
        .field_list(obj(&doc, play, &[0x192]), 0x21f)
        .iter()
        .map(|&m| (doc.obj(m).num(0x2af).unwrap(), doc.obj(m).num(0xa6).unwrap()))
        .collect();
    assert_eq!(markers, [(0.0, 0.0), (1.5, 1.5)]);
    assert!(doc.field_list(obj(&doc, play, &[0x10ea, 0x10e9]), 0x21f).is_empty(), "transients found afresh");
    let Some(Value::Blob(summary)) = doc.meta("structure") else { panic!("a summary") };
    let summary = decode(summary.bytes()).unwrap();
    let project = obj(&summary, summary.root.unwrap(), &[0xad2]);
    assert_eq!(summary.obj(summary.field_list(project, 0xace)[0]).class, 0x287, "listed as an audio track");
}

#[test]
fn automation_is_listed_when_the_template_has_no_lane_to_copy() {
    let no_lane = Template { automation_lane: false, ..Template::default() };
    // The template's VST2 track still has its plugin lane, but no track has a volume lane to copy.
    let (_, notes, _) = convert(&song(), &bitwig_template(no_lane));
    assert!(notes.contains(
        &"Snare volume: automation left out (the Bitwig template has no automation lane to copy)".to_owned()
    ));
}

#[test]
fn a_template_without_the_tracks_it_needs_is_turned_down() {
    assert_eq!(
        template_problem(b"not a project").as_deref(),
        Some("The Bitwig template couldn't be read (it isn't a Bitwig file)")
    );
    assert_eq!(template_problem(&bitwig_template(Template::default())), None);
    let no_audio = Template { audio_track: false, ..Template::default() };
    assert_eq!(template_problem(&bitwig_template(no_audio)), None, "audio tracks are optional");
}

#[test]
fn an_empty_song_still_writes_a_project_bitwig_can_read() {
    let mut s = song();
    s.channels.clear();
    s.automation.clear();
    let (doc, notes, bytes) = convert(&s, &bitwig_template(Template::default()));
    assert!(tracks(&doc).1.is_empty());
    assert!(notes.is_empty());
    assert!(doc.zip().is_none(), "no plugin settings, no zip");
    // Written again from its decode, byte for byte.
    assert_eq!(mo_formats::bitwig::encode(&doc, None).unwrap(), bytes);
}

#[test]
fn converting_twice_gives_fresh_ids_and_never_touches_the_template() {
    let template = bitwig_template(Template::default());
    let copy = template.clone();
    let (a, _, _) = convert(&song(), &template);
    let (b, _, _) = convert(&song(), &template);
    assert_eq!(template, copy);
    let id = |doc: &Document| doc.obj(tracks(doc).1[0]).get(0xa51).cloned();
    assert_ne!(id(&a), id(&b));
}

#[test]
fn audio_clips_are_listed_when_the_template_has_no_audio_track_and_the_rest_converts() {
    let mut s = song();
    let mut vox = channel(2, "Vox", None, r"C:\Samples\Snare 01.wav");
    vox.notes.clear();
    vox.clips.clear();
    vox.audio_clips = vec![FlAudioClip {
        start: 0,
        length: 96,
        from: 0.0,
        muted: false,
        track: 2,
        on_muted_track: false,
        partly_off: false,
    }];
    s.channels.push(vox);
    s.automation.push(automation("Vox volume", 2, FlControl::Volume, 0, &[(0.0, 0.5)]));
    let no_audio = Template { audio_track: false, ..Template::default() };
    let (doc, notes, _) = convert(&s, &bitwig_template(no_audio));
    let names: Vec<_> = tracks(&doc).1.iter().map(|&t| doc.obj(t).str(0x15b).to_owned()).collect();
    assert_eq!(names, ["Snare", "Serum"]);
    assert!(notes.iter().any(|n| n.starts_with("Vox: its audio clips are left out (the Bitwig template has no audio")));
}

#[test]
fn a_vst2_program_that_cant_become_a_bank_keeps_its_own_ending_and_a_missing_id_isnt_the_templates() {
    let mut s = song();
    s.channels.remove(0);
    s.automation.clear();
    let synth = s.channels[0].instrument.as_mut().unwrap();
    synth.device_id = String::new();
    synth.state.as_mut().unwrap().data = b"CcnK damaged".to_vec();
    let (doc, _, _) = convert(&s, &bitwig_template(Template::default()));
    let serum = *chain(&doc, tracks(&doc).1[0]).last().unwrap();
    let d = doc.obj(def(&doc, serum));
    assert!(d.str(0xbe5).ends_with(".fxp"), "{}", d.str(0xbe5));
    assert_eq!(d.num(0xce4), Some(0.0), "not Drumazon's id");
}
