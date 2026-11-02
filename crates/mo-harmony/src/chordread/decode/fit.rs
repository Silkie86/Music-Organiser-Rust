//! What the fit (chordfit) adds up over a decode (harmony/design/chord-reader-design.md, "Model, training and data").
//!
//! The fit learns the weights from labelled music by making the ways to read it that the labels allow likelier. For
//! one piece of music, it needs two sums over the ways the decode kept: Z, every way's mass (e to its score, times the
//! sharpness), and Z over the ways the labels allow; and with each, the evidence a way holds on average (each piece
//! of evidence, the pair term and no chord, summed over the way's spans). A way's score is the weights times its
//! evidence, so the slope of ln Z in each weight is the sharpness times the evidence expected over every way, and the
//! slope of the fit's aim, ln Z over the allowed ways less ln Z, is the sharpness times the difference.
//!
//! A way is allowed when every reading it takes is one the labels accept where it is, and it starts a chord only near
//! a labelled change ([`Gold`]). The readings the labels accept are kept on every span inside a labelled stretch,
//! whatever their rank (the decode keeps them while it's told the labels), so the allowed ways are never pruned away.
//!
//! The sums are worked out over the decode's own lattice, forward and back, in logarithms, the same way for every way
//! and for the allowed ones; over every way they are the decode's own ln Z and chances (a test checks both). The
//! evidence is then weighed back from each reading's chance of starting a chord or being held on: only on spans where
//! some reading has a chance worth counting, whose facts are worked out again.

use std::ops::Range;

use super::{Decoder, Id, Kind, LnSum, PerId, SHARPNESS, add_ln};
use crate::chordread::candidates::Label;
use crate::chordread::cores::{Core, Group};
use crate::chordread::hear::Hearing;
use crate::chordread::score::{Feature, PerFeature};
use crate::chordread::weights::{NUMBERS, Priors, Tables, Weights};

/// How far a labelled change may be from where an allowed way changes chord, and how far a reading may reach into a
/// stretch labelled with a chord it isn't: one unit (half a beat, or a beat in compound metre).
pub const LABEL_TOLERANCE: usize = 1;

/// The least chance a reading needs for its evidence to be counted in what a way holds on average: less moves the
/// average by no more than the last bits of a float.
const LEAST_CHANCE: f64 = 1e-15;

/// A set of readings ([`Id`]), one bit each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct IdSet([u64; 4]);

impl IdSet {
    /// The readings of some labels.
    pub(crate) fn of(labels: &[Label]) -> IdSet {
        labels.iter().fold(IdSet::default(), |set, &label| set.with(Id::of(Some(label))))
    }

    fn with(mut self, id: Id) -> IdSet {
        let (word, bit) = (usize::from(id.0 / 64), id.0 % 64);
        if let Some(w) = self.0.get_mut(word) {
            *w |= 1 << bit;
        }
        self
    }

    pub(super) fn has(self, id: Id) -> bool {
        let (word, bit) = (usize::from(id.0 / 64), id.0 % 64);
        self.0.get(word).is_some_and(|w| w >> bit & 1 == 1)
    }

    /// Whether it holds a reading: a label, or none for no chord.
    #[cfg(test)]
    pub(crate) fn has_label(self, label: Option<Label>) -> bool {
        self.has(Id::of(label))
    }

    fn and(self, other: IdSet) -> IdSet {
        let mut out = self;
        for (w, o) in out.0.iter_mut().zip(other.0) {
            *w &= o;
        }
        out
    }

    /// Its readings, in the order of their ids.
    pub(super) fn ids(self) -> impl Iterator<Item = Id> {
        (0..Id::COUNT as u8).map(Id).filter(move |&id| self.has(id))
    }
}

