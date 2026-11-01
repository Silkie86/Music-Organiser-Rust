//! What an FL song becomes: the C# `BitwigProjectTests` that are about choices rather than Bitwig's objects, on the
//! same made-up songs.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mo_convert::plan::{gain, insert_gain, played_seconds, sample_ticks, tempo_bpm};
use mo_convert::{Device, Options, Plan, Role, SampleFile, Target, TrackKind, TrackRef, from_fl};
use mo_formats::fl::automation::{FlAutomation, FlControl, FlPoint};
use mo_formats::fl::song::{FlAudioClip, FlChannel, FlClip, FlInsert, FlPlugin, FlSong, FlStretch, FlTrack};
use mo_formats::presets::PresetBlob;
use mo_formats::{NoteAttribute, NoteEvent, PluginKind};

fn note(start: i64, length: i64, key: u8) -> NoteEvent {
    let velocity = mo_formats::velocity::from_unit(100.0 / 128.0);
    NoteEvent { start, length, key, velocity, muted: false, step: false, attribute: NoteAttribute::NONE }
}

fn step(start: i64, key: u8) -> NoteEvent {
    NoteEvent { step: true, ..note(start, 0, key) }
}

fn state(name: &str) -> Option<PresetBlob> {
    Some(PresetBlob {
        name: name.into(),
        plugin: name.into(),
        extension: ".vstpreset".into(),
        data: vec![1, 2, 3],
        kind: PluginKind::Unknown,
        native: false,
        place: String::new(),
        channel: None,
        program: 0,
    })
}

fn plugin(name: &str, format: &'static str, id: &str) -> FlPlugin {
    FlPlugin {
        name: name.into(),
        vendor: "Someone".into(),
        format,
        device_id: id.into(),
        path: String::new(),
        nothing_saved: false,
        state: state(name),
        parameter_ids: None,
        slot: None,
        enabled: true,
    }
}

fn effect(name: &str, format: &'static str, slot: u32) -> FlPlugin {
    FlPlugin { slot: Some(slot), ..plugin(name, format, "1181700674") }
}

fn channel(index: u32, name: &str, instrument: Option<FlPlugin>, sample: &str, insert: u32) -> FlChannel {
    let notes = vec![note(0, 96, 60)];
    let clip = FlClip {
        name: "Pattern 1".into(),
        start: 0,
        length: 384,
        notes: notes.clone(),
        muted: false,
        track: 1,
        on_muted_track: false,
    };
    FlChannel {
        index,
        name: name.into(),
        instrument,
        sample: sample.into(),
        insert,
        notes,
        root_key: 60,
        pitch_cents: 0,
        volume: 0.78125,
        pan: 0.5,
        clips: vec![clip],
        volume_envelope: None,
        audio_clips: Vec::new(),
        stretch: FlStretch::default(),
        enabled: true,
        switched_by: Vec::new(),
    }
}

fn insert(index: u32, name: &str, effects: Vec<FlPlugin>) -> FlInsert {
    FlInsert { index, name: name.into(), effects, fader: None, pan: None }
}

fn song(channels: Vec<FlChannel>, inserts: Vec<FlInsert>, automation: Vec<FlAutomation>) -> FlSong {
    FlSong {
        title: "Test".into(),
        ppq: 96,
        tempo: 120.0,
        channels,
        inserts,
        routes: Default::default(),
        edit_events: Vec::new(),
        automation,
        muted_tracks: Vec::new(),
        curves: Default::default(),
        warning: None,
    }
}

fn automation(name: &str, channel: Option<u32>, control: FlControl, points: &[(f64, f64)]) -> FlAutomation {
    FlAutomation {
        name: name.into(),
        channel,
        control,
        parameter: 0,
        points: points.iter().map(|&(beat, value)| FlPoint { beat, value }).collect(),
        insert: None,
        slot: None,
        starts: vec![0.0],
        parts: Vec::new(),
    }
}

fn snare(_: &str) -> Option<SampleFile> {
    Some(SampleFile { bytes: 1000, md5: [7; 16], rate: 44100, channels: 2, seconds: 0.5 })
}

fn plan(song: &FlSong) -> Plan {
    from_fl(song, Options::default(), &mut snare)
}

