//! The numbers the evidence is weighed by ([`Weights`]), and the counted tables the two template priors read
//! ([`Tables`]).
//!
//! For now the weights are set by hand ([`HAND`]): chosen so the design's hand cases read as a player would read them,
//! one stretch at a time, on the tables counted from labelled music ([`COUNTED`](super::COUNTED)). The fit (chordfit)
//! will start from them and learn the rest from labelled music, keeping every rule [`Weights::check`] lists, so no
//! explanation can ever show a chord's own note counting against it.

use super::cores::Core;
use super::score::{Feature, PerFeature};

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
/// - A root heard only in the bass leans a little towards the label (0.4 a beat): a pianist's rootless voicing over
///   the bass player's root. So D under G C F reads Dm11, while C over G (C/G) stays C: its G is played in the chords
///   too. Without this, no weights on the notes alone could tell those two apart.
/// - A held bass leans towards the root (0.9 a beat), less towards the third or seventh, least towards the fifth; a
///   bass that doesn't belong costs a little: the notes above it still decide, as B D G over a held C is G.
/// - A bass struck where the stretch starts leans towards its own root once (2), never enough to make C over E (C/E)
///   read as Em.
/// - With no bass, the lowest chord note leans a little towards its own root (E G B D: Em7 rather than G6).
/// - A chord changing on a bar line costs nothing, on the middle of a bar a little, between beats most.
/// - The template priors are trusted a little, on the counted tables: 0.2 for how common the chord is and 0.06 for
///   its tensions, a beat. Where the notes leave readings close, the commoner chord wins (B D G over a C struck in the
///   bass reads G, not Csus2), and a tension labels rarely name costs a little (a ♭13 on a minor seventh more than a
///   9 on a major seventh). These are the trusts at which the hand cases' smallest lead is largest. The pair term
///   waits for its table (trust 0), so a rootless voicing with no bass (B E F A for G13) still ranks below the
///   readings on a root that's played.
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
        2.0,   // T15: the notes' leaning towards the root
        0.0,   // R1: the root played in the chords or the held tune
        0.4,   // R2: the root only in the bass
        -0.3,  // R3: no root, over a bass
        -1.0,  // R4: no root and no bass
        0.4,   // L1: no bass, the lowest chord note is the root
        0.9,   // B1: the held bass is the root
        0.8,   // B2: … the third
        0.2,   // B3: … the fifth
        0.5,   // B4: … the seventh or sixth
        -0.05, // B5: … a tension
        -0.1,  // B6: … a note that doesn't belong
        1.0,   // B7: a moving bass on the root
        0.5,   // B8: … on the chord's other notes
        -0.5,  // B9: … on a note that isn't its own, on a strong beat or held long
        2.0,   // E1: the first bass note struck is the root
        0.5,   // E2: the chord's notes struck where the stretch starts
        -1.0,  // E3: nothing struck where the stretch starts
        0.0,   // E4: starts on a bar line
        -0.2,  // E5: starts in the middle of a bar
        -0.5,  // E6: starts on another beat
        -1.0,  // E7: starts between beats
        -0.3,  // E8: starts at a pushed chord
        0.2,   // P1's trust (t_core)
        0.06,  // P2's trust (t_ten)
    ]),
    pair: 0.0,
    none: -0.5,
};

