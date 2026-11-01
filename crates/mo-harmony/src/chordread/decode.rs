//! Where the chords change, and which chord each stretch is: the second of the reader's three steps
//! (harmony/design/chord-reader-design.md, "Decoding").
//!
//! The music is cut into spans at the places a chord may change: where a chord or bass note is struck, at each bar
//! line, and at each felt beat a chord may be pushed to, which is read both ways (pushed or not). A span is at most
//! two bars long ([`SPAN_BARS`]). Every way of cutting the music into spans, with a reading on each, is scored, and the
//! best is reported ([`Decoder`]):
//!
//! - **Pass A** scores the readings worth scoring on each span on its own ([`SpanFacts::rank`]) and keeps the best
//!   eight ([`KEEP`]), the best rootless one, and no chord (N), which scores w_N for each felt beat the span sounds.
//! - **Pass B** finds the best way through the whole (Viterbi, exact on what pass A kept), adding the pair term for
//!   each change of chord ([`Tables::pair_prior`] times its trust): how much likelier the way the root moves is than a
//!   move picked at random. A move into or out of no chord adds nothing, as a move nothing is known about wouldn't.
//! - A chord held longer than a span is the same reading again, held on. It may only follow a span of the full two
//!   bars, so no way of reading the music is counted twice, and it scores only what counts for each felt beat
//!   ([`SpanFacts::held_score`]): where it starts was counted where it started. So a chord held, or struck again, for
//!   eight bars is one chord.
//! - **Forward-backward** adds up every way, each scored times the sharpness ([`SHARPNESS`]), into each reading's
//!   chance on each unit and the chance a chord changes at each unit.
//!
//! A pushed chord's sound is its own from the unit it's struck in ([`super::Evidence::facts`]), so pushing a chord
//! and changing early weigh the same notes, and only where the chord starts tells them apart (a push costs E8, a
//! change between beats E7).
//!
//! Silence of a bar or more (nothing in the chords, the bass or the tune) is a rest, read as no chord; a silence
//! shorter than that belongs to the chords around it, so no chord is only offered on a span that sounds (or on the
//! silence before the music starts, from the bar line before its first note). Stretches given by the caller
//! ([`PinnedSpan`]) are read as given: as a chord, or as one chord between the two places given. Every way changes
//! chord where a rest or a pin starts and ends, so any reading may follow any other there, even the same one.
//!
//! The best way is reported with the spans that read the same merged, though never across a rest's or a pin's edge,
//! each with the chances of its readings (each reading's chance on each of its units, averaged) and the chance a chord
//! changes where it starts. Every number is worked out in a fixed order, and ties are broken in an order that moves
//! with the music (the readings' order from the lowest note up, [`super::candidates()`]), so the same notes moved read
//! the same, moved, to the last bit; and the same music at another resolution or in another order reads the same.

use std::ops::{Index, IndexMut, Range};

use super::candidates::{Candidate, Label, Route};
use super::cores::{Core, Group};
use super::hear::{Hearing, Level, Mass, Stream};
use super::score::{Evidence, PerFeature, Scored, SpanFacts};
use super::weights::{Tables, Weights};
use crate::pitch::PitchClass;

/// The longest span, in bars: a chord held longer is read as the same chord again, held on.
pub const SPAN_BARS: usize = 2;

/// How many readings pass A keeps on each span (the design's K), besides its best rootless one and no chord.
pub const KEEP: usize = 8;

/// How sharp the chances are (the design's σ): every score is multiplied by it before the chances are worked out.
/// 1 until the calibration fits it.
pub const SHARPNESS: f64 = 1.0;

/// A stretch whose reading is given, by a Chord check answer or the user's fix: a control, not evidence.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PinnedSpan {
    /// Its units, from the first unit heard (a loop's copied bar included).
    pub units: Range<usize>,
    /// Its chord. None pins only where it starts and ends: it's then read as one chord, whichever reads best.
    pub label: Option<Label>,
}

/// A reading and its chance: a label, or none for no chord.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Chance {
    pub label: Option<Label>,
    pub chance: f64,
}

/// A stretch the best way reads as one chord, or as no chord.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedSpan {
    /// Its units, from the first unit heard, inside the units reported ([`Hearing::reported`]).
    pub units: Range<usize>,
    /// The units whose sound is its own: a unit before its first when its chord is pushed, and a unit short when the
    /// next one is (inside the units reported).
    pub sound: Range<usize>,
    /// Whether its chord is pushed: struck in the unit before its first and held over it, read from its first.
    pub pushed: bool,
    /// Its reading: a label, or none for no chord.
    pub label: Option<Label>,
    /// Whether it's a rest: silent for a bar or more.
    pub rest: bool,
    /// The chance a chord starts where it starts, over every way of reading the music: low where the best way starts
    /// one but others hold the chord before on.
    pub change: f64,
    /// Its readings' places in [`Decoding::chances`].
    chances: Range<usize>,
}

