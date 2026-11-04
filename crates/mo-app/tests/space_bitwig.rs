//! The Space page's look for Bitwig copies and its Fix, from start to end, on folders made in the test build's own
//! fixtures folder under fixed names: never a real Bitwig folder. The files are made-up bytes and made-up projects, not
//! anyone's sounds or songs.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
#[path = "../../mo-formats/tests/common/bitwig.rs"]
mod bitwig_fixtures;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use mo_app::space_bitwig::{Copies, Fixes, Fixing, fix, look};
use mo_formats::bitwig::{self, Field, Object, Slot, Text, Value};
use mo_space::key;

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(&dir).unwrap()
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

/// A project as a test compares it: its folder's name, the room its copies take, and each copy's file name with the
/// Library file it's the same as.
type Seen = (String, u64, Vec<(String, String)>);

/// What a look found, as names and sizes.
fn summary(found: &Copies) -> Vec<Seen> {
    let name = |p: &str| Path::new(p).file_name().unwrap().to_string_lossy().into_owned();
    found
        .projects
        .iter()
        .map(|p| {
            let copies = p.copies.iter().map(|c| (name(&c.in_project), name(&c.library_file))).collect();
            (p.name(), p.bytes(), copies)
        })
        .collect()
}

#[test]
fn the_copies_in_each_project_are_found_and_listed_biggest_first() {
    let dir = fixtures("mo-app-space-bitwig");
    let studio = dir.join("K/Bitwig Studio");
    let (kicks, pads, own, bass) = ([1u8; 4096], [2u8; 8192], [3u8; 100], [4u8; 2048]);
    write(&studio, "Library/Multisamples/Kicks 1 Select.multisample", &kicks);
    write(&studio, "Library/Multisamples/Pads.multisample", &pads);
    // Bitwig's own place for the Library, in Documents.
    let documents = dir.join("Documents");
    write(&documents, "Bitwig Studio/Library/Multisamples/Bass.multisample", &bass);

    let projects = studio.join("Projects");
    write(&projects, "GARAGE/GARAGE.bwproject", b"project");
    write(&projects, "GARAGE/multi-samples/Kicks 1 Select-3-2.multisample", &kicks);
    write(&projects, "GARAGE/multi-samples/Pads-2.multisample", &pads);
    // The project's own multisample, the same as no Library file: it stays the project's.
    write(&projects, "GARAGE/multi-samples/Own.multisample", &own);
    write(&projects, "Night/Night.bwproject", b"project");
    write(&projects, "Night/multi-samples/Kicks 1 Select-2.multisample", &kicks);
    write(&projects, "Sketch/Sketch.bwproject", b"project");
    write(&projects, "Sketch/multi-samples/Own.multisample", &own);
    // A project kept outside Bitwig Studio is compared with the Library too.
    let elsewhere = dir.join("I/Music");
    write(&elsewhere, "Song/Song.bwproject", b"project");
    write(&elsewhere, "Song/multi-samples/Bass-2.multisample", &bass);
    write(&elsewhere, "Song/multi-samples/Kicks 1 Select-2.multisample", &kicks);

    let roots = [projects.clone(), elsewhere.clone()];
    let stop = AtomicBool::new(false);
    let mut index = None;
    let mut heard = Vec::new();
    let found = look(&roots, Some(&documents), &mut index, &stop, |done, of| heard.push((done, of))).unwrap();
    assert_eq!(heard, [(0, 4), (1, 4), (2, 4), (3, 4), (4, 4)]);
    assert_eq!(found.folders, 4);
    assert_eq!(
        found.library.iter().map(|l| key(&l.to_string_lossy())).collect::<Vec<_>>(),
        [studio.join("Library"), documents.join("Bitwig Studio/Library")].map(|l| key(&l.to_string_lossy()))
    );
    let pair = |copy: &str, library: &str| (copy.to_owned(), library.to_owned());
    assert_eq!(
        summary(&found),
        [
            (
                "GARAGE".to_owned(),
                4096 + 8192,
                vec![
                    pair("Kicks 1 Select-3-2.multisample", "Kicks 1 Select.multisample"),
                    pair("Pads-2.multisample", "Pads.multisample")
                ]
            ),
            (
                "Song".to_owned(),
                2048 + 4096,
                vec![
                    pair("Bass-2.multisample", "Bass.multisample"),
                    pair("Kicks 1 Select-2.multisample", "Kicks 1 Select.multisample")
                ]
            ),
            ("Night".to_owned(), 4096, vec![pair("Kicks 1 Select-2.multisample", "Kicks 1 Select.multisample")]),
        ]
    );
    assert_eq!((found.bytes(), found.count()), (4096 + 8192 + 2048 + 4096 + 4096, 5));

    // The next look keeps the Library it read: a multisample added since is found, and one taken out isn't.
    write(&studio, "Library/Multisamples/Own.multisample", &own);
    fs::remove_file(studio.join("Library/Multisamples/Pads.multisample")).unwrap();
    let again = look(&roots, Some(&documents), &mut index, &stop, |_, _| {}).unwrap();
    let names: Vec<(String, usize)> = again.projects.iter().map(|p| (p.name(), p.copies.len())).collect();
    assert_eq!(
        names,
        [("Song".to_owned(), 2), ("GARAGE".to_owned(), 2), ("Night".to_owned(), 1), ("Sketch".to_owned(), 1)]
    );
}

