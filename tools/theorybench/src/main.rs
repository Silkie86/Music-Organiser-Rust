//! The theory bench: runs the harmony engine, and the C# app's engine as it was, over music other people labelled,
//! and says how often each gets the key and the chords right. See README.md in this folder.
//!
//! The labelled music is made from public datasets by the scripts in `convert/`, or from Solomon's own MIDI packs by
//! `theorybench packs` on his computer. It stays outside the repository; nothing from it goes into the app but the
//! numbers fitted on it (and from his packs, only those rule 5 allows).

mod answerfit;
mod baseline;
mod candidates;
mod chordcal;
mod chordfit;
mod chords;
mod fit;
mod harte;
mod item;
mod keycheck;
mod keyfind_v4;
mod keyfit;
mod keys;
mod metrics;
mod naming;
mod packs;
mod profiles;
mod ruled;
mod songkeys;
mod tables;

use std::process::ExitCode;

const USAGE: &str = "theorybench keys <labelled.jsonl>... [--part <role>] [--aligned] [--bars <lengths>]
theorybench keyfit <labelled.jsonl>... [--aligned] [--bars <lengths>] [--lambda-delta <x>|inf] [--inputs <kinds>]
                   [--picks <n>] [--weigh <source>=<w>] [--dump <file>] [--against <file>] [--hand <cases>]
                   [--shipped | --rescale] [--final | --write <weights.rs>]
theorybench packs <folder>... --out <file.jsonl>
theorybench keycheck <folder or file.mid>... [--changed | --counts]
theorybench songkeys <songs.db> [--misses | --counts]
theorybench chords <labelled.jsonl>... [--answers <name>=<answers.jsonl>]... [--engine rust] [--part <role>]
                   [--roles on|off] [--oracle-cuts] [--timing] [--weights <weights.json>] [--folds <folds>]
                   [--per-segment] [--by <tag>]... [--dump <file>]
theorybench chordfit <labelled.jsonl>... [--inputs <kinds>] [--weigh <source>=<w>]... [--train <folds>]
                   [--test <folds>] [--lambda <x>] [--steps <n>] [--from <weights.json>] [--fix <numbers>]
                   [--hand <mu>] [--hold <rounds>] [--out <weights.json>]
theorybench chordcal <labelled.jsonl>... [--weights <weights.json>] [--folds <folds>] [--picks <n>]
                   [--measure] [--calibration <calibration.json>] [--keep-sharpness] [--out <calibration.json>]