/// What the decode read ([`Decoder::decode`]): the spans of the best way, and each one's readings with their chances.
/// Keep one and reuse it.
#[derive(Clone, Debug, Default)]
pub struct Decoding {
    spans: Vec<DecodedSpan>,
    chances: Vec<Chance>,
}

impl Decoding {
    pub fn new() -> Decoding {
        Decoding::default()
    }

    /// The spans of the best way, in order: the next starts where one ends, from the first unit reported to the last.
    pub fn spans(&self) -> &[DecodedSpan] {
        &self.spans
    }

    /// A span's readings with their chances, best first: each reading's chance on each of the span's units of sound,
    /// averaged. Equal chances keep the readings' order from the span's lowest note up, no chord last.
    pub fn chances(&self, span: &DecodedSpan) -> &[Chance] {
        self.chances.get(span.chances.clone()).unwrap_or_default()
    }

    fn clear(&mut self) {
        self.spans.clear();
        self.chances.clear();
    }
}

/// A reading in the decode: a label (its root's number times 16, plus its core's place in [`Core::ALL`]), or no
/// chord. Only [`Id::of`] makes one, so it's always below [`Id::COUNT`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Id(u8);

impl Id {
    const NONE: Id = Id(192);
    const COUNT: usize = 193;

    fn of(label: Option<Label>) -> Id {
        label.map_or(Id::NONE, |l| Id(l.root.number() * 16 + l.core.index() as u8))
    }

    fn label(self) -> Option<Label> {
        if self == Id::NONE {
            return None;
        }
        let core = Core::ALL.get(usize::from(self.0 % 16)).copied()?;
        Some(Label::new(PitchClass::new(i32::from(self.0 / 16)), core))
    }
}

/// A value for each reading, picked by [`Id`] (so a pick can never miss).
#[derive(Clone, Copy, Debug)]
struct PerId<T>([T; Id::COUNT]);

impl<T: Copy> PerId<T> {
    fn filled(value: T) -> PerId<T> {
        PerId([value; Id::COUNT])
    }
}

impl<T: Copy + Default> Default for PerId<T> {
    fn default() -> PerId<T> {
        PerId::filled(T::default())
    }
}

impl<T> Index<Id> for PerId<T> {
    type Output = T;
    // An id is always below COUNT.
    #[allow(clippy::indexing_slicing)]
    fn index(&self, id: Id) -> &T {
        &self.0[usize::from(id.0)]
    }
}

impl<T> IndexMut<Id> for PerId<T> {
    #[allow(clippy::indexing_slicing)]
    fn index_mut(&mut self, id: Id) -> &mut T {
        &mut self.0[usize::from(id.0)]
    }
}

/// The pair term for each move, times its trust: by the group of the chord before, how far its root moves up, and the
/// group of the chord after ([`Tables::motion_prior`]).
#[derive(Clone, Copy, Debug, Default)]
struct Moves {
    motion: [[[f64; 3]; 12]; 3],
    /// Each core's group, by its place in [`Core::ALL`].
    groups: [usize; 16],
}

impl Moves {
    fn new(trust: f64, tables: &Tables) -> Moves {
        let mut motion = [[[0.0; 3]; 12]; 3];
        for (from, row) in Group::ALL.into_iter().zip(&mut motion) {
            for (up, cell) in (0u8..).zip(row.iter_mut()) {
                for (to, value) in Group::ALL.into_iter().zip(cell) {
                    *value = trust * tables.motion_prior(from, up, to);
                }
            }
        }
        let mut groups = [0; 16];
        for (core, group) in Core::ALL.into_iter().zip(&mut groups) {
            *group = core.group().index();
        }
        Moves { motion, groups }
    }

    /// The pair term for the reading `from` followed by `to`: 0 for the same reading again (the chord held on), and
    /// for a move into or out of no chord, which is no likelier or rarer than any other.
    fn between(&self, from: Id, to: Id) -> f64 {
        if from == to || from == Id::NONE || to == Id::NONE {
            return 0.0;
        }
        let group = |id: Id| self.groups.get(usize::from(id.0 % 16)).copied().unwrap_or(0);
        let up = (to.0 / 16 + 12 - from.0 / 16) % 12;
        let row = self.motion.get(group(from));
        let cell = row.and_then(|row| row.get(usize::from(up)));
        cell.and_then(|cell| cell.get(group(to))).copied().unwrap_or(0.0)
    }
}

/// ln (e^a + e^b), worked out so it never overflows; ln 0 is minus infinity.
fn add_ln(a: f64, b: f64) -> f64 {
    if a == f64::NEG_INFINITY {
        return b;
    }
    if b == f64::NEG_INFINITY {
        return a;
    }
    let (high, low) = if a >= b { (a, b) } else { (b, a) };
    high + (low - high).exp().ln_1p()
}

/// What a stretch is given as, where it isn't read freely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Given {
    /// Silent for a bar or more: no chord.
    Rest,
    /// A pinned chord.
    Chord(Label),
    /// Pinned only where it starts and ends: one chord, whichever reads best.
    Ends,
}

