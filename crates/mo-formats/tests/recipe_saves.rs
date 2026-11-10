//! Tests on saves FL Studio, Bitwig and Fender Studio (Studio One) really wrote, made by hand from the recipe in
//! `tests/fixtures/recipe/README.md`. Each test says what the save should read as. The `#[ignore]` ones are what the
//! readers get wrong or don't read yet: fix the reader, then take the `#[ignore]` off.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use mo_formats::arrangement::{self, Arrangement, AudioPart, LaneKind, PreviewLane, Stretch};

fn save(daw: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recipe").join(daw).join(name)
}

/// A save that didn't decode comes back as an empty view with a "couldn't be read" note.
fn decoded(path: &Path, a: &Arrangement) {
    assert!(!a.notes.iter().any(|n| n.contains("couldn't be read")), "{} didn't decode: {:?}", path.display(), a.notes);
}

/// The first view of a save (the DAW's own arrangement).
fn read(daw: &str, name: &str) -> Arrangement {
    let path = save(daw, name);
    let a = arrangement::read(&path).unwrap().into_iter().next().unwrap();
    decoded(&path, &a);
    a
}

fn fl(step: &str) -> Arrangement {
    read("fl", &format!("MO Recipe {step}.flp"))
}

fn bitwig(step: &str) -> Arrangement {
    read("bitwig", &format!("MO Recipe {step}.bwproject"))
}

fn fender(step: &str) -> Arrangement {
    read("studio-one", &format!("MO Recipe {step}.song"))
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

fn lane(a: &Arrangement, kind: LaneKind) -> &PreviewLane {
    a.lanes.iter().find(|l| l.kind == kind).unwrap()
}

/// The tone's audio part on the audio lane.
fn tone(a: &Arrangement) -> (f64, f64, AudioPart) {
    let clip = &lane(a, LaneKind::Audio).clips[0];
    (clip.start, clip.length, clip.audio[0].clone())
}

/// C, D, E, G as quarter notes over the first bar.
fn the_four_notes(a: &Arrangement) {
    let clip = &lane(a, LaneKind::Instrument).clips[0];
    let notes: Vec<_> = clip.notes.iter().map(|n| (n.start, n.length, n.key)).collect();
    assert_eq!(notes, [(0.0, 1.0, 60), (1.0, 1.0, 62), (2.0, 1.0, 64), (3.0, 1.0, 67)]);
    assert!(near(clip.start, 0.0) && near(clip.length, 4.0));
}

#[test]
fn every_save_reads() {
    for daw in ["fl", "bitwig", "studio-one"] {
        for entry in std::fs::read_dir(save(daw, "")).unwrap() {
            let path = entry.unwrap().path();
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
            if matches!(ext.as_deref(), Some("flp" | "bwproject" | "song")) {
                let views = arrangement::read(&path).unwrap();
                assert!(!views.is_empty(), "{}", path.display());
                for a in &views {
                    decoded(&path, a);
                }
            }
        }
    }
}

#[test]
fn the_presets_read() {
    let bw =
        mo_formats::bitwig::decode(std::fs::read(save("bitwig", "MO Recipe Polysynth.bwpreset")).unwrap()).unwrap();
    assert_eq!(bw.body_error, None);
    let fst = std::fs::read(save("fl", "MO Recipe Keys.fst")).unwrap();
    let fst = mo_formats::fl::events::load(&fst).unwrap();
    assert_eq!(fst.major, 25);
    assert_eq!(fst.warning, None);
    // A multisample is a zip of multisample.xml and the sounds: the three tones on C3, E3 and G3.
    let file = std::fs::File::open(save("bitwig", "MO Recipe Tones.multisample")).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("multisample.xml").unwrap(), &mut xml).unwrap();
    for (tone, key) in [("A", 60), ("B", 64), ("C", 67)] {
        assert!(xml.contains(&format!("file=\"MO Tone {tone}.wav\"")), "{tone}");
        assert!(xml.contains(&format!("root=\"{key}\"")), "{key}");
        assert!(zip.by_name(&format!("MO Tone {tone}.wav")).is_ok());
    }
}

