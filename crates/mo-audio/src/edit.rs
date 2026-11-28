//! Light edits to a sound (the 1.1 plan's "light sample edits"): trim, fades, reverse, gain, normalise and a loop
//! crossfade, a part picked out of it, and slices cut at its hits.
//!
//! Nothing here touches a file. A sound read whole goes through a list of [`Edit`]s, each applied to what the ones
//! before it made, so the list is the undo history as well: undo drops its last edit and applies the rest again. The
//! app saves what comes out as a new file; the original is never changed (rule 2).
//!
//! Every place in an edit is a frame of the sound as it is at that step (after a trim, frame 0 is where the trim
//! started), and every edit is clamped to the sound: one that reaches past the end stops there, and one left with
//! nothing to do does nothing. Cuts made inside a sound get a short fade, so nothing clicks where it was cut.

use crate::AudioData;

/// The frames from `start` up to (not including) `end`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    /// The frames between two places, whichever way round they're given.
    pub fn new(a: usize, b: usize) -> Self {
        Span { start: a.min(b), end: a.max(b) }
    }

    pub fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// This span inside a sound `frames` long, or `None` when nothing of it is.
    pub fn within(self, frames: usize) -> Option<Span> {
        let s = Span { start: self.start.min(frames), end: self.end.min(frames) };
        (!s.is_empty()).then_some(s)
    }
}

/// A fade's shape. A fade out is its fade in played backwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Curve {
    /// A straight line.
    #[default]
    Linear,
    /// Equal power (a quarter sine): no dip in the middle when two sounds cross.
    EqualPower,
    /// Slow at first and fast at the end (squared): a fade out drops fast and then tails off, as a sound decays.
    Exponential,
    /// Slow at both ends (smoothstep).
    SCurve,
}

impl Curve {
    /// How loud a fade in is `x` of the way through (0 to 1).
    pub fn gain(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Curve::Linear => x,
            Curve::EqualPower => (x * std::f32::consts::FRAC_PI_2).sin(),
            Curve::Exponential => x * x,
            Curve::SCurve => x * x * (3.0 - 2.0 * x),
        }
    }
}

/// Which way a fade goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Way {
    In,
    Out,
}

/// One edit, applied to the sound as the edits before it left it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Edit {
    /// Keeps these frames only. The end is faded out when it cuts into sound, and the start faded in.
    Trim(Span),
    /// Fades these frames in (from silence at the start to as loud as it was at the end) or out. A fade in from the
    /// sound's start, or a fade out to its end, is a corner handle's fade.
    Fade { span: Span, way: Way, curve: Curve },
    /// Plays these frames backwards (the whole sound for `None`), crossing over into them at their edges.
    Reverse(Option<Span>),
    /// Turns these frames (the whole sound for `None`) up or down by `db`, easing in and out at their edges.
    Gain { db: f32, span: Option<Span> },
    /// Turns the whole sound up or down so its loudest sample is `peak_db` (0 is full scale). A silent sound stays.
    Normalise { peak_db: f32 },
    /// Makes these frames (the whole sound for `None`) loop without a click: the loop's end is crossfaded over
    /// `frames` into the sound just before its start, so the end runs into the start as the sound itself ran on, and
    /// the start (often a downbeat's hit) stays as it was picked. With too little sound before the start, the start
    /// is crossfaded from the sound just past the end instead; with too little on either side, the crossfade is as
    /// long as there is sound to cross with. A loop of the whole sound has none: it comes out `frames` shorter.
    /// Leaves the loop only.
    LoopCrossfade { span: Option<Span>, frames: usize },
}

/// How long the fades at a cut inside a sound are (seconds): in at the start of a part, out at its end. Short enough
/// to keep a hit's attack, long enough not to click.
pub const EDGE_IN_SECONDS: f64 = 0.001;
pub const EDGE_OUT_SECONDS: f64 = 0.003;

/// How long a gain or reverse eases in and out at its edges inside a sound (seconds), at most a quarter of it.
const EASE_SECONDS: f64 = 0.002;

/// The most a gain or a normalise turns a sound up or down (dB): past it a float overflows.
const MOST_DB: f32 = 96.0;

/// Whether `audio` holds the whole of its file, so an edit of it can be saved: a read capped short of the file's
/// length (the analysis reads only a minute) mustn't be saved as if it were the sound.
pub fn is_whole(audio: &AudioData) -> bool {
    audio.sample_rate > 0 && audio.seconds() + 1.0 / f64::from(audio.sample_rate) >= audio.file_seconds * 0.999
}

/// The sound `audio` with `edits` applied in turn. Its notes (tempo, root note, loop) are kept; an edit list that
/// leaves no sound gives an empty one. What comes out is the whole of the edited sound (its `file_seconds` is its own
/// length), so check [`is_whole`] on what goes in before saving it.
pub fn apply(audio: &AudioData, edits: &[Edit]) -> AudioData {
    let mut out = audio.clone();
    // Every channel the same length, even one the file left short.
    let frames = out.channels.iter().map(Vec::len).min().unwrap_or(0);
    for ch in &mut out.channels {
        ch.truncate(frames);
        for s in ch.iter_mut() {
            if !s.is_finite() {
                *s = 0.0;
            }
        }
    }
    for edit in edits {
        apply_one(&mut out, *edit);
    }
    out.file_seconds = out.seconds();
    out
}

