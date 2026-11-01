//! `theorybench keyfit`: fits mo-harmony's key finder on labelled music, measures it on songs held out of each fit,
//! and writes the weights into the crate.
//!
//! - **Folds.** Each song goes to one of five folds by a hash of its name (`pop909/001`), so POP909 and POP909-CL put
//!   a song in the same fold and every run splits the songs the same way; songs whose notes are the same, moved to
//!   another key, share a fold. Fold 0 is kept back. Every choice (evidence, penalties, calibration) is made on folds
//!   1 to 4, each scored by a finder fitted on the other three. `--final` scores fold 0 on its own, with what was
//!   chosen, and adds a line to `test-runs.md`, so how often fold 0 was looked at is on record. `--write`, the last
//!   step, scores every fold before it writes the weights, so it adds a line too.
//! - **Cases.** Whole songs (read as songs); stretches of 1 to 16 bars cut along the bar lines (read as windows, and
//!   scored against the key labelled at their middle, as the C# bench did); and each song's first bars from its first
//!   bar with notes (read as loops, standing in for a pack's loops). Each is also heard as four other kinds of input:
//!   the tune alone, the piano without its bass line, the bass line taken from the piano, and the piano.
//! - **Model.** Each stream's weights are a counted table (the average share of each note counted up from the labelled
//!   tonic, over the training songs) times one trust weight, plus a correction for each note, held near 0 by its own
//!   penalty (`--lambda-delta`; `inf` keeps the tables as they are). A song is fitted with tables counted without its
//!   own fifth of the training songs, so the tables can't flatter themselves.
//! - **Calibration.** How sure a reading may be is fitted on scores of songs held out of the fit: a curve over how many
//!   bars differ, offsets for the kind of input, a curve a loop adds and an offset for a song (`Calibration` in the
//!   crate).
//! - **Answers.** Each reading is also graded as the app shows it (one key, a key or its relative, a home note, no
//!   clear key: `keyfind::grade`), and the report says how often each kind is given and how often it's right. The
//!   chance each kind needs at each length, and the share of notes in one scale a key needs, are fitted on the
//!   chances of songs held out ([`answerfit`]); each fold is graded with those fitted on the other folds.
//! - **Hand cases.** Every model fitted is read on the design's hand cases (`keyfind::cases`), and the report says how
//!   many keep each gate and meet each target. `--write` won't write weights that break a gate.
//!
//! Only stretches inside one key region are fitted on (at most eight a song for each length); every stretch is
//! measured, so the numbers stay comparable with the C# app's on the same cases.
//!
//! - **Sources.** Several files can be read at once. Only pieces whose licence lets fitted weights ship (MIT, CC0,
//!   CC BY: [`Item::may_learn`]) are learned from. The rest are measured only: by finders fitted without them, never
//!   from the kept-back fold, and by `--final`'s finder too. A piece only measured whose notes (in any key) are also
//!   in a piece learned from is left out, since its finder learned them. `--weigh source=w` sets how much a source's
//!   sets count in the fit and the calibration (1 unless it says otherwise; 0 counts it in the tables only), and
//!   `--weigh source/input=w` one kind of input's sets (`billboard/upper=0`). The report puts the first file's
//!   source first, as before, then each other source, with how often the key's tonic was found (chord charts label
//!   only the tonic).
//! - **Solomon's packs.** Pieces from `theorybench packs` are his own music ([`item::OWN`]): a plain run only measures
//!   them, each pack's files in one fold, each part heard whole as the MIDI page hears it (as a loop, or as a window
//!   when the page cut it from a part longer than 16 bars). `--shipped` scores the finder built in, as
//!   it is (folds 1 to 4; nothing fitted), to compare a refit with (`--dump`, then `--against`). `--rescale` fits only
//!   what rule 5's exception allows (Solomon, 2026-11-01): the built-in finder's weights for each stream scaled by one
//!   number, the minor weight and the calibration, on his pieces alone (everything else read is only measured); the
//!   answers' thresholds stay as built in, and the penalty holds the numbers fitted near the built-in ones. A number
//!   keeps its built-in value unless at least 30 of his pieces ([`answerfit::MIN_SONGS`]) have it: a stream they
//!   sound, a calibration term they use, and for the minor weight, 30 labelled major and 30 minor. Pieces with the same
//!   notes in other keys count once, and share one piece's weight in a fit. The calibration's curve over bars moves
//!   only with full mixes among them, since a part heard alone adds its own offset to it. The ranges draw a pack's
//!   files again together.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;

use mo_harmony::keyfind::cases::{self, Book, Verdict};
use mo_harmony::keyfind::{
    self, CALIBRATION_TERMS, Calibration, Config, FEATURES, KEY_FINDER_VERSION, KeyAnswer, KeyReader, MINOR, STREAMS,
    Span, Stream, Why,
};
use mo_harmony::{Grid, Key, PitchClass, Role};
use rayon::prelude::*;

use crate::answerfit::{self, Fitted};
use crate::baseline;
use crate::fit::{self, Choice, Scored};
use crate::item::{self, Item, Note, Whole};
use crate::keys::{self, Answer, Found, MIN_NOTES, Run};
use crate::metrics::{self, pct};

/// A comparison's set drawn from fewer songs or packs than this has a range that says little, and is named.
const FEWEST_DRAWN: usize = 5;
/// Clip lengths fitted and measured unless `--bars` says otherwise: from a one-bar loop to a 16-bar section.
const LENGTHS: [i64; 5] = [1, 2, 4, 8, 16];

/// Folds the songs are split into.
const FOLDS: u64 = 5;
/// The fold kept back for `--final`.
const FROZEN: usize = 0;
/// Inner folds for counting the tables a song is fitted with.
const INNER: u64 = 5;
/// Windows of each length fitted on from each song, unless `--picks` says otherwise.
const PICKS: usize = 8;
/// How much the whole songs (as a full mix) count in the fit, against 1 for every other set of cases.
const SONG_WEIGHT: f64 = 5.0;
/// The penalty on the trust weights and the minor weight.
const LAMBDA_W: f64 = 1e-4;
/// The penalty on the corrections unless `--lambda-delta` says otherwise.
const LAMBDA_DELTA: f64 = 1e-4;
/// `--rescale` fits a stream's scale, or a calibration number, only when at least this many pieces sound it; with
/// fewer, it's held at its built-in number (a few pieces would push it anywhere).
const SUPPORT: usize = answerfit::MIN_SONGS;

/// FNV-1a, 64 bits: the fold hash, the same on every computer. The finder hashes bars with FNV-1a too, but this is
/// its own copy: the folds must never move, even if how the finder tells bars apart changes.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The kinds of input every case is heard as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Input {
    /// Every pitched part: the tune (and POP909's second tune) as melody, the piano as harmony.
    Full,
    /// The tunes alone.
    Tune,
    /// The piano without its bass line.
    Upper,
    /// The piano's bass line, as a bass part.
    Bass,
    /// The piano.
    Piano,
}

impl Input {
    const ALL: [Input; 5] = [Input::Full, Input::Tune, Input::Upper, Input::Bass, Input::Piano];

    fn name(self) -> &'static str {
        match self {
            Input::Full => "full mix",
            Input::Tune => "tune only",
            Input::Upper => "upper piano",
            Input::Bass => "bass from the piano",
            Input::Piano => "piano",
        }
    }

    /// The first word of its name, as the command line gives it.
    fn short(self) -> &'static str {
        self.name().split(' ').next().unwrap_or_default()
    }

    fn parse(text: &str) -> Option<Input> {
        Input::ALL.into_iter().find(|i| i.short() == text)
    }
}

/// Where a case is cut from the song.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Cut {
    Song,
    /// A whole loop (a pack's file), heard as a loop.
    Loop,
    /// A whole piece the MIDI page cut from a longer part of a pack's file, heard as a window.
    Piece,
    /// A stretch of this many bars along the bar lines.
    Window(i64),
    /// The song's first bars, from its first bar with notes.
    Opening(i64),
}

impl Cut {
    fn span(self) -> Span {
        match self {
            Cut::Song => Span::Song,
            Cut::Loop => Span::Loop,
            Cut::Piece | Cut::Window(_) => Span::Window,
            Cut::Opening(_) => Span::Loop,
        }
    }

    fn name(self) -> String {
        match self {
            Cut::Song => "Whole songs".to_owned(),
            Cut::Loop => "Whole loops".to_owned(),
            Cut::Piece => "Pieces cut from longer parts".to_owned(),
            Cut::Window(b) => format!("{b}-bar windows"),
            Cut::Opening(b) => format!("{b}-bar openings"),
        }
    }
}

/// What the finder heard in one case, as the fit reads it: each stream's features for the keys with tonic C (any
/// other key's are the same, counted from its own tonic), and what the calibration and the answer need.
#[derive(Clone, Debug)]
struct Evidence {
    x: [[f64; 12]; STREAMS],
    bars: u32,
    config: Config,
    diatonic_fit: f64,
    notes: u32,
}

impl Evidence {
    fn of(heard: &keyfind::Heard, span: Span) -> Evidence {
        let mut x = [[0.0; 12]; STREAMS];
        heard.features(Key::major(PitchClass::C), span, |i, v| {
            if let Some(cell) = x.get_mut(i / 24).and_then(|row| row.get_mut(i % 24)) {
                *cell = v;
            }
        });
        Evidence {
            x,
            bars: heard.bars_heard(),
            config: heard.config(),
            diatonic_fit: heard.diatonic_fit(),
            notes: heard.notes,
        }
    }

    /// A key's features, exactly as `Heard::features` gives them (the same values, in the same order).
    fn features(&self, key: Key, each: &mut dyn FnMut(usize, f64)) {
        let mode = if key.is_minor() { 12 } else { 0 };
        let tonic = key.tonic.index();
        for (s, row) in self.x.iter().enumerate() {
            for d in 0..12 {
                let v = row[(tonic + d) % 12];
                if v != 0.0 {
                    each(s * 24 + mode + d, v);
                }
            }
        }
        if key.is_minor() {
            each(MINOR, 1.0);
        }
    }
}

/// One case: a song, or a stretch of it, heard as one kind of input.
struct Case {
    song: usize,
    truth: Key,
    cut: Cut,
    input: Input,
    /// It lies wholly inside one key region: only these are fitted on.
    one_key: bool,
    /// Picked to be fitted on: every song and opening, and a few windows of each length from each song.
    pick: bool,
    ev: Evidence,
    /// The C# app's key and confidence on the same notes.
    cs: Option<(Key, f64)>,
}

/// A song's name and folds, and how it's used.
#[derive(Clone, Debug, Default)]
struct Song {
    name: String,
    fold: usize,
    inner: usize,
    /// Which source it comes from, as an index into the sources in the order they were first read.
    group: usize,
    /// Its licence lets fitted weights ship, so it's learned from; the rest are only measured.
    learn: bool,
    /// How much its source's sets count in the fit and the calibration, for each kind of input in `Input::ALL`'s
    /// order (`--weigh`; 0 when it's only measured).
    weight: [f64; 5],
    /// Songs drawn into a fold together share this: a pack's files, a song and its twins. They're alike, so the
    /// ranges draw them again together, as one.
    draw: usize,
    /// Songs with the same notes, in this key or another, share this: what a number needs [`SUPPORT`] of.
    music: usize,
    /// How many songs read have its notes, in any key: each counts that much less in a fit, so a loop sold in every
    /// key counts as one.
    copies: usize,
}

impl Song {
    /// How much its sets of this kind of input count.
    fn weight(&self, input: Input) -> f64 {
        self.weight.get(input as usize).copied().unwrap_or(0.0)
    }

    /// Whether `--final`'s finder, fitted on folds 1 to 4, may score it: it's in the kept-back fold, or it's only
    /// measured and so never fitted on.
    fn unseen_by_final(&self) -> bool {
        self.fold == FROZEN || !self.learn
    }

    /// How much each of its cases counts against one with no twins.
    fn share(&self) -> f64 {
        1.0 / self.copies.max(1) as f64
    }
}

/// The song a case is drawn again with when its ranges are drawn: its [`Song::draw`].
fn draws(cases: &[Case], songs: &[Song], members: &[usize]) -> Vec<usize> {
    members.iter().map(|&i| cases.get(i).map_or(usize::MAX, |c| songs.get(c.song).map_or(c.song, |s| s.draw))).collect()
}

/// A song's name: POP909 and POP909-CL share their songs' names.
fn fold_name(item: &Item) -> String {
    let source = if item.source.starts_with("pop909") { "pop909" } else { item.source.as_str() };
    format!("{source}/{}", item.id)
}

/// What a song's fold is drawn from: its name, or its group's (a pack's files share a fold).
fn fold_draw(item: &Item) -> String {
    if item.group.is_empty() { fold_name(item) } else { format!("{}/{}", item.source, item.group) }
}

/// A hash of a song's notes that doesn't change when they're all moved up or down: the same song in another key.
fn shape_of(item: &Item) -> u64 {
    let notes = item.notes_of(|r| r != "drums");
    let low = notes.iter().map(|n| n.key).min().unwrap_or(0);
    let first = notes.first().map_or(0, |n| n.start);
    let mut bytes = Vec::with_capacity(notes.len() * 17);
    for n in &notes {
        bytes.extend((n.start - first).to_le_bytes());
        bytes.extend(n.length.to_le_bytes());
        bytes.push(n.key - low);
    }
    fnv1a(&bytes)
}

/// Which pieces a run learns from. `--shipped` learns nothing, but draws the folds as the run it's compared with
/// does (a rescale when Solomon's pieces are read, a plain run when they aren't), so their dumps pair case by case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Learning {
    /// Those whose licence lets fitted weights ship: a plain run.
    Licensed,
    /// Solomon's own alone (`--rescale`), of which nothing else is ever learned.
    Own,
}

/// What a run learns from: `--shipped` draws the folds as the run it's compared with does, a rescale when Solomon's
/// pieces are read.
fn learning_of(rescale: bool, shipped: bool, items: &[Item]) -> Learning {
    if rescale || (shipped && items.iter().any(Item::is_own)) { Learning::Own } else { Learning::Licensed }
}

/// Whether a run learns from a piece (with `--shipped`, whether its fold is drawn as if it did).
fn learns(item: &Item, learning: Learning) -> bool {
    match learning {
        Learning::Licensed => item.may_learn(),
        Learning::Own => item.is_own(),
    }
}

/// Each song's folds, songs with the same notes in another key going where the first of them by name goes (a pack's
/// files where its first by name goes). A song that's only measured goes to one of folds 1 to 4 (never the kept-back
/// fold), so every choosing run measures it.
fn songs_of(items: &[Item], sources: &[String], weigh: &HashMap<String, f64>, learning: Learning) -> Vec<Song> {
    let shapes: Vec<u64> = items.par_iter().map(shape_of).collect();
    let mut first_name: HashMap<u64, String> = HashMap::new();
    let mut copies: HashMap<u64, usize> = HashMap::new();
    for (item, shape) in items.iter().zip(&shapes) {
        let draw = fold_draw(item);
        first_name.entry(*shape).and_modify(|n| *n = n.clone().min(draw.clone())).or_insert(draw);
        *copies.entry(*shape).or_default() += 1;
    }
    // Numbers for the draws and the shapes, in the order they're first met.
    let mut draws: HashMap<String, usize> = HashMap::new();
    let mut musics: HashMap<u64, usize> = HashMap::new();
    items
        .iter()
        .zip(&shapes)
        .map(|(item, shape)| {
            let name = fold_name(item);
            let own = fold_draw(item);
            let drawn = first_name.get(shape).unwrap_or(&own);
            let h = fnv1a(drawn.as_bytes());
            let learn = learns(item, learning);
            let fold = if learn { (h % FOLDS) as usize } else { 1 + (h % (FOLDS - 1)) as usize };
            let n = draws.len();
            let draw = *draws.entry(drawn.clone()).or_insert(n);
            let n = musics.len();
            let music = *musics.entry(*shape).or_insert(n);
            Song {
                name,
                fold,
                inner: (h / FOLDS % INNER) as usize,
                group: sources.iter().position(|s| *s == item.source).unwrap_or(0),
                learn,
                weight: Input::ALL.map(|input| {
                    let named = format!("{}/{}", item.source, input.short());
                    let w = weigh.get(&named).or_else(|| weigh.get(&item.source)).copied().unwrap_or(1.0);
                    if learn { w } else { 0.0 }
                }),
                draw,
                music,
                copies: copies.get(shape).copied().unwrap_or(1),
            }
        })
        .collect()
}

