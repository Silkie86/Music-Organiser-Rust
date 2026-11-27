//! CLAP's numbers kept in a file (`index/kept.rs`): the same searches, one file, only its own files removed, and a
//! file that can't be read any more finds nothing rather than stopping the app.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use mo_sounds::index::{
    Gathered, SoundIndex, VectorIndex, chroma_downweight, is_kept_name, remove_kept, remove_kept_prints,
    room_coming_back,
};

/// An empty folder of this test's own, under the build's temp folder.
fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-sounds-kept").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// `count` made-up fingerprints of 16 numbers, the ids with gaps; more than one part of the index's rows.
fn gathered(count: i64, salt: i64) -> Gathered {
    let mut g = Gathered::default();
    for id in 1..=count {
        let v: Vec<f32> = (0..16).map(|d| ((id * 31 + d * 17 + salt) % 101) as f32 / 7.0 - 5.0).collect();
        g.push(id * 3, &v);
    }
    g
}

fn held(count: i64, salt: i64) -> VectorIndex {
    VectorIndex::from_gathered_halved(gathered(count, salt), false, None)
}

fn kept_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> =
        fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    names
}

fn bits(v: &[(i64, f64)]) -> Vec<(i64, u64)> {
    v.iter().map(|(id, s)| (*id, s.to_bits())).collect()
}

#[test]
fn a_kept_index_finds_exactly_what_the_held_one_does() {
    let dir = folder("same");
    let was = held(9000, 0);
    let mut kept = was.clone();
    let done = kept.keep_in(&dir).unwrap().unwrap();
    assert!(done.written && kept.is_kept_in_a_file() && kept.is_halved() && !was.is_kept_in_a_file());
    assert_eq!(done.bytes, 4096 + 9000 * 16 * 2);
    assert_eq!(fs::metadata(&done.file).unwrap().len(), done.bytes);
    assert_eq!(kept_files(&dir), [done.file.file_name().unwrap().to_str().unwrap()]);
    assert!(is_kept_name(&kept_files(&dir)[0]));
    assert_eq!((kept.len(), kept.dim(), kept.ids()), (was.len(), was.dim(), was.ids()));
    for id in [3, 4096 * 3, 4097 * 3, 9000 * 3] {
        assert_eq!(kept.get(id), was.get(id), "{id}");
        let q = was.get(id).unwrap();
        assert_eq!(kept.dot_with(id, &q).map(f64::to_bits), was.dot_with(id, &q).map(f64::to_bits));
        assert_eq!(bits(&kept.nearest(&q, 40)), bits(&was.nearest(&q, 40)), "{id}");
    }
    assert_eq!((kept.get(4), kept.dot_with(4, &[0.0; 16])), (None, None));
    // Nothing more to do for one already in a file.
    assert!(kept.keep_in(&dir).is_none());
    // A scan's change makes it a held one again, with the same rows as the held one's.
    let gone = [6i64].into_iter().collect();
    assert_eq!(
        kept.with(&gone, Gathered::default(), false, None).nearest(&was.get(3).unwrap(), 9000).len(),
        was.with(&gone, Gathered::default(), false, None).nearest(&was.get(3).unwrap(), 9000).len()
    );
    assert!(!kept.with(&gone, Gathered::default(), false, None).is_kept_in_a_file());
}

#[test]
fn the_file_of_an_earlier_run_is_used_and_a_damaged_one_is_written_again() {
    let dir = folder("again");
    let was = held(5000, 0);
    let mut first = was.clone();
    let done = first.keep_in(&dir).unwrap().unwrap();
    drop(first);
    let mut second = was.clone();
    let again = second.keep_in(&dir).unwrap().unwrap();
    assert!(!again.written);
    assert_eq!((again.file.clone(), again.bytes, again.removed), (done.file.clone(), done.bytes, 0));
    drop(second);
    // One number changed in the file: it isn't trusted.
    let mut bytes = fs::read(&done.file).unwrap();
    let at = bytes.len() - 7;
    bytes[at] ^= 0x40;
    fs::write(&done.file, &bytes).unwrap();
    let mut third = was.clone();
    let mended = third.keep_in(&dir).unwrap().unwrap();
    assert!(mended.written);
    let q = was.get(3).unwrap();
    assert_eq!(bits(&third.nearest(&q, 25)), bits(&was.nearest(&q, 25)));
    drop(third);
    // Cut short: the same.
    fs::write(&done.file, &bytes[..bytes.len() - 2]).unwrap();
    let mut fourth = was.clone();
    assert!(fourth.keep_in(&dir).unwrap().unwrap().written);
    assert_eq!(kept_files(&dir).len(), 1);
}

