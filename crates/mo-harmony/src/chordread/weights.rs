//! The numbers the evidence is weighed by ([`Weights`]), and the counted tables the two template priors read
//! ([`Tables`]).
//!
//! For now the weights are set by hand ([`HAND`]): chosen so the design's hand cases read as a player would read them,
//! one stretch at a time and read whole, on the tables counted from labelled music ([`COUNTED`](super::COUNTED)). The
//! fit (chordfit) will start from them and learn the rest from labelled music, keeping every rule [`Weights::check`]
//! lists, so no explanation can ever show a chord's own note counting against it.

use super::answer::Thresholds;
use super::candidates::Label;
use super::cores::{Core, Group};
use super::score::{Feature, PerFeature};

/// The hash of every number the reader is built from ([`super::numbers_hash`]) that these were written with, and the
/// version of the reader they were first given with ([`super::CHORD_READER_VERSION`]). A test checks both: change a
/// number here, in the counted tables or among those set by hand, and it asks for the new hash and a new version.
pub(super) const SHIPPED_HASH: u64 = 0x2a25_dc50_1312_92a7;
pub(super) const HASH_VERSION: u32 = 1;

/// What each piece of evidence is worth ([`Feature`]), the trust in the pair term, and what a stretch read as no chord
/// scores.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weights {
    /// One for each piece of evidence: the 35 weights, then the trusts in the two template priors (t_core and t_ten).
    pub features: PerFeature<f64>,
    /// The trust in the pair term (t_pair), used when deciding between neighbouring stretches.
    pub pair: f64,
    /// What a stretch read as no chord scores, for each felt beat sounding (w_N).
    pub none: f64,
}

/// The hand-set weights: the reader's first numbers, before any fitting.
///
/// - The chords' own notes count most (3 a beat for all of them), a tension struck with them a third of that, and a
///   note that doesn't belong, struck, as much against. Held over or carried in, it costs a third as much; one that
///   only passes by costs a sixth.
/// - A missing third costs most of the missing notes, a missing plain fifth very little (shells leave it out).
/// - A root heard only in the bass leans towards the label (0.7 a beat): a pianist's rootless voicing or shell over the
///   bass player's root. So D under G C F reads Dm11, and F C over a walking D F A A♭ reads Dm7, while C over G (C/G)
///   stays C: its G is played in the chords too. Without this, no weights on the notes alone could tell those apart.
///   The chords' own leaning towards a root counts 1 a beat, so it settles a voicing with no bass but never outweighs
///   the bass's root under a shell.
/// - A held bass leans towards the root (0.9 a beat), half as much towards the third or seventh, least towards the
///   fifth; a bass that doesn't belong costs a little: the notes above it still decide, as B D G over a held C is G.
///   The root leans enough more than the third that C E G A over C reads C6 for as long as it's held, not Am7, though
///   labels name minor sevenths far more often.
/// - Where a chord starts counts once: the bass moving to its root (2, never enough to make C over E (C/E) read as Em),
///   and its notes struck there (0.5). Starting where nothing is struck costs 2, and so does starting on a walking
///   bass note: one no longer than a felt beat, under chords held through it or struck again on the same notes. A
///   chord changing costs 1 on a bar line, 1.2 on the middle of a bar, 1.5 on another beat and 2 between beats. So a
///   chord held or struck again stays one chord, and a walking bass under it stays under it (keys in sixteenths over a
///   bass in eighths too), while chords struck anew change where they're struck.
/// - With no bass, the lowest chord note leans a little towards its own root (E G B D: Em7 rather than G6).
/// - The template priors are trusted a little, on the counted tables: 0.11 for how common the chord is and 0.06 for
///   its tensions, a beat. Where the notes leave readings close, the commoner chord wins (B D G over a C struck in the
///   bass reads G, not Csus2), and a tension labels rarely name costs a little (a ♭13 on a minor seventh more than a
///   9 on a major seventh). These are the trusts at which the hand cases' smallest lead is largest, every case still
///   passing a step of 0.01 away.
/// - The pair term is trusted a quarter: how much likelier the root's move is than one picked at random, between the
///   two kinds of chord. It holds a ii–V–I over a walking bass to Dm7 G7 Cmaj7 (not F G7 Em) when the other weights
///   are a step off, and it is the steadiest trust on the hand cases read whole: with the other weights moved about
///   (`show_the_grid`), every case reads right at more of the points than with any other trust; from 1 it starts to
///   read some of them wrongly. A rootless voicing with no bass (B E F A for G13) still ranks below the readings on a
///   root that's played: no trust in the pair term lifts it.
pub const HAND: Weights = Weights {
    features: PerFeature([
        3.0,   // T1: the chords' sound in the label's own notes
        1.0,   // T2: tensions struck with the chord
        -3.0,  // T3: notes that don't belong, struck
        -1.0,  // T4: notes that don't belong, carried in or held over
        -0.5,  // T5: notes that only pass by
        1.0,   // T6: the tune on the label's own notes
        0.4,   // T7: the tune on a tension
        -1.0,  // T8: the tune on a note that doesn't belong
        -2.0,  // T9: no third
        -1.0,  // T10: no seventh or sixth (a four-note chord)
        -1.5,  // T11: no fifth that says which chord it is (a flat or sharp fifth, a power chord's)
        -0.1,  // T12: no plain fifth (one shells leave out)
        1.0,   // T15: the notes' leaning towards the root
        0.0,   // R1: the root played in the chords or the held tune
        0.7,   // R2: the root only in the bass
        -0.3,  // R3: no root, over a bass
        -1.0,  // R4: no root and no bass
        0.4,   // L1: no bass, the lowest chord note is the root
        0.9,   // B1: the held bass is the root
        0.5,   // B2: … the third
        0.2,   // B3: … the fifth
        0.5,   // B4: … the seventh or sixth
        -0.05, // B5: … a tension
        -0.1,  // B6: … a note that doesn't belong
        1.0,   // B7: a moving bass on the root
        0.5,   // B8: … on the chord's other notes
        -0.5,  // B9: … on a note that isn't its own, on a strong beat or held long
        2.0,   // E1: the first bass note struck is the root
        0.5,   // E2: the chord's notes struck where the stretch starts
        -2.0,  // E3: nothing struck where the stretch starts
        -1.0,  // E4: starts on a bar line
        -1.2,  // E5: starts in the middle of a bar
        -1.5,  // E6: starts on another beat
        -2.0,  // E7: starts between beats
        -0.3,  // E8: starts at a pushed chord
        0.11,  // P1's trust (t_core)
        0.06,  // P2's trust (t_ten)
    ]),
    pair: 0.25,
    none: -0.5,
};