/// Leaves out the pieces only measured whose notes, in some key, are also in a piece learned from: they'd measure a
/// finder on notes it learned. Gives how many each source lost.
fn leave_out_measured_twins(items: &mut Vec<Item>, learning: Learning) -> HashMap<String, usize> {
    let shapes: Vec<u64> = items.par_iter().map(shape_of).collect();
    let learned: HashSet<u64> =
        items.iter().zip(&shapes).filter(|(item, _)| learns(item, learning)).map(|(_, shape)| *shape).collect();
    let mut left_out: HashMap<String, usize> = HashMap::new();
    // `retain` visits each piece once, in order, so the shapes line up.
    let mut shapes = shapes.into_iter();
    items.retain(|item| {
        let twin = shapes.next().is_some_and(|shape| !learns(item, learning) && learned.contains(&shape));
        if twin {
            *left_out.entry(item.source.clone()).or_default() += 1;
        }
        !twin
    });
    left_out
}

/// How a source's sets are weighed: "1", "0.25", or each kind of input's ("full 0.5, tune 1, upper 0, ...").
fn weighing(weights: [f64; 5]) -> String {
    if weights.iter().all(|w| *w == weights[0]) {
        format!("{}", weights[0])
    } else {
        let each: Vec<String> = Input::ALL.iter().zip(weights).map(|(i, w)| format!("{} {w}", i.short())).collect();
        each.join(", ")
    }
}

/// Who made a source the weights are fitted on, and its licence, for the weights file.
fn credit(source: &str, licence: &str) -> String {
    match source {
        "pop909cl" => "POP909-CL: Yao, Chen, Dubnov and Berg-Kirkpatrick (2026), MIT licence".to_owned(),
        "pop909" => "POP909: Wang, Chen, Jiang, Zhang, Xu, Dai, Gu and Xia (ISMIR 2020), MIT licence".to_owned(),
        "billboard" => "The McGill Billboard charts: Burgoyne, Wild and Fujinaga (ISMIR 2011), CC0".to_owned(),
        crate::packs::SOURCE => {
            "Solomon's own MIDI packs, on his computer: only the stream scales, the minor weight and \
                                 the calibration, by his leave (2026-11-01, rule 5)"
                .to_owned()
        }
        _ => format!("{source}: {licence}"),
    }
}

/// Stops a run that has nothing to fit the weights on, which would otherwise fit zeros without a word.
fn check_learning(songs: &[Song], rescale: bool) -> Result<(), String> {
    if rescale && !songs.iter().any(|s| s.learn) {
        return Err(
            "--rescale learns only from Solomon's own pieces (made by `theorybench packs`), and none was read".into()
        );
    }
    let folds: HashSet<usize> = songs.iter().filter(|s| s.learn && s.fold != FROZEN).map(|s| s.fold).collect();
    if rescale && folds.len() < 2 {
        return Err(format!(
            "--rescale: Solomon's pieces fall in {} of folds 1 to 4, since a pack's files share a fold. Each fold is \
             scored by a fit on the others, so read more packs, each in a folder of its own",
            folds.len()
        ));
    }
    if !songs.iter().any(|s| s.learn) {
        return Err("no piece read may be learned from: keyfit learns only from pieces whose licence is MIT, CC0 or \
                    CC BY, which each converter writes (files converted before that have none: convert them again)"
            .into());
    }
    if !songs.iter().any(|s| s.learn && s.weight.iter().any(|w| *w > 0.0)) {
        return Err("every source learned from is weighed 0 (--weigh), so there's nothing to fit the weights on".into());
    }
    Ok(())
}

/// A song's notes as each kind of input hears them, each list in order of start.
struct Voices {
    tune: Vec<Note>,
    /// The parts under the tune, by role: a dataset's bass part is heard as a bass part.
    others: Vec<(Role, Vec<Note>)>,
    /// The parts under the tune split into the bass line and the rest, as if they were one piano part.
    upper: Vec<Note>,
    bass: Vec<Note>,
    all: Vec<Note>,
}

impl Voices {
    fn of(item: &Item) -> Voices {
        let tune = item.notes_of(|r| r == "melody");
        let mut others: Vec<(Role, Vec<Note>)> = Vec::new();
        for part in item.parts.iter().filter(|p| p.role != "melody" && p.role != "drums") {
            let role = keys::role(&part.role);
            let notes = part.notes.iter().map(|&(start, length, key, velocity)| Note { start, length, key, velocity });
            match others.iter_mut().find(|(r, _)| *r == role) {
                Some((_, list)) => list.extend(notes),
                None => others.push((role, notes.collect())),
            }
        }
        for (_, list) in &mut others {
            list.sort_by_key(|n| (n.start, n.key));
        }
        let piano = item.notes_of(|r| r != "melody" && r != "drums");
        let (bass, upper) = split_bass(&piano);
        let all = item.notes_of(|r| r != "drums");
        Voices { tune, others, upper, bass, all }
    }

    /// The parts of one kind of input, cut to a stretch (moved to start at 0) when one is given.
    fn parts(&self, input: Input, cut: Option<(i64, i64)>) -> Vec<(Role, Vec<Note>)> {
        let others = self.others.iter().map(|(role, notes)| (*role, notes));
        let lists: Vec<(Role, &Vec<Note>)> = match input {
            Input::Full => std::iter::once((Role::Melody, &self.tune)).chain(others).collect(),
            Input::Tune => vec![(Role::Melody, &self.tune)],
            Input::Upper => vec![(Role::Harmony, &self.upper)],
            Input::Bass => vec![(Role::Bass, &self.bass)],
            Input::Piano => others.collect(),
        };
        lists
            .into_iter()
            .map(|(role, notes)| {
                (role, cut.map_or_else(|| notes.clone(), |(from, length)| item::cut(notes, from, length)))
            })
            .filter(|(_, notes)| !notes.is_empty())
            .collect()
    }

    /// How many notes start in a stretch, in every part.
    fn count(&self, from: i64, length: i64) -> usize {
        self.all.partition_point(|n| n.start < from + length) - self.all.partition_point(|n| n.start < from)
    }
}

/// The piano's bass line and the rest: a note is in the bass line when, as it starts, it's the lowest note the piano
/// sounds and nothing else sounds within a fourth above it (the key finder's own test for a bass). `notes` must be in
/// order of start.
pub(crate) fn split_bass(notes: &[Note]) -> (Vec<Note>, Vec<Note>) {
    let (mut bass, mut upper) = (Vec::new(), Vec::new());
    let mut sounding: Vec<Note> = Vec::new();
    let mut i = 0;
    while let Some(first) = notes.get(i) {
        let at = first.start;
        let together = notes.get(i..).map_or(0, |rest| rest.partition_point(|n| n.start == at)).max(1);
        let group = notes.get(i..i + together).unwrap_or_default();
        sounding.retain(|n| n.start + n.length.max(1) > at);
        for n in group {
            let low = u16::from(n.key);
            let alone = sounding.iter().chain(group).all(|m| {
                let k = u16::from(m.key);
                k == low || k >= low + 5
            });
            if alone {
                bass.push(*n);
            } else {
                upper.push(*n);
            }
        }
        sounding.extend_from_slice(group);
        i += together;
    }
    (bass, upper)
}

/// What one case gave: the finder's evidence, and the C# app's key and confidence.
type Heard = (Evidence, Option<(Key, f64)>);

/// Hears one case's parts and asks the C# app too.
fn hear(reader: &mut KeyReader, parts: &[(Role, Vec<Note>)], grid: Grid, ppq: u32, span: Span) -> Option<Heard> {
    let notes: Vec<Vec<mo_harmony::Note>> = parts.iter().map(|(_, notes)| keys::harmony_notes(notes)).collect();
    let heard_parts: Vec<mo_harmony::Part> =
        parts.iter().zip(&notes).map(|((role, _), notes)| mo_harmony::Part { role: *role, notes }).collect();
    let heard = reader.hear(&heard_parts, grid)?;
    let mut all: Vec<Note> = parts.iter().flat_map(|(_, n)| n.iter().copied()).collect();
    all.sort_by_key(|n| (n.start, n.key));
    let cs = baseline::key(&all, 4 * i64::from(ppq));
    Some((Evidence::of(&heard, span), cs))
}

/// Whether every key region a stretch touches is labelled `truth`.
fn one_key_in(item: &Item, from: i64, to: i64, truth: Key) -> bool {
    item.keys.iter().filter(|(s, e, _)| *s < to && *e > from).all(|(_, _, k)| Key::parse(k) == Some(truth))
}

/// What to cut from each song, and how each cut is heard.
struct Cutting<'a> {
    lengths: &'a [i64],
    inputs: &'a [Input],
    picks: usize,
}

/// Every case from one song.
fn song_cases(item: &Item, song: usize, name: &str, cutting: &Cutting) -> Vec<Case> {
    let mut reader = KeyReader::new();
    let voices = Voices::of(item);
    // A piece with no tune (a chord chart) has nothing to hear as the tune, and its piano is its full mix.
    let inputs: Vec<Input> = cutting
        .inputs
        .iter()
        .copied()
        .filter(|i| !voices.tune.is_empty() || !matches!(i, Input::Tune | Input::Piano))
        .collect();
    let mut out = Vec::new();
    let mut add = |cut: Cut, stretch: Option<(i64, i64)>, truth: Key, one_key: bool, pick: bool| {
        for &input in &inputs {
            let grid = keys::grid_of(item, stretch.is_some());
            if let Some((ev, cs)) = hear(&mut reader, &voices.parts(input, stretch), grid, item.ppq, cut.span()) {
                out.push(Case { song, truth, cut, input, one_key, pick, ev, cs });
            }
        }
    };
    if let Some(truth) = item.main_key().filter(|_| voices.all.len() >= MIN_NOTES) {
        let whole = match item.whole {
            Whole::Song => Cut::Song,
            Whole::Loop => Cut::Loop,
            Whole::Window => Cut::Piece,
        };
        add(whole, None, truth, item.one_key(), true);
    }
    let (bar, origin) = (item.bar(), item.grid_origin());
    // The first bar with notes in it, where the openings start.
    let opening = voices.all.iter().find(|n| n.start >= origin).map(|n| origin + (n.start - origin) / bar * bar);
    for &bars in cutting.lengths {
        let length = bars * bar;
        let mut windows = Vec::new();
        let mut at = origin;
        while at + length <= item.length {
            if let Some(truth) = item.key_at(at + length / 2).filter(|_| voices.count(at, length) >= MIN_NOTES) {
                windows.push((at, truth, one_key_in(item, at, at + length, truth)));
            }
            at += length;
        }
        // The windows fitted on: a few in one key, picked by a hash of the song and the bar.
        let mut ranked: Vec<(u64, i64)> = windows
            .iter()
            .filter(|w| w.2)
            .map(|w| (fnv1a(format!("{name}/{bars}/{}", (w.0 - origin) / bar).as_bytes()), w.0))
            .collect();
        ranked.sort_unstable();
        let picked: HashSet<i64> = ranked.iter().take(cutting.picks).map(|r| r.1).collect();
        for (at, truth, one_key) in windows {
            add(Cut::Window(bars), Some((at, length)), truth, one_key, picked.contains(&at));
        }
        // A piece cut from a longer part doesn't start where the music does, so its first bars aren't a loop.
        if let Some(at) = opening.filter(|at| at + length <= item.length && item.whole != Whole::Window)
            && let Some(truth) = item.key_at(at + length / 2).filter(|_| voices.count(at, length) >= MIN_NOTES)
        {
            let one_key = one_key_in(item, at, at + length, truth);
            add(Cut::Opening(bars), Some((at, length)), truth, one_key, true);
        }
    }
    out
}

/// The counted tables: for each stream and mode, the log of the average share of each note counted up from the
/// labelled tonic.
struct Tables([[[f64; 12]; 2]; STREAMS]);

impl Tables {
    /// Counts the tables from the songs `keep` takes: the note and bass streams and a song's ends from whole songs in
    /// one key, a loop's start from the shortest openings in one key.
    fn count(cases: &[Case], loop_cut: Cut, keep: impl Fn(usize) -> bool) -> Tables {
        let mut sum = [[[0.0f64; 12]; 2]; STREAMS];
        let mut songs = [[0.0f64; 2]; STREAMS];
        for c in cases.iter().filter(|c| c.input == Input::Full && c.one_key && keep(c.song)) {
            let streams: &[Stream] = match c.cut {
                Cut::Song => &[
                    Stream::Tune,
                    Stream::Harmony,
                    Stream::Bass,
                    Stream::MajorChords,
                    Stream::MinorChords,
                    Stream::FirstBassSong,
                    Stream::LastBassSong,
                ],
                cut if cut == loop_cut => &[Stream::FirstBassLoop],
                _ => &[],
            };
            let m = usize::from(c.truth.is_minor());
            let tonic = c.truth.tonic.index();
            for &s in streams {
                let row = &c.ev.x[s as usize];
                let total: f64 = row.iter().sum();
                if total <= 0.0 {
                    continue;
                }
                for (d, cell) in sum[s as usize][m].iter_mut().enumerate() {
                    *cell += row[(tonic + d) % 12] / total;
                }
                songs[s as usize][m] += 1.0;
            }
        }
        Tables(std::array::from_fn(|s| {
            std::array::from_fn(|m| std::array::from_fn(|d| ((sum[s][m][d] + 2.0 / 12.0) / (songs[s][m] + 2.0)).ln()))
        }))
    }

    /// The built-in finder's weights as tables, each stream's trust and corrections already in them: what
    /// `--rescale` scales, one number a stream.
    fn shipped() -> Tables {
        let w = keyfind::SHIPPED.weights;
        Tables(std::array::from_fn(|s| {
            std::array::from_fn(|m| std::array::from_fn(|d| w.get(s * 24 + m * 12 + d).copied().unwrap_or(0.0)))
        }))
    }
}

/// One case as the fitter sees it: a trust weight for each stream (its table times its features), the minor weight,
/// then a correction for each feature when they're fitted. In a rescale ([`Rescaling`]) the numbers fitted are how far
/// each stream's scale and the minor weight move from the built-in ones, so the penalty holds them there, and each
/// key's score starts from the built-in finder's.
struct Fitting<'a> {
    case: &'a Case,
    tables: &'a Tables,
    weight: f64,
    corrections: bool,
    rescaling: Option<&'a Rescaling>,
    /// Each key's score before the numbers fitted: the built-in finder's in a rescale, 0 otherwise. Worked out once.
    offsets: [f64; 24],
}

/// What a rescale fits: the scales of the streams, and the minor weight, that enough of its pieces have
/// ([`SUPPORT`]). The rest stay as built in.
struct Rescaling {
    streams: [bool; STREAMS],
    minor: bool,
}

impl<'a> Fitting<'a> {
    fn new(
        case: &'a Case,
        tables: &'a Tables,
        weight: f64,
        corrections: bool,
        rescaling: Option<&'a Rescaling>,
    ) -> Self {
        let offsets = match rescaling {
            Some(_) => Model { weights: *keyfind::SHIPPED.weights }.scores(&case.ev),
            None => [0.0; 24],
        };
        Fitting { case, tables, weight, corrections, rescaling, offsets }
    }

    /// Each stream's table times its features for key `k`; `each` is given every feature for the corrections.
    fn trust(&self, k: usize, each: &mut dyn FnMut(usize, f64)) -> [f64; STREAMS] {
        let mut trust = [0.0; STREAMS];
        let Some(key) = Key::ALL.get(k) else { return trust };
        self.case.ev.features(*key, &mut |i, v| {
            if i < MINOR {
                let (s, m, d) = (i / 24, i % 24 / 12, i % 12);
                trust[s] += self.tables.0[s][m][d] * v;
            }
            each(i, v);
        });
        trust
    }
}

impl Choice for Fitting<'_> {
    fn candidates(&self) -> usize {
        24
    }

    fn truth(&self) -> usize {
        self.case.truth.index()
    }

    fn weight(&self) -> f64 {
        self.weight
    }

    fn features(&self, k: usize, each: &mut dyn FnMut(usize, f64)) {
        let streams = self.rescaling.map_or([true; STREAMS], |r| r.streams);
        let minor = self.rescaling.is_none_or(|r| r.minor);
        let trust = self.trust(k, &mut |i, v| {
            if i >= MINOR {
                if minor {
                    each(STREAMS, v);
                }
            } else if self.corrections {
                each(STREAMS + 1 + i, v);
            }
        });
        for (s, (t, fitted)) in trust.iter().zip(streams).enumerate() {
            if *t != 0.0 && fitted {
                each(s, *t);
            }
        }
    }

    fn offset(&self, k: usize) -> f64 {
        self.offsets.get(k).copied().unwrap_or(0.0)
    }
}