theorybench candidates <labelled.jsonl>... [--inputs <kinds>] [--misses <file>]
theorybench tables <labelled.jsonl>... [--write <tables.rs>]

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
             own pieces alone (rule 5's exception); its thresholds are kept. Each fit is pulled back toward the
             finder built in, to the last point where every hand case keeps its gate
  packs      Solomon's MIDI packs as labelled music, on his computer: each part of each file whose name gives one key
  --out      where to write them: a .jsonl outside this repository
  keycheck   the key finder built in beside version 4's (before the rescale on Solomon's packs), part by part, on MIDI
             files: what each says, the key the file's name gives, and how often each agrees with the names. Prints
             only; writes nothing
  --changed  only the parts the two answer differently
  --counts   only the counts at the end
  songkeys   the key finder on Solomon's own songs, whole, as they play: the key typed on the Songs page (and,
             apart, the key a song's name gives) beside the old app's, version 4's and the one built in (as the song
             plays, and each pattern heard once). Reads the library and the projects, prints only; writes nothing
  --misses   only the songs the finder built in misses
  --counts   only the counts
  chords     how often each chord reader names the labelled chord, scored as mir_eval scores it, then the whole
             chord: its notes and bass, each extension, the root among the same notes, the bass kind, readings listed
  --answers  the chords an engine read in each piece, as the C# helper in cs/ writes them; the first engine given
             is the baseline
  --engine   rust: mo-harmony's chord reader, run here on the same notes (an engine like the others, in its place)
  --part     the parts the engines heard (as given to the helper; all but drums when left out): mo-harmony hears
             them too, and they give the roots no part plays
  --roles    off: mo-harmony hears every part as one, with no role, as the C# app does (on: each with its role)
  --oracle-cuts  mo-harmony given the labelled changes: each labelled chord read as one chord
  --timing   mo-harmony reads on one core and says how long a read takes: a piece, and 16 bars of it
  --weights  mo-harmony reads with the weights a fit wrote (chordfit --out; a number the file leaves out is the
             hand-set one); the built-in ones when left out
  --folds    only the pieces in these folds (1 to 4), drawn as chordfit draws them: a chart's renders with the chart
  --per-segment  mir_eval's scores per labelled chord too, each counted once
  --by       the chords named whole split by a tag convert/voice.py gives each chord it plays: set, recipe, texture,
             tune, push, chart or quality (and context and role in its sweep); given twice, by both
  --dump     write every piece's labels, chords read and scores to a file, for convert/check_mir_eval.py
  chordfit   fits mo-harmony's chord reader's 39 numbers on the chords labelled in folds 2 to 4 (the pieces whose
             licence lets fitted numbers ship), keeping every rule they keep, and measures the fit on fold 1
  --inputs   the ways a dataset's song is heard: piano, full (with the tune), split (the piano's bass line as a bass
             part), upper (the rest of the piano, no bass); all four when left out. voice.py's pieces as given
  --weigh    how much a source's pieces count, each way they're heard (1); source/input for one way, such as
             pop909cl/upper=0
  --train    the folds learned from (2,3,4); --test the folds measured (1). Fold 0 is kept back
  --lambda   how hard the numbers are pulled towards the ones the fit starts from (1e-3)
  --steps    the most steps the fit takes (40), in each round of --hold
  --from     starts from the weights a fit wrote; the hand-set ones when left out
  --fix      holds these numbers where they start (their codes, such as P1,P2,pair)
  --hand     how much the hand cases' gates count in the aim (0: left out), against the data's loss per bar
  --hold     keeps the hand cases' gates on music it learns from as straight-line rules, refitting at most this many
             more rounds while some break, each rule drawn again where the fit got to
  --out      writes the fitted numbers to a file, for chords --weights and chordfit --from; after a fit, it and the
             report say which hand cases' gates break
  chordcal   fits how sharp the chord reader's chances are (8 numbers, by how much music is heard and its texture)
             and then the thresholds its answers are graded by, on fold 1 of the pieces whose licence lets fitted
             numbers ship (the fold the built-in weights never learned from), read whole and cut into 1 to 16 bars
  --weights  reads with the weights a fit wrote (chordfit --out); the built-in ones when left out
  --folds    the folds fitted on (1)
  --picks    stretches of each length cut from each piece, each way it's heard (2)
  --measure  fits nothing: reads every piece given, of any licence and fold, and says how true the chances are and
             how often each answer is wrong, with the calibration built in (or --calibration's)
  --calibration  a file --out wrote, read in place of the calibration and thresholds built in
  --keep-sharpness  fits only the thresholds, on the sharpness built in (or --calibration's)
  --out      writes the calibration and the thresholds fitted to a file
  candidates how often mo-harmony's chord reader has the labelled chord among the readings it scores (its ceiling),
             with the labelled ends, and moved by up to a unit
  --inputs   the parts heard: piano (all but the tune), full, upper (the accompaniment without its bass line); piano,full
             when left out
  --misses   write each stretch whose chord is missed even with its ends moved to a file (tab-separated)
  tables     counts how often each core is the labelled chord and each tension is named with it (the chord reader's
             template priors), from the labels of folds 1 to 4
  --write    writes them as mo-harmony's counted tables (crates/mo-harmony/src/chordread/tables.rs)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    low_priority_threads();
    let result = match args.first().map(String::as_str) {
        Some("keys") => keys::run(&args[1..]),
        Some("keyfit") => keyfit::run(&args[1..]),
        Some("chords") => chords::run(&args[1..]),
        Some("chordfit") => chordfit::run(&args[1..]),
        Some("chordcal") => chordcal::run(&args[1..]),
        Some("candidates") => candidates::run(&args[1..]),
        Some("packs") => packs::run(&args[1..]),
        Some("keycheck") => keycheck::run(&args[1..]),
        Some("songkeys") => songkeys::run(&args[1..]),
        Some("tables") => tables::run(&args[1..]),
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
    let _ = at_low_priority(rayon::ThreadPoolBuilder::new().num_threads(threads)).build_global();
}

/// A pool whose threads run at low priority (rule 21).
fn at_low_priority(pool: rayon::ThreadPoolBuilder) -> rayon::ThreadPoolBuilder {
    pool.start_handler(|_| {
        let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
    })
}
