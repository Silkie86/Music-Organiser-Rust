//! Lists the MIDI parts the index would find in one project or `.mid` file, without the index: every part with its
//! notes, bars and kind, and the phrases joined from several channels with their pieces. Only reads the file.
//!
//! `cargo run -p mo-midi --example parts -- <file>`

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("cargo run -p mo-midi --example parts -- <project or .mid file>");
        return ExitCode::FAILURE;
    };
    let found = match mo_midi::extract::read(Path::new(&path), &[]) {
        Ok(found) => found,
        Err(e) => {
            eprintln!("Couldn't read it: {e:?}");
            return ExitCode::FAILURE;
        }
    };
    println!("{} parts", found.len());
    for f in &found {
        let bars = mo_midi::analyser::bars_of(&f.notes);
        let (low, high) = (f.notes.iter().map(|n| n.key).min(), f.notes.iter().map(|n| n.key).max());
        println!(
            "{:<28} {:<36} {:>4} notes {:>3} bars  {:?}  keys {:?}-{:?}",
            f.part,
            f.clip,
            f.notes.len(),
            bars,
            f.analysis.role,
            low.unwrap_or(0),
            high.unwrap_or(0)
        );
        for m in &f.members {
            println!("    piece {} from bar {}, {} bars", m.hash, m.start / mo_midi::BAR + 1, m.length / mo_midi::BAR);
        }
    }
    ExitCode::SUCCESS
}