fn apply_one(audio: &mut AudioData, edit: Edit) {
    let frames = audio.len();
    let rate = audio.sample_rate;
    match edit {
        Edit::Trim(span) => {
            if let Some(span) = span.within(frames) {
                *audio = part(audio, span);
            }
        }
        Edit::Fade { span, way, curve } => {
            let Some(span) = span.within(frames) else { return };
            let n = span.len();
            // One frame is no fade either way.
            if n < 2 {
                return;
            }
            for ch in &mut audio.channels {
                for (i, s) in ch[span.start..span.end].iter_mut().enumerate() {
                    // From the first frame's silence to the last frame's full level (a fade in), or back.
                    let x = i as f32 / (n - 1) as f32;
                    *s *= match way {
                        Way::In => curve.gain(x),
                        Way::Out => curve.gain(1.0 - x),
                    };
                }
            }
        }
        Edit::Reverse(span) => {
            let Some(span) = span.unwrap_or(Span { start: 0, end: frames }).within(frames) else { return };
            let ease = ease_frames(rate, span, frames);
            for ch in &mut audio.channels {
                let part = &mut ch[span.start..span.end];
                // Only the edges are crossed back to what was there: only they are kept.
                let n = part.len();
                let head: Vec<f32> = part[..ease.0].to_vec();
                let tail: Vec<f32> = part[n - ease.1..].to_vec();
                part.reverse();
                cross_edges(part, &head, &tail);
            }
        }
        Edit::Gain { db, span } => {
            let Some(span) = span.unwrap_or(Span { start: 0, end: frames }).within(frames) else { return };
            if !db.is_finite() {
                return;
            }
            let g = 10f32.powf(db.clamp(-MOST_DB, MOST_DB) / 20.0);
            let (ease_in, ease_out) = ease_frames(rate, span, frames);
            for ch in &mut audio.channels {
                let part = &mut ch[span.start..span.end];
                let n = part.len();
                for (i, s) in part.iter_mut().enumerate() {
                    // How far into the gain this frame is: eased at an edge that's inside the sound.
                    let into_in = if ease_in == 0 { 1.0 } else { ((i + 1) as f32 / (ease_in + 1) as f32).min(1.0) };
                    let into_out = if ease_out == 0 { 1.0 } else { ((n - i) as f32 / (ease_out + 1) as f32).min(1.0) };
                    let x = into_in.min(into_out);
                    *s *= 1.0 + (g - 1.0) * x;
                }
            }
        }
        Edit::Normalise { peak_db } => {
            if !peak_db.is_finite() {
                return;
            }
            let peak = peak(audio);
            // Silent (under -120 dB): nothing to turn up.
            if peak < 1e-6 {
                return;
            }
            let g = 10f32.powf(peak_db.clamp(-MOST_DB, MOST_DB) / 20.0) / peak;
            if !g.is_finite() {
                return;
            }
            for s in audio.channels.iter_mut().flatten() {
                *s *= g;
            }
        }
        Edit::LoopCrossfade { span, frames: cross } => {
            let Some(span) = span.unwrap_or(Span { start: 0, end: frames }).within(frames) else { return };
            *audio = looped(audio, span, cross);
        }
    }
}

/// How many frames a gain or reverse over `span` eases over at its start and at its end: none at the sound's own
/// start or end (nothing there to click against), else [`EASE_SECONDS`], at most a quarter of the span.
fn ease_frames(rate: u32, span: Span, frames: usize) -> (usize, usize) {
    let ease = ((EASE_SECONDS * f64::from(rate)).round() as usize).min(span.len() / 4);
    (if span.start == 0 { 0 } else { ease }, if span.end >= frames { 0 } else { ease })
}

/// Crosses the start of `now` back to `head` (what its first frames were) and its end back to `tail` (what its last
/// were), so it joins what's on either side as they did.
fn cross_edges(now: &mut [f32], head: &[f32], tail: &[f32]) {
    let n = now.len();
    let (ease_in, ease_out) = (head.len().min(n), tail.len().min(n));
    for (i, (s, &was)) in now.iter_mut().zip(head).enumerate() {
        let w = (i + 1) as f32 / (ease_in + 1) as f32;
        *s = was * (1.0 - w) + *s * w;
    }
    for (j, (s, &was)) in now[n - ease_out..].iter_mut().rev().zip(tail.iter().rev()).enumerate() {
        let w = (j + 1) as f32 / (ease_out + 1) as f32;
        *s = was * (1.0 - w) + *s * w;
    }
}

/// Frames every channel of `audio` has: a file can leave one short.
fn frames_of(audio: &AudioData) -> usize {
    audio.channels.iter().map(Vec::len).min().unwrap_or(0)
}

/// The loudest sample of `audio`, on any channel (0 to 1, or more for a float file that goes over).
pub fn peak(audio: &AudioData) -> f32 {
    audio.channels.iter().flatten().filter(|s| s.is_finite()).fold(0.0f32, |m, s| m.max(s.abs()))
}

