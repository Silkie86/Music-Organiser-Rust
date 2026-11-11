//! Pointing a Bitwig project's multisample copies back at the Library, on a made-up project in the test build's own
//! fixtures folder.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
#[path = "../../mo-formats/tests/common/bitwig.rs"]
mod fixtures;

use std::path::{Path, PathBuf};

use mo_convert::relink::{Relink, fix_bitwig_project, fixed_file, still_named};
use mo_formats::bitwig::{self, Blob, Field, Object, Slot, Text, Value};

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-convert").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn text(id: u32, t: &str) -> Field {
    Field { id, value: Value::Str(Text::new(t)) }
}

/// A project whose root holds a multisample reference laid out as Solomon's are: the copy's name, its relative and
/// absolute paths, and a child naming the Library file it came from (gone since).
fn project_with_copy(dir: &Path, copy: &str) -> PathBuf {
    let mut doc = bitwig::decode(fixtures::bitwig_template(fixtures::Template::default())).unwrap();
    let mut origin = Object::new(0x7f01);
    origin.fields.push(text(
        0x7f02,
        r"C:\Users\someone\Documents\Bitwig Studio\Library\Multisamples\Kicks 1 Select.multisample",
    ));
    // The same copy in another project it was copied from (gone since), as Bitwig keeps it beside the copy.
    origin.fields.push(text(0x7f08, &format!(r"Q:\Bitwig Studio\Projects\jingle garage\multi-samples\{copy}")));
    let origin = doc.add(origin);
    let mut file = Object::new(0x7f00);
    file.fields.push(text(0x7f03, copy));
    file.fields.push(text(0x7f04, &format!("multi-samples/{copy}")));
    // Saved on another drive, in a folder with this project's name.
    let folder = dir.file_name().unwrap().to_string_lossy();
    file.fields.push(text(0x7f05, &format!(r"I:\Bitwig Studio\Projects\{folder}\multi-samples\{copy}")));
    file.fields.push(Field { id: 0x7f06, value: Value::Obj(Slot::Obj(origin)) });
    let file = doc.add(file);
    doc.objects[origin.0 as usize].parent = Some((file, 0x7f06));
    // Bitwig lists the files a project uses in its metadata.
    for (k, v) in &mut doc.meta {
        if k.as_str() == "used_files" {
            *v = Value::Strings(vec![Text::new(&format!("multi-samples/{copy}"))]);
        }
    }
    let root = doc.root.unwrap();
    doc.objects[file.0 as usize].parent = Some((root, 0x7f10));
    doc.obj_mut(root).fields.push(Field { id: 0x7f10, value: Value::Obj(Slot::Obj(file)) });
    let zip = doc.zip().map(<[u8]>::to_vec);
    let bytes = bitwig::encode(&doc, zip.as_deref()).unwrap();
    std::fs::create_dir_all(dir.join("multi-samples")).unwrap();
    std::fs::write(dir.join("multi-samples").join(copy), b"<multisample/>").unwrap();
    let path = dir.join("Song.bwproject");
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Every text in a project, its metadata's included.
fn texts(path: &Path) -> Vec<String> {
    let doc = bitwig::decode(std::fs::read(path).unwrap()).unwrap();
    assert_eq!(doc.body_error, None);
    let fields = doc.objects.iter().flat_map(|o| &o.fields).map(|f| &f.value);
    doc.meta
        .iter()
        .map(|(_, v)| v)
        .chain(fields)
        .flat_map(|v| match v {
            Value::Str(t) => vec![t.as_str().to_owned()],
            Value::Strings(ts) => ts.iter().map(|t| t.as_str().to_owned()).collect(),
            _ => Vec::new(),
        })
        .collect()
}

/// A Library file on disk for the copies to point at.
fn library(dir: &Path, name: &str) -> PathBuf {
    let folder = dir.join("Library").join("Multisamples");
    std::fs::create_dir_all(&folder).unwrap();
    let file = std::path::absolute(folder.join(name)).unwrap();
    std::fs::write(&file, b"<multisample/>").unwrap();
    file
}

fn relink(dir: &Path, copy: &str, library: &Path) -> Relink {
    Relink { copy: dir.join("multi-samples").join(copy), library: library.to_path_buf() }
}

const COPY: &str = "Kicks 1 Select-3-2-2.multisample";

#[test]
fn every_reference_to_a_copy_points_at_the_library_file() {
    let dir = folder("relink");
    let project = project_with_copy(&dir, COPY);
    let before = std::fs::read(&project).unwrap();
    let library = library(&dir, "Kicks 1 Select.multisample");
    let relinks = [relink(&dir, COPY, &library)];
    let fixed = fix_bitwig_project(&project, &relinks).unwrap();

    assert_eq!(fixed.file, dir.join("Song (fixed).bwproject"));
    assert_eq!(fixed.changed, relinks);
    assert!(fixed.left.is_empty(), "{:?}", fixed.left);
    assert_eq!(std::fs::read(&project).unwrap(), before, "the original is never changed");
    let t = texts(&fixed.file);
    let full = library.to_string_lossy().into_owned();
    assert!(t.contains(&"Kicks 1 Select.multisample".to_owned()));
    assert_eq!(
        t.iter().filter(|x| **x == full).count(),
        4,
        "relative, absolute, the old Library path and the other project's copy: {t:?}"
    );
    // The used files list holds only files inside the project's folder, as Bitwig's does: the copy leaves it.
    let doc = bitwig::decode(std::fs::read(&fixed.file).unwrap()).unwrap();
    let used = doc.meta.iter().find(|(k, _)| k.as_str() == "used_files").map(|(_, v)| v.clone());
    assert_eq!(used, Some(Value::Strings(Vec::new())));
    assert!(!t.iter().any(|x| x.contains("-3-2-2")), "{t:?}");
    // Asked again: the first fixed project is kept, the new one is numbered, and the copy is still free to go.
    assert_eq!(fixed_file(&project), Some(dir.join("Song (fixed 2).bwproject")));
    let again = fix_bitwig_project(&project, &relinks).unwrap();
    assert_eq!(again.file, dir.join("Song (fixed 2).bwproject"));
    assert_eq!(again.changed, relinks);
}

#[test]
fn a_copy_still_named_elsewhere_isnt_offered_for_the_bin() {
    let dir = folder("relink kept");
    // A plugin keeping the copy's name in its own settings, where this doesn't change it.
    let project = project_with_copy(&dir, COPY);
    let mut doc = bitwig::decode(std::fs::read(&project).unwrap()).unwrap();
    let root = doc.root.unwrap();
    let settings: Vec<u8> = format!("path={COPY}").encode_utf16().flat_map(u16::to_be_bytes).collect();
    doc.obj_mut(root).fields.push(Field { id: 0x7f20, value: Value::Blob(Box::new(Blob::new(&settings))) });
    std::fs::write(&project, bitwig::encode(&doc, None).unwrap()).unwrap();

    let relinks = [relink(&dir, COPY, &library(&dir, "Kicks 1 Select.multisample"))];
    let err = fix_bitwig_project(&project, &relinks).unwrap_err();
    assert!(err.contains("Kicks 1 Select-3-2-2.multisample: the project still names it"), "{err}");
    assert!(!dir.join("Song (fixed).bwproject").exists());
}

#[test]
fn a_copy_another_project_in_the_folder_uses_is_kept() {
    let dir = folder("relink sibling");
    let project = project_with_copy(&dir, COPY);
    std::fs::copy(&project, dir.join("Song older.bwproject")).unwrap();
    let relinks = [relink(&dir, COPY, &library(&dir, "Kicks 1 Select.multisample"))];
    // The fixed project is still written (it plays from the Library), but the copy isn't offered for the bin.
    let fixed = fix_bitwig_project(&project, &relinks).unwrap();
    assert!(fixed.changed.is_empty());
    assert!(
        fixed.left[0].contains("Song older.bwproject in the project's folder may still use it"),
        "{:?}",
        fixed.left
    );
}

#[test]
fn a_copy_named_as_its_library_file_is_matched_by_its_folder() {
    let dir = folder("relink same name");
    let name = "Kicks 1 Select.multisample";
    let project = project_with_copy(&dir, name);
    let library = library(&dir, name);
    let fixed = fix_bitwig_project(&project, &[relink(&dir, name, &library)]).unwrap();
    assert_eq!(fixed.changed.len(), 1, "{:?}", fixed.left);
    let t = texts(&fixed.file);
    // The bare name was already the Library file's; the paths into the project's folder now lead to the Library.
    assert!(!t.iter().any(|x| x.to_lowercase().contains("multi-samples")), "{t:?}");
}

#[test]
fn bad_lists_and_unused_copies_write_nothing() {
    let dir = folder("relink none");
    let project = project_with_copy(&dir, "Kicks 1 Select-3.multisample");
    let library = library(&dir, "Other.multisample");
    let err = |relinks: &[Relink]| fix_bitwig_project(&project, relinks).unwrap_err();
    // Not used by this project.
    std::fs::write(dir.join("multi-samples").join("Other-2.multisample"), b"<multisample/>").unwrap();
    assert!(err(&[relink(&dir, "Other-2.multisample", &library)]).contains("uses none"));
    // Somewhere else, or a Library file that isn't there.
    let elsewhere = Relink { copy: dir.join("Kicks 1 Select-3.multisample"), library: library.clone() };
    assert!(err(&[elsewhere]).contains("isn't in this project's multi-samples folder"));
    let missing = dir.join("Library").join("Gone.multisample");
    assert!(err(&[relink(&dir, "Kicks 1 Select-3.multisample", &missing)]).contains("isn't a Library file"));
    assert!(!dir.join("Song (fixed).bwproject").exists());
}

/// A zip with one deflated file in it, as Bitwig keeps plugin settings.
fn zip_with(name: &str, data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    out.start_file(name, options).unwrap();
    out.write_all(data).unwrap();
    out.finish().unwrap().into_inner()
}

#[test]
fn a_copy_named_in_packed_plugin_settings_or_an_auto_backup_is_kept() {
    let dir = folder("relink zip");
    let project = project_with_copy(&dir, COPY);
    let doc = bitwig::decode(std::fs::read(&project).unwrap()).unwrap();
    let state: Vec<u8> = COPY.encode_utf16().flat_map(u16::to_le_bytes).collect();
    std::fs::write(&project, bitwig::encode(&doc, Some(&zip_with("plugin-states/1.state", &state))).unwrap()).unwrap();
    let relinks = [relink(&dir, COPY, &library(&dir, "Kicks 1 Select.multisample"))];
    let err = fix_bitwig_project(&project, &relinks).unwrap_err();
    assert!(err.contains("still names it"), "{err}");

    // An auto-backup in a folder below names it too.
    let dir = folder("relink backup");
    let project = project_with_copy(&dir, COPY);
    std::fs::create_dir_all(dir.join("auto-backups")).unwrap();
    std::fs::copy(&project, dir.join("auto-backups").join("Song-1.bwproject")).unwrap();
    let relinks = [relink(&dir, COPY, &library(&dir, "Kicks 1 Select.multisample"))];
    let fixed = fix_bitwig_project(&project, &relinks).unwrap();
    assert!(fixed.changed.is_empty());
    assert!(fixed.left[0].contains("Song-1.bwproject"), "{:?}", fixed.left);
}

#[test]
fn an_accented_copy_is_found_in_any_letter_case() {
    let dir = folder("relink accents");
    let copy = "Über Bass-2.multisample";
    let project = project_with_copy(&dir, copy);
    // Another project names it in capitals.
    let mut doc = bitwig::decode(std::fs::read(&project).unwrap()).unwrap();
    let root = doc.root.unwrap();
    doc.obj_mut(root).fields.push(Field { id: 0x7f21, value: Value::Str(Text::new(&copy.to_uppercase())) });
    std::fs::write(dir.join("Other.bwproject"), bitwig::encode(&doc, None).unwrap()).unwrap();
    let relinks = [relink(&dir, copy, &library(&dir, "Über Bass.multisample"))];
    let fixed = fix_bitwig_project(&project, &relinks).unwrap();
    assert!(fixed.changed.is_empty(), "{:?}", fixed.left);
    assert!(fixed.left[0].contains("Other.bwproject"), "{:?}", fixed.left);
    assert!(!texts(&fixed.file).iter().any(|t| t.contains("-2")), "the fixed project itself points at the Library");
}

#[test]
fn an_edited_copy_is_left_alone() {
    let dir = folder("relink edited");
    let project = project_with_copy(&dir, COPY);
    let library = library(&dir, "Kicks 1 Select.multisample");
    std::fs::write(dir.join("multi-samples").join(COPY), b"<multisample edited/>").unwrap();
    let err = fix_bitwig_project(&project, &[relink(&dir, COPY, &library)]).unwrap_err();
    assert!(err.contains("isn't the same as its Library file"), "{err}");
    assert!(!dir.join("Song (fixed).bwproject").exists());
}

#[test]
fn a_copy_is_asked_about_again_before_the_bin() {
    let dir = folder("relink still named");
    let project = project_with_copy(&dir, COPY);
    let relink = relink(&dir, COPY, &library(&dir, "Kicks 1 Select.multisample"));
    let fixed = fix_bitwig_project(&project, std::slice::from_ref(&relink)).unwrap();
    assert_eq!(still_named(&project, &relink), None, "only the original names it");
    // Saved again in Bitwig since, and the new save uses the copy again.
    std::fs::copy(&project, &fixed.file).unwrap();
    let why = still_named(&project, &relink).unwrap();
    assert!(why.starts_with("Song (fixed).bwproject still uses Kicks 1 Select-3-2-2.multisample"), "{why}");
    // The Library file gone: the copy stays.
    std::fs::write(&fixed.file, b"").unwrap();
    std::fs::remove_file(&relink.library).unwrap();
    assert!(still_named(&project, &relink).unwrap().contains("isn't a Library file"));
}

/// A file reference (0x2a) to a path.
fn file_ref(doc: &mut bitwig::Document, path: &str) -> bitwig::ObjId {
    let mut r = Object::new(0x2a);
    r.fields.push(text(0x38, "PC"));
    r.fields.push(text(0x3a, "user"));
    r.fields.push(text(0x3b, path));
    doc.add(r)
}

/// A sample file (0x4a6) laid out as Bitwig saves one: its name, where it is inside the project (or ""), its file
/// reference, the files it was found at before, and its size.
fn sample_file(
    doc: &mut bitwig::Document,
    name: &str,
    inside: &str,
    path: &str,
    found: &[&str],
    size: i32,
) -> bitwig::ObjId {
    let file = file_ref(doc, path);
    let before: Vec<Slot> = found.iter().map(|p| Slot::Obj(file_ref(doc, p))).collect();
    let mut location = Object::new(0x2b4);
    location.fields.push(Field { id: 0xc5b, value: Value::List(before.clone()) });
    let location = doc.add(location);
    let mut o = Object::new(0x4a6);
    o.fields.push(text(0x512, name));
    o.fields.push(text(0xd3a, inside));
    o.fields.push(text(0xcd4, ""));
    o.fields.push(Field { id: 0xd3b, value: Value::Obj(Slot::Obj(file)) });
    o.fields.push(Field { id: 0x15, value: Value::Obj(Slot::Obj(location)) });
    o.fields.push(Field { id: 0x17, value: Value::I32(size) });
    let o = doc.add(o);
    doc.objects[file.0 as usize].parent = Some((o, 0xd3b));
    doc.objects[location.0 as usize].parent = Some((o, 0x15));
    for s in before {
        if let Slot::Obj(r) = s {
            doc.objects[r.0 as usize].parent = Some((location, 0xc5b));
        }
    }
    let root = doc.root.unwrap();
    doc.objects[o.0 as usize].parent = Some((root, 0x7f10));
    doc.obj_mut(root).fields.push(Field { id: 0x7f10, value: Value::Obj(Slot::Obj(o)) });
    o
}

fn path_of(doc: &bitwig::Document, r: bitwig::ObjId) -> String {
    match doc.obj(r).get(0x3b) {
        Some(Value::Str(t)) => t.as_str().to_owned(),
        other => panic!("no path: {other:?}"),
    }
}

/// The texts of the nth sample file in the fixed project (in the order they were added): its name, where it is inside the project, its file reference and
/// the files it was found at before.
fn sample_file_texts(doc: &bitwig::Document, nth: usize) -> (String, String, String, Vec<String>) {
    let root = doc.root.unwrap();
    let added: Vec<Slot> = doc
        .obj(root)
        .fields
        .iter()
        .filter(|f| f.id == 0x7f10)
        .filter_map(|f| match f.value {
            Value::Obj(s) => Some(s),
            _ => None,
        })
        .collect();
    let o = doc.resolve(added[nth]).unwrap();
    let s = |id| match doc.obj(o).get(id) {
        Some(Value::Str(t)) => t.as_str().to_owned(),
        other => panic!("no text {id:#x}: {other:?}"),
    };
    let file = doc.field_obj(o, 0xd3b).unwrap();
    let location = doc.field_obj(o, 0x15).unwrap();
    let before = doc.field_list(location, 0xc5b).into_iter().map(|r| path_of(doc, r)).collect();
    (s(0x512), s(0xd3a), path_of(doc, file), before)
}

/// Solomon's neon rmx 40 (fixed) showed its multisamples as missing in Bitwig: the Library path had gone where Bitwig
/// keeps a path inside the project's folder, and the file reference still named the old drive. Now a copy's sample
/// file is written as Bitwig writes a Library file it didn't copy, and files whose folder moved (Bitwig Studio's folder
/// from I: to here, Bitwig's Library from Documents) are found where they are now, when the size is the same.
#[test]
fn a_copy_becomes_a_library_file_as_bitwig_writes_one_and_moved_files_are_found() {
    let root = folder("relink layout");
    let dir = root.join("Bitwig Studio").join("Projects").join("Song");
    std::fs::create_dir_all(dir.join("multi-samples")).unwrap();
    std::fs::write(dir.join("multi-samples").join(COPY), b"<multisample/>").unwrap();
    let library = library(&root.join("Bitwig Studio"), "Kicks 1 Select.multisample");
    let full = library.to_string_lossy().into_owned();
    // A wavetable in the Library, and a Library multisample a template's copy named, both saved on old drives.
    let tables = root.join("Bitwig Studio").join("Library").join("Wavetables").join("Analog");
    std::fs::create_dir_all(&tables).unwrap();
    std::fs::write(tables.join("Basic Shapes.wt"), [0u8; 12]).unwrap();
    std::fs::write(
        root.join("Bitwig Studio").join("Library").join("Multisamples").join("Snare 2 Select.multisample"),
        [0u8; 20],
    )
    .unwrap();
    std::fs::write(tables.join("Wrong Size.wt"), [0u8; 5]).unwrap();

    let mut doc = bitwig::decode(fixtures::bitwig_template(fixtures::Template::default())).unwrap();
    sample_file(
        &mut doc,
        COPY,
        &format!("multi-samples/{COPY}"),
        r"I:\Bitwig Studio\Projects\Garage Project\multi-samples\Kicks 1 Select-3-2.multisample",
        &[&format!(r"I:\Bitwig Studio\Projects\Song\multi-samples\{COPY}")],
        14,
    );
    // A sample file that names the copy only among the files it was found at before.
    sample_file(
        &mut doc,
        "Kicks 1 Select.multisample",
        "",
        r"I:\Bitwig Studio\Projects\Facebook Garage\multi-samples\Kicks 1 Select-2.multisample",
        &[
            &format!(r"I:\Bitwig Studio\Projects\Song\multi-samples\{COPY}"),
            r"D:\Library\Multisamples\Kicks 1 Select.multisample",
        ],
        14,
    );
    sample_file(
        &mut doc,
        "Basic Shapes.wt",
        "",
        r"I:\Bitwig Studio\Library\Wavetables\Analog\Basic Shapes.wt",
        &[],
        12,
    );
    sample_file(
        &mut doc,
        "Snare 2 Select.multisample",
        "",
        r"C:\Users\someone\Documents\Bitwig Studio\Library\Multisamples\Snare 2 Select.multisample",
        &[r"F:\Bitwig Studio\Library\Templates\Keys.bwtemplate\project\multi-samples\Snare 2 Select-3-2.multisample"],
        20,
    );
    sample_file(&mut doc, "Wrong Size.wt", "", r"I:\Bitwig Studio\Library\Wavetables\Analog\Wrong Size.wt", &[], 9);
    for (k, v) in &mut doc.meta {
        if k.as_str() == "used_files" {
            *v = Value::Strings(vec![Text::new(&format!("multi-samples/{COPY}")), Text::new("samples/Kick.wav")]);
        }
    }
    let zip = doc.zip().map(<[u8]>::to_vec);
    let project = dir.join("Song.bwproject");
    std::fs::write(&project, bitwig::encode(&doc, zip.as_deref()).unwrap()).unwrap();

    let relinks = [relink(&dir, COPY, &library)];
    let fixed = fix_bitwig_project(&project, &relinks).unwrap();
    assert_eq!(fixed.changed, relinks, "{:?}", fixed.left);
    let back = bitwig::decode(std::fs::read(&fixed.file).unwrap()).unwrap();
    assert_eq!(back.body_error, None);
    let lib = "Kicks 1 Select.multisample".to_owned();
    assert_eq!(sample_file_texts(&back, 0), (lib.clone(), String::new(), full.clone(), vec![full.clone()]));
    assert_eq!(sample_file_texts(&back, 1), (lib, String::new(), full.clone(), vec![full.clone(), full.clone()]));
    let shapes = tables.join("Basic Shapes.wt").to_string_lossy().into_owned();
    assert_eq!(sample_file_texts(&back, 2).2, shapes, "moved with Bitwig Studio's folder");
    let snare_now = root.join("Bitwig Studio").join("Library").join("Multisamples").join("Snare 2 Select.multisample");
    let snare_now = snare_now.to_string_lossy().into_owned();
    let (_, _, file, before) = sample_file_texts(&back, 3);
    assert_eq!(file, snare_now, "found in the Library now");
    assert_eq!(before, vec![snare_now], "and the template's copy that's gone names it too");
    assert_eq!(
        sample_file_texts(&back, 4).2,
        r"I:\Bitwig Studio\Library\Wavetables\Analog\Wrong Size.wt",
        "another size isn't the same file"
    );
    assert_eq!(fixed.found.len(), 2, "{:?}", fixed.found);
    let used = back.meta.iter().find(|(k, _)| k.as_str() == "used_files").map(|(_, v)| v.clone());
    assert_eq!(used, Some(Value::Strings(vec![Text::new("samples/Kick.wav")])));
}
