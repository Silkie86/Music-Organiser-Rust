//! `theorybench candidates`: how often the chord reader's candidates (the labels it scores for a stretch of music, see
//! `mo_harmony::chordread::candidates`) hold the labelled chord. A chord missing from them can never be read, so this
//! is the reader's ceiling; the design asks for at least 99% of POP909-CL's time.
//!
//! Each labelled chord is lined up with the units the reader hears (its ends moved to the nearest unit start) and cut
//! into stretches of at most two bars, the longest the reader scores; each stretch counts by its length. A label so
//! short that both its ends move to the same unit start can't be placed: the report says how much time those take.
//! A chord is in when its root and core are among the candidates. On POP909-CL, whose labels are a template's plain triads and
//! sevenths while the piano often plays more, a major label also takes a major seventh or sixth, a minor label a minor
//! seventh or sixth, and a sus4 label a 7sus4 (the design's accept sets).
//!
//! The reader decides where chords change too, so the last row moves the labelled chord's ends by up to a unit either
//! way (as the fit counts a reading right), keeping each stretch to two bars at most: what it misses even then, it
//! can't find.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::ops::Range;
use std::path::PathBuf;

use mo_harmony::chordread::{self, Candidate, Core, CoreSet, Hearing, Label};
use mo_harmony::{PcSet, PitchClass, Role, Span};
use rayon::prelude::*;

use crate::harte::{self, Named};
use crate::item::{self, Item};
use crate::keys::{self, Parts, SongNotes};

/// The longest stretch the reader scores, in bars.
const MOST_BARS: usize = 2;

/// Which parts the reader hears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Input {
    /// Every part but the tune and drums, each with its own role: the accompaniment.
    Piano,
    /// Every part but drums.
    Full,
    /// The accompaniment as one keys part with its bass line taken out (the key finder's split), and no bass part:
    /// chords with their roots often not played.
    Upper,
}

impl Input {
    fn parse(text: &str) -> Option<Input> {
        match text {
            "piano" => Some(Input::Piano),
            "full" => Some(Input::Full),
            "upper" => Some(Input::Upper),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Input::Piano => "accompaniment",
            Input::Full => "with the tune",
            Input::Upper => "upper part, no bass",
        }
    }

    /// The item's parts as this input hears them, each in order of start.
    fn parts(self, item: &Item) -> Vec<(Role, Vec<mo_harmony::Note>)> {
        let song = SongNotes::of(item, if self == Input::Full { &Parts::Pitched } else { &Parts::Accompaniment });
        match self {
            Input::Upper => {
                let (_, upper) = crate::keyfit::split_bass(&song.merged);
                vec![(Role::Harmony, keys::harmony_notes(&upper))]
            }
            Input::Piano | Input::Full => song.by_part(),
        }
    }
}

/// The rows of the report: the labelled chord let in by each way, and by every way with its ends moved.
const ROWS: [&str; 4] = [
    "the tables (root played, or rootless)",
    "+ open fifths (major and minor)",
    "+ the escape: the reader's candidates",
    "the reader's, ends within a unit of the label's",
];

/// How one stretch came out.
struct Stretch {
    beats: f64,
    /// Whether each row holds the labelled chord.
    held: [bool; ROWS.len()],
    /// How many candidates the reader has for it.
    candidates: usize,
    /// When the reader's candidates miss it, even with its ends moved.
    miss: Option<Miss>,
}

/// How a piece's labelled chords came out.
#[derive(Default)]
struct Measured {
    stretches: Vec<Stretch>,
    /// The labels too short to place (both ends at the same unit start), how many and how long in beats (their
    /// time in the music heard).
    unplaced: (usize, f64),
}