/// The frames of `span` of `audio` on their own, as dragging or saving a part makes them: faded in over
/// [`EDGE_IN_SECONDS`] when it starts inside the sound and out over [`EDGE_OUT_SECONDS`] when it ends inside it.
/// Its notes are kept, except that it's no longer known to be a loop.
pub fn part(audio: &AudioData, span: Span) -> AudioData {
    let frames = frames_of(audio);
    let span = span.within(frames).unwrap_or(Span { start: frames, end: frames });
    let mut out = AudioData {
        channels: audio.channels.iter().map(|ch| ch[span.start..span.end].to_vec()).collect(),
        embedded_is_loop: if span.len() == frames { audio.embedded_is_loop } else { None },
        ..audio.clone_notes()
    };
    ease_cuts(&mut out, span.start > 0, span.end < frames);
    out.file_seconds = out.seconds();
    out
}

/// Fades a part cut out of a sound in over [`EDGE_IN_SECONDS`] when it was cut at its start (`cut_start`) and out
/// over [`EDGE_OUT_SECONDS`] when it was cut at its end (`cut_end`), so neither cut clicks: [`part`]'s edges, for a
/// part read on its own ([`crate::read_span`]).
pub fn ease_cuts(audio: &mut AudioData, cut_start: bool, cut_end: bool) {
    let n = frames_of(audio);
    let rate = f64::from(audio.sample_rate);
    let edge_in = if cut_start { ((EDGE_IN_SECONDS * rate).round() as usize).min(n / 2) } else { 0 };
    let edge_out = if cut_end { ((EDGE_OUT_SECONDS * rate).round() as usize).min(n / 2) } else { 0 };
    for ch in &mut audio.channels {
        for (i, s) in ch.iter_mut().enumerate().take(edge_in) {
            *s *= i as f32 / edge_in as f32;
        }
        for j in 0..edge_out {
            ch[n - 1 - j] *= j as f32 / edge_out as f32;
        }
    }
}

impl AudioData {
    /// An empty sound with this one's rate and notes.
    fn clone_notes(&self) -> AudioData {
        AudioData {
            sample_rate: self.sample_rate,
            channels: Vec::new(),
            file_seconds: 0.0,
            embedded_tempo: self.embedded_tempo,
            embedded_root_note: self.embedded_root_note,
            embedded_is_loop: self.embedded_is_loop,
        }
    }
}

/// [`Edit::LoopCrossfade`]'s work: the loop `span` of `audio` (inside it) crossed over `cross` frames.
fn looped(audio: &AudioData, span: Span, cross: usize) -> AudioData {
    let frames = frames_of(audio);
    let (before, after) = (span.start, frames - span.end);
    let whole = before == 0 && after == 0;
    // Never more than half the loop, nor (unless it's the whole sound) more than the sound there is to cross with.
    let n = cross.min(span.len() / 2).min(if whole { usize::MAX } else { before.max(after) });
    let mut out = AudioData {
        channels: audio.channels.iter().map(|ch| ch[span.start..span.end].to_vec()).collect(),
        embedded_is_loop: Some(true),
        ..audio.clone_notes()
    };
    if n == 0 {
        out.file_seconds = out.seconds();
        return out;
    }
    let len = span.len();
    // The two sides that cross, as they are: the loop's end and the sound before its start, or the sound past its
    // end and its start, or (the whole sound) its end and its start.
    let (from, to, ends) = if before >= n {
        (span.end - n, span.start - n, true)
    } else if after >= n {
        (span.end, span.start, false)
    } else {
        (len - n, 0, false)
    };
    // Equal power when the two sides are unlike (two moments that don't add up as one), equal gain when they're
    // alike (a loop cut to whole cycles or bars, where equal power would swell by up to 3 dB and could clip).
    let alike = likeness(audio, from, to, n) >= 0.5;
    let curve = if alike { Curve::Linear } else { Curve::EqualPower };
    // The weights reach exactly 0 and 1 at the frames that join the loop's two ends, so the wrap is seamless.
    for (o, ch) in out.channels.iter_mut().zip(&audio.channels) {
        for i in 0..n {
            if ends {
                // The end crosses from itself into the sound before the start: its last frame is the one before it.
                let x = (i + 1) as f32 / n as f32;
                o[len - n + i] = ch[from + i] * curve.gain(1.0 - x) + ch[to + i] * curve.gain(x);
            } else {
                // The start crosses from the sound past the end (for the whole sound, its own last frames, taken off
                // the end below) into itself: its first frame is the one after the end.
                let x = i as f32 / n as f32;
                o[i] = ch[to + i] * curve.gain(x) + ch[from + i] * curve.gain(1.0 - x);
            }
        }
        if whole {
            o.truncate(len - n);
        }
    }
    out.file_seconds = out.seconds();
    out
}

