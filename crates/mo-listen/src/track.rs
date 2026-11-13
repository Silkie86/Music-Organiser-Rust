//! A track's summaries added up over a song: the notes heard in each bar, everything heard so far, and a rough tempo
//! from the hits.

use std::collections::{BTreeMap, VecDeque};

use crate::ear::{Ear, Heard};

/// The most bars a track keeps (the ones furthest from the newest go first): 1,024 bars is over half an hour at 120
/// beats a minute, and a few hundred kB.
pub const MOST_BARS: usize = 1024;
/// How much of the newest audio the tempo guess looks at, in seconds.
const TEMPO_SECONDS: f64 = 16.0;
/// The fewest hits a tempo guess needs.
const TEMPO_HITS: usize = 8;
/// The tempos it chooses between, in beats a minute, and the step between them.
const TEMPO_RANGE: (f64, f64, f64) = (70.0, 180.0, 0.5);
/// The score a tempo needs (1 is every gap whole beats), and how far above the average of all the tempos.
const TEMPO_SCORE: f64 = 0.7;
const TEMPO_CLEAR: f64 = 0.2;

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
    /// The last summary's end: its stream sample and the song's beat then (when playing).
    last: Option<(u64, Option<f64>)>,
}

impl Track {
    /// A track listened to by `ear`, counting 4 beats a bar.
    pub fn for_ear(ear: &Ear) -> Track {
        Track::new(ear.sample_rate())
    }

    /// A track listened to at `sample_rate` (the ear's: [`Track::for_ear`] can't get it wrong), counting 4 beats a
    /// bar.
    pub fn new(sample_rate: f64) -> Track {
        Track {
            rate: crate::rate(sample_rate),
            beats_per_bar: 4.0,
            bars: BTreeMap::new(),
            newest_bar: None,
            all: Bar::default(),
            hits: VecDeque::new(),
            last: None,
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
    /// [`Track::all`]. The notes go to the bar the middle of their window was in: worked out from the beat the last
    /// summary ended at when the song played on smoothly since, else the bar `beat` is in.
    pub fn add(&mut self, heard: &Heard, beat: Option<f64>) {
        let beat = beat.filter(|b| b.is_finite());
        if self.last.is_some_and(|(to, _)| heard.samples_to < to) {
            // The stream began again (a new tap): the hits before belong to another count.
            self.hits.clear();
        }
        let weight = heard.chroma_seconds.min(heard.seconds) as f32;
        if heard.chroma.iter().any(|c| *c > 0.0) && weight > 0.0 {
            add_into(&mut self.all, &heard.chroma, weight);
            if let Some(end) = beat {
                let middle = end - f64::from(weight) / 2.0 * self.beats_a_second(heard, end).unwrap_or(0.0);
                let bar = (middle / self.beats_per_bar).floor() as i64;
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
        self.last = Some((heard.samples_to, beat));
    }

    /// How fast the song's beats went during `heard`, when it played on smoothly from the last summary (between 30
    /// and 600 beats a minute; a jump, a loop or a stop gives none).
    fn beats_a_second(&self, heard: &Heard, end: f64) -> Option<f64> {
        let (to, Some(was)) = self.last? else { return None };
        let seconds = heard.samples_to.checked_sub(to)? as f64 / self.rate;
        let speed = (end - was) / seconds;
        (seconds > 0.0 && (0.5..=10.0).contains(&speed)).then_some(speed)
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
        self.last = None;
    }

    /// A rough tempo from how the newest hits are spaced, in beats a minute (70 to 180), for when the host gives
    /// none; `None` with too few hits or no steady beat. Inside a DAW, take the host's tempo instead. It weighs every
    /// tempo against every pair of hits, so call it now and then (once a second), not on every summary.
    pub fn tempo(&self) -> Option<f64> {
        if self.hits.len() < TEMPO_HITS {
            return None;
        }
        let (low, high, step) = TEMPO_RANGE;
        let steps = ((high - low) / step).round() as usize;
        let mut scores = Vec::with_capacity(steps + 1);
        for n in 0..=steps {
            let beat = 60.0 / (low + n as f64 * step);
            let (mut score, mut weights) = (0f64, 0f64);
            for (i, &(a, ra)) in self.hits.iter().enumerate() {
                for &(b, rb) in self.hits.range(i + 1..) {
                    let gap = b - a;
                    if gap > 4.0 * beat {
                        break;
                    }
                    // A gap of whole beats scores 1, one of half a beat (off-beats, eighth notes) 0.5, anything else
                    // less: the beat and its halves line up, scattered hits don't.
                    let turn = 2.0 * std::f64::consts::PI * gap / beat;
                    let w = f64::from(ra.min(rb));
                    score += w * (2.0 + turn.cos() + (2.0 * turn).cos()) / 4.0;
                    weights += w;
                }
            }
            scores.push(if weights > 0.0 { score / weights } else { 0.0 });
        }
        let mean = scores.iter().sum::<f64>() / scores.len().max(1) as f64;
        let (best, score) = scores.iter().copied().enumerate().max_by(|a, b| a.1.total_cmp(&b.1))?;
        // A steady beat stands well clear of the other tempos; scattered hits score about the same for all.
        (score >= TEMPO_SCORE && score - mean >= TEMPO_CLEAR).then_some(low + best as f64 * step)
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