/// The hand-set thresholds the answers are graded by (texture by texture: a full mix, an accompaniment, chords alone,
/// one line), until the calibration's fit sets them. Set from the hand cases: keys alone that a player could read
/// more than one way (cases 13, 20, 22 and 33 with no bass) give their first reading a chance of 0.63 to 0.76 and
/// must not be one chord, while the chords the cases want as one chord read 0.98 or more (cases 1, 10, 11 and 54).
/// The same notes need their group at 0.9: were the chances true, the chord would be outside it at most one time in
/// ten, the design's target. One line alone is never one chord or the same notes: it's implied.
pub const THRESHOLDS: Thresholds = Thresholds { one: [0.8, 0.8, 0.8, 1.0], same: [0.9, 0.9, 0.9, 1.0] };

/// How many numbers a set of weights is ([`Weights::numbers`]), the numbers the fit sets: a weight for each piece of
/// evidence (the two template priors' trusts among them), the trust in the pair term ([`Weights::PAIR`]), and w_N
/// ([`Weights::NONE`]).
pub const NUMBERS: usize = Feature::COUNT + 2;

/// One way a rule holds a number ([`Rule`]), each number by its place among [`Weights::numbers`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    /// The number is 0 or more.
    AtLeast0(usize),
    /// The number is 0 or less.
    AtMost0(usize),
    /// The first number is at most the second.
    AtMost(usize, usize),
}

impl Bound {
    /// Whether some numbers keep it (never, where one it reads isn't a number or isn't there).
    pub fn holds(self, numbers: &[f64]) -> bool {
        let at = |i: usize| numbers.get(i).copied().unwrap_or(f64::NAN);
        match self {
            Bound::AtLeast0(i) => at(i) >= 0.0,
            Bound::AtMost0(i) => at(i) <= 0.0,
            Bound::AtMost(i, j) => at(i) <= at(j),
        }
    }
}