/// How alike `n` frames of `audio` from `a` and from `b` are (their channels mixed): their correlation, -1 to 1 (0
/// when either is silent).
fn likeness(audio: &AudioData, a: usize, b: usize, n: usize) -> f32 {
    let (mut ab, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
    for ch in &audio.channels {
        for i in 0..n {
            let (x, y) = (f64::from(ch[a + i]), f64::from(ch[b + i]));
            ab += x * y;
            aa += x * x;
            bb += y * y;
        }
    }
    if aa <= 1e-12 || bb <= 1e-12 { 0.0 } else { (ab / (aa * bb).sqrt()) as f32 }
}

/// The parts a sound `frames` long falls into when cut at `cuts` (frames, in any order): from its start to the first
/// cut, from each cut to the next, and from the last to the end. A cut at the start or past the end, or twice at one
/// frame, counts once; a sound with no cuts is one part, and an empty one has none. The part before the first hit is
/// often silence: [`has_sound`] tells.
pub fn slices(frames: usize, cuts: &[usize]) -> Vec<Span> {
    if frames == 0 {
        return Vec::new();
    }
    let mut at: Vec<usize> = cuts.iter().copied().filter(|&c| c > 0 && c < frames).collect();
    at.sort_unstable();
    at.dedup();
    let mut starts = Vec::with_capacity(at.len() + 1);
    starts.push(0);
    starts.extend(at);
    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| Span { start, end: starts.get(i + 1).copied().unwrap_or(frames) })
        .collect()
}

/// Whether the frames of `span` of `audio` have any sound in them: a sample louder than `level` (0 to 1).
pub fn has_sound(audio: &AudioData, span: Span, level: f32) -> bool {
    let Some(span) = span.within(frames_of(audio)) else { return false };
    audio.channels.iter().any(|ch| ch[span.start..span.end].iter().any(|s| s.is_finite() && s.abs() > level))
}

/// How long the sound is (frames) before each of `edits` and after the last, `frames` long to start with: as [`apply`]
/// makes it, without doing the work.
pub fn lengths(frames: usize, edits: &[Edit]) -> Vec<usize> {
    let mut out = Vec::with_capacity(edits.len() + 1);
    let mut n = frames;
    out.push(n);
    for edit in edits {
        n = match *edit {
            Edit::Trim(span) => span.within(n).map_or(n, Span::len),
            Edit::LoopCrossfade { span, frames: cross } => match span.unwrap_or(Span { start: 0, end: n }).within(n) {
                // A loop of the whole sound comes out shorter by its crossfade ([`looped`]).
                Some(s) if s.start == 0 && s.end == n => n - cross.min(n / 2),
                Some(s) => s.len(),
                None => n,
            },
            _ => n,
        };
        out.push(n);
    }
    out
}

/// Where frame `at` of the sound `edits` made (from one `frames` long) came from in the sound before them: through a
/// trim or a loop it's as far in again as they started, through a reverse it's mirrored, and a fade, a gain or a
/// normalise leaves it where it was. So the original can be heard from the place the edit has reached. A place past
/// the end is the end.
pub fn place_before(frames: usize, edits: &[Edit], at: usize) -> usize {
    let lengths = lengths(frames, edits);
    let mut at = at.min(lengths.last().copied().unwrap_or(frames));
    for (edit, &n) in edits.iter().zip(&lengths).rev() {
        at = match *edit {
            Edit::Trim(span) => span.within(n).map_or(at, |s| at + s.start),
            Edit::LoopCrossfade { span, .. } => {
                span.unwrap_or(Span { start: 0, end: n }).within(n).map_or(at, |s| at + s.start)
            }
            Edit::Reverse(span) => mirrored(span, n, at),
            _ => at,
        }
        .min(n);
    }
    at
}

/// Where frame `at` of the sound before `edits` (`frames` long) is in the sound they make: [`place_before`] the other
/// way. A frame a trim or a loop cut off is at the edge it was cut at.
pub fn place_after(frames: usize, edits: &[Edit], at: usize) -> usize {
    let lengths = lengths(frames, edits);
    let mut at = at.min(frames);
    for (edit, n) in edits.iter().zip(lengths.windows(2)) {
        let (before, after) = (n[0], n[1]);
        at = match *edit {
            Edit::Trim(span) => span.within(before).map_or(at, |s| at.saturating_sub(s.start)),
            Edit::LoopCrossfade { span, .. } => {
                span.unwrap_or(Span { start: 0, end: before }).within(before).map_or(at, |s| at.saturating_sub(s.start))
            }
            Edit::Reverse(span) => mirrored(span, before, at),
            _ => at,
        }
        .min(after);
    }
    at
}

