//! The Curves tab's curves: a `curves.db` made as the app makes it from made-up curve files, read by the plugin
//! without changing it, searched as the app's list is, and a curve handed to a drag as a Bitwig curve file.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use mo_app::presets::CurveFilters;
use mo_formats::bitwig::curve;
use mo_plugin::curves::{self, Read};
use mo_presets::curves::{CurvesDb, FILE_NAME, Progress};
use std::path::{Path, PathBuf};

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A made-up curve file in Bitwig's library format, rising through `ys`.
fn curve_file(dir: &Path, name: &str, ys: &[f64]) -> String {
    let n = ys.len() as f64 - 1.0;
    let points = ys.iter().enumerate().map(|(i, &y)| curve::CurvePoint { x: i as f64 / n, y, bend: 0.0 }).collect();
    let c = curve::Curve {
        name: name.into(),
        creator: "Test".into(),
        comment: String::new(),
        category: "Sequence".into(),
        tags: String::new(),
        beat_length: Some(4.0),
        points,
    };
    let path = dir.join(format!("{name}.bwcurve"));
    std::fs::write(&path, curve::write(&c).unwrap()).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn the_apps_curves_are_listed_searched_and_dragged_as_a_bitwig_curve() {
    let dir = folder("curves-listed");
    let db_path = dir.join(FILE_NAME);
    assert!(matches!(curves::read_from(&db_path), Read::Not(why) if why.contains("open Presets")));
    assert!(!db_path.exists(), "the plugin makes no curves.db of its own");
    {
        let mut db = CurvesDb::open(&db_path).unwrap();
        let files = [curve_file(&dir, "Rise", &[0.0, 1.0]), curve_file(&dir, "Fall", &[1.0, 0.2, 0.0])];
        db.scan(&files, &Progress::default()).unwrap();
    }
    let before = std::fs::read(&db_path).unwrap();
    let Read::Ready(found) = curves::read_from(&db_path) else { panic!("not read") };
    assert_eq!(found.curves.len(), 2);
    assert_eq!(found.view(&CurveFilters::default()).0.len(), 2);
    let rise = CurveFilters { search: "rise".into(), ..CurveFilters::default() };
    let shown = found.view(&rise).0;
    assert_eq!(shown.len(), 1);
    assert_eq!(found.curves[shown[0]].name, "Rise");
    assert_eq!(curves::count_line(2, Some(1)), "1 of 2 curves");

    let temp = dir.join("temp");
    std::fs::create_dir_all(&temp).unwrap();
    let dragged = found.to_drag(shown[0], &temp).unwrap();
    assert_eq!(dragged.file_name().unwrap(), "Rise.bwcurve");
    assert!(dragged.starts_with(&temp));
    let back = curve::read(std::fs::read(&dragged).unwrap()).unwrap();
    assert_eq!(back.points, found.curves[shown[0]].points);
    assert!(found.to_drag(99, &temp).is_err());
    assert_eq!(std::fs::read(&db_path).unwrap(), before, "curves.db is only read");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_edited_curve_is_kept_as_a_new_curve_in_the_apps_list() {
    let dir = folder("curves-saved");
    let db_path = dir.join(FILE_NAME);
    let p = |x: f64, y: f64| curve::CurvePoint { x, y, bend: 0.0 };
    let shape = (vec![p(0.0, 0.0), p(0.5, 1.0), p(1.0, 0.0)], 4.0);
    // No list yet: nothing is made for it, and the words say what to do.
    let none = curves::save_as_new(&db_path, "from".into(), "Rise", shape.clone()).err().unwrap();
    assert!(none.contains("Open Presets"), "{none}");
    assert!(!db_path.exists(), "the plugin makes no curves.db of its own");
    {
        let mut db = CurvesDb::open(&db_path).unwrap();
        db.scan(&[curve_file(&dir, "Rise", &[0.0, 1.0])], &Progress::default()).unwrap();
    }
    let Read::Ready(found) = curves::read_from(&db_path) else { panic!("not read") };
    let from = found.curves[0].id.clone();
    let saved = curves::save_as_new(&db_path, from.clone(), "Rise", shape.clone()).unwrap();
    assert_eq!((saved.curve.name.as_str(), saved.from.as_str()), ("Rise (edited)", from.as_str()));
    assert_eq!(saved.curve.points, shape.0);
    let id = saved.curve.id.clone();
    let (list, at) = found.with(saved);
    assert_eq!((list.curves.len(), at), (2, 1));
    // The app reads it from its own file, and the plugin reads it again next time.
    assert!(CurvesDb::open(&db_path).unwrap().list().unwrap().iter().any(|c| c.id == id && c.name == "Rise (edited)"));
    let Read::Ready(again) = curves::read_from(&db_path) else { panic!("not read") };
    assert_eq!(again.curves.len(), 2);
    // The same shape saved again is the same curve, in its place in the list.
    let twice = curves::save_as_new(&db_path, from, "Another name", shape).unwrap();
    assert_eq!(twice.curve.name, "Rise (edited)");
    let (list, at) = list.with(twice);
    assert_eq!((list.curves.len(), at), (2, 1));
    // What's no curve is refused, in words.
    let bad = curves::save_as_new(&db_path, String::new(), "Bad", (vec![p(0.0, 0.0)], 4.0)).err().unwrap();
    assert!(bad.starts_with("Couldn't save the curve"), "{bad}");
    let _ = std::fs::remove_dir_all(&dir);
}