/// What the labels say about some music, in units: the stretches labelled with a chord the reader has a reading for,
/// each with the readings it accepts, and where a chord may start. Stretches labelled with no chord, or with a chord
/// the reader can't read (one with no third, 4th, 2nd or fifth), aren't among them: any reading may be taken there,
/// and a chord may start anywhere in them or next to them.
#[derive(Clone, Debug, Default)]
pub(crate) struct Gold {
    units: usize,
    /// The labelled stretches, in order of their starts.
    stretches: Vec<(Range<usize>, IdSet)>,
    /// For each unit's start, from the first to where the last unit ends: whether an allowed way may start a chord
    /// there (within [`LABEL_TOLERANCE`] of a labelled stretch's start or end, or next to a unit no stretch covers).
    may_start: Vec<bool>,
}

impl Gold {
    /// The labels of music `units` long: the stretches labelled with a chord, each with the readings it accepts (an
    /// empty one is left out).
    pub(crate) fn new(units: usize, stretches: Vec<(Range<usize>, IdSet)>) -> Gold {
        let mut gold = Gold { units, stretches, may_start: Vec::new() };
        gold.mark();
        gold
    }

    /// Whether any stretch is labelled.
    pub(crate) fn is_labelled(&self) -> bool {
        !self.stretches.is_empty()
    }

    /// Puts the stretches in order, leaving out the empty ones, and marks where a chord may start.
    fn mark(&mut self) {
        let units = self.units;
        self.stretches.retain(|(r, _)| r.start < r.end.min(units));
        self.stretches.sort_by_key(|(r, _)| (r.start, r.end));
        let mut covered = vec![false; units];
        let mut near = vec![false; units + 1];
        for (r, _) in &self.stretches {
            for c in covered.get_mut(r.start..r.end.min(units)).unwrap_or_default() {
                *c = true;
            }
            for edge in [r.start, r.end.min(units)] {
                let to = (edge + LABEL_TOLERANCE).min(units);
                for n in near.get_mut(edge.saturating_sub(LABEL_TOLERANCE)..=to).unwrap_or_default() {
                    *n = true;
                }
            }
        }
        let free = |u: Option<usize>| u.and_then(|u| covered.get(u)).is_some_and(|c| !c);
        self.may_start =
            (0..=units).map(|u| near.get(u) == Some(&true) || free(u.checked_sub(1)) || free(Some(u))).collect();
    }

    /// Moves each labelled change that no place a chord may change is within [`LABEL_TOLERANCE`] of to the nearest
    /// such place (the earlier of two as near), so the labels never ask for a change no way can make: where the labels'
    /// time is off, or the music holds on through the change (struck before it, nothing struck near it). A stretch
    /// left with nothing in it goes. `places` are the units a chord may change at, in order. Gives how many of the
    /// stretches' ends moved.
    pub(crate) fn settle(&mut self, places: &[usize]) -> usize {
        let nearest = |u: usize| -> usize {
            let i = places.partition_point(|&p| p < u);
            let after = places.get(i).copied();
            let before = i.checked_sub(1).and_then(|i| places.get(i)).copied();
            match (before, after) {
                (Some(b), Some(a)) if u - b <= a - u => b,
                (_, Some(a)) => a,
                (Some(b), None) => b,
                (None, None) => u,
            }
        };
        let units = self.units;
        let mut moved = 0;
        for (r, _) in &mut self.stretches {
            for edge in [&mut r.start, &mut r.end] {
                let at = (*edge).min(units);
                let to = nearest(at);
                if at.abs_diff(to) > LABEL_TOLERANCE {
                    *edge = to;
                    moved += 1;
                }
            }
        }
        if moved > 0 {
            self.mark();
        }
        moved
    }

    /// The readings a span over `units` may take in an allowed way: those every labelled stretch it reaches more than
    /// [`LABEL_TOLERANCE`] into accepts (none, when they accept none in common). None when it reaches into none: any
    /// reading may.
    pub(crate) fn accepts(&self, units: Range<usize>) -> Option<IdSet> {
        let mut out: Option<IdSet> = None;
        // The stretches start in order, so only those starting before the span ends can reach into it.
        let before = self.stretches.partition_point(|(r, _)| r.start < units.end);
        for (r, set) in self.stretches.get(..before).unwrap_or_default() {
            let overlap = r.end.min(units.end).saturating_sub(r.start.max(units.start));
            if overlap > LABEL_TOLERANCE {
                out = Some(out.map_or(*set, |o| o.and(*set)));
            }
        }
        out
    }