/// The stretch of the sound before `edits` (`frames` long) that the frames `span` of the sound they make came from:
/// the shortest that holds where every one of them did ([`place_before`]). Through a trim or a loop it's as far in
/// again as they started; a reverse that holds all of it only turns it round, and one that reaches part way into it
/// brings frames in from further off, so it widens to hold those too. None when nothing of `span` is in the sound.
pub fn came_from(frames: usize, edits: &[Edit], span: Span) -> Option<Span> {
    let lengths = lengths(frames, edits);
    let mut at = span.within(lengths.last().copied().unwrap_or(frames))?;
    for (edit, &n) in edits.iter().zip(&lengths).rev() {
        let whole = Span { start: 0, end: n };
        let moved = move |by: Span| Span { start: at.start + by.start, end: at.end + by.start };
        at = match *edit {
            Edit::Trim(span) => span.within(n).map_or(at, moved),
            Edit::LoopCrossfade { span, .. } => span.unwrap_or(whole).within(n).map_or(at, moved),
            Edit::Reverse(span) => match span.unwrap_or(whole).within(n) {
                // What's inside the reverse came from its mirror there; what's either side of it stayed.
                Some(r) if at.start < r.end && at.end > r.start => {
                    let (first, past) = (at.start.max(r.start), at.end.min(r.end));
                    Span {
                        start: if at.start < r.start { at.start } else { r.start + r.end - past },
                        end: if at.end > r.end { at.end } else { r.start + r.end - first },
                    }
                }
                _ => at,
            },
            _ => at,
        };
        at = Span { start: at.start.min(n), end: at.end.min(n) };
    }
    Some(at)
}

