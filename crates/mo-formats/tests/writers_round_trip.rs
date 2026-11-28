//! Every recipe save (Solomon's hand-made test projects, rule 4) goes through both writers, and each copy is read back
//! with the app's own readers: what the copy says must be what the save said.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

use mo_formats::ableton::write as live;
use mo_formats::arrangement::{
    self, Arrangement, AudioPart, LaneKind, PreviewClip, PreviewLane, PreviewMarker, PreviewNote, TimeSignature,
};
use mo_formats::fl;
use mo_formats::studio_one::write as s1;
use proptest::prelude::*;

fn saves() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recipe");
    let mut all = Vec::new();
    for daw in ["fl", "bitwig", "studio-one", "ableton"] {
        let Ok(dir) = std::fs::read_dir(root.join(daw)) else { continue };
        for entry in dir {
            let path = entry.unwrap().path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
            if ["flp", "bwproject", "song", "als"].contains(&ext) {
                all.push(path);
            }
        }
    }
    all.sort();
    all
}

/// Every sample is a 4-second stereo file at 44.1 kHz, as far as the Studio One writer asks.
fn any_format(_: &str) -> Option<s1::SampleFormat> {
    Some(s1::SampleFormat { rate: 44_100, channels: 2, seconds: 4.0 })
}

/// How finely a copy keeps a time: FL Studio counts 96ths of a beat (its own new projects' setting, which the copy
/// keeps), Studio One far finer.
#[derive(Clone, Copy)]
struct Grid(f64);

type Note = (i64, u8, i64, u16, bool);

const FL: Grid = Grid(96.0);
const FINE: Grid = Grid(960.0);

impl Grid {
    fn tick(self, beats: f64) -> i64 {
        (beats * self.0).round() as i64
    }
}

/// Every note in the song, as (where it starts, key, how long, velocity, muted), sorted. A step note's length is
/// the DAW's to choose (FL plays a step to its sample's end), so it isn't compared; a muted clip or track mutes its
/// notes.
fn notes(song: &Arrangement, grid: Grid) -> Vec<Note> {
    let mut all: Vec<_> = song
        .lanes
        .iter()
        .flat_map(|l| l.clips.iter().map(move |c| (l, c)))
        .flat_map(|(l, c)| {
            c.notes.iter().map(move |n| {
                let length = if n.step { -1 } else { grid.tick(n.length) };
                (grid.tick(c.start + n.start), n.key, length, n.velocity, n.muted || c.muted || l.muted)
            })
        })
        .collect();
    all.sort_unstable();
    all
}

/// Where every clip of notes is, empty ones too, as (start, muted), sorted. A clip of an FL pattern that holds
/// several instruments' parts is a clip for each in a Studio One copy, so only places are compared.
fn note_clips(song: &Arrangement, grid: Grid) -> Vec<(i64, bool)> {
    let mut all: Vec<_> = song
        .lanes
        .iter()
        .filter(|l| l.kind != LaneKind::Automation)
        .flat_map(|l| &l.clips)
        .filter(|c| c.audio.is_empty() && c.points.is_empty())
        .map(|c| (grid.tick(c.start), c.muted))
        .collect();
    all.sort_unstable();
    all.dedup();
    all
}

/// Every audio part, as (the file's name, where it starts, how long it plays, its fades), sorted.
fn audio(song: &Arrangement, grid: Grid) -> Vec<(String, i64, i64, i64, i64)> {
    let mut all: Vec<_> = song
        .lanes
        .iter()
        .flat_map(|l| &l.clips)
        .flat_map(|c| {
            c.audio.iter().map(move |a| {
                let name = a.path.rsplit(['/', '\\']).next().unwrap_or_default().to_owned();
                let (fade_in, fade_out) = c.fades_of(a);
                (name, grid.tick(c.start + a.start), grid.tick(a.length), grid.tick(fade_in), grid.tick(fade_out))
            })
        })
        .collect();
    all.sort();
    all
}

