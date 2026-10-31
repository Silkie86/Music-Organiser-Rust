//! Pointing a Bitwig project's multisample copies back at the Library, on a made-up project in the test build's own
//! fixtures folder.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
#[path = "../../mo-formats/tests/common/bitwig.rs"]
mod fixtures;

use std::path::{Path, PathBuf};

use mo_convert::relink::{Relink, fix_bitwig_project, fixed_file};
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
fn project_with_copy(dir: &Path, copy: &str, also: Option<&str>) -> PathBuf {
    let mut doc = bitwig::decode(fixtures::bitwig_template(fixtures::Template::default())).unwrap();
    let mut origin = Object::new(0x7f01);
    origin.fields.push(text(
        0x7f02,
        r"C:\Users\someone\Documents\Bitwig Studio\Library\Multisamples\Kicks 1 Select.multisample",
    ));
    let origin = doc.add(origin);
    let mut file = Object::new(0x7f00);
    file.fields.push(text(0x7f03, copy));
    file.fields.push(text(0x7f04, &format!("multi-samples/{copy}")));
    // Saved on another drive, in a folder with this project's name.
    let folder = dir.file_name().unwrap().to_string_lossy();
    file.fields.push(text(0x7f05, &format!(r"I:\Bitwig Studio\Projects\{folder}\multi-samples\{copy}")));
    file.fields.push(Field { id: 0x7f06, value: Value::Obj(Slot::Obj(origin)) });
    if let Some(other) = also {
        file.fields.push(text(0x7f07, other));
    }
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
    let project = project_with_copy(&dir, COPY, None);
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
        "relative, absolute, the old Library path and the used files list: {t:?}"
    );
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
    let project = project_with_copy(&dir, COPY, None);
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
    let project = project_with_copy(&dir, COPY, None);
    std::fs::copy(&project, dir.join("Song older.bwproject")).unwrap();
    let relinks = [relink(&dir, COPY, &library(&dir, "Kicks 1 Select.multisample"))];
    // The fixed project is still written (it plays from the Library), but the copy isn't offered for the bin.
    let fixed = fix_bitwig_project(&project, &relinks).unwrap();
    assert!(fixed.changed.is_empty());
    assert!(fixed.left[0].contains("Song older.bwproject in the same folder may still use it"), "{:?}", fixed.left);
}

#[test]
fn a_copy_named_as_its_library_file_is_matched_by_its_folder() {
    let dir = folder("relink same name");
    let name = "Kicks 1 Select.multisample";
    let project = project_with_copy(&dir, name, None);
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
    let project = project_with_copy(&dir, "Kicks 1 Select-3.multisample", None);
    let library = library(&dir, "Other.multisample");
    // Not used by this project.
    assert!(fix_bitwig_project(&project, &[relink(&dir, "Other-2.multisample", &library)]).is_err());
    // No name, somewhere else, or a Library file that isn't there.
    assert!(fix_bitwig_project(&project, &[relink(&dir, "", &library)]).is_err());
    let elsewhere = Relink { copy: dir.join("Kicks 1 Select-3.multisample"), library: library.clone() };
    assert!(fix_bitwig_project(&project, &[elsewhere]).is_err());
    let missing = dir.join("Library").join("Gone.multisample");
    assert!(fix_bitwig_project(&project, &[relink(&dir, "Kicks 1 Select-3.multisample", &missing)]).is_err());
    assert!(!dir.join("Song (fixed).bwproject").exists());
}