/// A rest or a pin.
#[derive(Clone, Debug)]
struct Region {
    units: Range<usize>,
    given: Given,
    /// For a stretch pinned only where it starts and ends, the readings kept for all of it: their places in
    /// `Decoder::region_ids`.
    ids: Range<usize>,
}

/// What may happen at a place a chord may change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Where the music starts: nothing comes before.
    Start,
    /// Anything may change, but the same reading may only follow a span of the full length.
    Free,
    /// Where a rest or a pin starts or ends: every way changes here, so any reading may follow any other.
    Edge,
    /// Inside a rest or a pin, every two bars: only the same reading may follow.
    Inside,
    /// Where the music ends.
    End,
}

/// A place a chord may change: a unit's start, read plain or with a chord pushed to it.
#[derive(Clone, Debug)]
struct Node {
    unit: usize,
    /// Whether a chord is pushed to it (struck in the unit before it and held over), as its place in
    /// [`Hearing::pushes`].
    push: Option<usize>,
    kind: Kind,
    /// Where a span from it may end: exactly there, from the start of a rest or a pin or inside one (`exact`);
    /// otherwise anywhere up to there.
    reach: usize,
    exact: bool,
    /// The rest or pin it starts or is inside, when `exact`.
    region: Option<usize>,
    /// The spans that end at it, and the readings they bring here (places in `Decoder::spans` and
    /// `Decoder::entries`); the spans that start at it (places in `Decoder::by_start`).
    into: Range<usize>,
    entries: Range<usize>,
    out: Range<usize>,
}

impl Node {
    fn pushed(&self) -> bool {
        self.push.is_some()
    }

    /// Where the sound of the span after it starts, and of the span before it ends: a unit early when a chord is pushed
    /// to it.
    fn sound(&self) -> usize {
        self.unit - usize::from(self.pushed() && self.unit > 0)
    }
}

/// A span between two places, and the readings kept on it.
#[derive(Clone, Debug)]
struct Span {
    /// Its first and last places, as places in `Decoder::nodes`.
    from: usize,
    to: usize,
    /// Whether it's as long as a span may be: then the same reading may follow it.
    full: bool,
    rest: bool,
    /// Its readings, as places in `Decoder::kept`.
    kept: Range<usize>,
}

/// A reading kept on a span.
#[derive(Clone, Copy, Debug)]
struct Kept {
    id: Id,
    span: usize,
    /// ψ: its score on the span; and its score when the same reading is held on into the span from before, which
    /// leaves out what counts where a chord starts ([`SpanFacts::held_score`]).
    score: f64,
    held: f64,
    /// The ln of the forward mass of the ways that reach the span's start and go on as it: those that start it there
    /// (a new chord, or a chord again where a rest or pin starts or ends), those that hold it on into the span, and
    /// those that change chord there.
    into: f64,
    into_held: f64,
    change: f64,
    /// The reading it brings to the span's end (a place in `Decoder::entries`), when a way reaches it.
    entry: Option<usize>,
    /// μ: the chance the music is read with this span read as it.
    chance: f64,
}

/// Where the best way to a reading at a place came from: the reading kept on its last span (a place in
/// `Decoder::kept`), and the reading at the span's start it followed (and whether through that one's full-length
/// best), none at the start of the music.
#[derive(Clone, Copy, Debug, Default)]
struct Back {
    kept: usize,
    from: Option<(usize, bool)>,
}

/// A reading at a place, brought by the spans that end there read as it.
#[derive(Clone, Copy, Debug)]
struct Entry {
    id: Id,
    /// The best score of a way that ends here read as it, by any span and by a span of the full length, and where
    /// each came from.
    best: f64,
    best_full: f64,
    back: Back,
    back_full: Back,
    /// The ln of the forward mass of those ways (each scored times the sharpness), and of the backward mass after.
    alpha: f64,
    alpha_full: f64,
    beta: f64,
    beta_full: f64,
}

impl Entry {
    fn new(id: Id) -> Entry {
        let none = f64::NEG_INFINITY;
        Entry {
            id,
            best: none,
            best_full: none,
            back: Back::default(),
            back_full: Back::default(),
            alpha: none,
            alpha_full: none,
            beta: none,
            beta_full: none,
        }
    }
}

/// What the ways that reach a place bring to a span from it read as some reading: for the ways that start the reading
/// there and for those that hold it on, the best score and where it came from, and the ln of their forward mass; and
/// the ln of the forward mass of those that change chord there.
#[derive(Clone, Copy, Debug)]
struct Ready {
    best: f64,
    from: Option<(usize, bool)>,
    alpha: f64,
    held_best: f64,
    held_from: Option<(usize, bool)>,
    held_alpha: f64,
    change: f64,
}

impl Default for Ready {
    fn default() -> Ready {
        let none = f64::NEG_INFINITY;
        Ready { best: none, from: None, alpha: none, held_best: none, held_from: None, held_alpha: none, change: none }
    }
}

