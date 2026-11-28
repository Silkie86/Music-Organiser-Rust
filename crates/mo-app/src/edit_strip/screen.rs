//! The Edit strip's screen with no window (Solomon's "Build it like this" of 27 Nov 2026, pictures 12 to 14): a
//! sound's outline, kept fine enough to draw any stretch of it at once; the waveform over the stretch on show, a
//! column to each pixel ([`Wave`]), which changes shape under a fade while its handle is dragged; and the picture a
//! host draws of it ([`scene`]), on the graphics card or the processor, so the app and the plugin show the same
//! screen.
//!
//! The picture is everything on the screen that stays put between two shows: the ruler's lines, the hits' marks along
//! the foot, and the waveform, its peaks see-through around a solid body, red where it would clip. It's drawn three
//! times, one under the other: as it is, dim (what's outside the part picked) and lit (what has played), so a host
//! only moves a window over each as the part is dragged or the sound plays. What moves (the part picked, the play
//! head, the fades' handles, the cuts) is the host's own, laid over it.

use std::sync::Arc;

use mo_audio::AudioData;
use mo_audio::edit::Curve;

use crate::shapes::{Colour, MOST_PIXELS, Scene};

/// How fine a sound's outline is kept: a step every quarter of a millisecond (half a million steps for two minutes, 6
/// MB), so even a zoom in on a short part draws at once.
const STEP_SECONDS: f64 = 0.000_25;

/// The shortest stretch a column's body is measured over (seconds): a narrower column takes its body from this much
/// of the sound around it, so the body stays a calm shape inside the peaks.
const BODY_SECONDS: f64 = 0.003;

/// How far a body reaches for how loud its stretch is on the whole (the root of its mean square): a little further,
/// so a plain tone's body all but fills its peaks and a hit's peaks stand well out of its body.
const BODY_REACH: f32 = 1.2;

/// From how wide a column (seconds) the waveform is drawn the same above and below the middle, as far as the sound
/// gets either way; and under how wide it's drawn as it is, each side as far as the sound goes on that side. From far
/// out a low note's waves are only a few columns each, and drawn as they are (now above the middle, now below) they'd
/// be a comb where there's a filled shape to see; from near, they're the note's own shape. In between, part way.
const EVEN_FROM: f64 = 0.004;
const AS_IT_IS_UNDER: f64 = 0.002;

/// The shape of the corner fades, as the strip makes them ([`super::CORNER`]).
const CORNER: Curve = super::CORNER;

/// What a fade can't bring back by turning up: under this much of full level, what's left of the sound says too little
/// about what it was.
const LEAST_GAIN: f64 = 1e-4;

/// A column a fade turns up or down by less than this is as it was.
const SAME: f64 = 1e-6;

/// A sound's outline: of each step [`STEP_SECONDS`] long (as near as whole frames go), the highest and the lowest
/// sample on any channel (never under the middle and never over it: a step all on one side reaches from the middle, so
/// the waveform is a filled shape and not a thin line) and the mean of every sample's square; and the loudest sample on
/// the left and on the right.
pub struct Outline {
    /// How long each step is (seconds).
    step: f64,
    high: Box<[f32]>,
    low: Box<[f32]>,
    power: Box<[f32]>,
    peaks: [f32; 2],
}

impl Outline {
    /// The outline of `audio`. Never on the window's thread: it reads every sample.
    pub(super) fn of(audio: &AudioData) -> Outline {
        let rate = f64::from(audio.sample_rate.max(1));
        let each = ((rate * STEP_SECONDS).round() as usize).max(1);
        let frames = audio.channels.iter().map(Vec::len).min().unwrap_or(0);
        let steps = frames.div_ceil(each);
        let (mut high, mut low, mut power) =
            (Vec::with_capacity(steps), Vec::with_capacity(steps), Vec::with_capacity(steps));
        for i in 0..steps {
            let (from, to) = (i * each, ((i + 1) * each).min(frames));
            let (mut top, mut bottom, mut squares) = (0.0f32, 0.0f32, 0.0f64);
            for ch in &audio.channels {
                for s in ch[from..to].iter().filter(|s| s.is_finite()) {
                    top = top.max(*s);
                    bottom = bottom.min(*s);
                    squares += f64::from(*s) * f64::from(*s);
                }
            }
            let samples = ((to - from) * audio.channels.len()).max(1);
            high.push(top);
            low.push(bottom);
            power.push((squares / samples as f64) as f32);
        }
        let loudest =
            |ch: &Vec<f32>| ch[..frames].iter().filter(|s| s.is_finite()).fold(0.0f32, |most, s| most.max(s.abs()));
        let left = audio.channels.first().map_or(0.0, loudest);
        let right = audio.channels.get(1).map_or(left, loudest);
        Outline {
            step: each as f64 / rate,
            high: high.into(),
            low: low.into(),
            power: power.into(),
            peaks: [left, right],
        }
    }

    /// The loudest sample on the left and on the right (the one channel's twice, for a sound with one): 1 is full
    /// scale.
    pub(super) fn peaks(&self) -> [f32; 2] {
        self.peaks
    }

    /// The loudest sample of all.
    pub(super) fn peak(&self) -> f32 {
        self.peaks[0].max(self.peaks[1])
    }

