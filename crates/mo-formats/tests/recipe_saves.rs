//! Tests on saves FL Studio, Bitwig and Fender Studio (Studio One) really wrote, made by hand from the recipe in
//! `tests/fixtures/recipe/README.md`. Each test says what the save should read as. The `#[ignore]` ones are what the
//! readers get wrong or don't read yet: fix the reader, then take the `#[ignore]` off.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use mo_formats::arrangement::{self, Arrangement, AudioPart, LaneKind, PreviewLane};

fn save(daw: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recipe").join(daw).join(name)
}

/// The first view of a save (the DAW's own arrangement).
fn read(daw: &str, name: &str) -> Arrangement {
    let path = save(daw, name);
    let raw = std::fs::read(&path).unwrap();
    arrangement::read_bytes(&path, &raw).into_iter().next().unwrap()
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
                let raw = std::fs::read(&path).unwrap();
                assert!(!arrangement::read_bytes(&path, &raw).is_empty(), "{}", path.display());
            }
        }
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
    // MO Tone A is half a second: 1.0667 beats at 128 (FL rounds the clip to 1.0625, Fender's was cut to 1 beat).
    for (a, length) in
        [(fl("04 audio clip"), 1.0625), (bitwig("04 audio clip"), 1.0667), (fender("04 audio clip"), 1.0)]
    {
        let (start, len, part) = tone(&a);
        assert!(near(start, 4.0), "{start}");
        assert!(near(len, length), "{len}");
        assert!(part.path.ends_with("MO Tone A.wav"));
        assert!(near(part.from, 0.0));
        assert!(near(part.seconds_per_beat.unwrap(), 60.0 / 128.0));
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
    let (start, _, part) = tone(&fender("11 trimmed"));
    assert!(near(start, 4.25) && near(part.from, 0.2344));
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
    let depths: Vec<_> = a.lanes.iter().map(|l| (l.kind, l.depth)).collect();
    assert_eq!(depths[0], (LaneKind::Group, 0));
    assert_eq!(depths[1], (LaneKind::Instrument, 1));
    assert_eq!(depths[2], (LaneKind::Audio, 1));
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
#[ignore = "known gap: Bitwig reversed clips read as forwards with a negative rate"]
fn bitwig_reversed_clip() {
    let part = tone(&bitwig("13 reversed")).2;
    assert!(part.reversed);
    assert!(part.seconds_per_beat.unwrap() > 0.0);
}

#[test]
#[ignore = "known gap: FL Stretch pro / e2 / e3 clips read a whole-file length millions of beats long"]
fn fl_stretched_clips_have_a_sane_length() {
    for step in ["11b stretch pro", "11c e2 generic", "11d e3 mono"] {
        let part = tone(&fl(step)).2;
        let beats = part.whole_file_beats.or(part.seconds_per_beat.map(|s| 0.5 / s)).unwrap();
        assert!(beats < 100.0, "{step}: {beats}");
    }
}

#[test]
#[ignore = "known gap: Fender Timestretch events read as if they play at the song's speed"]
fn fender_timestretch_keeps_the_file_tempo() {
    let part = tone(&fender("08 stretch sound")).2;
    assert!(near(part.seconds_per_beat.unwrap(), 60.0 / 128.0));
}

#[test]
#[ignore = "known gap: Bitwig time signature and cue markers are not read"]
fn bitwig_time_signature_and_marker() {
    let a = bitwig("16 time signature and marker");
    assert_eq!(a.beats_per_bar, 3);
    assert_eq!(a.markers.len(), 1);
}

#[test]
#[ignore = "known gap: Bitwig and Fender arranger automation is not shown"]
fn bitwig_and_fender_automation() {
    for a in [bitwig("03 automation"), fender("03 automation")] {
        assert!(a.lanes.iter().any(|l| l.clips.iter().any(|c| !c.points.is_empty())));
    }
}

#[test]
#[ignore = "known gap: deactivated (Bitwig) and disabled (Fender) tracks are not marked"]
fn switched_off_tracks() {
    assert!(lane(&bitwig("06 group and off"), LaneKind::Audio).muted);
}