/// What the ways that reach a place bring to a reading of a span from it, from the readings there (`entries`, the
/// first at `first` in `Decoder::entries`).
fn ready_from(entries: &[Entry], first: usize, id: Id, kind: Kind, moves: &Moves) -> Ready {
    let mut out = Ready::default();
    for (e, entry) in (first..).zip(entries) {
        // The first of equals: the readings come in an order that moves with the music.
        if entry.id == id && kind != Kind::Edge {
            // The same reading held on, after a span of the full length.
            if entry.best_full > out.held_best {
                out.held_best = entry.best_full;
                out.held_from = Some((e, true));
            }
            out.held_alpha = add_ln(out.held_alpha, entry.alpha_full);
            continue;
        }
        if kind == Kind::Inside {
            continue;
        }
        // Another reading, or at a rest's or a pin's edge, where every way starts a chord, any reading after any span.
        let pair = moves.between(entry.id, id);
        let (best, alpha) = (entry.best + pair, entry.alpha + SHARPNESS * pair);
        if best > out.best {
            out.best = best;
            out.from = Some((e, false));
        }
        out.alpha = add_ln(out.alpha, alpha);
        if entry.id != id {
            out.change = add_ln(out.change, alpha);
        }
    }
    out
}

/// What the ways that reach one place bring to each reading, worked out once each.
#[derive(Clone, Debug, Default)]
struct Slot {
    owner: Option<usize>,
    done: PerId<bool>,
    ready: PerId<Ready>,
}

/// Whether nothing at all sounds in a unit: no chord, bass or tune note, nor what the chords hold over.
fn silent(hearing: &Hearing, unit: usize) -> bool {
    [Mass::Harmony, Mass::Bass, Mass::Melody, Mass::HeldOver]
        .into_iter()
        .all(|mass| hearing.mass(unit..unit + 1, mass).total() == 0)
}

/// The readings pass A keeps from a ranking, with their scores and held-on scores: the best [`KEEP`], and the best
/// rootless one after them.
fn keep(ranked: &[Scored], out: &mut Vec<(Id, f64, f64)>) {
    out.extend(ranked.iter().take(KEEP).map(|s| (Id::of(Some(s.label)), s.score, s.held)));
    if let Some(s) = ranked.iter().skip(KEEP).find(|s| s.route == Route::Rootless) {
        out.push((Id::of(Some(s.label)), s.score, s.held));
    }
}

