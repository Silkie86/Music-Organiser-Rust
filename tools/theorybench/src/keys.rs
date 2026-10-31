//! `theorybench keys`: how often each key finder names the labelled key, on whole songs and on 8-bar clips (about
//! what the MIDI page and a pack's loop give it to go on).

use std::path::PathBuf;
use std::time::Instant;

use mo_harmony::{Grid, Key, KeyRelation, Role, Span};
use rayon::prelude::*;

use crate::item::{self, Item, Note};
use crate::metrics::{self, pct, pct_range};
use crate::{baseline, profiles};

/// What a key finder said: the key, how sure it is (0 to 1), and, for a finder that gives them, the chance of each of
/// the 24 keys (in the order of [`Key::ALL`]).
#[derive(Clone, Copy, Debug)]
pub struct Found {
    pub key: Key,
    pub sure: f64,
    pub chances: Option<[f64; 24]>,
    /// Whether the finder would show this key on its own, for a finder that says so itself (mo-harmony's graded
    /// answer); `None` when its confidence cut decides.
    pub alone: Option<bool>,
}

/// How a key finder is called: a case to its answer, or `None` when it gives none.
pub type Find = Box<dyn Fn(&Case) -> Option<Found> + Sync>;

/// A key finder as the bench runs it.
pub struct Finder {
    pub name: String,
    /// Below this confidence the app shows no key; `None` for a finder that says itself when it shows a key
    /// ([`Found::alone`]).
    pub cut: Option<f64>,
    pub find: Find,
}

/// The new engine's finder, with the weights built into mo-harmony.
pub fn harmony_finder() -> Finder {
    Finder {
        name: "mo-harmony".into(),
        cut: None,
        find: Box::new(|case| {
            let reading = mo_harmony::find_key(&case.harmony_parts(), case.grid, case.span)?;
            let (key, sure) = reading.best();
            let alone = Some(matches!(reading.answer, mo_harmony::KeyAnswer::Key(_)));
            Some(Found { key, sure, chances: Some(reading.chances), alone })
        }),
    }
}

/// The C# app's finder, run as the Songs page ran it.
pub fn cs_finder() -> Finder {
    Finder {
        name: "C# app (Songs page)".into(),
        cut: Some(baseline::SONG_MIN_CONFIDENCE),
        // The C# app counted a bar as four beats, whatever the time signature.
        find: Box::new(|case| {
            baseline::key(&case.notes, 4 * i64::from(case.ppq)).map(|(key, sure)| Found {
                key,
                sure,
                chances: None,
                alone: None,
            })
        }),
    }
}

/// Every finder the bench compares, the C# app's first: the others are measured against it.
pub fn finders() -> Vec<Finder> {
    let mut all = vec![cs_finder()];
    if mo_harmony::keyfind::FITTED {
        all.push(harmony_finder());
    }
    for (which, (name, _, _)) in profiles::PROFILES.iter().enumerate() {
        all.push(Finder {
            name: (*name).into(),
            cut: Some(baseline::SONG_MIN_CONFIDENCE),
            find: Box::new(move |case| {
                profiles::key(which, &case.notes, 4 * i64::from(case.ppq)).map(|(key, sure)| Found {
                    key,
                    sure,
                    chances: None,
                    alone: None,
                })
            }),
        });
    }
    all
}

/// Some notes to find the key of, the key labelled for them, and the song they're from.
pub struct Case {
    pub song: usize,
    pub ppq: u32,
    pub truth: Key,
    /// Every part's notes together, in order of start: what the C# app heard.
    pub notes: Vec<Note>,
    /// The notes part by part, with each part's job, as mo-harmony hears them.
    pub parts: Vec<(Role, Vec<mo_harmony::Note>)>,
    pub grid: Grid,
    /// A whole song, or a stretch cut from one.
    pub span: Span,
}

impl Case {
    /// The parts as mo-harmony takes them.
    pub fn harmony_parts(&self) -> Vec<mo_harmony::Part<'_>> {
        self.parts.iter().map(|(role, notes)| mo_harmony::Part { role: *role, notes }).collect()
    }
}

/// The fewest notes a case needs: the C# bench's rule (its finder gives no key below it either).
pub const MIN_NOTES: usize = 8;

/// How long the clips `keys` scores are, in bars, unless `--bars` says otherwise.
const CLIP_BARS: [i64; 1] = [8];

/// Which parts' notes the finders hear.
pub enum Parts {
    /// Every part but drums.
    Pitched,
    /// Only the parts with this role.
    Role(String),
}

