//! `theorybench keys`: how often each key finder names the labelled key, on whole songs and on 8-bar clips (about
//! what the MIDI page and a pack's loop give it to go on).

use std::path::PathBuf;
use std::time::Instant;

use mo_harmony::{Key, KeyRelation};
use rayon::prelude::*;

use crate::item::{self, Item, Note};
use crate::metrics::{self, pct, pct_range};
use crate::{baseline, profiles};

/// How a key finder is called: the notes (in order of start) and the ticks in a beat, to the key and how sure it is
/// (0 to 1), or `None` when it gives no answer.
pub type Find = Box<dyn Fn(&[Note], u32) -> Option<(Key, f64)> + Sync>;

/// A key finder as the bench runs it.
pub struct Finder {
    pub name: String,
    /// Below this confidence the app shows no key.
    pub cut: f64,
    pub find: Find,
}

/// Every finder the bench compares, the C# app's first: the others are measured against it.
pub fn finders() -> Vec<Finder> {
    let mut all = vec![Finder {
        name: "C# app (Songs page)".into(),
        cut: baseline::SONG_MIN_CONFIDENCE,
        // The C# app counted a bar as four beats, whatever the time signature.
        find: Box::new(|notes, ppq| baseline::key(notes, 4 * i64::from(ppq))),
    }];
    for (which, (name, _, _)) in profiles::PROFILES.iter().enumerate() {
        all.push(Finder {
            name: (*name).into(),
            cut: baseline::SONG_MIN_CONFIDENCE,
            find: Box::new(move |notes, ppq| profiles::key(which, notes, 4 * i64::from(ppq))),
        });
    }
    all
}

/// Some notes to find the key of, the key labelled for them, and the song they're from.
struct Case {
    song: usize,
    ppq: u32,
    truth: Key,
    notes: Vec<Note>,
}

/// The fewest notes a case needs: the C# bench's rule (its finder gives no key below it either).
const MIN_NOTES: usize = 8;

/// How long a clip is, in bars.
const CLIP_BARS: i64 = 8;

/// Which parts' notes the finders hear.
enum Parts {
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

pub fn run(args: &[String]) -> Result<(), String> {
    let mut files = Vec::new();
    let mut parts = Parts::Pitched;
    let mut aligned_only = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--part" => parts = Parts::Role(args.next().ok_or("--part needs a role, such as melody")?.clone()),
            "--aligned" => aligned_only = true,
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

    let (one_key, changing, clips) = cases(&items, &parts);
    let finders = finders();
    report("Whole songs with one key", "the labelled key", &one_key, &finders);
    report("Whole songs that change key", "the key labelled for the longest time", &changing, &finders);
    report(&format!("{CLIP_BARS}-bar clips"), "the key labelled at the middle of the clip", &clips, &finders);
    Ok(())
}

/// The cases: whole songs with one key, whole songs that change key, and clips.
fn cases(items: &[Item], parts: &Parts) -> (Vec<Case>, Vec<Case>, Vec<Case>) {
    let per_song: Vec<(Option<Case>, Vec<Case>)> = items
        .par_iter()
        .enumerate()
        .map(|(song, item)| {
            let notes = item.notes_of(|role| parts.keeps(role));
            let whole = item.main_key().filter(|_| notes.len() >= MIN_NOTES).map(|truth| Case {
                song,
                ppq: item.ppq,
                truth,
                notes: notes.clone(),
            });
            (whole, clips(item, song, &notes))
        })
        .collect();
    let (mut one_key, mut changing, mut clips) = (Vec::new(), Vec::new(), Vec::new());
    for ((whole, c), item) in per_song.into_iter().zip(items) {
        if let Some(w) = whole {
            if item.one_key() { one_key.push(w) } else { changing.push(w) }
        }
        clips.extend(c);
    }
    (one_key, changing, clips)
}

/// The song cut into clips along its bar lines, each with the key labelled at its middle.
fn clips(item: &Item, song: usize, notes: &[Note]) -> Vec<Case> {
    let length = CLIP_BARS * item.bar();
    let mut out = Vec::new();
    let mut at = item.grid_origin();
    while at + length <= item.length {
        if let Some(truth) = item.key_at(at + length / 2) {
            let clip = item::cut(notes, at, length);
            if clip.len() >= MIN_NOTES {
                out.push(Case { song, ppq: item.ppq, truth, notes: clip });
            }
        }
        at += length;
    }
    out
}

/// What one finder said about one case.
#[derive(Clone, Copy)]
struct Answer {
    truth: Key,
    found: Option<(Key, f64)>,
}

impl Answer {
    fn right(&self) -> bool {
        self.found.is_some_and(|(k, _)| k == self.truth)
    }