impl Weights {
    /// The rules every set of weights keeps (the design's signs and orders), as the broken ones' descriptions; empty
    /// when it keeps them all.
    pub fn check(&self) -> Vec<&'static str> {
        use Feature::*;
        let w = |f: Feature| self.features[f];
        let rules: [(bool, &'static str); 27] = [
            (w(ChordTones) >= 0.0, "T1 counts for the label"),
            ((0.0..=w(ChordTones)).contains(&w(Tensions)), "T2 is between 0 and T1"),
            (w(WrongStruck) <= 0.0, "T3 counts against the label"),
            ((w(WrongStruck)..=0.0).contains(&w(WrongHeld)), "T4 is between T3 and 0"),
            ((w(WrongStruck)..=0.0).contains(&w(Passing)), "T5 is between T3 and 0"),
            (w(TuneTones) >= 0.0 && w(TuneTensions) >= 0.0, "T6 and T7 count for the label"),
            (w(TuneWrong) <= 0.0, "T8 counts against the label"),
            (w(NoThird) <= 0.0 && w(NoSeventh) <= 0.0 && w(NoCoreFifth) <= 0.0, "T9 to T11 count against"),
            ((w(NoThird)..=0.0).contains(&w(NoPlainFifth)), "T12 is between T9 and 0"),
            (w(RootSupport) >= 0.0, "T15 counts for the label"),
            (w(RootPlayed) >= 0.0 && w(RootInBass) >= 0.0, "R1 and R2 count for the label"),
            ((w(RootlessNoBass)..=0.0).contains(&w(RootlessOverBass)), "R3 is between R4 and 0"),
            (w(LowestIsRoot) >= 0.0, "L1 counts for the label"),
            ((0.0..=w(BassRoot)).contains(&w(BassThird)), "B2 is between 0 and B1"),
            ((0.0..=w(BassRoot)).contains(&w(BassFifth)), "B3 is between 0 and B1"),
            ((0.0..=w(BassRoot)).contains(&w(BassSeventh)), "B4 is between 0 and B1"),
            ((w(BassWrong)..=0.0).contains(&w(BassTension)), "B5 is between B6 and 0"),
            (w(WalkingRoot) >= 0.0, "B7 counts for the label"),
            ((0.0..=w(WalkingRoot)).contains(&w(WalkingTones)), "B8 is between 0 and B7"),
            (w(WalkingWrong) <= 0.0, "B9 counts against the label"),
            (w(BassStrikesRoot) >= 0.0 && w(StruckAtStart) >= 0.0, "E1 and E2 count for the label"),
            (w(NothingStruck) <= 0.0 && w(Pushed) <= 0.0, "E3 and E8 count against the label"),
            (w(OnBar) <= 0.0, "E4 is 0 or less"),
            (w(MidBar) <= w(OnBar), "E5 is at most E4"),
            (w(OnBeat) <= w(MidBar) && w(OffBeat) <= w(OnBeat), "E7 is at most E6, at most E5"),
            (w(CorePrior) >= 0.0 && w(TensionPrior) >= 0.0 && self.pair >= 0.0, "the trusts are 0 or more"),
            (self.is_finite(), "every number is finite"),
        ];
        rules.into_iter().filter(|(kept, _)| !kept).map(|(_, rule)| rule).collect()
    }

    fn is_finite(&self) -> bool {
        self.features.0.iter().all(|w| w.is_finite()) && self.pair.is_finite() && self.none.is_finite()
    }
}

/// How many cores the tables count.
const CORES: usize = Core::ALL.len();

/// The counted tables the template priors read: how often each core is the chord (f_core), and how often each of its
/// tensions is played with it (f_ten). The reader's are counted from labelled music ([`COUNTED`](super::COUNTED));
/// [`EVEN`] holds the tables before counting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tables {
    /// For each core (in [`Core::ALL`]'s order), the share of chords that are it.
    pub core: [f64; CORES],
    /// For each core, and each interval above its root, the share of its chords that play that tension.
    pub tension: [[f64; 12]; CORES],
}

/// The tables before counting: every core as likely as any other, and each tension played half the time. P1 is then
/// the same for every reading; P2 costs ln 2 for each tension the core can carry, heard or not.
pub const EVEN: Tables = Tables { core: [1.0 / CORES as f64; CORES], tension: [[0.5; 12]; CORES] };

/// The least share a prior reads, and 1 less it the most: a core or a tension never counted (or always played) still
/// costs a finite amount, so no score is ever infinite and a trust of 0 always leaves the prior out.
const FLOOR: f64 = 1e-6;

/// ln of a share, kept between the floor and 1 less it; a share that isn't a number reads as the floor.
fn ln_share(share: f64) -> f64 {
    let kept = if share >= FLOOR { share.min(1.0 - FLOOR) } else { FLOOR };
    kept.ln()
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
}