/// Frame `at` of a sound `n` long, after (or before) `span` of it (all of it for `None`) is reversed.
fn mirrored(span: Option<Span>, n: usize, at: usize) -> usize {
    match span.unwrap_or(Span { start: 0, end: n }).within(n) {
        Some(s) if at >= s.start && at < s.end => s.start + s.end - 1 - at,
        _ => at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn sound(channels: Vec<Vec<f32>>) -> AudioData {
        let mut a = AudioData { sample_rate: RATE, channels, ..AudioData::default() };
        a.file_seconds = a.seconds();
        a
    }

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| i as f32 / n as f32).collect()
    }

    fn sine(n: usize, hz: f32) -> Vec<f32> {
        (0..n).map(|i| (i as f32 * hz * std::f32::consts::TAU / RATE as f32).sin() * 0.5).collect()
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn no_edits_give_the_sound_back_with_damage_as_silence() {
        let mut a = sound(vec![ramp(100), ramp(90)]);
        a.channels[0][3] = f32::NAN;
        let out = apply(&a, &[]);
        assert_eq!(out.len(), 90, "every channel as long as the shortest");
        assert_eq!(out.channels[0][3], 0.0);
        assert_eq!(out.channels[1], ramp(90));
    }

    #[test]
    fn a_trim_keeps_its_frames_faded_where_it_cuts_into_sound() {
        let a = sound(vec![vec![0.5; 48_000]]);
        let out = apply(&a, &[Edit::Trim(Span::new(30_000, 12_000))]);
        assert_eq!(out.len(), 18_000);
        assert_eq!(out.channels[0][0], 0.0, "faded in from a cut");
        assert!(close(out.channels[0][100], 0.5));
        assert_eq!(out.channels[0][17_999], 0.0, "faded out at a cut");
        assert!((out.file_seconds - 0.375).abs() < 1e-9);
        // From the start: nothing to fade in there.
        let out = apply(&a, &[Edit::Trim(Span::new(0, 1_000))]);
        assert!(close(out.channels[0][0], 0.5));
        // Past the end it stops at the end; one with nothing in it does nothing.
        assert_eq!(apply(&a, &[Edit::Trim(Span::new(47_000, 90_000))]).len(), 1_000);
        assert_eq!(apply(&a, &[Edit::Trim(Span::new(60_000, 90_000))]).len(), 48_000);
        assert_eq!(apply(&a, &[Edit::Trim(Span::new(5, 5))]).len(), 48_000);
    }

    #[test]
    fn edits_apply_to_what_the_ones_before_made() {
        let a = sound(vec![ramp(48_000)]);
        // After a trim from 24,000, frame 0 is the old 24,000: the second trim keeps the old 28,800..33,600.
        let out = apply(&a, &[Edit::Trim(Span::new(24_000, 48_000)), Edit::Trim(Span::new(4_800, 9_600))]);
        assert_eq!(out.len(), 4_800);
        assert!(close(out.channels[0][2_400], 0.65));
    }

    #[test]
    fn fades_run_from_silence_to_full_in_each_shape() {
        for curve in [Curve::Linear, Curve::EqualPower, Curve::Exponential, Curve::SCurve] {
            assert_eq!(curve.gain(0.0), 0.0);
            assert!(close(curve.gain(1.0), 1.0));
            assert!(curve.gain(0.25) < curve.gain(0.75), "{curve:?} rises");
            let a = sound(vec![vec![1.0; 1_000]]);
            let fin = apply(&a, &[Edit::Fade { span: Span::new(0, 101), way: Way::In, curve }]);
            assert_eq!(fin.channels[0][0], 0.0);
            assert!(close(fin.channels[0][100], 1.0));
            assert!(close(fin.channels[0][500], 1.0), "after the fade, as it was");
            let fout = apply(&a, &[Edit::Fade { span: Span::new(899, 1_000), way: Way::Out, curve }]);
            assert!(close(fout.channels[0][899], 1.0));
            assert_eq!(fout.channels[0][999], 0.0);
            // A fade out is its fade in backwards.
            assert!(close(fout.channels[0][899 + 25], fin.channels[0][100 - 25]));
        }
        assert!(Curve::EqualPower.gain(0.5) > 0.7 && Curve::Exponential.gain(0.5) < 0.3);
        // One frame is no fade either way.
        let a = sound(vec![vec![1.0; 10]]);
        for way in [Way::In, Way::Out] {
            assert_eq!(apply(&a, &[Edit::Fade { span: Span::new(5, 6), way, curve: Curve::Linear }]), apply(&a, &[]));
        }
    }

    #[test]
    fn reversing_the_whole_sound_plays_it_backwards() {
        let a = sound(vec![ramp(1_000), vec![0.25; 1_000]]);
        let out = apply(&a, &[Edit::Reverse(None)]);
        let mut back = ramp(1_000);
        back.reverse();
        assert_eq!(out.channels[0], back);
        assert_eq!(out.channels[1], vec![0.25; 1_000]);
        // Twice is the sound again.
        assert_eq!(apply(&a, &[Edit::Reverse(None), Edit::Reverse(None)]).channels[0], ramp(1_000));
    }

    #[test]
    fn reversing_a_part_joins_its_neighbours_without_a_jump() {
        let a = sound(vec![ramp(48_000)]);
        let span = Span::new(12_000, 24_000);
        let out = apply(&a, &[Edit::Reverse(Some(span))]);
        let ch = &out.channels[0];
        // Untouched outside; in the middle, backwards.
        assert_eq!(ch[..12_000], a.channels[0][..12_000]);
        assert_eq!(ch[24_000..], a.channels[0][24_000..]);
        assert!(close(ch[18_000], a.channels[0][24_000 - 1 - 6_000]));
        // A ramp reversed jumps by a quarter at each edge; eased, no step is bigger than the jump spread over 2 ms.
        let biggest = ch.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(biggest < 0.25 / 90.0, "{biggest}");
    }

    #[test]
    fn gain_turns_a_part_up_and_eases_at_its_edges() {
        let a = sound(vec![vec![0.5; 48_000]]);
        let out = apply(&a, &[Edit::Gain { db: -6.0206, span: Some(Span::new(10_000, 20_000)) }]);
        let ch = &out.channels[0];
        assert!(close(ch[5_000], 0.5));
        assert!(close(ch[15_000], 0.25));
        assert!(close(ch[25_000], 0.5));
        let biggest = ch.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(biggest < 0.25 / 90.0, "eased: {biggest}");
        // The whole sound has no inside edge: every frame turned down.
        let all = apply(&a, &[Edit::Gain { db: 6.0206, span: None }]);
        assert!(all.channels[0].iter().all(|&s| close(s, 1.0)));
        assert_eq!(apply(&a, &[Edit::Gain { db: f32::NAN, span: None }]), apply(&a, &[]));
        // However far it's turned, every sample stays a number.
        let loud = apply(&sound(vec![vec![0.0, 1.0, 0.0]]), &[Edit::Gain { db: 900.0, span: None }]);
        assert!(loud.channels[0].iter().all(|s| s.is_finite()), "{loud:?}");
        let loud = apply(&sound(vec![vec![0.0, 1e-5, 0.0]]), &[Edit::Normalise { peak_db: 1e9 }]);
        assert!(loud.channels[0].iter().all(|s| s.is_finite()), "{loud:?}");
    }

    #[test]
    fn normalise_sets_the_loudest_sample_and_leaves_silence() {
        let mut quiet = sine(4_800, 440.0);
        quiet.iter_mut().for_each(|s| *s *= 0.2);
        let a = sound(vec![quiet.clone(), quiet]);
        let out = apply(&a, &[Edit::Normalise { peak_db: -1.0 }]);
        assert!((peak(&out) - 10f32.powf(-1.0 / 20.0)).abs() < 1e-4);
        let silent = sound(vec![vec![0.0; 100]]);
        assert_eq!(apply(&silent, &[Edit::Normalise { peak_db: 0.0 }]).channels, silent.channels);
    }

    /// The step from a loop's last frame back to its first, against the biggest step inside it.
    fn wrap_step(ch: &[f32]) -> (f32, f32) {
        let inside = ch.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        (ch.first().zip(ch.last()).map_or(0.0, |(f, l)| (f - l).abs()), inside)
    }

    #[test]
    fn a_loop_of_a_part_keeps_its_length_and_runs_on_into_its_start() {
        // A 440 Hz tone cut at a point mid-cycle: as it is, it jumps where it loops.
        let a = sound(vec![sine(48_000, 440.0)]);
        let span = Span::new(10_000, 30_037);
        let raw = part(&a, span);
        let (jump, _) = wrap_step(&raw.channels[0]);
        let out = apply(&a, &[Edit::LoopCrossfade { span: Some(span), frames: 480 }]);
        assert_eq!(out.len(), span.len(), "the loop keeps its length");
        assert_eq!(out.embedded_is_loop, Some(true));
        let (wrap, inside) = wrap_step(&out.channels[0]);
        assert!(wrap <= inside * 1.01, "{wrap} > {inside} (it jumped {jump} before)");
        // At the end of the sound there's nothing after it: the sound before the start crosses in instead.
        let span = Span::new(10_037, 48_000);
        let out = apply(&a, &[Edit::LoopCrossfade { span: Some(span), frames: 480 }]);
        assert_eq!(out.len(), span.len());
        let (wrap, inside) = wrap_step(&out.channels[0]);
        assert!(wrap <= inside * 1.01, "{wrap} > {inside}");
    }

    #[test]
    fn a_loop_keeps_its_start_as_picked_when_there_is_sound_before_it() {
        let a = sound(vec![sine(48_000, 440.0)]);
        let span = Span::new(10_000, 30_037);
        let out = apply(&a, &[Edit::LoopCrossfade { span: Some(span), frames: 480 }]);
        assert_eq!(out.channels[0][..19_000], a.channels[0][10_000..29_000], "the downbeat is untouched");
        // With no sound before it, the start crosses from the sound after the end, and the length is kept.
        let span = Span::new(100, 20_037);
        let out = apply(&a, &[Edit::LoopCrossfade { span: Some(span), frames: 480 }]);
        assert_eq!(out.len(), span.len());
        let (wrap, inside) = wrap_step(&out.channels[0]);
        assert!(wrap <= inside * 1.01, "{wrap} > {inside}");
        // Little sound on either side: a shorter crossfade, and still the same length.
        let a = sound(vec![sine(10_000, 440.0)]);
        let out = apply(&a, &[Edit::LoopCrossfade { span: Some(Span::new(100, 9_900)), frames: 480 }]);
        assert_eq!(out.len(), 9_800);
        let (wrap, inside) = wrap_step(&out.channels[0]);
        assert!(wrap <= inside * 1.01, "{wrap} > {inside}");
    }

    #[test]
    fn a_loop_cut_to_whole_cycles_doesnt_swell_where_it_crosses() {
        // 480 Hz at 48 kHz: 100 frames a cycle, and the loop is 200 whole cycles.
        let a = sound(vec![sine(48_000, 480.0).iter().map(|s| s * 1.6).collect()]);
        let out = apply(&a, &[Edit::LoopCrossfade { span: Some(Span::new(10_000, 30_000)), frames: 2_000 }]);
        assert!(peak(&out) <= 0.8 + 1e-3, "{}", peak(&out));
    }

    #[test]
    fn a_loop_of_the_whole_sound_comes_out_shorter_by_its_crossfade() {
        let a = sound(vec![sine(30_037, 440.0), sine(30_037, 220.0)]);
        let out = apply(&a, &[Edit::LoopCrossfade { span: None, frames: 1_000 }]);
        assert_eq!(out.len(), 29_037);
        for ch in &out.channels {
            let (wrap, inside) = wrap_step(ch);
            assert!(wrap <= inside * 1.01, "{wrap} > {inside}");
        }
        // A crossfade longer than half the loop is half of it; none is the loop as it was.
        assert_eq!(apply(&a, &[Edit::LoopCrossfade { span: None, frames: 1_000_000 }]).len(), 30_037 - 15_018);
        assert_eq!(apply(&a, &[Edit::LoopCrossfade { span: None, frames: 0 }]).channels, a.channels);
    }

    #[test]
    fn a_part_is_faded_only_where_it_cuts_into_sound() {
        let a = sound(vec![vec![0.5; 4_800]]);
        let mid = part(&a, Span::new(1_000, 2_000));
        assert_eq!(mid.len(), 1_000);
        assert_eq!(mid.channels[0][0], 0.0);
        assert!(close(mid.channels[0][48], 0.5), "faded in over 1 ms");
        assert_eq!(mid.channels[0][999], 0.0);
        assert!(close(mid.channels[0][999 - 144], 0.5), "faded out over 3 ms");
        let whole = part(&a, Span::new(0, 4_800));
        assert_eq!(whole.channels, a.channels);
        assert_eq!(part(&a, Span::new(9_000, 9_500)).len(), 0, "nothing of it is there");
        // A channel the file left short: the part stops where it does.
        let short = sound(vec![vec![0.5; 100], vec![0.5; 50]]);
        assert_eq!(part(&short, Span::new(10, 90)).channels.iter().map(Vec::len).collect::<Vec<_>>(), [40, 40]);
        assert!(!has_sound(&sound(vec![vec![0.0; 100], vec![0.5; 50]]), Span::new(60, 90), 0.1));
    }

    #[test]
    fn slices_run_from_each_cut_to_the_next() {
        let s = |v: &[(usize, usize)]| v.iter().map(|&(a, b)| Span::new(a, b)).collect::<Vec<_>>();
        assert_eq!(slices(100, &[50, 20, 20, 0, 100, 300],), s(&[(0, 20), (20, 50), (50, 100)]));
        // A hit at the very start is the first slice's start, never lost.
        assert_eq!(slices(100, &[0, 50]), s(&[(0, 50), (50, 100)]));
        assert_eq!(slices(100, &[]), s(&[(0, 100)]));
        assert!(slices(0, &[]).is_empty());
    }

    #[test]
    fn only_a_whole_read_is_whole() {
        let mut a = sound(vec![vec![0.0; 48_000]]);
        assert!(is_whole(&a));
        a.file_seconds = 180.0;
        assert!(!is_whole(&a), "a read capped at a second of three minutes");
        assert!(!is_whole(&AudioData::default()));
    }

    #[test]
    fn sound_is_told_from_silence() {
        let mut ch = vec![0.0; 1_000];
        ch[700] = 0.2;
        let a = sound(vec![ch]);
        assert!(!has_sound(&a, Span::new(0, 500), 0.01));
        assert!(has_sound(&a, Span::new(600, 800), 0.01));
        assert!(!has_sound(&a, Span::new(2_000, 3_000), 0.01));
    }

    #[test]
    fn lengths_are_as_the_edits_make_them() {
        let a = sound(vec![sine(48_000, 220.0)]);
        let lists: [&[Edit]; 4] = [
            &[],
            &[Edit::Trim(Span::new(1_000, 40_000)), Edit::Reverse(None), Edit::Gain { db: -3.0, span: None }],
            &[Edit::LoopCrossfade { span: None, frames: 480 }, Edit::Trim(Span::new(100, 90_000))],
            &[
                Edit::LoopCrossfade { span: Some(Span::new(2_000, 30_000)), frames: 960 },
                Edit::Trim(Span::new(50_000, 60_000)),
                Edit::Normalise { peak_db: 0.0 },
            ],
        ];
        for edits in lists {
            let lengths = lengths(a.len(), edits);
            assert_eq!(lengths.len(), edits.len() + 1);
            for (i, &n) in lengths.iter().enumerate() {
                assert_eq!(n, apply(&a, &edits[..i]).len(), "after {:?}", &edits[..i]);
            }
        }
    }

    #[test]
    fn a_place_in_the_edit_is_found_in_the_sound_before_it_and_back() {
        // Each frame holds its own number, so where a frame came from can be read off it.
        let a = sound(vec![(0..48_000).map(|i| i as f32).collect()]);
        let edits = [
            Edit::Trim(Span::new(1_000, 41_000)),
            Edit::Reverse(Some(Span::new(5_000, 20_000))),
            Edit::Fade { span: Span::new(0, 480), way: Way::In, curve: Curve::Linear },
            Edit::LoopCrossfade { span: Some(Span::new(2_000, 30_000)), frames: 200 },
        ];
        let out = apply(&a, &edits);
        // Away from the fades, the crossfade and the eased edges of the reverse.
        for at in [10, 500, 2_999, 3_100, 10_000, 17_000, 17_900, 18_200, 25_000, 27_500] {
            let from = place_before(a.len(), &edits, at);
            assert_eq!(out.channels[0][at], from as f32, "frame {at}");
            assert_eq!(place_after(a.len(), &edits, from), at, "and back, from {from}");
        }
        // Cut off: at the edge it was cut at. Past the end: the end.
        assert_eq!(place_after(a.len(), &edits, 0), 0);
        assert_eq!(place_after(a.len(), &edits, 47_000), out.len());
        assert_eq!(place_before(a.len(), &edits, 1_000_000), place_before(a.len(), &edits, out.len()));
        // No edits: where it was.
        assert_eq!(place_before(a.len(), &[], 123), 123);
        assert_eq!(place_after(a.len(), &[], 123), 123);
    }

    #[test]
    fn the_stretch_a_part_of_the_edit_came_from_holds_every_frame_of_it_and_no_more() {
        let frames = 240;
        let lists: [&[Edit]; 6] = [
            &[],
            &[Edit::Trim(Span::new(40, 200))],
            &[Edit::Reverse(None), Edit::Gain { db: -3.0, span: None }],
            // A reverse inside a part, one over its edge, and one past it.
            &[Edit::Trim(Span::new(20, 220)), Edit::Reverse(Some(Span::new(50, 90))), Edit::Normalise { peak_db: 0.0 }],
            &[
                Edit::Reverse(Some(Span::new(10, 120))),
                Edit::LoopCrossfade { span: Some(Span::new(30, 200)), frames: 8 },
                Edit::Reverse(Some(Span::new(100, 160))),
                Edit::Trim(Span::new(5, 150)),
            ],
            &[
                Edit::LoopCrossfade { span: None, frames: 16 },
                Edit::Fade { span: Span::new(0, 9), way: Way::In, curve: Curve::Linear },
            ],
        ];
        for edits in lists {
            let after = lengths(frames, edits).last().copied().unwrap();
            for start in (0..after).step_by(7) {
                for end in (start + 1..=after).step_by(5) {
                    let places = (start..end).map(|at| place_before(frames, edits, at));
                    let (first, last) = places.fold((usize::MAX, 0), |(a, b), p| (a.min(p), b.max(p)));
                    let from = came_from(frames, edits, Span { start, end });
                    assert_eq!(from, Some(Span { start: first, end: last + 1 }), "{start}..{end} of {edits:?}");
                }
            }
            // Nothing of it in the sound: nowhere. Reaching past the end: what there is of it.
            assert_eq!(came_from(frames, edits, Span { start: after, end: after + 10 }), None);
            assert_eq!(came_from(frames, edits, Span { start: 3, end: 3 }), None);
            assert_eq!(
                came_from(frames, edits, Span { start: 0, end: after + 10 }),
                came_from(frames, edits, Span { start: 0, end: after })
            );
        }
    }
}
