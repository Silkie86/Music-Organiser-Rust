//! `theorybench pairs`: how well the app tells which chords go with which tune (`mo_harmony::together`), beside the
//! C# app's way of doing it (`HarmonyMatcher`, ported here as it was).
//!
//! Every song with a tune and an accompaniment is cut into stretches of a few bars. A stretch's own tune and
//! accompaniment were written for each other, so a good matcher puts them together: given the tune, it should rank
//! its own accompaniment above another song's (tried in all 12 keys, as the search tries every part), and above
//! another stretch of the same song (same key and style, other chords: the test of fitting bar by bar). The same the
//! other way round, from the accompaniment to the tune.
//!
//! `--fit` fits the numbers (`Sitting`'s worths) instead of measuring: each stretch asks two questions, which of its
//! own accompaniment and 20 other songs' its tune goes with, and which tune goes with its accompaniment, each pairing
//! tried at all 12 moves; the right answer is its own pair, unmoved. The worths are the ones that give the right
//! answers the best chances (a softmax over the answers, the scores being the shares times the worths, so there's one
//! best set of worths whatever the fit starts from). Only pieces whose licence lets fitted numbers ship are learned
//! from, and never Solomon's own (rule 5). Fit on folds 2 to 4, report on fold 1; fold 0 is kept back for a last look.
//!
//! `--quick N` measures the first look instead (`mo_harmony::Outline`): a library can't take the close look at every
//! part, since that needs each part's notes from disk, so a first look sorts every part by a little kept in memory
//! and only the best share get the close look. For N stretches, every stretch of another song is tried both ways
//! round, and it prints how often the close look's best, and how many of its best ten, are in the share kept.
//!
//! The songs are pop, and say nothing of jazz, gospel or blues. So the fit is held to two rules they can't teach
//! (`--tie colour=fifth --without above3,above4,above7,above8,above9`, see `Sitting`), and the numbers it gives must
//! pass the hand cases in `mo_harmony::together`'s tests before they ship.

use std::path::PathBuf;

use mo_harmony::{
    Backing, ChordContext, ChordReader, Grid, KINDS, LineUp, MOST_SLOTS, Note, Outline, Part, Role, Shares, Sitting,
    Slot, Span, WORTH_NAMES, WORTHS, shares, slots_over,
};
use rayon::prelude::*;

use crate::item::{self, Item, LEAST_BACKING, LEAST_TUNE};
use crate::keyfit;
use crate::keys;
use crate::metrics;

/// Every piece is brought to this many ticks a beat, so two songs' stretches can be played together.
const PPQ: i64 = 480;

/// One stretch of a song: its tune and its accompaniment, from tick 0.
struct Stretch {
    song: usize,
    tune: Vec<Note>,
    /// Its accompaniment's chords, as the app reads them; none when the reader hears nothing in it.
    read: Option<Backing>,
    /// The same chords over all of the stretch's bars from its first bar line, as bars picked from a song are
    /// fitted to ([`Backing::over`]): what the first look is given.
    whole: Option<Backing>,
    old: Old,
    /// How long each plays each pitch class over all of it, with no timing: what the library has kept of every part
    /// so far ([`kept_chroma`]).
    tune_chroma: [f64; 12],
    backing_chroma: [f64; 12],
    /// The same two beats at a time: what the first look goes by.
    tune_slots: Vec<Slot>,
    backing_slots: Vec<Slot>,
}

/// What the C# app kept of a part to match it: how long each pitch class sounds, and its notes half a bar at a time.
struct Old {
    /// Shares adding up to 1, of the tune and of the accompaniment.
    tune_chroma: [f32; 12],
    backing_chroma: [f32; 12],
    /// 12 a half bar: how much of it each pitch class sounds (the tune), and how each sits (the accompaniment).
    tune_weights: Vec<f32>,
    backing_sits: Vec<f32>,
    count: usize,
}

/// Which way of matching is scored.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Way {
    /// `mo_harmony::together`.
    New,
    /// The C# app's: its first look picks 3 of the 12 moves by the notes alone, its closer look scores those.
    Old,
    /// The C# app's closer look on all 12 moves.
    OldAll,
}

impl Way {
    const ALL: [Way; 3] = [Way::New, Way::Old, Way::OldAll];