    /// Whether an allowed way may start a chord at a unit's start.
    pub(crate) fn may_start(&self, unit: usize) -> bool {
        self.may_start.get(unit).copied().unwrap_or(true)
    }

    /// The labelled stretches, in order of their starts.
    #[cfg(test)]
    pub(crate) fn stretches(&self) -> &[(Range<usize>, IdSet)] {
        &self.stretches
    }
}

/// The motion prior of every way to move on ([`Tables::motion_prior`]), worked out once: what the pair term's trust is
/// weighed by.
struct Motion([[[f64; 3]; 12]; 3]);

impl Motion {
    fn new(tables: &Tables) -> Motion {
        let mut out = [[[0.0; 3]; 12]; 3];
        for (from, row) in Group::ALL.into_iter().zip(&mut out) {
            for (up, cell) in (0u8..).zip(row.iter_mut()) {
                for (to, value) in Group::ALL.into_iter().zip(cell) {
                    *value = tables.motion_prior(from, up, to);
                }
            }
        }
        Motion(out)
    }

    /// The motion prior between two readings, as the decode's pair term reads it: 0 for the same reading again and for
    /// a move into or out of no chord.
    fn between(&self, from: Id, to: Id) -> f64 {
        if from == to || from == Id::NONE || to == Id::NONE {
            return 0.0;
        }
        let group = |id: Id| Core::ALL.get(usize::from(id.0 % 16)).map_or(0, |c| c.group().index());
        let up = usize::from((to.0 / 16 + 12 - from.0 / 16) % 12);
        let row = self.0.get(group(from));
        row.and_then(|row| row.get(up)).and_then(|cell| cell.get(group(to))).copied().unwrap_or(0.0)
    }
}

/// Which steps a reading kept on a span may take in the ways summed: start a chord there (`STARTS`), or be held on
/// into the span (`HOLDS`).
const STARTS: u8 = 1;
const HOLDS: u8 = 2;

/// One sweep forward and back over the lattice, for every way or for the allowed ones: ln Z, and each kept reading's
/// chance of starting a chord on its span and of being held on into it, with the motion prior expected over the
/// chords before it when it starts one.
#[derive(Clone, Debug, Default)]
struct Sweep {
    ln_z: f64,
    started: Vec<f64>,
    held: Vec<f64>,
    motion: Vec<f64>,
}

/// The decode's working space for the sums, kept between reads.
#[derive(Clone, Debug, Default)]
pub(crate) struct FitWork {
    steps: Vec<u8>,
    sweeps: [Sweep; 2],
    started_in: Vec<f64>,
    held_in: Vec<f64>,
    after: Vec<f64>,
    acc: Vec<LnSum>,
    acc_full: Vec<LnSum>,
    alpha: Vec<f64>,
    alpha_full: Vec<f64>,
    beta: Vec<f64>,
    beta_full: Vec<f64>,
    terms: Vec<(f64, f64)>,
}

/// What the fit adds up over one decode ([`Decoder::fit_sums`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitSums {
    /// ln Z over every way the decode kept, and over the ways the labels allow (minus infinity when none does).
    pub ln_z: f64,
    pub ln_z_labelled: f64,
    /// The evidence a way holds on average, over every way and over the allowed ones, in [`Weights::numbers`]'s order:
    /// each piece of evidence summed over its spans (those counted for each felt beat times the beats), the motion
    /// prior summed over its changes of chord (for the pair term's trust), and the beats it reads as no chord (for
    /// w_N). Zero for the allowed ways when there are none.
    pub expected: [f64; NUMBERS],
    pub expected_labelled: [f64; NUMBERS],
}

impl Decoder {
    /// Keeps the readings `gold` accepts on every span inside a labelled stretch in the decodes that follow, whatever
    /// their rank, until it's given empty labels; and says which ways [`Decoder::fit_sums`] allows.
    pub(crate) fn set_gold(&mut self, gold: Gold) {
        self.gold = gold;
    }

    /// How many readings the last decode kept only because the labels accept them.
    pub(crate) fn forced(&self) -> usize {
        self.forced
    }