#[test]
fn without_projects_or_a_library_nothing_is_read() {
    let dir = fixtures("mo-app-space-bitwig-none");
    let stop = AtomicBool::new(false);
    // No project keeps copies of multisamples.
    write(&dir, "Projects/Sketch/Sketch.bwproject", b"project");
    let found = look(&[dir.join("Projects")], None, &mut None, &stop, |_, _| {}).unwrap();
    assert_eq!(found, Copies::default());
    // Copies, but no Bitwig Studio folder above them and none in Documents: there's no Library to compare them with.
    write(&dir, "Projects/Song/Song.bwproject", b"project");
    write(&dir, "Projects/Song/multi-samples/Kicks-2.multisample", b"kicks");
    let mut index = None;
    let found = look(&[dir.join("Projects")], Some(&dir.join("No documents")), &mut index, &stop, |_, _| {}).unwrap();
    assert_eq!((found.folders, found.library.len(), found.projects.len()), (1, 0, 0));
    assert!(index.is_none(), "no Library was read");
}

#[test]
fn a_stop_stops_it_and_the_next_look_starts_again() {
    let dir = fixtures("mo-app-space-bitwig-stop");
    let studio = dir.join("Bitwig Studio");
    write(&studio, "Library/Kicks.multisample", b"kicks");
    write(&studio, "Projects/Song/Song.bwproject", b"project");
    write(&studio, "Projects/Song/multi-samples/Kicks-2.multisample", b"kicks");
    let roots = [studio.join("Projects")];
    let mut index = None;
    assert!(look(&roots, None, &mut index, &AtomicBool::new(true), |_, _| {}).is_err());
    let found = look(&roots, None, &mut index, &AtomicBool::new(false), |_, _| {}).unwrap();
    assert_eq!(found.count(), 1);
}

/// A made-up Bitwig project saved `minutes` after a fixed time, naming a copy in its `multi-samples` folder the ways
/// Bitwig does (its name, its place in the project and where the project was saved), as the converter's own tests make
/// them.
fn project(path: &Path, copy: &str, minutes: u64) {
    let text = |id: u32, t: &str| Field { id, value: Value::Str(Text::new(t)) };
    let mut doc = bitwig::decode(bitwig_fixtures::bitwig_template(bitwig_fixtures::Template::default())).unwrap();
    let mut file = Object::new(0x7f00);
    file.fields.push(text(0x7f03, copy));
    file.fields.push(text(0x7f04, &format!("multi-samples/{copy}")));
    file.fields.push(text(0x7f05, &format!(r"I:\Bitwig Studio\Projects\Song\multi-samples\{copy}")));
    let file = doc.add(file);
    let root = doc.root.unwrap();
    doc.objects[file.0 as usize].parent = Some((root, 0x7f10));
    doc.obj_mut(root).fields.push(Field { id: 0x7f10, value: Value::Obj(Slot::Obj(file)) });
    let zip = doc.zip().map(<[u8]>::to_vec);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bitwig::encode(&doc, zip.as_deref()).unwrap()).unwrap();
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + minutes * 60);
    fs::File::options().write(true).open(path).unwrap().set_modified(at).unwrap();
}