#[test]
fn other_numbers_get_a_file_of_their_own_and_only_this_codes_files_are_removed() {
    let dir = folder("sweep");
    let mut old = held(300, 0);
    let old_file = old.keep_in(&dir).unwrap().unwrap().file;
    drop(old);
    // Yours, a folder with a name like ours, a half-written file from now and one from yesterday.
    fs::write(dir.join("sound-likeness-notes.rows"), b"mine").unwrap();
    fs::write(dir.join("sounds-rust.db"), b"mine").unwrap();
    fs::create_dir(dir.join("sound-likeness-0123456789abcdef.rows")).unwrap();
    let fresh = dir.join("sound-likeness-aaaaaaaaaaaaaaaa.rows.77-0.mo-part");
    fs::write(&fresh, b"being written").unwrap();
    let stale = dir.join("sound-likeness-bbbbbbbbbbbbbbbb.rows.78-3.mo-part");
    fs::write(&stale, b"left behind").unwrap();
    let yesterday = SystemTime::now() - Duration::from_secs(24 * 3600);
    fs::File::options().write(true).open(&stale).unwrap().set_modified(yesterday).unwrap();

    let mut new = held(300, 5);
    let done = new.keep_in(&dir).unwrap().unwrap();
    assert_ne!(done.file, old_file);
    assert_eq!(done.removed, 2, "the old numbers' file and the half-written one left behind");
    let mut want = vec![
        "sound-likeness-0123456789abcdef.rows".to_owned(),
        "sound-likeness-aaaaaaaaaaaaaaaa.rows.77-0.mo-part".to_owned(),
        "sound-likeness-notes.rows".to_owned(),
        "sounds-rust.db".to_owned(),
        done.file.file_name().unwrap().to_str().unwrap().to_owned(),
    ];
    want.sort();
    assert_eq!(kept_files(&dir), want);
    drop(new);
    // A library with no CLAP index keeps none.
    assert_eq!(remove_kept(&dir), 1);
    want.retain(|n| *n != done.file.file_name().unwrap().to_str().unwrap());
    assert_eq!(kept_files(&dir), want);
    assert_eq!(fs::read(dir.join("sound-likeness-notes.rows")).unwrap(), b"mine");
}

#[cfg(unix)]
/// The index built again while the app runs (a scan forgot sounds): the old file goes before the new one is written,
/// but the old index still reads it, so its room comes back only once that index is let go. The room check counts it
/// (the whole-app review of 26 Nov, SK-1: the new file was refused for room only the old one held).
#[test]
fn a_file_removed_while_an_index_still_reads_it_is_room_coming_back() {
    let dir = folder("coming-back");
    let mut old = held(300, 0);
    let first = old.keep_in(&dir).unwrap().unwrap();
    assert_eq!(room_coming_back(&dir), 0);
    let mut new = held(300, 1);
    let second = new.keep_in(&dir).unwrap().unwrap();
    assert_eq!(second.removed, 1);
    assert!(!first.file.exists());
    assert_eq!(room_coming_back(&dir), first.bytes);
    // Another index reading the same file counts it once; another folder's room isn't this one's.
    let also = old.clone();
    assert_eq!(room_coming_back(&dir), first.bytes);
    assert_eq!(room_coming_back(&dir.join("elsewhere")), 0);
    drop(old);
    assert_eq!(room_coming_back(&dir), first.bytes, "still read by the other");
    drop(also);
    assert_eq!(room_coming_back(&dir), 0);
    drop(new);
    assert_eq!(kept_files(&dir), [second.file.file_name().unwrap().to_str().unwrap()]);
}

#[test]
fn a_link_with_one_of_its_names_is_never_followed_or_removed() {
    let dir = folder("link");
    let yours = dir.join("yours.bin");
    fs::write(&yours, b"mine").unwrap();
    let link = dir.join("sound-likeness-cccccccccccccccc.rows");
    std::os::unix::fs::symlink(&yours, &link).unwrap();
    let mut index = held(100, 0);
    let done = index.keep_in(&dir).unwrap().unwrap();
    assert_eq!(done.removed, 0);
    assert_eq!(remove_kept(&dir), 1, "its own");
    assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    assert_eq!(fs::read(&yours).unwrap(), b"mine");
}

