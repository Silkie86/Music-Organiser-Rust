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
    /// Makes these frames (the whole sound for `None`) loop without a click: the sound just past the loop's end (or
    /// just before its start) is crossfaded over `frames` into its start (or its end), so the end runs into the start
    /// as the sound itself ran on. The loop keeps its length when there is sound outside it to cross with; a loop
    /// of the whole sound has none, and comes out `frames` shorter. Leaves the loop only.
    LoopCrossfade { span: Option<Span>, frames: usize },
}

/// How long the fades at a cut inside a sound are (seconds): in at the start of a part, out at its end. Short enough
/// to keep a hit's attack, long enough not to click.
pub const EDGE_IN_SECONDS: f64 = 0.001;
pub const EDGE_OUT_SECONDS: f64 = 0.003;

/// How long a gain or reverse eases in and out at its edges inside a sound (seconds), at most a quarter of it.
const EASE_SECONDS: f64 = 0.002;

/// The sound `audio` with `edits` applied in turn. Its notes (tempo, root note, loop) are kept; an edit list that
/// leaves no sound gives an empty one.
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
            for ch in &mut audio.channels {
                for (i, s) in ch[span.start..span.end].iter_mut().enumerate() {
                    // From the first frame's silence to the last frame's full level (a fade in), or back.
                    let x = if n <= 1 { 1.0 } else { i as f32 / (n - 1) as f32 };
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
                let before: Vec<f32> = ch[span.start..span.end].to_vec();
                ch[span.start..span.end].reverse();
                cross_edges(&mut ch[span.start..span.end], &before, ease);
            }
        }
        Edit::Gain { db, span } => {
            let Some(span) = span.unwrap_or(Span { start: 0, end: frames }).within(frames) else { return };
            if !db.is_finite() {
                return;
            }
            let g = 10f32.powf(db / 20.0);
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
            let g = 10f32.powf(peak_db / 20.0) / peak;
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

/// Crosses `now` back to `before` over its first `ease.0` frames and its last `ease.1`, so it joins what's on either
/// side as `before` did.
fn cross_edges(now: &mut [f32], before: &[f32], (ease_in, ease_out): (usize, usize)) {
    let n = now.len();
    for i in 0..ease_in.min(n) {
        let w = (i + 1) as f32 / (ease_in + 1) as f32;
        now[i] = before[i] * (1.0 - w) + now[i] * w;
    }
    for j in 0..ease_out.min(n) {
        let i = n - 1 - j;
        let w = (j + 1) as f32 / (ease_out + 1) as f32;
        now[i] = before[i] * (1.0 - w) + now[i] * w;
    }
}

/// The loudest sample of `audio`, on any channel (0 to 1, or more for a float file that goes over).
pub fn peak(audio: &AudioData) -> f32 {
    audio.channels.iter().flatten().filter(|s| s.is_finite()).fold(0.0f32, |m, s| m.max(s.abs()))
}

/// The frames of `span` of `audio` on their own, as dragging or saving a part makes them: faded in over
/// [`EDGE_IN_SECONDS`] when it starts inside the sound and out over [`EDGE_OUT_SECONDS`] when it ends inside it.
/// Its notes are kept, except that it's no longer known to be a loop.
pub fn part(audio: &AudioData, span: Span) -> AudioData {
    let frames = audio.len();
    let span = span.within(frames).unwrap_or(Span { start: frames, end: frames });
    let rate = f64::from(audio.sample_rate);
    let mut out = AudioData {
        channels: audio.channels.iter().map(|ch| ch[span.start..span.end].to_vec()).collect(),
        embedded_is_loop: if span.len() == frames { audio.embedded_is_loop } else { None },
        ..audio.clone_notes()
    };
    let n = span.len();
    let edge_in = if span.start > 0 { ((EDGE_IN_SECONDS * rate).round() as usize).min(n / 2) } else { 0 };
    let edge_out = if span.end < frames { ((EDGE_OUT_SECONDS * rate).round() as usize).min(n / 2) } else { 0 };
    for ch in &mut out.channels {
        for (i, s) in ch.iter_mut().enumerate().take(edge_in) {
            *s *= i as f32 / edge_in as f32;
        }
        for j in 0..edge_out {
            ch[n - 1 - j] *= j as f32 / edge_out as f32;
        }
    }
    out.file_seconds = out.seconds();
    out
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
    let frames = audio.len();
    let n = cross.min(span.len() / 2);
    let mut out = AudioData {
        channels: audio.channels.iter().map(|ch| ch[span.start..span.end].to_vec()).collect(),
        embedded_is_loop: Some(true),
        ..audio.clone_notes()
    };
    if n == 0 {
        out.file_seconds = out.seconds();
        return out;
    }
    // Equal power: what crosses is two different moments of the sound, which don't cancel or add up like one. The
    // weights reach exactly 0 and 1 at the frames that join the loop's two ends, so the wrap is seamless.
    let fade_in = |x: f32| Curve::EqualPower.gain(x);
    let fade_out = |x: f32| Curve::EqualPower.gain(1.0 - x);
    let at = |i: usize| i as f32 / n as f32;
    let after = |i: usize| (i + 1) as f32 / n as f32;
    let len = span.len();
    if frames - span.end >= n {
        // The sound after the end runs on into the start: the start crosses from it to itself.
        for (o, ch) in out.channels.iter_mut().zip(&audio.channels) {
            for i in 0..n {
                o[i] = ch[span.start + i] * fade_in(at(i)) + ch[span.end + i] * fade_out(at(i));
            }
        }
    } else if span.start >= n {
        // The sound before the start runs on from the end: the end crosses from itself to it.
        for (o, ch) in out.channels.iter_mut().zip(&audio.channels) {
            for i in 0..n {
                o[len - n + i] = ch[span.end - n + i] * fade_out(after(i)) + ch[span.start - n + i] * fade_in(after(i));
            }
        }
    } else {
        // The whole sound: its last `n` frames cross into its start, and are taken off the end.
        for o in &mut out.channels {
            for i in 0..n {
                o[i] = o[i] * fade_in(at(i)) + o[len - n + i] * fade_out(at(i));
            }
            o.truncate(len - n);
        }
    }
    out.file_seconds = out.seconds();
    out
}

/// The parts a sound `frames` long falls into when cut at `cuts` (frames, in any order): from each cut to the next,
/// the last to the end. Cuts at the start, past the end, or twice at one frame count once; a sound with no cuts is
/// one part. The part before the first cut is left out when `skip_lead` says to (silence before the first hit).
pub fn slices(frames: usize, cuts: &[usize], skip_lead: bool) -> Vec<Span> {
    let mut at: Vec<usize> = cuts.iter().copied().filter(|&c| c > 0 && c < frames).collect();
    at.sort_unstable();
    at.dedup();
    let mut starts = Vec::with_capacity(at.len() + 1);
    if !skip_lead || at.is_empty() {
        starts.push(0);
    }
    starts.extend(at);
    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| Span { start, end: starts.get(i + 1).copied().unwrap_or(frames) })
        .collect()
}

/// Whether the frames of `span` of `audio` have any sound in them: a sample louder than `level` (0 to 1).
pub fn has_sound(audio: &AudioData, span: Span, level: f32) -> bool {
    let Some(span) = span.within(audio.len()) else { return false };
    audio.channels.iter().any(|ch| ch[span.start..span.end].iter().any(|s| s.is_finite() && s.abs() > level))
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
    }

    #[test]
    fn slices_run_from_each_cut_to_the_next() {
        let s = |v: &[(usize, usize)]| v.iter().map(|&(a, b)| Span::new(a, b)).collect::<Vec<_>>();
        assert_eq!(slices(100, &[50, 20, 20, 0, 100, 300], false), s(&[(0, 20), (20, 50), (50, 100)]));
        assert_eq!(slices(100, &[50, 20], true), s(&[(20, 50), (50, 100)]));
        assert_eq!(slices(100, &[], true), s(&[(0, 100)]));
        assert_eq!(slices(0, &[], false), s(&[(0, 0)]));
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
}