/// One of the rules every set of weights keeps (the design's signs and orders): the bounds it puts on the numbers,
/// and what it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub bounds: &'static [Bound],
    pub says: &'static str,
}

/// A feature's place among the numbers.
const fn n(feature: Feature) -> usize {
    feature as usize
}

/// The rules every set of weights keeps, besides every number being finite ([`Weights::check`]): each piece of
/// evidence counts the way it says (a chord's own note never counts against it), and the costs of starting a chord
/// rise from a bar line to between beats. The fit keeps them all.
pub const RULES: [Rule; 26] = {
    use Bound::*;
    use Feature::*;
    [
        Rule { bounds: &[AtLeast0(n(ChordTones))], says: "T1 counts for the label" },
        Rule { bounds: &[AtLeast0(n(Tensions)), AtMost(n(Tensions), n(ChordTones))], says: "T2 is between 0 and T1" },
        Rule { bounds: &[AtMost0(n(WrongStruck))], says: "T3 counts against the label" },
        Rule { bounds: &[AtMost(n(WrongStruck), n(WrongHeld)), AtMost0(n(WrongHeld))], says: "T4 is between T3 and 0" },
        Rule { bounds: &[AtMost(n(WrongStruck), n(Passing)), AtMost0(n(Passing))], says: "T5 is between T3 and 0" },
        Rule { bounds: &[AtLeast0(n(TuneTones)), AtLeast0(n(TuneTensions))], says: "T6 and T7 count for the label" },
        Rule { bounds: &[AtMost0(n(TuneWrong))], says: "T8 counts against the label" },
        Rule {
            bounds: &[AtMost0(n(NoThird)), AtMost0(n(NoSeventh)), AtMost0(n(NoCoreFifth))],
            says: "T9 to T11 count against",
        },
        Rule {
            bounds: &[AtMost(n(NoThird), n(NoPlainFifth)), AtMost0(n(NoPlainFifth))],
            says: "T12 is between T9 and 0",
        },
        Rule { bounds: &[AtLeast0(n(RootSupport))], says: "T15 counts for the label" },
        Rule { bounds: &[AtLeast0(n(RootPlayed)), AtLeast0(n(RootInBass))], says: "R1 and R2 count for the label" },
        Rule {
            bounds: &[AtMost(n(RootlessNoBass), n(RootlessOverBass)), AtMost0(n(RootlessOverBass))],
            says: "R3 is between R4 and 0",
        },
        Rule { bounds: &[AtLeast0(n(LowestIsRoot))], says: "L1 counts for the label" },
        Rule { bounds: &[AtLeast0(n(BassThird)), AtMost(n(BassThird), n(BassRoot))], says: "B2 is between 0 and B1" },
        Rule { bounds: &[AtLeast0(n(BassFifth)), AtMost(n(BassFifth), n(BassRoot))], says: "B3 is between 0 and B1" },
        Rule {
            bounds: &[AtLeast0(n(BassSeventh)), AtMost(n(BassSeventh), n(BassRoot))],
            says: "B4 is between 0 and B1",
        },
        Rule {
            bounds: &[AtMost(n(BassWrong), n(BassTension)), AtMost0(n(BassTension))],
            says: "B5 is between B6 and 0",
        },
        Rule { bounds: &[AtLeast0(n(WalkingRoot))], says: "B7 counts for the label" },
        Rule {
            bounds: &[AtLeast0(n(WalkingTones)), AtMost(n(WalkingTones), n(WalkingRoot))],
            says: "B8 is between 0 and B7",
        },
        Rule { bounds: &[AtMost0(n(WalkingWrong))], says: "B9 counts against the label" },
        Rule {
            bounds: &[AtLeast0(n(BassStrikesRoot)), AtLeast0(n(StruckAtStart))],
            says: "E1 and E2 count for the label",
        },
        Rule { bounds: &[AtMost0(n(NothingStruck)), AtMost0(n(Pushed))], says: "E3 and E8 count against the label" },
        Rule { bounds: &[AtMost0(n(OnBar))], says: "E4 is 0 or less" },
        Rule { bounds: &[AtMost(n(MidBar), n(OnBar))], says: "E5 is at most E4" },
        Rule {
            bounds: &[AtMost(n(OnBeat), n(MidBar)), AtMost(n(OffBeat), n(OnBeat))],
            says: "E7 is at most E6, at most E5",
        },
        Rule {
            bounds: &[AtLeast0(n(CorePrior)), AtLeast0(n(TensionPrior)), AtLeast0(Weights::PAIR)],
            says: "the trusts are 0 or more",
        },
    ]
};