fn names(devices: &[Device]) -> Vec<String> {
    devices
        .iter()
        .map(|d| match d {
            Device::Sampler { sample, .. } => format!("Sampler {sample}"),
            Device::Transpose { octaves, semitones } => format!("Transpose {octaves} {semitones}"),
            Device::Plugin { plugin, .. } => plugin.name.clone(),
        })
        .collect()
}

#[test]
fn samplers_keep_their_root_key_and_plugins_are_transposed_by_octaves_and_semitones() {
    // A4, E5, D6, C4, C6 and C0 (four octaves up: three on the octave knob, the rest in semitones).
    let roots = [57u8, 64, 74, 48, 72, 12];
    let channels = roots
        .iter()
        .enumerate()
        .map(|(i, &root)| FlChannel {
            root_key: root,
            ..channel(i as u32, &format!("Synth {i}"), Some(plugin("Serum", "VST2", "1483109208")), "", 0)
        })
        .collect();
    let p = plan(&song(channels, vec![], vec![]));
    let shifts: Vec<_> = p
        .tracks
        .iter()
        .map(|t| match t.devices[0] {
            Device::Transpose { octaves, semitones } => (octaves, semitones),
            _ => panic!("a transpose first"),
        })
        .collect();
    assert_eq!(shifts, [(0, 3), (0, -4), (-1, -2), (1, 0), (-1, 0), (3, 12)]);

    let mut kick = channel(0, "Kick", None, r"C:\Samples\Kick.wav", 0);
    kick.root_key = 62;
    kick.pitch_cents = 150;
    let p = plan(&song(vec![kick], vec![], vec![]));
    assert_eq!(p.tracks[0].kind, TrackKind::Sampler);
    match &p.tracks[0].devices[0] {
        Device::Sampler { root_key, tune, info, .. } => {
            assert_eq!((*root_key, *tune), (62, 1.5));
            assert_eq!(info.as_ref().unwrap().seconds, 0.5);
        }
        d => panic!("a sampler, not {d:?}"),
    }
}

#[test]
fn a_pitch_knob_between_semitones_is_rounded_and_said() {
    let mut synth = channel(0, "Lead", Some(plugin("Serum", "VST2", "1483109208")), "", 0);
    synth.pitch_cents = 250;
    let p = plan(&song(vec![synth], vec![], vec![]));
    // 2.5 rounds to the even 2, as C#'s Math.Round does.
    assert_eq!(names(&p.tracks[0].devices), ["Transpose 0 2", "Serum"]);
    assert!(p.notes.contains(&"Lead: its pitch knob (+2.5 semitones) is rounded to whole semitones".to_owned()));
}

#[test]
fn an_inserts_effects_go_after_the_instrument_and_the_master_inserts_on_the_master() {
    let keys = plugin("Keys", "VST3", "565354536B657973");
    let eq = effect("Pro-Q 3", "VST3", 0);
    let reverb = FlPlugin { format: "FL", state: None, ..effect("Fruity Reeverb 2", "FL", 1) };
    let limiter = effect("Pro-L 2", "VST2", 0);
    let s = song(
        vec![channel(0, "Keys", Some(keys.clone()), "", 3), channel(1, "Pad", Some(keys), "", 3)],
        vec![insert(0, "", vec![limiter]), insert(3, "Keys bus", vec![eq, reverb])],
        vec![],
    );
    let p = plan(&s);
    for t in &p.tracks {
        assert_eq!(names(&t.devices), ["Keys", "Pro-Q 3"]);
        assert!(matches!(&t.devices[1], Device::Plugin { role: Role::Effect, enabled: true, .. }));
    }
    assert_eq!(names(&p.master_effects), ["Pro-L 2"]);
    assert!(p.notes.iter().any(|n| n.starts_with("Keys: left out Fruity Reeverb 2 (FL Studio plugin)")));
    assert!(p.notes.iter().any(|n| n.contains("own copy of Keys bus's effects")));
}

