//! Tries the clone line-up on FL projects from a terminal.
//!
//!   cargo run --release -p mo-convert --example fl_lineup -- <project.flp>           what would move (writes nothing)
//!   cargo run --release -p mo-convert --example fl_lineup -- <project.flp> --write   writes "<name> (fixed).flp" beside it
//!   cargo run --release -p mo-convert --example fl_lineup -- --survey <folder>       counts over every .flp in a folder
//!
//! The project itself is never changed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mo_convert::fl_clones::{fix_fl_project, line_up};
use rayon::prelude::*;

fn flps(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            flps(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("flp")) {
            out.push(p);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag, folder] if flag == "--survey" => survey(Path::new(folder)),
        [file] => show(Path::new(file), false),
        [file, flag] if flag == "--write" => show(Path::new(file), true),
        _ => eprintln!("fl_lineup <project.flp> [--write] | fl_lineup --survey <folder>"),
    }
}

fn show(file: &Path, write: bool) {
    let raw = match std::fs::read(file) {
        Ok(r) => r,
        Err(e) => return eprintln!("Couldn't read {}: {e}", file.display()),
    };
    match line_up(&raw) {
        Ok(l) => {
            for g in &l.groups {
                println!("group: P{} with clones {:?}", g.source, g.clones);
            }
            for s in l.summary() {
                println!("move: {s}");
            }
            for s in &l.left {
                println!("note: {s}");
            }
            if l.rows {
                println!("rows: this save's pattern rows become pattern clips on tracks");
            }
        }
        Err(e) => println!("can't: {e}"),
    }
    if write {
        match fix_fl_project(file) {
            Ok(f) => println!("written: {}", f.file.display()),
            Err(e) => println!("not written: {e}"),
        }
    }
}

/// Groups, moved clips, notes, and whether it had pattern rows.
type Counts = (usize, usize, usize, bool);

fn survey(folder: &Path) {
    let mut files = Vec::new();
    flps(folder, &mut files);
    let results: Vec<(PathBuf, Result<Counts, String>)> = files
        .par_iter()
        .map(|f| {
            let r = std::fs::read(f).map_err(|e| e.to_string()).and_then(|raw| line_up(&raw));
            (f.clone(), r.map(|l| (l.groups.len(), l.moves.len(), l.left.len(), l.rows)))
        })
        .collect();
    let mut errors: BTreeMap<String, Vec<&PathBuf>> = BTreeMap::new();
    let (mut with_groups, mut with_moves, mut rows, mut moves, mut notes) = (0, 0, 0, 0, 0);
    for (f, r) in &results {
        match r {
            Ok((g, m, n, row)) => {
                with_groups += usize::from(*g > 0);
                with_moves += usize::from(*m > 0);
                rows += usize::from(*row);
                moves += m;
                notes += n;
            }
            Err(e) => errors.entry(e.clone()).or_default().push(f),
        }
    }
    println!(
        "{} projects; {with_groups} with clones; {with_moves} with clips to move ({moves} clips, {notes} notes); {rows} with pattern rows",
        results.len()
    );
    for (e, fs) in &errors {
        println!("{} x {e}", fs.len());
        for f in fs.iter().take(5) {
            println!("    {}", f.display());
        }
    }
}