/// What Studio One keeps of each audio part besides where it is: where in the file it starts (ms), its speed
/// (thousandths of the file a beat), transpose (cents), gain (hundredths of a dB) and its fades (960ths of a beat,
/// shared out as `PreviewClip::fades_of` says).
fn audio_detail(song: &Arrangement) -> Vec<(i64, i64, i64, i64, i64, i64)> {
    let r = |x: f64, by: f64| if x.is_finite() { (x * by).round() as i64 } else { i64::MIN };
    let mut all: Vec<_> = song
        .lanes
        .iter()
        .flat_map(|l| &l.clips)
        .flat_map(|c| {
            c.audio.iter().map(move |a| {
                let (fade_in, fade_out) = c.fades_of(a);
                (
                    r(a.from, 1000.0),
                    r(a.rate(4.0), 1000.0),
                    r(a.semitones, 100.0),
                    r(a.gain_db, 100.0),
                    r(fade_in, 960.0),
                    r(fade_out, 960.0),
                )
            })
        })
        .collect();
    all.sort_unstable();
    all
}

/// Every marker, as (where, its name). A tab or line break in a name may come back as a space (the Studio One copy
/// writes names on one line), spaces around a name may go (Live's reader trims them), and a marker with no name comes back with its number ("#2"), so those aren't compared.
fn markers(song: &Arrangement, grid: Grid) -> Vec<(i64, String)> {
    let name = |n: usize, name: &str| {
        let line = name.replace(['\t', '\n', '\r'], " ").trim().to_owned();
        if line.trim().is_empty() || line == format!("#{}", n + 1) { "unnamed".into() } else { line }
    };
    song.markers.iter().enumerate().map(|(n, m)| (grid.tick(m.beat), name(n, &m.name))).collect()
}

/// What differs between the save and its copy, in words.
fn differences(save: &Arrangement, copy: &Arrangement, grid: Grid) -> Vec<String> {
    let mut out = Vec::new();
    if (save.tempo - copy.tempo).abs() > 1e-6 {
        out.push(format!("tempo {} became {}", save.tempo, copy.tempo));
    }
    if save.signature != copy.signature {
        out.push(format!("time signature {:?} became {:?}", save.signature, copy.signature));
    }
    let (a, b) = (notes(save, grid), notes(copy, grid));
    // FL keeps a velocity as 0 to 128, so it may move by up to half of one of those steps; Live's softest is 1 of 127.
    let soft = |v: u16| i32::from(v.max(65_535 / 127));
    let near = |x: &Note, y: &Note| {
        (x.0, x.1, x.4) == (y.0, y.1, y.4)
            && (x.2 == y.2 || x.2 == -1)
            && (soft(x.3) - soft(y.3)).abs() <= 65_535 / 256 + 1
    };
    if a.len() != b.len() || !all_paired(&a, &b, near) {
        out.push(format!("notes: {} became {}; first apart: {:?}", a.len(), b.len(), first_apart(&a, &b)));
    }
    let (a, b) = (note_clips(save, grid), note_clips(copy, grid));
    if a != b {
        out.push(format!("clips of notes {a:?} became {b:?}"));
    }
    let (a, b) = (audio(save, grid), audio(copy, grid));
    if a != b {
        out.push(format!("audio {a:?} became {b:?}"));
    }
    let (a, b) = (markers(save, grid), markers(copy, grid));
    if a != b {
        out.push(format!("markers {a:?} became {b:?}"));
    }
    let empty = save.lanes.iter().all(|l| l.clips.is_empty());
    out.extend(
        copy.notes
            .iter()
            .filter(|n| !save.notes.contains(n) && !(empty && n.starts_with("Nothing is on")))
            .map(|n| format!("reader says: {n}")),
    );
    out
}

/// Whether each of the save's notes can have a note of the copy's that's near it, no two the same one. A step note
/// is near a note of any length, so the first near one isn't always the one to take: where notes share a place and a
/// key, the pairing that leaves none out is looked for. Both lists are sorted, so a note's near ones start at its tick.
fn all_paired(a: &[Note], b: &[Note], near: impl Fn(&Note, &Note) -> bool) -> bool {
    let may_take: Vec<Vec<usize>> = a
        .iter()
        .map(|x| {
            let from = b.partition_point(|y| y.0 < x.0);
            (from..b.len()).take_while(|&j| b[j].0 == x.0).filter(|&j| near(x, &b[j])).collect()
        })
        .collect();
    let mut taken_by = vec![None; b.len()];
    let mut tried = vec![false; b.len()];
    (0..a.len()).all(|i| {
        tried.fill(false);
        takes_one(i, &may_take, &mut taken_by, &mut tried)
    })
}