#[test]
fn an_effect_switched_off_in_fl_stays_off_and_its_settings_are_automated_on_every_copy() {
    let off = FlPlugin { enabled: false, ..effect("Pro-Q 3", "VST2", 1) };
    let s = song(
        vec![channel(0, "Keys", Some(plugin("Keys", "VST2", "1")), "", 2), channel(1, "Pad", None, "pad.wav", 2)],
        vec![insert(2, "", vec![off])],
        vec![FlAutomation {
            insert: Some(2),
            slot: Some(1),
            parameter: 5,
            ..automation("Pro-Q 3 - Gain", None, FlControl::InsertEffect, &[(0.0, 0.2), (4.0, 0.8)])
        }],
    );
    let p = plan(&s);
    assert!(matches!(&p.tracks[0].devices[1], Device::Plugin { enabled: false, .. }));
    let lanes: Vec<_> = p.lanes.iter().map(|l| (l.track, l.target.clone())).collect();
    let target = |device| Target::Plugin {
        device,
        path: "CONTENTS/ROOT_GENERIC_MODULE/PARAM5".into(),
        id: 5,
        name: "Gain".into(),
    };
    // The Keys track has the effect after its plugin; the Pad track after its sampler.
    assert_eq!(lanes, [(TrackRef::Track(0), target(1)), (TrackRef::Track(1), target(1))]);
}

#[test]
fn an_effect_slot_switched_on_and_off_is_a_switch_lane_on_every_copy() {
    let s = song(
        vec![channel(0, "Keys", Some(plugin("Keys", "VST2", "1")), "", 2), channel(1, "Pad", None, "pad.wav", 2)],
        vec![insert(2, "", vec![effect("Pro-Q 3", "VST2", 1)])],
        vec![
            FlAutomation {
                insert: Some(2),
                slot: Some(1),
                ..automation("Pro-Q 3 on", None, FlControl::InsertSlotOnOff, &[(0.0, 1.0), (4.0, 0.2), (8.0, 0.5)])
            },
            // A slot with nothing converted in it.
            FlAutomation {
                insert: Some(2),
                slot: Some(5),
                ..automation("Empty slot", None, FlControl::InsertSlotOnOff, &[(0.0, 1.0)])
            },
        ],
    );
    let p = plan(&s);
    let lanes: Vec<_> = p.lanes.iter().map(|l| (l.track, l.target.clone())).collect();
    assert_eq!(
        lanes,
        [(TrackRef::Track(0), Target::OnOff { device: 1 }), (TrackRef::Track(1), Target::OnOff { device: 1 })]
    );
    // FL counts 0.5 and above as on.
    let values: Vec<_> = p.lanes[0].points.iter().map(|x| (x.beat, x.value)).collect();
    assert_eq!(values, [(0.0, 1.0), (4.0, 0.0), (8.0, 1.0)]);
    assert!(p.notes.contains(&"Empty slot: automation left out (its effect isn't converted)".to_owned()));
}

