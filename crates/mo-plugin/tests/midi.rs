//! The MIDI tab's parts: a small MIDI index of made-up files, read as the plugin reads the app's, filtered with the
//! app's own filters, and a part written out as the MIDI file a drag carries.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use mo_midi::indexer::{self, Wanted};
use mo_midi::store::MidiStore;
use mo_midi::{Role, SourceKind};
use mo_plugin::midi::{self, Asked, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// A one-track MIDI file at 96 PPQ playing `keys` as quarter notes, one after another.
fn smf(keys: &[u8]) -> Vec<u8> {
    let mut track = Vec::new();
    for &key in keys {
        track.extend([0x00, 0x90, key, 100, 0x60, 0x80, key, 0]);
    }
    track.extend([0x00, 0xFF, 0x2F, 0x00]);
    let mut out = b"MThd".to_vec();
    out.extend([0, 0, 0, 6, 0, 0, 0, 1, 0, 96]);
    out.extend(b"MTrk");
    out.extend((track.len() as u32).to_be_bytes());
    out.extend(track);
    out
}

const BASS: &[u8] = &[36, 36, 43, 41, 36, 36, 43, 41];
const TUNE: &[u8] = &[72, 74, 76, 79, 76, 74, 72, 71];

/// A folder of made-up MIDI files and its index, made as the app's MIDI page makes one.
fn index(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Basslines")).unwrap();
    std::fs::write(dir.join("Basslines").join("Low 01.mid"), smf(BASS)).unwrap();
    std::fs::write(dir.join("Tune.mid"), smf(TUNE)).unwrap();
    let wanted: Vec<Wanted> = ["Basslines/Low 01.mid", "Tune.mid"]
        .iter()
        .map(|f| {
            let path = dir.join(f);
            let meta = std::fs::metadata(&path).unwrap();
            let mtime = meta.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
            Wanted { path: path.to_string_lossy().into_owned(), kind: SourceKind::File, mtime, size: meta.len() as i64 }
        })
        .collect();
    let db = dir.join("midi-index.db");
    let mut store = MidiStore::open(&db).unwrap();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let plan = indexer::plan(&wanted, &store.stamps().unwrap(), &[], false);
    let folders = vec![dir.to_string_lossy().into_owned()];
    indexer::run(&mut store, &plan, &folders, &pool, 2, &AtomicBool::new(false), &|_, _, _| {}).unwrap();
    db
}

fn parts(db: &Path, smallest: usize) -> std::sync::Arc<midi::Parts> {
    match midi::read_from(db, smallest) {
        Read::Ready(parts) => parts,
        Read::Not(why) => panic!("{why}"),
    }
}

#[test]
fn lists_filters_and_writes_a_part_out() {
    let db = index("mo-plugin-midi");
    let parts = parts(&db, 1);
    let (all, counts) = parts.view(&Asked::default());
    assert_eq!(all.len(), 2);
    assert_eq!(parts.lines(), 2);
    // The app's own filters: search words, and a kind.
    let low = Asked { filters: mo_app::midi::Filters { text: "low".into(), ..Default::default() }, ..Asked::default() };
    let (found, _) = parts.view(&low);
    assert_eq!(found.len(), 1);
    let found_part = parts.entry(found[0]).unwrap();
    assert!(mo_app::midi::title(found_part).contains("Low 01"), "{}", mo_app::midi::title(found_part));
    let role = parts.entry(all[0]).unwrap().role();
    let mut by_kind = Asked::default();
    by_kind.filters.toggle(mo_app::midi::KIND, role.plural());
    assert!(parts.view(&by_kind).0.iter().all(|&at| parts.entry(at).unwrap().role() == role));
    // The filter column has every kind, with counts.
    let chips = midi::chips(Some(&counts), &Asked::default());
    assert_eq!(chips.kind.chips.len() + chips.kind.more.len(), Role::ALL.len());

    // Heard: with no SoundFont there's nothing to play it with, and a file that isn't one is said so.
    assert_eq!(parts.sound(found[0], None, Some(120.0)).unwrap_err(), midi::NO_SOUNDFONT);
    let not_a_font = db.parent().unwrap().join("Tune.mid");
    assert!(parts.sound(found[0], Some(&not_a_font), None).unwrap_err().contains("isn't a SoundFont"));
    // Sent as MIDI, it needs no SoundFont: its eight quarter notes at 120 BPM, half a second each, as it loops.
    let (made, beats) = parts.notes_out(found[0], Some(120.0)).unwrap();
    let mo_plugin::audition::MadeLoop::Notes { seconds, notes } = made else { panic!("not notes") };
    assert_eq!(beats, Some((seconds * 2.0, 120.0)), "in beats at the DAW's tempo");
    assert_eq!(notes.len(), BASS.len());
    assert_eq!(notes.iter().map(|n| n.2).collect::<Vec<_>>(), BASS);
    let (start, end, _, velocity) = notes[1];
    assert!((start - 0.5).abs() < 1e-9 && (end - 1.0).abs() < 1e-9, "{start} to {end}");
    assert!((velocity - 100.0 / 127.0).abs() < 0.01, "{velocity}");

    // Its piano roll, as the app's MIDI page draws it: as big as asked, notes in the accent colour on the well.
    let colours = mo_app::piano_roll::Colours {
        well: 0x101010,
        accent: mo_app::shapes::Colour::rgb(0x3DDC97, 1.0),
        ..Default::default()
    };
    let roll = parts.roll(found[0], (200.0, 60.0, 2.0), &colours, false).unwrap();
    assert_eq!((roll.width, roll.height), (400, 120));
    assert_eq!(roll.rgba.len(), 400 * 120 * 4);
    assert!(roll.rgba.chunks(4).any(|p| p[1] > p[0].saturating_add(60)), "a note is drawn, in the accent colour");
    assert!(parts.roll(found[0], (0.0, 0.0, 1.0), &colours, false).is_err(), "no room, no roll");
    // The play head runs over the loop's bars: from the keyboard's edge to the right, where the first note starts.
    assert_eq!((roll.points, roll.bars), (200.0, (52.0, 200.0)));
    let first_note = (0..roll.width as usize)
        .find(|&x| {
            (0..roll.height as usize).any(|y| {
                let p = &roll.rgba[(y * roll.width as usize + x) * 4..][..4];
                p[1] > p[0].saturating_add(60)
            })
        })
        .unwrap();
    assert!((104..=106).contains(&first_note), "the first note starts where the bars do: {first_note}");
    // A part of four bars or fewer isn't drawn wider to follow: it's all in sight.
    assert_eq!(parts.roll(found[0], (200.0, 60.0, 2.0), &colours, true).unwrap().points, 200.0);

    // Dragged out: a MIDI file of its notes.
    let temp = db.parent().unwrap().join("Temp");
    std::fs::create_dir_all(&temp).unwrap();
    let file = parts.file(found[0], &temp).unwrap();
    let bytes = std::fs::read(&file).unwrap();
    assert!(bytes.starts_with(b"MThd"));
    assert_eq!(file.extension().and_then(|e| e.to_str()), Some("mid"));
}

#[test]
fn smallest_part_hides_short_ones() {
    let db = index("mo-plugin-midi-smallest");
    let parts = parts(&db, 9);
    // Both made-up parts have 8 notes.
    assert_eq!(parts.view(&Asked::default()).0.len(), 0);
    assert_eq!(parts.lines(), 0);
}