#[test]
fn fix_writes_a_fixed_project_beside_the_ticked_saves_and_the_next_look_sees_it() {
    let dir = fixtures("mo-app-space-bitwig-fix");
    let studio = dir.join("Bitwig Studio");
    let kicks = b"<multisample kicks/>";
    write(&studio, "Library/Multisamples/Kicks.multisample", kicks);
    let song = studio.join("Projects/Song");
    write(&song, "multi-samples/Kicks-2.multisample", kicks);
    project(&song.join("Song v1.bwproject"), "Kicks-2.multisample", 10);
    project(&song.join("Song v2.bwproject"), "Kicks-2.multisample", 20);
    let roots = [studio.join("Projects")];
    let stop = AtomicBool::new(false);
    let mut index = None;
    let found = look(&roots, None, &mut index, &stop, |_, _| {}).unwrap();
    let saves: Vec<String> = found.projects[0].saves.iter().map(|s| name(&s.path)).collect();
    assert_eq!(saves, ["Song v2.bwproject", "Song v1.bwproject"]);

    // Out of the box only the newest save is ticked.
    let mut fixes = Fixes::default();
    let jobs = fixes.to_fix(&found);
    assert_eq!(jobs.iter().map(|j| name(&j.save)).collect::<Vec<_>>(), ["Song v2.bwproject"]);
    fixes.start(&jobs);
    let Fixing::Written(fixed) = fix(&jobs[0]) else { panic!("the newest save should be fixed") };
    assert_eq!(fixed, song.join("Song v2 (fixed).bwproject"));
    let back = bitwig::decode(fs::read(&fixed).unwrap()).unwrap();
    assert_eq!(back.body_error, None);
    // The saves and the copy are as they were.
    assert!(song.join("multi-samples/Kicks-2.multisample").is_file());
    fixes.done(jobs[0].save.clone(), Fixing::Written(fixed.clone()));
    fixes.finished();

    // The next look sees it fixed on disk, which counts from then on, so nothing is ticked; the older save can be
    // ticked and fixed too.
    let again = look(&roots, None, &mut index, &stop, |_, _| {}).unwrap();
    fixes.looked();
    assert!(fixes.fixing.is_empty());
    let newest = &again.projects[0].saves[0];
    assert_eq!((newest.is_fixed(), newest.fixed.clone().map(|f| f.0)), (true, Some(fixed.clone())));
    assert!(fixes.to_fix(&again).is_empty());
    fixes.ticks.insert(song.join("Song v1.bwproject"), true);
    let jobs = fixes.to_fix(&again);
    assert_eq!(jobs.iter().map(|j| name(&j.save)).collect::<Vec<_>>(), ["Song v1.bwproject"]);
    assert_eq!(fix(&jobs[0]), Fixing::Written(song.join("Song v1 (fixed).bwproject")));

    // A save that names none of the copies isn't fixed, and says why.
    let other = studio.join("Projects/Other");
    write(&other, "multi-samples/Kicks-3.multisample", kicks);
    project(&other.join("Other.bwproject"), "Snare.multisample", 30);
    let found = look(&roots, None, &mut index, &stop, |_, _| {}).unwrap();
    let other = found.projects.iter().find(|p| p.name() == "Other").unwrap();
    let job = mo_app::space_bitwig::ToFix { save: other.saves[0].path.clone(), copies: other.copies.clone() };
    let Fixing::Failed(why) = fix(&job) else { panic!("a save that uses none of the copies isn't fixed") };
    assert!(why.contains("nothing was written"), "{why}");
}

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}