/// A fitted finder: one weight per feature, as `weights.rs` keeps them.
struct Model {
    weights: [f64; FEATURES],
}

impl Model {
    /// The weights from the fitted numbers: each stream's trust times its table, plus the corrections.
    fn of(fitted: &[f64], tables: &Tables, corrections: bool) -> Model {
        let at = |i: usize| fitted.get(i).copied().unwrap_or(0.0);
        let weights = std::array::from_fn(|i| {
            if i >= MINOR {
                return at(STREAMS);
            }
            let (s, m, d) = (i / 24, i % 24 / 12, i % 12);
            at(s) * tables.0[s][m][d] + if corrections { at(STREAMS + 1 + i) } else { 0.0 }
        });
        Model { weights }
    }

    /// Every key's score, as `keyfind::score` works it out.
    fn scores(&self, ev: &Evidence) -> [f64; 24] {
        Key::ALL.map(|k| {
            let mut s = 0.0;
            ev.features(k, &mut |i, v| s += v * self.weights.get(i).copied().unwrap_or(0.0));
            s
        })
    }
}

/// What a run is told on the command line.
struct Options {
    /// The penalty on the corrections; `None` fits the tables' trust weights alone.
    lambda_delta: Option<f64>,
    /// The shortest opening, whose first bass the loop table is counted from.
    loop_cut: Cut,
    /// `--rescale`: only the built-in finder's stream scales, minor weight and calibration, on Solomon's own pieces.
    rescale: bool,
}

/// How much each set of cases counts in a fit or a calibration, in total.
fn set_weight(cut: Cut, input: Input) -> f64 {
    if matches!(cut, Cut::Song | Cut::Loop) && input == Input::Full { SONG_WEIGHT } else { 1.0 }
}

/// Each case's share of its set's weight, over the cases `keep` takes (0 for the rest). Each source has sets of its
/// own, which count as much as its songs' weight says (`--weigh`); a song only measured, weighing 0, takes no share.
/// Songs with the same notes in other keys split one song's share ([`Song::copies`]).
fn case_weights(cases: &[Case], songs: &[Song], keep: impl Fn(&Case) -> bool) -> Vec<f64> {
    let song = |c: &Case| songs.get(c.song).map_or((0, 0.0, 1.0), |s| (s.group, s.weight(c.input), s.share()));
    let mut sizes: HashMap<(usize, Cut, Input), f64> = HashMap::new();
    for c in cases.iter().filter(|c| keep(c) && song(c).1 > 0.0) {
        *sizes.entry((song(c).0, c.cut, c.input)).or_default() += song(c).2;
    }
    cases
        .iter()
        .map(|c| {
            let (group, weight, share) = song(c);
            if keep(c) && weight > 0.0 {
                let size = sizes.get(&(group, c.cut, c.input)).copied().unwrap_or(1.0);
                if size > 0.0 { weight * set_weight(c.cut, c.input) * share / size } else { 0.0 }
            } else {
                0.0
            }
        })
        .collect()
}

/// Which songs a fit learns from.
type Training<'a> = &'a (dyn Fn(usize) -> bool + Sync);

/// Fits a finder on the songs `train` takes: the tables from all of them, and the weights with each song's features
/// made from tables counted without its inner fold. Also gives the fit and how many cases it was fitted on.
fn fit_model(
    cases: &[Case],
    songs: &[Song],
    train: Training,
    opts: &Options,
) -> Result<(Model, fit::Fitted, usize), String> {
    if opts.rescale {
        return rescale(cases, songs, train);
    }
    let corrections = opts.lambda_delta.is_some();
    let all = Tables::count(cases, opts.loop_cut, train);
    let inner: Vec<Tables> = (0..INNER as usize)
        .map(|i| Tables::count(cases, opts.loop_cut, |s| train(s) && songs.get(s).is_some_and(|g| g.inner != i)))
        .collect();
    let fitted_on = |c: &Case| train(c.song) && c.one_key && c.pick;
    let weights = case_weights(cases, songs, fitted_on);
    let choices: Vec<Fitting> = cases
        .iter()
        .zip(&weights)
        .filter(|(c, w)| fitted_on(c) && **w > 0.0)
        .filter_map(|(case, &weight)| {
            let tables = inner.get(songs.get(case.song)?.inner)?;
            Some(Fitting::new(case, tables, weight, corrections, None))
        })
        .collect();
    let mut l2 = vec![LAMBDA_W; STREAMS + 1];
    if let Some(ld) = opts.lambda_delta {
        l2.extend(std::iter::repeat_n(ld, MINOR));
    }
    let fitted = fit::fit(&choices, l2.len(), &l2)?;
    Ok((Model::of(&fitted.weights, &all, corrections), fitted, choices.len()))
}

/// `--rescale`'s fit: the built-in finder's weights for each stream scaled by one number, and the minor weight, fitted
/// on the songs `train` takes, each held near its built-in value by the penalty. A stream fewer than [`SUPPORT`] of
/// them sound keeps a scale of 1: left free, a few pieces could push it anywhere. The minor weight is kept as built in
/// unless at least [`SUPPORT`] of them are labelled major and as many minor: pieces nearly all in one mode would teach
/// the finder to name that mode whatever it hears. Songs with the same notes in other keys count once.
fn rescale(cases: &[Case], songs: &[Song], train: Training) -> Result<(Model, fit::Fitted, usize), String> {
    let shipped = Tables::shipped();
    let fitted_on = |c: &Case| train(c.song) && c.one_key && c.pick;
    let weights = case_weights(cases, songs, fitted_on);
    let picked: Vec<(&Case, f64)> =
        cases.iter().zip(&weights).filter(|(c, w)| fitted_on(c) && **w > 0.0).map(|(c, w)| (c, *w)).collect();
    let music = |c: &Case| songs.get(c.song).map_or(c.song, |s| s.music);
    let mut heard_in: [HashSet<usize>; STREAMS] = std::array::from_fn(|_| HashSet::new());
    let mut in_mode: [HashSet<usize>; 2] = std::array::from_fn(|_| HashSet::new());
    for (c, _) in &picked {
        for (row, pieces) in c.ev.x.iter().zip(heard_in.iter_mut()) {
            if row.iter().any(|v| *v != 0.0) {
                pieces.insert(music(c));
            }
        }
        if let Some(pieces) = in_mode.get_mut(usize::from(c.truth.is_minor())) {
            pieces.insert(music(c));
        }
    }
    let rescaling = Rescaling {
        streams: std::array::from_fn(|s| heard_in.get(s).is_some_and(|pieces| pieces.len() >= SUPPORT)),
        minor: in_mode.iter().all(|pieces| pieces.len() >= SUPPORT),
    };
    let choices: Vec<Fitting> = picked
        .into_iter()
        .map(|(case, weight)| Fitting::new(case, &shipped, weight, false, Some(&rescaling)))
        .collect();
    let fitted = fit::fit(&choices, STREAMS + 1, &[LAMBDA_W; STREAMS + 1])?;
    // The numbers as `Model::of` takes them: each stream's scale, then the minor weight.
    let moved = |i: usize, free: bool| if free { fitted.weights.get(i).copied().unwrap_or(0.0) } else { 0.0 };
    let mut numbers: Vec<f64> = rescaling.streams.iter().enumerate().map(|(s, free)| 1.0 + moved(s, *free)).collect();
    numbers.push(keyfind::SHIPPED.weights.get(MINOR).copied().unwrap_or(0.0) + moved(STREAMS, rescaling.minor));
    Ok((Model::of(&numbers, &shipped, false), fitted, choices.len()))
}

/// Each stream's scale in a rescaled finder: its weights over the built-in ones (1 where they're all 0).
fn scales_of(model: &Model) -> [f64; STREAMS] {
    std::array::from_fn(|s| {
        (0..24)
            .find_map(|i| {
                let was = keyfind::SHIPPED.weights.get(s * 24 + i).copied().unwrap_or(0.0);
                (was != 0.0).then(|| model.weights.get(s * 24 + i).copied().unwrap_or(0.0) / was)
            })
            .unwrap_or(1.0)
    })
}

/// The stream scales, as a run and the weights file print them: "Tune 1.020, Harmony 0.970, …; minor weight -0.120
/// (as built in)".
fn scales_line(model: &Model) -> String {
    let scales = scales_of(model);
    let each: Vec<String> = Stream::ALL.iter().zip(scales).map(|(s, v)| format!("{s:?} {v:.3}")).collect();
    let minor = model.weights.get(MINOR).copied().unwrap_or(0.0);
    let kept = if Some(minor) == keyfind::SHIPPED.weights.get(MINOR).copied() { " (as built in)" } else { "" };
    format!("{}; minor weight {minor:.3}{kept}", each.join(", "))
}

/// The average surprise (negative log-likelihood) of the labelled keys under scores, weighted.
fn weighted_surprise(cases: &[Case], scores: &[Option<[f64; 24]>], weights: &[f64]) -> f64 {
    let (mut total, mut loss) = (0.0, 0.0);
    for ((c, s), w) in cases.iter().zip(scores).zip(weights) {
        if let Some(s) = s
            && *w > 0.0
        {
            total += w;
            loss += w * fit::surprise(s, c.truth.index());
        }
    }
    if total > 0.0 { loss / total } else { 0.0 }
}

/// Scores for the cases of some folds, each from a finder fitted on those folds without its own (`None` for the rest).
type Held = Vec<Option<[f64; 24]>>;

/// Each case's scores from a finder fitted without its fold, for the cases in `folds` (fitting on those folds only).
fn cross_validate(
    cases: &[Case],
    songs: &[Song],
    folds: &[usize],
    opts: &Options,
) -> Result<(Held, Vec<(usize, Model)>), String> {
    let fold = |s: usize| songs.get(s).map_or(usize::MAX, |g| g.fold);
    let learn = |s: usize| songs.get(s).is_some_and(|g| g.learn);
    let mut held: Held = vec![None; cases.len()];
    let mut models = Vec::with_capacity(folds.len());
    for &f in folds {
        let train = |s: usize| fold(s) != f && folds.contains(&fold(s)) && learn(s);
        let (model, fitted, n) = fit_model(cases, songs, &train, opts)?;
        held.par_iter_mut()
            .zip(cases)
            .filter(|(_, c)| fold(c.song) == f)
            .for_each(|(h, c)| *h = Some(model.scores(&c.ev)));
        let weights = case_weights(cases, songs, |c| fold(c.song) == f && c.one_key && c.pick);
        println!(
            "  fold {f}: fitted on {n} cases, {} Newton steps; loss {:.4} fitting, {:.4} on the fold held out",
            fitted.steps,
            fitted.loss,
            weighted_surprise(cases, &held, &weights)
        );
        if opts.rescale {
            println!("    scales: {}", scales_line(&model));
        }
        models.push((f, model));
    }
    Ok((held, models))
}

/// The calibration fitted on held-out scores of the one-key cases `keep` takes, each set counting as in the fit. With
/// `--rescale`, a number fewer than [`SUPPORT`] of their pieces have (a song's, for loops alone) is held at its
/// built-in one. A piece counts by the most any of its cases has of the number: one whose longest stretch is 12 bars
/// is part 8 bars, part 16, so it counts as a part of a piece for each. The curve over bars counts only full mixes as
/// having it: every other kind of input adds its own number to the curve, so pieces that are all one part alone (a
/// pack's) can't tell the curve from those numbers, and would move it for full mixes too.
fn calibrate(
    cases: &[Case],
    songs: &[Song],
    held: &[Option<[f64; 24]>],
    keep: impl Fn(&Case) -> bool,
    opts: &Options,
) -> Result<Calibration, String> {
    let weights = case_weights(cases, songs, |c| keep(c) && c.one_key);
    let taken: Vec<(&Case, &[f64; 24], f64)> = cases
        .iter()
        .zip(held)
        .zip(&weights)
        .filter(|(_, w)| **w > 0.0)
        .filter_map(|((c, s), w)| Some((c, s.as_ref()?, *w)))
        .collect();
    let terms_of = |c: &Case| Calibration::terms(c.ev.bars, c.ev.config, c.cut.span());
    let built_in = keyfind::SHIPPED.calibration.0;
    let mut kept = [false; CALIBRATION_TERMS];
    if opts.rescale {
        // Songs with the same notes in other keys count once.
        let music = |c: &Case| songs.get(c.song).map_or(c.song, |s| s.music);
        let mut had: [HashMap<usize, f64>; CALIBRATION_TERMS] = std::array::from_fn(|_| HashMap::new());
        for (c, _, _) in &taken {
            let full = c.ev.config == Config::Full;
            for (t, v) in terms_of(c).iter().enumerate() {
                if *v != 0.0
                    && (full || t >= keyfind::KNOTS)
                    && let Some(pieces) = had.get_mut(t)
                {
                    let most = pieces.entry(music(c)).or_insert(0.0);
                    *most = most.max(v.abs());
                }
            }
        }
        kept = std::array::from_fn(|t| had.get(t).is_none_or(|pieces| pieces.values().sum::<f64>() < SUPPORT as f64));
    }
    let scored: Vec<Scored> = taken
        .iter()
        .map(|(c, s, w)| {
            let terms = terms_of(c);
            let fixed = terms.iter().zip(kept).zip(built_in).filter(|((_, k), _)| *k).map(|((t, _), b)| t * b).sum();
            Scored {
                scores: s.to_vec(),
                truth: c.truth.index(),
                weight: *w,
                terms: terms.iter().zip(kept).map(|(t, k)| if k { 0.0 } else { *t }).collect(),
                fixed,
            }
        })
        .collect();
    let numbers = fit::calibration(&scored, CALIBRATION_TERMS)?;
    let mut c = [0.0; CALIBRATION_TERMS];
    for ((slot, v), (k, b)) in c.iter_mut().zip(numbers).zip(kept.into_iter().zip(built_in)) {
        *slot = if k { b } else { v };
    }
    Ok(Calibration(c))
}

/// The chances a calibration gives a case's scores.
fn chances_of(case: &Case, scores: &[f64; 24], calibration: &Calibration) -> [f64; 24] {
    let sharp = calibration.sharpness(case.ev.bars, case.ev.config, case.cut.span());
    keyfind::chances(scores.map(|s| s * sharp))
}

fn print_calibration(title: &str, c: &Calibration) {
    let n = |i: usize| c.0.get(i).copied().unwrap_or(0.0);
    println!(
        "{title}: at 1, 2, 4, 8, 16 bars {}; melody only {:+.3}, bass only {:+.3}, accompaniment {:+.3}; a loop adds \
         {}; a song adds {:+.3}.",
        (0..keyfind::KNOTS).map(|i| format!("{:.3}", n(i))).collect::<Vec<_>>().join(", "),
        n(keyfind::KNOTS),
        n(keyfind::KNOTS + 1),
        n(keyfind::KNOTS + 2),
        (0..keyfind::KNOTS).map(|i| format!("{:+.3}", n(keyfind::KNOTS + 3 + i))).collect::<Vec<_>>().join(", "),
        n(2 * keyfind::KNOTS + 3),
    );
}

/// The one-key cases `keep` takes that have chances, as the answers are fitted on them.
fn graded_cases(cases: &[Case], chances: &[Option<[f64; 24]>], keep: impl Fn(&Case) -> bool) -> Vec<answerfit::Graded> {
    cases
        .iter()
        .zip(chances)
        .filter(|(c, _)| keep(c) && c.one_key)
        .filter_map(|(c, chances)| {
            Some(answerfit::Graded {
                song: c.song,
                truth: c.truth,
                knot: answerfit::knot_of(c.ev.bars, c.cut == Cut::Song),
                notes: c.ev.notes,
                diatonic_fit: c.ev.diatonic_fit,
                chances: (*chances)?,
            })
        })
        .collect()
}

/// The thresholds and the share of notes in one scale fitted on the chances of the one-key cases `keep` takes; with
/// `--rescale`, the built-in ones, and how they come out on those cases.
fn fit_answers(cases: &[Case], chances: &[Option<[f64; 24]>], keep: impl Fn(&Case) -> bool, opts: &Options) -> Fitted {
    let graded = graded_cases(cases, chances, keep);
    if opts.rescale {
        answerfit::kept(&graded, keyfind::SHIPPED.thresholds, keyfind::SHIPPED.not_tonal_below)
    } else {
        answerfit::fit(&graded, keyfind::SHIPPED.not_tonal_below)
    }
}

fn print_answers(title: &str, fitted: &Fitted) {
    println!("{title}:");
    println!("  {}", fitted.scale_line);
    for line in &fitted.knot_lines {
        println!("    {line}");
    }
}