    /// How many of the labels' changes the last decode moved to a place a chord may change ([`Gold::settle`]).
    pub(crate) fn moved(&self) -> usize {
        self.moved
    }

    /// The units a chord may change at, in order: every place but those inside a rest or a pin (where only the same
    /// reading may follow).
    pub(super) fn change_places(&self) -> Vec<usize> {
        let mut out: Vec<usize> = self.nodes.iter().filter(|n| n.kind != Kind::Inside).map(|n| n.unit).collect();
        out.dedup();
        out
    }

    /// The fit's sums over the last decode of `hearing` ([`FitSums`]), on the labels it was given ([`Decoder::set_gold`])
    /// and the priors of the tables it was read with. Call it only after a decode that read something.
    pub(crate) fn fit_sums(&mut self, hearing: &Hearing, tables: &Tables, work: &mut FitWork) -> FitSums {
        let motion = Motion::new(tables);
        let mut out = FitSums {
            ln_z: f64::NEG_INFINITY,
            ln_z_labelled: f64::NEG_INFINITY,
            expected: [0.0; NUMBERS],
            expected_labelled: [0.0; NUMBERS],
        };
        let mut sweeps = std::mem::take(&mut work.sweeps);
        for (pass, sweep) in sweeps.iter_mut().enumerate() {
            self.steps_allowed(pass == 1, &mut work.steps);
            self.sweep(&motion, work, sweep);
        }
        let [free, labelled] = &sweeps;
        out.ln_z = free.ln_z;
        out.ln_z_labelled = labelled.ln_z;
        let passes: [(&Sweep, &mut [f64; NUMBERS]); 2] =
            [(free, &mut out.expected), (labelled, &mut out.expected_labelled)];
        self.expect(hearing, &Priors::of(tables), passes);
        work.sweeps = sweeps;
        out
    }

    /// Which steps each kept reading may take in the ways summed: every step, or the steps the labels allow (`gold`).
    fn steps_allowed(&self, gold: bool, out: &mut Vec<u8>) {
        out.clear();
        out.resize(self.kept.len(), STARTS | HOLDS);
        if !gold {
            return;
        }
        for span in &self.spans {
            let (Some(from), Some(to)) = (self.nodes.get(span.from), self.nodes.get(span.to)) else { continue };
            // A rest is no chord whatever the labels say: nothing else can be read there.
            let accepts = if span.rest { None } else { self.gold.accepts(from.unit..to.unit) };
            let starts = matches!(from.kind, Kind::Start | Kind::Edge) || self.gold.may_start(from.unit);
            for (k, kept) in span.kept.clone().zip(self.kept.get(span.kept.clone()).unwrap_or_default()) {
                let label_ok = accepts.is_none_or(|set| set.has(kept.id));
                let steps = match (label_ok, starts) {
                    (false, _) => 0,
                    (true, true) => STARTS | HOLDS,
                    (true, false) => HOLDS,
                };
                if let Some(s) = out.get_mut(k) {
                    *s = steps;
                }
            }
        }
    }

    /// One sweep forward and back over the lattice, each kept reading taking only the steps `work.steps` allows.
    fn sweep(&self, motion: &Motion, work: &mut FitWork, out: &mut Sweep) {
        let (kept, entries) = (self.kept.len(), self.entries.len());
        let none = f64::NEG_INFINITY;
        for v in [&mut work.started_in, &mut work.held_in, &mut work.after, &mut out.motion] {
            v.clear();
            v.resize(kept, none);
        }
        for v in [&mut work.acc, &mut work.acc_full] {
            v.clear();
            v.resize(entries, LnSum::EMPTY);
        }
        for v in [&mut work.alpha, &mut work.alpha_full, &mut work.beta, &mut work.beta_full] {
            v.clear();
            v.resize(entries, none);
        }
        self.forward_sweep(motion, work, &mut out.motion);
        let end = self.nodes.last().map_or(0..0, |end| end.entries.clone());
        let mut z = LnSum::EMPTY;
        for &alpha in work.alpha.get(end).unwrap_or_default() {
            z.add(alpha);
        }
        out.ln_z = z.ln();
        self.backward_sweep(work);
        // Each reading's chance of starting a chord on its span, and of being held on into it.
        out.started.clear();
        out.held.clear();
        let ln_z = out.ln_z;
        for (k, kept) in self.kept.iter().enumerate() {
            let at = |v: &[f64]| v.get(k).copied().unwrap_or(none);
            let after = at(&work.after);
            let chance = |into: f64, score: f64| {
                if into == none || after == none || ln_z == none {
                    0.0
                } else {
                    (into + SHARPNESS * score + after - ln_z).exp()
                }
            };
            out.started.push(chance(at(&work.started_in), kept.score));
            out.held.push(chance(at(&work.held_in), kept.held));
        }
    }