#[test]
fn tempos() {
    assert!(near(fl("01 empty").tempo, 140.0));
    assert!(near(bitwig("01 empty").tempo, 140.0));
    assert!(near(fender("01 empty").tempo, 140.0));
    assert!(near(fl("02 notes").tempo, 128.0));
    assert!(near(bitwig("02 notes").tempo, 128.0));
    assert!(near(fender("02 notes").tempo, 128.0));
    assert!(near(fender("08 stretch sound").tempo, 100.0));
}

#[test]
fn notes() {
    the_four_notes(&fl("02 notes"));
    the_four_notes(&bitwig("03 automation"));
    the_four_notes(&fender("02 notes"));
    // Bitwig's step 2 was saved with the notes as 16ths (fixed by hand before step 3).
    let a = bitwig("02 notes");
    let clip = &lane(&a, LaneKind::Instrument).clips[0];
    let notes: Vec<_> = clip.notes.iter().map(|n| (n.start, n.length, n.key)).collect();
    assert_eq!(notes, [(0.0, 0.25, 60), (0.25, 0.25, 62), (0.5, 0.25, 64), (0.75, 0.25, 67)]);
}

#[test]
fn fl_volume_automation_goes_from_full_to_half_over_bar_1() {
    let a = fl("03 automation");
    let points: Vec<_> = lane(&a, LaneKind::Automation).clips[0].points.iter().map(|p| (p.beat, p.value)).collect();
    assert_eq!(points.len(), 2);
    assert!(near(points[0].0, 0.0) && near(points[0].1, 1.0));
    assert!(near(points[1].0, 4.0) && near(points[1].1, 0.5));
}

#[test]
fn the_tone_lands_at_bar_2() {
    // MO Tone A is half a second: 1.0667 beats at 128 (FL rounds the clip to 1.0625). Fender Studio stretches it by
    // default: it took the tone for one beat at 120, so it fills one beat and half a second of it plays to a beat.
    for (a, length, seconds_per_beat) in [
        (fl("04 audio clip"), 1.0625, 60.0 / 128.0),
        (bitwig("04 audio clip"), 1.0667, 60.0 / 128.0),
        (fender("04 audio clip"), 1.0, 0.5),
    ] {
        let (start, len, part) = tone(&a);
        assert!(near(start, 4.0), "{start}");
        assert!(near(len, length), "{len}");
        assert!(part.path.ends_with("MO Tone A.wav"));
        assert!(near(part.from, 0.0));
        assert!(near(part.seconds_per_beat.unwrap(), seconds_per_beat));
        assert!(!part.reversed);
    }
}

#[test]
fn fl_resample_at_200_percent_plays_twice_as_long() {
    let (_, len, part) = tone(&fl("10 stretch resample"));
    assert!(near(len, 2.1354));
    assert!(near(part.seconds_per_beat.unwrap(), 60.0 / 128.0 / 2.0));
}

#[test]
fn trimmed_by_a_16th() {
    // The recipe said a beat; the saves' left edges moved one grid step, a 16th (a quarter of a beat).
    let (start, len, part) = tone(&fl("15 trimmed"));
    assert!(near(start, 4.25) && near(len, 0.8125) && near(part.from, 0.25 * 60.0 / 128.0));
    // Fender's tone plays at twice its speed there (step 8f), so the whole of it takes half a beat: a 16th off the
    // left is half of it, a quarter of a second.
    let (start, len, part) = tone(&fender("11 trimmed"));
    assert!(near(start, 4.25) && near(len, 0.25) && near(part.from, 0.25), "{part:?}");
}

#[test]
fn reversed_fl_clip() {
    assert!(tone(&fl("13 reversed")).2.reversed);
    // Step 14 was made from 13, so it is still reversed.
    assert!(tone(&fl("14 gain and fades")).2.reversed);
    assert!(!tone(&fl("15 trimmed")).2.reversed);
}

#[test]
fn muted_tracks() {
    assert!(lane(&fl("07 muted track"), LaneKind::Instrument).muted);
    assert!(!lane(&fl("06 two arrangements"), LaneKind::Instrument).muted);
    assert!(lane(&fender("06 mixer"), LaneKind::Audio).muted);
    assert!(!lane(&fender("05 drum pads"), LaneKind::Audio).muted);
}

