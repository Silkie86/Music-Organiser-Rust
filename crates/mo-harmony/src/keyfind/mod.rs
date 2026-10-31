//! The key of some notes: a part, a loop, a pack's clip or a whole song (spec section 5.1, part 2 of the engine).
//!
//! The finder first hears the notes ([`KeyReader::hear`]), beat by beat on the grid, as whole-number counts
//! ([`Heard`]): how long each note sounds in the tune and in the other parts, the bass of each beat and the chords
//! standing on it, and where the bass starts and ends. Each of these is a "stream" of evidence ([`Stream`]).
//!
//! Then each of the 24 keys is scored by how the counts sit in it: a weight for each note counted up from the tonic,
//! in major and in minor, for each stream. The weights are the same for every key, so moving the notes up a semitone
//! moves the answer up a semitone, exactly. The scores become chances by a softmax over the 24 keys ([`KeyReading`]),
//! sharpened more the more bars were heard ([`Calibration`]).
//!
//! The weights and the calibration were fitted on music people labelled (see `weights.rs`, made by `theorybench
//! keyfit`), and only change when the bench measures a change as better; each change bumps [`KEY_FINDER_VERSION`].

mod heard;
mod weights;

pub use heard::{Heard, KeyReader, MOST_BEATS, RoleSet};

use crate::key::Key;
use crate::notes::{Grid, Part};
use crate::pitch::PitchClass;

/// Bumped whenever the finder can give a different answer for the same notes (new weights, new evidence), so the
/// answers the app keeps (the MIDI page's index) know to be worked out again.
pub const KEY_FINDER_VERSION: u32 = 1;

/// What the notes are, as cut from the music: it says what their first and last bass mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Span {
    /// A loop: a pack's part or clip, a pattern. It starts where its music starts, so its first bass counts; it often
    /// ends on the fifth, ready to go round again, so its last bass doesn't.
    Loop,
    /// A whole song or arrangement. Songs mostly start and end at home.
    Song,
    /// A stretch cut from longer music: it starts and ends anywhere, so neither end counts.
    Window,
}

impl Span {
    pub const ALL: [Span; 3] = [Span::Loop, Span::Song, Span::Window];
}

/// Which streams have sound: what the notes can say, and so how sure the finder may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Config {
    /// A tune and other parts.
    Full,
    /// Only a tune.
    MelodyOnly,
    /// Only a bass part.
    BassOnly,
    /// Chords, or chords and bass, with no tune.
    Accompaniment,
}

impl Config {
    pub const ALL: [Config; 4] = [Config::Full, Config::MelodyOnly, Config::BassOnly, Config::Accompaniment];
}

/// The streams of evidence, in the order the weights keep them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stream {
    /// How long each note sounds in the tune.
    Tune,
    /// How long each note sounds in the other parts: which notes the music uses.
    Harmony,
    /// The bass of each beat, weighted by where the beat falls in the bar: home is where the bass sits on strong
    /// beats.
    Bass,
    /// Major chords standing on the bass, by the bass note: the home chord over its own root.
    MajorOnBass,
    /// Minor chords standing on the bass.
    MinorOnBass,
    /// A loop's first bass: where a loop really starts.
    FirstBassLoop,
    /// A song's first bass.
    FirstBassSong,
    /// A song's last bass: songs end at home. Never used for a loop, which often ends on the fifth.
    LastBassSong,
}

impl Stream {
    pub const ALL: [Stream; STREAMS] = [
        Stream::Tune,
        Stream::Harmony,
        Stream::Bass,
        Stream::MajorOnBass,
        Stream::MinorOnBass,
        Stream::FirstBassLoop,
        Stream::FirstBassSong,
        Stream::LastBassSong,
    ];
}

/// How many streams there are.
pub const STREAMS: usize = 8;

/// How many weights the finder has: twelve notes above the tonic, in major and in minor, for each stream, and one
/// for minor keys as a whole.
pub const FEATURES: usize = STREAMS * 24 + 1;

/// The feature minor keys have as a whole: the music the finder learned from has fewer minor keys.
pub const MINOR: usize = STREAMS * 24;

/// Knots of the calibration curves: 1, 2, 4, 8 and 16 bars heard (flat past 16 bars).
pub const KNOTS: usize = 5;