    /// The forward half of a sweep, place by place: what the ways that reach each place bring to the readings of the
    /// spans that start there, started there or held on, pushed on to where each span ends. `motion` gets, for each
    /// reading kept, the motion prior expected over the readings before it when it starts a chord.
    fn forward_sweep(&self, motion: &Motion, work: &mut FitWork, out_motion: &mut [f64]) {
        let mut at: PerId<Option<usize>> = PerId::filled(None);
        let mut ready: PerId<Option<(f64, f64)>> = PerId::filled(None);
        let mut touched: Vec<Id> = Vec::new();
        for (i, node) in self.nodes.iter().enumerate() {
            for e in node.entries.clone() {
                let (acc, acc_full) = (work.acc.get(e).copied(), work.acc_full.get(e).copied());
                if let (Some(a), Some(slot)) = (acc, work.alpha.get_mut(e)) {
                    *slot = a.ln();
                }
                if let (Some(a), Some(slot)) = (acc_full, work.alpha_full.get_mut(e)) {
                    *slot = a.ln();
                }
            }
            if node.kind == Kind::End {
                continue;
            }
            for (e, entry) in node.entries.clone().zip(self.entries.get(node.entries.clone()).unwrap_or_default()) {
                at[entry.id] = Some(e);
            }
            let holds = node.kind != Kind::Edge && !node.pushed();
            for &s in self.by_start.get(node.out.clone()).unwrap_or_default() {
                let Some(span) = self.spans.get(s) else { continue };
                debug_assert_eq!(span.from, i);
                for (k, kept) in span.kept.clone().zip(self.kept.get(span.kept.clone()).unwrap_or_default()) {
                    let Some(e_to) = kept.entry else { continue };
                    let steps = work.steps.get(k).copied().unwrap_or(0);
                    let id = kept.id;
                    let mut started = f64::NEG_INFINITY;
                    if steps & STARTS != 0 {
                        let (into, expected) = match ready[id] {
                            Some(r) => r,
                            None => {
                                let r = self.start_into(node.kind, id, &node.entries, motion, work);
                                ready[id] = Some(r);
                                touched.push(id);
                                r
                            }
                        };
                        started = into;
                        if let Some(m) = out_motion.get_mut(k) {
                            *m = expected;
                        }
                    }
                    let held = match at[id] {
                        Some(e) if holds && steps & HOLDS != 0 => {
                            work.alpha_full.get(e).copied().unwrap_or(f64::NEG_INFINITY)
                        }
                        _ => f64::NEG_INFINITY,
                    };
                    if let Some(slot) = work.started_in.get_mut(k) {
                        *slot = started;
                    }
                    if let Some(slot) = work.held_in.get_mut(k) {
                        *slot = held;
                    }
                    let (a, b) = (started + SHARPNESS * kept.score, held + SHARPNESS * kept.held);
                    if let Some(acc) = work.acc.get_mut(e_to) {
                        acc.add(a);
                        acc.add(b);
                    }
                    if span.full
                        && let Some(acc) = work.acc_full.get_mut(e_to)
                    {
                        acc.add(a);
                        acc.add(b);
                    }
                }
            }
            for e in self.entries.get(node.entries.clone()).unwrap_or_default() {
                at[e.id] = None;
            }
            for id in touched.drain(..) {
                ready[id] = None;
            }
        }
    }