/// Gives the save's note `i` one of the copy's notes it may take: a free one, or one whose holder can take another.
fn takes_one(i: usize, may_take: &[Vec<usize>], taken_by: &mut [Option<usize>], tried: &mut [bool]) -> bool {
    for &j in &may_take[i] {
        if std::mem::replace(&mut tried[j], true) {
            continue;
        }
        let free = match taken_by[j] {
            Some(holder) => takes_one(holder, may_take, taken_by, tried),
            None => true,
        };
        if free {
            taken_by[j] = Some(i);
            return true;
        }
    }
    false
}

fn first_apart<T: std::fmt::Debug + PartialEq>(a: &[T], b: &[T]) -> String {
    let at = a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    format!("{:?} vs {:?}", a.get(at), b.get(at))
}

/// Two notes in one place on one key, one of them a step: each is paired with its own copy. A step note pairs with
/// any length, and it once took the other note's copy, which left that note with none: the check on random songs
/// failed whenever a song had such a pair (a run on GitHub found one, 2026-11-28).
#[test]
fn a_step_note_and_a_note_in_one_place_are_each_paired_with_their_own_copy() {
    let note = |length: f64, velocity: u16, step: bool| PreviewNote {
        start: 1.0,
        length,
        key: 60,
        velocity,
        muted: false,
        step,
        part: 0,
    };
    let clip = |notes: Vec<PreviewNote>, parts: Vec<String>| PreviewClip {
        start: 4.0,
        length: 8.0,
        notes,
        parts,
        ..PreviewClip::default()
    };
    let mut drums = PreviewLane::new("Drums", LaneKind::Instrument);
    drums.clips = vec![clip(vec![note(2.0, 33_867, true)], vec!["Kick".into()])];
    let mut keys = PreviewLane::new("Keys", LaneKind::Instrument);
    keys.clips = vec![clip(vec![note(0.25, 33_611, false)], Vec::new())];
    let song = Arrangement {
        view: "made up",
        tempo: 120.0,
        signature: TimeSignature::new(4, 4).unwrap(),
        lanes: vec![drums, keys],
        markers: Vec::new(),
        notes: Vec::new(),
    };
    let copy = s1::write(&song, "Copy", &any_format);
    let back = arrangement::read_bytes(Path::new("x.song"), &copy.bytes);
    // The step is listed first of the save's two (its length isn't compared), and last of the copy's (it's the longer).
    let lengths = |song: &Arrangement| notes(song, FINE).iter().map(|n| n.2).collect::<Vec<_>>();
    assert_eq!((lengths(&song), lengths(&back[0])), (vec![-1, 240], vec![240, 1920]));
    assert_eq!(differences(&song, &back[0], FINE), Vec::<String>::new());
}

