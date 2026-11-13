//! A track's summaries added up over a song: the notes heard in each bar, everything heard so far, and a rough tempo
//! from the hits.

use std::collections::{BTreeMap, VecDeque};

use crate::ear::Heard;

/// The most bars a track keeps (the ones furthest from the newest go first): 1,024 bars is over half an hour at 120
/// beats a minute, and a few hundred kB.
pub const MOST_BARS: usize = 1024;
/// How much of the newest audio the tempo guess looks at, in seconds.
const TEMPO_SECONDS: f64 = 16.0;
/// The fewest hits a tempo guess needs.
const TEMPO_HITS: usize = 8;
/// The tempos it chooses between, in beats a minute, and the step between them.
const TEMPO_RANGE: (f64, f64, f64) = (70.0, 180.0, 0.5);

/// What a track heard in one bar.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bar {
    /// The 12 notes heard (C first), summed: each summary's chroma counted by how much audio it covered, in
    /// seconds. Not scaled, so bars can be compared and added.
    pub chroma: [f32; 12],
    /// How much audio with notes in it the bar heard, in seconds.
    pub seconds: f32,
}

/// One track's listening over a song. Feed it each [`Heard`] with where the song was ([`Track::add`]).
pub struct Track {
    rate: f64,
    beats_per_bar: f64,
    bars: BTreeMap<i64, Bar>,
    newest_bar: Option<i64>,
    all: Bar,
    /// The newest hits: stream seconds and how sharp.
    hits: VecDeque<(f64, f32)>,
}

impl Track {
    /// A track listened to at `sample_rate` (the ear's), counting 4 beats a bar.
    pub fn new(sample_rate: f64) -> Track {
        Track {
            rate: if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 },
            beats_per_bar: 4.0,
            bars: BTreeMap::new(),
            newest_bar: None,
            all: Bar::default(),
            hits: VecDeque::new(),
        }
    }

    /// Counts `beats` a bar instead (the host's time signature); a number that can't be one leaves it as it was.
    pub fn with_bar(mut self, beats: f64) -> Track {
        self.set_bar(beats);
        self
    }

    /// Changes the beats a bar (the host's time signature changed). The bars heard so far stay as they were counted.
    pub fn set_bar(&mut self, beats: f64) {
        if beats.is_finite() && beats > 0.0 {
            self.beats_per_bar = beats;
        }
    }

    /// Adds one summary. `beat` is where the song was (the host's play position, in beats from the song's start) at
    /// the end of the audio it covers, when the song is playing; without one the notes count only towards
    /// [`Track::all`].
    pub fn add(&mut self, heard: &Heard, beat: Option<f64>) {
        let weight = heard.seconds as f32;
        if heard.chroma.iter().any(|c| *c > 0.0) && weight > 0.0 {
            add_into(&mut self.all, &heard.chroma, weight);
            if let Some(bar) = beat.filter(|b| b.is_finite()).map(|b| (b / self.beats_per_bar).floor() as i64) {
                add_into(self.bars.entry(bar).or_default(), &heard.chroma, weight);
                self.newest_bar = Some(bar);
                self.keep_bars(bar);
            }
        }
        for hit in &heard.hits {
            self.hits.push_back((hit.at as f64 / self.rate, hit.rise_db));
        }
        let now = heard.samples_to as f64 / self.rate;
        while self.hits.front().is_some_and(|(at, _)| *at < now - TEMPO_SECONDS) {
            self.hits.pop_front();
        }
    }

    /// Everything the track heard, bar or no bar.
    pub fn all(&self) -> Bar {
        self.all
    }

    /// What it heard in bar `bar` (0 is the song's first bar).
    pub fn bar(&self, bar: i64) -> Option<Bar> {
        self.bars.get(&bar).copied()
    }

    /// Every bar it heard notes in, first bar first.
    pub fn bars(&self) -> impl Iterator<Item = (i64, Bar)> + '_ {
        self.bars.iter().map(|(n, b)| (*n, *b))
    }

    /// The bar it last heard notes in.
    pub fn newest_bar(&self) -> Option<i64> {
        self.newest_bar
    }

    /// Forgets everything heard (a new song).
    pub fn clear(&mut self) {
        self.bars.clear();
        self.newest_bar = None;
        self.all = Bar::default();
        self.hits.clear();
    }

    /// A rough tempo from how the newest hits are spaced, in beats a minute (70 to 180), for when the host gives
    /// none; `None` with too few hits or no steady beat. Inside a DAW, take the host's tempo instead.
    pub fn tempo(&self) -> Option<f64> {
        if self.hits.len() < TEMPO_HITS {
            return None;
        }
        let hits: Vec<(f64, f32)> = self.hits.iter().copied().collect();
        let (low, high, step) = TEMPO_RANGE;
        let (mut best, mut best_score, mut total) = (None, 0f64, 0f64);
        let mut bpm = low;
        while bpm <= high {
            let beat = 60.0 / bpm;
            let mut score = 0f64;
            let mut weights = 0f64;
            for (i, &(a, ra)) in hits.iter().enumerate() {
                for &(b, rb) in &hits[i + 1..] {
                    let gap = b - a;
                    if gap > 4.0 * beat {
                        break;
                    }
                    // A gap of whole beats scores 1, one half a beat off scores -1.
                    let w = f64::from(ra.min(rb));
                    score += w * (2.0 * std::f64::consts::PI * gap / beat).cos();
                    weights += w;
                }
            }
            let score = if weights > 0.0 { score / weights } else { 0.0 };
            total = total.max(weights);
            if score > best_score {
                (best, best_score) = (Some(bpm), score);
            }
            bpm += step;
        }
        // A steady beat lines most gaps up; scattered hits never get near.
        best.filter(|_| best_score >= 0.5 && total > 0.0)
    }

    fn keep_bars(&mut self, newest: i64) {
        while self.bars.len() > MOST_BARS {
            let (Some(first), Some(last)) =
                (self.bars.first_key_value().map(|(k, _)| *k), self.bars.last_key_value().map(|(k, _)| *k))
            else {
                return;
            };
            let far = if newest.abs_diff(first) >= newest.abs_diff(last) { first } else { last };
            self.bars.remove(&far);
        }
    }
}

fn add_into(bar: &mut Bar, chroma: &[f32; 12], seconds: f32) {
    for (sum, c) in bar.chroma.iter_mut().zip(chroma) {
        *sum += c * seconds;
    }
    bar.seconds += seconds;
}