#[test]
fn a_file_that_cant_be_read_any_more_finds_nothing_and_says_so_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static TOLD: AtomicUsize = AtomicUsize::new(0);
    mo_sounds::index::on_kept_trouble(|what| {
        let ours = what.contains("sound-likeness-") || what.contains("sound-prints-");
        assert!(ours && what.contains("couldn't be read"), "{what}");
        // Only this test's own file: the tests run side by side, and another's cut-short file is told of here too.
        if Path::new(what.split(" couldn't be read").next().unwrap()).parent().is_some_and(|d| d.ends_with("gone")) {
            TOLD.fetch_add(1, Ordering::Relaxed);
        }
    });
    let dir = folder("gone");
    let was = held(9000, 0);
    let mut kept = was.clone();
    let done = kept.keep_in(&dir).unwrap().unwrap();
    let q = was.get(3).unwrap();
    assert_eq!(kept.nearest(&q, 5).len(), 5);
    // Cut short behind its back (as a drive that stops answering part way): the rows past the cut can't be read.
    fs::File::options().write(true).open(&done.file).unwrap().set_len(4096 + 100 * 16 * 2).unwrap();
    let before = TOLD.load(Ordering::Relaxed);
    assert_eq!(kept.get(9000 * 3), None);
    assert_eq!(kept.dot_with(9000 * 3, &q), None);
    assert!(kept.get(3).is_some(), "what can still be read is");
    // The first part reaches past the cut, and so does every other: nothing is found, nothing panics.
    assert!(kept.nearest(&q, 5).is_empty());
    // A scan's change leaves it as it is.
    let gone = [6i64].into_iter().collect();
    assert!(kept.with(&gone, Gathered::default(), false, None).is_kept_in_a_file());
    assert_eq!(TOLD.load(Ordering::Relaxed), before + 1, "once for the file");
}

#[test]
fn a_folder_that_isnt_there_leaves_the_index_as_it_was() {
    let dir = folder("missing").join("not made");
    let was = held(100, 0);
    let mut index = was.clone();
    assert!(index.keep_in(&dir).unwrap().is_err());
    assert!(!index.is_kept_in_a_file());
    assert_eq!(index, was);
    // Full precision and empty ones have nothing to keep.
    assert!(VectorIndex::from_gathered(gathered(10, 0), false, None).keep_in(&dir).is_none());
    assert!(VectorIndex::default().keep_in(&dir).is_none());
}

#[test]
fn the_sound_index_keeps_claps_numbers_only() {
    let dir = folder("sound-index");
    let mut index = SoundIndex::from_gathered(gathered(50, 1), gathered(50, 2), 50);
    assert!(index.deep && index.clap.is_some());
    let was = index.clone();
    // Shared with `was`: left alone.
    assert!(index.keep_clap_in(&dir).is_none());
    let q = was.active().get(3).unwrap().into_owned();
    let want = bits(&was.active().nearest(&q, 10));
    drop(was);
    let done = index.keep_clap_in(&dir).unwrap().unwrap();
    assert!(done.written && index.active().is_kept_in_a_file() && !index.features.is_kept_in_a_file());
    assert_eq!(bits(&index.active().nearest(&q, 10)), want);
    // None to keep without CLAP.
    assert!(SoundIndex::from_gathered(gathered(50, 1), Gathered::default(), 50).keep_clap_in(&dir).is_none());
}

