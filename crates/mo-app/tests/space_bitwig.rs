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
use mo_app::space_fix_note;
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
    let store = dir.join("data").join(mo_space::prints::FILE_NAME);
    // Settings names the Bitwig Studio folder: its Library comes first, before the one in Documents.
    let studios = [studio.clone(), documents.join("Bitwig Studio")];
    let found =
        look(&roots, &studios, &[], &mut index, Some(&store), None, &stop, |done, of| heard.push((done, of))).unwrap();
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

    // After a restart (a new index) the fingerprints kept in the store give the same copies.
    let restarted =
        look(&roots, &[documents.join("Bitwig Studio")], &[], &mut None, Some(&store), None, &stop, |_, _| {}).unwrap();
    assert_eq!(summary(&restarted), summary(&found));

    // The next look keeps the Library it read: a multisample added since is found, and one taken out isn't.
    write(&studio, "Library/Multisamples/Own.multisample", &own);
    fs::remove_file(studio.join("Library/Multisamples/Pads.multisample")).unwrap();
    let again = look(&roots, &[documents.join("Bitwig Studio")], &[], &mut index, Some(&store), None, &stop, |_, _| {})
        .unwrap();
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
    let found = look(&[dir.join("Projects")], &[], &[], &mut None, None, None, &stop, |_, _| {}).unwrap();
    assert_eq!(found, Copies::default());
    // Copies, but no Bitwig Studio folder above them and none in Documents: there's no Library to compare them with.
    write(&dir, "Projects/Song/Song.bwproject", b"project");
    write(&dir, "Projects/Song/multi-samples/Kicks-2.multisample", b"kicks");
    let mut index = None;
    let found = look(
        &[dir.join("Projects")],
        &[dir.join("No documents").join("Bitwig Studio")],
        &[],
        &mut index,
        None,
        None,
        &stop,
        |_, _| {},
    )
    .unwrap();
    assert_eq!((found.folders, found.library.len(), found.projects.len()), (1, 0, 0));
    assert!(index.is_none(), "no Library was read");
}