impl Parts {
    fn keeps(&self, role: &str) -> bool {
        match self {
            Parts::Pitched => role != "drums",
            Parts::Role(r) => role == r,
        }
    }
}

/// A part's job as mo-harmony knows it, from the role the dataset gives.
pub fn role(name: &str) -> Role {
    match name {
        "melody" => Role::Melody,
        "bass" => Role::Bass,
        "drums" => Role::Drums,
        "chords" | "accompaniment" => Role::Harmony,
        _ => Role::Unknown,
    }
}

/// A MIDI 1.0 velocity in MIDI 2.0's sixteen bits, by the rule the app's readers use (rule 22).
pub fn velocity16(v: u8) -> u16 {
    mo_formats::velocity::from_7(v)
}

/// The options both `keys` and `keyfit` take, and the files read.
pub struct Setup {
    pub items: Vec<Item>,
    pub parts: Parts,
    pub lengths: Vec<i64>,
    /// Anything else on the command line, for the command to read: its options with their values, and its flags.
    pub rest: Vec<String>,
    pub flags: Vec<String>,
}

/// Reads the files and the options `keys` and `keyfit` share (`--part`, `--aligned`, `--bars`, the clip lengths
/// being `lengths` unless it's given), prints what was read, and hands back the command's own options (`own`, each
/// taking a value) with their values, and its own flags (`flags`, taking none).
pub fn setup(args: &[String], own: &[&str], flags: &[&str], lengths: &[i64]) -> Result<Setup, String> {
    let mut files = Vec::new();
    let mut parts = Parts::Pitched;
    let mut aligned_only = false;
    let mut lengths = lengths.to_vec();
    let mut rest = Vec::new();
    let mut set = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--part" => parts = Parts::Role(args.next().ok_or("--part needs a role, such as melody")?.clone()),
            "--aligned" => aligned_only = true,
            "--bars" => {
                let list = args.next().ok_or("--bars needs clip lengths, such as 1,2,4,8,16")?;
                lengths = list
                    .split(',')
                    .map(|b| b.trim().parse::<i64>().ok().filter(|&b| b > 0))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| format!("--bars {list}: give whole numbers of bars, such as 1,2,4,8,16"))?;
            }
            a if own.contains(&a) => {
                rest.push(a.to_owned());
                rest.push(args.next().ok_or_else(|| format!("{a} needs a value"))?.clone());
            }
            a if flags.contains(&a) => set.push(a.to_owned()),
            a if a.starts_with("--") => return Err(format!("Unknown option {a}.\n\n{}", crate::USAGE)),
            a => files.push(PathBuf::from(a)),
        }
    }
    if files.is_empty() {
        return Err(crate::USAGE.to_owned());
    }
    let mut items = Vec::new();
    for f in &files {
        items.extend(item::read(f)?);
    }
    let read = items.len();
    // The C# bench used only songs with four beats a bar whose labelled bar lines fall on the MIDI's grid.
    items.retain(|i| !aligned_only || (i.aligned && i.beats_per_bar == 4));
    items.retain(|i| !i.keys.is_empty() && !i.parts.is_empty());

    let mut labels: Vec<&str> = items.iter().map(|i| i.key_labels.as_str()).collect();
    labels.sort_unstable();
    labels.dedup();
    println!(
        "{} pieces read, {} with notes and key labels{} (keys labelled by {}).",
        read,
        items.len(),
        if aligned_only { " and bar lines on the MIDI's 4-beat grid" } else { "" },
        if labels.is_empty() { "nobody".to_owned() } else { labels.join(", ") }
    );
    if let Parts::Role(r) = &parts {
        println!("Only the {r} parts are heard.");
    }
    Ok(Setup { items, parts, lengths, rest, flags: set })
}

pub fn run(args: &[String]) -> Result<(), String> {
    let setup = setup(args, &[], &[], &CLIP_BARS)?;
    if !mo_harmony::keyfind::FITTED {
        println!(
            "mo-harmony is left out: its weights aren't fitted yet, so every key scores the same. `theorybench \
             keyfit` measures it."
        );
    } else if setup.items.iter().any(|i| i.source.starts_with("pop909")) {
        println!(
            "mo-harmony's weights were fitted on these songs, so its numbers here flatter it: `theorybench keyfit` \
             gives the fair ones, from songs held out of the fit."
        );
    }
    let finders = finders();
    for (title, truth, cases) in all_cases(&setup.items, &setup.parts, &setup.lengths) {
        let runs: Vec<Run> = finders.iter().map(|f| run_finder(f, &cases)).collect();
        let song: Vec<usize> = cases.iter().map(|c| c.song).collect();
        report(&title, &truth, &song, &runs);
    }
    Ok(())
}