#[test]
fn many_sounds_at_once_get_what_each_gets_on_its_own_however_the_numbers_are_held() {
    let dir = folder("each");
    let full = VectorIndex::from_gathered(gathered(9000, 0), false, None);
    let half = held(9000, 0);
    let mut kept = half.clone();
    kept.keep_in(&dir).unwrap().unwrap();
    let mut full_kept = full.clone();
    full_kept.keep_full_in(&dir).unwrap().unwrap();
    let q = half.get(3).unwrap().into_owned();
    // Every sound of the first part and a few of the others (so a part read whole, and sounds read one by one), out
    // of order, with ones the index doesn't have and one asked for twice.
    let mut ids: Vec<i64> = (1..=4096).rev().map(|n| n * 3).collect();
    ids.extend([9000 * 3, 4, 4097 * 3, -1, 6000 * 3, 3, 9000 * 3 + 3]);
    for (name, index) in [("full", &full), ("half", &half), ("kept", &kept), ("full kept", &full_kept)] {
        let dots = index.each(&ids, |k, row| (k, row.dot(&q).to_bits(), row.to_vec()));
        assert_eq!(dots.len(), ids.len(), "{name}");
        for (k, (id, got)) in ids.iter().zip(&dots).enumerate() {
            let want = index.get(*id).map(|v| (k, index.dot_with(*id, &q).unwrap().to_bits(), v.into_owned()));
            assert_eq!(got, &want, "{name} {id}");
        }
        assert_eq!(dots.iter().filter(|d| d.is_none()).count(), 3, "{name}");
        assert!(index.each(&[], |_, row| row.dot(&q)).is_empty(), "{name}");
        // One after another: the same sounds, in the order asked.
        let mut seen = Vec::new();
        index.in_order(&ids, |k, v| seen.push((k, v.to_vec())));
        let want: Vec<(usize, Vec<f32>)> =
            ids.iter().enumerate().filter_map(|(k, id)| Some((k, index.get(*id)?.into_owned()))).collect();
        assert_eq!(seen, want, "{name}");
    }
    // The file and the held numbers are the same bits, so they give the same answers.
    let bits = |index: &VectorIndex| index.each(&ids, |_, row| row.dot(&q).to_bits());
    assert_eq!(bits(&kept), bits(&half));
    assert_eq!(bits(&full_kept), bits(&full));
}

/// The analyser's fingerprints as the app builds them: standardised, chroma counting less, at full precision.
fn prints(count: i64, salt: i64) -> VectorIndex {
    VectorIndex::from_gathered(gathered(count, salt), true, Some(chroma_downweight))
}

#[test]
fn full_precision_numbers_kept_in_a_file_find_exactly_what_the_held_ones_do() {
    let dir = folder("full");
    let was = prints(9000, 0);
    let mut kept = was.clone();
    // Not as CLAP's are kept: that's for half precision only.
    assert!(kept.keep_in(&dir).is_none());
    let done = kept.keep_full_in(&dir).unwrap().unwrap();
    assert!(done.written && kept.is_kept_in_a_file() && !kept.is_halved());
    assert_eq!(done.bytes, 4096 + 9000 * 16 * 4);
    assert_eq!(fs::metadata(&done.file).unwrap().len(), done.bytes);
    let name = done.file.file_name().unwrap().to_str().unwrap().to_owned();
    assert!(name.starts_with("sound-prints-") && is_kept_name(&name), "{name}");
    assert_eq!((kept.len(), kept.dim(), kept.ids()), (was.len(), was.dim(), was.ids()));
    for id in [3, 4096 * 3, 4097 * 3, 9000 * 3] {
        let q = was.get(id).unwrap();
        assert_eq!(kept.get(id).unwrap().iter().map(|x| x.to_bits()).collect::<Vec<_>>(), {
            q.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
        });
        assert_eq!(kept.dot_with(id, &q).map(f64::to_bits), was.dot_with(id, &q).map(f64::to_bits));
        assert_eq!(bits(&kept.nearest(&q, 40)), bits(&was.nearest(&q, 40)), "{id}");
    }
    assert_eq!(kept.get(4), None);
    // Kept already: nothing to do. The same numbers again use the file there is.
    assert!(kept.keep_full_in(&dir).is_none());
    let mut again = was.clone();
    assert!(!again.keep_full_in(&dir).unwrap().unwrap().written);
    assert_eq!(again, kept);
    // A half-precision index has nothing to keep this way.
    assert!(held(10, 0).keep_full_in(&dir).is_none());
    assert!(VectorIndex::default().keep_full_in(&dir).is_none());
}

#[test]
fn each_kind_of_file_is_swept_on_its_own() {
    let dir = folder("two-kinds");
    let mut clap = held(300, 0);
    let clap_file = clap.keep_in(&dir).unwrap().unwrap().file;
    let mut old = prints(300, 0);
    let old_file = old.keep_full_in(&dir).unwrap().unwrap().file;
    assert_eq!(kept_files(&dir).len(), 2);
    // Other fingerprints: their file takes the old one's place, and CLAP's stays.
    let mut new = prints(300, 5);
    let done = new.keep_full_in(&dir).unwrap().unwrap();
    assert_eq!(done.removed, 1);
    assert!(clap_file.exists() && !old_file.exists() && done.file.exists());
    // Other CLAP numbers: the same the other way round.
    let mut clap_new = held(300, 5);
    let clap_done = clap_new.keep_in(&dir).unwrap().unwrap();
    assert_eq!(clap_done.removed, 1);
    assert!(!clap_file.exists() && done.file.exists() && clap_done.file.exists());
    // The old index, its file gone, still reads what it had open (or finds nothing): it never stops the app.
    let _ = old.nearest(&[1.0; 16], 5);
    // Each kind's own are removed alone (nothing holding them open by now).
    drop((clap, old, new, clap_new));
    assert_eq!(remove_kept(&dir), 1);
    assert!(done.file.exists() && !clap_done.file.exists());
    assert_eq!(remove_kept_prints(&dir), 1);
    assert!(kept_files(&dir).is_empty());
}