#[test]
fn bitwig_group() {
    let a = bitwig("06 group and off");
    // The save still holds step 3's automation: the tempo's curve first, the volume and pan under the Polysynth.
    let depths: Vec<_> = a.lanes.iter().map(|l| (l.name.as_str(), l.kind, l.depth)).collect();
    assert_eq!(
        depths,
        [
            ("Tempo", LaneKind::Automation, 0),
            ("Group 1", LaneKind::Group, 0),
            ("Polysynth", LaneKind::Instrument, 1),
            ("Volume", LaneKind::Automation, 2),
            ("Pan", LaneKind::Automation, 2),
            ("MO Tone A", LaneKind::Audio, 1),
            ("Drum Machine", LaneKind::Instrument, 0),
        ]
    );
}

#[test]
fn fl_reads_the_first_arrangement_only() {
    // The second arrangement ("Second", Pattern 1 at bar 3) must not leak into the first.
    let a = fl("06 two arrangements");
    let clips = &lane(&a, LaneKind::Instrument).clips;
    assert_eq!(clips.len(), 1);
    assert!(near(clips[0].start, 0.0));
}

#[test]
fn time_signature_and_marker() {
    let a = fl("16 time signature and marker");
    assert_eq!(a.beats_per_bar, 3);
    assert_eq!(a.markers.len(), 1);
    assert_eq!(a.markers[0].name, "Verse");
    assert!(near(a.markers[0].beat, 3.0));
    // Fender's marker went on beat 4 (where the tone starts), not bar 2 of 3/4.
    let a = fender("12 time signature and marker");
    assert_eq!(a.beats_per_bar, 3);
    assert_eq!(a.markers[0].name, "Verse");
    assert!(near(a.markers[0].beat, 4.0));
}

#[test]
fn fender_follow_moves_but_does_not_stretch() {
    // Follow keeps the file's own speed: at 100 a beat is 0.6 s, so the clip is 0.5 s / 0.6 s = 0.83 beats long.
    let (_, len, part) = tone(&fender("08e follow"));
    assert!(near(len, 0.8333));
    assert!(near(part.seconds_per_beat.unwrap(), 0.6));
}

// What the readers get wrong or don't read yet. Each is a fix to make; take off the `#[ignore]` with it.

#[test]
fn bitwig_reversed_clip() {
    let part = tone(&bitwig("13 reversed")).2;
    assert!(part.reversed);
    assert!(near(part.seconds_per_beat.unwrap(), 60.0 / 128.0));
    // It plays the whole tone, from its end back to its start.
    assert!(near(part.from, 0.0) && near(part.length, 1.0667));
    // Trimmed by a 16th on the left: what's cut is the tone's end, so it still reaches back to the start.
    let (start, _, part) = tone(&bitwig("14 trimmed"));
    assert!(near(start, 4.25) && part.reversed && near(part.from, 0.0) && near(part.length, 0.8167));
    assert!(!tone(&bitwig("12 gain and fades")).2.reversed);
}

#[test]
fn bitwig_stretch_modes() {
    // Each piece of audio has its own stretch mode; the recipe's tone starts in Raw.
    for (step, mode) in [
        ("04 audio clip", Stretch::Raw),
        ("09 stretch hd", Stretch::StretchHd),
        ("09b stretch old", Stretch::Stretch),
        ("09c elastique pro", Stretch::ElastiquePro),
        ("10 repitch", Stretch::Repitch),
        ("11 transpose", Stretch::StretchHd),
    ] {
        assert_eq!(tone(&bitwig(step)).2.stretch, Some(mode), "{step}");
    }
}

#[test]
fn bitwig_transpose_gain_and_fades() {
    let plain = bitwig("04 audio clip");
    let (_, _, part) = tone(&plain);
    assert!(near(part.semitones, 0.0) && near(part.gain_db, 0.0));
    let clip = &lane(&plain, LaneKind::Audio).clips[0];
    assert!(near(clip.fade_in, 0.0) && near(clip.fade_out, 0.0));

    assert!(near(tone(&bitwig("11 transpose")).2.semitones, 3.0));
    // Step 12 was made from 11, so it keeps the +3.
    let a = bitwig("12 gain and fades");
    let (_, _, part) = tone(&a);
    assert!(near(part.gain_db, -6.0) && near(part.semitones, 3.0));
    // The fades sit on the clip, not on the audio in it: a 16th each (Bitwig's unit looks like beats).
    let clip = &lane(&a, LaneKind::Audio).clips[0];
    assert!(near(clip.fade_in, 0.25) && near(clip.fade_out, 0.25));
}