#[test]
fn volume_and_pan_get_one_lane_each_in_song_order() {
    let keys = FlPlugin { parameter_ids: Some(vec![10, 20, 2_000_003]), ..plugin("Keys", "VST3", "565354536B657973") };
    let setting = |name: &str, n: u32, points: &[(f64, f64)]| FlAutomation {
        parameter: n,
        ..automation(name, Some(0), FlControl::PluginParameter, points)
    };
    let s = song(
        vec![channel(0, "Keys", Some(keys), "", 0)],
        vec![],
        vec![
            automation(
                "Fade",
                Some(0),
                FlControl::Volume,
                &[(0.0, 0.78125), (1.0, 0.1667), (1.5, 0.8798), (3.25, 0.1744)],
            ),
            automation("Pan later", Some(0), FlControl::Pan, &[(8.0, 0.5), (12.0, 0.0)]),
            automation("Pan first", Some(0), FlControl::Pan, &[(0.0, 0.0), (4.0, 1.0)]),
            automation("Bend", Some(0), FlControl::Pitch, &[(0.0, 0.5)]),
            setting("Keys - Brightness", 2, &[(0.0, 0.25), (2.0, 0.75)]),
            setting("Keys - Unknown", 3, &[(0.0, 1.0)]),
            automation("Elsewhere", Some(7), FlControl::Volume, &[(0.0, 1.0)]),
            setting("Keys - Warmth", 0, &[(0.0, 0.5)]),
        ],
    );
    let p = plan(&s);
    let targets: Vec<_> = p
        .lanes
        .iter()
        .map(|l| match &l.target {
            Target::Volume => "Volume".to_owned(),
            Target::Pan => "Pan".to_owned(),
            Target::Plugin { name, path, .. } => format!("{name} {path}"),
            Target::OnOff { device } => format!("On/Off {device}"),
            Target::Tempo => "Tempo".to_owned(),
        })
        .collect();
    assert_eq!(
        targets,
        [
            "Volume",
            "Pan",
            "Brightness CONTENTS/ROOT_GENERIC_MODULE/PID1e8483",
            "Warmth CONTENTS/ROOT_GENERIC_MODULE/PIDa"
        ]
    );
    let points = |i: usize| -> Vec<(f64, f64)> {
        p.lanes[i].points.iter().map(|x| (x.beat, (x.value * 1e4).round() / 1e4)).collect()
    };
    // FL's 78% fader is a gain of 1 (0 dB); Bitwig's fader is its cube root (the C# test's 0.3571 is cbrt(0.0455)).
    assert_eq!(points(0), [(0.0, 1.0), (1.0, 0.0455), (1.5, 1.2682), (3.25, 0.0498)]);
    assert!((gain(0.1667).cbrt() - 0.3571).abs() < 1e-4);
    // "Pan first" ends right at 4; "Pan later" starts at 8, so the pan holds until then and jumps (C# slid it).
    assert_eq!(points(1), [(0.0, -1.0), (4.0, 1.0), (8.0, 1.0), (8.0, 0.0), (12.0, -1.0)]);
    assert_eq!(points(2), [(0.0, 0.25), (2.0, 0.75)]);
    assert!(matches!(p.lanes[2].target, Target::Plugin { device: 0, id: 2_000_003, .. }));

    let automation: Vec<_> = p.notes.iter().filter(|n| n.contains("automation")).cloned().collect();
    assert_eq!(
        automation,
        [
            "Bend: automation left out (pitch has no equivalent here)",
            "Keys - Unknown: automation left out (FL didn't save which of the plugin's settings it moves)",
            "Elsewhere: automation left out (what it moves isn't converted)",
        ]
    );
}

#[test]
fn where_two_clips_moving_a_setting_meet_the_lane_goes_on_from_the_one_starting_there() {
    // Two automation clips moving the channel's volume: the second starts at beat 4, where the first ends, so its point
    // there has the last word, wherever it comes in the song's list.
    let first = automation("Fade in", Some(0), FlControl::Volume, &[(0.0, 0.0), (4.0, 0.5)]);
    let second = FlAutomation {
        starts: vec![4.0],
        parts: Vec::new(),
        ..automation("Drop", Some(0), FlControl::Volume, &[(4.0, 0.2), (8.0, 0.78125)])
    };
    let p =
        plan(&song(vec![channel(0, "Keys", Some(plugin("Keys", "VST2", "1")), "", 0)], vec![], vec![second, first]));
    assert_eq!(p.lanes.len(), 1);
    let got: Vec<_> = p.lanes[0].points.iter().map(|x| (x.beat, x.from)).collect();
    assert_eq!(got, [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (8.0, 4.0)]);
}

#[test]
fn tempo_automation_is_a_song_lane_in_bpm_and_moved_faders_are_listed() {
    let mut bus = insert(3, "Bass", vec![]);
    bus.fader = Some(9000);
    let tempo = automation("Tempo", None, FlControl::Tempo, &[(8.0, 0.5), (12.0, 0.0)]);
    let p = plan(&song(vec![channel(0, "Bass", Some(plugin("Bass", "VST2", "1")), "", 3)], vec![bus], vec![tempo]));
    assert_eq!(p.lanes.len(), 1);
    assert_eq!((p.lanes[0].track, &p.lanes[0].target), (TrackRef::Song, &Target::Tempo));
    // Until the clip starts at beat 8 the song keeps the project's 120 BPM; then 0.5 is 266 BPM and 0 is 10 BPM.
    let points: Vec<_> = p.lanes[0].points.iter().map(|x| (x.beat, x.value)).collect();
    assert_eq!(points, [(0.0, 120.0), (8.0, 120.0), (8.0, 266.0), (12.0, 10.0)]);
    assert!(p.notes.iter().any(|n| n.starts_with("Mixer: the fader or pan of Bass (insert 3)")));
}