    /// The ln mass of the ways that reach a place (of kind `kind`, the readings there at `entries`) and start the
    /// reading `id` there, with the pair term; and the motion prior expected over the readings they come from. Where
    /// the music starts, nothing comes before: ln 1 and no move.
    fn start_into(
        &self,
        kind: Kind,
        id: Id,
        entries: &Range<usize>,
        motion: &Motion,
        work: &mut FitWork,
    ) -> (f64, f64) {
        if kind == Kind::Start {
            return (0.0, 0.0);
        }
        // From another reading, or at a rest's or a pin's edge from any; never inside a rest or a pin.
        let starts = |from: Id| kind == Kind::Edge || (kind != Kind::Inside && from != id);
        work.terms.clear();
        for (e, entry) in entries.clone().zip(self.entries.get(entries.clone()).unwrap_or_default()) {
            let alpha = work.alpha.get(e).copied().unwrap_or(f64::NEG_INFINITY);
            if alpha == f64::NEG_INFINITY || !starts(entry.id) {
                continue;
            }
            let term = alpha + SHARPNESS * self.moves.between(entry.id, id).0;
            work.terms.push((term, motion.between(entry.id, id)));
        }
        let top = work.terms.iter().fold(f64::NEG_INFINITY, |top, &(t, _)| top.max(t));
        if top == f64::NEG_INFINITY {
            return (f64::NEG_INFINITY, 0.0);
        }
        let (mut sum, mut weighed) = (0.0, 0.0);
        for &(t, m) in &work.terms {
            let w = (t - top).exp();
            sum += w;
            weighed += w * m;
        }
        (top + sum.ln(), weighed / sum)
    }

    /// The backward half of a sweep, place by place from the last: the ln mass of the ways on from each reading that
    /// reaches each place, and from each kept reading's span's end (`work.after`).
    fn backward_sweep(&self, work: &mut FitWork) {
        let mut after: PerId<LnSum> = PerId::filled(LnSum::EMPTY);
        let mut after_held: PerId<LnSum> = PerId::filled(LnSum::EMPTY);
        let mut seen: PerId<bool> = PerId::filled(false);
        let mut touched: Vec<Id> = Vec::new();
        for node in self.nodes.iter().rev() {
            if node.kind == Kind::End {
                for e in node.entries.clone() {
                    if let Some(b) = work.beta.get_mut(e) {
                        *b = 0.0;
                    }
                    if let Some(b) = work.beta_full.get_mut(e) {
                        *b = 0.0;
                    }
                }
                continue;
            }
            for &s in self.by_start.get(node.out.clone()).unwrap_or_default() {
                let Some(span) = self.spans.get(s) else { continue };
                for (k, kept) in span.kept.clone().zip(self.kept.get(span.kept.clone()).unwrap_or_default()) {
                    let Some(e_to) = kept.entry else { continue };
                    let betas = if span.full { &work.beta_full } else { &work.beta };
                    let beta = betas.get(e_to).copied().unwrap_or(f64::NEG_INFINITY);
                    if let Some(a) = work.after.get_mut(k) {
                        *a = beta;
                    }
                    let id = kept.id;
                    if !seen[id] {
                        seen[id] = true;
                        touched.push(id);
                        after[id] = LnSum::EMPTY;
                        after_held[id] = LnSum::EMPTY;
                    }
                    let steps = work.steps.get(k).copied().unwrap_or(0);
                    if steps & STARTS != 0 {
                        after[id].add(SHARPNESS * kept.score + beta);
                    }
                    if steps & HOLDS != 0 {
                        after_held[id].add(SHARPNESS * kept.held + beta);
                    }
                }
            }
            // Whether a way through the reading `from` here moves on to the reading `next`: to another reading, or at
            // a rest's or a pin's edge to any; never inside a rest or a pin.
            let moves_on = |from: Id, next: Id| node.kind == Kind::Edge || (node.kind != Kind::Inside && next != from);
            for (e, entry) in node.entries.clone().zip(self.entries.get(node.entries.clone()).unwrap_or_default()) {
                let mut moved = LnSum::EMPTY;
                for &next in &touched {
                    if moves_on(entry.id, next) {
                        moved.add(SHARPNESS * self.moves.between(entry.id, next).0 + after[next].ln());
                    }
                }
                let moved = moved.ln();
                let own = if seen[entry.id] && !node.pushed() { after_held[entry.id].ln() } else { f64::NEG_INFINITY };
                let (beta, beta_full) = match node.kind {
                    Kind::Edge => (moved, moved),
                    Kind::Inside => (f64::NEG_INFINITY, own),
                    _ => (moved, add_ln(moved, own)),
                };
                if let Some(b) = work.beta.get_mut(e) {
                    *b = beta;
                }
                if let Some(b) = work.beta_full.get_mut(e) {
                    *b = beta_full;
                }
            }
            for id in touched.drain(..) {
                seen[id] = false;
            }
        }
    }