#[test]
fn single_sample_copies_are_found_through_the_sounds_library() {
    let dir = fixtures("mo-app-space-bitwig-samples");
    let studio = dir.join("K/Bitwig Studio");
    write(&studio, "Library/Multisamples/Pads.multisample", &[2u8; 8192]);
    let (snare, other) = ([5u8; 3000], [6u8; 3000]);
    // The originals, in a sample pack, as the Sounds library knows them.
    write(&dir, "F/Samples/ukg_snr_blaze.wav", &snare);
    write(&dir, "F/Samples/ukg_snr_bingo.wav", &other);
    let projects = studio.join("Projects");
    write(&projects, "707/707.bwproject", b"project");
    write(&projects, "707/samples/ukg_snr_blaze-2-2-3.wav", &snare);
    // Named like a sound, but not the same bytes: the project's own.
    write(&projects, "707/samples/ukg_snr_bingo-2.wav", &[7u8; 3000]);
    // A recording of the project's own, named like no sound.
    write(&projects, "707/samples/vocal take.wav", &other);
    let sounds = [dir.join("F/Samples/ukg_snr_blaze.wav"), dir.join("F/Samples/ukg_snr_bingo.wav")];
    let named = |name: &str| -> Vec<String> {
        sounds
            .iter()
            .filter(|s| s.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)))
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    };
    let stop = AtomicBool::new(false);
    let found =
        look(std::slice::from_ref(&projects), &[], &[], &mut None, None, Some(&named), &stop, |_, _| {}).unwrap();
    assert_eq!(
        summary(&found),
        vec![("707".to_string(), 3000, vec![("ukg_snr_blaze-2-2-3.wav".to_string(), "ukg_snr_blaze.wav".to_string())])]
    );
    // Without the Sounds library, the samples folder isn't looked through.
    let found = look(&[projects], &[], &[], &mut None, None, None, &stop, |_, _| {}).unwrap();
    assert!(found.projects.is_empty());
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
    assert!(look(&roots, &[], &[], &mut index, None, None, &AtomicBool::new(true), |_, _| {}).is_err());
    let found = look(&roots, &[], &[], &mut index, None, None, &AtomicBool::new(false), |_, _| {}).unwrap();
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
    // And a sample file as Bitwig keeps one (0x4a6): its name, its place in the project, and its file reference (0x2a,
    // the path in 0x3b) where it was when the project was saved.
    let mut reference = Object::new(0x2a);
    reference.fields.push(text(0x3b, &format!(r"I:\Bitwig Studio\Projects\Song\multi-samples\{copy}")));
    let reference = doc.add(reference);
    let mut sample = Object::new(0x4a6);
    sample.fields.push(text(0x512, copy));
    sample.fields.push(text(0xd3a, &format!("multi-samples/{copy}")));
    sample.fields.push(Field { id: 0xd3b, value: Value::Obj(Slot::Obj(reference)) });
    let sample = doc.add(sample);
    doc.objects[reference.0 as usize].parent = Some((sample, 0xd3b));
    doc.objects[sample.0 as usize].parent = Some((root, 0x7f11));
    doc.obj_mut(root).fields.push(Field { id: 0x7f11, value: Value::Obj(Slot::Obj(sample)) });
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
    // The app's note of what Fix wrote, here rather than in the data folder.
    let note = dir.join(space_fix_note::FILE_NAME);
    write(&song, "multi-samples/Kicks-2.multisample", kicks);
    project(&song.join("Song v1.bwproject"), "Kicks-2.multisample", 10);
    project(&song.join("Song v2.bwproject"), "Kicks-2.multisample", 20);
    let roots = [studio.join("Projects")];
    let stop = AtomicBool::new(false);
    let mut index = None;
    let found = look(&roots, &[], &[], &mut index, None, None, &stop, |_, _| {}).unwrap();
    let saves: Vec<String> = found.projects[0].saves.iter().map(|s| name(&s.path)).collect();
    assert_eq!(saves, ["Song v2.bwproject", "Song v1.bwproject"]);

    // Out of the box only the newest save is ticked.
    let mut fixes = Fixes::default();
    let jobs = fixes.to_fix(&found);
    assert_eq!(jobs.iter().map(|j| name(&j.save)).collect::<Vec<_>>(), ["Song v2.bwproject"]);
    fixes.start(&jobs);
    let Fixing::Written(fixed) = fix(&jobs[0], &note) else { panic!("the newest save should be fixed") };
    assert_eq!(fixed, song.join("Song v2 (fixed).bwproject"));
    let back = bitwig::decode(fs::read(&fixed).unwrap()).unwrap();
    assert_eq!(back.body_error, None);
    // The saves and the copy are as they were.
    assert!(song.join("multi-samples/Kicks-2.multisample").is_file());
    fixes.done(jobs[0].save.clone(), Fixing::Written(fixed.clone()));
    fixes.finished();

    // The next look sees it fixed on disk, which counts from then on, so nothing is ticked; the older save can be
    // ticked and fixed too.
    let again = look(&roots, &[], &[], &mut index, None, None, &stop, |_, _| {}).unwrap();
    fixes.looked();
    assert!(fixes.fixing.is_empty());
    let newest = &again.projects[0].saves[0];
    assert_eq!((newest.is_fixed(), newest.fixed.clone().map(|f| f.0)), (true, Some(fixed.clone())));
    assert!(fixes.to_fix(&again).is_empty());
    fixes.ticks.insert(song.join("Song v1.bwproject"), true);
    let jobs = fixes.to_fix(&again);
    assert_eq!(jobs.iter().map(|j| name(&j.save)).collect::<Vec<_>>(), ["Song v1.bwproject"]);
    assert_eq!(fix(&jobs[0], &note), Fixing::Written(song.join("Song v1 (fixed).bwproject")));

    // Each is on the app's note as Fix left it, so the button that sends every fixed project to the Recycle Bin may
    // send it; one you've saved in since stays (the whole-app review of 14 Nov, FX-5).
    let v1 = song.join("Song v1 (fixed).bwproject");
    let check = space_fix_note::Check::new(&note);
    for file in [&fixed, &v1] {
        assert_eq!(check.as_fix_left(file, &fs::metadata(file).unwrap()), Ok(()), "{}", file.display());
    }
    fs::write(&v1, b"saved in Bitwig").unwrap();
    let later = SystemTime::now() + Duration::from_secs(60);
    fs::File::options().write(true).open(&v1).unwrap().set_modified(later).unwrap();
    let changed = space_fix_note::Stays::Changed;
    assert_eq!(check.as_fix_left(&v1, &fs::metadata(&v1).unwrap()), Err(changed.why().to_owned()));
    assert_eq!(check.stayed().get(&key(&v1.to_string_lossy())), Some(&changed));

    // A save that names none of the copies isn't fixed, and says why.
    let other = studio.join("Projects/Other");
    write(&other, "multi-samples/Kicks-3.multisample", kicks);
    project(&other.join("Other.bwproject"), "Snare.multisample", 30);
    let found = look(&roots, &[], &[], &mut index, None, None, &stop, |_, _| {}).unwrap();
    let other = found.projects.iter().find(|p| p.name() == "Other").unwrap();
    let job = mo_app::space_bitwig::ToFix {
        save: other.saves[0].path.clone(),
        copies: other.copies.clone(),
        libraries: Vec::new(),
    };
    let Fixing::Failed(why) = fix(&job, &note) else { panic!("a save that uses none of the copies isn't fixed") };
    assert!(why.contains("nothing was written"), "{why}");
}

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