#[test]
fn the_insert_fader_follows_the_table_measured_in_fl_studio() {
    let db = |v: f64| 20.0 * insert_gain(v).log10();
    assert_eq!(insert_gain(0.0), 0.0);
    assert!(db(0.8).abs() < 1e-9);
    assert!((db(0.5) + 9.179).abs() < 1e-9);
    assert!((db(1.0) - 5.59).abs() < 1e-9);
    // Between two measured points, the gain runs in a straight line.
    let half = (insert_gain(0.80) + insert_gain(0.81)) / 2.0;
    assert!((insert_gain(0.805) - half).abs() < 1e-12);
    assert_eq!(insert_gain(1.5), insert_gain(1.0));
    assert_eq!(tempo_bpm(0.5), 266.0);
}

#[test]
fn an_insert_fader_and_pan_are_automated_on_the_tracks_it_feeds_after_their_own_level() {
    let fader = |name: &str, insert, control, points: &[(f64, f64)]| FlAutomation {
        insert: Some(insert),
        ..automation(name, None, control, points)
    };
    let mut quiet = channel(1, "Pad", Some(plugin("Pad", "VST2", "2")), "", 3);
    quiet.volume = 0.5;
    let mut left = channel(2, "Keys", Some(plugin("Keys", "VST2", "3")), "", 4);
    left.pan = 0.25;
    let mut loud = channel(3, "Loud", Some(plugin("Loud", "VST2", "4")), "", 5);
    loud.volume = 1.0;
    let s = song(
        vec![channel(0, "Bass", Some(plugin("Bass", "VST2", "1")), "", 3), quiet, left, loud],
        vec![
            insert(3, "Bus", vec![]),
            insert(4, "Keys", vec![]),
            insert(5, "Loud", vec![]),
            insert(9, "Empty", vec![]),
        ],
        vec![
            fader("Bus fader", 3, FlControl::InsertVolume, &[(0.0, 0.8), (4.0, 0.5)]),
            fader("Bus pan", 3, FlControl::InsertPan, &[(0.0, 0.5), (4.0, 1.0)]),
            fader("Keys pan", 4, FlControl::InsertPan, &[(0.0, 0.0)]),
            fader("Master fader", 0, FlControl::InsertVolume, &[(0.0, 1.0)]),
            fader("Nobody", 9, FlControl::InsertVolume, &[(0.0, 1.0)]),
            fader("Loud fader", 5, FlControl::InsertVolume, &[(0.0, 1.0)]),
            // Its own volume is automated too, so that lane stays.
            automation("Pad volume", Some(1), FlControl::Volume, &[(0.0, 0.2)]),
        ],
    );
    let p = plan(&s);
    let lane = |track, target: Target| p.lanes.iter().find(|l| l.track == track && l.target == target);
    let values = |track, target| lane(track, target).unwrap().points.iter().map(|x| x.value).collect::<Vec<_>>();
    // Bass is at FL's default level (gain 1), so its fader lane is the insert's alone, with a point every 0.05 of
    // fader travel so the fade keeps FL's curve.
    let bass = values(TrackRef::Track(0), Target::Volume);
    let want: Vec<f64> = (0..=6).map(|i| insert_gain(0.8 - 0.05 * i as f64)).collect();
    assert_eq!(bass.len(), want.len(), "{bass:?}");
    assert!(bass.iter().zip(&want).all(|(b, w)| (b - w).abs() < 1e-9), "{bass:?}");
    assert_eq!(values(TrackRef::Track(3), Target::Volume), [2.0]);
    assert_eq!(values(TrackRef::Track(0), Target::Pan), [0.0, 1.0]);
    assert_eq!(values(TrackRef::Track(1), Target::Volume), [gain(0.2)]);
    assert_eq!(values(TrackRef::Track(1), Target::Pan), [0.0, 1.0]);
    assert!(lane(TrackRef::Track(2), Target::Pan).is_none(), "Keys is panned on its channel");
    assert_eq!(values(TrackRef::Master, Target::Volume), [insert_gain(1.0)]);
    for note in [
        "Bus fader: automation left out on some tracks (Pad has its own volume automated)",
        "Keys pan: automation left out (Keys's own pan isn't in the middle)",
        "Nobody: automation left out (no converted channel feeds it)",
        "Loud fader: on Loud the fader and the channel's own level come to more than Bitwig's +6 dB, so the loudest \
         part stays at +6 dB",
    ] {
        assert!(p.notes.contains(&note.to_owned()), "{note}: {:?}", p.notes);
    }
}