/// A finder with these weights, this calibration and the answers fitted.
fn finder<'a>(weights: &'a [f64; FEATURES], calibration: &'a Calibration, answers: &'a Fitted) -> keyfind::Model<'a> {
    keyfind::Model { weights, calibration, thresholds: &answers.thresholds, not_tonal_below: answers.not_tonal_below }
}

/// Each case's answer as the app would show it, with the thresholds `answers_of` gives its case.
fn answers_of<'a>(
    cases: &[Case],
    chances: &[Option<[f64; 24]>],
    answers_of: impl Fn(&Case) -> Option<&'a Fitted> + Sync,
) -> Vec<Option<(KeyAnswer, f64)>> {
    cases
        .par_iter()
        .zip(chances)
        .map(|(c, chances)| Some(graded_answer(c, chances.as_ref()?, answers_of(c)?)))
        .collect()
}

/// For each fold, the thresholds fitted on the other folds' chances (of those `folds` lists), to grade it with.
fn fitted_per_fold(
    cases: &[Case],
    chances: &[Option<[f64; 24]>],
    folds: &[usize],
    fold: &(dyn Fn(&Case) -> usize + Sync),
    taught: &(dyn Fn(&Case) -> bool + Sync),
    opts: &Options,
) -> HashMap<usize, Fitted> {
    folds
        .iter()
        .map(|&f| (f, fit_answers(cases, chances, |c| fold(c) != f && folds.contains(&fold(c)) && taught(c), opts)))
        .collect()
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut setup = keys::setup(
        args,
        &["--lambda-delta", "--write", "--picks", "--inputs", "--dump", "--against", "--hand", "--weigh"],
        &["--final", "--rescale", "--shipped"],
        &LENGTHS,
    )?;
    if !matches!(setup.parts, keys::Parts::Pitched) {
        return Err("keyfit hears every kind of input itself (--inputs); --part is for `keys`".into());
    }
    let mut lambda_delta = Some(LAMBDA_DELTA);
    let mut lambda_given = false;
    let mut write: Option<PathBuf> = None;
    let mut picks = PICKS;
    let mut inputs = Input::ALL.to_vec();
    let mut dump: Option<PathBuf> = None;
    let mut before: Option<PathBuf> = None;
    let mut show: Vec<String> = Vec::new();
    let mut weigh: HashMap<String, f64> = HashMap::new();
    for pair in setup.rest.chunks(2) {
        match pair {
            [o, v] if o == "--lambda-delta" => {
                lambda_given = true;
                lambda_delta = if v == "inf" {
                    None
                } else {
                    let ld = v.parse().ok().filter(|&x: &f64| x > 0.0);
                    Some(ld.ok_or("--lambda-delta needs a number above 0, or inf")?)
                };
            }
            [o, v] if o == "--write" => write = Some(PathBuf::from(v)),
            [o, v] if o == "--picks" => picks = v.parse().map_err(|_| "--picks needs a whole number")?,
            [o, v] if o == "--dump" => dump = Some(PathBuf::from(v)),
            [o, v] if o == "--against" => before = Some(PathBuf::from(v)),
            [o, v] if o == "--hand" => show = v.split(',').map(|n| n.trim().to_owned()).collect(),
            [o, v] if o == "--weigh" => {
                let bad =
                    || format!("--weigh {v}: give a source (or source/input) and a weight of 0 or more: billboard=0.5");
                let (source, w) = v.split_once('=').ok_or_else(bad)?;
                let w = w.parse::<f64>().ok().filter(|w| w.is_finite() && *w >= 0.0).ok_or_else(bad)?;
                weigh.insert(source.trim().to_owned(), w);
            }
            [o, v] if o == "--inputs" => {
                inputs = v
                    .split(',')
                    .map(|i| {
                        Input::parse(i.trim()).ok_or_else(|| format!("--inputs: {i}? (full, tune, upper, bass, piano)"))
                    })
                    .collect::<Result<_, _>>()?;
                if !inputs.contains(&Input::Full) {
                    return Err("--inputs needs full: the tables are counted from the full mix".into());
                }
            }
            _ => return Err(crate::USAGE.to_owned()),
        }
    }
    let final_run = setup.flags.iter().any(|f| f == "--final");
    let rescale = setup.flags.iter().any(|f| f == "--rescale");
    let shipped_run = setup.flags.iter().any(|f| f == "--shipped");
    if final_run && write.is_some() {
        return Err("--final scores the kept-back fold; --write fits on every song. One at a time.".into());
    }
    if shipped_run && (final_run || write.is_some() || rescale) {
        return Err("--shipped scores the finder built in, as it is: not with --final, --write or --rescale".into());
    }
    if lambda_given && (rescale || shipped_run) {
        return Err(
            "--lambda-delta is for a plain fit: --rescale fits no corrections, and --shipped fits nothing".into()
        );
    }
    if (final_run || write.is_some()) && (dump.is_some() || before.is_some()) {
        return Err(
            "--dump and --against compare runs on folds 1 to 4, for choosing; not with --final or --write".into()
        );
    }
    if let Some(path) = &dump
        && setup.items.iter().any(Item::is_own)
    {
        crate::packs::outside_checkout("--dump", path, "the names of Solomon's files in it")?;
    }
    // Read before this run writes its own dump, which may go to the same file.
    let before = before
        .map(|path| {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok::<_, String>((path.display().to_string(), text))
        })
        .transpose()?;
    let mut lengths = setup.lengths.clone();
    lengths.sort_unstable();
    lengths.dedup();
    let loop_cut = Cut::Opening(lengths.first().copied().unwrap_or(1));
    let opts = Options { lambda_delta: if rescale { None } else { lambda_delta }, loop_cut, rescale };
    let learning = learning_of(rescale, shipped_run, &setup.items);

    // The sources in the order they were first read: the first file's comes first in the report, as before.
    let mut sources: Vec<String> = Vec::new();
    for item in &setup.items {
        if !sources.contains(&item.source) {
            sources.push(item.source.clone());
        }
    }
    for named in weigh.keys() {
        let (source, input) = named.split_once('/').map_or((named.as_str(), None), |(s, i)| (s, Some(i)));
        if !sources.iter().any(|s| s == source) {
            return Err(format!("--weigh {named}: no piece read comes from {source}"));
        }
        if let Some(input) = input.filter(|i| Input::parse(i).is_none()) {
            return Err(format!("--weigh {named}: {input}? (full, tune, upper, bass, piano)"));
        }
    }
    let left_out = leave_out_measured_twins(&mut setup.items, learning);
    let songs = songs_of(&setup.items, &sources, &weigh, learning);
    // How each source was used, for the run log and the weights file ("billboard at 0.25", "rs200 measured only").
    let mut used: Vec<String> = Vec::new();
    // Each source learned from, for the weights file.
    let mut learned_from: Vec<Learned> = Vec::new();
    for (g, source) in sources.iter().enumerate() {
        let of: Vec<&Song> = songs.iter().filter(|s| s.group == g).collect();
        let learned = of.iter().filter(|s| s.learn).count();
        // A source's songs learned from all weigh the same (`--weigh` names sources), so the first one says.
        let weights = of.iter().find(|s| s.learn).map_or([0.0; 5], |s| s.weight);
        let own = setup.items.iter().any(|i| i.source == *source && i.is_own());
        let mut how = if shipped_run {
            used.push(source.clone());
            "scored by the finder built in (nothing is fitted)".to_owned()
        } else if learned == 0 {
            used.push(format!("{source} measured only"));
            match (rescale, own) {
                (_, true) => "measured only (Solomon's own: only --rescale learns from it)",
                (true, false) => "measured only (--rescale learns from Solomon's own pieces alone)",
                (false, false) => "measured only (its licence doesn't let fitted weights ship)",
            }
            .to_owned()
        } else if weights.iter().all(|w| *w == 0.0) {
            used.push(format!("{source} in the tables only"));
            "counted in the tables only".to_owned()
        } else {
            let counting = weighing(weights);
            used.push(if counting == "1" { source.clone() } else { format!("{source} at {counting}") });
            format!("learned from, its sets counting {counting}")
        };
        if learned > 0 {
            let licence =
                setup.items.iter().find(|i| i.source == *source && learns(i, learning)).map(|i| i.licence.as_str());
            learned_from.push(Learned {
                name: source.clone(),
                how: used.last().cloned().unwrap_or_default(),
                credit: credit(source, licence.unwrap_or_default()),
            });
        }
        if learned > 0 && learned < of.len() && !shipped_run {
            let _ = write!(how, "; {} of them only measured (their licence doesn't)", of.len() - learned);
        }
        if let Some(n) = left_out.get(source) {
            let _ = write!(how, "; {n} more left out, their notes being in a piece learned from");
        }
        println!("{source}: {} songs, {how}.", of.len());
    }
    if !shipped_run {
        check_learning(&songs, rescale)?;
    }
    if write.is_some() && !songs.iter().any(|s| s.group == 0 && s.learn) {
        return Err(format!(
            "--write: the weights file says how often the first source's songs were right, so read one learned from \
             first, not {}",
            sources.first().map_or("", String::as_str)
        ));
    }
    let cutting = Cutting { lengths: &lengths, inputs: &inputs, picks };
    let cases: Vec<Case> = setup
        .items
        .par_iter()
        .zip(&songs)
        .enumerate()
        .flat_map_iter(|(i, (item, song))| song_cases(item, i, &song.name, &cutting))
        .collect();
    let per_fold: Vec<usize> = (0..FOLDS as usize).map(|f| songs.iter().filter(|s| s.fold == f).count()).collect();
    println!(
        "{} cases ({} kinds of input); songs in folds 0 to 4: {}. Fold {FROZEN} is kept back for --final.",
        cases.len(),
        inputs.len(),
        per_fold.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")
    );
    println!();
    match opts.lambda_delta {
        _ if shipped_run => println!(
            "Scoring the finder built in (version {KEY_FINDER_VERSION}), as it is: nothing fitted, its own calibration \
             and thresholds."
        ),
        _ if rescale => println!(
            "Rescaling the finder built in (version {KEY_FINDER_VERSION}): one scale for each of its {STREAMS} streams \
             and the minor weight (penalty {LAMBDA_W}), then the calibration; its thresholds kept."
        ),
        Some(ld) => println!(
            "Fitting {STREAMS} trust weights and the minor weight (penalty {LAMBDA_W}) and {MINOR} corrections \
             (penalty {ld})."
        ),
        None => println!("Fitting {STREAMS} trust weights and the minor weight (penalty {LAMBDA_W}); no corrections."),
    }

    let fold = |c: &Case| songs.get(c.song).map_or(usize::MAX, |s| s.fold);
    let group = |c: &Case| songs.get(c.song).map_or(usize::MAX, |s| s.group);
    let learn = |s: usize| songs.get(s).is_some_and(|g| g.learn);
    // The cases the calibration and the answers' thresholds are fitted on: those the fit learns from.
    let taught = |c: &Case| songs.get(c.song).is_some_and(|s| s.learn && s.weight(c.input) > 0.0);
    let design: Vec<usize> = (0..FOLDS as usize).filter(|&f| f != FROZEN).collect();
    let every: Vec<usize> = (0..FOLDS as usize).collect();

    // How the run is logged in test-runs.md: a rescale says so.
    let label = |run: &str| format!("{}{run} ({})", if rescale { "--rescale " } else { "" }, used.join("; "));
    let in_design = |c: &Case| design.contains(&fold(c));

    if shipped_run {
        println!("Folds 1 to 4 (fold {FROZEN} isn't touched), scored by the finder built in:");
        let model = Model { weights: *keyfind::SHIPPED.weights };
        let calibration = *keyfind::SHIPPED.calibration;
        let chances: Held =
            cases.par_iter().map(|c| in_design(c).then(|| chances_of(c, &model.scores(&c.ev), &calibration))).collect();
        let built_in = answerfit::kept(
            &graded_cases(&cases, &chances, in_design),
            keyfind::SHIPPED.thresholds,
            keyfind::SHIPPED.not_tonal_below,
        );
        print_answers("Answers (the thresholds built in, as they come out on these cases)", &built_in);
        let answers = answers_of(&cases, &chances, |_| Some(&built_in));
        report(&cases, &chances, &answers, &songs, |c| in_design(c) && group(c) == 0, &inputs);
        for (g, source) in sources.iter().enumerate().skip(1) {
            source_report(source, &cases, &chances, |c| in_design(c) && group(c) == g, &inputs);
        }
        compare_and_dump(&cases, &chances, &songs, &sources, &in_design, &inputs, before.as_ref(), dump.as_ref())?;
        hand_cases(&[("built in".to_owned(), keyfind::SHIPPED)], &show)?;
    } else if final_run {
        println!("Choosing the calibration and the answers' thresholds on folds 1 to 4:");
        let (held, _) = cross_validate(&cases, &songs, &design, &opts)?;
        let calibration = calibrate(&cases, &songs, &held, in_design, &opts)?;
        print_calibration("Calibration", &calibration);
        let design_chances: Held =
            cases.par_iter().zip(&held).map(|(c, h)| h.as_ref().map(|s| chances_of(c, s, &calibration))).collect();
        let fitted_answers = fit_answers(&cases, &design_chances, |c| in_design(c) && taught(c), &opts);
        print_answers("Answers", &fitted_answers);
        let train = |s: usize| songs.get(s).is_some_and(|g| g.fold != FROZEN && g.learn);
        let (model, fitted, n) = fit_model(&cases, &songs, &train, &opts)?;
        let measured = songs.iter().filter(|s| !s.learn).count();
        println!(
            "Fitted on folds 1 to 4: {n} cases, {} Newton steps. Scoring fold {FROZEN}, kept back until now{}.",
            fitted.steps,
            if measured > 0 { format!(", and the {measured} songs only measured") } else { String::new() }
        );
        if rescale {
            println!("Scales: {}.", scales_line(&model));
        }
        let unseen = |c: &Case| songs.get(c.song).is_some_and(Song::unseen_by_final);
        let chances: Held =
            cases.par_iter().map(|c| unseen(c).then(|| chances_of(c, &model.scores(&c.ev), &calibration))).collect();
        let answers = answers_of(&cases, &chances, |_| Some(&fitted_answers));
        let summary = report(&cases, &chances, &answers, &songs, |c| unseen(c) && group(c) == 0, &inputs);
        for (g, source) in sources.iter().enumerate().skip(1) {
            source_report(source, &cases, &chances, |c| unseen(c) && group(c) == g, &inputs);
        }
        log_run(&label("--final"), &summary, &opts)?;
        let finder = finder(&model.weights, &calibration, &fitted_answers);
        hand_cases(&[("fitted on folds 1 to 4".to_owned(), finder)], &show)?;
    } else if let Some(path) = write {
        // A rescale keeps the credits of the weights it scales: read them first, so a wrong file stops the run now.
        let earlier = if rescale { Some(earlier_credits(&path)?) } else { None };
        println!("Every fold held out in turn (fold {FROZEN} included: these weights are the ones that ship):");
        let (held, _) = cross_validate(&cases, &songs, &every, &opts)?;
        let calibration = calibrate(&cases, &songs, &held, |_| true, &opts)?;
        print_calibration("Calibration", &calibration);
        let chances: Held =
            cases.par_iter().zip(&held).map(|(c, h)| h.as_ref().map(|s| chances_of(c, s, &calibration))).collect();
        let fitted_answers = fit_answers(&cases, &chances, taught, &opts);
        print_answers(
            "Answers (the ones written; each fold below is graded with those fitted on the others)",
            &fitted_answers,
        );
        let per_fold = fitted_per_fold(&cases, &chances, &every, &fold, &taught, &opts);
        let answers = answers_of(&cases, &chances, |c| per_fold.get(&fold(c)));
        // The summary scores the first source read, which the weights file names.
        let (first, scored) = (0, sources.first().map_or("", String::as_str));
        let summary = report(&cases, &chances, &answers, &songs, |c| group(c) == first, &inputs);
        for (g, source) in sources.iter().enumerate().skip(1) {
            source_report(source, &cases, &chances, |c| group(c) == g, &inputs);
        }
        log_run(&label("--write"), &summary, &opts)?;
        let (model, fitted, n) = fit_model(&cases, &songs, &learn, &opts)?;
        println!();
        println!("Fitted on every song: {n} cases, {} Newton steps.", fitted.steps);
        if rescale {
            println!("Scales: {}.", scales_line(&model));
        }
        let shipped = finder(&model.weights, &calibration, &fitted_answers);
        if !hand_cases(&[("fitted on every song".to_owned(), shipped)], &show)? {
            return Err("these weights break a hand case's gate (above), so they aren't written".into());
        }
        let file = weights_file(
            &model,
            &calibration,
            &fitted_answers,
            &opts,
            &learned_from,
            (scored, &summary),
            earlier.as_deref(),
        );
        std::fs::write(&path, file).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("Written to {}.", path.display());
    } else {
        println!("Folds 1 to 4, each held out in turn (fold {FROZEN} isn't touched):");
        let (held, models) = cross_validate(&cases, &songs, &design, &opts)?;
        // Each fold's chances come from a calibration fitted on the other folds' held-out scores.
        let mut calibrations = HashMap::new();
        for &f in &design {
            let c = calibrate(&cases, &songs, &held, |c| fold(c) != f && in_design(c), &opts)?;
            calibrations.insert(f, c);
        }
        let all = calibrate(&cases, &songs, &held, in_design, &opts)?;
        print_calibration(
            "Calibration (on all four folds; each fold is scored with one fitted on the other three)",
            &all,
        );
        let chances: Held = cases
            .par_iter()
            .zip(&held)
            .map(|(c, h)| Some(chances_of(c, h.as_ref()?, calibrations.get(&fold(c))?)))
            .collect();
        // Each fold is graded with the thresholds fitted on the other folds' chances. Those printed are fitted on
        // all four folds' chances from the calibration printed above, as `--write` fits them.
        let fitted_answers = fitted_per_fold(&cases, &chances, &design, &fold, &taught, &opts);
        let chances_all: Held = cases
            .par_iter()
            .zip(&held)
            .map(|(c, h)| in_design(c).then(|| Some(chances_of(c, h.as_ref()?, &all))).flatten())
            .collect();
        print_answers(
            "Answers (fitted on all four folds; each fold is graded with those fitted on the other three)",
            &fit_answers(&cases, &chances_all, |c| in_design(c) && taught(c), &opts),
        );
        let answers = answers_of(&cases, &chances, |c| fitted_answers.get(&fold(c)));
        report(&cases, &chances, &answers, &songs, |c| in_design(c) && group(c) == 0, &inputs);
        for (g, source) in sources.iter().enumerate().skip(1) {
            source_report(source, &cases, &chances, |c| in_design(c) && group(c) == g, &inputs);
        }
        compare_and_dump(&cases, &chances, &songs, &sources, &in_design, &inputs, before.as_ref(), dump.as_ref())?;
        let finders: Vec<(String, keyfind::Model)> = models
            .iter()
            .filter_map(|(f, m)| {
                Some((format!("fold {f} held out"), finder(&m.weights, calibrations.get(f)?, fitted_answers.get(f)?)))
            })
            .collect();
        hand_cases(&finders, &show)?;
    }
    Ok(())
}