    /// The steps that hold the stretch from `from` to `to` seconds: at least one, none past the end.
    fn steps(&self, from: f64, to: f64) -> std::ops::Range<usize> {
        let n = self.high.len();
        let first = ((from / self.step).floor().max(0.0) as usize).min(n);
        let past = ((to / self.step).ceil().max(0.0) as usize).clamp(first, n);
        first..past.max(first + 1).min(n)
    }

    /// The highest and the lowest the sound gets from `from` to `to` seconds.
    fn reach(&self, from: f64, to: f64) -> (f32, f32) {
        let steps = self.steps(from, to);
        let top = self.high[steps.clone()].iter().fold(f32::MIN, |m, v| m.max(*v));
        let bottom = self.low[steps].iter().fold(f32::MAX, |m, v| m.min(*v));
        if top < bottom { (0.0, 0.0) } else { (top, bottom) }
    }

    /// How far the body reaches either side of the middle from `from` to `to` seconds ([`BODY_REACH`]).
    fn body(&self, from: f64, to: f64) -> f32 {
        let steps = self.steps(from, to);
        if steps.is_empty() {
            return 0.0;
        }
        let count = steps.len();
        let mean = self.power[steps].iter().map(|p| f64::from(*p)).sum::<f64>() / count as f64;
        mean.max(0.0).sqrt() as f32 * BODY_REACH
    }

    /// The outline as the tests look at it: the highest and the lowest of each step.
    #[cfg(test)]
    pub(super) fn steps_for_tests(&self) -> (&[f32], &[f32]) {
        (&self.high, &self.low)
    }
}

/// One column of a waveform, a pixel wide: how far above and below the middle it reaches (1 is full scale, and it may
/// go over; from near, the highest and the lowest the sound gets there, and from far out the further of the two both
/// ways: [`EVEN_FROM`]), and how far its body reaches either side of the middle.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Column {
    pub top: f32,
    pub bottom: f32,
    pub body: f32,
}

/// The highest and the lowest `audio` gets from `from` to `to` seconds, from its samples themselves, with a straight
/// line from each to the next: for a column narrower than the outline's steps, so a zoom right in shows the wave
/// itself, one column joined to the next.
fn trace(audio: &AudioData, from: f64, to: f64) -> (f32, f32) {
    let rate = f64::from(audio.sample_rate.max(1));
    let frames = audio.channels.iter().map(Vec::len).min().unwrap_or(0);
    if frames == 0 {
        return (0.0, 0.0);
    }
    let last = (frames - 1) as f64;
    let (first, second) = ((from * rate).clamp(0.0, last), (to * rate).clamp(0.0, last));
    let (mut top, mut bottom) = (f32::MIN, f32::MAX);
    let mut take = |v: f32| {
        if v.is_finite() {
            top = top.max(v);
            bottom = bottom.min(v);
        }
    };
    for ch in &audio.channels {
        // Where the line between two samples is, part way from one to the next.
        let at = |place: f64| {
            let before = place.floor() as usize;
            let (a, b) = (ch[before], ch[(before + 1).min(frames - 1)]);
            a + (b - a) * (place - before as f64) as f32
        };
        take(at(first));
        take(at(second));
        let (inside_from, inside_to) = (first.ceil() as usize, second.floor() as usize);
        if inside_from <= inside_to {
            ch[inside_from..=inside_to].iter().for_each(|s| take(*s));
        }
    }
    if top < bottom { (0.0, 0.0) } else { (top, bottom) }
}

/// The waveform of `audio` (whose outline is `outline`) as `count` columns, each `per` seconds wide, the first
/// starting `from` seconds into what's shown. `place` says where in `audio` a moment of what's shown is: itself for
/// the edit; through its edits, for the original under it (not a number, where it's nowhere in it).
pub(super) fn columns(
    audio: &AudioData,
    outline: &Outline,
    from: f64,
    per: f64,
    count: usize,
    place: &dyn Fn(f64) -> f64,
) -> Vec<Column> {
    if !(per.is_finite() && per > 0.0 && from.is_finite()) {
        return vec![Column::default(); count];
    }
    let seconds = audio.seconds();
    // Narrower than two of the outline's steps: from the samples themselves.
    let fine = per < 2.0 * outline.step;
    // How far towards the same above and below each column is drawn ([`EVEN_FROM`]).
    let even = ((per - AS_IT_IS_UNDER) / (EVEN_FROM - AS_IT_IS_UNDER)).clamp(0.0, 1.0) as f32;
    (0..count)
        .map(|i| {
            let middle = place(from + (i as f64 + 0.5) * per);
            // Outside the sound (or nowhere in it, for a place that has none): nothing.
            if !(middle >= 0.0 && middle < seconds) {
                return Column::default();
            }
            let (start, end) = (middle - per / 2.0, middle + per / 2.0);
            if fine {
                // The wave itself, solid.
                let (top, bottom) = trace(audio, start, end);
                Column { top, bottom, body: top.abs().max(bottom.abs()) }
            } else {
                let (top, bottom) = outline.reach(start, end);
                let far = top.max(-bottom);
                let (top, bottom) =
                    if even >= 1.0 { (far, -far) } else { (top + (far - top) * even, bottom - (far + bottom) * even) };
                let half = per.max(BODY_SECONDS) / 2.0;
                Column { top, bottom, body: outline.body(middle - half, middle + half) }
            }
        })
        .collect()
}