#[test]
fn steps_play_until_their_sample_ends_or_the_next_note() {
    let mut kick = channel(0, "Kick", None, "kick.wav", 0);
    kick.clips[0].notes = vec![step(0, 60), step(192, 60), step(0, 72)];
    let s = song(vec![kick], vec![], vec![]);
    // Half a second at 120 BPM is a beat (96 ticks); an octave up, half that.
    let lengths = |p: &Plan| p.tracks[0].clips[0].notes.iter().map(|n| n.length).collect::<Vec<_>>();
    assert_eq!(lengths(&plan(&s)), [96, 96, 48]);
    let until_next = from_fl(&s, Options { steps_until_next_note: true, ..Options::default() }, &mut snare);
    assert_eq!(lengths(&until_next), [192, 192, 384]);
    assert_eq!(sample_ticks(0.5, 12.0, 120.0, 96), 48);
}

#[test]
fn channels_the_playlist_never_plays_come_across_with_their_sound_but_no_clips() {
    let mut idle = channel(0, "Idle", Some(plugin("Pad", "VST2", "1")), "", 0);
    idle.clips.clear();
    idle.notes.clear();
    let p = plan(&song(vec![idle], vec![], vec![]));
    assert_eq!((p.tracks.len(), p.tracks[0].clips.len()), (1, 0));
    assert!(p.notes.iter().any(|n| n.starts_with("Idle: the FL Studio playlist never plays it")));
}

#[test]
fn switched_off_channels_and_clips_on_muted_playlist_tracks_are_muted() {
    let mut off = channel(0, "Off", Some(plugin("Pad", "VST2", "1")), "", 0);
    off.enabled = false;
    let mut hidden = channel(1, "Hidden", Some(plugin("Pad", "VST2", "1")), "", 0);
    hidden.clips[0].on_muted_track = true;
    hidden.clips[0].track = 4;
    let mut some = channel(2, "Some", Some(plugin("Pad", "VST2", "1")), "", 0);
    some.clips.push(FlClip { start: 384, on_muted_track: true, track: 4, ..some.clips[0].clone() });
    let mut s = song(vec![off, hidden, some], vec![], vec![]);
    s.muted_tracks = vec![FlTrack { number: 4, name: "Vox".into() }];
    let p = plan(&s);
    assert_eq!(p.tracks.iter().map(|t| t.muted).collect::<Vec<_>>(), [true, true, false]);
    assert!(!p.tracks[1].clips[0].muted, "the track is muted, not its clip");
    assert_eq!(p.tracks[2].clips.iter().map(|c| c.muted).collect::<Vec<_>>(), [false, true]);
    assert!(p.notes.contains(&"Off: switched off in FL Studio's Channel Rack, so its track is muted".to_owned()));
    assert!(
        p.notes.contains(
            &"Hidden: muted, because its clips are all on a muted playlist track in FL Studio (Vox, playlist track 4)"
                .to_owned()
        )
    );
}

#[test]
fn overlapping_audio_clips_go_on_tracks_of_their_own_that_the_channels_volume_moves() {
    let mut vox = channel(0, "Vox", None, "vox.wav", 0);
    vox.clips.clear();
    vox.notes.clear();
    let audio = |start, length| FlAudioClip {
        start,
        length,
        from: 0.0,
        muted: false,
        track: 2,
        on_muted_track: false,
        partly_off: false,
    };
    vox.audio_clips = vec![audio(0, 384), audio(192, 96), audio(384, 96)];
    let fade = automation("Vox volume", Some(0), FlControl::Volume, &[(0.0, 0.5)]);
    let p = plan(&song(vec![vox], vec![], vec![fade]));
    assert_eq!(
        p.tracks.iter().map(|t| (t.name.as_str(), t.kind)).collect::<Vec<_>>(),
        [("Vox", TrackKind::Audio), ("Vox 2", TrackKind::Audio)]
    );
    assert_eq!(p.tracks[0].audio_clips.iter().map(|c| c.start).collect::<Vec<_>>(), [0.0, 4.0]);
    assert_eq!(p.lanes.iter().map(|l| l.track).collect::<Vec<_>>(), [TrackRef::Track(0), TrackRef::Track(1)]);
    assert!(p.notes.contains(&"Vox: its audio clips overlap in FL Studio, so they're on 2 tracks".to_owned()));

    // Without audio tracks, they're listed instead.
    let mut s = song(vec![p_channel_with_audio()], vec![], vec![]);
    s.channels[0].clips.clear();
    let without = from_fl(&s, Options { audio_tracks: false, ..Options::default() }, &mut snare);
    assert!(without.tracks.is_empty());
    assert!(without.notes.iter().any(|n| n.starts_with("Vox: its audio clips are left out (the Bitwig template")));
}