    fn relation(&self) -> Option<KeyRelation> {
        self.found.map(|(k, _)| k.relation_to(self.truth))
    }
}

fn report(title: &str, truth: &str, cases: &[Case], finders: &[Finder]) {
    let mut songs: Vec<usize> = cases.iter().map(|c| c.song).collect();
    let song: Vec<usize> = songs.clone();
    songs.dedup();
    println!();
    println!("=== {title}: {} cases from {} songs, against {truth} ===", cases.len(), songs.len());
    if cases.is_empty() {
        return;
    }
    let minor = cases.iter().filter(|c| c.truth.is_minor()).count();
    println!("{} of the labelled keys are minor.", pct(minor as f64 / cases.len() as f64));

    let mut runs: Vec<(&Finder, Vec<Answer>, f64)> = Vec::new();
    for f in finders {
        let started = Instant::now();
        let answers: Vec<Answer> =
            cases.par_iter().map(|c| Answer { truth: c.truth, found: (f.find)(&c.notes, c.ppq) }).collect();
        let micros = started.elapsed().as_secs_f64() * 1e6 / cases.len() as f64;
        runs.push((f, answers, micros));
    }

    println!();
    println!(
        "{:<28} {:<24} {:>6} {:>8} {:>10} {:>8} {:>8} {:>9}",
        "", "right (likely range)", "MIREX", "shown", "right when", "major", "minor", "µs a case"
    );
    println!("{:<28} {:<24} {:>6} {:>8} {:>10}", "", "", "", "", "shown");
    for (f, answers, micros) in &runs {
        let right: Vec<bool> = answers.iter().map(Answer::right).collect();
        let mirex: Vec<f64> =
            answers.iter().map(|a| a.found.map_or(0.0, |(k, _)| k.mirex_score(a.truth, false))).collect();
        let shown: Vec<&Answer> = answers.iter().filter(|a| a.found.is_some_and(|(_, c)| c >= f.cut)).collect();
        let right_shown = shown.iter().filter(|a| a.right()).count();
        let of_mode = |minor: bool| {
            let these: Vec<&Answer> = answers.iter().filter(|a| a.truth.is_minor() == minor).collect();
            if these.is_empty() {
                "-".to_owned()
            } else {
                pct(these.iter().filter(|a| a.right()).count() as f64 / these.len() as f64)
            }
        };
        println!(
            "{:<28} {:<24} {:>6.1} {:>8} {:>10} {:>8} {:>8} {:>9.1}",
            f.name,
            pct_range(metrics::share(&right, &song)),
            100.0 * mirex.iter().sum::<f64>() / mirex.len() as f64,
            pct(shown.len() as f64 / answers.len() as f64),
            if shown.is_empty() { "-".to_owned() } else { pct(right_shown as f64 / shown.len() as f64) },
            of_mode(false),
            of_mode(true),
            micros
        );
    }

    println!();
    println!("What the wrong answers were, as a share of all cases:");
    println!(
        "{:<28} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "", "5th up", "5th down", "relative", "parallel", "other", "no key"
    );
    for (f, answers, _) in &runs {
        let share = |want: Option<KeyRelation>| {
            pct(answers.iter().filter(|a| a.relation() == want).count() as f64 / answers.len() as f64)
        };
        println!(
            "{:<28} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
            f.name,
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
    for (f, answers, _) in &runs {
        let sure: Vec<(Option<f64>, bool)> = answers.iter().map(|a| (a.found.map(|(_, c)| c), a.right())).collect();
        let cells: Vec<String> = coverages
            .iter()
            .map(|&c| format!("{:>8}", metrics::right_when_most_sure(&sure, c).map_or("-".to_owned(), pct)))
            .collect();
        println!("{:<28} {}", f.name, cells.join(" "));
    }

    if let Some(((first, base, _), rest)) = runs.split_first()
        && !rest.is_empty()
    {
        println!();
        println!("Against the {} (a difference whose range doesn't cross 0 is real):", first.name);
        let base_right: Vec<bool> = base.iter().map(Answer::right).collect();
        for (f, answers, _) in rest {
            let right: Vec<bool> = answers.iter().map(Answer::right).collect();
            let p = metrics::paired(&base_right, &right, &song);
            println!(
                "{:<28} {:>+6.1} points ({:+.1} to {:+.1}); right only here {}, only there {}; McNemar p {:.2e}",
                f.name,
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
