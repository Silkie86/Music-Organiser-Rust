//! Bitwig's copies of Library multisamples, on folders made in the test build's own fixtures folder under fixed
//! names. The files are made-up bytes, not anyone's sounds.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use mo_space::bitwig::{
    FixedProject, LibraryCopy, LibraryIndex, fixed_projects, project_folders, remove_copies, remove_fixed_projects,
    sample_copies_in, saves_in,
};
use mo_space::clean::{Bin, Problem, Removed, What, undo};
use mo_space::key;

mod common;

use common::folder_link;

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A found copy with its paths as Space compares them (on Windows, a path written with `/` is found with `\`).
fn keyed(c: &LibraryCopy) -> (String, String, u64) {
    (key(&c.in_project), key(&c.library_file), c.bytes)
}

#[test]
fn a_projects_copy_is_found_only_when_its_the_same_as_a_library_file_byte_for_byte() {
    let dir = fixtures("mo-space-bitwig-copies");
    let (library, project) = (dir.join("Library/Multisamples"), dir.join("Projects/Garage"));
    let kicks = [1u8; 4096];
    let mut hats = [2u8; 4096];
    write(&library, "Kicks 1 Select.multisample", &kicks);
    // The same bytes under another name in the Library: the copy's own name wins.
    write(&library, "Older/Kicks Backup.multisample", &kicks);
    write(&library, "OHH 1 Select.multisample", &hats);
    write(&library, "Snare.multisample", b"snare");

    write(&project, "multi-samples/Kicks 1 Select-3-2-2-3-2-2-2-2-2.multisample", &kicks);
    // Same size as the Library's, one byte different (edited in the project): it stays the project's own.
    hats[100] = 9;
    write(&project, "multi-samples/OHH 1 Select-2-2-2-3-2-2-2.multisample", &hats);
    // A size no Library file has.
    write(&project, "multi-samples/Pad-2.multisample", b"pad");
    // Not a multisample, and not in the multisample folder.
    write(&project, "multi-samples/notes.txt", &kicks);
    write(&project, "samples/Kicks 1 Select.multisample", &kicks);

    let stop = AtomicBool::new(false);
    let index = LibraryIndex::new(&[dir.join("Library")], &stop).unwrap();
    let copies = index.copies_in(&project, &stop).unwrap();
    assert_eq!(
        copies.iter().map(keyed).collect::<Vec<_>>(),
        [keyed(&LibraryCopy {
            in_project: s(&project.join("multi-samples/Kicks 1 Select-3-2-2-3-2-2-2-2-2.multisample")),
            library_file: s(&library.join("Kicks 1 Select.multisample")),
            bytes: 4096,
        })]
    );

    // A project without the folder has none.
    assert!(index.copies_in(&dir.join("Projects"), &stop).unwrap().is_empty());
}

#[test]
fn a_copy_named_otherwise_takes_the_library_file_with_the_shortest_name() {
    let dir = fixtures("mo-space-bitwig-renamed");
    let (library, project) = (dir.join("Library"), dir.join("Project"));
    let bass = [7u8; 1000];
    write(&library, "Bass Long Name.multisample", &bass);
    // The shortest name, though not the shortest path.
    write(&library, "Deep/Down/Here/Bass.multisample", &bass);
    write(&project, "multi-samples/My Bass-2.multisample", &bass);
    let stop = AtomicBool::new(false);
    let copies = LibraryIndex::new(std::slice::from_ref(&library), &stop).unwrap().copies_in(&project, &stop).unwrap();
    assert_eq!(copies.len(), 1);
    assert_eq!(key(&copies[0].library_file), key(&s(&library.join("Deep/Down/Here/Bass.multisample"))));
}

/// Two Library folders hold the same file: the copy points at the first folder's (Settings' Library comes first),
/// though the other sorts before it by path (the whole-app review of 19 Nov, FS-19). With no name to match, the same.
#[test]
fn of_two_libraries_with_the_same_file_the_first_folder_given_wins() {
    let dir = fixtures("mo-space-bitwig-two-libraries");
    let (k, c, project) = (dir.join("K Library"), dir.join("C Library"), dir.join("Project"));
    let kicks = [3u8; 2048];
    let pad = [4u8; 3072];
    for library in [&k, &c] {
        write(library, "Kicks 1 Select.multisample", &kicks);
        write(library, "Pad.multisample", &pad);
    }
    write(&project, "multi-samples/Kicks 1 Select-2.multisample", &kicks);
    write(&project, "multi-samples/Warm-2.multisample", &pad);
    let stop = AtomicBool::new(false);
    let copies = LibraryIndex::new(&[k.clone(), c.clone()], &stop).unwrap().copies_in(&project, &stop).unwrap();
    let libraries: Vec<String> = copies.iter().map(|c| key(&c.library_file)).collect();
    assert_eq!(libraries, [key(&s(&k.join("Kicks 1 Select.multisample"))), key(&s(&k.join("Pad.multisample")))]);
    // Given the other way round, the other folder's.
    let copies = LibraryIndex::new(&[c.clone(), k], &stop).unwrap().copies_in(&project, &stop).unwrap();
    assert!(copies.iter().all(|copy| key(&copy.library_file).starts_with(&key(&s(&c)))), "{copies:?}");
}

/// A multisample in a folder below the project's multi-samples folder isn't one of Bitwig's copies: it's left out,
/// so it can't make every Fix of the project fail (the whole-app review of 19 Nov, FS-21).
#[test]
fn a_multisample_in_a_folder_below_multi_samples_isnt_a_copy() {
    let dir = fixtures("mo-space-bitwig-below");
    let (library, project) = (dir.join("Library"), dir.join("Project"));
    let pad = [5u8; 1024];
    write(&library, "Pad.multisample", &pad);
    write(&project, "multi-samples/Pads/Pad-2.multisample", &pad);
    write(&project, "multi-samples/Pad-2.multisample", &pad);
    let stop = AtomicBool::new(false);
    let copies = LibraryIndex::new(std::slice::from_ref(&library), &stop).unwrap().copies_in(&project, &stop).unwrap();
    assert_eq!(copies.len(), 1);
    assert_eq!(key(&copies[0].in_project), key(&s(&project.join("multi-samples/Pad-2.multisample"))));
}

#[test]
fn a_name_with_its_own_number_is_matched_before_a_shorter_one() {
    let dir = fixtures("mo-space-bitwig-own-number");
    let (library, project) = (dir.join("Library"), dir.join("Project"));
    let pad = [5u8; 300];
    write(&library, "Pad.multisample", &pad);
    write(&library, "Pad-10.multisample", &pad);
    write(&project, "multi-samples/Pad-10-2.multisample", &pad);
    let stop = AtomicBool::new(false);
    let copies = LibraryIndex::new(std::slice::from_ref(&library), &stop).unwrap().copies_in(&project, &stop).unwrap();
    assert_eq!(copies[0].library_file, s(&library.join("Pad-10.multisample")));
}

#[test]
fn another_projects_copies_never_count_as_library_files() {
    let dir = fixtures("mo-space-bitwig-other-project");
    let projects = dir.join("Projects");
    let kicks = [1u8; 512];
    write(&projects, "A/multi-samples/Kicks-2.multisample", &kicks);
    write(&projects, "B/multi-samples/Kicks-2.multisample", &kicks);
    // A template keeps its copies in a multi-samples folder too.
    write(&projects, "Keys.bwtemplate/project/multi-samples/Kicks-3.multisample", &kicks);
    let stop = AtomicBool::new(false);
    // The folder holding the projects given as a Library folder.
    let index = LibraryIndex::new(std::slice::from_ref(&projects), &stop).unwrap();
    assert!(index.copies_in(&projects.join("A"), &stop).unwrap().is_empty());
    assert!(index.copies_in(&projects.join("B"), &stop).unwrap().is_empty());
}

#[test]
fn a_library_file_changed_since_it_was_read_is_read_again() {
    let dir = fixtures("mo-space-bitwig-library-changed");
    let library = dir.join("Library");
    let old = [1u8; 256];
    write(&library, "Kicks.multisample", &old);
    write(&dir, "First/multi-samples/Kicks-2.multisample", &old);
    write(&dir, "Second/multi-samples/Kicks-2.multisample", &old);
    let stop = AtomicBool::new(false);
    let index = LibraryIndex::new(std::slice::from_ref(&library), &stop).unwrap();
    assert_eq!(index.copies_in(&dir.join("First"), &stop).unwrap().len(), 1);
    // Saved again in Bitwig: the same size, other bytes, a later time.
    fs::write(library.join("Kicks.multisample"), [2u8; 256]).unwrap();
    let later = SystemTime::now() + Duration::from_secs(3600);
    fs::File::options().write(true).open(library.join("Kicks.multisample")).unwrap().set_modified(later).unwrap();
    assert!(index.copies_in(&dir.join("Second"), &stop).unwrap().is_empty());
}

#[test]
fn the_next_look_lists_the_library_again_and_reads_again_only_what_changed() {
    let dir = fixtures("mo-space-bitwig-look-again");
    let library = dir.join("Library");
    let (kicks, pads) = ([1u8; 256], [3u8; 512]);
    write(&library, "Kicks.multisample", &kicks);
    write(&dir, "Song/multi-samples/Kicks-2.multisample", &kicks);
    write(&dir, "Song/multi-samples/Pads-2.multisample", &pads);
    let stop = AtomicBool::new(false);
    let mut index = LibraryIndex::new(std::slice::from_ref(&library), &stop).unwrap();
    assert_eq!(index.copies_in(&dir.join("Song"), &stop).unwrap().len(), 1);
    // Added to the Library since: found on the next look.
    write(&library, "Pads.multisample", &pads);
    index.list_again(std::slice::from_ref(&library), &stop).unwrap();
    assert_eq!(index.copies_in(&dir.join("Song"), &stop).unwrap().len(), 2);
    // A copy saved again in Bitwig since (the same size, other bytes, a later time) is read again: it's its own now.
    let copy = dir.join("Song/multi-samples/Kicks-2.multisample");
    fs::write(&copy, [2u8; 256]).unwrap();
    let later = SystemTime::now() + Duration::from_secs(3600);
    fs::File::options().write(true).open(&copy).unwrap().set_modified(later).unwrap();
    let copies = index.copies_in(&dir.join("Song"), &stop).unwrap();
    assert_eq!(
        copies.iter().map(|c| key(&c.library_file)).collect::<Vec<_>>(),
        [key(&s(&library.join("Pads.multisample")))]
    );
    // Stopped, the Library stays as it was listed.
    write(&library, "Kicks Again.multisample", &[2u8; 256]);
    assert!(index.list_again(std::slice::from_ref(&library), &AtomicBool::new(true)).is_err());
    assert_eq!(index.copies_in(&dir.join("Song"), &stop).unwrap().len(), 1);
}

#[test]
fn a_sample_copy_points_at_its_original_never_at_another_projects_copy() {
    let dir = fixtures("mo-space-bitwig-sample-copies");
    let snare = [5u8; 3000];
    write(&dir, "Library/Pads.multisample", b"pads");
    write(&dir, "Pack/ukg_snr_blaze.wav", &snare);
    // The same sound in another project's samples folder: that's a copy too, never the original.
    write(&dir, "Projects/Garage/Garage.bwproject", b"project");
    write(&dir, "Projects/Garage/samples/ukg_snr_blaze-2.wav", &snare);
    let project = dir.join("Projects/707");
    write(&project, "707.bwproject", b"project");
    write(&project, "samples/ukg_snr_blaze-2-2-3.wav", &snare);
    // In a folder below samples (Bitwig keeps only the top level): left alone.
    write(&project, "samples/old/ukg_snr_blaze-2.wav", &snare);
    let sounds = [
        dir.join("Pack/ukg_snr_blaze.wav"),
        dir.join("Projects/Garage/samples/ukg_snr_blaze-2.wav"),
        project.join("samples/ukg_snr_blaze-2-2-3.wav"),
    ];
    let named = |name: &str| -> Vec<String> {
        sounds.iter().filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == name)).map(|p| s(p)).collect()
    };
    let stop = AtomicBool::new(false);
    let index = LibraryIndex::new(&[dir.join("Library")], &stop).unwrap();
    let copies = sample_copies_in(&index, &project, &named, &stop).unwrap();
    assert_eq!(
        copies.iter().map(keyed).collect::<Vec<_>>(),
        vec![(
            key(&s(&project.join("samples/ukg_snr_blaze-2-2-3.wav"))),
            key(&s(&dir.join("Pack/ukg_snr_blaze.wav"))),
            3000
        )]
    );
    assert!(sample_copies_in(&index, &project, &named, &AtomicBool::new(true)).is_err());
}

#[test]
fn a_stop_stops_it() {
    let dir = fixtures("mo-space-bitwig-stop");
    write(&dir, "Library/A.multisample", b"a");
    write(&dir, "Project/multi-samples/A-2.multisample", b"a");
    let stopped = AtomicBool::new(true);
    assert!(LibraryIndex::new(&[dir.join("Library")], &stopped).is_err());
    let index = LibraryIndex::new(&[dir.join("Library")], &AtomicBool::new(false)).unwrap();
    assert!(index.copies_in(&dir.join("Project"), &stopped).is_err());
}

#[cfg(unix)]
#[test]
fn a_copy_that_is_a_link_or_a_multisample_folder_that_is_a_link_is_left_out() {
    let dir = fixtures("mo-space-bitwig-links");
    let a = [3u8; 64];
    write(&dir, "Library/A.multisample", &a);
    write(&dir, "Elsewhere/A-2.multisample", &a);
    fs::create_dir_all(dir.join("Project/multi-samples")).unwrap();
    std::os::unix::fs::symlink(dir.join("Library/A.multisample"), dir.join("Project/multi-samples/A-3.multisample"))
        .unwrap();
    fs::create_dir_all(dir.join("Linked")).unwrap();
    std::os::unix::fs::symlink(dir.join("Elsewhere"), dir.join("Linked/multi-samples")).unwrap();
    let stop = AtomicBool::new(false);
    let index = LibraryIndex::new(&[dir.join("Library")], &stop).unwrap();
    assert!(index.copies_in(&dir.join("Project"), &stop).unwrap().is_empty());
    assert!(index.copies_in(&dir.join("Linked"), &stop).unwrap().is_empty());
}

#[test]
fn a_library_folder_moved_away_with_a_link_in_its_place_is_still_read() {
    let dir = fixtures("mo-space-bitwig-library-moved");
    let kicks = [4u8; 256];
    write(&dir, "D/Packages/Kicks.multisample", &kicks);
    write(&dir, "Projects/Garage/multi-samples/Kicks-2.multisample", &kicks);
    // The packages folder was moved to another drive, and a link took its place (a junction on Windows, as Space
    // makes).
    let (moved, library) = (dir.join("D/Packages"), dir.join("installed-packages"));
    folder_link(&moved, &library);
    let stop = AtomicBool::new(false);
    let index = LibraryIndex::new(std::slice::from_ref(&library), &stop).unwrap();
    let copies = index.copies_in(&dir.join("Projects/Garage"), &stop).unwrap();
    // Found, under the path you know, through the link: it stays right if the folder is moved back.
    assert_eq!(
        copies.iter().map(keyed).collect::<Vec<_>>(),
        [keyed(&LibraryCopy {
            in_project: s(&dir.join("Projects/Garage/multi-samples/Kicks-2.multisample")),
            library_file: s(&library.join("Kicks.multisample")),
            bytes: 256,
        })]
    );
}

#[test]
fn the_project_folders_are_the_ones_with_a_bitwig_project_and_its_copies() {
    let dir = fixtures("mo-space-bitwig-project-folders");
    let projects = dir.join("Projects");
    // A project with copies, and its "(fixed)" save beside it: the folder is listed once.
    write(&projects, "Night Drive/Night Drive.bwproject", b"project");
    write(&projects, "Night Drive/Night Drive (fixed).bwproject", b"project");
    write(&projects, "Night Drive/multi-samples/Kicks-2.multisample", b"kicks");
    // Deeper down, and a project kept straight in the folder you gave.
    write(&projects, "2024/Garage/Garage.BWPROJECT", b"project");
    write(&projects, "2024/Garage/multi-samples/Pads.multisample", b"pads");
    write(&dir, "Loose/Loose.bwproject", b"project");
    write(&dir, "Loose/multi-samples/Bass.multisample", b"bass");
    // A folder with a project made for trying the app out beside one of yours is still yours.
    write(&projects, "2024/Garage/MO Recipe 05 drum machine.bwproject", b"project");
    // Not these: a project without copies, a multi-samples folder with no project, an FL Studio save beside one, a
    // project inside a project's copies, a project in a folder a move left behind, and a project made for trying the
    // app out (Solomon's F12 note, 25 Nov 2026).
    write(&projects, "MO Recipe 05 drum machine/MO Recipe 05 drum machine.bwproject", b"project");
    write(&projects, "MO Recipe 05 drum machine/multi-samples/MO Recipe Tones.multisample", b"tones");
    write(&projects, "Sketch/Sketch.bwproject", b"project");
    write(&projects, "Stems/multi-samples/Lead.multisample", b"lead");
    write(&projects, "Beat/Beat.flp", b"flp");
    write(&projects, "Beat/multi-samples/Hat.multisample", b"hat");
    write(&projects, "Night Drive/multi-samples/Inner/Inner.bwproject", b"project");
    write(&projects, "Inner/multi-samples/x.multisample", b"x");
    write(&dir, "Old (old copy)/Song/Song.bwproject", b"project");
    write(&dir, "Old (old copy)/Song/multi-samples/Kicks.multisample", b"kicks");
    let stop = AtomicBool::new(false);
    let roots = [projects.clone(), dir.join("Loose"), dir.join("Old (old copy)"), dir.join("Not there")];
    let found: Vec<String> = project_folders(&roots, &stop).unwrap().iter().map(|p| key(&s(p))).collect();
    let mut wanted =
        [projects.join("2024/Garage"), projects.join("Night Drive"), dir.join("Loose")].map(|p| key(&s(&p))).to_vec();
    wanted.sort();
    assert_eq!(found, wanted);

    // A folder of projects moved to another drive with a link in its place keeps the paths you know.
    let (moved, linked) = (dir.join("D/Moved"), dir.join("Linked"));
    write(&moved, "Song/Song.bwproject", b"project");
    write(&moved, "Song/multi-samples/Kicks.multisample", b"kicks");
    folder_link(&moved, &linked);
    let found = project_folders(std::slice::from_ref(&linked), &stop).unwrap();
    assert_eq!(found.iter().map(|p| key(&s(p))).collect::<Vec<_>>(), [key(&s(&linked.join("Song")))]);
    // A folder reached two ways is listed once: through the link and where it really is, and through roots inside
    // each other.
    let twice = [moved.clone(), linked.clone(), projects.clone(), projects.join("2024")];
    let found: Vec<String> = project_folders(&twice, &stop).unwrap().iter().map(|p| key(&s(p))).collect();
    let mut wanted =
        [moved.join("Song"), projects.join("2024/Garage"), projects.join("Night Drive")].map(|p| key(&s(&p))).to_vec();
    wanted.sort();
    assert_eq!(found, wanted);

    // A stop stops it.
    assert!(project_folders(&roots, &AtomicBool::new(true)).is_err());
}

/// A Bitwig project saved `minutes` after a fixed time.
fn saved(dir: &Path, rel: &str, minutes: u64) -> PathBuf {
    write(dir, rel, b"project");
    let path = dir.join(rel);
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + minutes * 60);
    fs::File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
    path
}

#[test]
fn a_project_folders_saves_are_listed_newest_first_each_with_its_fixed_project() {
    let dir = fixtures("mo-space-bitwig-saves");
    let song = dir.join("Song");
    let older = saved(&song, "Song.bwproject", 10);
    let newest = saved(&song, "Song v2.bwproject", 30);
    // The newest save's fixed project, written after it; and the older one's, written before it was saved again.
    let fixed = saved(&song, "Song v2 (fixed).bwproject", 40);
    let old_fixed = saved(&song, "Song (fixed).bwproject", 5);
    // Bitwig's auto-backups in a folder below are saves too. A fixed project whose save has gone is no one's, and a
    // Mac's "._" file isn't a save.
    let backup = saved(&song, "auto-backups/Song v2-1.bwproject", 20);
    saved(&song, "Gone (fixed 2).bwproject", 50);
    saved(&song, "._Song v2.bwproject", 60);
    write(&song, "multi-samples/Kicks-2.multisample", b"kicks");
    let stop = AtomicBool::new(false);
    type Seen = (PathBuf, bool, Option<(PathBuf, bool)>);
    let seen = |saves: &[mo_space::bitwig::Save]| -> Vec<Seen> {
        saves.iter().map(|s| (s.path.clone(), s.below, s.fixed.clone())).collect()
    };
    let saves = saves_in(&song, &stop).unwrap();
    assert_eq!(
        seen(&saves),
        [
            (newest.clone(), false, Some((fixed.clone(), true))),
            (older.clone(), false, Some((old_fixed.clone(), false))),
            (backup.clone(), true, None),
        ]
    );
    assert!(saves[0].is_fixed() && !saves[1].is_fixed() && !saves[2].is_fixed());

    // Of two fixed projects for one save the newest counts, and once the save is saved again it's out of date.
    let second = saved(&song, "Song v2 (fixed 2).bwproject", 45);
    saved(&song, "Song v2.bwproject", 50);
    let saves = saves_in(&song, &stop).unwrap();
    assert_eq!(saves[0].fixed, Some((second, false)));

    // A fixed project an older Fix wrote (before 11 Nov 2026) is out of date too, though it's newer than its save.
    let other = dir.join("Other");
    saved(&other, "Other.bwproject", 0);
    let early = other.join("Other (fixed).bwproject");
    write(&other, "Other (fixed).bwproject", b"project");
    let at = mo_space::bitwig::fix_writes_library_files() - Duration::from_secs(60);
    fs::File::options().write(true).open(&early).unwrap().set_modified(at).unwrap();
    let older_save = other.join("Other.bwproject");
    fs::File::options().write(true).open(&older_save).unwrap().set_modified(at - Duration::from_secs(600)).unwrap();
    assert_eq!(saves_in(&other, &stop).unwrap()[0].fixed, Some((early, false)));
    assert!(saves_in(&song, &AtomicBool::new(true)).is_err());
}

#[test]
fn a_library_folder_inside_the_project_never_counts() {
    let dir = fixtures("mo-space-bitwig-library-in-project");
    let project = dir.join("Projects/Garage");
    write(&project, "multi-samples/Kicks 1 Select-2.multisample", &[1u8; 64]);
    write(&project, "multi-samples/Kicks 1 Select.multisample", &[1u8; 64]);
    let stop = AtomicBool::new(false);
    // The project (or its multisample folder) given as a Library folder by mistake.
    for library in [project.clone(), project.join("multi-samples")] {
        let index = LibraryIndex::new(&[library], &stop).unwrap();
        assert!(index.copies_in(&project, &stop).unwrap().is_empty());
    }
}

/// While Solomon tests Fix: every "(fixed)" project Fix wrote goes to the bin in one go, and only those (each still as
/// Fix left it, which the app's note answers: here, all are); Put back brings them back.
#[test]
fn the_fixed_projects_fix_wrote_go_to_the_bin_and_come_back() {
    let dir = fixtures("mo-space-bitwig-fixed-projects");
    let (garage, other) = (dir.join("Projects/Garage"), dir.join("Projects/Other"));
    for name in ["Garage 2.bwproject", "Garage 2 (fixed).bwproject", "Garage 2 (fixed 3).bwproject"] {
        write(&garage, name, b"BtWg");
    }
    // Not Fix's: a save of yours, one whose save isn't beside it, another kind of file, and one in a folder below.
    write(&garage, "Garage 2 (fixed)(fixed).bwproject", b"BtWg");
    write(&garage, "Gone (fixed).bwproject", b"BtWg");
    write(&garage, "Garage 2 (fixed).txt", b"notes");
    write(&garage, "auto-backups/Garage 2 (fixed).bwproject", b"BtWg");
    write(&other, "Other.bwproject", b"BtWg");
    write(&other, "Other (fixed).bwproject", b"BtWg!");
    let stop = AtomicBool::new(false);
    let folders = [garage.clone(), dir.join("Projects/Missing"), other.clone()];
    let found = fixed_projects(&folders, &stop).unwrap();
    let fixed = [
        garage.join("Garage 2 (fixed 3).bwproject"),
        garage.join("Garage 2 (fixed).bwproject"),
        other.join("Other (fixed).bwproject"),
    ];
    assert_eq!(found, fixed);

    // One gone by the time it's sent, and one that was never Fix's, said plainly.
    let mut sent = found.clone();
    sent.push(garage.join("Gone (fixed).bwproject"));
    fs::remove_file(&fixed[0]).unwrap();
    let bin = FolderBin::new(&dir);
    let done = remove_fixed_projects(&sent, &|_, _| Ok(()), &bin, &stop);
    let gone: Vec<&str> = done.removed.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(gone, [s(&fixed[1]), s(&fixed[2])]);
    assert!(done.removed.iter().all(|r| r.what == What::FixedProject));
    assert_eq!(done.bytes(), 9);
    let why: Vec<&str> = done.problems.iter().map(|p| p.why.as_str()).collect();
    assert_eq!(
        why,
        ["It isn't there any more.", "The save it was written from isn't beside it, so it may not be Fix's: it stays.",]
    );
    assert!(garage.join("Garage 2.bwproject").is_file() && other.join("Other.bwproject").is_file(), "the saves stay");
    assert!(garage.join("auto-backups/Garage 2 (fixed).bwproject").is_file());
    let mut removed = done.removed.clone();
    assert!(bin.find(&mut removed).is_empty());
    assert!(undo(&mo_space::clean::Done { removed, ..done }, &bin).is_empty());
    assert!(fixed[1].is_file() && fixed[2].is_file(), "Put back brings them back");
}

/// One you've saved in since Fix wrote it is yours (rule 3): it stays where it is, with why, as the app's note of what
/// Fix wrote says (the whole-app review of 14 Nov, FX-5). The note is asked once for each, with the file's own size.
#[test]
fn a_fixed_project_saved_in_since_fix_wrote_it_stays() {
    let dir = fixtures("mo-space-bitwig-fixed-saved-in");
    let garage = dir.join("Projects/Garage");
    write(&garage, "Garage 2.bwproject", b"BtWg");
    write(&garage, "Garage 2 (fixed).bwproject", b"BtWg fixed");
    write(&garage, "Garage 2 (fixed 2).bwproject", b"BtWg fixed, then saved in");
    let as_left = garage.join("Garage 2 (fixed).bwproject");
    let saved_in = garage.join("Garage 2 (fixed 2).bwproject");
    let asked = RefCell::new(Vec::new());
    let as_fix_left = |path: &Path, meta: &fs::Metadata| {
        asked.borrow_mut().push((s(path), meta.len()));
        if path == saved_in { Err("Saved in since.".to_owned()) } else { Ok(()) }
    };
    let stop = AtomicBool::new(false);
    let found = fixed_projects(std::slice::from_ref(&garage), &stop).unwrap();
    assert_eq!(found, [saved_in.clone(), as_left.clone()]);
    let bin = FolderBin::new(&dir);
    let done = remove_fixed_projects(&found, &as_fix_left, &bin, &stop);
    let gone: Vec<&str> = done.removed.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(gone, [s(&as_left)]);
    assert_eq!(done.bytes(), 10);
    assert_eq!(done.problems, [Problem { path: s(&saved_in), why: "Saved in since.".to_owned() }]);
    assert!(saved_in.is_file() && !as_left.exists());
    assert_eq!(*asked.borrow(), [(s(&saved_in), 25), (s(&as_left), 10)]);
}

/// A folder standing in for the Recycle Bin.
struct FolderBin {
    dir: PathBuf,
    held: RefCell<Vec<(String, PathBuf)>>,
}

impl FolderBin {
    fn new(dir: &Path) -> Self {
        FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) }
    }
}

impl Bin for FolderBin {
    fn send(&self, path: &Path) -> Result<(), String> {
        let mut held = self.held.borrow_mut();
        let to = self.dir.join(held.len().to_string());
        fs::create_dir_all(&self.dir).unwrap();
        fs::rename(path, &to).map_err(|e| e.to_string())?;
        held.push((s(path), to));
        Ok(())
    }
    fn find(&self, removed: &mut [Removed]) -> Vec<Problem> {
        for r in removed {
            r.bin_id = self.held.borrow().iter().find(|(p, _)| key(p) == key(&r.real)).map(|(_, at)| s(at));
        }
        Vec::new()
    }
    fn put_back(&self, removed: &[Removed]) -> Vec<Problem> {
        for r in removed {
            let at =
                self.held.borrow().iter().find(|(_, at)| Some(s(at)) == r.bin_id).map(|(_, at)| at.clone()).unwrap();
            fs::rename(at, &r.path).unwrap();
        }
        Vec::new()
    }
}

/// Nothing else in the project folder names a copy.
fn named_nowhere(_: &Path, _: &Path) -> Option<String> {
    None
}

fn fixed<'a>(file: &'a Path, changed: &'a [(PathBuf, PathBuf)], library: &'a [PathBuf]) -> FixedProject<'a> {
    FixedProject { file, changed, library_folders: library, still_named: &named_nowhere }
}

#[test]
fn a_copy_the_fixed_project_no_longer_uses_goes_only_while_its_still_the_library_files_twin() {
    let dir = fixtures("mo-space-bitwig-remove");
    let (library, project) = (dir.join("Library"), dir.join("Projects/Garage"));
    let kicks = [1u8; 2048];
    write(&library, "Kicks 1 Select.multisample", &kicks);
    write(&library, "OHH 1 Select.multisample", &[2u8; 2048]);
    write(&project, "Garage.bwproject", b"original");
    write(&project, "Garage (fixed).bwproject", b"fixed");
    write(&project, "multi-samples/Kicks 1 Select-3-2.multisample", &kicks);
    // Edited in the project since it was found: the same size, other bytes.
    write(&project, "multi-samples/OHH 1 Select-2.multisample", &[3u8; 2048]);
    // Not in the multisample folder.
    write(&project, "samples/Kicks 1 Select.multisample", &kicks);
    // What it'd use instead is another project's copy, outside the Library.
    write(&dir, "Projects/Other/multi-samples/Kicks.multisample", &kicks);
    write(&project, "multi-samples/Kicks-4.multisample", &kicks);
    // Its Library file has gone.
    write(&project, "multi-samples/Snare-2.multisample", b"snare");
    // Another save in the folder still names it.
    write(&project, "multi-samples/Kicks-5.multisample", &kicks);

    let ms = project.join("multi-samples");
    let changed = vec![
        (ms.join("Kicks 1 Select-3-2.multisample"), library.join("Kicks 1 Select.multisample")),
        (ms.join("OHH 1 Select-2.multisample"), library.join("OHH 1 Select.multisample")),
        (project.join("samples/Kicks 1 Select.multisample"), library.join("Kicks 1 Select.multisample")),
        (ms.join("Kicks-4.multisample"), dir.join("Projects/Other/multi-samples/Kicks.multisample")),
        (ms.join("Snare-2.multisample"), library.join("Snare.multisample")),
        (ms.join("Kicks-5.multisample"), library.join("Kicks 1 Select.multisample")),
        // Twice: once is enough.
        (ms.join("Kicks 1 Select-3-2.multisample"), library.join("Kicks 1 Select.multisample")),
    ];
    let libraries = [library.clone()];
    let still_named = |copy: &Path, _: &Path| {
        copy.ends_with("Kicks-5.multisample").then(|| "Garage 19.bwproject still uses it, so it stays.".to_owned())
    };
    let file = project.join("Garage (fixed).bwproject");
    let fix = FixedProject { still_named: &still_named, ..fixed(&file, &changed, &libraries) };
    let bin = FolderBin::new(&dir);
    let done = remove_copies(&fix, &bin, &AtomicBool::new(false));
    let gone: Vec<&str> = done.removed.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(gone, [s(&ms.join("Kicks 1 Select-3-2.multisample"))]);
    assert_eq!(done.removed[0].what, What::BitwigCopy);
    assert_eq!(done.bytes(), 2048);
    let why: Vec<&str> = done.problems.iter().map(|p| p.why.as_str()).collect();
    assert_eq!(
        why,
        [
            "It isn't the same as the Library file now, so it stays.",
            "It isn't a multisample or sample file of the project's own, so it stays.",
            "What the fixed project uses instead isn't in the Library, so the copy stays.",
            "The Library file the fixed project uses isn't there now, so the copy stays.",
            "Garage 19.bwproject still uses it, so it stays.",
        ]
    );
    // The Library file stays, and undo puts the copy back.
    assert!(library.join("Kicks 1 Select.multisample").exists());
    assert!(undo(&done, &bin).is_empty());
    assert!(ms.join("Kicks 1 Select-3-2.multisample").exists());
}

#[test]
fn a_library_folder_that_holds_projects_never_gives_a_projects_copy() {
    let dir = fixtures("mo-space-bitwig-remove-wide-library");
    let project = dir.join("Projects/Garage");
    write(&project, "Garage (fixed).bwproject", b"fixed");
    write(&project, "multi-samples/A-2.multisample", b"a");
    write(&project, "multi-samples/Self-2.multisample", b"self");
    write(&dir, "Projects/Other/multi-samples/A.multisample", b"a");
    let ms = project.join("multi-samples");
    let changed = [
        (ms.join("A-2.multisample"), dir.join("Projects/Other/multi-samples/A.multisample")),
        // Pointed at itself.
        (ms.join("Self-2.multisample"), ms.join("Self-2.multisample")),
    ];
    // The whole Bitwig folder given as the Library.
    let libraries = [dir.clone()];
    let done = remove_copies(
        &fixed(&project.join("Garage (fixed).bwproject"), &changed, &libraries),
        &FolderBin::new(&dir),
        &AtomicBool::new(false),
    );
    assert!(done.removed.is_empty());
    assert!(done.problems.iter().all(|p| p.why.contains("is a project's copy too")), "{:?}", done.problems);
}

#[test]
fn without_a_fixed_project_or_after_a_stop_every_copy_stays() {
    let dir = fixtures("mo-space-bitwig-remove-no-fix");
    let (library, project) = (dir.join("Library"), dir.join("Project"));
    write(&library, "A.multisample", b"a");
    write(&project, "multi-samples/A-2.multisample", b"a");
    write(&project, "Project.bwproject", b"original");
    let changed = [(project.join("multi-samples/A-2.multisample"), library.join("A.multisample"))];
    let libraries = [library.clone()];
    let bin = FolderBin::new(&dir);
    let go = |file: &str, stop: bool| {
        let done = remove_copies(&fixed(&project.join(file), &changed, &libraries), &bin, &AtomicBool::new(stop));
        assert!(done.removed.is_empty());
        done.problems[0].why.clone()
    };
    assert_eq!(
        go("Project (fixed).bwproject", false),
        "The fixed project isn't there now, so the copies it was made for stay."
    );
    // The original project, given by mistake.
    assert_eq!(go("Project.bwproject", false), "The project given isn't a \"(fixed)\" one, so its copies stay.");
    write(&project, "Project (fixed 2).bwproject", b"fixed");
    assert_eq!(go("Project (fixed 2).bwproject", true), "Stopped before it went.");
    assert!(project.join("multi-samples/A-2.multisample").exists());
}

#[cfg(unix)]
#[test]
fn a_link_a_linked_folder_or_a_hard_link_to_the_library_file_stays() {
    let dir = fixtures("mo-space-bitwig-remove-links");
    let library = dir.join("Library");
    write(&library, "A.multisample", b"a");
    write(&dir, "Elsewhere/A-2.multisample", b"a");
    let libraries = [library.clone()];
    let bin = FolderBin::new(&dir);
    let go = |project: &Path| {
        write(project, "P (fixed).bwproject", b"fixed");
        let changed = [(project.join("multi-samples/A-2.multisample"), library.join("A.multisample"))];
        let done = remove_copies(
            &fixed(&project.join("P (fixed).bwproject"), &changed, &libraries),
            &bin,
            &AtomicBool::new(false),
        );
        assert!(done.removed.is_empty());
        done.problems[0].why.clone()
    };
    // The copy is a link to a file elsewhere.
    let one = dir.join("One");
    fs::create_dir_all(one.join("multi-samples")).unwrap();
    std::os::unix::fs::symlink(dir.join("Elsewhere/A-2.multisample"), one.join("multi-samples/A-2.multisample"))
        .unwrap();
    assert_eq!(go(&one), "It isn't a multisample or sample file of the project's own, so it stays.");
    // The multisample folder is a link to a folder elsewhere.
    let two = dir.join("Two");
    fs::create_dir_all(&two).unwrap();
    std::os::unix::fs::symlink(dir.join("Elsewhere"), two.join("multi-samples")).unwrap();
    assert_eq!(go(&two), "It isn't in the fixed project's own multi-samples or samples folder, so it stays.");
    // The copy is the Library file itself, by a hard link.
    let three = dir.join("Three");
    fs::create_dir_all(three.join("multi-samples")).unwrap();
    fs::hard_link(library.join("A.multisample"), three.join("multi-samples/A-2.multisample")).unwrap();
    assert!(go(&three).contains("a hard link"));
    assert!(dir.join("Elsewhere/A-2.multisample").exists() && library.join("A.multisample").exists());
}

#[test]
fn one_copy_given_two_ways_goes_once_and_a_good_second_way_still_counts() {
    let dir = fixtures("mo-space-bitwig-remove-two-ways");
    let (library, project) = (dir.join("Library"), dir.join("Project"));
    write(&library, "A.multisample", b"a");
    write(&project, "P (fixed).bwproject", b"fixed");
    write(&project, "multi-samples/A-2.multisample", b"a");
    write(&project, "multi-samples/B-2.multisample", b"a");
    folder_link(&project.join("multi-samples"), &project.join("Old name"));
    let ms = project.join("multi-samples");
    let changed = [
        // B: first with a Library file that isn't there, then as it should be.
        (ms.join("B-2.multisample"), library.join("Gone.multisample")),
        (ms.join("B-2.multisample"), library.join("A.multisample")),
        // A: then again through a linked folder's name.
        (ms.join("A-2.multisample"), library.join("A.multisample")),
        (project.join("Old name/A-2.multisample"), library.join("A.multisample")),
    ];
    let libraries = [library.clone()];
    let file = project.join("P (fixed).bwproject");
    let done = remove_copies(&fixed(&file, &changed, &libraries), &FolderBin::new(&dir), &AtomicBool::new(false));
    let gone: Vec<&str> = done.removed.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(gone, [s(&ms.join("B-2.multisample")), s(&ms.join("A-2.multisample"))]);
    let why: Vec<&str> = done.problems.iter().map(|p| p.why.as_str()).collect();
    assert_eq!(why.len(), 2);
    assert!(why[0].contains("isn't there now") && why[1].starts_with("It's the same file as "), "{why:?}");
}

#[test]
fn a_sample_copy_goes_only_while_its_original_is_outside_every_project() {
    let dir = fixtures("mo-space-bitwig-remove-samples");
    let project = dir.join("Projects/Garage");
    write(&project, "Garage (fixed).bwproject", b"fixed");
    write(&project, "samples/Snare-2.wav", b"snare");
    write(&project, "samples/Hat-2.wav", b"hat");
    write(&project, "samples/Clap-2.wav", b"clap");
    write(&project, "samples/Pad-2.multisample", b"pad");
    write(&dir, "Packs/Snare.wav", b"snare");
    write(&dir, "Projects/Other/samples/Hat.wav", b"hat");
    write(&dir, "Projects/Other/Other.bwproject", b"other");
    write(&dir, "Projects/Old (old copy)/Clap.wav", b"clap");
    write(&dir, "Library/Pad.multisample", b"pad");
    let sm = project.join("samples");
    let changed = [
        // From a sample pack: it goes.
        (sm.join("Snare-2.wav"), dir.join("Packs/Snare.wav")),
        // Another project's own copy: never what's used instead.
        (sm.join("Hat-2.wav"), dir.join("Projects/Other/samples/Hat.wav")),
        // In a folder a move left behind: it may go any time, so it's never what's used instead.
        (sm.join("Clap-2.wav"), dir.join("Projects/Old (old copy)/Clap.wav")),
        // A multisample belongs in multi-samples: one in samples isn't a copy this knows, so it stays.
        (sm.join("Pad-2.multisample"), dir.join("Library/Pad.multisample")),
    ];
    let libraries = [dir.join("Library")];
    let bin = FolderBin::new(&dir);
    let done = remove_copies(
        &fixed(&project.join("Garage (fixed).bwproject"), &changed, &libraries),
        &bin,
        &AtomicBool::new(false),
    );
    let gone: Vec<&str> = done.removed.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(gone, [s(&sm.join("Snare-2.wav"))], "{:?}", done.problems);
    assert_eq!(done.problems.len(), 3, "{:?}", done.problems);
    assert!(dir.join("Packs/Snare.wav").exists() && sm.join("Hat-2.wav").exists() && sm.join("Clap-2.wav").exists());
    assert!(undo(&done, &bin).is_empty());
    assert!(sm.join("Snare-2.wav").exists());
}