    fn name(self) -> &'static str {
        match self {
            Way::New => "new (chords read, notes judged over them)",
            Way::Old => "old app (3 moves picked by notes alone)",
            Way::OldAll => "old app's closer look, all 12 moves",
        }
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut paths = Vec::new();
    let (mut bars, mut others, mut folds, mut how) = (8i64, 50usize, vec![1usize], Sitting::SHIPPED);
    let (mut fit_steps, mut pull) = (0usize, 1e-4f64);
    let mut quick_asked: Option<usize> = None;
    let mut without: Vec<usize> = Vec::new();
    let mut ties: Vec<(usize, usize)> = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--bars" => bars = value("--bars")?.parse().map_err(|_| "--bars takes a number".to_owned())?,
            "--others" => others = value("--others")?.parse().map_err(|_| "--others takes a number".to_owned())?,
            "--folds" => {
                folds = value("--folds")?
                    .split(',')
                    .map(|f| f.trim().parse::<usize>().map_err(|_| format!("{f} is no fold")))
                    .collect::<Result<_, _>>()?;
            }
            "--set" => set(&mut how, value("--set")?)?,
            "--fit" => fit_steps = value("--fit")?.parse().map_err(|_| "--fit takes how many steps".to_owned())?,
            "--without" => {
                for name in value("--without")?.split(',') {
                    let name = name.trim();
                    without.push(
                        WORTH_NAMES.iter().position(|n| *n == name).ok_or_else(|| format!("{name} is no worth"))?,
                    );
                }
            }
            "--tie" => {
                for pair in value("--tie")?.split(',') {
                    let place = |name: &str| {
                        WORTH_NAMES.iter().position(|n| *n == name.trim()).ok_or_else(|| format!("{name} is no worth"))
                    };
                    let (tied, to) =
                        pair.split_once('=').ok_or_else(|| format!("--tie takes worth=worth, not {pair}"))?;
                    ties.push((place(tied)?, place(to)?));
                }
            }
            "--pull" => pull = value("--pull")?.parse().map_err(|_| "--pull takes a number".to_owned())?,
            "--quick" => {
                let asked = value("--quick")?.parse::<usize>().ok().filter(|n| *n > 0);
                quick_asked = Some(asked.ok_or_else(|| "--quick takes how many stretches, 1 or more".to_owned())?);
            }
            _ if arg.starts_with("--") => return Err(format!("pairs doesn't know {arg}")),
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    if paths.is_empty() || !(1..=64).contains(&bars) || others == 0 {
        return Err(crate::USAGE.to_owned());
    }
    if quick_asked.is_some() && fit_steps > 0 {
        return Err("--quick measures the first look and --fit fits the worths: give one of them".to_owned());
    }
    // A first look keeps no more of a part than this: longer stretches would measure the cut, not the look.
    let most_bars = MOST_SLOTS / 2;
    if quick_asked.is_some() && bars as usize > most_bars {
        return Err(format!(
            "--quick looks at {most_bars} bars of a part at the most: give --bars {most_bars} or fewer"
        ));
    }
    let mut items = Vec::new();
    for path in &paths {
        items.extend(item::read(path)?);
    }
    let fitting = fit_steps > 0;
    let kept: Vec<&Item> = items
        .iter()
        .filter(|i| i.aligned && i.beats_per_bar == 4 && folds.contains(&keyfit::own_fold(i)))
        // A fit learns only from music whose licence lets fitted numbers ship, and never from Solomon's own (rule 5).
        .filter(|i| !fitting || (i.may_learn() && !i.is_own()))
        .collect();
    let stretches: Vec<Stretch> =
        kept.par_iter().enumerate().flat_map_iter(|(song, item)| stretches_of(song, item, bars)).collect();
    println!(
        "{} songs of {} (4/4, bar lines on the grid, folds {folds:?}{}), {} stretches of {bars} bars; each against \
         {} from other songs",
        kept.len(),
        items.len(),
        if fitting { ", a licence to learn from" } else { "" },
        stretches.len(),
        if fitting { others.min(FIT_OTHERS) } else { others }
    );
    let unread = stretches.iter().filter(|s| s.read.is_none()).count();
    if unread > 0 {
        println!("{unread} stretches whose chords the reader hears nothing in count as lost for the new way");
    }
    if stretches.len() < 2 {
        return Err("too few stretches to measure".to_owned());
    }
    if fitting {
        fit(&stretches, others.min(FIT_OTHERS), &how, fit_steps, pull, &without, &ties);
        return Ok(());
    }
    if let Some(asked) = quick_asked {
        quick(&stretches, asked, &how);
        return Ok(());
    }
    let mut by_way = Vec::new();
    for way in Way::ALL {
        let tallies: Vec<Tally> =
            (0..stretches.len()).into_par_iter().map(|q| tally(&stretches, q, others, way, &how)).collect();
        let sum = tallies.iter().fold(Tally::default(), |a, b| a.plus(b));
        println!("\n{}", way.name());
        sum.print();
        by_way.push(tallies);
    }
    if let [new, old, ..] = by_way.as_slice() {
        let song: Vec<usize> = stretches.iter().map(|s| s.song).collect();
        beside(new, old, &song);
    }
    Ok(())
}

/// Prints how much more often the new way is right than the old app's, in points, each with the range it most likely
/// lies in when the songs are drawn again (a song's stretches go wrong together, so whole songs are drawn).
fn beside(new: &[Tally], old: &[Tally], song: &[usize]) {
    let gain = |right: fn(&Tally) -> f64, of: fn(&Tally) -> f64| -> String {
        let column = |tallies: &[Tally], f: fn(&Tally) -> f64| -> Vec<f64> { tallies.iter().map(f).collect() };
        let (diff, low, high) = metrics::paired_ratio(&column(old, right), &column(new, right), &column(new, of), song);
        format!("{:+5.1} ({:+.1} to {:+.1})", 100.0 * diff, 100.0 * low, 100.0 * high)
    };
    println!("\nnew over the old app, in points (the range it most likely lies in, the songs drawn again)");
    println!(
        "  chords for a tune:  its own over another song's {}   first of them all {}   over another stretch of its \
         song {}",
        gain(|t| t.chords_wins, |t| t.chords_pairs),
        gain(|t| t.chords_first, |t| t.asked),
        gain(|t| t.chords_near_wins, |t| t.chords_near_pairs)
    );
    println!(
        "  a tune for chords:  its own over another song's {}   first of them all {}   over another stretch of its \
         song {}",
        gain(|t| t.tune_wins, |t| t.tune_pairs),
        gain(|t| t.tune_first, |t| t.asked),
        gain(|t| t.tune_near_wins, |t| t.tune_near_pairs)
    );
    println!("  its own pair sits best unmoved {}", gain(|t| t.unmoved, |t| t.asked));
}

/// Sets some of the numbers by name: `outside=-0.4,longest=2`.
fn set(how: &mut Sitting, text: &str) -> Result<(), String> {
    for pair in text.split(',') {
        let (name, value) = pair.split_once('=').ok_or_else(|| format!("--set takes name=number, not {pair}"))?;
        let value: f64 = value.trim().parse().map_err(|_| format!("{value} is no number"))?;
        if !how.set(name.trim(), value) {
            return Err(format!("{} is none of the numbers", name.trim()));
        }
    }
    Ok(())
}

/// The shares of stretch `t`'s tune over stretch `b`'s accompaniment.
fn shares_of(stretches: &[Stretch], t: usize, b: usize, how: &Sitting) -> Option<Shares> {
    let (tune, back) = (stretches.get(t)?, stretches.get(b)?);
    shares(&tune.tune, back.read.as_ref()?, LineUp::AsWritten, how)
}