/// Library folders added on Settings › DAWs (an old Library on another drive) are looked in straight after your Bitwig
/// Studio folder's own, before the one in Documents; one that isn't there, or is listed twice, is left out.
#[test]
fn added_library_folders_come_after_the_bitwig_studio_folders_own() {
    let dir = fixtures("mo-app-space-bitwig-extra");
    let studio = dir.join("K/Bitwig Studio");
    write(&studio, "Library/Multisamples/Kicks.multisample", b"k");
    let old = dir.join("D/Old Library");
    write(&old, "Multisamples/Snare.multisample", b"s");
    let documents = dir.join("Documents/Bitwig Studio");
    write(&documents, "Library/Multisamples/Bass.multisample", b"b");
    let studios = [studio.clone(), documents.clone()];
    let extra = [old.clone(), dir.join("not there"), studio.join("Library")];
    let found = mo_app::space_bitwig::library_folders(&[], &studios, &extra);
    assert_eq!(found, [studio.join("Library"), old, documents.join("Library")]);
}

/// A Recycle Bin in a folder of the test's own.
struct FolderBin {
    dir: PathBuf,
    held: std::cell::RefCell<Vec<(String, PathBuf)>>,
}

impl mo_space::clean::Bin for FolderBin {
    fn send(&self, path: &Path) -> Result<(), String> {
        let mut held = self.held.borrow_mut();
        let to = self.dir.join(held.len().to_string());
        fs::create_dir_all(&self.dir).unwrap();
        fs::rename(path, &to).map_err(|e| e.to_string())?;
        held.push((path.to_string_lossy().into_owned(), to));
        Ok(())
    }
    fn find(&self, _removed: &mut [mo_space::clean::Removed]) -> Vec<mo_space::clean::Problem> {
        Vec::new()
    }
    fn put_back(&self, _removed: &[mo_space::clean::Removed]) -> Vec<mo_space::clean::Problem> {
        Vec::new()
    }
}