/// How much of a sound a corner fade `long` seconds long lets through, `into` seconds in from its corner (the sound's
/// start for a fade in, its end for a fade out): all of it with no fade, or past it.
fn corner_gain(into: f64, long: f64) -> f64 {
    if long <= 0.0 || into >= long { 1.0 } else { f64::from(CORNER.gain((into.max(0.0) / long) as f32)) }
}

/// How much louder or quieter a moment `into` seconds in from a corner is with that corner's fade `now` seconds long
/// than with it `was` seconds long.
fn corner_change(into: f64, was: f64, now: f64) -> f64 {
    let (before, after) = (corner_gain(into, was), corner_gain(into, now));
    if before > LEAST_GAIN {
        after / before
    } else if now > 0.0 {
        // Right in the corner both fades are near silence: what they come to there, one against the other.
        (was / now).min(after / LEAST_GAIN)
    } else {
        after / LEAST_GAIN
    }
}

/// `columns` (each `per` seconds wide, the first starting `from` seconds into a sound `seconds` long) as they are with
/// the sound's corner fades `now` long (in and out, seconds) rather than `was`: each column turned up or down as the
/// fades change it there, with no edit made. With it, the first column that changes and one past the last (none when
/// nothing does).
///
/// A fade is measured at the edge of the column further from its corner: that's where a faded column is loudest, so
/// where its top and bottom came from, and a fade taken away again gives back just what it took.
pub(super) fn refaded(
    columns: &[Column],
    (from, per, seconds): (f64, f64, f64),
    was: (f64, f64),
    now: (f64, f64),
) -> (Vec<Column>, Option<(usize, usize)>) {
    let mut changed: Option<(usize, usize)> = None;
    let half = per / 2.0;
    let out = columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let at = from + (i as f64 + 0.5) * per;
            let by = corner_change(at + half, was.0, now.0) * corner_change(seconds - at + half, was.1, now.1);
            if (by - 1.0).abs() < SAME || !by.is_finite() {
                return *c;
            }
            changed = Some(changed.map_or((i, i + 1), |(first, _)| (first, i + 1)));
            let by = by as f32;
            Column { top: c.top * by, bottom: c.bottom * by, body: c.body * by }
        })
        .collect();
    (out, changed)
}

/// The room the strip's screen has, as its host lays it out, in points: how wide and high the whole of it is, how far
/// in from its sides the waveform starts and ends, how high the ruler's band along its top is, where a waveform at full
/// scale reaches up to and down to (from the screen's top; what's under it is the foot, with the hits' marks), and how
/// many pixels there are to a point.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Room {
    pub width: f64,
    pub height: f64,
    pub inset: f64,
    pub band: f64,
    pub top: f64,
    pub bottom: f64,
    pub scale: f64,
}

impl Room {
    /// Whether there's a screen to draw: a few points of waveform each way, under the band and inside the screen.
    pub fn fits(&self) -> bool {
        let all = [self.width, self.height, self.inset, self.band, self.top, self.bottom, self.scale];
        all.iter().all(|v| v.is_finite() && *v >= 0.0)
            && self.scale > 0.0
            && self.width - 2.0 * self.inset >= 8.0
            && self.top >= self.band
            && self.bottom - self.top >= 8.0
            && self.bottom <= self.height
    }

    /// How wide the waveform is, in pixels (seldom a whole number of them).
    fn wave_pixels(&self) -> f64 {
        ((self.width - 2.0 * self.inset) * self.scale).max(1.0)
    }

    /// How many columns the waveform has: one to a pixel.
    pub(super) fn columns(&self) -> usize {
        (self.wave_pixels().ceil() as usize).clamp(1, MOST_PIXELS as usize)
    }

    /// How long a column lasts when the waveform shows `seconds`.
    pub(super) fn per_column(&self, seconds: f64) -> f64 {
        seconds / self.wave_pixels()
    }
}

/// How a stretch of the sound was before a fade, for its outline: from which column on, and each one's top and
/// bottom.
pub type Before = (usize, Arc<[(f32, f32)]>);

/// What the strip's screen draws that stays put between two shows: the waveform over the stretch on show and the marks
/// behind it. [`scene`] makes the picture of it; a host draws that again only when the strip hands it another `Wave`.
#[derive(Clone, Debug, PartialEq)]
pub struct Wave {
    /// The room it was worked out for.
    pub room: Room,
    /// A column to each pixel across the waveform.
    pub columns: Arc<[Column]>,
    /// While a fade's handle is dragged, how the sound was before.
    pub before: Option<Before>,
    /// It's the original (O is held): in grey.
    pub original: bool,
    /// The ruler's marks: where (0 to 1 across the waveform), and whether it's a bar's (or a whole second's).
    pub marks: Vec<(f32, bool)>,
    /// The hits in sight, 0 to 1 across, from the left.
    pub hits: Vec<f32>,
}