/// The other songs' stretches stretch `q` is asked against: the same ones every run, none of them twice.
fn others_of(stretches: &[Stretch], q: usize, others: usize) -> Vec<usize> {
    let Some(own) = stretches.get(q) else { return Vec::new() };
    let mut seed = (q as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x1234_5678_9abc_def0;
    let mut out = Vec::with_capacity(others);
    let mut tries = 0;
    while out.len() < others && tries < others * 20 {
        tries += 1;
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let o = (seed % stretches.len() as u64) as usize;
        if stretches.get(o).is_some_and(|s| s.song != own.song) && !out.contains(&o) {
            out.push(o);
        }
    }
    out
}

/// The most other songs' stretches a fit's question is asked against.
const FIT_OTHERS: usize = 20;

/// Where the fit puts the best tenth of own pairs and the middle of another song's parts ([`fit`]).
const TOP: f64 = 0.95;
const MIDDLE: f64 = 0.5;

/// One question the fit learns from: the ways to answer it (each a pairing and a move, as its shares), the first
/// being the right one (the stretch's own pair, unmoved).
struct Question {
    options: Vec<[f32; WORTHS]>,
}

/// Fits the worths: for each stretch, its own tune and accompaniment unmoved should sit better than its tune over
/// another song's accompaniment, another song's tune over its accompaniment, and its own pair moved, at any move.
/// `without` holds worths at 0; a worth in `ties` is kept the same as another. Of `how`, only how much a note counts
/// by its length is used. Prints the fitted numbers as a `--set` line.
fn fit(
    stretches: &[Stretch],
    others: usize,
    how: &Sitting,
    steps: usize,
    pull: f64,
    without: &[usize],
    ties: &[(usize, usize)],
) {
    let questions: Vec<Question> = (0..stretches.len())
        .into_par_iter()
        .flat_map_iter(|q| {
            let Some(own) = shares_of(stretches, q, q, how) else { return Vec::new() };
            let drawn = others_of(stretches, q, others);
            let narrow = |x: &[f64; WORTHS]| x.map(|v| v as f32);
            let ask = |pairs: Vec<Option<Shares>>| {
                let mut options: Vec<[f32; WORTHS]> = own.by_move.iter().map(narrow).collect();
                options.extend(pairs.iter().flatten().flat_map(|s| s.by_move.iter().map(narrow)));
                Question { options }
            };
            vec![
                ask(drawn.iter().map(|&o| shares_of(stretches, q, o, how)).collect()),
                ask(drawn.iter().map(|&o| shares_of(stretches, o, q, how)).collect()),
            ]
        })
        .collect();
    // From nothing: the loss has one lowest point, so where the fit starts makes no difference to where it ends.
    let mut w = vec![0.0f64; WORTHS];
    let (mut m, mut v) = (vec![0.0; WORTHS], vec![0.0; WORTHS]);
    for step in 1..=steps {
        let (loss, grad) = questions
            .par_iter()
            .map(|q| {
                let scores: Vec<f64> =
                    q.options.iter().map(|x| x.iter().zip(&w).map(|(x, w)| f64::from(*x) * w).sum()).collect();
                let chances = crate::fit::softmax(&scores);
                let mut grad = [0.0; WORTHS];
                for (k, (x, p)) in q.options.iter().zip(&chances).enumerate() {
                    let d = p - f64::from(u8::from(k == 0));
                    for (g, x) in grad.iter_mut().zip(x) {
                        *g += d * f64::from(*x);
                    }
                }
                (crate::fit::surprise(&scores, 0), grad)
            })
            .reduce(
                || (0.0, [0.0; WORTHS]),
                |(l0, g0), (l1, g1)| {
                    let mut g = g0;
                    g.iter_mut().zip(g1).for_each(|(a, b)| *a += b);
                    (l0 + l1, g)
                },
            );
        let n = questions.len().max(1) as f64;
        // A tied worth moves with the one it's tied to: its pull on the loss is that one's too.
        let mut grad = grad;
        for &(tied, to) in ties {
            let g = grad.get(tied).copied().unwrap_or(0.0);
            if let Some(cell) = grad.get_mut(to) {
                *cell += g;
            }
        }
        for k in 0..WORTHS {
            let (Some(wk), Some(mk), Some(vk), Some(gk)) = (w.get_mut(k), m.get_mut(k), v.get_mut(k), grad.get(k))
            else {
                continue;
            };
            if without.contains(&k) || ties.iter().any(|t| t.0 == k) {
                continue;
            }
            let g = gk / n + pull * *wk;
            *mk = 0.9 * *mk + 0.1 * g;
            *vk = 0.999 * *vk + 0.001 * g * g;
            let (mh, vh) = (*mk / (1.0 - 0.9f64.powi(step as i32)), *vk / (1.0 - 0.999f64.powi(step as i32)));
            *wk -= 0.05 * mh / (vh.sqrt() + 1e-8);
        }
        for &(tied, to) in ties {
            let value = w.get(to).copied().unwrap_or(0.0);
            if let Some(cell) = w.get_mut(tied) {
                *cell = value;
            }
        }
        if step % 50 == 0 || step == steps {
            println!("step {step}: loss {:.4} a question ({} questions)", loss / n, questions.len());
        }
    }
    // Brought to the app's scale. The kinds' worths can all move by any one amount (every heard note has one kind, so
    // it moves every answer alike), and the softmax keeps no scale; so the scale and that amount are set to make the best tenth of the
    // stretches' own pairs sit at TOP or more and the middle one of another song's parts, at its best move, at MIDDLE.
    let raw = |x: &[f32; WORTHS]| -> f64 { x.iter().zip(&w).map(|(x, w)| f64::from(*x) * w).sum() };
    let mut own: Vec<f64> = questions.iter().filter_map(|q| q.options.first().map(raw)).collect();
    let mut other: Vec<f64> = questions
        .iter()
        .flat_map(|q| q.options.chunks(12).skip(1).map(|moves| moves.iter().map(raw).fold(f64::NEG_INFINITY, f64::max)))
        .collect();
    own.sort_by(f64::total_cmp);
    other.sort_by(f64::total_cmp);
    let at = |v: &[f64], share: f64| v.get(((v.len() as f64 - 1.0) * share) as usize).copied().unwrap_or(0.0);
    let (top, middle) = (at(&own, 0.9), at(&other, 0.5));
    let scale = if top > middle { (TOP - MIDDLE) / (top - middle) } else { 1.0 };
    let kinds_move = TOP - top * scale;
    let shown = |v: &[f64]| -> String {
        [0.05, 0.25, 0.5, 0.75, 0.9, 0.95].map(|s| format!("{:.2}", at(v, s) * scale + kinds_move)).join(" ")
    };
    println!("own pairs sit at (5%, 25%, half, 75%, 90%, 95% of them below): {}", shown(&own));
    println!("another song's part at its best move:                         {}", shown(&other));
    let line: Vec<String> = WORTH_NAMES
        .iter()
        .zip(&w)
        .enumerate()
        .map(|(k, (name, w))| format!("{name}={:.3}", w * scale + if k < KINDS { kinds_move } else { 0.0 }))
        .collect();
    println!("--set {}", line.join(","));
}

/// A song's stretches of `bars` bars, one after another from its first bar line, each with enough notes in both.
fn stretches_of(song: usize, item: &Item, bars: i64) -> Vec<Stretch> {
    // The tune is the first melody part alone (POP909's second, BRIDGE, is a counter-line that comes and goes).
    let lead = item.parts.iter().find(|p| p.role == "melody");
    let mut tune: Vec<item::Note> = lead
        .map(|p| p.notes.iter().map(|&(start, length, key, velocity)| item::Note { start, length, key, velocity }))
        .into_iter()
        .flatten()
        .collect();
    tune.sort_by_key(|n| (n.start, n.key));
    let backing = item.notes_of(|role| role != "melody" && role != "drums");
    // A stretch's notes as mo-harmony takes them, from tick 0 and at PPQ ticks a beat.
    let scale = |t: i64| (i128::from(t) * i128::from(PPQ) / i128::from(item.ppq.max(1))) as i64;
    let cut = |notes: &[item::Note], from: i64, length: i64| -> Vec<Note> {
        keys::harmony_notes(&item::cut(notes, from, length))
            .into_iter()
            .map(|n| Note { start: scale(n.start), length: scale(n.length).max(1), ..n })
            .collect()
    };
    let length = bars * item.bar();
    let mut reader = ChordReader::new();
    let mut out = Vec::new();
    for from in item.stretch_starts(bars) {
        let (t, b) = (cut(&tune, from, length), cut(&backing, from, length));
        if t.len() < LEAST_TUNE || b.len() < LEAST_BACKING {
            continue;
        }
        out.push(Stretch::of(song, &mut reader, t, &b, bars));
    }
    out
}

impl Stretch {
    /// A stretch of `bars` bars from tick 0: its accompaniment read as the app reads it, and what each way of
    /// matching keeps of the two.
    fn of(song: usize, reader: &mut ChordReader, tune: Vec<Note>, backing: &[Note], bars: i64) -> Stretch {
        let parts = [Part { role: Role::Harmony, notes: backing }];
        let grid = Grid::four_four(PPQ as u32);
        let length = bars * 4 * PPQ;
        let reading = reader.read(&parts, grid, &ChordContext::new(Span::Window));
        Stretch {
            song,
            read: reading.as_ref().map(|r| Backing::of(r, &parts)),
            whole: reading.as_ref().map(|r| Backing::over(r, &parts, 0, length)),
            old: Old::of(&tune, backing, bars as usize * 2),
            tune_chroma: kept_chroma(&tune),
            backing_chroma: kept_chroma(backing),
            tune_slots: slots_over(&tune, SLOT, 0, length),
            backing_slots: slots_over(backing, SLOT, 0, length),
            tune,
        }
    }
}

/// What the library has kept in memory of a part's notes so far: how long each pitch class sounds over all of it,
/// each note counted from a sixteenth to two bars, the strongest as 255 (`mo_midi::analyser::chroma_of`, as bytes).
fn kept_chroma(notes: &[Note]) -> [f64; 12] {
    let bar = (4 * PPQ) as f64;
    let mut chroma = [0f64; 12];
    for n in notes {
        if let Some(c) = chroma.get_mut(n.pitch_class().index()) {
            *c += (n.duration() as f64 / bar).clamp(1.0 / 16.0, 2.0);
        }
    }
    let most = chroma.iter().copied().fold(0.0, f64::max).max(1e-9);
    chroma.map(|c| (c / most * 255.0).round_ties_even().clamp(0.0, 255.0))
}

/// How alike a tune's notes and some chords' notes are with no timing, at the best of the 12 moves (the close
/// look's `alike`, on what the library keeps of a part).
fn alike(tune: &[f64; 12], chords: &[f64; 12]) -> f64 {
    let (mine, theirs): (f64, f64) = (tune.iter().map(|t| t * t).sum(), chords.iter().map(|t| t * t).sum());
    if mine <= 0.0 || theirs <= 0.0 {
        return f64::NEG_INFINITY;
    }
    (0..12)
        .map(|s| {
            let on: f64 = tune
                .iter()
                .enumerate()
                .map(|(pc, time)| time * chords.get((pc + s) % 12).copied().unwrap_or(0.0))
                .sum();
            on / (mine * theirs).sqrt()
        })
        .fold(f64::NEG_INFINITY, f64::max)
}

/// The shares of the library a first look might keep for the close look.
const KEPT: [f64; 6] = [0.005, 0.01, 0.02, 0.05, 0.1, 0.2];
/// How many of the close look's best are looked for among those kept.
const BEST: usize = 10;

/// The first looks tried, each with which way round it's asked (true: a tune for the stretch's chords).
const QUICK_WAYS: [(&str, bool); 6] = [
    ("a tune for chords, by their notes with no timing (how alike)", true),
    ("a tune for chords, two beats at a time (the first look: 12 bytes each a part)", true),
    ("chords for a tune, by their notes with no timing (how alike)", false),
    ("chords for a tune, each chord part guessed two beats at a time (the first look)", false),
    ("chords for a tune, each chord part's own chords kept two beats at a time (12 numbers each)", false),
    ("chords for a tune, the old app's closer look (24 numbers a half bar kept)", false),
];

/// One stretch's answers: for each first look and each share kept, whether the close look's best is among those
/// kept, and how many of its best [`BEST`] are.
#[derive(Clone, Copy, Default)]
struct Kept {
    first: [[f64; KEPT.len()]; QUICK_WAYS.len()],
    best: [[f64; KEPT.len()]; QUICK_WAYS.len()],
    asked: [f64; QUICK_WAYS.len()],
}

/// What a first look goes by, worked out once for every stretch.
struct Looks {
    /// Each stretch's chords as read; none where the reader hears nothing in them.
    outlines: Vec<Option<Outline>>,
    /// Each stretch's chords guessed from its accompaniment's slots.
    guessed: Vec<Outline>,
}

impl Looks {
    fn of(stretches: &[Stretch], how: &Sitting) -> Looks {
        Looks {
            outlines: stretches.iter().map(|s| s.whole.as_ref().map(|b| b.outline(SLOT, how))).collect(),
            guessed: stretches.iter().map(|s| Outline::guessed(&s.backing_slots, how)).collect(),
        }
    }
}

/// What each first look keeps for the stretches in `queries`, each asked against every stretch of another song.
fn first_looks(stretches: &[Stretch], looks: &Looks, queries: &[usize], how: &Sitting) -> Vec<Kept> {
    let nothing = f64::NEG_INFINITY;
    queries
        .par_iter()
        .map(|&q| {
            let mut kept = Kept::default();
            let Some(own) = stretches.get(q) else { return kept };
            // Its own chords as read: only looking for a tune needs them. Looking for chords needs its tune alone.
            let outline = looks.outlines.get(q).and_then(Option::as_ref);
            let others: Vec<usize> =
                stretches.iter().enumerate().filter(|(_, s)| s.song != own.song).map(|(o, _)| o).collect();
            // The close look, each way round.
            let close_of = |tune_for_chords: bool| -> Vec<f64> {
                others
                    .iter()
                    .map(|&o| {
                        let (t, b) = if tune_for_chords { (o, q) } else { (q, o) };
                        score(stretches, t, b, Way::New, how).map_or(nothing, |s| s.0)
                    })
                    .collect()
            };
            let (tunes, chords) = (close_of(true), close_of(false));
            let first_look = |way: usize, o: usize| -> f64 {
                let Some(other) = stretches.get(o) else { return nothing };
                let best = |sits: Option<mo_harmony::Sits>| sits.map_or(nothing, |s| s.best().1);
                match way {
                    0 => alike(&other.tune_chroma, &own.backing_chroma),
                    1 => best(outline.and_then(|outline| outline.sits(&other.tune_slots))),
                    2 => alike(&own.tune_chroma, &other.backing_chroma),
                    3 => best(looks.guessed.get(o).and_then(|guess| guess.sits(&own.tune_slots))),
                    4 => best(looks.outlines.get(o).and_then(|o| o.as_ref()).and_then(|o| o.sits(&own.tune_slots))),
                    _ => old_fit(&own.old, &other.old, 12).0,
                }
            };
            for (way, (_, tune_for_chords)) in QUICK_WAYS.iter().enumerate() {
                if *tune_for_chords && outline.is_none() {
                    continue;
                }
                let close = if *tune_for_chords { &tunes } else { &chords };
                let at = |i: usize| close.get(i).copied().unwrap_or(nothing);
                // The close look's best, in order.
                let mut by_close: Vec<usize> = (0..others.len()).filter(|&i| at(i) > nothing).collect();
                by_close.sort_by(|&a, &b| at(b).total_cmp(&at(a)));
                by_close.truncate(BEST);
                let Some(&the_best) = by_close.first() else { continue };
                let looked: Vec<f64> = others.iter().map(|&o| first_look(way, o)).collect();
                // Each part's place in the first look's order.
                let mut order: Vec<usize> = (0..others.len()).collect();
                order.sort_by(|&a, &b| {
                    looked.get(b).copied().unwrap_or(nothing).total_cmp(&looked.get(a).copied().unwrap_or(nothing))
                });
                let mut place = vec![usize::MAX; others.len()];
                for (at, &i) in order.iter().enumerate() {
                    if let Some(p) = place.get_mut(i) {
                        *p = at;
                    }
                }
                let place_of = |i: usize| place.get(i).copied().unwrap_or(usize::MAX);
                if let Some(a) = kept.asked.get_mut(way) {
                    *a = 1.0;
                }
                for (k, share) in KEPT.iter().enumerate() {
                    let keep = ((others.len() as f64 * share).ceil() as usize).max(1);
                    if let Some(cell) = kept.first.get_mut(way).and_then(|w| w.get_mut(k)) {
                        *cell = f64::from(u8::from(place_of(the_best) < keep));
                    }
                    if let Some(cell) = kept.best.get_mut(way).and_then(|w| w.get_mut(k)) {
                        *cell = by_close.iter().filter(|&&i| place_of(i) < keep).count() as f64 / by_close.len() as f64;
                    }
                }
            }
            kept
        })
        .collect()
}

/// `--quick N`: what a first look loses, and how long it takes. See the top of the file.
fn quick(stretches: &[Stretch], asked: usize, how: &Sitting) {
    let looks = Looks::of(stretches, how);
    let step = (stretches.len() / asked.max(1)).max(1);
    let queries: Vec<usize> = (0..stretches.len()).step_by(step).collect();
    let answers = first_looks(stretches, &looks, &queries, how);

    println!(
        "\nA first look at every part, then the close look at the share it keeps ({} stretches asked,",
        queries.len()
    );
    println!("each against the {} or so stretches of other songs)", stretches.len());
    println!("  share kept:                 {}", KEPT.map(|s| format!("{:>6.1}%", s * 100.0)).join(" "));
    for (way, (name, _)) in QUICK_WAYS.iter().enumerate() {
        let asked: f64 = answers.iter().filter_map(|a| a.asked.get(way)).sum();
        let line = |cells: fn(&Kept) -> &[[f64; KEPT.len()]; QUICK_WAYS.len()]| -> String {
            (0..KEPT.len())
                .map(|k| {
                    let sum: f64 =
                        answers.iter().filter_map(|a| cells(a).get(way).and_then(|w| w.get(k))).copied().sum();
                    format!("{:>6.1}%", if asked > 0.0 { 100.0 * sum / asked } else { 0.0 })
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        println!("\n{name}");
        println!("  the close look's best kept: {}", line(|a| &a.first));
        println!("  of its best {BEST}:             {}", line(|a| &a.best));
    }

    // How long each look takes a part, on one core: every part against the first stretch whose chords were read.
    let read = looks.outlines.iter().position(Option::is_some);
    let (Some(read), Some(own)) = (read, stretches.first()) else {
        println!("\nNo stretch's chords were read: nothing to time.");
        return;
    };
    let Some(Some(outline)) = looks.outlines.get(read) else { return };
    let per_part = |seconds: f64, rounds: usize| seconds * 1e6 / (rounds * stretches.len()) as f64;
    let mut sum = 0.0;
    let started = std::time::Instant::now();
    for _ in 0..20 {
        for other in stretches {
            sum += outline.sits(&other.tune_slots).map_or(0.0, |s| s.best().1);
        }
    }
    let tune_first = per_part(started.elapsed().as_secs_f64(), 20);
    let started = std::time::Instant::now();
    for _ in 0..20 {
        for other in stretches {
            sum += Outline::guessed(&other.backing_slots, how).sits(&own.tune_slots).map_or(0.0, |s| s.best().1);
        }
    }
    let chords_first = per_part(started.elapsed().as_secs_f64(), 20);
    let started = std::time::Instant::now();
    for o in 0..stretches.len() {
        sum += score(stretches, o, read, Way::New, how).map_or(0.0, |s| s.0);
    }
    let tune_close = per_part(started.elapsed().as_secs_f64(), 1);
    println!(
        "\nA part takes, on one core: the first look {tune_first:.2} microseconds as a tune and {chords_first:.2} as \
         chords; the close look {tune_close:.2} as a tune, with its notes already read ({sum:.0})"
    );
}

/// What one stretch's questions came to.
#[derive(Clone, Copy, Default)]
struct Tally {
    /// Given the tune: its own accompaniment against another song's (wins, a tie half), and first of them all.
    chords_wins: f64,
    chords_pairs: f64,
    chords_first: f64,
    /// Against another stretch of its own song.
    chords_near_wins: f64,
    chords_near_pairs: f64,
    /// Given the accompaniment: its own tune against another song's.
    tune_wins: f64,
    tune_pairs: f64,
    tune_first: f64,
    tune_near_wins: f64,
    tune_near_pairs: f64,
    /// Its own pair sits best where it is (no move).
    unmoved: f64,
    asked: f64,
}

impl Tally {
    fn plus(&self, o: &Tally) -> Tally {
        Tally {
            chords_wins: self.chords_wins + o.chords_wins,
            chords_pairs: self.chords_pairs + o.chords_pairs,
            chords_first: self.chords_first + o.chords_first,
            chords_near_wins: self.chords_near_wins + o.chords_near_wins,
            chords_near_pairs: self.chords_near_pairs + o.chords_near_pairs,
            tune_wins: self.tune_wins + o.tune_wins,
            tune_pairs: self.tune_pairs + o.tune_pairs,
            tune_first: self.tune_first + o.tune_first,
            tune_near_wins: self.tune_near_wins + o.tune_near_wins,
            tune_near_pairs: self.tune_near_pairs + o.tune_near_pairs,
            unmoved: self.unmoved + o.unmoved,
            asked: self.asked + o.asked,
        }
    }

    fn print(&self) {
        let pc = |a: f64, b: f64| if b > 0.0 { 100.0 * a / b } else { 0.0 };
        println!(
            "  chords for a tune:  its own over another song's {:5.1}%   first of them all {:5.1}%   over another \
             stretch of its song {:5.1}%",
            pc(self.chords_wins, self.chords_pairs),
            pc(self.chords_first, self.asked),
            pc(self.chords_near_wins, self.chords_near_pairs)
        );
        println!(
            "  a tune for chords:  its own over another song's {:5.1}%   first of them all {:5.1}%   over another \
             stretch of its song {:5.1}%",
            pc(self.tune_wins, self.tune_pairs),
            pc(self.tune_first, self.asked),
            pc(self.tune_near_wins, self.tune_near_pairs)
        );
        println!("  its own pair sits best unmoved {:5.1}%", pc(self.unmoved, self.asked));
    }
}

/// How stretch `t`'s tune sits over stretch `b`'s accompaniment, at its best move, and that move (0 to 11 up). None
/// when the way can't say: chords the reader hears nothing in, a tune that hardly plays where they sound.
fn score(stretches: &[Stretch], t: usize, b: usize, way: Way, how: &Sitting) -> Option<(f64, usize)> {
    let (tune, back) = (stretches.get(t)?, stretches.get(b)?);
    match way {
        Way::New => {
            let sits = shares(&tune.tune, back.read.as_ref()?, LineUp::AsWritten, how)?.sits(how);
            let (semitones, score) = sits.best();
            Some((score, semitones.rem_euclid(12) as usize))
        }
        Way::Old => Some(old_fit(&tune.old, &back.old, 3)),
        Way::OldAll => Some(old_fit(&tune.old, &back.old, 12)),
    }
}

fn tally(stretches: &[Stretch], q: usize, others: usize, way: Way, how: &Sitting) -> Tally {
    let Some(own) = stretches.get(q) else { return Tally::default() };
    let mut t = Tally { asked: 1.0, ..Tally::default() };
    // A stretch whose own pair the way can't score is lost on every count; another song's part it can't score would
    // never be offered, so the stretch's own beats it.
    let nothing = f64::NEG_INFINITY;
    let own_pair = score(stretches, q, q, way, how);
    let (mine, scored) = (own_pair.map_or(nothing, |m| m.0), own_pair.is_some());
    t.unmoved = f64::from(u8::from(own_pair.is_some_and(|m| m.1 == 0)));
    let other = |t: usize, b: usize| score(stretches, t, b, way, how).map_or(nothing, |s| s.0);
    let win = |a: f64, b: f64| {
        if !scored {
            0.0
        } else if a > b {
            1.0
        } else if a == b {
            0.5
        } else {
            0.0
        }
    };
    let (mut chords_first, mut tune_first) = (true, true);
    for o in others_of(stretches, q, others) {
        let (chords, tune) = (other(q, o), other(o, q));
        t.chords_wins += win(mine, chords);
        t.chords_pairs += 1.0;
        t.tune_wins += win(mine, tune);
        t.tune_pairs += 1.0;
        chords_first &= mine > chords;
        tune_first &= mine > tune;
    }
    t.chords_first = f64::from(u8::from(chords_first));
    t.tune_first = f64::from(u8::from(tune_first));
    // The other stretches of its own song.
    for (o, _) in stretches.iter().enumerate().filter(|(o, s)| *o != q && s.song == own.song) {
        t.chords_near_wins += win(mine, other(q, o));
        t.chords_near_pairs += 1.0;
        t.tune_near_wins += win(mine, other(o, q));
        t.tune_near_pairs += 1.0;
    }
    t
}

// The C# app's matcher (HarmonyMatch.cs), as it was. Its chord-order score (ChordFollow) is left out: it only helped
// pick which 3 moves got the closer look, and the closer look alone was the score.

/// Half a bar, in ticks.
const SLOT: i64 = 2 * PPQ;

/// `HarmonyTimeline.Rub`.
const RUB: [f32; 12] = [0.0, 1.0, 0.2, 0.0, 0.0, 0.0, 0.35, 0.0, 0.0, 0.0, 0.2, 0.25];

impl Old {
    fn of(tune: &[Note], backing: &[Note], count: usize) -> Old {
        Old {
            tune_chroma: old_chroma(tune),
            backing_chroma: old_chroma(backing),
            tune_weights: old_weights(tune, count),
            backing_sits: old_sits(&old_weights(backing, count), count),
            count,
        }
    }
}

/// `PhraseAnalyser`'s chroma: each note by its length in bars (a sixteenth of a bar at least, two bars at most), as
/// shares adding up to 1.
fn old_chroma(notes: &[Note]) -> [f32; 12] {
    let mut chroma = [0f32; 12];
    for n in notes {
        if let Some(c) = chroma.get_mut(usize::from(n.key % 12)) {
            *c += (n.length as f64 / (4 * PPQ) as f64).clamp(1.0 / 16.0, 2.0) as f32;
        }
    }
    let total: f32 = chroma.iter().sum();
    if total > 0.0 {
        chroma.iter_mut().for_each(|c| *c /= total);
    }
    chroma
}

/// `HarmonyTimeline.Weights`.
fn old_weights(notes: &[Note], count: usize) -> Vec<f32> {
    let mut w = vec![0f32; count * 12];
    for n in notes {
        let (from, to) = (n.start.max(0), n.start + n.length.max(1));
        let mut k = (from / SLOT) as usize;
        while k < count && (k as i64) * SLOT < to {
            let overlap = to.min((k as i64 + 1) * SLOT) - from.max(k as i64 * SLOT);
            if overlap > 0
                && let Some(cell) = w.get_mut(k * 12 + usize::from(n.key % 12))
            {
                *cell += overlap as f32 / SLOT as f32;
            }
            k += 1;
        }
    }
    w
}

/// `HarmonyTimeline.Of`: how each pitch class sits in each half bar; NaN in the first of a half bar with no sound.
fn old_sits(w: &[f32], count: usize) -> Vec<f32> {
    let mut sits = vec![0f32; count * 12];
    for k in 0..count {
        let at = k * 12;
        let slot: Vec<f32> = w.get(at..at + 12).unwrap_or_default().to_vec();
        let max = slot.iter().copied().fold(0f32, f32::max);
        let Some(out) = sits.get_mut(at..at + 12) else { continue };
        if max <= 0.0 {
            if let Some(first) = out.first_mut() {
                *first = f32::NAN;
            }
            continue;
        }
        for (p, cell) in out.iter_mut().enumerate() {
            let share = slot.get(p).copied().unwrap_or(0.0) / max;
            if share >= 0.25 {
                *cell = 1.0;
                continue;
            }
            let mut rub = 0f32;
            for (q, other) in slot.iter().enumerate() {
                let other = other / max;
                if other >= 0.25 {
                    rub = rub.max(other * RUB.get((p + 12 - q) % 12).copied().unwrap_or(0.0));
                }
            }
            *cell = (0.6 + share * 1.6 - rub).clamp(-0.5, 1.0);
        }
    }
    sits
}

/// `HarmonyMatcher.Refine` for two loops of the same length: the best `looks` moves by the notes alone (`Moves`,
/// without the chord order), each played together (`Together`); the best, and its move.
fn old_fit(tune: &Old, back: &Old, looks: usize) -> (f64, usize) {
    // `Strongest`: the target's chroma with its strongest at 1.
    let top = back.backing_chroma.iter().copied().fold(0f32, f32::max);
    let target: Vec<f32> = back.backing_chroma.iter().map(|c| if top > 0.0 { c / top } else { 0.0 }).collect();
    let (mut total, mut mine, mut theirs) = (0f64, 0f64, 0f64);
    for &c in &tune.tune_chroma {
        total += f64::from(c);
        mine += f64::from(c) * f64::from(c);
    }
    for &t in &target {
        theirs += f64::from(t) * f64::from(t);
    }
    let mut moves: Vec<(f64, usize)> = (0..12)
        .map(|s| {
            let (mut on, mut off) = (0f64, 0f64);
            if total > 0.0 && theirs > 0.0 {
                for (p, &c) in tune.tune_chroma.iter().enumerate().filter(|(_, c)| **c > 0.0) {
                    let t = target.get((p + s) % 12).copied().unwrap_or(0.0);
                    on += f64::from(c) * f64::from(t);
                    if t < 0.1 {
                        off += f64::from(c);
                    }
                }
            }
            let (tonal, clash) =
                if total > 0.0 && theirs > 0.0 { (on / (mine * theirs).sqrt(), off / total) } else { (0.0, 0.0) };
            let semitones = if s <= 5 { s as f64 } else { 12.0 - s as f64 };
            ((tonal - 0.6 * clash).clamp(0.0, 1.0) - semitones * 0.0005, s)
        })
        .map(|(score, s)| (score.clamp(0.0, 1.0), s))
        .collect();
    moves.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut best: Option<(f64, usize)> = None;
    for &(first_look, s) in moves.iter().take(looks) {
        let Some(together) = old_together(tune, back, s) else { continue };
        let score = (together + 0.001 * first_look).clamp(0.0, 1.0);
        if best.is_none_or(|b| score > b.0) {
            best = Some((score, s));
        }
    }
    best.or_else(|| moves.first().copied()).unwrap_or((0.0, 0))
}

/// `HarmonyMatcher.Together` for two loops from the same bar.
fn old_together(tune: &Old, back: &Old, shift: usize) -> Option<f64> {
    let (count, theirs_count) = (tune.count, back.count);
    if count == 0 || theirs_count == 0 {
        return None;
    }
    let gcd = {
        let (mut a, mut b) = (count, theirs_count);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a.max(1)
    };
    let both = count / gcd * theirs_count;
    let length = if both <= 64 { both } else { count.max(theirs_count) };
    let (mut sum, mut heard, mut all) = (0f64, 0f64, 0f64);
    for i in 0..length {
        let mine = tune.tune_weights.get((i % count) * 12..(i % count) * 12 + 12).unwrap_or_default();
        let theirs = back.backing_sits.get((i % theirs_count) * 12..(i % theirs_count) * 12 + 12).unwrap_or_default();
        let w: f64 = mine.iter().map(|&x| f64::from(x)).sum();
        if w <= 0.0 {
            continue;
        }
        all += w;
        if theirs.first().is_none_or(|f| f.is_nan()) {
            continue;
        }
        heard += w;
        for (p, &m) in mine.iter().enumerate().filter(|(_, m)| **m > 0.0) {
            sum += f64::from(m) * f64::from(theirs.get((p + shift) % 12).copied().unwrap_or(0.0));
        }
    }
    if all <= 0.0 || heard < 0.25 * all {
        return None;
    }
    Some((sum / heard).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: i64, length: i64, key: u8) -> Note {
        Note { start, length, key, velocity: 50_000 }
    }

    #[test]
    fn the_old_way_scores_a_tune_on_the_chord_and_one_a_half_step_off() {
        // C major held for a bar; a tune on E, and one on F (a half step above E: the avoid note).
        let chord = [note(0, 4 * PPQ, 48), note(0, 4 * PPQ, 52), note(0, 4 * PPQ, 55)];
        let on = Old::of(&[note(0, 4 * PPQ, 76)], &chord, 2);
        let off = Old::of(&[note(0, 4 * PPQ, 77)], &chord, 2);
        assert_eq!(old_together(&on, &on, 0), Some(1.0));
        // 0.6 - 1.0 (the rub): under 0, kept at 0.
        assert_eq!(old_together(&off, &off, 0), Some(0.0));
        // Moved down a half step it's the E again.
        assert_eq!(old_together(&off, &off, 11), Some(1.0));
        assert_eq!(old_fit(&off, &off, 12).1, 11);
    }

    /// A bar of C major under an E; its chords read, or not (as when the reader hears nothing in them).
    fn stretch(song: usize, read: bool) -> Stretch {
        let chord = [note(0, 4 * PPQ, 48), note(0, 4 * PPQ, 52), note(0, 4 * PPQ, 55)];
        let tune = vec![note(0, 4 * PPQ, 76)];
        let mut stretch = Stretch::of(song, &mut ChordReader::new(), tune, &chord, 1);
        assert!(stretch.read.is_some());
        if !read {
            stretch.read = None;
        }
        stretch
    }

    /// Thirteen songs of a bar each, all in whole half bars: C then G under the first, and a tune of two notes
    /// over each, a different step apart in every song.
    fn half_bar_songs() -> Vec<Stretch> {
        let half = 2 * PPQ;
        let chords = [
            note(0, half, 48),
            note(0, half, 52),
            note(0, half, 55),
            note(half, half, 43),
            note(half, half, 47),
            note(half, half, 50),
        ];
        let mut reader = ChordReader::new();
        (0..13u8)
            .map(|song| {
                let tune = vec![note(0, half, 72), note(half, half, 72 + song)];
                Stretch::of(usize::from(song), &mut reader, tune, &chords, 1)
            })
            .collect()
    }

    #[test]
    fn a_first_look_is_counted_by_where_it_puts_the_close_looks_best() {
        let how = Sitting::SHIPPED;
        let stretches = half_bar_songs();
        let kept = first_looks(&stretches, &Looks::of(&stretches, &how), &[0], &how);
        let kept = kept.first().expect("one stretch asked");
        // Way 1, a tune for the chords two beats at a time: these tunes are whole half bars, so the first look says
        // what the close look does, and puts its best first whatever share is kept.
        assert_eq!(kept.asked[1], 1.0);
        assert_eq!(kept.first[1], [1.0; KEPT.len()]);
        // Of 12 other songs, the shares keep 1, 1, 1, 1, 2 and 3 parts: that many of the close look's best ten.
        assert_eq!(kept.best[1], [0.1, 0.1, 0.1, 0.1, 0.2, 0.3]);
    }

    #[test]
    fn a_stretch_whose_chords_arent_read_still_asks_for_chords_for_its_tune() {
        let how = Sitting::SHIPPED;
        let mut stretches = half_bar_songs();
        stretches[0].read = None;
        stretches[0].whole = None;
        let kept = first_looks(&stretches, &Looks::of(&stretches, &how), &[0], &how);
        // Looking for a tune for its chords can't be asked; looking for chords for its tune can.
        assert_eq!(kept.first().map(|k| k.asked), Some([0.0, 0.0, 1.0, 1.0, 1.0, 1.0]));
    }

    #[test]
    fn quick_says_what_it_cant_measure() {
        let run = |args: &[&str]| run(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>());
        let none = run(&["no-such-file.jsonl", "--quick", "0"]).unwrap_err();
        assert!(none.contains("1 or more"), "{none}");
        let fit = run(&["no-such-file.jsonl", "--quick", "5", "--fit", "10"]).unwrap_err();
        assert!(fit.contains("give one of them"), "{fit}");
        let long = run(&["no-such-file.jsonl", "--quick", "5", "--bars", "40"]).unwrap_err();
        assert!(long.contains("--bars 32 or fewer"), "{long}");
    }

    #[test]
    fn notes_with_no_timing_are_alike_at_their_best_move() {
        let c = kept_chroma(&[note(0, 4 * PPQ, 48), note(0, 4 * PPQ, 52), note(0, 2 * PPQ, 55)]);
        assert_eq!((c[0], c[4], c[7]), (255.0, 255.0, 128.0));
        // The same notes a fourth up are as alike as can be, moved back.
        let f = kept_chroma(&[note(0, 4 * PPQ, 53), note(0, 4 * PPQ, 57), note(0, 2 * PPQ, 60)]);
        assert!((alike(&c, &f) - 1.0).abs() < 1e-9);
        assert_eq!(alike(&c, &[0.0; 12]), f64::NEG_INFINITY);
    }

    #[test]
    fn no_stretch_is_drawn_twice_and_none_from_its_own_song() {
        let stretches: Vec<Stretch> = (0..60).map(|i| stretch(i / 10, false)).collect();
        for q in [0, 17, 59] {
            let drawn = others_of(&stretches, q, 20);
            let mut unique = drawn.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!((drawn.len(), unique.len()), (20, 20));
            assert!(drawn.iter().all(|&o| o / 10 != q / 10));
        }
        // When there aren't that many, it stops with what there is.
        assert!(others_of(&stretches[..12], 0, 20).len() <= 2);
    }

    #[test]
    fn a_pair_the_new_way_cannot_score_is_lost() {
        // Three songs with the same bar; the first one's chords aren't read.
        let stretches = vec![stretch(0, false), stretch(1, true), stretch(2, true)];
        let lost = tally(&stretches, 0, 2, Way::New, &Sitting::SHIPPED);
        assert_eq!((lost.asked, lost.chords_pairs, lost.tune_pairs), (1.0, 2.0, 2.0));
        assert_eq!((lost.unmoved, lost.chords_wins, lost.tune_wins), (0.0, 0.0, 0.0));
        assert_eq!((lost.chords_first, lost.tune_first), (0.0, 0.0));
        // The second sits best unmoved. Its tune over the third's chords ties (the same music) and beats the chords
        // that can't be read; over its own chords the other two tunes (the same tune) tie with its own.
        let read = tally(&stretches, 1, 2, Way::New, &Sitting::SHIPPED);
        assert_eq!((read.unmoved, read.chords_wins, read.tune_wins), (1.0, 1.5, 1.0));
        assert_eq!((read.chords_first, read.tune_first), (0.0, 0.0), "a tie is not first");
    }

    #[test]
    fn a_number_is_set_by_name() {
        let mut how = Sitting::SHIPPED;
        set(&mut how, "outside=-0.25, longest=1").unwrap();
        assert_eq!((how.outside, how.longest), (-0.25, 1.0));
        assert!(set(&mut how, "outside").is_err());
        assert!(set(&mut how, "outside=low").is_err());
        assert!(set(&mut how, "loud=1").is_err());
    }
}