fn p_channel_with_audio() -> FlChannel {
    let mut vox = channel(0, "Vox", None, "vox.wav", 0);
    vox.notes.clear();
    vox.audio_clips = vec![FlAudioClip {
        start: 0,
        length: 96,
        from: 0.0,
        muted: false,
        track: 1,
        on_muted_track: false,
        partly_off: false,
    }];
    vox
}

#[test]
fn audio_clips_are_stretched_pitched_and_trimmed_as_in_fl() {
    let mut vox = p_channel_with_audio();
    vox.clips.clear();
    // Resampled up an octave: a half-second sample plays a quarter second (half a beat at 120 BPM).
    vox.stretch = FlStretch { pitch_cents: 1200, ..FlStretch::default() };
    vox.audio_clips[0].from = 0.25;
    let p = plan(&song(vec![vox.clone()], vec![], vec![]));
    let clip = &p.tracks[0].audio_clips[0];
    assert_eq!((clip.seconds, clip.played, clip.offset, clip.length), (0.5, 0.5, 0.25, 1.0));
    assert!((clip.semitones - 12.0).abs() < 1e-9);
    // Stretched to a length instead (not resampled): 384 of FL's Time units are two beats, a second at 120 BPM.
    vox.stretch = FlStretch { mode: 1, time: 384, pitch_cents: 300, ..FlStretch::default() };
    assert_eq!(played_seconds(&vox.stretch, 0.5, 120.0), 1.0);
    let p = plan(&song(vec![vox.clone()], vec![], vec![]));
    assert_eq!((p.tracks[0].audio_clips[0].played, p.tracks[0].audio_clips[0].semitones), (2.0, 3.0));
    // A sample that can't be read: its length is guessed from the clip, and it's said.
    let p = from_fl(&song(vec![vox], vec![], vec![]), Options::default(), &mut |_| None);
    assert!(p.notes.iter().any(|n| n.starts_with("Vox: couldn't read vox.wav")));
}

#[test]
fn an_empty_song_makes_an_empty_plan() {
    let p = plan(&song(vec![], vec![], vec![]));
    assert!(p.tracks.is_empty() && p.lanes.is_empty() && p.notes.is_empty() && p.master_effects.is_empty());
    assert_eq!((p.tempo, p.ppq), (120.0, 96));
}

#[test]
fn between_two_clips_moving_one_thing_the_value_holds_then_jumps() {
    let a = automation("Rise", Some(0), FlControl::Pan, &[(0.0, 0.5), (4.0, 1.0)]);
    let b =
        FlAutomation { starts: vec![8.0], ..automation("Drop", Some(0), FlControl::Pan, &[(8.0, 0.0), (12.0, 0.5)]) };
    let p = plan(&song(vec![channel(0, "Keys", Some(plugin("Keys", "VST2", "1")), "", 0)], vec![], vec![a, b]));
    let got: Vec<_> = p.lanes[0].points.iter().map(|x| (x.beat, x.value)).collect();
    assert_eq!(got, [(0.0, 0.0), (4.0, 1.0), (8.0, 1.0), (8.0, -1.0), (12.0, 0.0)]);
    assert_eq!(p.lanes[0].clips, ["Rise", "Drop"]);
}