/// The screen's colours, as the host's theme has them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Colours {
    /// Under everything (0xRRGGBB): the well the screen sits in.
    pub well: u32,
    /// The lines behind the waveform: the band's, one down from each of the ruler's marks, and the middle's.
    pub line: Colour,
    /// The hits' marks along the foot.
    pub mark: Colour,
    /// The waveform: the colour of the sound's type.
    pub tint: Colour,
    /// The waveform of the original, while O is held.
    pub grey: Colour,
    /// What has played, and the outline of how the sound was before a fade.
    pub lit: Colour,
    /// What has played of the original.
    pub bright: Colour,
    /// What's over full scale.
    pub over: Colour,
}

/// How much of the waveform's colour its peaks show, around the solid body; and of what has played's colour.
const PEAKS: f64 = 0.55;
const LIT_PEAKS: f64 = 0.72;
/// How much of its grey the original shows: its peaks, and its body.
const ORIGINAL_PEAKS: f64 = 0.5;
const ORIGINAL_BODY: f64 = 0.9;
/// How much of that the waveform shows where it's dim: outside the part picked.
const DIM: f64 = 0.3;
/// How many screens the picture is, one under the other ([`scene`]).
pub const SCREENS: usize = 3;
/// How much of its colour the outline of how the sound was shows.
const BEFORE: f64 = 0.4;
/// How much of the lines' colour each line shows: a bar's, a beat's, the band's and the middle's.
const BAR_LINE: f64 = 0.85;
const BEAT_LINE: f64 = 0.36;
const BAND_LINE: f64 = 0.3;
const MID_LINE: f64 = 0.42;
/// A hit's mark along the foot: how much of the marks' colour it shows, how wide and how high it is, how far above
/// the screen's foot it ends, and how near another may come before the two are one (all in points).
const HIT_MARK: f64 = 0.6;
const HIT_WIDE: f64 = 1.5;
const HIT_HIGH: f64 = 5.0;
const HIT_UP: f64 = 2.0;
const HITS_APART: f64 = 3.0;
/// How high the cap on a column that's over full scale is, in points.
const OVER_CAP: f64 = 2.2;