#[test]
fn fingerprints_in_a_file_take_a_scans_new_ones_as_held_ones_do() {
    use std::collections::HashSet;
    let dir = folder("full-with");
    let was = prints(5000, 0);
    let mut kept = was.clone();
    let first = kept.keep_full_in(&dir).unwrap().unwrap();
    // A scan: two sounds gone, one listened to again, one new.
    let without: HashSet<i64> = [6, 4097 * 3].into_iter().collect();
    let more = || {
        let mut g = Gathered::default();
        g.push(9, &[0.5; 16].map(|x: f32| x + 1.0));
        g.push(20_000, &(0..16).map(|d| d as f32 / 3.0).collect::<Vec<f32>>());
        g
    };
    let want = was.with(&without, more(), true, Some(chroma_downweight));
    let mut got = kept.with(&without, more(), true, Some(chroma_downweight));
    assert!(!got.is_kept_in_a_file(), "held again, as they are others now");
    assert_eq!(got, want);
    // Nothing changed: the same index, still in its file.
    let same = kept.with(&HashSet::new(), Gathered::default(), true, Some(chroma_downweight));
    assert!(same.is_kept_in_a_file());
    assert_eq!(same, kept);
    // Kept again: a file of the new numbers, the old one removed, and the same answers.
    let second = got.keep_full_in(&dir).unwrap().unwrap();
    assert!(second.written && second.removed == 1 && second.file != first.file);
    let q = want.get(20_000).unwrap();
    assert_eq!(bits(&got.nearest(&q, 30)), bits(&want.nearest(&q, 30)));
    assert_eq!(got.len(), 5000 - 2 + 1);
}

#[test]
fn the_sound_index_keeps_the_analysers_fingerprints_beside_claps() {
    let dir = folder("sound-index-both");
    let mut index = SoundIndex::from_gathered(gathered(50, 1), gathered(50, 2), 50);
    let was = index.clone();
    let q = was.features.get(3).unwrap().into_owned();
    let want = bits(&was.features.nearest(&q, 10));
    drop(was);
    assert!(index.keep_clap_in(&dir).unwrap().unwrap().written);
    let done = index.keep_features_in(&dir).unwrap().unwrap();
    assert!(done.written && done.removed == 0 && index.features.is_kept_in_a_file());
    assert_eq!(kept_files(&dir).len(), 2);
    assert_eq!(bits(&index.features.nearest(&q, 10)), want);
    assert!(index.keep_features_in(&dir).is_none());
    assert!(SoundIndex::default().keep_features_in(&dir).is_none());
}

#[test]
fn many_sounds_at_once_from_a_file_that_cant_be_read_get_none() {
    let dir = folder("each-cut");
    let mut kept = held(9000, 0);
    let done = kept.keep_in(&dir).unwrap().unwrap();
    // Cut short behind its back: the first part can still be read, the rest can't.
    let file = fs::OpenOptions::new().write(true).open(&done.file).unwrap();
    file.set_len(4096 + 4096 * 16 * 2).unwrap();
    drop(file);
    let ids: Vec<i64> = (1..=9000).map(|n| n * 3).collect();
    let got = kept.each(&ids, |_, row| row.to_vec().len());
    assert!(got[..4096].iter().all(|g| *g == Some(16)));
    assert!(got[4096..].iter().all(Option::is_none));
    // Cut part way through a part: the part can't be read whole, and its sounds that can still be read are.
    let file = fs::OpenOptions::new().write(true).open(&done.file).unwrap();
    file.set_len(4096 + 100 * 16 * 2).unwrap();
    drop(file);
    let got = kept.each(&ids, |_, row| row.to_vec().len());
    assert!(got[..100].iter().all(|g| *g == Some(16)));
    assert!(got[100..].iter().all(Option::is_none));
    // In order, the same ones.
    let mut seen = Vec::new();
    kept.in_order(&ids, |k, v| seen.push((k, v.len())));
    assert_eq!(seen, (0..100).map(|k| (k, 16)).collect::<Vec<_>>());
    // One by one too.
    assert_eq!(kept.each(&[3, 5000 * 3], |_, row| row.to_vec().len()), [Some(16), None]);
}