struct Miss {
    id: String,
    start: i64,
    end: i64,
    label: String,
    core: Core,
    why: &'static str,
    heard: PcSet,
    candidates: Vec<Label>,
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut files = Vec::new();
    let mut inputs = vec![Input::Piano, Input::Full];
    let mut misses_to = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--inputs" => {
                let list = args.next().ok_or("--inputs needs a list, such as piano,full,upper")?;
                inputs = list
                    .split(',')
                    .map(|i| Input::parse(i.trim()).ok_or_else(|| format!("--inputs: {i}? (piano, full, upper)")))
                    .collect::<Result<_, _>>()?;
            }
            "--misses" => misses_to = Some(PathBuf::from(args.next().ok_or("--misses needs a file")?)),
            a if a.starts_with("--") => return Err(format!("Unknown option {a}.\n\n{}", crate::USAGE)),
            a => files.push(PathBuf::from(a)),
        }
    }
    if files.is_empty() {
        return Err(crate::USAGE.to_owned());
    }
    let mut misses = match &misses_to {
        Some(path) => Some(std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?),
        None => None,
    };
    for file in &files {
        let mut items = item::read(file)?;
        items.retain(|i| !i.chords.is_empty());
        println!();
        println!("{} ({} pieces with chord labels)", file.display(), items.len());
        for &input in &inputs {
            let measured: Vec<Measured> =
                items.par_iter().map_init(Hearing::new, |hearing, item| measure(hearing, item, input)).collect();
            let unplaced = measured.iter().fold((0, 0.0), |(n, b), m| (n + m.unplaced.0, b + m.unplaced.1));
            let stretches: Vec<Stretch> = measured.into_iter().flat_map(|m| m.stretches).collect();
            report(input, &stretches, unplaced);
            if let Some(out) = misses.as_mut() {
                for (s, m) in stretches.iter().filter_map(|s| s.miss.as_ref().map(|m| (s, m))) {
                    let names: Vec<String> = m.candidates.iter().map(Label::to_string).collect();
                    writeln!(
                        out,
                        "{}\t{}\t{}\t{}\t{}..{}\t{:.2}\t{}\t{}\t{}",
                        file.display(),
                        input.name(),
                        m.id,
                        m.label,
                        m.start,
                        m.end,
                        s.beats,
                        m.why,
                        m.heard,
                        names.join(" ")
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
        }
    }
    Ok(())
}

/// The labelled chord's root and core, and the cores that count as it: its own, or on POP909-CL (`lenient`) the
/// design's accept set. `None` for no chord, an unknown one, or one with no core (a root alone).
fn gold(text: &str, lenient: bool) -> Option<(Label, CoreSet)> {
    let Named::Chord(chord) = harte::named(text) else { return None };
    let core = Core::of(chord.intervals())?;
    let accept = match core {
        Core::Major if lenient => CoreSet::EMPTY.with(Core::Major).with(Core::Major7).with(Core::Major6),
        Core::Minor if lenient => CoreSet::EMPTY.with(Core::Minor).with(Core::Minor7).with(Core::Minor6),
        Core::Sus4 if lenient => CoreSet::EMPTY.with(Core::Sus4).with(Core::Dominant7Sus4),
        core => CoreSet::EMPTY.with(core),
    };
    Some((Label::new(chord.root(), core), accept))
}

/// The reader's candidates for some units, and the notes heard there.
fn candidates_for(hearing: &Hearing, units: Range<usize>, out: &mut Vec<Candidate>) -> PcSet {
    let ticks = hearing.presence_ticks(units.clone());
    let heard = chordread::heard_in(&ticks, hearing.sounding_ticks(units.clone()));
    // With no note to go by, nothing is heard and there are no candidates: any note will do.
    let lowest = hearing.lowest(units).map_or(PitchClass::C, PitchClass::of_key);
    chordread::candidates(heard, &ticks, lowest, out);
    heard
}

/// Every labelled chord of a piece, cut into the stretches the reader scores, and how each came out.
fn measure(hearing: &mut Hearing, item: &Item, input: Input) -> Measured {
    let parts = input.parts(item);
    let heard_parts: Vec<mo_harmony::Part> =
        parts.iter().map(|(role, notes)| mo_harmony::Part { role: *role, notes }).collect();
    if !hearing.hear(&heard_parts, keys::grid_of(item, false), Span::Song) {
        return Measured::default();
    }
    let lenient = item.source == "pop909cl";
    let most = MOST_BARS * hearing.bar_units().max(1);
    let units = hearing.units();
    let ppq = f64::from(item.ppq.max(1));
    let (music_start, music_end) = (hearing.start(0), hearing.start(units));
    let mut out = Measured::default();
    let (mut found, mut near) = (Vec::new(), Vec::new());
    for (start, end, text) in &item.chords {
        let Some((label, accept)) = gold(text, lenient) else { continue };
        let is_it = |l: Label| l.root == label.root && accept.contains(l.core);
        let (from, to) = (hearing.nearest_unit(*start), hearing.nearest_unit(*end));
        // A label is too short to place when both its ends move to the same unit start while some of it lies in
        // the music heard (not before the first unit or after the last).
        let inside = (*end).min(music_end) - (*start).max(music_start);
        if from == to && inside > 0 {
            out.unplaced.0 += 1;
            out.unplaced.1 += inside as f64 / ppq;
        }
        let mut at = from;
        while at < to {
            let stretch = at..(at + most).min(to);
            at = stretch.end;
            let beats = (hearing.start(stretch.end) - hearing.start(stretch.start)) as f64 / ppq;
            let heard = candidates_for(hearing, stretch.clone(), &mut found);
            let rel = heard.above(label.root);
            let tables = if rel.has(0) { chordread::valid(rel) } else { chordread::valid_rootless(rel) };
            let by_tables = tables.iter().any(|c| accept.contains(c));
            let by_fifths = by_tables || chordread::open_fifth(rel).iter().any(|c| accept.contains(c));
            let by_reader = found.iter().any(|c| is_it(c.label));
            // The labelled chord's own ends moved by up to a unit (not where a long one was cut).
            let mut nearby = by_reader;
            for (a, b) in moved(stretch.clone(), from, to, units, most) {
                if nearby {
                    break;
                }
                candidates_for(hearing, a..b, &mut near);
                nearby = near.iter().any(|c| is_it(c.label));
            }
            let miss = (!nearby).then(|| Miss {
                id: item.id.clone(),
                start: hearing.start(stretch.start),
                end: hearing.start(stretch.end),
                label: text.clone(),
                core: label.core,
                why: why(rel, label.core),
                heard,
                candidates: found.iter().map(|c| c.label).collect(),
            });
            let held = [by_tables, by_fifths, by_reader, nearby];
            out.stretches.push(Stretch { beats, held, candidates: found.len(), miss });
        }
    }
    out
}

/// The stretches with the labelled chord's ends (`from`, `to`) moved by up to a unit each, where they are its ends
/// (not where a long chord was cut), inside the units heard and at most `most` units long (the longest the reader
/// scores).
fn moved(
    stretch: Range<usize>,
    from: usize,
    to: usize,
    units: usize,
    most: usize,
) -> impl Iterator<Item = (usize, usize)> {
    let starts: &'static [isize] = if stretch.start == from { &[-1, 0, 1] } else { &[0] };
    let ends: &'static [isize] = if stretch.end == to { &[-1, 0, 1] } else { &[0] };
    let (a, b) = (stretch.start as isize, stretch.end as isize);
    starts
        .iter()
        .flat_map(move |&d| ends.iter().map(move |&e| (a + d, b + e)))
        .filter(move |&(x, y)| (x, y) != (a, b) && x >= 0 && x < y && y as usize <= units)
        .map(|(x, y)| (x as usize, y as usize))
        .filter(move |&(x, y)| y - x <= most)
}

/// Why a label isn't let in, for the list of misses.
fn why(rel: PcSet, core: Core) -> &'static str {
    if !rel.has(0) {
        "root not heard"
    } else if !rel.contains_all(core.third()) && !(core == Core::Dominant7Sus4 && rel.has(2)) {
        "third (or sus note) not heard"
    } else if !rel.contains_all(core.seventh()) {
        "seventh or sixth not heard"
    } else if !rel.contains_all(core.fifth()) {
        "fifth not heard"
    } else {
        "only here, not a unit off"
    }
}

/// The rows for one input, and what's missed most; `unplaced` is how many labels were too short to place, and how
/// long they are in beats.
fn report(input: Input, stretches: &[Stretch], unplaced: (usize, f64)) {
    let total: f64 = stretches.iter().map(|s| s.beats).sum::<f64>().max(1e-9);
    let mean = stretches.iter().map(|s| s.candidates).sum::<usize>() as f64 / stretches.len().max(1) as f64;
    println!(
        "  {}: {} stretches, {:.0} beats; {:.1} candidates a stretch on average",
        input.name(),
        stretches.len(),
        total,
        mean
    );
    println!(
        "    left out, too short to place: {} labels, {:.2}% as long as the stretches measured",
        unplaced.0,
        100.0 * unplaced.1 / total
    );
    for (r, row) in ROWS.iter().enumerate() {
        let held: f64 = stretches.iter().filter(|s| s.held.get(r).copied().unwrap_or(false)).map(|s| s.beats).sum();
        println!("    {row:<50} {:>7.2}%", 100.0 * held / total);
    }
    let mut by: BTreeMap<(&str, &str), f64> = BTreeMap::new();
    for s in stretches {
        if let Some(m) = &s.miss {
            *by.entry((m.core.name(), m.why)).or_default() += s.beats;
        }
    }
    let mut rows: Vec<_> = by.into_iter().collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    for ((core, why), beats) in rows.into_iter().take(6) {
        let core = if core.is_empty() { "maj" } else { core };
        println!("      missed even so: {core:<8} {why:<30} {:>5.2}% of the time", 100.0 * beats / total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(chords: &[(i64, i64, &str)], parts: Vec<item::Part>) -> Item {
        let mut item: Item =
            serde_json::from_str(r#"{"source":"x","id":"1","ppq":480,"length":3840,"parts":[]}"#).unwrap();
        item.chords = chords.iter().map(|&(s, e, t)| (s, e, t.to_owned())).collect();
        item.parts = parts;
        item
    }

    fn part(role: &str, notes: &[(i64, i64, u8)]) -> item::Part {
        item::Part { role: role.to_owned(), notes: notes.iter().map(|&(s, l, k)| (s, l, k, 100)).collect() }
    }

    #[test]
    fn a_label_is_its_root_and_core_and_pop909_cl_takes_the_design_accept_sets() {
        let (c, exact) = gold("C:maj", false).unwrap();
        assert_eq!((c.to_string(), exact), ("C".to_owned(), CoreSet::EMPTY.with(Core::Major)));
        let (_, lenient) = gold("C:maj", true).unwrap();
        assert!(lenient.contains(Core::Major7) && lenient.contains(Core::Major6) && !lenient.contains(Core::Dominant7));
        assert_eq!(gold("A:min7/b3", true).unwrap().1, CoreSet::EMPTY.with(Core::Minor7), "sevenths stay exact");
        assert_eq!(gold("G:9", false).unwrap().0.to_string(), "G7");
        assert_eq!(gold("G:sus4(b7)", false).unwrap().0.to_string(), "G7sus4");
        assert!(gold("Bb:sus4", true).unwrap().1.contains(Core::Dominant7Sus4));
        assert_eq!(gold("N", false), None);
        assert_eq!(gold("X", false), None);
        assert_eq!(gold("C:1", false), None, "a root alone has no core");
    }

    #[test]
    fn each_labelled_chord_is_cut_into_stretches_and_each_row_counts_it() {
        // Bar 1: C E G under a C label. Bar 2: an open fifth, A E, under an A minor label: only the open-fifth way
        // lets it in. A G7 label after the music: no units, nothing to count.
        let keys = part("chords", &[(0, 1920, 60), (0, 1920, 64), (0, 1920, 67), (1920, 1920, 57), (1920, 1920, 64)]);
        let piece = item(&[(0, 1920, "C:maj"), (1920, 3840, "A:min"), (3840, 9600, "G:7")], vec![keys]);
        let mut hearing = Hearing::new();
        let measured = measure(&mut hearing, &piece, Input::Piano);
        let held: Vec<[bool; 4]> = measured.stretches.iter().map(|s| s.held).collect();
        assert_eq!(held, [[true; 4], [false, true, true, true]]);
        assert!(measured.stretches.iter().all(|s| s.miss.is_none() && (s.beats - 4.0).abs() < 1e-9));
        assert_eq!(measured.unplaced.0, 0, "after the music isn't too short");
        // A chord whose notes don't sound is missed, and listed with the candidates there (not those of the stretches
        // with its ends moved, which reach into the G chord after it).
        let keys =
            part("chords", &[(0, 960, 60), (0, 960, 64), (0, 960, 67), (960, 960, 62), (960, 960, 67), (960, 960, 71)]);
        let piece = item(&[(0, 960, "D:min7"), (960, 1920, "G:maj")], vec![keys]);
        let stretches = measure(&mut hearing, &piece, Input::Piano).stretches;
        let miss = stretches[0].miss.as_ref().expect("missed");
        assert_eq!((miss.why, miss.heard.to_string()), ("root not heard", "C E G".to_owned()));
        assert_eq!((miss.candidates.len(), stretches[0].candidates), (6, 6));
        assert_eq!(stretches[0].held, [false; 4]);
        // A chord longer than two bars is cut into stretches of two bars at most.
        let keys = part("chords", &[(0, 5760, 60), (0, 5760, 64), (0, 5760, 67)]);
        let piece = item(&[(0, 5760, "C:maj")], vec![keys]);
        let beats: Vec<f64> = measure(&mut hearing, &piece, Input::Piano).stretches.iter().map(|s| s.beats).collect();
        assert_eq!(beats, [8.0, 4.0]);
        // A label shorter than half a unit between two others can't be placed, and is counted as left out.
        let keys = part("chords", &[(0, 3840, 60), (0, 3840, 64), (0, 3840, 67)]);
        let piece = item(&[(0, 1900, "C:maj"), (1900, 1940, "G:maj"), (1940, 3840, "C:maj")], vec![keys]);
        let measured = measure(&mut hearing, &piece, Input::Piano);
        assert_eq!(measured.stretches.len(), 2);
        assert_eq!(measured.unplaced.0, 1);
        assert!((measured.unplaced.1 - 40.0 / 480.0).abs() < 1e-9);
        // Nor is a label before the music starts (the units start at the bar of the first note).
        let keys = part("chords", &[(1920, 1920, 60), (1920, 1920, 64), (1920, 1920, 67)]);
        let piece = item(&[(0, 1900, "F:maj"), (1900, 3840, "C:maj")], vec![keys]);
        let measured = measure(&mut hearing, &piece, Input::Piano);
        assert_eq!((measured.stretches.len(), measured.unplaced.0), (1, 0));
        assert_eq!(measured.stretches[0].held, [true; 4]);
    }

    #[test]
    fn moved_ends_stay_inside_the_units_and_keep_where_a_long_chord_is_cut() {
        let all: Vec<_> = moved(4..8, 4, 8, 8, 16).collect();
        assert_eq!(all, [(3, 7), (3, 8), (4, 7), (5, 7), (5, 8)]);
        // Cut at 8 (the label runs on to 12): only its start moves.
        assert_eq!(moved(4..8, 4, 12, 16, 16).collect::<Vec<_>>(), [(3, 8), (5, 8)]);
        assert_eq!(moved(0..1, 0, 1, 16, 16).collect::<Vec<_>>(), [(0, 2), (1, 2)]);
        // Never longer than the longest stretch the reader scores.
        assert_eq!(moved(4..8, 4, 8, 16, 4).collect::<Vec<_>>(), [(3, 7), (4, 7), (5, 7), (5, 8), (5, 9)]);
        assert_eq!(moved(4..8, 4, 12, 16, 4).collect::<Vec<_>>(), [(5, 8)]);
    }
}