#[test]
fn fl_stretched_clips_have_a_sane_length() {
    for step in ["11b stretch pro", "11c e2 generic", "11d e3 mono"] {
        // The whole tone plays over the clip: FL set the Time knob (a float in FL 25: 410 of 192 to a beat) and Mul
        // doubles it, so it spans 4.27 beats.
        let (_, len, part) = tone(&fl(step));
        assert!(near(len, 4.2708), "{step}: {len}");
        assert!(near(part.whole_file_beats.unwrap(), 4.2708), "{step}: {:?}", part.whole_file_beats);
    }
}

#[test]
fn fl_stretch_modes_and_pitch() {
    for (step, mode) in [
        // Resample with no length set plays at the sample's own speed whatever the tempo.
        ("04 audio clip", Stretch::Raw),
        ("11 stretch keep pitch", Stretch::ElastiquePro),
        ("11b stretch pro", Stretch::StretchPro),
        ("11c e2 generic", Stretch::Elastique),
        ("11d e3 mono", Stretch::ElastiqueSolo),
    ] {
        let part = tone(&fl(step)).2;
        assert_eq!((part.stretch, part.semitones), (Some(mode), 0.0), "{step}");
    }
    // Resampled to twice as long, it sounds an octave down.
    let part = tone(&fl("10 stretch resample")).2;
    assert!(part.stretch == Some(Stretch::Raw) && near(part.semitones, -12.0), "{part:?}");
    // Up 300 cents in Auto mode (FL's 5): three semitones, at the sample's own speed.
    let part = tone(&fl("12 pitch")).2;
    assert!(near(part.seconds_per_beat.unwrap(), 60.0 / 128.0) && near(part.semitones, 3.0), "{part:?}");
    assert_eq!(part.stretch, Some(Stretch::Auto));
}

#[test]
fn fl_clip_fades() {
    // Step 14b: the clip's own fades, dragged by hand to about a 16th each (112 and 127 milliseconds at 128 BPM), in
    // both views.
    let views = arrangement::read(&save("fl", "MO Recipe 14b clip fades.flp")).unwrap();
    assert_eq!(views.len(), 2);
    for view in &views {
        let clip = &lane(view, LaneKind::Audio).clips[0];
        assert!(near(clip.fade_in, 0.2396) && near(clip.fade_out, 0.2708), "{}: {clip:?}", view.view);
    }
    // A clip with none has none.
    let plain = fl("04 audio clip");
    let clip = &lane(&plain, LaneKind::Audio).clips[0];
    assert_eq!((clip.fade_in, clip.fade_out), (0.0, 0.0));
}

#[test]
fn fender_timestretch_plays_the_file_at_its_own_tempo() {
    // Fender Studio took the half-second tone for one beat at 120. Stretched, that beat is spread over one of the
    // song's beats whatever the song's tempo: half a second of the file to a beat, and the event one beat long.
    for (step, mode) in [
        ("04 audio clip", Stretch::Elastique),
        ("08 stretch sound", Stretch::Elastique),
        ("08b stretch drums", Stretch::Elastique),
        ("08c stretch solo", Stretch::ElastiqueSolo),
        ("08d stretch tape", Stretch::Repitch),
    ] {
        let (_, len, part) = tone(&fender(step));
        assert!(near(len, 1.0) && near(part.seconds_per_beat.unwrap(), 0.5), "{step}: {len} {part:?}");
        assert_eq!(part.stretch, Some(mode), "{step}");
    }
    // Speedup 2 plays it twice as fast: the whole tone in half a beat.
    let (_, len, part) = tone(&fender("08f speedup"));
    assert!(near(len, 0.5) && near(part.seconds_per_beat.unwrap(), 1.0));
    // Follow moves the event with the tempo but plays it at its own speed.
    assert_eq!(tone(&fender("08e follow")).2.stretch, Some(Stretch::Raw));
}

