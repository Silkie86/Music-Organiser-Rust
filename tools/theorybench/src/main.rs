//! The theory bench: runs the harmony engine, and the C# app's engine as it was, over music other people labelled,
//! and says how often each gets the key and the chords right. See README.md in this folder.
//!
//! The labelled music is made from public datasets by the scripts in `convert/` and stays outside the repository;
//! nothing from it goes into the app.

mod baseline;
mod chords;
mod harte;
mod item;
mod keys;
mod metrics;
mod profiles;

use std::process::ExitCode;

const USAGE: &str = "theorybench keys <labelled.jsonl>... [--part <role>] [--aligned] [--bars <lengths>]
theorybench chords <labelled.jsonl>... --answers <name>=<answers.jsonl> [--answers ...]

  keys       how often each key finder names the labelled key, on whole songs and on clips (8 bars long unless --bars
             gives other lengths, such as 1,2,4,8,16)
  --part     hear only the parts with this role (melody, accompaniment, bass, chords); all pitched parts when left out
  --aligned  only songs with four beats a bar whose labelled bar lines fall on the MIDI's grid (the C# bench's songs)
  chords     how often each chord reader names the labelled chord, scored as mir_eval scores it
  --answers  the chords an engine read in each piece, as the C# helper in cs/ writes them; the first is the baseline";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    low_priority_threads();
    let result = match args.first().map(String::as_str) {
        Some("keys") => keys::run(&args[1..]),
        Some("chords") => chords::run(&args[1..]),
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}

/// All cores but one, at low priority (rule 21), so the computer stays quick while the bench runs.
fn low_priority_threads() {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).max(1));
    // If this pool can't be set up, rayon's own one does the work at normal priority: the numbers are the same.
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .start_handler(|_| {
            let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
        })
        .build_global();
}