/// Every set of cases the bench scores: whole songs with one key, whole songs that change key, and the clips of each
/// length, each with its title and what it's measured against.
pub fn all_cases(items: &[Item], parts: &Parts, lengths: &[i64]) -> Vec<(String, String, Vec<Case>)> {
    let songs: Vec<SongNotes> = items.par_iter().map(|item| SongNotes::of(item, parts)).collect();
    let (one_key, changing) = whole_songs(items, &songs);
    let mut out = vec![
        ("Whole songs with one key".to_owned(), "the labelled key".to_owned(), one_key),
        ("Whole songs that change key".to_owned(), "the key labelled for the longest time".to_owned(), changing),
    ];
    for &bars in lengths {
        let clips = all_clips(items, &songs, bars);
        out.push((format!("{bars}-bar clips"), "the key labelled at the middle of the clip".to_owned(), clips));
    }
    out
}

/// One song's notes as the finders hear them: all together, and part by part with each part's role, each in order of
/// start. Sorted once a song, then cut for each clip.
struct SongNotes {
    merged: Vec<Note>,
    parts: Vec<(Role, Vec<Note>)>,
}

impl SongNotes {
    fn of(item: &Item, parts: &Parts) -> SongNotes {
        let merged = item.notes_of(|r| parts.keeps(r));
        let parts = item
            .parts
            .iter()
            .filter(|p| parts.keeps(&p.role))
            .map(|p| {
                let mut notes: Vec<Note> = p
                    .notes
                    .iter()
                    .map(|&(start, length, key, velocity)| Note { start, length, key, velocity })
                    .collect();
                notes.sort_by_key(|n| (n.start, n.key));
                (role(&p.role), notes)
            })
            .collect();
        SongNotes { merged, parts }
    }

    /// The notes from `from` for `length` ticks (the whole song when `None`), moved to start at 0 when cut: all
    /// together, and part by part as mo-harmony takes them.
    fn heard(&self, cut: Option<(i64, i64)>) -> (Vec<Note>, Vec<(Role, Vec<mo_harmony::Note>)>) {
        let take = |notes: &[Note]| cut.map_or_else(|| notes.to_vec(), |(from, length)| item::cut(notes, from, length));
        let by_part = self
            .parts
            .iter()
            .map(|(role, notes)| {
                let notes = take(notes)
                    .iter()
                    .map(|n| mo_harmony::Note {
                        start: n.start,
                        length: n.length,
                        key: n.key,
                        velocity: velocity16(n.velocity),
                    })
                    .collect();
                (*role, notes)
            })
            .collect();
        (take(&self.merged), by_part)
    }
}

/// The song's grid: its beat and bar, with a bar line where the labels put one (or at 0 for a clip, which is cut
/// along the bar lines).
pub fn grid_of(item: &Item, clip: bool) -> Grid {
    Grid {
        ticks_per_beat: item.ppq,
        beats_per_bar: item.beats_per_bar,
        bar_line: if clip { 0 } else { item.grid_origin() },
    }
}

/// The whole songs as cases: those with one key, and those that change key.
fn whole_songs(items: &[Item], songs: &[SongNotes]) -> (Vec<Case>, Vec<Case>) {
    let per_song: Vec<Option<Case>> = items
        .par_iter()
        .zip(songs)
        .enumerate()
        .map(|(song, (item, notes))| {
            let (notes, by_part) = notes.heard(None);
            item.main_key().filter(|_| notes.len() >= MIN_NOTES).map(|truth| Case {
                song,
                ppq: item.ppq,
                truth,
                notes,
                parts: by_part,
                grid: grid_of(item, false),
                span: Span::Song,
            })
        })
        .collect();
    let (mut one_key, mut changing) = (Vec::new(), Vec::new());
    for (whole, item) in per_song.into_iter().zip(items) {
        if let Some(w) = whole {
            if item.one_key() { one_key.push(w) } else { changing.push(w) }
        }
    }
    (one_key, changing)
}

/// Every song cut into clips of `bars` bars.
fn all_clips(items: &[Item], songs: &[SongNotes], bars: i64) -> Vec<Case> {
    items
        .par_iter()
        .zip(songs)
        .enumerate()
        .flat_map_iter(|(song, (item, notes))| clips(item, song, notes, bars))
        .collect()
}