#[test]
fn effects_on_an_insert_no_converted_channel_feeds_are_listed() {
    let s = song(
        vec![channel(0, "Keys", Some(plugin("Keys", "VST2", "1")), "", 1)],
        vec![insert(1, "", vec![]), insert(10, "Reverb bus", vec![effect("Valhalla", "VST3", 0)])],
        vec![],
    );
    let p = plan(&s);
    assert!(p.notes.iter().any(|n| n.starts_with("Reverb bus (insert 10): its effects (Valhalla) are left out")));
}

#[test]
fn a_channel_with_notes_and_audio_clips_names_each_track_apart() {
    let mut vox = p_channel_with_audio();
    vox.instrument = None;
    let mut reads = 0;
    let p = from_fl(&song(vec![vox], vec![], vec![]), Options::default(), &mut |path| {
        reads += 1;
        snare(path)
    });
    assert_eq!(p.tracks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Vox", "Vox 2"]);
    assert_eq!(reads, 1, "the sample is read once");
}

#[test]
fn a_bus_fader_reaches_the_channels_routed_to_it_through_other_inserts_but_not_through_a_send() {
    let fader = |name: &str, insert| FlAutomation {
        insert: Some(insert),
        ..automation(name, None, FlControl::InsertVolume, &[(0.0, 0.8)])
    };
    // Bass goes to insert 2, which goes on to the bus on insert 1, then the master. Keys's insert 3 goes to the master
    // and is sent to the bus too, so the bus moves only part of its sound.
    let mut s = song(
        vec![
            channel(0, "Bass", Some(plugin("Bass", "VST2", "1")), "", 2),
            channel(1, "Keys", Some(plugin("Keys", "VST2", "2")), "", 3),
        ],
        vec![insert(1, "Bus", vec![]), insert(2, "Bass", vec![]), insert(3, "Keys", vec![])],
        vec![fader("Bass fader", 2), fader("Bus fader", 1), fader("Keys fader", 3)],
    );
    s.routes = [(0, vec![]), (1, vec![0]), (2, vec![1]), (3, vec![0, 1])].into_iter().collect();
    let p = plan(&s);
    let lanes: Vec<_> = p.lanes.iter().map(|l| (l.track, l.from_insert)).collect();
    assert_eq!(lanes, [(TrackRef::Track(0), Some(2)), (TrackRef::Track(1), Some(3))]);
    // Bass follows its own insert, nearer to it than the bus (Bitwig has one fader), and the bus has only part of Keys.
    let note = "Bus fader: automation left out (Bass follows insert 2's fader, nearer to it; Keys reaches it through a send or \
         sidechain)";
    assert!(p.notes.contains(&note.to_owned()), "{:?}", p.notes);
    // The same whichever clip comes first.
    s.automation.reverse();
    let reversed = plan(&s);
    let mut lanes: Vec<_> = reversed.lanes.iter().map(|l| (l.track, l.from_insert)).collect();
    lanes.sort();
    assert_eq!(lanes, [(TrackRef::Track(0), Some(2)), (TrackRef::Track(1), Some(3))]);
    assert!(reversed.notes.contains(&note.to_owned()), "{:?}", reversed.notes);

    // On its own, the bus fader goes on Bass.
    s.automation = vec![fader("Bus fader", 1)];
    let p = plan(&s);
    assert_eq!(p.lanes.iter().map(|l| (l.track, l.from_insert)).collect::<Vec<_>>(), [(TrackRef::Track(0), Some(1))]);
    assert!(p.notes.contains(
        &"Bus fader: automation left out on some tracks (Keys reaches it through a send or sidechain)".to_owned()
    ));
}

#[test]
fn edit_events_are_listed_as_not_carried() {
    let mut s = song(vec![channel(0, "Bass", Some(plugin("Bass", "VST2", "1")), "", 0)], vec![], vec![]);
    s.edit_events = vec![(1, "Lead".to_owned()), (4, "Pattern 4".to_owned())];
    let p = plan(&s);
    let note = r#"Edit events: the knob moves recorded into patterns "Lead", "Pattern 4" for anything but a plugin's setting aren't carried over yet"#;
    assert!(p.notes.contains(&note.to_owned()), "{:?}", p.notes);
    // A long list stops at eight.
    s.edit_events = (1..=10).map(|i| (i, format!("P{i}"))).collect();
    let p = plan(&s);
    assert!(p.notes.iter().any(|n| n.contains(r#""P8" and 2 more for anything"#)), "{:?}", p.notes);
}