/// Compares this run with one dumped before (`--against`), source by source, and writes this run's outcomes
/// (`--dump`), both over the cases `keep` takes.
#[allow(clippy::too_many_arguments)]
fn compare_and_dump(
    cases: &[Case],
    chances: &[Option<[f64; 24]>],
    songs: &[Song],
    sources: &[String],
    keep: &dyn Fn(&Case) -> bool,
    inputs: &[Input],
    before: Option<&(String, String)>,
    dump: Option<&PathBuf>,
) -> Result<(), String> {
    let group = |c: &Case| songs.get(c.song).map_or(usize::MAX, |s| s.group);
    if let Some((name, text)) = before {
        let was = read_dump(name, text, cases, songs)?;
        let mut worse = against(cases, chances, songs, |c| keep(c) && group(c) == 0, inputs, &was);
        // Another source is compared too when the run dumped before had it.
        for (g, source) in sources.iter().enumerate().skip(1) {
            let had = cases.iter().zip(&was.outcomes).any(|(c, o)| group(c) == g && o.is_some());
            if had {
                println!();
                println!("({source}:)");
                let of = |c: &Case| keep(c) && group(c) == g;
                let worse_here = against(cases, chances, songs, of, inputs, &was);
                worse.extend(worse_here.into_iter().map(|w| format!("{source}: {w}")));
            }
        }
        if worse.is_empty() {
            println!("No set got worse beyond its range.");
        } else {
            println!("Worse beyond its range, or not comparable:");
            for line in worse {
                println!("  {line}");
            }
        }
    }
    if let Some(path) = dump {
        std::fs::write(path, dump_text(cases, chances, songs, keep)).map_err(|e| format!("{}: {e}", path.display()))?;
        println!();
        println!("Each case's outcome written to {}.", path.display());
    }
    Ok(())
}

/// The cases of one set heard as one kind of input, among those `keep` takes, in the run's order.
fn members(cases: &[Case], keep: &dyn Fn(&Case) -> bool, cut: Cut, one_key: Option<bool>, input: Input) -> Vec<usize> {
    cases
        .iter()
        .enumerate()
        .filter(|(_, c)| keep(c) && c.cut == cut && c.input == input && one_key.is_none_or(|o| c.one_key == o))
        .map(|(i, _)| i)
        .collect()
}

/// How a case came out: whether the likeliest key is the one labelled, and the log-loss (the chance given to the key
/// labelled, as -ln).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Outcome {
    right: bool,
    nll: f64,
}

impl Outcome {
    fn of(case: &Case, chances: &[f64; 24]) -> Outcome {
        let p = chances.get(case.truth.index()).copied().unwrap_or(0.0);
        Outcome { right: keyfind::best(chances).0 == case.truth, nll: -p.max(1e-12).ln() }
    }
}

/// What a case is, as a dump says it: its song, cut and kind of input (with its place in the run, enough to tell a
/// dump made with other cases).
fn identity(case: &Case, songs: &[Song]) -> String {
    let song = songs.get(case.song).map_or("?", |s| s.name.as_str());
    format!("{song}\t{}\t{}", case.cut.name(), case.input.name())
}

/// How each case `keep` takes came out, as `--dump` writes it: a line each, with its place in the run and what it is.
fn dump_text(cases: &[Case], chances: &[Option<[f64; 24]>], songs: &[Song], keep: impl Fn(&Case) -> bool) -> String {
    let mut out = String::from("# theorybench keyfit --dump: case, song, cut, input, right (1) or not (0), log-loss\n");
    for (i, (case, chances)) in cases.iter().zip(chances).enumerate() {
        if let Some(chances) = chances.as_ref().filter(|_| keep(case)) {
            let o = Outcome::of(case, chances);
            let _ = writeln!(out, "{i}\t{}\t{}\t{:?}", identity(case, songs), u8::from(o.right), o.nll);
        }
    }
    out
}

/// A run's outcomes read back from its dump (`--against`), by place in this run; refused if the dump's cases aren't
/// this run's (other --bars or --inputs, or other music). Cases missing from it are named by the comparison.
struct Dump {
    name: String,
    outcomes: Vec<Option<Outcome>>,
}

fn read_dump(name: &str, text: &str, cases: &[Case], songs: &[Song]) -> Result<Dump, String> {
    let mut outcomes = vec![None; cases.len()];
    for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#')) {
        let bad = || format!("{name} line {}: not a line --dump writes", n + 1);
        let fields: Vec<&str> = line.split('\t').collect();
        let [i, song, cut, input, right, nll] = fields.as_slice() else { return Err(bad()) };
        let i: usize = i.parse().map_err(|_| bad())?;
        let case = cases.get(i).ok_or_else(|| format!("{name}: case {i} isn't in this run"))?;
        if identity(case, songs) != format!("{song}\t{cut}\t{input}") {
            return Err(format!(
                "{name}: case {i} is {song}, {cut}, {input} there but {} here; was it made with other --bars or --inputs?",
                identity(case, songs).replace('\t', ", ")
            ));
        }
        let right = match *right {
            "1" => true,
            "0" => false,
            _ => return Err(bad()),
        };
        let nll: f64 = nll.parse().map_err(|_| bad())?;
        match outcomes.get_mut(i) {
            Some(slot @ None) => *slot = Some(Outcome { right, nll }),
            _ => return Err(format!("{name} line {}: case {i} is there twice", n + 1)),
        }
    }
    Ok(Dump { name: name.to_owned(), outcomes })
}

/// Prints how this run compares with a run dumped before (`--against`), set by set and input by input: the share
/// right and the log-loss before and now, and each difference with its likely range (each case paired with itself,
/// songs drawn again). Gives every set that got worse beyond its range, which a change being chosen mustn't do.
fn against(
    cases: &[Case],
    chances: &[Option<[f64; 24]>],
    songs: &[Song],
    keep: impl Fn(&Case) -> bool,
    inputs: &[Input],
    was: &Dump,
) -> Vec<String> {
    println!();
    println!(
        "=== Against {} (each case in both runs paired with itself; ranges with the songs, or packs, drawn again) ===",
        was.name
    );
    println!(
        "{:<30} {:<20} {:>7} {:>7} {:>7}   {:<22} {:>7} {:>7}   difference (range)",
        "", "", "cases", "before", "now", "difference (range)", "NLL was", "now"
    );
    let mut worse = Vec::new();
    let mut thin = Vec::new();
    for (cut, one_key) in sets(cases, &keep) {
        for &input in inputs {
            let members = members(cases, &keep, cut, one_key, input);
            let pairs: Vec<(usize, Outcome, Outcome)> = members
                .iter()
                .filter_map(|&i| {
                    let case = cases.get(i)?;
                    let now = Outcome::of(case, chances.get(i)?.as_ref()?);
                    Some((case.song, was.outcomes.get(i).copied().flatten()?, now))
                })
                .collect();
            let title = set_title(cut, one_key);
            // A case with an outcome in only one run (a dump cut short, or a finder that now has no chances there)
            // can't be paired: say so, rather than compare the rest as if they were all.
            if pairs.len() < members.len() {
                worse.push(format!(
                    "{title}, {}: {} of {} cases not in both runs, left out",
                    input.name(),
                    members.len() - pairs.len(),
                    members.len()
                ));
            }
            if pairs.is_empty() {
                continue;
            }
            // Songs drawn into a fold together (a pack's files) are drawn again together.
            let song: Vec<usize> = pairs.iter().map(|p| songs.get(p.0).map_or(p.0, |s| s.draw)).collect();
            let drawn = song.iter().collect::<HashSet<_>>().len();
            if drawn < FEWEST_DRAWN {
                thin.push(format!("{title}, {} ({drawn})", input.name()));
            }
            let right = |f: fn(&(usize, Outcome, Outcome)) -> bool| pairs.iter().map(f).collect::<Vec<bool>>();
            let (before, now) = (right(|p| p.1.right), right(|p| p.2.right));
            let share = |r: &[bool]| r.iter().filter(|r| **r).count() as f64 / r.len() as f64;
            let p = metrics::paired(&before, &now, &song);
            let nll = |f: fn(&(usize, Outcome, Outcome)) -> f64| pairs.iter().map(f).sum::<f64>() / pairs.len() as f64;
            let diff: Vec<f64> = pairs.iter().map(|(_, b, n)| n.nll - b.nll).collect();
            let d = metrics::mean(&diff, &song);
            println!(
                "{:<30} {:<20} {:>7} {:>6.1}% {:>6.1}%   {:<22} {:>7.3} {:>7.3}   {:+.3} ({:+.3} to {:+.3})",
                title,
                input.name(),
                pairs.len(),
                100.0 * share(&before),
                100.0 * share(&now),
                format!("{:+.1} ({:+.1} to {:+.1})", 100.0 * p.diff, 100.0 * p.low, 100.0 * p.high),
                nll(|p| p.1.nll),
                nll(|p| p.2.nll),
                d.value,
                d.low,
                d.high,
            );
            if p.high < 0.0 {
                worse.push(format!(
                    "{title}, {}: {:+.1} points right ({:+.1} to {:+.1})",
                    input.name(),
                    100.0 * p.diff,
                    100.0 * p.low,
                    100.0 * p.high
                ));
            }
            if d.low > 0.0 {
                worse.push(format!(
                    "{title}, {}: log-loss {:+.3} ({:+.3} to {:+.3})",
                    input.name(),
                    d.value,
                    d.low,
                    d.high
                ));
            }
        }
    }
    if !thin.is_empty() {
        println!(
            "From fewer than {FEWEST_DRAWN} songs or packs, so their ranges say little (a pack's files are drawn again \
             together): {}.",
            thin.join("; ")
        );
    }
    worse
}

/// The sets a report is made of, in the order it prints them: whole songs in one key, those that change key, then
/// each cut by length.
fn sets(cases: &[Case], keep: &dyn Fn(&Case) -> bool) -> Vec<(Cut, Option<bool>)> {
    let mut cuts: Vec<Cut> =
        cases.iter().filter(|c| keep(c)).map(|c| c.cut).collect::<HashSet<_>>().into_iter().collect();
    cuts.sort();
    let mut out = Vec::new();
    for cut in cuts {
        if cut == Cut::Song {
            out.push((cut, Some(true)));
            out.push((cut, Some(false)));
        } else {
            out.push((cut, None));
        }
    }
    out
}

fn set_title(cut: Cut, one_key: Option<bool>) -> String {
    match one_key {
        Some(true) => "Whole songs with one key".to_owned(),
        Some(false) => "Whole songs that change key".to_owned(),
        None => cut.name(),
    }
}

/// What the report sums up, for the weights file and the log of final runs.
struct Summary {
    lines: Vec<String>,
    /// Each set's title, share right, and the C# app's share right, for the full mix.
    full: Vec<(String, f64, f64)>,
}

/// Prints the report for the cases `keep` takes, with the chances given.
fn report(
    cases: &[Case],
    chances: &[Option<[f64; 24]>],
    answers: &[Option<(KeyAnswer, f64)>],
    songs: &[Song],
    keep: impl Fn(&Case) -> bool,
    inputs: &[Input],
) -> Summary {
    let mut summary = Summary { lines: Vec::new(), full: Vec::new() };
    let mut compact = Vec::new();
    let right = |answers: &[Answer]| answers.iter().filter(|a| a.right()).count() as f64 / answers.len().max(1) as f64;
    for (cut, one_key) in sets(cases, &keep) {
        for &input in inputs {
            let members = members(cases, &keep, cut, one_key, input);
            if members.is_empty() {
                continue;
            }
            let song = draws(cases, songs, &members);
            let mo: Vec<Answer> = members
                .iter()
                .map(|&i| Answer {
                    truth: cases[i].truth,
                    found: chances[i].map(|c| {
                        let (key, sure) = keyfind::best(&c);
                        let alone = Some(matches!(answers[i], Some((KeyAnswer::Key(_), _))));
                        Found { key, sure, chances: Some(c), alone }
                    }),
                })
                .collect();
            let cs: Vec<Answer> = members
                .iter()
                .map(|&i| Answer {
                    truth: cases[i].truth,
                    found: cases[i].cs.map(|(key, sure)| Found { key, sure, chances: None, alone: None }),
                })
                .collect();
            let title = set_title(cut, one_key);
            if input == Input::Full {
                let graded = graded(&members.iter().map(|&i| (&cases[i], answers[i])).collect::<Vec<_>>());
                let runs = [
                    Run {
                        name: "C# app (Songs page)".into(),
                        cut: Some(baseline::SONG_MIN_CONFIDENCE),
                        answers: cs.clone(),
                        micros: None,
                    },
                    Run { name: "mo-harmony (held out)".into(), cut: None, answers: mo.clone(), micros: None },
                ];
                keys::report(&title, "the key labelled (at the middle of a stretch)", &song, &runs);
                println!("{graded}");
                summary.lines.push(format!("{title}: {} right ({} cases)", pct(right(&mo)), mo.len()));
                summary.full.push((title.clone(), right(&mo), right(&cs)));
            }
            let cs_right: Vec<bool> = cs.iter().map(Answer::right).collect();
            let mo_right: Vec<bool> = mo.iter().map(Answer::right).collect();
            let p = metrics::paired(&cs_right, &mo_right, &song);
            let sure: Vec<(Option<f64>, bool)> = mo.iter().map(|a| (a.found.map(|f| f.sure), a.right())).collect();
            let nll =
                members.iter().filter_map(|&i| Some(Outcome::of(&cases[i], chances[i].as_ref()?).nll)).sum::<f64>()
                    / mo.len().max(1) as f64;
            let mean_sure = sure.iter().filter_map(|s| s.0).sum::<f64>() / mo.len().max(1) as f64;
            compact.push(format!(
                "{:<30} {:<20} {:>7} {:>7.1}% {:>7.1}% {:>+6.1} ({:+.1} to {:+.1}) {:>5.1}% {:>6.3} {:>6.3} {:>6.3}",
                title,
                input.name(),
                mo.len(),
                100.0 * right(&cs),
                100.0 * right(&mo),
                100.0 * p.diff,
                100.0 * p.low,
                100.0 * p.high,
                100.0 * mean_sure,
                nll,
                metrics::calibration_error_equal_mass(&sure, 15).unwrap_or(f64::NAN),
                metrics::risk_coverage_area(&sure).unwrap_or(f64::NAN),
            ));
        }
    }
    let used: HashSet<usize> = cases.iter().filter(|c| keep(c)).map(|c| c.song).collect();
    println!();
    println!("=== Every kind of input, from {} of the {} songs ===", used.len(), songs.len());
    // "sure" is the average chance mo-harmony gave its answers: above its share right, it was too sure.
    println!(
        "{:<30} {:<20} {:>7} {:>8} {:>8} {:>26} {:>6} {:>6} {:>6} {:>6}",
        "", "", "cases", "C# app", "mo-harm.", "difference (likely range)", "sure", "NLL", "ECE", "AURC"
    );
    for line in compact {
        println!("{line}");
    }
    summary
}