#[test]
fn fingerprints_in_a_file_that_cant_be_read_find_nothing_and_a_scan_says_so() {
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::AtomicBool;
    let dir = folder("full-cut");
    let mut index = SoundIndex::from_gathered(gathered(9000, 1), Gathered::default(), 9000);
    let done = index.keep_features_in(&dir).unwrap().unwrap();
    let q = index.features.get(3).unwrap().into_owned();
    assert_eq!(index.features.nearest(&q, 5).len(), 5);
    let quiet = |_: mo_sounds::galaxy::Step| {};
    let never = AtomicBool::new(false);
    let laid_out = mo_sounds::galaxy::update(&index, "features", HashMap::new(), false, &quiet, &never);
    let saved = laid_out.expect("laid out from the file").map;
    assert_eq!(saved.len(), 9000);

    // Cut short behind its back: only the first hundred sounds' numbers can still be read.
    fs::File::options().write(true).open(&done.file).unwrap().set_len(4096 + 100 * 16 * 4).unwrap();
    let kept = &index.features;
    assert_eq!(kept.get(9000 * 3), None);
    assert_eq!(kept.dot_with(9000 * 3, &q), None);
    assert!(kept.get(3).is_some(), "what can still be read is");
    assert!(kept.nearest(&q, 5).is_empty());
    let ids: Vec<i64> = (1..=9000).map(|n| n * 3).collect();
    let got = kept.each(&ids, |_, row| row.to_vec().len());
    assert!(got[..100].iter().all(|g| *g == Some(16)) && got[100..].iter().all(Option::is_none));
    // A scan's new sound can't be put in: said, so the app builds the index afresh from the library, rather than
    // being handed the same index as if it had worked.
    let more = || {
        let mut g = Gathered::default();
        g.push(50_000, &[0.25; 16]);
        g
    };
    let none = HashSet::new();
    assert!(kept.try_with(&none, more(), true, Some(chroma_downweight)).is_none());
    assert_eq!(&kept.with(&none, more(), true, Some(chroma_downweight)), kept);
    assert!(index.with_features(&none, &none, more()).is_none());
    // Nothing changed needs no read, and is still the same index.
    assert!(index.with_features(&none, &none, Gathered::default()).is_some());
    // The galaxy places nothing from numbers it couldn't read: afresh, or a few new stars.
    assert!(mo_sounds::galaxy::update(&index, "features", HashMap::new(), true, &quiet, &never).is_none());
    let mut fewer = saved.clone();
    fewer.remove(&(9000 * 3));
    assert!(mo_sounds::galaxy::update(&index, "features", fewer, false, &quiet, &never).is_none());
}

#[test]
fn the_galaxy_reads_many_fingerprints_from_files_as_it_reads_each() {
    use mo_sounds::galaxy::fingerprints;
    let dir = folder("galaxy-many");
    let held = SoundIndex::from_gathered(gathered(5000, 1), gathered(5000, 2), 5000);
    // CLAP's numbers a copy of their own: shared, they couldn't be put in a file.
    let mut kept = held.clone();
    kept.clap = held.clap.as_deref().cloned().map(std::sync::Arc::new);
    assert!(kept.keep_clap_in(&dir).unwrap().unwrap().written);
    assert!(kept.keep_features_in(&dir).unwrap().unwrap().written);
    let mut ids: Vec<i64> = (1..=5000).rev().map(|n| n * 3).collect();
    ids.extend([4, -1, 3]);
    for space in ["features", "clap", "clap+features"] {
        for (name, index) in [("held", &held), ("kept", &kept)] {
            let prints = fingerprints(index, space);
            let many = prints.many(&ids);
            assert_eq!(many.len(), ids.len());
            for (id, got) in ids.iter().zip(&many) {
                assert_eq!(got.as_ref(), prints.get(*id).as_ref(), "{space} {name} {id}");
            }
            assert!(!prints.unread(), "{space} {name}");
        }
        // The file holds the same bits.
        assert_eq!(fingerprints(&held, space).many(&ids), fingerprints(&kept, space).many(&ids), "{space}");
    }
}