#[test]
fn fender_transpose_gain_and_fades() {
    let (_, _, part) = tone(&fender("08f speedup"));
    assert!(near(part.semitones, 0.0) && near(part.gain_db, 0.0));
    assert!(near(tone(&fender("09 transpose")).2.semitones, 3.0));
    // Step 10 was made from 9, so it keeps the +3. The save's gain is 0.8 of the sound, which is -1.9 dB.
    let a = fender("10 gain and fades");
    let (_, _, part) = tone(&a);
    assert!(near(part.semitones, 3.0) && near(part.gain_db, -1.932), "{part:?}");
    // The fades are saved in seconds (0.091 and 0.071); at 128 a beat is 0.469 s.
    let clip = &lane(&a, LaneKind::Audio).clips[0];
    assert!(near(clip.fade_in, 0.1944) && near(clip.fade_out, 0.1523), "{} {}", clip.fade_in, clip.fade_out);
    let plain = fender("09 transpose");
    let clip = &lane(&plain, LaneKind::Audio).clips[0];
    assert!(near(clip.fade_in, 0.0) && near(clip.fade_out, 0.0));
}

#[test]
fn fender_bus() {
    // Step 6 sends the Mai Tai track to a new bus; before that it goes to the main output. The track is named after
    // its instrument, so the instrument isn't said again.
    assert_eq!(lane(&fender("05 drum pads"), LaneKind::Instrument).detail, "");
    assert_eq!(lane(&fender("06 mixer"), LaneKind::Instrument).detail, "To Bus 1");
    assert_eq!(lane(&fender("06 mixer"), LaneKind::Audio).detail, "");
}

#[test]
fn bitwig_time_signature_and_marker() {
    let a = bitwig("16 time signature and marker");
    assert_eq!(a.beats_per_bar, 3);
    assert_eq!(a.markers.len(), 1);
    assert_eq!(a.markers[0].name, "Verse");
    // The save has the cue marker on beat 4, where the tone starts.
    assert!(near(a.markers[0].beat, 4.0));
}

#[test]
fn bitwig_automation() {
    let a = bitwig("03 automation");
    let points = |name: &str| -> Vec<(f64, f64)> {
        let lane = a.lanes.iter().find(|l| l.kind == LaneKind::Automation && l.name == name).unwrap();
        lane.clips.iter().flat_map(|c| c.points.iter().map(move |p| (c.start + p.beat, p.value))).collect()
    };
    // The volume goes from 0 dB to -6 dB over bar 1. Bitwig saves the cube root of the level, and the top of its
    // fader (1 here) is +6 dB.
    let db = |v: f64| 60.0 * (v * 2f64.cbrt()).log10();
    let volume = points("Volume");
    assert!(near(volume[0].0, 0.0) && near(volume[1].0, 4.0), "{volume:?}");
    assert!(db(volume[0].1).abs() < 0.01 && (db(volume[1].1) + 6.0).abs() < 0.1, "{volume:?}");
    // The pan holds one side to bar 2, then crosses to the other by bar 3.
    assert_eq!(points("Pan"), [(0.0, 1.0), (4.0, 1.0), (8.0, 0.0)]);
    // The tempo holds 128 to bar 2, then climbs to 140 at bar 3: 0 to 1 is Bitwig's 20 to 666 BPM.
    let tempo: Vec<_> = points("Tempo").iter().map(|(beat, v)| (*beat, 20.0 + 646.0 * v)).collect();
    assert_eq!(tempo.len(), 3);
    assert!(near(tempo[0].0, 0.0) && near(tempo[1].0, 4.0) && near(tempo[2].0, 8.0), "{tempo:?}");
    assert!(near(tempo[0].1, 128.0) && near(tempo[1].1, 128.0) && near(tempo[2].1, 140.0), "{tempo:?}");
    // The curves sit under their track and say what they move.
    let names: Vec<_> = a.lanes.iter().map(|l| (l.name.as_str(), l.detail.as_str(), l.depth)).collect();
    assert_eq!(
        names,
        [
            ("Tempo", "Song tempo", 0),
            ("Polysynth", "Polysynth", 0),
            ("Volume", "Moves Polysynth", 1),
            ("Pan", "Moves Polysynth", 1)
        ]
    );
    // A project whose settings never move has no curves.
    assert!(bitwig("02 notes").lanes.iter().all(|l| l.kind != LaneKind::Automation));
}