/// The report for a source after the first: for each set and kind of input, how often the likeliest key was the one
/// labelled, and how often its tonic was the one labelled (chord charts label only the tonic; their mode is read from
/// the tonic chord), for the C# app and for mo-harmony, and mo-harmony's log-loss.
fn source_report(
    source: &str,
    cases: &[Case],
    chances: &[Option<[f64; 24]>],
    keep: impl Fn(&Case) -> bool,
    inputs: &[Input],
) {
    let songs: HashSet<usize> = cases.iter().filter(|c| keep(c)).map(|c| c.song).collect();
    if songs.is_empty() {
        return;
    }
    println!();
    println!("=== {source}: {} songs, the key and its tonic alone ===", songs.len());
    println!(
        "{:<30} {:<20} {:>7} {:>9} {:>9} {:>9} {:>9} {:>7}",
        "", "", "cases", "C# key", "mo key", "C# tonic", "mo tonic", "NLL"
    );
    for (cut, one_key) in sets(cases, &keep) {
        for &input in inputs {
            let scored: Vec<(&Case, &[f64; 24])> = members(cases, &keep, cut, one_key, input)
                .into_iter()
                .filter_map(|i| Some((cases.get(i)?, chances.get(i)?.as_ref()?)))
                .collect();
            if scored.is_empty() {
                continue;
            }
            let (mut cs_key, mut mo_key, mut cs_tonic, mut mo_tonic, mut nll) = (0, 0, 0, 0, 0.0);
            for &(case, chances) in &scored {
                let best = keyfind::best(chances).0;
                mo_key += usize::from(best == case.truth);
                mo_tonic += usize::from(best.tonic == case.truth.tonic);
                if let Some((key, _)) = case.cs {
                    cs_key += usize::from(key == case.truth);
                    cs_tonic += usize::from(key.tonic == case.truth.tonic);
                }
                nll += Outcome::of(case, chances).nll;
            }
            let n = scored.len() as f64;
            let share = |hits: usize| 100.0 * hits as f64 / n;
            println!(
                "{:<30} {:<20} {:>7} {:>8.1}% {:>8.1}% {:>8.1}% {:>8.1}% {:>7.3}",
                set_title(cut, one_key),
                input.name(),
                scored.len(),
                share(cs_key),
                share(mo_key),
                share(cs_tonic),
                share(mo_tonic),
                nll / n,
            );
        }
    }
}

/// A case's answer as the app would show it, graded with these fitted thresholds, and the chance shown with it.
fn graded_answer(case: &Case, chances: &[f64; 24], answers: &Fitted) -> (KeyAnswer, f64) {
    let grading = keyfind::Grading {
        bars_heard: case.ev.bars,
        notes: case.ev.notes,
        diatonic_fit: case.ev.diatonic_fit,
        span: case.cut.span(),
    };
    // Only the thresholds and the share of notes in one scale grade; the weights are the ones built in.
    keyfind::grade(chances, grading, &finder(keyfind::SHIPPED.weights, keyfind::SHIPPED.calibration, answers))
}

/// How the answers would be shown (one key, a key or its relative, a home note, no clear key), how often each kind is
/// right (a pair when the labelled key is either, a home note when it's the tonic), and how sure the finder said it
/// was (the chance shown, on average): said above right, it was too sure.
fn graded(cases: &[(&Case, Option<(KeyAnswer, f64)>)]) -> String {
    // Given, right, chance shown; and for no clear key, why.
    let mut tally = [(0usize, 0usize, 0.0f64); 4];
    let mut why = [0usize; 3];
    for (case, answer) in cases {
        let Some((answer, shown)) = *answer else { continue };
        let kind = match answer {
            KeyAnswer::Key(_) => 0,
            KeyAnswer::OrRelative(_) => 1,
            KeyAnswer::HomeNote { .. } => 2,
            KeyAnswer::Unclear { why: w, .. } => {
                why[match w {
                    Why::Spread => 0,
                    Why::NotTonal => 1,
                    Why::TooFewNotes => 2,
                }] += 1;
                3
            }
        };
        let t = &mut tally[kind];
        t.0 += 1;
        t.1 += usize::from(answer.names(case.truth));
        t.2 += shown;
    }
    let total = tally.iter().map(|t| t.0).sum::<usize>().max(1) as f64;
    let share = |n: usize, of: f64| 100.0 * n as f64 / of.max(1.0);
    let names = ["one key", "key or its relative", "home note"];
    let mut parts: Vec<String> = names
        .iter()
        .zip(&tally)
        .map(|(name, &(n, right, said))| {
            if n == 0 {
                return format!("{name} 0%");
            }
            let of = n as f64;
            format!("{name} {:.1}% ({:.1}% right, said {:.1}%)", share(n, total), share(right, of), 100.0 * said / of)
        })
        .collect();
    let [spread, not_tonal, few] = why;
    parts.push(format!(
        "no clear key {:.1}% (spread {:.1}, not in one scale {:.1}, too few notes {:.1})",
        share(tally[3].0, total),
        share(spread, total),
        share(not_tonal, total),
        share(few, total)
    ));
    format!("  Shown as: {}.", parts.join(", "))
}

/// Reads the design's hand cases with each fitted model and prints how many models keep each case's gate and meet its
/// target, with what broke a gate. Returns whether every model keeps every gate.
fn hand_cases(models: &[(String, keyfind::Model)], show: &[String]) -> Result<bool, String> {
    let books: Vec<(&str, Book)> =
        models.iter().map(|(name, model)| Ok((name.as_str(), Book::read(model)?))).collect::<Result<_, String>>()?;
    let judged: Vec<(&str, Vec<Verdict>)> = books.iter().map(|(name, book)| (*name, book.verdicts())).collect();
    let n = judged.len();
    println!();
    println!(
        "=== The hand cases: gates (must hold) and targets (to reach), for {n} fitted model{} ===",
        if n == 1 { "" } else { "s" }
    );
    let mut every = true;
    let mut broken = Vec::new();
    // Every case the hand cases judge (each model judges the same ones).
    let cases: Vec<u8> = judged.first().map(|(_, vs)| vs.iter().map(|v| v.case).collect()).unwrap_or_default();
    for case in cases {
        let verdicts: Vec<(&str, &Verdict)> =
            judged.iter().filter_map(|(name, vs)| vs.iter().find(|v| v.case == case).map(|v| (*name, v))).collect();
        let Some((_, first)) = verdicts.first() else { continue };
        let gates = verdicts.iter().filter(|(_, v)| v.passes_gate()).count();
        let targets: Vec<bool> = verdicts.iter().filter_map(|(_, v)| v.meets_target()).collect();
        let has_gate = verdicts.iter().any(|(_, v)| v.gate.is_some());
        let gate = if has_gate { format!("gate {gates}/{n}") } else { "no gate".to_owned() };
        let target = if targets.is_empty() {
            "no target".to_owned()
        } else {
            format!("target {}/{n}", targets.iter().filter(|t| **t).count())
        };
        println!("{case:>3}  {gate:<10} {target:<11} {}", first.about);
        if gates < n {
            every = false;
            if let Some((name, v)) = verdicts.iter().find(|(_, v)| !v.passes_gate()) {
                broken.extend(v.broken().iter().map(|g| format!("  {name}: {g}")));
            }
        }
    }
    if !broken.is_empty() {
        println!("Gates broken (the first model to break each case):");
        for line in &broken {
            println!("{line}");
        }
    }
    for name in show {
        println!("Case {name} (--hand):");
        for (model, book) in &books {
            let read = book.readings.iter().find(|(c, _)| c.name == name.as_str()).map(|(_, r)| r);
            match read {
                None => println!("  {model}: no case of that name"),
                Some(None) => println!("  {model}: nothing pitched heard"),
                Some(Some((_, r))) => {
                    let top: Vec<String> = r.ranked().iter().take(4).map(|(k, p)| format!("{k} {p:.3}")).collect();
                    println!("  {model}: {}; {}", cases::describe(r.answer), top.join(", "));
                }
            }
        }
    }
    Ok(every)
}

/// Today's date, from the clock (UTC).
fn today() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400);
    // Howard Hinnant's days-to-civil.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Adds a line to `test-runs.md` for a run that showed fold 0: `--final`, or `--write` (which scores every fold
/// before it writes the weights). `run` says which, and what it read.
fn log_run(run: &str, summary: &Summary, opts: &Options) -> Result<(), String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-runs.md");
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    // A run from changes not yet committed is marked, as `git describe --dirty` does: the commit alone isn't the code.
    let commit = match (git(&["rev-parse", "--short", "HEAD"]), git(&["status", "--porcelain", "--untracked-files=no"]))
    {
        (Some(c), Some(changes)) if !changes.is_empty() => format!("{c}-dirty"),
        (Some(c), _) => c,
        (None, _) => "unknown".to_owned(),
    };
    let mut text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        "# Runs on the kept-back fold\n\nEvery `theorybench keyfit --final` or `--write` run adds a line here: \
         fold 0 is only looked at these ways (`--final` scores it alone; `--write` scores every fold, fold 0 \
         included, before it writes the weights that ship), so this is how often it was. A commit marked -dirty \
         had changes not yet committed when it ran.\n\n| Date | Run | Finder version | Commit | Corrections penalty \
         | Full mix: mo-harmony (C# app) |\n|---|---|---|---|---|---|\n"
            .to_owned()
    });
    let results: Vec<String> =
        summary.full.iter().map(|(t, r, c)| format!("{t} {:.1}% ({:.1}%)", 100.0 * r, 100.0 * c)).collect();
    let penalty = match opts.lambda_delta {
        _ if opts.rescale => "none (rescale)".to_owned(),
        Some(l) => format!("{l}"),
        None => "inf".to_owned(),
    };
    let _ = writeln!(
        text,
        "| {} | {run} | {KEY_FINDER_VERSION} | {commit} | {penalty} | {} |",
        today(),
        results.join("; ")
    );
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    println!();
    println!("Logged in {}.", path.display());
    Ok(())
}

/// A source the weights learned from: its name, how it was weighed ("billboard at 0.25"), and its credit.
struct Learned {
    name: String,
    how: String,
    credit: String,
}

/// The weights as Rust source for `crates/mo-harmony/src/keyfind/weights.rs`, from the sources learned from, and the
/// summary of how often the key was right on the songs of `scored`.
/// The lines of the weights file a rescale scales that say what it was fitted on and credit its sources ("//! Fitted
/// on: …" and the "//! - …" lines after it), from the file at `path`: the one built into this bench, checked by the
/// hash of its numbers.
fn earlier_credits(path: &std::path::Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("--write {}: {e}", path.display()))?;
    credits_in(&text, path)
}

/// [`earlier_credits`] from the file's text.
fn credits_in(text: &str, path: &std::path::Path) -> Result<Vec<String>, String> {
    let hash = format!("pub(super) const SHIPPED_HASH: u64 = {:#018x};", keyfind::SHIPPED_HASH.0);
    if !text.lines().any(|l| l.trim() == hash) {
        return Err(format!(
            "--rescale --write {}: that isn't the weights file built into this theorybench (its numbers' hash \
             differs). Point --write at crates/mo-harmony/src/keyfind/weights.rs, built as it is.",
            path.display()
        ));
    }
    let lines: Vec<String> = text
        .lines()
        .skip_while(|l| !l.starts_with("//! Fitted on:"))
        .enumerate()
        .take_while(|(i, l)| *i == 0 || l.starts_with("//! - "))
        .map(|(_, l)| l.to_owned())
        .collect();
    if lines.is_empty() {
        return Err(format!("--rescale --write {}: it doesn't say what it was fitted on", path.display()));
    }
    if text.lines().any(|l| l.starts_with("//! Then rescaled on")) {
        return Err(format!(
            "--rescale --write {}: these weights were rescaled already; rescale the ones fitted before",
            path.display()
        ));
    }
    Ok(lines)
}