#[test]
fn free_the_space_sends_a_copy_once_every_song_using_it_is_fixed_and_checked() {
    use mo_app::space_bitwig::{ToFree, free, free_button, free_noted, left_behind};
    // As the page fills them in from the library before anything goes: nothing elsewhere uses the copy.
    let asked = |mut jobs: Vec<ToFree>| {
        for job in &mut jobs {
            job.elsewhere = Ok(Vec::new());
            job.reread = mo_library::song_writes::BitwigReread::Done;
        }
        jobs
    };
    let dir = fixtures("mo-app-space-bitwig-free");
    let studio = dir.join("Bitwig Studio");
    let kicks = b"<multisample kicks/>";
    write(&studio, "Library/Multisamples/Kicks.multisample", kicks);
    let song = studio.join("Projects/Song");
    let note = dir.join(space_fix_note::FILE_NAME);
    write(&song, "multi-samples/Kicks-2.multisample", kicks);
    // Two saves of one song, a remix made from the same sounds, and one of Bitwig's backups.
    project(&song.join("Song v1.bwproject"), "Kicks-2.multisample", 10);
    project(&song.join("Song v2.bwproject"), "Kicks-2.multisample", 20);
    project(&song.join("Song remix.bwproject"), "Kicks-2.multisample", 30);
    project(&song.join("auto-backups/Song v2-1.bwproject"), "Kicks-2.multisample", 25);
    let roots = [studio.join("Projects")];
    let stop = AtomicBool::new(false);
    let mut index = None;
    let found = look(&roots, &[], &[], &mut index, None, None, &stop, |_, _| {}).unwrap();

    // Each song's newest save is fixed.
    let mut fixes = Fixes::default();
    let jobs = fixes.to_fix(&found);
    let mut names: Vec<String> = jobs.iter().map(|j| name(&j.save)).collect();
    names.sort();
    assert_eq!(names, ["Song remix.bwproject", "Song v2.bwproject"]);
    for job in &jobs {
        assert!(matches!(fix(job, &note), Fixing::Written(_)));
    }
    let found = look(&roots, &[], &[], &mut index, None, None, &stop, |_, _| {}).unwrap();
    assert!(fixes.to_fix(&found).is_empty() && fixes.to_free(&found).is_empty(), "nothing is checked out of the box");
    assert_eq!(free_button(Some(&found), &fixes, false), None);

    // Only the song is checked: the remix's own save still uses the copy, so it stays.
    fixes.set_tick(Some(&found), song.join("Song v2.bwproject"), true);
    let unasked = fixes.to_free(&found);
    let jobs = asked(unasked.clone());
    assert_eq!(
        free_button(Some(&found), &fixes, false),
        Some((
            "Free the space: copies of 1 checked song to the Recycle Bin"
                .replace("Recycle Bin", mo_app::space_bin::BIN),
            true
        ))
    );
    let behind: Vec<String> = left_behind(&jobs[0]).iter().map(|p| name(p)).collect();
    assert_eq!(behind, ["Song v2.bwproject", "Song v1.bwproject", "Song v2-1.bwproject"]);
    let bin = FolderBin { dir: dir.join("bin"), held: Default::default() };
    // Until the library has read every Bitwig save again for multisamples outside their folders, a multisample copy
    // stays (FS-5).
    let mut not_read = jobs[0].clone();
    not_read.reread = mo_library::song_writes::BitwigReread::Waiting;
    let done = free(&not_read, &bin, &stop);
    assert!(done.removed.is_empty() && !done.problems.is_empty(), "{:?}", done.problems);
    assert!(done.problems.iter().all(|p| p.why.contains("still reading")), "{:?}", done.problems);
    not_read.reread = mo_library::song_writes::BitwigReread::Owed(2);
    let done = free(&not_read, &bin, &stop);
    assert!(
        done.problems.iter().any(|p| p.why.contains("2 Bitwig saves couldn't be read again")),
        "{:?}",
        done.problems
    );
    // Until the library has been asked who else uses them, nothing goes; and as nothing went, the fixed project's mark
    // (put on before the first copy would go: FS-23) comes off again.
    let done = free_noted(&unasked[0], &bin, &stop, &note, 100);
    assert!(done.removed.is_empty() && done.problems.iter().all(|p| p.why.contains("library")), "{:?}", done.problems);
    assert_eq!(space_fix_note::is_freed(&note, &song.join("Song v2 (fixed).bwproject")), Ok(false));
    // You split v1 off into a song of its own on the Songs page: it isn't left behind, so it keeps its copies.
    let mut split = jobs[0].clone();
    let v1 = song.join("Song v1.bwproject").to_string_lossy().into_owned();
    split.choices = mo_library::songs::SongChoices::from_rows([("split", v1.as_str(), "")]);
    let behind: Vec<String> = left_behind(&split).iter().map(|p| name(p)).collect();
    assert_eq!(behind, ["Song v2.bwproject", "Song v2-1.bwproject"]);
    let done = free(&jobs[0], &bin, &stop);
    assert!(done.removed.is_empty(), "{:?}", done.problems);
    assert!(done.problems.iter().any(|p| p.why.contains("Song remix.bwproject still uses")), "{:?}", done.problems);
    assert!(song.join("multi-samples/Kicks-2.multisample").is_file());

    // Both checked: the copy goes; every save stays.
    fixes.set_tick(Some(&found), song.join("Song remix.bwproject"), true);
    let jobs = asked(fixes.to_free(&found));
    assert_eq!(jobs.len(), 1);

    // A save in another folder names the copy by its full path (saved without copying its samples): it stays.
    let elsewhere = studio.join("Projects/Elsewhere/Elsewhere.bwproject");
    write(&studio, "Projects/Elsewhere/Elsewhere.bwproject", b"a save");
    let copy = song.join("multi-samples/Kicks-2.multisample").to_string_lossy().into_owned();
    let mut used = jobs[0].clone();
    used.elsewhere = Ok(vec![(copy.clone(), elsewhere.to_string_lossy().into_owned())]);
    let done = free(&used, &bin, &stop);
    assert!(done.removed.is_empty(), "{:?}", done.problems);
    assert!(done.problems.iter().any(|p| p.why.contains("Elsewhere.bwproject uses")), "{:?}", done.problems);

    // An older save of a checked song saved again since the look (your newest work may be in it now): nothing goes.
    let v1 = song.join("Song v1.bwproject");
    let looked = fs::metadata(&v1).unwrap().modified().unwrap();
    fs::File::options().write(true).open(&v1).unwrap().set_modified(SystemTime::now()).unwrap();
    let done = free(&jobs[0], &bin, &stop);
    assert!(
        done.removed.is_empty() && done.problems.iter().all(|p| p.why.contains("Song v1.bwproject\" was saved again"))
    );
    fs::File::options().write(true).open(&v1).unwrap().set_modified(looked).unwrap();

    // The Library file Fix pointed the copy at was renamed since: the page now finds the same sound under its new name,
    // but the fixed songs still use the old one, so the copy stays (FS-2).
    let multisamples = studio.join("Library/Multisamples");
    fs::rename(multisamples.join("Kicks.multisample"), multisamples.join("Kicks renamed.multisample")).unwrap();
    let renamed = look(&roots, &[], &[], &mut index, None, None, &stop, |_, _| {}).unwrap();
    let renamed_jobs = asked(fixes.to_free(&renamed));
    assert_eq!(renamed_jobs.len(), 1);
    let done = free(&renamed_jobs[0], &bin, &stop);
    assert!(done.removed.is_empty(), "{:?}", done.problems);
    assert!(done.problems.iter().any(|p| p.why.contains("doesn't use")), "{:?}", done.problems);
    fs::rename(multisamples.join("Kicks renamed.multisample"), multisamples.join("Kicks.multisample")).unwrap();

    // Saved again since the page looked: nothing goes until it looks again.
    let remix = song.join("Song remix.bwproject");
    let later = SystemTime::now() + Duration::from_secs(120);
    fs::File::options().write(true).open(&remix).unwrap().set_modified(later).unwrap();
    let done = free(&jobs[0], &bin, &stop);
    assert!(
        done.removed.is_empty() && done.problems.iter().all(|p| p.why.contains("saved again")),
        "{:?}",
        done.problems
    );
    fs::File::options()
        .write(true)
        .open(&remix)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + 30 * 60))
        .unwrap();
    // A save the library knows in this folder is the folder's own check's to judge: it doesn't keep the copy.
    let copy = song.join("multi-samples/Kicks-2.multisample").to_string_lossy().into_owned();
    used.elsewhere = Ok(vec![(copy, song.join("Song v1.bwproject").to_string_lossy().into_owned())]);
    let done = free_noted(&used, &bin, &stop, &note, 200);
    assert_eq!(done.removed.len(), 1, "{:?}", done.problems);
    assert!(!song.join("multi-samples/Kicks-2.multisample").exists());
    assert!(studio.join("Library/Multisamples/Kicks.multisample").is_file());
    for save in ["Song v1", "Song v2", "Song remix", "Song v2 (fixed)", "Song remix (fixed)"] {
        assert!(song.join(format!("{save}.bwproject")).is_file(), "{save} stays");
    }

    // Their fixed projects are the ones that play now: the button for every fixed project leaves them.
    let freed = space_fix_note::Stays::Freed;
    let judged = |fixed: &Path| space_fix_note::Check::new(&note).as_fix_left(fixed, &fs::metadata(fixed).unwrap());
    let fixed = song.join("Song v2 (fixed).bwproject");
    let remix_fixed = song.join("Song remix (fixed).bwproject");
    for f in [&fixed, &remix_fixed] {
        assert_eq!(judged(f), Err(freed.why().to_owned()), "{}", f.display());
    }
    let copy = song.join("multi-samples/Kicks-2.multisample").to_string_lossy().into_owned();
    // Put back with the copy left in the bin keeps the marks.
    space_fix_note::put_back(&note, 200, &[&copy], &[&copy]);
    assert_eq!(judged(&fixed), Err(freed.why().to_owned()));
    // Another Free the space's Put back doesn't take this one's off (FS-1).
    space_fix_note::put_back(&note, 300, &[&copy], &[]);
    assert_eq!(judged(&remix_fixed), Err(freed.why().to_owned()));
    // Once this one's copies are all back, they're fixed projects like any other.
    space_fix_note::put_back(&note, 200, &[&copy], &[]);
    for f in [&fixed, &remix_fixed] {
        assert_eq!(judged(f), Ok(()), "{}", f.display());
    }
}