/// The picture of `wave`, in pixels (draw it at one pixel to a point): the screen as it is; under that, the same with
/// the waveform dim, for what's outside the part picked; and under that, with it lit, for what has played. Each is as
/// big as the room, so the whole picture is [`SCREENS`] times as high, and a host shows of each the stretch that's
/// so. A room with no screen to draw ([`Room::fits`]) is only its background.
pub fn scene(wave: &Wave, colours: &Colours) -> Scene {
    let room = wave.room;
    let s = if room.scale.is_finite() && room.scale > 0.0 { room.scale } else { 1.0 };
    let most = f64::from(MOST_PIXELS);
    let size = |points: f64, most: f64| if points.is_finite() { (points * s).ceil().clamp(1.0, most) } else { 1.0 };
    // All of them fit in the biggest picture.
    let screens = SCREENS as f64;
    let (width, height) = (size(room.width, most), size(room.height, (most / screens).floor()));
    let mut scene = Scene::new(width, screens * height, colours.well);
    if !room.fits() {
        return scene;
    }
    // A line a point wide, in whole pixels.
    let hair = s.round().max(1.0);
    // The waveform starts on a whole pixel, so each of its columns is one.
    let left = (room.inset * s).round();
    let wide = (room.width - 2.0 * room.inset) * s;
    let band = (room.band * s).round();
    let (top, bottom) = ((room.top * s).round(), (room.bottom * s).round());
    // Where each screen starts down the picture: as it is, dim, lit.
    let [plain_at, dim_at, lit_at] = [0.0, height, 2.0 * height];
    let all = [plain_at, dim_at, lit_at];
    // The pixel a moment (0 to 1 across the waveform) is in: its column's.
    let x_of = |at: f32| (left + f64::from(at) * wide).floor();
    // The lines behind the waveform, the same on every screen: one down from each of the ruler's marks, the band's
    // across the whole screen, and one along the middle.
    for (bar, shows) in [(true, BAR_LINE), (false, BEAT_LINE)] {
        for down in all {
            for &(at, _) in wave.marks.iter().filter(|m| m.1 == bar) {
                scene.rect((x_of(at), down + band, hair, height - band), colours.line.showing(shows));
            }
        }
    }
    let middle = (top + bottom) / 2.0;
    for down in all {
        scene.rect((0.0, down + band - hair, width, hair), colours.line.showing(BAND_LINE));
        scene.rect((left, down + middle - hair / 2.0, wide, hair), colours.line.showing(MID_LINE));
    }
    // The hits, short marks along the foot: where a part's edges snap to.
    let hit_wide = (HIT_WIDE * s).round().max(1.0);
    let hit_high = (HIT_HIGH * s).round().max(1.0);
    let hit_top = height - (HIT_UP * s).round() - hit_high;
    for down in all {
        let mut last = f64::NEG_INFINITY;
        for &at in &wave.hits {
            let x = x_of(at);
            // Hits a few points apart are one mark.
            if x - last >= HITS_APART * s {
                let from = x - ((hit_wide - 1.0) / 2.0).floor();
                scene.rect((from, down + hit_top, hit_wide, hit_high), colours.mark.showing(HIT_MARK));
                last = x;
            }
        }
    }
    // The waveform, a column to a pixel.
    let reach = ((bottom - top) / 2.0).max(1.0);
    // Where a level (1 is full scale) is down the screen, kept to full scale.
    let y_of = |level: f32| middle - f64::from(level.clamp(-1.0, 1.0)) * reach;
    // A column's top and bottom on the screen: never thinner than a line, so silence is a line along the middle.
    let ends = |high: f32, low: f32| {
        let (a, b) = (y_of(high), y_of(low));
        if b - a >= hair { (a, b) } else { ((a + b - hair) / 2.0, (a + b + hair) / 2.0) }
    };
    let (plain, peaks_show, body_show, lit) = if wave.original {
        (colours.grey, ORIGINAL_PEAKS, ORIGINAL_BODY, colours.bright)
    } else {
        (colours.tint, PEAKS, 1.0, colours.lit)
    };
    let cap = (OVER_CAP * s).round().max(1.0);
    for (down, peaks, body) in [
        (plain_at, plain.showing(peaks_show), plain.showing(body_show)),
        (dim_at, plain.showing(peaks_show * DIM), plain.showing(body_show * DIM)),
        (lit_at, lit.showing(LIT_PEAKS), lit),
    ] {
        for (i, c) in wave.columns.iter().enumerate() {
            let (a, b) = ends(c.top, c.bottom);
            scene.rect((left + i as f64, down + a, 1.0, b - a), peaks);
        }
        for (i, c) in wave.columns.iter().enumerate() {
            // The body, inside the peaks.
            let (a, b) = ends(c.top.min(c.body), c.bottom.max(-c.body));
            scene.rect((left + i as f64, down + a, 1.0, b - a), body);
        }
        // What's over full scale: a red cap where it's cut off.
        for (i, c) in wave.columns.iter().enumerate() {
            if c.top > 1.0 {
                scene.rect((left + i as f64, down + y_of(1.0), 1.0, cap), colours.over);
            }
            if c.bottom < -1.0 {
                scene.rect((left + i as f64, down + y_of(-1.0) - cap, 1.0, cap), colours.over);
            }
        }
    }
    // How the sound was before the fade being dragged: a thin outline, each column's joined to the next one's.
    // Nothing plays as a handle is dragged, so the lit screen has none.
    if let Some((first, before)) = &wave.before {
        let outline = colours.lit.showing(BEFORE);
        for (down, edge) in [(plain_at, 0), (plain_at, 1), (dim_at, 0), (dim_at, 1)] {
            let y = |i: usize| {
                let (high, low) = before[i];
                if edge == 0 { y_of(high) } else { y_of(low) - hair }
            };
            for i in 0..before.len() {
                let (here, next) = (y(i), y((i + 1).min(before.len() - 1)));
                let (a, b) = (here.min(next), here.max(next) + hair);
                scene.rect((left + (first + i) as f64, down + a, 1.0, b - a), outline);
            }
        }
    }
    scene
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::Shape;

    fn sound(rate: u32, channels: Vec<Vec<f32>>) -> AudioData {
        let mut a = AudioData { sample_rate: rate, channels, ..AudioData::default() };
        a.file_seconds = a.seconds();
        a
    }

    fn itself(at: f64) -> f64 {
        at
    }

    #[test]
    fn an_outline_keeps_the_highest_the_lowest_and_the_power_of_each_step() {
        // 8 kHz: a step is two frames.
        let mut left = vec![0.5f32; 16];
        left[4] = 0.9;
        left[5] = -0.3;
        let mut right = vec![0.0f32; 16];
        right[5] = -0.7;
        right[9] = f32::NAN;
        let o = Outline::of(&sound(8_000, vec![left, right]));
        assert_eq!(o.high.len(), 8);
        assert!((o.step - 0.000_25).abs() < 1e-12);
        assert_eq!((o.high[2], o.low[2]), (0.9, -0.7));
        assert_eq!((o.high[0], o.low[0]), (0.5, 0.0));
        // Step 0: 0.5, 0.5 on the left and silence on the right.
        assert!((o.power[0] - 0.125).abs() < 1e-6);
        assert_eq!(o.peaks(), [0.9, 0.7]);
        assert_eq!(o.peak(), 0.9);
        // One channel is both sides.
        assert_eq!(Outline::of(&sound(8_000, vec![vec![-0.25; 4]])).peaks(), [0.25, 0.25]);
        // No sound at all.
        let none = Outline::of(&sound(8_000, Vec::new()));
        assert_eq!((none.high.len(), none.peak()), (0, 0.0));
        assert_eq!(none.reach(0.0, 1.0), (0.0, 0.0));
        assert_eq!(none.body(0.0, 1.0), 0.0);
    }

    #[test]
    fn columns_come_from_the_outline_or_from_the_samples_themselves() {
        // A second at 8 kHz: a ramp from -1 up to 1.
        let ramp: Vec<f32> = (0..8_000).map(|i| i as f32 / 4_000.0 - 1.0).collect();
        let audio = sound(8_000, vec![ramp]);
        let outline = Outline::of(&audio);
        // The whole of it on 1,000 columns: 1 ms each, from the outline, as it is.
        let whole = columns(&audio, &outline, 0.0, 0.001, 1_000, &itself);
        assert_eq!(whole.len(), 1_000);
        // A column all on one side of the middle reaches from the middle: a filled shape, not a thin line.
        assert!(whole[0].bottom <= -0.99 && whole[0].top == 0.0, "{:?}", whole[0]);
        assert!(whole[999].top >= 0.99 && whole[999].bottom == 0.0, "{:?}", whole[999]);
        assert!(whole.windows(2).all(|w| w[1].top >= w[0].top), "it goes up all the way");
        // Its body reaches as far as the ramp is loud there: next to nothing in the middle, most at its ends.
        assert!(whole[500].body < 0.05 && whole[0].body > 1.1, "{:?} {:?}", whole[500], whole[0]);
        // On 100 columns, 10 ms each, it's from far out: the same above and below, as far as it gets either way.
        let far = columns(&audio, &outline, 0.0, 0.01, 100, &itself);
        assert!(far.iter().all(|c| c.top == -c.bottom), "{:?}", far[0]);
        assert!(far[0].top >= 0.99 && far[50].top < 0.03 && far[99].top >= 0.99, "{:?} {:?}", far[0], far[50]);
        // Ten samples on 100 columns: a tenth of a sample each, from the samples, joined by straight lines.
        let near = columns(&audio, &outline, 0.5, 0.1 / 8_000.0, 100, &itself);
        assert!(near.windows(2).all(|w| (w[1].bottom - w[0].top).abs() < 1e-6), "each starts where the last ended");
        assert!((near[0].bottom - 0.0).abs() < 1e-6 && (near[99].top - 10.0 / 4_000.0).abs() < 1e-5);
        assert!(near.iter().all(|c| c.body >= c.top.abs().max(c.bottom.abs())), "the wave itself is solid");
        // Past the sound's end, and before its start: nothing.
        let past = columns(&audio, &outline, 0.99, 0.01, 3, &itself);
        assert!(past[0].top > 0.9 && past[1] == Column::default() && past[2] == Column::default());
        assert_eq!(columns(&audio, &outline, -0.02, 0.01, 3, &itself)[..2], [Column::default(); 2]);
        // A column no time wide: nothing, and no panic.
        assert_eq!(columns(&audio, &outline, 0.0, 0.0, 2, &itself), vec![Column::default(); 2]);
        // Through a place that turns it round, it comes down instead; where the place is nowhere, nothing.
        let turned = columns(&audio, &outline, 0.0, 0.001, 1_000, &|at| 1.0 - at);
        assert!(turned[0].top >= 0.99 && turned[999].bottom <= -0.99);
        assert_eq!(columns(&audio, &outline, 0.0, 0.01, 2, &|_| f64::NAN), vec![Column::default(); 2]);
    }

    #[test]
    fn a_low_note_is_a_filled_shape_from_far_out_and_its_own_wave_from_near() {
        // A second of a 50 Hz note at 8 kHz, as loud as it goes: a wave every 20 ms.
        let note: Vec<f32> = (0..8_000).map(|i| (std::f32::consts::TAU * 50.0 * i as f32 / 8_000.0).sin()).collect();
        let audio = sound(8_000, vec![note]);
        let outline = Outline::of(&audio);
        // The whole loop it might be in, from far out: 7 ms a column, three columns to a wave. Drawn as it is, every
        // third column would stop at the middle; it's the same above and below instead, near full scale all along.
        let far = columns(&audio, &outline, 0.0, 0.007, 140, &itself);
        assert!(far.iter().all(|c| c.top > 0.85 && c.bottom < -0.85), "{:?}", far.iter().find(|c| c.top <= 0.85));
        // From near, 1 ms a column, it's the note's own wave: columns all above the middle, then all below.
        let near = columns(&audio, &outline, 0.0, 0.001, 1_000, &itself);
        assert!(near.iter().any(|c| c.top > 0.9 && c.bottom == 0.0), "a column all above the middle");
        assert!(near.iter().any(|c| c.bottom < -0.9 && c.top == 0.0), "and one all below");
        // In between it's part way: every column reaches both ways, the nearer side less far.
        let between = columns(&audio, &outline, 0.0, 0.003, 333, &itself);
        assert!(between.iter().all(|c| c.top > 0.1 && c.bottom < -0.1), "no column stops at the middle");
        assert!(between.iter().any(|c| c.top < 0.9 * -c.bottom), "but the two sides still differ");
    }

    #[test]
    fn a_fade_dragged_changes_the_columns_at_once() {
        let flat = vec![Column { top: 0.8, bottom: -0.8, body: 0.6 }; 100];
        // A second on 100 columns; no fades before, a fade out of 0.4 s now.
        let (now, changed) = refaded(&flat, (0.0, 0.01, 1.0), (0.0, 0.0), (0.0, 0.4));
        // The column the fade starts in is still as loud where it starts.
        assert_eq!(changed, Some((61, 100)), "only the fade's columns change");
        assert_eq!(now[60], flat[60]);
        assert!(now[61].top < 0.8 && now[99].top < 0.05, "{:?} {:?}", now[61], now[99]);
        assert!(now.windows(2).all(|w| w[1].top <= w[0].top), "it only goes down");
        // The fade made shorter again, from what the longer one left: back up to what it was, where it no longer
        // reaches.
        let (back, changed) = refaded(&now, (0.0, 0.01, 1.0), (0.0, 0.4), (0.0, 0.1));
        assert_eq!(changed, Some((61, 100)));
        assert!((back[70].top - 0.8).abs() < 1e-3 && (back[89].top - 0.8).abs() < 1e-3, "{:?}", back[70]);
        assert!(back[99].top < 0.13);
        // Taken away altogether: all of it back, right to the last column.
        let (none, _) = refaded(&now, (0.0, 0.01, 1.0), (0.0, 0.4), (0.0, 0.0));
        assert!(none.iter().all(|c| (c.top - 0.8).abs() < 1e-3), "{:?}", none[99]);
        // A fade in works from the start, and nothing changes with the fades as they were.
        let (faded_in, changed) = refaded(&flat, (0.0, 0.01, 1.0), (0.0, 0.0), (0.2, 0.0));
        assert_eq!(changed, Some((0, 19)));
        assert!(faded_in[0].top < 0.07 && faded_in[19] == flat[19]);
        assert_eq!(refaded(&flat, (0.0, 0.01, 1.0), (0.1, 0.3), (0.1, 0.3)), (flat.clone(), None));
    }

    fn room() -> Room {
        Room { width: 120.0, height: 100.0, inset: 10.0, band: 20.0, top: 24.0, bottom: 96.0, scale: 1.0 }
    }

    #[test]
    fn a_room_gives_a_column_to_each_pixel() {
        let room = Room { width: 820.0, height: 200.0, inset: 10.0, band: 18.0, top: 23.0, bottom: 188.0, scale: 1.25 };
        assert!(room.fits());
        assert_eq!(room.columns(), 1_000);
        assert!((room.per_column(4.0) - 0.004).abs() < 1e-12);
        assert!(!Room { width: 20.0, ..room }.fits());
        assert!(!Room { bottom: 30.0, ..room }.fits(), "no height for a waveform");
        assert!(!Room { top: 10.0, ..room }.fits(), "full scale starts under the band");
        assert!(!Room { bottom: 201.0, ..room }.fits(), "and ends inside the screen");
        assert!(!Room { scale: 0.0, ..room }.fits());
        assert!(!Room { width: f64::NAN, ..room }.fits());
        assert!(!Room::default().fits());
    }

    fn rects(scene: &Scene) -> Vec<(f32, f32, f32, f32, Colour)> {
        scene
            .shapes
            .iter()
            .filter_map(|s| match *s {
                Shape::Rect { x, y, w, h, fill } => Some((x, y, w, h, fill)),
                _ => None,
            })
            .collect()
    }

    fn colours() -> Colours {
        Colours {
            well: 0x0E0E0E,
            line: Colour::rgb(0x323232, 1.0),
            mark: Colour::rgb(0xA3A3A3, 1.0),
            tint: Colour::rgb(0x9AB8D6, 1.0),
            grey: Colour::rgb(0x8E8E8E, 1.0),
            lit: Colour::rgb(0xE6EDF5, 1.0),
            bright: Colour::rgb(0xF2F2F2, 1.0),
            over: Colour::rgb(0xF0616D, 1.0),
        }
    }

    #[test]
    fn the_picture_is_the_screen_three_times_as_it_is_dim_and_lit() {
        let mut columns = vec![Column { top: 0.5, bottom: -0.25, body: 0.2 }; 100];
        // One that's over full scale, and one that's silent.
        columns[10] = Column { top: 1.5, bottom: -1.2, body: 0.9 };
        columns[20] = Column::default();
        let wave = Wave {
            room: room(),
            columns: columns.into(),
            before: None,
            original: false,
            marks: vec![(0.0, true), (0.5, false), (1.0, true)],
            hits: vec![0.25, 0.255, 0.75],
        };
        let c = colours();
        let scene = scene(&wave, &c);
        assert_eq!((scene.width, scene.height, scene.background), (120.0, 300.0, c.well));
        let all = rects(&scene);
        // Full scale reaches from 24 down to 96: the middle is 60, 36 either way.
        let peaks = c.tint.showing(PEAKS);
        let first = all.iter().find(|r| r.4 == peaks).expect("the peaks");
        assert_eq!((first.0, first.1, first.2, first.3), (10.0, 42.0, 1.0, 27.0));
        // Its body, solid, inside its peaks.
        let body = all.iter().find(|r| r.4 == c.tint).expect("the bodies");
        assert!((body.1 - 52.8).abs() < 1e-4 && (body.3 - 14.4).abs() < 1e-4, "{body:?}");
        // The same again dim, a room lower, and lit, two rooms lower.
        let dim = all.iter().find(|r| r.4 == c.tint.showing(DIM)).expect("dim bodies");
        assert!((dim.1 - 152.8).abs() < 1e-4, "{dim:?}");
        let lit = all.iter().find(|r| r.4 == c.lit).expect("lit bodies");
        assert!((lit.1 - 252.8).abs() < 1e-4, "{lit:?}");
        assert_eq!(all.iter().filter(|r| r.4 == peaks).count(), 100);
        assert_eq!(all.iter().filter(|r| r.4 == c.tint.showing(PEAKS * DIM)).count(), 100);
        assert_eq!(all.iter().filter(|r| r.4 == c.lit.showing(LIT_PEAKS)).count(), 100);
        assert!(all.iter().all(|r| r.4 != c.bright), "the original's own lit colour is the original's alone");
        // The column over full scale is cut off at it, with a red cap at each end, on every screen.
        let over: Vec<_> = all.iter().filter(|r| r.4 == c.over).collect();
        assert_eq!(over.len(), 6);
        assert_eq!((over[0].0, over[0].1, over[0].3), (20.0, 24.0, 2.0));
        assert_eq!((over[1].1, over[1].3), (94.0, 2.0));
        let cut = all.iter().find(|r| r.4 == peaks && r.0 == 20.0).expect("that column");
        assert_eq!((cut.1, cut.3), (24.0, 72.0));
        // Silence is a line along the middle, on the middle's own line.
        let silent = all.iter().find(|r| r.4 == peaks && r.0 == 30.0).expect("the silent column");
        assert_eq!((silent.1, silent.3), (59.5, 1.0));
        let middle: Vec<_> = all.iter().filter(|r| r.4 == c.line.showing(MID_LINE)).collect();
        assert_eq!(middle.len(), 3);
        assert_eq!((middle[0].0, middle[0].1, middle[0].2, middle[0].3), (10.0, 59.5, 100.0, 1.0));
        // The band's line runs across the whole screen, on each of them; the marks' lines run from it to the foot.
        let band = c.line.showing(BAND_LINE);
        let lines: Vec<_> = all.iter().filter(|r| r.4 == band).collect();
        assert_eq!(lines.len(), 3);
        assert_eq!((lines[0].0, lines[0].1, lines[0].2, lines[0].3), (0.0, 19.0, 120.0, 1.0));
        assert_eq!((lines[1].1, lines[2].1), (119.0, 219.0));
        let bars: Vec<_> = all.iter().filter(|r| r.4 == c.line.showing(BAR_LINE)).collect();
        assert_eq!(bars.len(), 6);
        assert_eq!((bars[0].0, bars[0].1, bars[0].3), (10.0, 20.0, 80.0));
        assert_eq!(bars[1].0, 110.0);
        assert_eq!(all.iter().filter(|r| r.4 == c.line.showing(BEAT_LINE)).count(), 3);
        // The hits along the foot, two points wide here and five high, ending two above it: two of them half a point
        // apart are one mark.
        let hits: Vec<_> = all.iter().filter(|r| r.4 == c.mark.showing(HIT_MARK)).collect();
        assert_eq!(hits.len(), 6);
        assert_eq!((hits[0].0, hits[0].1, hits[0].2, hits[0].3), (35.0, 93.0, 2.0, 5.0));
        assert_eq!((hits[1].0, hits[3].1, hits[5].1), (85.0, 193.0, 293.0));
    }

    #[test]
    fn the_original_is_grey_and_a_dragged_fade_shows_how_it_was() {
        let room = Room { width: 40.0, height: 60.0, inset: 0.0, band: 10.0, top: 12.0, bottom: 50.0, scale: 2.0 };
        let wave = Wave {
            room,
            columns: vec![Column { top: 0.5, bottom: -0.5, body: 0.3 }; 80].into(),
            before: Some((70, vec![(1.0, -1.0); 10].into())),
            original: true,
            marks: Vec::new(),
            hits: Vec::new(),
        };
        let c = colours();
        let scene = scene(&wave, &c);
        // Two pixels to a point: twice the room, three times over.
        assert_eq!((scene.width, scene.height), (80.0, 360.0));
        let all = rects(&scene);
        assert!(all.iter().all(|r| r.4 != c.tint && r.4 != c.tint.showing(PEAKS)), "grey, not the type's colour");
        assert_eq!(all.iter().filter(|r| r.4 == c.grey.showing(ORIGINAL_BODY)).count(), 80);
        assert_eq!(all.iter().filter(|r| r.4 == c.grey.showing(ORIGINAL_BODY * DIM)).count(), 80);
        // What has played of it is in its own bright colour, not the edit's.
        assert_eq!(all.iter().filter(|r| r.4 == c.bright).count(), 80);
        assert!(all.iter().all(|r| r.4 != c.lit));
        // The waveform keeps to full scale (24 to 100), on the first screen.
        let lines = [c.line.showing(BAND_LINE), c.line.showing(MID_LINE)];
        assert!(all.iter().filter(|r| r.1 < 120.0 && !lines.contains(&r.4)).all(|r| r.1 >= 24.0 && r.1 + r.3 <= 100.0));
        // How it was: a line along the top and one along the bottom of the last ten columns, as it is and dim.
        let before: Vec<_> = all.iter().filter(|r| r.4 == c.lit.showing(BEFORE)).collect();
        assert_eq!(before.len(), 40);
        assert!(before.iter().all(|r| r.0 >= 70.0 && r.1 < 240.0 && r.3 == 2.0), "{before:?}");
        assert_eq!(before.iter().filter(|r| r.1 < 120.0).count(), 20);
        // A room with no waveform to draw is only its background.
        let none = Wave { room: Room { bottom: 14.0, ..room }, ..wave };
        assert!(super::scene(&none, &c).shapes.is_empty());
    }
}