/// The weights file. `earlier` holds, for a rescale, the lines of the file it scales that say what that was fitted on
/// and credit its sources ([`earlier_credits`]): they stay, and the rescale is told after them.
fn weights_file(
    model: &Model,
    calibration: &Calibration,
    answers: &Fitted,
    opts: &Options,
    learned: &[Learned],
    (scored, summary): (&str, &Summary),
    earlier: Option<&[String]>,
) -> String {
    let penalty = opts.lambda_delta.map_or("none fitted".to_owned(), |l| format!("penalty {l}"));
    let mut out = String::new();
    out.push_str(
        "//! The key finder's weights. Made by `theorybench keyfit`; don't edit by hand, except the last two lines when\n\
         //! a test asks (another number the finder is built from changed).\n//!\n",
    );
    match earlier {
        Some(lines) => {
            for line in lines {
                let _ = writeln!(out, "{line}");
            }
        }
        None => {
            let _ = writeln!(
                out,
                "//! Fitted on: {} (trust weights penalty {LAMBDA_W}, corrections {penalty}).",
                learned.iter().map(|l| l.how.as_str()).collect::<Vec<_>>().join(", ")
            );
            for l in learned {
                let _ = writeln!(out, "//! - {}", l.credit);
            }
        }
    }
    // A paragraph of its own: right after the list, it would be read as part of its last line.
    out.push_str("//!\n//! Only these numbers come from them.\n");
    let (pieces, by) = if earlier.is_some() {
        let _ = writeln!(
            out,
            "//!\n//! Then rescaled on {}: each stream's weights scaled by one number, and the minor weight and the\n\
             //! calibration fitted again (penalty {LAMBDA_W}); nothing else comes from them. {}.",
            learned.iter().map(|l| l.how.as_str()).collect::<Vec<_>>().join(", "),
            learned.iter().map(|l| l.credit.as_str()).collect::<Vec<_>>().join("; ")
        );
        let _ = writeln!(out, "//! Scales: {}.", scales_line(model));
        ("pieces", "pack")
    } else {
        ("songs", "song")
    };
    let _ = writeln!(
        out,
        "//!\n//! Right on {scored} {pieces} held out of the fit (cross-validation by {by}, every fold):"
    );
    for line in &summary.lines {
        let _ = writeln!(out, "//! - {line}");
    }
    out.push_str("\nuse super::{Calibration, FEATURES, Thresholds};\n\n");
    out.push_str("/// Whether these weights were fitted (`false` for the placeholder).\npub(super) const FITTED: bool = true;\n\n");
    // A rescale learned from the sources the weights it scaled were fitted on, too.
    let mut names: Vec<String> = Vec::new();
    if earlier.is_some() {
        names.extend(keyfind::LEARNED_FROM.iter().map(|n| format!("{n:?}")));
    }
    for l in learned {
        let name = format!("{:?}", l.name);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    out.push_str("/// The sources learned from, by the names `theorybench` gives them.\n");
    let _ = writeln!(out, "pub(super) const LEARNED_FROM: &[&str] = &[{}];\n", names.join(", "));
    out.push_str(
        "/// One weight per feature (see `Heard::features`): for each stream, twelve for the notes above a major\n",
    );
    out.push_str("/// key's tonic, then twelve for a minor key's; last, one for minor keys as a whole.\n");
    out.push_str("#[rustfmt::skip]\npub(super) const WEIGHTS: [f64; FEATURES] = [\n");
    for (s, stream) in Stream::ALL.iter().enumerate() {
        for (m, mode) in ["major", "minor"].iter().enumerate() {
            let _ = writeln!(out, "    // {stream:?}, {mode}: the tonic, then each semitone up.");
            let row: Vec<String> = (0..12)
                .map(|d| format!("{:?}", model.weights.get(s * 24 + m * 12 + d).copied().unwrap_or(0.0)))
                .collect();
            let _ = writeln!(out, "    {},", row.join(", "));
        }
    }
    let _ = writeln!(out, "    // Minor keys.\n    {:?},\n];", model.weights.get(MINOR).copied().unwrap_or(0.0));
    out.push_str(
        "/// How much the scores are sharpened (see `Calibration`): the knots at 1 to 16 bars; melody only, bass only,\n",
    );
    out.push_str("/// accompaniment; what a loop adds at 1 to 16 bars; what a song adds.\n");
    let numbers: Vec<String> = calibration.0.iter().map(|v| format!("{v:?}")).collect();
    let _ = writeln!(
        out,
        "#[rustfmt::skip]\npub(super) const CALIBRATION: Calibration = Calibration([{}]);",
        numbers.join(", ")
    );
    let knots = |k: &[f64]| k.iter().map(|v| format!("{v:?}")).collect::<Vec<_>>().join(", ");
    if earlier.is_some() {
        out.push_str(
            "/// How likely each kind of answer must be (see `Thresholds`) at 1, 2, 4, 8, 16 and 32 bars: kept as they were\n",
        );
        let _ =
            writeln!(out, "/// fitted before the rescale, and how they come out on the {scored} {pieces} held out:");
    } else {
        out.push_str(
            "/// How likely each kind of answer must be (see `Thresholds`) at 1, 2, 4, 8, 16 and 32 bars, fitted on the chances\n",
        );
        out.push_str("/// of songs held out (`theorybench` `answerfit`):\n");
    }
    for line in &answers.knot_lines {
        let _ = writeln!(out, "/// - {line}");
    }
    let t = &answers.thresholds;
    let _ = writeln!(
        out,
        "#[rustfmt::skip]\npub(super) const THRESHOLDS: Thresholds = Thresholds {{\n    one: [{}],\n    pair: [{}],\n    \
         home: [{}],\n}};",
        knots(&t.one),
        knots(&t.pair),
        knots(&t.home),
    );
    out.push_str(
        "/// Notes with less than this share of their time inside one scale have no key (see `Why::NotTonal`).\n",
    );
    let _ = writeln!(out, "/// {}", answers.scale_line);
    let _ = writeln!(out, "pub(super) const NOT_TONAL_BELOW: f64 = {:?};", answers.not_tonal_below);
    // The hash of every number, and the version they were first given with: the next one when any number changed.
    let hash = keyfind::numbers_hash(&finder(&model.weights, calibration, answers));
    let (shipped_hash, shipped_version) = keyfind::SHIPPED_HASH;
    let version = if hash == shipped_hash { shipped_version } else { KEY_FINDER_VERSION + 1 };
    out.push_str(
        "/// The hash of every number the finder is built from (see `numbers_hash`) when this file was written, and the version\n",
    );
    out.push_str(
        "/// of the finder these numbers were first given with: a test checks both, so changing any number fails until the\n",
    );
    out.push_str("/// version is bumped.\n");
    let _ = writeln!(out, "pub(super) const SHIPPED_HASH: u64 = {hash:#018x};");
    let _ = writeln!(out, "pub(super) const HASH_VERSION: u32 = {version};");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: i64, length: i64, key: u8) -> Note {
        Note { start, length, key, velocity: 100 }
    }

    #[test]
    fn the_fit_reads_the_same_features_the_finder_scores_with() {
        let tune = [note(0, 96, 72), note(96, 192, 74), note(288, 96, 71)];
        let piano = [note(0, 384, 48), note(0, 384, 63), note(0, 384, 67), note(384, 384, 43), note(384, 384, 62)];
        let parts = vec![(Role::Melody, tune.to_vec()), (Role::Harmony, piano.to_vec())];
        let mut reader = KeyReader::new();
        let grid = Grid::four_four(96);
        for span in Span::ALL {
            let (ev, _) = hear(&mut reader, &parts, grid, 96, span).unwrap();
            let notes: Vec<Vec<mo_harmony::Note>> = parts
                .iter()
                .map(|(_, n)| {
                    n.iter()
                        .map(|n| mo_harmony::Note { start: n.start, length: n.length, key: n.key, velocity: 1 << 15 })
                        .collect()
                })
                .collect();
            let heard_parts: Vec<mo_harmony::Part> =
                parts.iter().zip(&notes).map(|((role, _), notes)| mo_harmony::Part { role: *role, notes }).collect();
            let heard = reader.hear(&heard_parts, grid).unwrap();
            for key in Key::ALL {
                let (mut ours, mut theirs) = (Vec::new(), Vec::new());
                ev.features(key, &mut |i, v| ours.push((i, v)));
                heard.features(key, span, |i, v| theirs.push((i, v)));
                assert_eq!(ours, theirs, "{key:?} {span:?}");
            }
            // And a fitted model scores as the crate does, to the last bit.
            let weights: [f64; FEATURES] = std::array::from_fn(|i| (i as f64 * 0.37).sin());
            let model = Model { weights };
            let scores = model.scores(&ev);
            for key in Key::ALL {
                assert_eq!(scores[key.index()].to_bits(), keyfind::score(&heard, key, span, &weights).to_bits());
            }
        }
    }

    #[test]
    fn the_piano_bass_is_the_lowest_note_standing_apart() {
        // C2 under a C chord, then a close E-G with nothing under it, then G1 under D2 (a fifth up).
        let notes = [
            note(0, 96, 36),
            note(0, 96, 60),
            note(0, 96, 64),
            note(96, 96, 64),
            note(96, 96, 67),
            note(192, 96, 31),
            note(192, 96, 38),
        ];
        let (bass, upper) = split_bass(&notes);
        assert_eq!(bass.iter().map(|n| n.key).collect::<Vec<_>>(), [36, 31]);
        assert_eq!(upper.len(), 5);
    }

    /// A whole song as a full mix, labelled C major, with no evidence (the comparison only reads chances).
    fn song_case(song: usize) -> Case {
        let ev = Evidence { x: [[0.0; 12]; STREAMS], bars: 8, config: Config::Full, diatonic_fit: 0.9, notes: 20 };
        let truth = Key::major(PitchClass::C);
        Case { song, truth, cut: Cut::Song, input: Input::Full, one_key: true, pick: true, ev, cs: None }
    }

    /// Chances giving C major `p` and A minor the rest.
    fn c_or_a(p: f64) -> Option<[f64; 24]> {
        let mut chances = [0.0; 24];
        chances[Key::major(PitchClass::C).index()] = p;
        chances[Key::minor(PitchClass::A).index()] = 1.0 - p;
        Some(chances)
    }

    #[test]
    fn a_dumped_run_is_read_back_case_by_case() {
        let songs: Vec<Song> = (0..3)
            .map(|i| Song {
                name: format!("pop909/{i:03}"),
                fold: 1,
                learn: true,
                weight: [1.0; 5],
                draw: i,
                music: i,
                copies: 1,
                ..Song::default()
            })
            .collect();
        let cases: Vec<Case> = (0..3).map(song_case).collect();
        let chances = vec![c_or_a(0.9), c_or_a(0.2), None];
        let text = dump_text(&cases, &chances, &songs, |_| true);
        assert!(text.contains("0\tpop909/000\tWhole songs\tfull mix\t1\t"), "{text}");
        let was = read_dump("before.tsv", &text, &cases, &songs).unwrap();
        assert_eq!(was.outcomes[0], Some(Outcome { right: true, nll: -(0.9f64.ln()) }));
        assert_eq!(was.outcomes[1].map(|o| o.right), Some(false));
        assert_eq!(was.outcomes[2], None, "a case with no chances isn't written");
        // A dump made with other cases is refused, not compared.
        let other: Vec<Case> = (0..3).map(|i| Case { input: Input::Tune, ..song_case(i) }).collect();
        let refused = read_dump("before.tsv", &text, &other, &songs).err().unwrap();
        assert!(refused.contains("tune only here"), "{refused}");
        assert!(read_dump("before.tsv", "0\tpop909/000\tWhole songs\tfull mix\t2\t0.1\n", &cases, &songs).is_err());
        // Blank lines are skipped; a case written twice is refused.
        assert!(read_dump("before.tsv", &format!("\n{text}\n\n"), &cases, &songs).is_ok());
        let twice = read_dump("before.tsv", &format!("{text}{text}"), &cases, &songs).err().unwrap();
        assert!(twice.contains("case 0 is there twice"), "{twice}");
    }

    #[test]
    fn a_set_that_got_worse_beyond_its_range_is_named() {
        let songs: Vec<Song> = (0..60)
            .map(|i| Song {
                name: format!("pop909/{i:03}"),
                fold: 1,
                learn: true,
                weight: [1.0; 5],
                draw: i,
                music: i,
                copies: 1,
                ..Song::default()
            })
            .collect();
        let cases: Vec<Case> = (0..60).map(song_case).collect();
        let sure: Vec<Option<[f64; 24]>> = (0..60).map(|_| c_or_a(0.9)).collect();
        let was = read_dump("before.tsv", &dump_text(&cases, &sure, &songs, |_| true), &cases, &songs).unwrap();
        // The same chances: nothing got worse.
        assert!(against(&cases, &sure, &songs, |_| true, &[Input::Full], &was).is_empty());
        // A third of the songs now wrong: the share right and the log-loss both got worse beyond their ranges.
        let worse: Vec<Option<[f64; 24]>> = (0..60).map(|i| c_or_a(if i % 3 == 0 { 0.3 } else { 0.9 })).collect();
        let named = against(&cases, &worse, &songs, |_| true, &[Input::Full], &was);
        assert_eq!(named.len(), 2, "{named:?}");
        assert!(named[0].starts_with("Whole songs with one key, full mix: -33.3 points right"), "{named:?}");
        assert!(named[1].contains("log-loss +"), "{named:?}");
        // A dump cut short: the cases it lacks are named, not quietly left out.
        let short: String =
            dump_text(&cases, &sure, &songs, |_| true).lines().take(41).map(|l| format!("{l}\n")).collect();
        let was = read_dump("before.tsv", &short, &cases, &songs).unwrap();
        let named = against(&cases, &sure, &songs, |_| true, &[Input::Full], &was);
        assert_eq!(named, ["Whole songs with one key, full mix: 20 of 60 cases not in both runs, left out"]);
    }

    /// A piece from a source with a licence: one note `n` ticks long on `key`, so pieces whose notes are as long are
    /// the same notes in another key.
    fn piece_on(source: &str, id: &str, n: i64, key: u8, licence: &str) -> Item {
        serde_json::from_str(&format!(
            r#"{{"source":"{source}","id":"{id}","ppq":96,"length":4096,"licence":"{licence}",
                "parts":[{{"role":"melody","notes":[[0,{n},{key},100]]}}],"keys":[[0,4096,"C:maj"]]}}"#
        ))
        .unwrap()
    }

    /// A piece from a source with a licence, its one note as long as `n` ticks so no two share a shape.
    fn piece(source: &str, n: i64, licence: &str) -> Item {
        piece_on(source, &format!("{n:03}"), n, 60, licence)
    }

    #[test]
    fn pieces_only_measured_with_the_notes_of_one_learned_from_are_left_out() {
        let mut items = vec![
            piece_on("a", "1", 100, 60, "MIT"),
            piece_on("a", "2", 100, 64, "MIT"),
            piece_on("b", "1", 100, 62, "measuring only"),
            piece_on("b", "2", 200, 62, "measuring only"),
        ];
        let left_out = leave_out_measured_twins(&mut items, Learning::Licensed);
        // a/2 is learned from too, so it stays; b/1 is a/1 a tone up, so it goes; b/2's notes are its own.
        let kept: Vec<String> = items.iter().map(|i| format!("{}/{}", i.source, i.id)).collect();
        assert_eq!(kept, ["a/1", "a/2", "b/2"]);
        assert_eq!(left_out, HashMap::from([("b".to_owned(), 1)]));
    }

    #[test]
    fn a_run_with_nothing_to_learn_from_stops() {
        let song = |learn, w| Song { fold: 1, learn, weight: [w; 5], ..Song::default() };
        assert!(check_learning(&[song(false, 0.0)], false).unwrap_err().contains("licence"));
        assert!(check_learning(&[song(false, 0.0)], true).unwrap_err().contains("theorybench packs"));
        assert!(check_learning(&[song(true, 0.0)], false).unwrap_err().contains("weighed 0"));
        assert!(check_learning(&[song(false, 0.0), song(true, 0.0), song(true, 1.0)], false).is_ok());
        // A rescale's pieces all in one fold (one pack) can't be scored by a fit on the others.
        assert!(check_learning(&[song(true, 1.0), song(true, 1.0)], true).unwrap_err().contains("1 of folds 1 to 4"));
        let other = Song { fold: 3, ..song(true, 1.0) };
        assert!(check_learning(&[song(true, 1.0), other], true).is_ok());
    }

    #[test]
    fn the_final_finder_scores_the_kept_back_fold_and_the_songs_only_measured() {
        let song = |fold, learn| Song { fold, learn, weight: [1.0; 5], ..Song::default() };
        assert!(song(FROZEN, true).unseen_by_final());
        assert!(song(2, false).unseen_by_final());
        assert!(!song(2, true).unseen_by_final());
    }

    #[test]
    fn only_shippable_licences_are_learned_from_and_the_rest_stay_out_of_the_kept_back_fold() {
        let items: Vec<Item> = (1..=200)
            .map(|n| if n % 2 == 0 { piece("a", n, "MIT") } else { piece("b", n, "measuring only") })
            .collect();
        let sources = vec!["a".to_owned(), "b".to_owned()];
        let weigh = HashMap::from([("a".to_owned(), 0.5), ("a/upper".to_owned(), 0.0)]);
        let songs = songs_of(&items, &sources, &weigh, Learning::Licensed);
        for (item, song) in items.iter().zip(&songs) {
            assert_eq!(song.learn, item.source == "a");
            assert_eq!(song.group, usize::from(item.source == "b"));
            if song.learn {
                assert_eq!((song.weight(Input::Full), song.weight(Input::Upper)), (0.5, 0.0));
            } else {
                assert_ne!(song.fold, FROZEN, "{} is only measured", song.name);
                assert!(Input::ALL.iter().all(|&i| song.weight(i) == 0.0));
            }
        }
        // The kept-back fold still gets its share of the songs learned from.
        assert!(songs.iter().any(|s| s.learn && s.fold == FROZEN));
    }

    #[test]
    fn each_source_has_sets_of_its_own_weighed_as_asked() {
        // Songs 0 to 2 from one source, 3 and 4 from another weighed 0.5: each source's whole songs add up to the
        // songs' set weight times its own.
        let songs: Vec<Song> = (0..5)
            .map(|i| Song {
                name: format!("s/{i}"),
                fold: 1,
                group: usize::from(i >= 3),
                learn: true,
                weight: [if i >= 3 { 0.5 } else { 1.0 }; 5],
                draw: i,
                music: i,
                copies: 1,
                ..Song::default()
            })
            .collect();
        let cases: Vec<Case> = (0..5).map(song_case).collect();
        let w = case_weights(&cases, &songs, |_| true);
        assert!((w[..3].iter().sum::<f64>() - SONG_WEIGHT).abs() < 1e-12, "{w:?}");
        assert!((w[3..].iter().sum::<f64>() - 0.5 * SONG_WEIGHT).abs() < 1e-12, "{w:?}");
        // A song only measured counts nothing, and the songs learned from share their set's whole weight.
        let mut measured = songs;
        measured[4].learn = false;
        measured[4].weight = [0.0; 5];
        let w = case_weights(&cases, &measured, |_| true);
        assert_eq!(w[4], 0.0);
        assert!((w[3] - 0.5 * SONG_WEIGHT).abs() < 1e-12, "{w:?}");
        // Songs 1 and 2 are one song in two keys: they split one song's share, and the set still adds up.
        let mut twins = measured;
        for i in [1, 2] {
            twins[i].music = 1;
            twins[i].copies = 2;
        }
        let w = case_weights(&cases, &twins, |_| true);
        assert!((w[1] + w[2] - w[0]).abs() < 1e-12 && (w[1] - w[2]).abs() < 1e-12, "{w:?}");
        assert!((w[..3].iter().sum::<f64>() - SONG_WEIGHT).abs() < 1e-12, "{w:?}");
    }

    #[test]
    fn folds_are_drawn_by_name() {
        // Fixed for good: changing the hash would move songs in and out of the kept-back fold.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn dates_are_written_year_first() {
        let today = today();
        assert!(today.len() == 10 && today.starts_with("20") && today.as_bytes()[4] == b'-');
    }

    #[test]
    fn the_weights_file_carries_the_version_its_numbers_belong_to() {
        let opts = Options { lambda_delta: Some(LAMBDA_DELTA), loop_cut: Cut::Opening(1), rescale: false };
        let summary =
            Summary { lines: vec!["Whole songs with one key: 90.0% right (1 cases)".into()], full: Vec::new() };
        let calibration = *keyfind::SHIPPED.calibration;
        let answers = Fitted {
            thresholds: *keyfind::SHIPPED.thresholds,
            not_tonal_below: keyfind::SHIPPED.not_tonal_below,
            scale_line: "Notes in one scale a key needs: 0.750.".into(),
            knot_lines: vec!["1 bar (10 cases): one key from 0.85.".into()],
        };
        let file = |weights: [f64; FEATURES]| {
            let x = Learned { name: "x".into(), how: "x at 0.5".into(), credit: "x: CC0".into() };
            weights_file(&Model { weights }, &calibration, &answers, &opts, &[x], ("x", &summary), None)
        };
        // The numbers built in: the hash and version built in.
        let same = file(*keyfind::SHIPPED.weights);
        assert!(
            same.contains("//! Fitted on: x at 0.5 (") && same.contains("//! - x: CC0\n//!\n//! Only these"),
            "{same}"
        );
        assert!(same.contains("//! Right on x songs held out of the fit (cross-validation by song, every fold):\n"));
        assert!(same.contains("//! - Whole songs with one key: 90.0% right (1 cases)\n"));
        assert!(same.contains("pub(super) const LEARNED_FROM: &[&str] = &[\"x\"];\n"), "{same}");
        assert!(same.contains("/// - 1 bar (10 cases): one key from 0.85.\n"), "{same}");
        assert!(same.contains("/// Notes in one scale a key needs: 0.750.\npub(super) const NOT_TONAL_BELOW"));
        let (hash, version) = keyfind::SHIPPED_HASH;
        assert!(same.contains(&format!("SHIPPED_HASH: u64 = {hash:#018x};")), "{same}");
        assert!(same.contains(&format!("HASH_VERSION: u32 = {version};")));
        assert!(same.contains("pub(super) const THRESHOLDS: Thresholds = Thresholds {"));
        assert!(same.contains(&format!("NOT_TONAL_BELOW: f64 = {:?};", keyfind::SHIPPED.not_tonal_below)));
        // Any other number: the next version, so the tests fail until the crate's version is bumped too.
        let mut weights = *keyfind::SHIPPED.weights;
        weights[5] += 0.25;
        let changed = file(weights);
        assert!(changed.contains(&format!("HASH_VERSION: u32 = {};", KEY_FINDER_VERSION + 1)));
        assert!(!changed.contains(&format!("{hash:#018x}")));
    }

    /// Songs learned from, all in fold 1.
    fn learned_songs(n: usize) -> Vec<Song> {
        (0..n)
            .map(|i| Song {
                name: format!("packs/{i}"),
                fold: 1,
                learn: true,
                weight: [1.0; 5],
                draw: i,
                music: i,
                copies: 1,
                ..Song::default()
            })
            .collect()
    }

    /// A case whose only evidence is a tune: the major scale on `tonic`, a stray note on every fifth, labelled `truth`.
    fn tune_case(song: usize, tonic: usize, truth: Key, cut: Cut) -> Case {
        let mut x = [[0.0; 12]; STREAMS];
        for (n, d) in [0, 2, 4, 5, 7, 9, 11].into_iter().enumerate() {
            x[0][(tonic + d) % 12] = 1.0 + ((song + n) % 3) as f64 * 0.25;
        }
        if song.is_multiple_of(5) {
            x[0][(tonic + 1) % 12] = 0.5;
        }
        let ev = Evidence { x, bars: 2, config: Config::MelodyOnly, diatonic_fit: 0.95, notes: 20 };
        Case { song, truth, cut, input: Input::Full, one_key: true, pick: true, ev, cs: None }
    }

    /// Tunes in C or G major, every fourth one labelled as the relative minor.
    fn tune_cases(cut: Cut) -> Vec<Case> {
        (0..60)
            .map(|i| {
                let tonic = if i % 2 == 0 { 0 } else { 7 };
                let major = Key::major(PitchClass::new(tonic as i32));
                tune_case(i, tonic, if i % 4 == 3 { major.relative() } else { major }, cut)
            })
            .collect()
    }

    #[test]
    fn a_rescale_scales_only_the_streams_its_pieces_sound() {
        // A bass under ten of the tunes: too few to scale the bass stream by.
        let mut cases = tune_cases(Cut::Loop);
        for c in cases.iter_mut().take(10) {
            c.ev.x[Stream::Bass as usize][c.truth.tonic.index()] = 1.0;
        }
        let (model, _, n) = rescale(&cases, &learned_songs(60), &|_| true).unwrap();
        assert_eq!(n, 60);
        let scales = scales_of(&model);
        assert!(scales[0] > 0.0 && (scales[0] - 1.0).abs() > 1e-6, "{scales:?}");
        // Every other stream keeps the built-in weights exactly: the pieces never sound them.
        for (s, scale) in scales.iter().enumerate().skip(1) {
            assert_eq!(*scale, 1.0, "{s}");
            for i in s * 24..(s + 1) * 24 {
                assert_eq!(model.weights[i].to_bits(), keyfind::SHIPPED.weights[i].to_bits());
            }
        }
        // The tune's weights keep their shape: each is the built-in one times the same number.
        for i in 0..24 {
            let built_in = keyfind::SHIPPED.weights[i];
            assert!((model.weights[i] - scales[0] * built_in).abs() < 1e-9 * built_in.abs().max(1.0));
        }
        // Only 15 of the tunes are labelled minor: the minor weight keeps its built-in number.
        assert_eq!(model.weights[MINOR].to_bits(), keyfind::SHIPPED.weights[MINOR].to_bits());
        assert!(scales_line(&model).ends_with(" (as built in)"), "{}", scales_line(&model));
        // Half of them minor (30 of each): it's fitted.
        let halves: Vec<Case> = (0..60)
            .map(|i| {
                let tonic = if i % 2 == 0 { 0 } else { 7 };
                let major = Key::major(PitchClass::new(tonic as i32));
                tune_case(i, tonic, if i % 4 >= 2 { major.relative() } else { major }, Cut::Loop)
            })
            .collect();
        let (model, _, _) = rescale(&halves, &learned_songs(60), &|_| true).unwrap();
        assert_ne!(model.weights[MINOR], keyfind::SHIPPED.weights[MINOR]);
        assert!(!scales_line(&model).ends_with(" (as built in)"));
    }

    #[test]
    fn a_rescale_keeps_the_calibration_terms_its_pieces_never_have() {
        // Two-bar windows of full mixes: only the 2-bar knot is heard (no loop, no song, no part alone).
        let cases = tune_cases(Cut::Window(2))
            .into_iter()
            .map(|c| Case { ev: Evidence { config: Config::Full, ..c.ev }, ..c })
            .collect::<Vec<_>>();
        let songs = learned_songs(60);
        let shipped = Model { weights: *keyfind::SHIPPED.weights };
        let held: Held = cases.iter().map(|c| Some(shipped.scores(&c.ev))).collect();
        let opts = |rescale| Options { lambda_delta: None, loop_cut: Cut::Opening(1), rescale };
        let built_in = keyfind::SHIPPED.calibration.0;
        let c = calibrate(&cases, &songs, &held, |_| true, &opts(true)).unwrap();
        for t in (0..CALIBRATION_TERMS).filter(|&t| t != 1) {
            assert_eq!(c.0[t].to_bits(), built_in[t].to_bits(), "term {t}");
        }
        assert_ne!(c.0[1], built_in[1]);
        // A plain run fits every term: those no case has go to 0.
        let plain = calibrate(&cases, &songs, &held, |_| true, &opts(false)).unwrap();
        assert!(plain.0[CALIBRATION_TERMS - 1].abs() < 1e-9 && plain.0[1] == c.0[1], "{plain:?}");
        // Tunes alone: their own number is fitted, the curve (which full mixes read) kept.
        let alone = tune_cases(Cut::Window(2));
        let held: Held = alone.iter().map(|c| Some(shipped.scores(&c.ev))).collect();
        let c = calibrate(&alone, &songs, &held, |_| true, &opts(true)).unwrap();
        let melody_only = keyfind::KNOTS;
        for t in (0..CALIBRATION_TERMS).filter(|&t| t != melody_only) {
            assert_eq!(c.0[t].to_bits(), built_in[t].to_bits(), "term {t}");
        }
        assert_ne!(c.0[melody_only], built_in[melody_only]);
        // Three-bar windows are part 2 bars (0.415 each: 25 pieces' worth of 60), part 4 (0.585: 35): only the 4-bar
        // knot is fitted.
        let threes = tune_cases(Cut::Window(2))
            .into_iter()
            .map(|c| Case { ev: Evidence { config: Config::Full, bars: 3, ..c.ev }, ..c })
            .collect::<Vec<_>>();
        let c = calibrate(&threes, &songs, &held, |_| true, &opts(true)).unwrap();
        assert_eq!(c.0[1].to_bits(), built_in[1].to_bits());
        assert_ne!(c.0[2], built_in[2]);
    }

    #[test]
    fn a_rescale_learns_only_from_solomons_own_pieces_and_a_packs_files_share_a_fold() {
        let mut items: Vec<Item> = (1..=40).map(|n| piece("pop909cl", n, "MIT")).collect();
        for n in 41..=120 {
            let mut own = piece("packs", n, item::OWN);
            own.group = format!("Pack {}", n % 7);
            items.push(own);
        }
        let sources = vec!["pop909cl".to_owned(), "packs".to_owned()];
        for learning in [Learning::Licensed, Learning::Own] {
            let songs = songs_of(&items, &sources, &HashMap::new(), learning);
            for (item, song) in items.iter().zip(&songs) {
                let learn = match learning {
                    Learning::Licensed => item.may_learn(),
                    Learning::Own => item.is_own(),
                };
                assert_eq!(song.learn, learn, "{}", song.name);
                // Each piece keeps its own name; only its fold is drawn by its pack.
                assert_eq!(song.name, fold_name(item));
                assert!(!item.is_own() || song.name == format!("packs/{}", item.id));
            }
            for pack in 0..7 {
                let group = format!("Pack {pack}");
                let of = || items.iter().zip(&songs).filter(|(i, _)| i.group == group).map(|(_, s)| s);
                let folds: HashSet<usize> = of().map(|s| s.fold).collect();
                assert_eq!(folds.len(), 1, "{group}: {folds:?}");
                // The ranges draw a pack again as one, as its fold was drawn; each file is still music of its own.
                assert_eq!(of().map(|s| s.draw).collect::<HashSet<_>>().len(), 1, "{group}");
                assert!(of().map(|s| s.music).collect::<HashSet<_>>().len() > 1, "{group}");
            }
        }
        // Only measured, Solomon's pieces never sit in the kept-back fold.
        let plain = songs_of(&items, &sources, &HashMap::new(), Learning::Licensed);
        assert!(items.iter().zip(&plain).all(|(i, s)| !i.is_own() || s.fold != FROZEN));
        // A run that only scores draws the folds as the run it's compared with: a rescale when his pieces are read.
        assert_eq!(learning_of(false, true, &items), Learning::Own);
        assert_eq!(learning_of(false, true, &items[..40]), Learning::Licensed);
        assert_eq!(learning_of(false, false, &items), Learning::Licensed);
        assert_eq!(learning_of(true, false, &items), Learning::Own);
        // A loop sold in two keys is one piece of music, counted once: its copies split one piece's share.
        let mut twins = items[40..42].to_vec();
        twins[1].parts[0].notes = twins[0].parts[0].notes.iter().map(|&(s, l, k, v)| (s, l, k + 5, v)).collect();
        let songs = songs_of(&twins, &sources, &HashMap::new(), Learning::Own);
        assert_eq!((songs[0].music, songs[0].copies), (songs[1].music, 2));
        assert_eq!(songs[0].draw, songs[1].draw);
    }

    #[test]
    fn a_loop_is_heard_whole_as_a_loop() {
        assert_eq!((Cut::Loop.span(), Cut::Loop.name()), (Span::Loop, "Whole loops".to_owned()));
        assert_eq!(set_weight(Cut::Loop, Input::Full), SONG_WEIGHT);
        let mut item = piece("packs", 400, item::OWN);
        item.whole = Whole::Loop;
        item.parts[0].notes = (0..16).map(|i| (i * 96, 96, [60, 62, 64, 65][i as usize % 4], 100)).collect();
        item.length = 16 * 96;
        let cutting = Cutting { lengths: &[1, 2, 4], inputs: &[Input::Full], picks: 8 };
        let wholes = |item: &Item| -> Vec<Cut> {
            let cases = song_cases(item, 0, "packs/400", &cutting);
            cases.iter().filter(|c| !matches!(c.cut, Cut::Window(_) | Cut::Opening(_))).map(|c| c.cut).collect()
        };
        assert_eq!(wholes(&item), [Cut::Loop]);
        let openings =
            |item: &Item| song_cases(item, 0, "packs/400", &cutting).iter().any(|c| matches!(c.cut, Cut::Opening(_)));
        assert!(openings(&item));
        // A piece the MIDI page cut from a longer part is heard whole as a window, as the page hears it, and its first
        // bars aren't a loop.
        item.whole = Whole::Window;
        assert_eq!(wholes(&item), [Cut::Piece]);
        assert!(!openings(&item));
        assert_eq!((Cut::Piece.span(), set_weight(Cut::Piece, Input::Full)), (Span::Window, 1.0));
        // The field reads from the file, a song when it isn't there.
        let line = r#"{"source":"x","id":"1","ppq":96,"length":96,"parts":[],"whole":"window"}"#;
        assert_eq!(serde_json::from_str::<Item>(line).unwrap().whole, Whole::Window);
        assert_eq!(serde_json::from_str::<Item>(&line.replace(r#","whole":"window""#, "")).unwrap().whole, Whole::Song);
    }

    #[test]
    fn a_rescale_keeps_the_credits_of_the_weights_it_scales() {
        let hash = format!("pub(super) const SHIPPED_HASH: u64 = {:#018x};", keyfind::SHIPPED_HASH.0);
        let file = format!(
            "//! The key finder's weights.\n//!\n//! Fitted on: a, b at 0.25 (penalties).\n//! - A: MIT\n//! - B: CC0\n\
             //!\n//! Only these numbers come from them.\n{hash}\n"
        );
        let path = std::path::Path::new("weights.rs");
        let credits = credits_in(&file, path).unwrap();
        assert_eq!(credits, ["//! Fitted on: a, b at 0.25 (penalties).", "//! - A: MIT", "//! - B: CC0"]);
        // Another file's numbers, or weights rescaled already: refused.
        assert!(credits_in(&file.replace(&hash, "pub(super) const SHIPPED_HASH: u64 = 0x1;"), path).is_err());
        assert!(credits_in(&format!("{file}//! Then rescaled on packs: …\n"), path).is_err());

        let opts = Options { lambda_delta: None, loop_cut: Cut::Opening(1), rescale: true };
        let summary = Summary { lines: vec!["Whole loops: 80.0% right (10 cases)".into()], full: Vec::new() };
        let answers = Fitted {
            thresholds: *keyfind::SHIPPED.thresholds,
            not_tonal_below: keyfind::SHIPPED.not_tonal_below,
            scale_line: "Notes in one scale a key needs: 0.750.".into(),
            knot_lines: vec!["1 bar (10 cases): one key from 0.87.".into()],
        };
        let packs = Learned { name: "packs".into(), how: "packs".into(), credit: credit("packs", item::OWN) };
        let mut weights = *keyfind::SHIPPED.weights;
        weights[0] *= 1.5;
        let written = weights_file(
            &Model { weights },
            keyfind::SHIPPED.calibration,
            &answers,
            &opts,
            &[packs],
            ("packs", &summary),
            Some(&credits),
        );
        assert!(
            written.contains(
                "//! - B: CC0\n//!\n//! Only these numbers come from them.\n//!\n//! Then rescaled on packs:"
            )
        );
        assert!(written.contains("//! Scales: Tune 1.500, Harmony 1.000"), "{written}");
        assert!(
            written.contains("//! Right on packs pieces held out of the fit (cross-validation by pack, every fold):")
        );
        let mut names: Vec<String> = keyfind::LEARNED_FROM.iter().map(|n| format!("{n:?}")).collect();
        names.push("\"packs\"".into());
        assert!(written.contains(&format!("LEARNED_FROM: &[&str] = &[{}];", names.join(", "))), "{written}");
        assert!(written.contains("kept as they were\n/// fitted before the rescale"), "{written}");
        assert!(written.contains(&format!("HASH_VERSION: u32 = {};", KEY_FINDER_VERSION + 1)));
    }
}