#[test]
fn every_recipe_save_comes_back_the_same_from_both_writers() {
    let mut report = Vec::new();
    let mut checked = 0;
    for path in saves() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        for (v, view) in arrangement::read(&path).unwrap().iter().enumerate() {
            let ext = path.extension().unwrap().to_str().unwrap();
            if ext != "flp" {
                let written = fl::write::write(view, "Copy", 46_000.0);
                let back = arrangement::read_bytes(Path::new("x.flp"), &written.bytes);
                for d in differences(view, &back[0], FL) {
                    report.push(format!("{name} [{v}] → FL: {d} (left out {:?})", written.left_out));
                }
                checked += 1;
            }
            if ext != "als" {
                let written = live::write(view, &any_format, Some("C:/Export/Song/Ableton Live"));
                let back = arrangement::read_bytes(Path::new("x.als"), &written.bytes);
                let mut found = differences(view, &back[0], FINE);
                let (a, b) = (audio_detail(view), audio_detail(&back[0]));
                if a != b {
                    found.push(format!("audio details {a:?} became {b:?}"));
                }
                for d in found {
                    report.push(format!("{name} [{v}] → Live: {d} (left out {:?})", written.left_out));
                }
                checked += 1;
            }
            if ext != "song" {
                let written = s1::write(view, "Copy", &any_format);
                let back = arrangement::read_bytes(Path::new("x.song"), &written.bytes);
                let mut found = differences(view, &back[0], FINE);
                let (a, b) = (audio_detail(view), audio_detail(&back[0]));
                if a != b {
                    found.push(format!("audio details {a:?} became {b:?}"));
                }
                for d in found {
                    report.push(format!("{name} [{v}] → Studio One: {d} (left out {:?})", written.left_out));
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 40, "{checked}");
    assert!(report.is_empty(), "\n{}", report.join("\n"));
}

// Made-up songs: anything a reader could hand the writers, on FL's grid of 96ths of a beat so both copies can be held
// to the exact place. Names carry what XML and FL's text need care with.

const NAMES: [&str; 6] = ["Lead", "", "Pad & \"Hook\" <1>", "Ünïcødé ✓", "  spaced  ", "Tab\there"];

fn on_grid(max: u32) -> impl Strategy<Value = f64> {
    (0..max).prop_map(|t| f64::from(t) / 96.0)
}

fn a_note(length: f64) -> impl Strategy<Value = PreviewNote> {
    let ticks = ((length * 96.0) as u32).max(1);
    (0..ticks, 1..=192u32, 0..=127u8, 1..=u16::MAX, any::<bool>()).prop_map(move |(at, len, key, velocity, muted)| {
        let start = f64::from(at) / 96.0;
        // Notes stay inside their clip, as a DAW keeps them.
        let length = (f64::from(len) / 96.0).min(f64::from(ticks - at) / 96.0);
        PreviewNote { start, length, key, velocity, muted, step: false, part: 0 }
    })
}

fn a_note_clip() -> impl Strategy<Value = PreviewClip> {
    (on_grid(96 * 64), 1..=16u32, 0..6usize, any::<bool>())
        .prop_flat_map(|(start, bars, name, muted)| {
            let length = f64::from(bars) * 4.0;
            (Just((start, length, name, muted)), prop::collection::vec(a_note(length), 0..12))
        })
        .prop_map(|((start, length, name, muted), notes)| PreviewClip {
            name: NAMES[name].into(),
            start,
            length,
            muted,
            notes,
            ..PreviewClip::default()
        })
}

fn an_audio_clip(tempo: f64) -> impl Strategy<Value = PreviewClip> {
    (on_grid(96 * 64), 1..=96u32 * 8, 1..=3usize, 0..3u32, 0..48u32, 0..48u32, any::<bool>(), 0..3usize).prop_map(
        move |(start, ticks, parts, from, fade_in, fade_out, muted, file)| {
            let piece = f64::from(ticks) / 96.0;
            let audio = (0..parts)
                .map(|i| AudioPart {
                    path: format!("C:/Samples/Loop {file}.wav"),
                    start: piece * i as f64,
                    length: piece,
                    from: f64::from(from) * 0.25,
                    // Played at the song's speed: FL's copy keeps nothing else.
                    seconds_per_beat: Some(60.0 / tempo),
                    ..AudioPart::default()
                })
                .collect();
            PreviewClip {
                name: format!("Loop {file}"),
                start,
                length: piece * parts as f64,
                muted,
                audio,
                fade_in: f64::from(fade_in) / 96.0,
                fade_out: f64::from(fade_out) / 96.0,
                ..PreviewClip::default()
            }
        },
    )
}

fn a_lane(tempo: f64) -> impl Strategy<Value = PreviewLane> {
    (any::<bool>(), 0..6usize, any::<bool>()).prop_flat_map(move |(audio, name, muted)| {
        let clips = if audio {
            prop::collection::vec(an_audio_clip(tempo), 0..4).boxed()
        } else {
            prop::collection::vec(a_note_clip(), 0..4).boxed()
        };
        clips.prop_map(move |clips| {
            let mut lane = PreviewLane::new(NAMES[name], if audio { LaneKind::Audio } else { LaneKind::Instrument });
            lane.muted = muted;
            lane.clips = clips;
            lane
        })
    })
}

fn a_song() -> impl Strategy<Value = Arrangement> {
    (4_000..=24_000u32, prop::sample::select(vec![(4, 4), (3, 4), (6, 8), (7, 8), (5, 4), (2, 2)]))
        .prop_flat_map(|(hundredths, (top, bottom))| {
            let tempo = f64::from(hundredths) / 100.0;
            let markers = prop::collection::vec((on_grid(96 * 128), 0..6usize), 0..4);
            (Just((tempo, top, bottom)), prop::collection::vec(a_lane(tempo), 0..6), markers)
        })
        .prop_map(|((tempo, top, bottom), lanes, markers)| {
            let mut markers: Vec<_> =
                markers.into_iter().map(|(beat, n)| PreviewMarker { beat, name: NAMES[n].into() }).collect();
            markers.sort_by(|a, b| a.beat.total_cmp(&b.beat));
            markers.dedup_by(|a, b| a.beat == b.beat);
            Arrangement {
                view: "made up",
                tempo,
                signature: TimeSignature::new(top, bottom).unwrap(),
                lanes,
                markers,
                notes: Vec::new(),
            }
        })
}

/// What only an FL Studio save brings (a pattern holding several channels' parts, step notes) and what only the
/// Studio One copy keeps (audio at its own speed, transposed, with its own gain), laid over a made-up song by a list
/// of random choices.
fn with_extras(mut song: Arrangement, choices: &[u8]) -> Arrangement {
    let mut next = choices.iter().copied().cycle();
    let tempo = song.tempo;
    for clip in song.lanes.iter_mut().flat_map(|l| &mut l.clips) {
        if !clip.notes.is_empty() && next.next().unwrap_or(0) % 2 == 0 {
            clip.parts = vec!["Kick".into(), "Snare".into(), String::new()];
            for n in &mut clip.notes {
                n.part = u16::from(next.next().unwrap_or(0) % 3);
                n.step = next.next().unwrap_or(0) % 2 == 0;
            }
        }
        for part in &mut clip.audio {
            let pick = next.next().unwrap_or(0);
            part.semitones = [0.0, 3.0, -12.5, 7.25][usize::from(pick % 4)];
            part.gain_db = [0.0, -6.0, 3.5, f64::NEG_INFINITY][usize::from(pick / 4 % 4)];
            match pick / 16 % 3 {
                0 => {}
                1 => part.seconds_per_beat = Some(60.0 / tempo * 1.5),
                _ => (part.seconds_per_beat, part.whole_file_beats) = (None, Some(8.0)),
            }
        }
    }
    song
}

proptest! {
    // 256 songs, or PROPTEST_CASES when it's set (for a longer run by hand).
    #![proptest_config(ProptestConfig {
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|n| n.parse().ok()).unwrap_or(256),
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn a_made_up_song_comes_back_the_same_from_both_writers(song in a_song()) {
        let fl_copy = fl::write::write(&song, "Copy", 46_000.0);
        let back = arrangement::read_bytes(Path::new("x.flp"), &fl_copy.bytes);
        let found = differences(&song, &back[0], FL);
        prop_assert!(found.is_empty(), "FL: {found:#?}");
        let s1_copy = s1::write(&song, "Copy", &any_format);
        let back = arrangement::read_bytes(Path::new("x.song"), &s1_copy.bytes);
        let mut found = differences(&song, &back[0], FINE);
        let (a, b) = (audio_detail(&song), audio_detail(&back[0]));
        if a != b {
            found.push(format!("audio details {a:?} became {b:?}"));
        }
        prop_assert!(found.is_empty(), "Studio One: {found:#?}");
        let live_copy = live::write(&song, &any_format, None);
        let back = arrangement::read_bytes(Path::new("x.als"), &live_copy.bytes);
        let mut found = differences(&song, &back[0], FINE);
        let (a, b) = (audio_detail(&song), audio_detail(&back[0]));
        if a != b {
            found.push(format!("audio details {a:?} became {b:?}"));
        }
        prop_assert!(found.is_empty(), "Live: {found:#?}");
    }

    #[test]
    fn a_made_up_fl_song_comes_back_the_same_from_the_studio_one_writer(
        song in a_song(),
        choices in prop::collection::vec(any::<u8>(), 1..64),
    ) {
        let song = with_extras(song, &choices);
        let s1_copy = s1::write(&song, "Copy", &any_format);
        let back = arrangement::read_bytes(Path::new("x.song"), &s1_copy.bytes);
        let mut found = differences(&song, &back[0], FINE);
        let (a, b) = (audio_detail(&song), audio_detail(&back[0]));
        if a != b {
            found.push(format!("audio details {a:?} became {b:?}"));
        }
        prop_assert!(found.is_empty(), "Studio One: {found:#?}");
        let live_copy = live::write(&song, &any_format, None);
        let back = arrangement::read_bytes(Path::new("x.als"), &live_copy.bytes);
        let mut found = differences(&song, &back[0], FINE);
        let (a, b) = (audio_detail(&song), audio_detail(&back[0]));
        if a != b {
            found.push(format!("audio details {a:?} became {b:?}"));
        }
        prop_assert!(found.is_empty(), "Live: {found:#?}");
    }
}

/// A song past what each DAW holds, with numbers that aren't: both copies still open, and say what they left out.
#[test]
fn a_song_past_every_limit_still_makes_copies_that_open() {
    let notes = |n: u32| {
        vec![PreviewNote {
            start: 0.0,
            length: 1.0,
            key: (n % 128) as u8,
            velocity: 40_000,
            muted: false,
            step: false,
            part: 0,
        }]
    };
    let mut lanes: Vec<PreviewLane> = (0..520u32)
        .map(|i| {
            let mut lane = PreviewLane::new(format!("Track {i}"), LaneKind::Instrument);
            // Each a different clip: 520 tracks × 2 clips is past FL's 999 patterns.
            lane.clips = (0..2u32)
                .map(|c| PreviewClip {
                    start: f64::from(c) * 4.0,
                    length: 4.0,
                    notes: notes(i * 2 + c),
                    ..PreviewClip::default()
                })
                .collect();
            lane
        })
        .collect();
    lanes[0].clips.push(PreviewClip {
        start: f64::NAN,
        length: f64::INFINITY,
        notes: notes(1),
        ..PreviewClip::default()
    });
    lanes[1].clips.push(PreviewClip { start: -8.0, length: 4.0, notes: notes(2), ..PreviewClip::default() });
    lanes[2].clips.push(PreviewClip {
        start: 0.0,
        length: 4.0,
        fade_in: f64::NAN,
        fade_out: -1.0,
        audio: vec![AudioPart {
            path: String::new(),
            length: f64::NAN,
            from: f64::NAN,
            semitones: f64::NAN,
            gain_db: f64::NAN,
            ..AudioPart::default()
        }],
        ..PreviewClip::default()
    });
    for tempo in [5.0, 700.0, f64::NAN] {
        let song = Arrangement {
            view: "too much",
            tempo,
            signature: TimeSignature::FOUR_FOUR,
            lanes: lanes.clone(),
            markers: vec![
                PreviewMarker { beat: f64::NAN, name: "Lost".into() },
                PreviewMarker { beat: -1.0, name: "Before".into() },
            ],
            notes: Vec::new(),
        };
        let fl_copy = fl::write::write(&song, "Copy", 46_000.0);
        let back = &arrangement::read_bytes(Path::new("x.flp"), &fl_copy.bytes)[0];
        assert!((10.0..=522.0).contains(&back.tempo), "{}", back.tempo);
        assert_eq!(back.lanes.iter().filter(|l| l.kind != LaneKind::Automation).count(), 500);
        assert!(fl_copy.left_out.iter().any(|n| n.contains("999")), "{:?}", fl_copy.left_out);
        assert!(fl_copy.left_out.iter().any(|n| n.contains("500")), "{:?}", fl_copy.left_out);
        let s1_copy = s1::write(&song, "Copy", &any_format);
        let back = &arrangement::read_bytes(Path::new("x.song"), &s1_copy.bytes)[0];
        assert!((10.0..=400.0).contains(&back.tempo), "{}", back.tempo);
        assert!(back.note_count() >= 1040, "{}", back.note_count());
        assert!(!back.notes.iter().any(|n| n.contains("couldn't be read")), "{:?}", back.notes);
        if tempo.is_finite() {
            assert!(fl_copy.left_out.iter().any(|n| n.starts_with("The tempo")), "{:?}", fl_copy.left_out);
            assert!(s1_copy.left_out.iter().any(|n| n.starts_with("The tempo")), "{:?}", s1_copy.left_out);
        }
    }
}