/// Numbers in a [`Calibration`]: the curve's knots, three for the kinds of input (melody only, bass only,
/// accompaniment; a full mix is 0), the knots of what a loop adds, and what a song adds.
pub const CALIBRATION_TERMS: usize = KNOTS + 3 + KNOTS + 1;

/// The least sharpness a reading can have, so a calibration fitted on longer music can't turn a short stretch's
/// scores upside down.
pub const LEAST_SHARPNESS: f64 = 0.05;

/// How much the scores are sharpened before they become chances: a curve over how many bars differ (on a log scale,
/// straight between the knots, flat past 16 bars, so it has no jumps), plus an offset for what kind of input it is,
/// and for a loop a curve of its own added on, for a song an offset. Fitted on scores of songs held out of each fit.
///
/// - The curve is a window's. A loop starts where its music starts and its first bass says a lot, so a one-bar loop
///   may be surer than one bar cut from the middle of a song; by 16 bars the two are much the same.
/// - A song's offset sits on the curve's end: songs are longer than 16 bars, and how sure a song may be hardly
///   changes with its length (each stream's evidence already counts more the more beats it has).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Calibration(pub [f64; CALIBRATION_TERMS]);

/// Writes the weights of a tent curve over `bars` on a log scale into `terms`: 1 on the knot at `bars` (1, 2, 4 …),
/// split between the two knots around it in between, and the last knot's past them.
fn tent(terms: &mut [f64], bars: u32) {
    let last = terms.len().saturating_sub(1);
    let at = f64::from(bars.max(1)).log2().clamp(0.0, last as f64);
    let below = at.floor() as usize;
    let above = at - below as f64;
    if let Some(t) = terms.get_mut(below) {
        *t = 1.0 - above;
    }
    if above > 0.0
        && let Some(t) = terms.get_mut(below + 1)
    {
        *t = above;
    }
}

impl Calibration {
    /// What each of the calibration's numbers is multiplied by for this much music: the sharpness is their sum. The
    /// bench fits the numbers through this.
    pub fn terms(bars: u32, config: Config, span: Span) -> [f64; CALIBRATION_TERMS] {
        let mut terms = [0.0; CALIBRATION_TERMS];
        let (curve, rest) = terms.split_at_mut(KNOTS);
        let (inputs, rest) = rest.split_at_mut(3);
        let (loops, song) = rest.split_at_mut(KNOTS);
        tent(curve, bars);
        let input = match config {
            Config::Full => None,
            Config::MelodyOnly => Some(0),
            Config::BassOnly => Some(1),
            Config::Accompaniment => Some(2),
        };
        if let Some(t) = input.and_then(|i| inputs.get_mut(i)) {
            *t = 1.0;
        }
        match span {
            Span::Loop => tent(loops, bars),
            Span::Song => song.fill(1.0),
            Span::Window => {}
        }
        terms
    }

    /// The sharpness for this much music: at least [`LEAST_SHARPNESS`].
    pub fn sharpness(&self, bars: u32, config: Config, span: Span) -> f64 {
        let terms = Calibration::terms(bars, config, span);
        let s: f64 = self.0.iter().zip(terms).map(|(c, t)| c * t).sum();
        if s.is_finite() { s.max(LEAST_SHARPNESS) } else { LEAST_SHARPNESS }
    }
}

/// A fitted finder: the weights of every feature and the calibration. [`SHIPPED`] is the one built in; the bench
/// passes the ones it fits.
#[derive(Clone, Copy, Debug)]
pub struct Model<'a> {
    pub weights: &'a [f64; FEATURES],
    pub calibration: &'a Calibration,
}

/// The finder built into the app.
pub const SHIPPED: Model<'static> = Model { weights: &weights::WEIGHTS, calibration: &weights::CALIBRATION };

/// A key's score: how well what was heard fits it, before the scores become chances. Summed in the order
/// [`Heard::features`] gives, so it's the same to the last bit for notes moved to another key.
pub fn score(heard: &Heard, key: Key, span: Span, weights: &[f64]) -> f64 {
    let mut s = 0.0;
    heard.features(key, span, |i, v| s += v * weights.get(i).copied().unwrap_or(0.0));
    s
}

