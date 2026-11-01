//! The theory bench: runs the harmony engine, and the C# app's engine as it was, over music other people labelled,
//! and says how often each gets the key and the chords right. See README.md in this folder.
//!
//! The labelled music is made from public datasets by the scripts in `convert/`, or from Solomon's own MIDI packs by
//! `theorybench packs` on his computer. It stays outside the repository; nothing from it goes into the app but the
//! numbers fitted on it (and from his packs, only those rule 5 allows).

mod answerfit;
mod baseline;
mod chords;
mod fit;
mod harte;
mod item;
mod keyfit;
mod keys;
mod metrics;
mod naming;
mod packs;
mod profiles;

use std::process::ExitCode;

const USAGE: &str = "theorybench keys <labelled.jsonl>... [--part <role>] [--aligned] [--bars <lengths>]
theorybench keyfit <labelled.jsonl>... [--aligned] [--bars <lengths>] [--lambda-delta <x>|inf] [--inputs <kinds>]
                   [--picks <n>] [--weigh <source>=<w>] [--dump <file>] [--against <file>] [--hand <cases>]
                   [--shipped | --rescale] [--final | --write <weights.rs>]
theorybench packs <folder>... --out <file.jsonl>
theorybench chords <labelled.jsonl>... --answers <name>=<answers.jsonl> [--answers ...] [--part <role>]
                   [--per-segment] [--by <tag>]... [--dump <file>]

  keys       how often each key finder names the labelled key, on whole songs and on clips (8 bars long unless --bars
             gives other lengths, such as 1,2,4,8,16)
  --part     hear only the parts with this role (melody, accompaniment, bass, chords); all pitched parts when left out
  --aligned  only songs with four beats a bar whose labelled bar lines fall on the MIDI's grid (the C# bench's songs)
  keyfit     fits mo-harmony's key finder on folds 1 to 4 of the songs (each fold scored by a fit on the others)
             and measures it beside the C# app: whole songs, stretches and openings of 1 to 16 bars (--bars)
  --lambda-delta  how hard the per-note corrections are pulled to 0 (1e-4; inf fits the counted tables alone)
  --inputs   the kinds of input heard: full,tune,upper,bass,piano (all)
  --picks    stretches of each length fitted on from each song (8)
  --weigh    how much a source's sets count in the fit (1; 0 for its tables only); source/input for one kind of
             input, such as billboard/upper=0. Pieces whose licence isn't MIT, CC0 or CC BY are only measured
  --dump     writes how each case of folds 1 to 4 came out, to compare a later run with (--against)
  --against  compares this run with one dumped before, case by case: each set's share right and log-loss, the
             differences with their likely ranges, and every set that got worse beyond its range
  --hand     prints each fitted model's reading of these hand cases (such as 13,14,16)
  --final    fits on folds 1 to 4 and scores the kept-back fold 0, once, logging the run in test-runs.md
  --write    fits on every song and writes the weights file
  --shipped  scores the finder built in, as it is (folds 1 to 4; nothing fitted), to compare a refit with
  --rescale  fits only each stream's scale, the minor weight and the calibration of the finder built in, on Solomon's
             own pieces alone (rule 5's exception); its thresholds are kept
  packs      Solomon's MIDI packs as labelled music, on his computer: each part of each file whose name gives one key
  --out      where to write them: a .jsonl outside this repository
  chords     how often each chord reader names the labelled chord, scored as mir_eval scores it, then the whole
             chord: its notes and bass, each extension, the root among the same notes, the bass kind, readings listed
  --answers  the chords an engine read in each piece, as the C# helper in cs/ writes them; the first is the baseline
  --part     the parts the engines heard (as given to the helper; all but drums when left out), for the roots no
             part plays
  --per-segment  mir_eval's scores per labelled chord too, each counted once
  --by       the chords named whole split by a tag convert/voice.py gives each chord it plays: set, recipe, texture,
             tune, push, chart or quality (and context and role in its sweep); given twice, by both
  --dump     write every piece's labels, chords read and scores to a file, for convert/check_mir_eval.py";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    low_priority_threads();
    let result = match args.first().map(String::as_str) {
        Some("keys") => keys::run(&args[1..]),
        Some("keyfit") => keyfit::run(&args[1..]),
        Some("chords") => chords::run(&args[1..]),
        Some("packs") => packs::run(&args[1..]),
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