/// The song cut into clips of `bars` bars along its bar lines, each with the key labelled at its middle.
fn clips(item: &Item, song: usize, notes: &SongNotes, bars: i64) -> Vec<Case> {
    let length = bars * item.bar();
    let mut out = Vec::new();
    let mut at = item.grid_origin();
    while at + length <= item.length {
        if let Some(truth) = item.key_at(at + length / 2) {
            let (notes, by_part) = notes.heard(Some((at, length)));
            if notes.len() >= MIN_NOTES {
                out.push(Case {
                    song,
                    ppq: item.ppq,
                    truth,
                    notes,
                    parts: by_part,
                    grid: grid_of(item, true),
                    span: Span::Window,
                });
            }
        }
        at += length;
    }
    out
}

/// What one finder said about one case.
#[derive(Clone, Copy)]
pub struct Answer {
    pub truth: Key,
    pub found: Option<Found>,
}

impl Answer {
    pub fn right(&self) -> bool {
        self.found.is_some_and(|f| f.key == self.truth)
    }

    /// The right home note, major or minor either way.
    fn tonic_right(&self) -> bool {
        self.found.is_some_and(|f| f.key.tonic == self.truth.tonic)
    }

    fn relation(&self) -> Option<KeyRelation> {
        self.found.map(|f| f.key.relation_to(self.truth))
    }

    fn sure(&self) -> Option<f64> {
        self.found.map(|f| f.sure)
    }

    /// Whether the finder would show its key on its own: as it says itself, or by its confidence cut.
    fn shown(&self, cut: Option<f64>) -> bool {
        self.found.is_some_and(|f| f.alone.unwrap_or_else(|| cut.is_some_and(|c| f.sure >= c)))
    }

    /// The chance the finder gave the labelled key, when it gives chances.
    fn chance_of_truth(&self) -> Option<f64> {
        self.found.and_then(|f| f.chances).and_then(|c| c.get(self.truth.index()).copied())
    }
}

/// One finder's answers to a set of cases.
pub struct Run {
    pub name: String,
    /// The finder's confidence cut ([`Finder::cut`]).
    pub cut: Option<f64>,
    pub answers: Vec<Answer>,
    /// Microseconds a case, or `None` when the answers were worked out elsewhere.
    pub micros: Option<f64>,
}

/// Runs a finder over the cases, timing it.
pub fn run_finder(f: &Finder, cases: &[Case]) -> Run {
    let started = Instant::now();
    let answers: Vec<Answer> = cases.par_iter().map(|c| Answer { truth: c.truth, found: (f.find)(c) }).collect();
    let micros = started.elapsed().as_secs_f64() * 1e6 / cases.len().max(1) as f64;
    Run { name: f.name.clone(), cut: f.cut, answers, micros: Some(micros) }
}

/// The negative log-likelihood (natural log) of the labelled keys and the Brier score, for a finder that gives
/// chances: how much its chances say about the truth, beyond whether its first guess was right.
fn likelihoods(answers: &[Answer]) -> Option<(f64, f64)> {
    let mut nll = 0.0;
    let mut brier = 0.0;
    for a in answers {
        let chances = a.found.and_then(|f| f.chances)?;
        nll -= a.chance_of_truth()?.max(1e-12).ln();
        brier += crate::fit::brier(&chances, a.truth.index());
    }
    let n = answers.len().max(1) as f64;
    Some((nll / n, brier / n))
}

