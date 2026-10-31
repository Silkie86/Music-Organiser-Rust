//! Pointing a Bitwig project's multisample copies back at the Library, on a made-up project in the test build's own
//! fixtures folder.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
#[path = "../../mo-formats/tests/common/bitwig.rs"]
mod fixtures;

use std::path::{Path, PathBuf};

use mo_convert::relink::{Relink, fix_bitwig_project, fixed_file};
use mo_formats::bitwig::{self, Field, Object, Slot, Text, Value};

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
    file.fields.push(text(0x7f05, &format!(r"I:\Bitwig Studio\Projects\Song\multi-samples\{copy}")));
    file.fields.push(Field { id: 0x7f06, value: Value::Obj(Slot::Obj(origin)) });
    if let Some(other) = also {
        file.fields.push(text(0x7f07, other));
    }
    let file = doc.add(file);
    doc.objects[origin.0 as usize].parent = Some((file, 0x7f06));
    let root = doc.root.unwrap();
    doc.objects[file.0 as usize].parent = Some((root, 0x7f10));
    doc.obj_mut(root).fields.push(Field { id: 0x7f10, value: Value::Obj(Slot::Obj(file)) });
    let zip = doc.zip().map(<[u8]>::to_vec);
    let bytes = bitwig::encode(&doc, zip.as_deref()).unwrap();
    let path = dir.join("Song.bwproject");
    std::fs::write(&path, bytes).unwrap();
    path
}

fn texts(path: &Path) -> Vec<String> {
    let doc = bitwig::decode(std::fs::read(path).unwrap()).unwrap();
    assert_eq!(doc.body_error, None);
    doc.objects
        .iter()
        .flat_map(|o| &o.fields)
        .filter_map(|f| match &f.value {
            Value::Str(t) => Some(t.as_str().to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn every_reference_to_a_copy_points_at_the_library_file() {
    let dir = folder("relink");
    let copy = "Kicks 1 Select-3-2-2.multisample";
    let project = project_with_copy(&dir, copy, None);
    let before = std::fs::read(&project).unwrap();
    let library = dir.join("Library").join("Multisamples").join("Kicks 1 Select.multisample");
    let relinks = [Relink { copy: dir.join("multi-samples").join(copy), library: library.clone() }];
    let fixed = fix_bitwig_project(&project, &relinks).unwrap();

    assert_eq!(fixed.file, dir.join("Song (fixed).bwproject"));
    assert_eq!(fixed.changed, relinks);
    assert!(fixed.left.is_empty(), "{:?}", fixed.left);
    assert_eq!(std::fs::read(&project).unwrap(), before, "the original is never changed");
    let t = texts(&fixed.file);
    let full = library.to_string_lossy().into_owned();
    assert!(t.contains(&"Kicks 1 Select.multisample".to_owned()));
    assert_eq!(t.iter().filter(|x| **x == full).count(), 3, "relative, absolute and the old Library path: {t:?}");
    assert!(!t.iter().any(|x| x.contains("-3-2-2")), "{t:?}");
    // Asked again: the first fixed project is kept, the new one is numbered.
    assert_eq!(fixed_file(&project), dir.join("Song (fixed 2).bwproject"));
}

#[test]
fn a_copy_still_named_elsewhere_isnt_offered_for_the_bin() {
    let dir = folder("relink kept");
    let copy = "Kicks 1 Select-3-2-2.multisample";
    // A plugin keeping the copy's name in a list this doesn't change.
    let project = project_with_copy(&dir, copy, None);
    let mut doc = bitwig::decode(std::fs::read(&project).unwrap()).unwrap();
    let root = doc.root.unwrap();
    doc.obj_mut(root).fields.push(Field { id: 0x7f20, value: Value::Strings(vec![Text::new(copy)]) });
    let zip = doc.zip().map(<[u8]>::to_vec);
    std::fs::write(&project, bitwig::encode(&doc, zip.as_deref()).unwrap()).unwrap();

    let relinks = [
        Relink { copy: dir.join("multi-samples").join(copy), library: dir.join("Kicks 1 Select.multisample") },
        Relink { copy: dir.join("multi-samples").join("Other-2.multisample"), library: dir.join("Other.multisample") },
    ];
    let err = fix_bitwig_project(&project, &relinks).unwrap_err();
    assert!(err.contains("Kicks 1 Select-3-2-2.multisample is still mentioned"), "{err}");
    assert!(!dir.join("Song (fixed).bwproject").exists());
}

#[test]
fn a_project_that_uses_none_of_the_copies_gets_no_fixed_file() {
    let dir = folder("relink none");
    let project = project_with_copy(&dir, "Kicks 1 Select-3.multisample", None);
    let relinks = [Relink {
        copy: dir.join("multi-samples").join("Other-2.multisample"),
        library: dir.join("Other.multisample"),
    }];
    assert!(fix_bitwig_project(&project, &relinks).is_err());
    assert!(!dir.join("Song (fixed).bwproject").exists());
}