/// Reads where the chords change in some music and which chord each stretch is ([`Decoder::decode`]). Keep one and
/// reuse it: it keeps its working space between reads.
#[derive(Debug, Default)]
pub struct Decoder {
    /// The longest span, in units.
    cap: usize,
    moves: Moves,
    evidence: Evidence,
    facts: SpanFacts,
    candidates: Vec<Candidate>,
    ranked: Vec<Scored>,
    features: PerFeature<f64>,
    labels: Vec<(Id, f64, f64)>,
    regions: Vec<Region>,
    region_ids: Vec<Id>,
    nodes: Vec<Node>,
    spans: Vec<Span>,
    kept: Vec<Kept>,
    entries: Vec<Entry>,
    by_start: Vec<usize>,
    /// ln of the total mass of every way (Z), and the chance a chord changes at each unit.
    total: f64,
    change: Vec<f64>,
    // Working space.
    slots: Vec<Slot>,
    entry_of: PerId<Option<usize>>,
    after: PerId<f64>,
    after_held: PerId<f64>,
    touched: Vec<Id>,
    path: Vec<(usize, Id)>,
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder::default()
    }

    /// Reads where the chords change in what was heard and which chord each stretch is, into `out`, weighing the
    /// evidence by `weights` and the priors by `tables`; `pins` are stretches whose reading is given (one that
    /// overlaps an earlier one is left out). False, with nothing in `out`, when nothing was heard.
    pub fn decode(
        &mut self,
        hearing: &Hearing,
        weights: &Weights,
        tables: &Tables,
        pins: &[PinnedSpan],
        out: &mut Decoding,
    ) -> bool {
        out.clear();
        self.clear();
        let units = hearing.units();
        if units == 0 {
            return false;
        }
        self.cap = SPAN_BARS * hearing.bar_units().max(1);
        self.moves = Moves::new(weights.pair, tables);
        self.evidence.prepare(hearing);
        self.find_regions(hearing, pins, weights, tables);
        self.place(hearing);
        self.forward(hearing, weights, tables);
        let reached = self.nodes.last().is_some_and(|end| !end.entries.is_empty());
        if !reached {
            return false;
        }
        self.backward(units);
        self.report(hearing, out);
        true
    }

    /// Each reading's chance on each of some units, averaged over them, from the last decode of `hearing` (none before
    /// one): best first, equal chances in the readings' order from the lowest note up, no chord last.
    pub fn chances(&self, hearing: &Hearing, units: Range<usize>, out: &mut Vec<Chance>) {
        out.clear();
        self.add_chances(hearing, units, out);
    }

    /// The chance a chord changes at a unit's start, from the last decode (1 where the music starts).
    pub fn change(&self, unit: usize) -> f64 {
        // Never above 1 by a rounding error.
        self.change.get(unit).map_or(0.0, |c| c.min(1.0))
    }

    fn clear(&mut self) {
        self.regions.clear();
        self.region_ids.clear();
        self.nodes.clear();
        self.spans.clear();
        self.kept.clear();
        self.entries.clear();
        self.by_start.clear();
        self.change.clear();
        self.path.clear();
        self.total = f64::NEG_INFINITY;
    }

    /// The rests and the pins, in order: the pins inside what's heard (one that overlaps an earlier one left out), and
    /// the runs of a bar or more of silence outside them.
    fn find_regions(&mut self, hearing: &Hearing, pins: &[PinnedSpan], weights: &Weights, tables: &Tables) {
        let units = hearing.units();
        let mut pinned: Vec<Region> = pins
            .iter()
            .map(|pin| {
                let end = pin.units.end.min(units);
                let given = pin.label.map_or(Given::Ends, Given::Chord);
                Region { units: pin.units.start.min(end)..end, given, ids: 0..0 }
            })
            .filter(|r| !r.units.is_empty())
            .collect();
        pinned.sort_by_key(|r| (r.units.start, r.units.end));
        let mut kept_end = 0;
        pinned.retain(|r| {
            let free = r.units.start >= kept_end;
            if free {
                kept_end = r.units.end;
            }
            free
        });

        let bar = hearing.bar_units().max(1);
        let mut rests: Vec<Region> = Vec::new();
        let mut u = 0;
        while u < units {
            if !silent(hearing, u) {
                u += 1;
                continue;
            }
            let start = u;
            while u < units && silent(hearing, u) {
                u += 1;
            }
            // The run, less the pins in it.
            let mut from = start;
            for pin in pinned.iter().filter(|p| p.units.start < u && p.units.end > start) {
                if pin.units.start >= from + bar {
                    rests.push(Region { units: from..pin.units.start, given: Given::Rest, ids: 0..0 });
                }
                from = from.max(pin.units.end);
            }
            if u >= from + bar {
                rests.push(Region { units: from..u, given: Given::Rest, ids: 0..0 });
            }
        }

        self.regions.extend(pinned);
        self.regions.extend(rests);
        self.regions.sort_by_key(|r| r.units.start);
        // A stretch pinned only at its ends keeps the same readings all through, ranked on all of it.
        for r in 0..self.regions.len() {
            let Some(region) = self.regions.get(r) else { break };
            if region.given != Given::Ends {
                continue;
            }
            self.evidence.facts(hearing, region.units.clone(), None, None, &mut self.facts);
            self.facts.candidates(&mut self.candidates);
            self.facts.rank(&self.candidates, weights, tables, &mut self.ranked);
            self.labels.clear();
            keep(&self.ranked, &mut self.labels);
            let first = self.region_ids.len();
            self.region_ids.extend(self.labels.iter().map(|&(id, _, _)| id));
            if let Some(region) = self.regions.get_mut(r) {
                region.ids = first..self.region_ids.len();
            }
        }
    }

    /// The places a chord may change, in order: where a chord or bass note is struck, each bar line, each felt beat a
    /// chord may be pushed to (plain, then pushed), and the edges of the rests and pins; inside those, only every two
    /// bars from where they start.
    fn place(&mut self, hearing: &Hearing) {
        let units = hearing.units();
        let pushes = hearing.pushes();
        let cap = self.cap;
        // The first rest or pin that doesn't end before the unit.
        let mut r = 0;
        for u in 0..=units {
            while self.regions.get(r).is_some_and(|region| region.units.end < u) {
                r += 1;
            }
            let region = self.regions.get(r);
            if let Some(inside) = region.filter(|region| region.units.start < u && u < region.units.end) {
                if (u - inside.units.start) % cap == 0 {
                    let reach = (u + cap).min(inside.units.end);
                    self.nodes.push(Node::at(u, None, Kind::Inside, reach, Some(r)));
                }
                continue;
            }
            // The rest or pin that starts here, and where the next one starts.
            let mut next = r;
            while self.regions.get(next).is_some_and(|region| region.units.start < u) {
                next += 1;
            }
            let starts = self.regions.get(next).filter(|region| region.units.start == u).map(|_| next);
            let edge = starts.is_some() || region.is_some_and(|region| region.units.end == u);
            let push = pushes.binary_search_by_key(&u, |p| p.unit).ok();
            let struck = hearing.struck(u, Stream::Harmony).union(hearing.struck(u, Stream::Bass));
            let bar = u < units && hearing.level(u) == Level::Bar;
            if !(u == 0 || u == units || edge || !struck.is_empty() || bar || push.is_some()) {
                continue;
            }
            let kind = match u {
                0 => Kind::Start,
                _ if u == units => Kind::End,
                _ if edge => Kind::Edge,
                _ => Kind::Free,
            };
            let (reach, region) = match starts {
                Some(s) => (self.regions.get(s).map_or(u, |region| (u + cap).min(region.units.end)), Some(s)),
                None => {
                    let limit = self.regions.get(next).map_or(units, |region| region.units.start);
                    ((u + cap).min(limit).min(units), None)
                }
            };
            self.nodes.push(Node::at(u, None, kind, reach, region));
            if let (Some(p), true) = (push, u > 0 && u < units) {
                self.nodes.push(Node::at(u, Some(p), kind, reach, region));
            }
        }
    }

    /// Pass A on every span, and the forward pass of pass B and of forward-backward: for each place in order, the
    /// spans that end there, the readings pass A keeps on each, and what every way brings to each reading there.
    fn forward(&mut self, hearing: &Hearing, weights: &Weights, tables: &Tables) {
        let ring = 2 * self.cap + 4;
        self.slots.resize_with(ring, Slot::default);
        for slot in &mut self.slots {
            slot.owner = None;
        }
        let mut lo = 0;
        for j in 1..self.nodes.len() {
            let Some(w) = self.nodes.get(j).cloned() else { break };
            while self.nodes.get(lo).is_some_and(|v| v.unit + self.cap < w.unit) {
                lo += 1;
            }
            let (first_span, first_entry) = (self.spans.len(), self.entries.len());
            self.touched.clear();
            for i in lo..j {
                let Some(v) = self.nodes.get(i).cloned() else { break };
                let reaches = if v.exact { w.unit == v.reach } else { w.unit <= v.reach && w.kind != Kind::Inside };
                if w.unit <= v.unit || !reaches || v.sound() >= w.sound() {
                    continue;
                }
                self.keep_on(hearing, weights, tables, &v, &w);
                let span = self.spans.len();
                let first = self.kept.len();
                let full = w.unit - v.unit == self.cap;
                let rest =
                    v.region.and_then(|r| self.regions.get(r)).is_some_and(|r| r.given == Given::Rest && v.exact);
                for l in 0..self.labels.len() {
                    let Some(&(id, score, held)) = self.labels.get(l) else { break };
                    let ready = self.ready(i, id);
                    let mut kept = Kept {
                        id,
                        span,
                        score,
                        held,
                        into: ready.alpha,
                        into_held: ready.held_alpha,
                        change: ready.change,
                        entry: None,
                        chance: 0.0,
                    };
                    // Started there, or held on into the span: the better, starting it where they tie.
                    let (best, from) = if ready.held_best + held > ready.best + score {
                        (ready.held_best + held, ready.held_from)
                    } else {
                        (ready.best + score, ready.from)
                    };
                    let alpha = add_ln(ready.alpha + SHARPNESS * score, ready.held_alpha + SHARPNESS * held);
                    if best > f64::NEG_INFINITY {
                        let e = match self.entry_of[id] {
                            Some(e) => e,
                            None => {
                                self.entries.push(Entry::new(id));
                                self.entry_of[id] = Some(self.entries.len() - 1);
                                self.touched.push(id);
                                self.entries.len() - 1
                            }
                        };
                        if let Some(entry) = self.entries.get_mut(e) {
                            let back = Back { kept: first + l, from };
                            if best > entry.best {
                                entry.best = best;
                                entry.back = back;
                            }
                            if full && best > entry.best_full {
                                entry.best_full = best;
                                entry.back_full = back;
                            }
                            entry.alpha = add_ln(entry.alpha, alpha);
                            if full {
                                entry.alpha_full = add_ln(entry.alpha_full, alpha);
                            }
                        }
                        kept.entry = Some(e);
                    }
                    self.kept.push(kept);
                }
                self.spans.push(Span { from: i, to: j, full, rest, kept: first..self.kept.len() });
            }
            for &id in &self.touched {
                self.entry_of[id] = None;
            }
            if let Some(node) = self.nodes.get_mut(j) {
                node.into = first_span..self.spans.len();
                node.entries = first_entry..self.entries.len();
            }
        }
    }

    /// The readings pass A keeps on the span from `v` to `w`, with their scores, into `labels`: the given one inside a
    /// rest or a pin; otherwise the best eight, the best rootless one, and no chord where the span sounds or the
    /// music starts.
    fn keep_on(&mut self, hearing: &Hearing, weights: &Weights, tables: &Tables, v: &Node, w: &Node) {
        let pushes = hearing.pushes();
        let (start, end) = (v.push.and_then(|p| pushes.get(p)), w.push.and_then(|p| pushes.get(p)));
        self.evidence.facts(hearing, v.unit..w.unit, start, end, &mut self.facts);
        self.labels.clear();
        let given = v.region.filter(|_| v.exact).and_then(|r| self.regions.get(r));
        match given.map(|r| (r.given, r.ids.clone())) {
            Some((Given::Rest, _)) => {
                let none = self.facts.none_score(weights);
                self.labels.push((Id::NONE, none, none));
            }
            Some((Given::Chord(label), _)) => {
                self.facts.features(label, tables, &mut self.features);
                let (score, held) =
                    (self.facts.score(&self.features, weights), self.facts.held_score(&self.features, weights));
                self.labels.push((Id::of(Some(label)), score, held));
            }
            Some((Given::Ends, ids)) => {
                for &id in self.region_ids.get(ids).unwrap_or_default() {
                    if let Some(label) = id.label() {
                        self.facts.features(label, tables, &mut self.features);
                        let (score, held) =
                            (self.facts.score(&self.features, weights), self.facts.held_score(&self.features, weights));
                        self.labels.push((id, score, held));
                    }
                }
                let none = self.facts.none_score(weights);
                self.labels.push((Id::NONE, none, none));
            }
            None => {
                self.facts.candidates(&mut self.candidates);
                self.facts.rank(&self.candidates, weights, tables, &mut self.ranked);
                keep(&self.ranked, &mut self.labels);
                if self.facts.sounding() > 0 || v.kind == Kind::Start {
                    let none = self.facts.none_score(weights);
                    self.labels.push((Id::NONE, none, none));
                }
            }
        }
    }

    /// What the ways that reach place `i` bring to a span from it read as `id`, worked out once.
    fn ready(&mut self, i: usize, id: Id) -> Ready {
        let ring = self.slots.len().max(1);
        let Some(slot) = self.slots.get_mut(i % ring) else { return Ready::default() };
        if slot.owner != Some(i) {
            slot.owner = Some(i);
            slot.done = PerId::filled(false);
        }
        if slot.done[id] {
            return slot.ready[id];
        }
        let value = match self.nodes.get(i) {
            Some(node) if node.kind == Kind::Start => Ready { best: 0.0, alpha: 0.0, change: 0.0, ..Ready::default() },
            Some(node) => {
                let entries = self.entries.get(node.entries.clone()).unwrap_or_default();
                ready_from(entries, node.entries.start, id, node.kind, &self.moves)
            }
            None => Ready::default(),
        };
        slot.done[id] = true;
        slot.ready[id] = value;
        value
    }

    /// The backward pass of forward-backward: for each place from the last, the backward mass after each reading that
    /// ends there; each kept reading's chance (μ), and the chance a chord changes at each unit.
    fn backward(&mut self, units: usize) {
        // The spans by where they start.
        let mut counts = vec![0usize; self.nodes.len() + 1];
        for span in &self.spans {
            if let Some(c) = counts.get_mut(span.from + 1) {
                *c += 1;
            }
        }
        for k in 1..counts.len() {
            let before = counts.get(k - 1).copied().unwrap_or(0);
            if let Some(c) = counts.get_mut(k) {
                *c += before;
            }
        }
        for (node, w) in self.nodes.iter_mut().zip(counts.windows(2)) {
            if let [from, to] = *w {
                node.out = from..to;
            }
        }
        self.by_start.clear();
        self.by_start.resize(self.spans.len(), 0);
        let mut next = counts;
        for (s, span) in self.spans.iter().enumerate() {
            if let Some(place) = next.get_mut(span.from) {
                if let Some(slot) = self.by_start.get_mut(*place) {
                    *slot = s;
                }
                *place += 1;
            }
        }

        let end = self.nodes.last().map_or(0..0, |end| end.entries.clone());
        self.total =
            self.entries.get(end).unwrap_or_default().iter().fold(f64::NEG_INFINITY, |z, e| add_ln(z, e.alpha));
        self.change.clear();
        self.change.resize(units + 1, 0.0);
        let total = self.total;
        for i in (0..self.nodes.len()).rev() {
            let Some(node) = self.nodes.get(i).cloned() else { continue };
            if node.kind == Kind::End {
                for entry in self.entries.get_mut(node.entries.clone()).unwrap_or_default() {
                    entry.beta = 0.0;
                    entry.beta_full = 0.0;
                }
                continue;
            }
            // What follows each reading of a span from here, started here or held on into the span.
            self.touched.clear();
            let mut changes = 0.0;
            for &s in self.by_start.get(node.out.clone()).unwrap_or_default() {
                let Some(span) = self.spans.get(s) else { continue };
                for kept in self.kept.get_mut(span.kept.clone()).unwrap_or_default() {
                    let Some(entry) = kept.entry.and_then(|e| self.entries.get(e)) else { continue };
                    let beta = if span.full { entry.beta_full } else { entry.beta };
                    let (after, held) = (SHARPNESS * kept.score + beta, SHARPNESS * kept.held + beta);
                    if !self.touched.contains(&kept.id) {
                        self.touched.push(kept.id);
                        self.after[kept.id] = f64::NEG_INFINITY;
                        self.after_held[kept.id] = f64::NEG_INFINITY;
                    }
                    self.after[kept.id] = add_ln(self.after[kept.id], after);
                    self.after_held[kept.id] = add_ln(self.after_held[kept.id], held);
                    kept.chance = (add_ln(kept.into + after, kept.into_held + held) - total).exp();
                    changes += (kept.change + after - total).exp();
                }
            }
            if let Some(c) = self.change.get_mut(node.unit) {
                *c += changes;
            }
            // The backward mass after each reading that ends here.
            for entry in self.entries.get_mut(node.entries.clone()).unwrap_or_default() {
                let own = if self.touched.contains(&entry.id) { self.after_held[entry.id] } else { f64::NEG_INFINITY };
                let mut moved = f64::NEG_INFINITY;
                if node.kind != Kind::Inside {
                    for &next in &self.touched {
                        if next != entry.id || node.kind == Kind::Edge {
                            moved = add_ln(moved, SHARPNESS * self.moves.between(entry.id, next) + self.after[next]);
                        }
                    }
                }
                (entry.beta, entry.beta_full) = match node.kind {
                    Kind::Edge => (moved, moved),
                    Kind::Inside => (f64::NEG_INFINITY, own),
                    _ => (moved, add_ln(moved, own)),
                };
            }
        }
    }

    /// The best way, its spans that read the same merged (never across a rest's or a pin's edge) and clipped to the
    /// units reported, with their chances.
    fn report(&mut self, hearing: &Hearing, out: &mut Decoding) {
        // The best way, back from where the music ends.
        let end = self.nodes.last().map_or(0..0, |end| end.entries.clone());
        let mut best: Option<(usize, f64)> = None;
        for (e, entry) in end.clone().zip(self.entries.get(end).unwrap_or_default()) {
            if best.is_none_or(|(_, score)| entry.best > score) {
                best = Some((e, entry.best));
            }
        }
        self.path.clear();
        let mut at = best.map(|(e, _)| (e, false));
        while let Some((e, full)) = at {
            let Some(entry) = self.entries.get(e) else { break };
            let back = if full { entry.back_full } else { entry.back };
            let Some(kept) = self.kept.get(back.kept) else { break };
            self.path.push((kept.span, kept.id));
            at = back.from;
        }
        self.path.reverse();

        let reported = hearing.reported();
        let mut p = 0;
        while let Some(&(first, id)) = self.path.get(p) {
            let rest = self.spans.get(first).is_some_and(|s| s.rest);
            let mut last = first;
            p += 1;
            while let Some(&(s, next)) = self.path.get(p) {
                // Never across a rest's or a pin's edge, so a pin's stretch is reported as given.
                let span = self.spans.get(s);
                let edge = span.and_then(|s| self.nodes.get(s.from)).is_some_and(|n| n.kind == Kind::Edge);
                if next != id || span.is_some_and(|s| s.rest) != rest || edge {
                    break;
                }
                last = s;
                p += 1;
            }
            let from = self.spans.get(first).and_then(|s| self.nodes.get(s.from));
            let to = self.spans.get(last).and_then(|s| self.nodes.get(s.to));
            let (Some(from), Some(to)) = (from, to) else { continue };
            if to.unit <= reported.start {
                continue;
            }
            let start = from.unit.max(reported.start);
            let pushed = from.pushed() && from.unit >= reported.start;
            let sound = from.sound().max(reported.start)..to.sound();
            let sound = if sound.is_empty() { start..to.unit } else { sound };
            let chances = out.chances.len();
            self.add_chances(hearing, sound.clone(), &mut out.chances);
            out.spans.push(DecodedSpan {
                units: start..to.unit,
                sound,
                pushed,
                label: id.label(),
                rest,
                change: self.change(start),
                chances: chances..out.chances.len(),
            });
        }
    }

    /// Adds each reading's chance on each of some units, averaged over them, to `out`, sorted.
    fn add_chances(&self, hearing: &Hearing, units: Range<usize>, out: &mut Vec<Chance>) {
        if units.is_empty() {
            return;
        }
        let mut sum: PerId<f64> = PerId::filled(0.0);
        // The spans whose sound may overlap the units end after the first, and less than a span after the last.
        let from = self.nodes.partition_point(|n| n.unit <= units.start);
        let to = self.nodes.partition_point(|n| n.unit <= units.end + self.cap + 1);
        for node in self.nodes.get(from..to).unwrap_or_default() {
            for span in self.spans.get(node.into.clone()).unwrap_or_default() {
                let (Some(a), Some(b)) = (self.nodes.get(span.from), self.nodes.get(span.to)) else { continue };
                let (lo, hi) = (a.sound().max(units.start), b.sound().min(units.end));
                if lo >= hi {
                    continue;
                }
                let overlap = (hi - lo) as f64;
                for kept in self.kept.get(span.kept.clone()).unwrap_or_default() {
                    sum[kept.id] += kept.chance * overlap;
                }
            }
        }
        let first = out.len();
        let length = units.len() as f64;
        for id in (0..Id::COUNT as u8).map(Id) {
            if sum[id] > 0.0 {
                out.push(Chance { label: id.label(), chance: (sum[id] / length).min(1.0) });
            }
        }
        let lowest = hearing.lowest(units).map_or(PitchClass::C, PitchClass::of_key);
        let order = |c: &Chance| c.label.map_or((1, 0, 0), |l| (0, l.root.above(lowest), l.core.index()));
        if let Some(added) = out.get_mut(first..) {
            added.sort_by(|a, b| b.chance.total_cmp(&a.chance).then_with(|| order(a).cmp(&order(b))));
        }
    }
}

impl Node {
    fn at(unit: usize, push: Option<usize>, kind: Kind, reach: usize, region: Option<usize>) -> Node {
        let exact = region.is_some();
        Node { unit, push, kind, reach, exact, region, into: 0..0, entries: 0..0, out: 0..0 }
    }
}