/// Prints how each finder did on one set of cases (`song` is each case's song, for the ranges); the first finder is
/// the one the others are measured against.
pub fn report(title: &str, truth: &str, song: &[usize], runs: &[Run]) {
    let mut songs: Vec<usize> = song.to_vec();
    songs.sort_unstable();
    songs.dedup();
    println!();
    println!("=== {title}: {} cases from {} songs, against {truth} ===", song.len(), songs.len());
    let Some(first) = runs.first().filter(|_| !song.is_empty()) else { return };
    let minor = first.answers.iter().filter(|a| a.truth.is_minor()).count();
    println!("{} of the labelled keys are minor.", pct(minor as f64 / song.len() as f64));

    println!();
    println!(
        "{:<28} {:<24} {:>6} {:>7} {:>7} {:>8} {:>10} {:>8} {:>8} {:>6} {:>6} {:>6} {:>6} {:>6} {:>9}",
        "",
        "right (likely range)",
        "home",
        "MIREX",
        "MIREX",
        "shown",
        "right when",
        "major",
        "minor",
        "ECE",
        "ECE",
        "AURC",
        "NLL",
        "Brier",
        "µs a case"
    );
    // mir_eval's release gives half a point only for a fifth up; MIREX has counted either fifth since 2017. The
    // calibration error is over ten bins of equal width, then fifteen of equal size.
    println!(
        "{:<28} {:<24} {:>6} {:>7} {:>7} {:>8} {:>10} {:>8} {:>8} {:>6} {:>6}",
        "", "", "note", "5th up", "any 5th", "", "shown", "", "", "width", "size"
    );
    for r in runs {
        let answers = &r.answers;
        let right: Vec<bool> = answers.iter().map(Answer::right).collect();
        let mirex = |both_fifths: bool| {
            let total: f64 =
                answers.iter().map(|a| a.found.map_or(0.0, |f| f.key.mirex_score(a.truth, both_fifths))).sum();
            100.0 * total / answers.len().max(1) as f64
        };
        let shown: Vec<&Answer> = answers.iter().filter(|a| a.shown(r.cut)).collect();
        let right_shown = shown.iter().filter(|a| a.right()).count();
        let of_mode = |minor: bool| {
            let these: Vec<&Answer> = answers.iter().filter(|a| a.truth.is_minor() == minor).collect();
            if these.is_empty() {
                "-".to_owned()
            } else {
                pct(these.iter().filter(|a| a.right()).count() as f64 / these.len() as f64)
            }
        };
        let sure: Vec<(Option<f64>, bool)> = answers.iter().map(|a| (a.sure(), a.right())).collect();
        let (nll, brier) = likelihoods(answers)
            .map_or(("-".to_owned(), "-".to_owned()), |(n, b)| (format!("{n:.3}"), format!("{b:.3}")));
        let three = |x: Option<f64>| x.map_or("-".to_owned(), |e| format!("{e:.3}"));
        println!(
            "{:<28} {:<24} {:>6} {:>7.1} {:>7.1} {:>8} {:>10} {:>8} {:>8} {:>6} {:>6} {:>6} {:>6} {:>6} {:>9}",
            r.name,
            pct_range(metrics::share(&right, song)),
            pct(answers.iter().filter(|a| a.tonic_right()).count() as f64 / answers.len() as f64),
            mirex(false),
            mirex(true),
            pct(shown.len() as f64 / answers.len() as f64),
            if shown.is_empty() { "-".to_owned() } else { pct(right_shown as f64 / shown.len() as f64) },
            of_mode(false),
            of_mode(true),
            three(metrics::calibration_error(&sure)),
            three(metrics::calibration_error_equal_mass(&sure, 15)),
            three(metrics::risk_coverage_area(&sure)),
            nll,
            brier,
            r.micros.map_or("-".to_owned(), |m| format!("{m:.1}"))
        );
    }

    println!();
    println!("What the wrong answers were, as a share of all cases:");
    println!(
        "{:<28} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "", "5th up", "5th down", "relative", "parallel", "other", "no key"
    );
    for r in runs {
        let share = |want: Option<KeyRelation>| {
            pct(r.answers.iter().filter(|a| a.relation() == want).count() as f64 / r.answers.len() as f64)
        };
        println!(
            "{:<28} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
            r.name,
            share(Some(KeyRelation::FifthAbove)),
            share(Some(KeyRelation::FifthBelow)),
            share(Some(KeyRelation::Relative)),
            share(Some(KeyRelation::Parallel)),
            share(Some(KeyRelation::Other)),
            share(None)
        );
    }

    println!();
    println!("Right when only the surest answers are given (the rest left blank), by share of cases answered:");
    let coverages = [1.0, 0.9, 0.75, 0.5, 0.25];
    println!("{:<28} {}", "", coverages.iter().map(|c| format!("{:>8}", pct(*c))).collect::<Vec<_>>().join(" "));
    for r in runs {
        let sure: Vec<(Option<f64>, bool)> = r.answers.iter().map(|a| (a.sure(), a.right())).collect();
        let cells: Vec<String> = coverages
            .iter()
            .map(|&c| format!("{:>8}", metrics::right_when_most_sure(&sure, c).map_or("-".to_owned(), pct)))
            .collect();
        println!("{:<28} {}", r.name, cells.join(" "));
    }

    if let Some((first, rest)) = runs.split_first()
        && !rest.is_empty()
    {
        println!();
        println!("Against the {} (a difference whose range doesn't cross 0 is real):", first.name);
        let base_right: Vec<bool> = first.answers.iter().map(Answer::right).collect();
        for r in rest {
            let right: Vec<bool> = r.answers.iter().map(Answer::right).collect();
            let p = metrics::paired(&base_right, &right, song);
            println!(
                "{:<28} {:>+6.1} points ({:+.1} to {:+.1}); right only here {}, only there {}; McNemar p {:.2e}",
                r.name,
                100.0 * p.diff,
                100.0 * p.low,
                100.0 * p.high,
                p.only_b,
                p.only_a,
                p.p
            );
        }
    }
}