impl Weights {
    /// Where the pair term's trust is among the numbers ([`Weights::numbers`]), and w_N.
    pub const PAIR: usize = Feature::COUNT;
    pub const NONE: usize = Feature::COUNT + 1;

    /// The rules every set of weights keeps ([`RULES`], then every number being finite), as the broken ones'
    /// descriptions; empty when it keeps them all.
    pub fn check(&self) -> Vec<&'static str> {
        let numbers = self.numbers();
        let mut broken: Vec<&'static str> = RULES
            .iter()
            .filter(|rule| !rule.bounds.iter().all(|bound| bound.holds(&numbers)))
            .map(|rule| rule.says)
            .collect();
        if !numbers.iter().all(|x| x.is_finite()) {
            broken.push("every number is finite");
        }
        broken
    }

    /// Every number, in the fit's order: the weight of each piece of evidence in [`Feature::ALL`]'s order (the two
    /// template priors' trusts last among them), then the trust in the pair term ([`Weights::PAIR`]), then w_N
    /// ([`Weights::NONE`]).
    pub fn numbers(&self) -> [f64; NUMBERS] {
        let mut out = [0.0; NUMBERS];
        for (slot, w) in out.iter_mut().zip(self.features.0.iter().chain([&self.pair, &self.none])) {
            *slot = *w;
        }
        out
    }

    /// The weights some numbers are, in [`Weights::numbers`]'s order.
    pub fn from_numbers(numbers: &[f64; NUMBERS]) -> Weights {
        let mut features = PerFeature::<f64>::default();
        for (w, x) in features.0.iter_mut().zip(numbers) {
            *w = *x;
        }
        let at = |i: usize| numbers.get(i).copied().unwrap_or(0.0);
        Weights { features, pair: at(Weights::PAIR), none: at(Weights::NONE) }
    }

    /// A number's name, by its place: a piece of evidence's code ("T1", "E8", "P2"), "pair" or "none".
    pub fn number_name(place: usize) -> &'static str {
        match Feature::ALL.get(place) {
            Some(feature) => feature.code(),
            None if place == Weights::PAIR => "pair",
            None if place == Weights::NONE => "none",
            None => "?",
        }
    }
}

/// How many cores the tables count, and how many groups of them the pair term tells apart ([`Group`]).
const CORES: usize = Core::ALL.len();
const GROUPS: usize = Group::ALL.len();

/// The counted tables the priors read: how often each core is the chord (f_core), how often each of its tensions is
/// played with it (f_ten), and how chords move from one to the next (f_motion). The reader's are counted from labelled
/// music ([`COUNTED`](super::COUNTED)); [`EVEN`] holds the tables before counting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tables {
    /// For each core (in [`Core::ALL`]'s order), the share of chords that are it.
    pub core: [f64; CORES],
    /// For each core, and each interval above its root, the share of its chords that play that tension.
    pub tension: [[f64; 12]; CORES],
    /// For each group of the chord before ([`Group::ALL`]'s order), how often the next chord's root lies each
    /// interval above its root, with the next chord in each group: each group's 36 shares add up to 1.
    pub motion: [[[f64; GROUPS]; 12]; GROUPS],
}

/// The tables before counting: every core as likely as any other, each tension played half the time, and every way
/// to move on as likely as any other. P1 is then the same for every reading; P2 costs ln 2 for each tension the core
/// can carry, heard or not; and P3 is 0 for every change of chord.
pub const EVEN: Tables = Tables {
    core: [1.0 / CORES as f64; CORES],
    tension: [[0.5; 12]; CORES],
    motion: [[[1.0 / Group::MOVES as f64; GROUPS]; 12]; GROUPS],
};