    /// The evidence a way holds on average, for each pass (its sweep, and the sums to add to): each kept reading's
    /// evidence on its span, weighed by its chance of starting a chord there (all of it) and of being held on (what
    /// counts for each felt beat), and the motion prior weighed by the chance of each change of chord.
    fn expect(&mut self, hearing: &Hearing, priors: &Priors, passes: [(&Sweep, &mut [f64; NUMBERS]); 2]) {
        let [(free, free_out), (labelled, labelled_out)] = passes;
        let pushes = hearing.pushes();
        let mut features = PerFeature::<f64>::default();
        for span in &self.spans {
            let range = span.kept.clone();
            let counts = |sweep: &Sweep, k: usize| {
                let p = sweep.started.get(k).copied().unwrap_or(0.0) + sweep.held.get(k).copied().unwrap_or(0.0);
                p > LEAST_CHANCE
            };
            if !range.clone().any(|k| counts(free, k) || counts(labelled, k)) {
                continue;
            }
            let (Some(from), Some(to)) = (self.nodes.get(span.from), self.nodes.get(span.to)) else { continue };
            let (start, end) = (from.push.and_then(|p| pushes.get(p)), to.push.and_then(|p| pushes.get(p)));
            self.evidence.facts(hearing, from.unit..to.unit, start, end, &mut self.facts);
            let beats = self.facts.beats();
            for (k, kept) in range.clone().zip(self.kept.get(range.clone()).unwrap_or_default()) {
                let label = kept.id.label();
                if let Some(label) = label.filter(|_| counts(free, k) || counts(labelled, k)) {
                    self.facts.features(label, priors, &mut features);
                }
                for (sweep, out) in [(free, &mut *free_out), (labelled, &mut *labelled_out)] {
                    if !counts(sweep, k) {
                        continue;
                    }
                    let started = sweep.started.get(k).copied().unwrap_or(0.0);
                    let all = started + sweep.held.get(k).copied().unwrap_or(0.0);
                    match label {
                        None => {
                            if let Some(x) = out.get_mut(Weights::NONE) {
                                *x += all * beats;
                            }
                        }
                        Some(_) => {
                            for (x, f) in out.iter_mut().zip(Feature::ALL) {
                                *x += if f.once() { started * features[f] } else { all * beats * features[f] };
                            }
                        }
                    }
                }
            }
        }
        for (sweep, out) in [(free, free_out), (labelled, labelled_out)] {
            let moves: f64 =
                sweep.started.iter().zip(&sweep.motion).filter(|(p, _)| **p > 0.0).map(|(p, m)| p * m).sum();
            if let Some(x) = out.get_mut(Weights::PAIR) {
                *x += moves;
            }
        }
    }
}

#[cfg(test)]
impl FitWork {
    /// Each kept reading's chances in the last sums, in the decode's order (the lattice's): of starting a chord on its
    /// span and of being held on into it, over every way (`labelled` false) or over the ways the labels allow.
    pub(crate) fn chances(&self, labelled: bool) -> Vec<(f64, f64)> {
        let sweep = &self.sweeps[usize::from(labelled)];
        sweep.started.iter().copied().zip(sweep.held.iter().copied()).collect()
    }
}