#[test]
fn fender_automation() {
    let a = fender("03 automation");
    let points = |name: &str| -> Vec<(f64, f64)> {
        let lane = a.lanes.iter().find(|l| l.kind == LaneKind::Automation && l.name == name).unwrap();
        lane.clips.iter().flat_map(|c| c.points.iter().map(move |p| (c.start + p.beat, p.value))).collect()
    };
    // The volume goes from 0 dB to -6 dB over bar 1. Fender's 1 is +10 dB, so 0 dB is 0.316 and -6 dB half of that.
    let db = |v: f64| 20.0 * v.log10() + 10.0;
    let volume = points("Volume");
    assert!(near(volume[0].0, 0.0) && near(volume[1].0, 4.0), "{volume:?}");
    assert!(db(volume[0].1).abs() < 0.01 && (db(volume[1].1) + 6.0).abs() < 0.1, "{volume:?}");
    // The pan goes from one side to the other from bar 2 (Solomon's hand left the end a 16th after bar 3).
    let pan = points("Pan L/R");
    assert_eq!(pan, [(4.0, 1.0), (8.25, 0.0)]);
    // The tempo holds 128 to bar 2, then climbs to 140 at bar 3: 0 to 1 is 10 to 400 BPM.
    let tempo: Vec<_> = points("Tempo").iter().map(|(beat, v)| (*beat, 10.0 + 390.0 * v)).collect();
    assert_eq!(tempo.len(), 3);
    assert!(near(tempo[0].0, 0.0) && near(tempo[1].0, 4.0) && near(tempo[2].0, 8.0), "{tempo:?}");
    assert!(near(tempo[0].1, 128.0) && near(tempo[1].1, 128.0) && near(tempo[2].1, 140.0), "{tempo:?}");
    // The curves sit under their track and say what they move.
    let names: Vec<_> = a.lanes.iter().map(|l| (l.name.as_str(), l.detail.as_str(), l.depth)).collect();
    assert_eq!(
        names,
        [
            ("Tempo", "Song tempo", 0),
            ("Mai Tai", "", 0),
            ("Volume", "Moves Mai Tai", 1),
            ("Pan L/R", "Moves Mai Tai", 1)
        ]
    );
    // A song whose settings never move has no curves.
    assert!(fender("02 notes").lanes.iter().all(|l| l.kind != LaneKind::Automation));
}

#[test]
fn bitwig_deactivated_track() {
    let off = bitwig("06 group and off");
    assert!(lane(&off, LaneKind::Audio).muted && lane(&off, LaneKind::Audio).disabled);
    assert!(off.lanes.iter().filter(|l| l.kind != LaneKind::Audio).all(|l| !l.muted && !l.disabled));
    let on = bitwig("05 drum machine");
    assert!(on.lanes.iter().all(|l| !l.muted && !l.disabled));
}

#[test]
fn fender_disabled_track() {
    // 06 is muted only; 06b is disabled too.
    let muted = fender("06 mixer");
    let disabled = fender("06b disabled");
    assert!(lane(&muted, LaneKind::Audio).muted && !lane(&muted, LaneKind::Audio).disabled);
    assert!(lane(&disabled, LaneKind::Audio).muted && lane(&disabled, LaneKind::Audio).disabled);
    assert!(!lane(&disabled, LaneKind::Instrument).disabled);
}

#[test]
fn bitwig_looped_audio_clip() {
    // Step 15 loops the trimmed tone over 3 beats. The loop is 1.07 beats (0 to 1.07) and the tone starts 0.25 into
    // it; the clip plays from its start marker at 0.25. So the first pass is 0.82 beats, and each pass after starts
    // 0.25 beats of silence later: three parts in 3 beats.
    let a = bitwig("15 looped");
    let clip = &lane(&a, LaneKind::Audio).clips[0];
    assert!(near(clip.length, 3.0));
    let parts: Vec<(f64, f64, f64)> = clip.audio.iter().map(|p| (p.start, p.length, p.from)).collect();
    assert_eq!(parts.len(), 3, "{parts:?}");
    for (part, start) in parts.iter().zip([0.0, 1.0667, 2.1333]) {
        assert!(near(part.0, start) && near(part.1, 0.8167) && near(part.2, 0.0), "{parts:?}");
    }
}