/// The least share a prior reads, and 1 less it the most: a core or a tension never counted (or always played) still
/// costs a finite amount, so no score is ever infinite and a trust of 0 always leaves the prior out.
pub(super) const FLOOR: f64 = 1e-6;

/// A share, kept between the floor and 1 less it; a share that isn't a number reads as the floor.
fn kept(share: f64) -> f64 {
    if share >= FLOOR { share.min(1.0 - FLOOR) } else { FLOOR }
}

/// ln of a share, kept between the floor and 1 less it.
fn ln_share(share: f64) -> f64 {
    kept(share).ln()
}

impl Tables {
    /// ln f_core: how likely the core is, before the notes are heard.
    pub fn core_prior(&self, core: Core) -> f64 {
        ln_share(self.core.get(core.index()).copied().unwrap_or(1.0 / CORES as f64))
    }

    /// What a tension (by its interval above the root) adds to a core's tension prior: ln f_ten when it's heard, and
    /// ln (1 − f_ten) when it isn't.
    pub fn tension_prior(&self, core: Core, interval: u8, heard: bool) -> f64 {
        let row = self.tension.get(core.index());
        let f = row.and_then(|row| row.get(usize::from(interval))).copied().unwrap_or(0.5);
        ln_share(if heard { f } else { 1.0 - f })
    }

    /// What the pair term reads for a chord moving on from `from` to `to`: how much likelier the way its root moves is
    /// than a move picked at random, between chords of the same two groups ([`Group`], [`Tables::motion_prior`]). How
    /// common each kind of chord is, P1 counts already; this adds only how the root moves. The same label again is no
    /// move but the chord held on, past the longest stretch the reader reads at once: 0, as the tables count only
    /// changes of chord.
    pub fn pair_prior(&self, from: Label, to: Label) -> f64 {
        if from == to {
            return 0.0;
        }
        self.motion_prior(from.core.group(), to.root.above(from.root), to.core.group())
    }

    /// ln (12 f_motion(up | from, to)) for one way to move on, from a chord in the group `from`, its root moving `up`
    /// semitones, to one in the group `to`: the share of that root move among the 12 the tables count between the two
    /// groups, times 12, so a move likelier than one picked at random reads above 0 and a rarer one below.
    pub fn motion_prior(&self, from: Group, up: u8, to: Group) -> f64 {
        let row = self.motion.get(from.index());
        let share = |up: usize| {
            let cell = row.and_then(|row| row.get(up));
            kept(cell.and_then(|cell| cell.get(to.index())).copied().unwrap_or(1.0 / Group::MOVES as f64))
        };
        let all: f64 = (0..12).map(share).sum();
        (12.0 * share(usize::from(up)) / all).ln()
    }
}

/// The priors as the scoring reads them for each label ([`super::SpanFacts::features`]), worked out once from some
/// tables: the same numbers [`Tables::core_prior`] and [`Tables::tension_prior`] give, without working out a logarithm
/// for each label scored.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Priors {
    /// ln f_core, by core.
    core: [f64; CORES],
    /// By core and interval above the root: what a tension adds when it isn't heard, and when it is.
    tension: [[[f64; 2]; 12]; CORES],
}

impl Priors {
    /// The priors some tables give.
    pub fn of(tables: &Tables) -> Priors {
        let mut out = Priors { core: [0.0; CORES], tension: [[[0.0; 2]; 12]; CORES] };
        for ((core, ln), row) in Core::ALL.into_iter().zip(&mut out.core).zip(&mut out.tension) {
            *ln = tables.core_prior(core);
            for (interval, cell) in (0u8..).zip(row.iter_mut()) {
                *cell = [tables.tension_prior(core, interval, false), tables.tension_prior(core, interval, true)];
            }
        }
        out
    }

    /// ln f_core ([`Tables::core_prior`]).
    pub fn core(&self, core: Core) -> f64 {
        self.core.get(core.index()).copied().unwrap_or(0.0)
    }

    /// What a tension adds to a core's tension prior ([`Tables::tension_prior`]).
    pub fn tension(&self, core: Core, interval: u8, heard: bool) -> f64 {
        let row = self.tension.get(core.index());
        let cell = row.and_then(|row| row.get(usize::from(interval)));
        cell.and_then(|cell| cell.get(usize::from(heard))).copied().unwrap_or(0.0)
    }
}