/// The softmax of 24 numbers, as chances adding up to 1. The terms are added smallest first, so numbers moved round
/// the keys (notes moved to another key) give the same chances moved round, to the last bit.
pub fn chances(z: [f64; 24]) -> [f64; 24] {
    let z = z.map(|v| if v.is_nan() { f64::NEG_INFINITY } else { v });
    let top = z.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !top.is_finite() {
        return [1.0 / 24.0; 24];
    }
    let raised = z.map(|v| (v - top).exp());
    let mut sorted = raised;
    sorted.sort_by(f64::total_cmp);
    let total: f64 = sorted.iter().sum();
    if total > 0.0 && total.is_finite() { raised.map(|r| r / total) } else { [1.0 / 24.0; 24] }
}

/// The finder's answer: a chance for every key, and what it heard.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyReading {
    /// The chance of each key, in the order of [`Key::ALL`]; they add up to 1.
    pub chances: [f64; 24],
    /// How many bars differ in what was heard.
    pub bars_heard: u32,
    pub config: Config,
    /// The largest share of the notes inside one scale ([`Heard::diatonic_fit`]).
    pub diatonic_fit: f64,
    /// Notes heard.
    pub notes: u32,
}

/// The answer as a person would give it, from how sure the finder is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAnswer {
    /// One key.
    Key(Key),
    /// A key or its relative ("C minor or Eb major"), the likelier first: the notes fit both, and which note is home
    /// isn't clear.
    OrRelative(Key),
    /// The home note is clear, major or minor isn't (a vamp that never plays the third, a blues).
    HomeNote(PitchClass),
    /// No clear key.
    Unclear,
}

/// A key is given alone when it's at least this likely.
pub const ONE_KEY: f64 = 0.6;
/// A key and its relative, or a home note in either mode, are given when together they're at least this likely.
pub const TWO_KEYS: f64 = 0.75;

impl KeyReading {
    /// The chance of one key.
    pub fn chance(&self, key: Key) -> f64 {
        self.chances.get(key.index()).copied().unwrap_or(0.0)
    }

    /// The keys from likeliest down, with their chances (ties go to the key first in [`Key::ALL`]).
    pub fn ranked(&self) -> Vec<(Key, f64)> {
        let mut all: Vec<(Key, f64)> = Key::ALL.iter().map(|k| (*k, self.chance(*k))).collect();
        all.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.index().cmp(&b.0.index())));
        all
    }

    /// The likeliest key and its chance.
    pub fn best(&self) -> (Key, f64) {
        self.ranked().first().copied().unwrap_or((Key::major(PitchClass::C), 0.0))
    }

    /// The answer to show, and the chance that it's right.
    pub fn answer(&self) -> (KeyAnswer, f64) {
        let (best, p) = self.best();
        if p >= ONE_KEY {
            return (KeyAnswer::Key(best), p);
        }
        let pair = p + self.chance(best.relative());
        let home = self.chance(Key::major(best.tonic)) + self.chance(Key::minor(best.tonic));
        if pair >= TWO_KEYS && pair >= home {
            (KeyAnswer::OrRelative(best), pair)
        } else if home >= TWO_KEYS {
            (KeyAnswer::HomeNote(best.tonic), home)
        } else {
            (KeyAnswer::Unclear, 1.0 - p)
        }
    }
}

/// Reads the key of what was heard with a model (the bench passes the ones it fits).
pub fn read_heard(heard: &Heard, span: Span, model: &Model) -> KeyReading {
    let (bars_heard, config) = (heard.bars_heard(), heard.config());
    let sharp = model.calibration.sharpness(bars_heard, config, span);
    let z = Key::ALL.map(|k| score(heard, k, span, model.weights) * sharp);
    KeyReading { chances: chances(z), bars_heard, config, diatonic_fit: heard.diatonic_fit(), notes: heard.notes }
}

impl KeyReader {
    /// Hears the parts on the grid and reads their key. `None` when nothing pitched is heard.
    pub fn read(&mut self, parts: &[Part], grid: Grid, span: Span) -> Option<KeyReading> {
        let heard = self.hear(parts, grid)?;
        Some(read_heard(&heard, span, &SHIPPED))
    }
}

/// Finds the key of the parts on the grid. `None` when they have no pitched notes. To read many parts, keep a
/// [`KeyReader`] and call [`KeyReader::read`], which reuses its working space.
pub fn find_key(parts: &[Part], grid: Grid, span: Span) -> Option<KeyReading> {
    KeyReader::new().read(parts, grid, span)
}

#[cfg(test)]
mod tests;
