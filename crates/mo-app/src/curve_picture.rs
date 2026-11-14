//! The curve editor's picture, worked out with no screen: where a beat and a value sit in it ([`Frame`]), how much
//! of the curve is in sight ([`View`], the zoom), the still part of the picture as a [`Scene`] for either painter
//! ([`picture`]: the value guides, the time grid, the clips' seams, the highlight's band, the fill under the curve and
//! the curve itself), and the ruler's marks ([`ruler`]).
//!
//! What moves with the mouse (the dots, the handles, the play line, the read-out) is a few small items the page lays
//! over this picture, placed with the same [`Frame`], so a hover never draws the curve again.

use crate::curve_edit::{Grid, Shape, Span};
use crate::shapes::{Colour, Scene};

/// The room at the picture's left and right: a handle at the curve's start or end stays inside. The ruler and the
/// clip strip keep the same room, so a bar line runs straight down through all three.
pub const SIDE: f64 = 14.0;
/// The room at the picture's top and bottom: the top one holds the highlight's bar.
pub const TOP: f64 = 18.0;
/// Beats in a bar.
pub const BAR: f64 = 4.0;
/// The least time the picture can show across, in beats: zoomed in to a 1/16 note.
pub const CLOSEST: f64 = 0.25;
/// Grid lines closer than this, in points, aren't drawn; nor are the ruler's beats numbered closer than
/// [`NUMBERED`].
const TIGHT: f64 = 5.0;
const NUMBERED: f64 = 12.0;

/// How much of the curve is in sight: from `start`, `span` beats across.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub start: f64,
    pub span: f64,
}

impl View {
    /// The whole of a curve `length` beats long.
    pub fn whole(length: f64) -> View {
        View { start: 0.0, span: if length.is_finite() && length > 0.0 { length } else { BAR } }
    }

    /// Kept inside a curve `length` beats long: never closer than [`CLOSEST`] (or the curve, when it's shorter),
    /// never wider than the curve, never past either end.
    pub fn within(self, length: f64) -> View {
        let whole = View::whole(length);
        if !self.start.is_finite() || !self.span.is_finite() {
            return whole;
        }
        let span = self.span.clamp(CLOSEST.min(whole.span), whole.span);
        View { start: self.start.clamp(0.0, whole.span - span), span }
    }

    /// Zoomed by `by` (above 1 closer) with the beat `around` staying where it is on the screen (the wheel over the
    /// ruler zooms round the mouse).
    pub fn zoomed(self, around: f64, by: f64, length: f64) -> View {
        if !by.is_finite() || by <= 0.0 || !around.is_finite() {
            return self.within(length);
        }
        let at = if self.span > 0.0 { (around - self.start) / self.span } else { 0.5 };
        // The new span first, as `within` allows it, so the beat stays put even at the closest and widest zoom.
        let span = View { start: 0.0, span: self.span / by }.within(length).span;
        View { start: around - at * span, span }.within(length)
    }

    /// Moved along by `by` beats.
    pub fn moved(self, by: f64, length: f64) -> View {
        View { start: self.start + if by.is_finite() { by } else { 0.0 }, ..self }.within(length)
    }

    /// Whether the whole curve is in sight.
    pub fn is_whole(self, length: f64) -> bool {
        self.start < 1e-9 && self.span > View::whole(length).span - 1e-9
    }
}

/// The picture's size in points and what's in sight: where a beat and a value sit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub width: f64,
    pub height: f64,
    pub view: View,
}

impl Frame {
    /// Whether there's room to draw in.
    pub fn fits(self) -> bool {
        let sizes = [self.width, self.height, self.view.start, self.view.span];
        sizes.iter().all(|v| v.is_finite())
            && self.width > 2.0 * SIDE + 8.0
            && self.height > 2.0 * TOP + 8.0
            && self.view.span > 0.0
    }

    /// The width the time in sight is drawn over.
    fn across(self) -> f64 {
        (self.width - 2.0 * SIDE).max(1.0)
    }

    /// The height the values 0 to 1 are drawn over.
    fn tall(self) -> f64 {
        (self.height - 2.0 * TOP).max(1.0)
    }

    /// How many points a beat is wide.
    pub fn beat_width(self) -> f64 {
        self.across() / self.view.span
    }

    /// Where `beat` is, in points from the picture's left.
    pub fn x(self, beat: f64) -> f64 {
        SIDE + (beat - self.view.start) * self.beat_width()
    }

    /// Where `value` (0 to 1) is, in points from the picture's top: 1 at the top.
    pub fn y(self, value: f64) -> f64 {
        TOP + (1.0 - value) * self.tall()
    }

    /// The beat at `x` points from the picture's left (it may be outside what's in sight).
    pub fn beat_at(self, x: f64) -> f64 {
        self.view.start + (x - SIDE) / self.beat_width()
    }

    /// The value at `y` points from the picture's top, kept within 0 to 1.
    pub fn value_at(self, y: f64) -> f64 {
        (1.0 - (y - TOP) / self.tall()).clamp(0.0, 1.0)
    }
}

/// The picture's colours, as the window's theme has them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Colours {
    /// Under everything (0xRRGGBB): the well the picture sits in (`Theme.well`).
    pub well: u32,
    /// The guides and the grid (`Theme.stroke`).
    pub stroke: Colour,
    /// The curve and the fill under it (`Theme.text`).
    pub text: Colour,
    /// The clips' seams (`Theme.muted`).
    pub muted: Colour,
    /// The highlight (`Theme.accent`).
    pub accent: Colour,
}

/// `c` showing `part` of what it shows.
fn faint(c: Colour, part: f64) -> Colour {
    c.showing(part)
}

/// How many beats apart the picture's bar lines are: every bar, or every second, fourth... where they'd be closer
/// than [`TIGHT`] points.
fn bar_lines(frame: Frame) -> f64 {
    let mut bars = BAR;
    while bars * frame.beat_width() < TIGHT && bars < 4096.0 {
        bars *= 2.0;
    }
    bars
}

/// The multiples of `step` beats in sight, from the first at or after the view's start, with how many steps from the
/// curve's start each is. None when they'd be closer than [`TIGHT`] points.
fn marks(frame: Frame, step: f64, length: f64) -> Vec<(u64, f64)> {
    if !(step.is_finite() && step > 0.0) || step * frame.beat_width() < TIGHT {
        return Vec::new();
    }
    let end = (frame.view.start + frame.view.span).min(length);
    let first = (frame.view.start / step - 1e-9).ceil().max(0.0) as u64;
    (first..).map(|i| (i, i as f64 * step)).take_while(|&(_, beat)| beat <= end + 1e-9).take(20_000).collect()
}

/// The still part of the picture, back to front: the value guides, the time grid (bars, beats and `grid`'s squares),
/// the clips' seams, the highlight's band, the fill under the curve and the curve, the highlighted part in the accent.
/// `scale` is pixels to a point: the fill is drawn a pixel column at a time.
pub fn picture(shape: &Shape, highlight: Option<Span>, grid: Grid, frame: Frame, scale: f64, c: &Colours) -> Scene {
    if !frame.fits() {
        // No room (or no size yet): an empty picture of a size a painter can make.
        let size = |v: f64| if v.is_finite() && v >= 1.0 { v } else { 1.0 };
        return Scene::new(size(frame.width), size(frame.height), c.well);
    }
    let length = shape.length();
    // Only the curve's own time is ever in sight, whatever the page asked for.
    let frame = Frame { view: frame.view.within(length), ..frame };
    let mut scene = Scene::new(frame.width, frame.height, c.well);
    let (left, right) = (frame.x(frame.view.start), frame.x((frame.view.start + frame.view.span).min(length)));
    let (top, bottom) = (frame.y(1.0), frame.y(0.0));
    // 1. Value guides.
    for (value, part) in [(0.0, 0.6), (0.25, 0.35), (0.5, 0.7), (0.75, 0.35), (1.0, 0.6)] {
        scene.rect((left, frame.y(value) - 0.5, right - left, 1.0), faint(c.stroke, part));
    }
    // 2. The time grid: each line once, as the strongest thing it is.
    let upright = |scene: &mut Scene, beat: f64, colour: Colour| {
        scene.rect((frame.x(beat) - 0.5, top, 1.0, bottom - top), colour);
    };
    let on = |beat: f64, step: f64| ((beat / step).round() * step - beat).abs() < 1e-6;
    if let Some(step) = grid.beats() {
        for (_, beat) in marks(frame, step, length) {
            if !on(beat, 1.0) {
                upright(&mut scene, beat, faint(c.stroke, 0.4));
            }
        }
    }
    for (_, beat) in marks(frame, 1.0, length) {
        if !on(beat, BAR) {
            upright(&mut scene, beat, faint(c.stroke, 0.85));
        }
    }
    // Bars, wider apart when zoomed far out.
    for (_, beat) in marks(frame, bar_lines(frame), length) {
        upright(&mut scene, beat, c.stroke);
        // A shade brighter than the beats' lines.
        upright(&mut scene, beat, faint(c.text, 0.12));
    }
    // 3. The clips' seams, where one clip ends and the next begins.
    let in_sight = |beat: f64| beat > frame.view.start + 1e-9 && beat < frame.view.start + frame.view.span - 1e-9;
    let mut seams: Vec<f64> = shape.clips().iter().flat_map(|clip| [clip.span.start, clip.span.end]).collect();
    seams.sort_by(f64::total_cmp);
    // Two clips that touch share one seam.
    seams.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    for beat in seams {
        if in_sight(beat) && beat > 1e-9 && beat < length - 1e-9 {
            scene.rect((frame.x(beat) - 0.5, 0.0, 1.0, frame.height), faint(c.muted, 0.45));
        }
    }
    // 4. The highlight's band: its time, the picture's full height.
    let lit = highlight.filter(|h| h.start.is_finite() && h.end.is_finite());
    if let Some(h) = lit {
        let (from, to) = (frame.x(h.start).clamp(left, right), frame.x(h.end).clamp(left, right));
        scene.rect((from, 0.0, to - from, frame.height), faint(c.accent, 0.07));
    }
    // 5. The fill under the curve, a pixel column at a time: each column on whole pixels, so no two overlap or leave
    // a gap, and next-door columns of one height as one shape (a gate's steps are a handful).
    let scale = crate::shapes::usable_scale(scale);
    let (first, last) = ((left * scale).round() as i64, (right * scale).round() as i64);
    let mut values = Columns::new(shape);
    let mut run: Option<(i64, f64)> = None;
    for px in first..=last.min(first + 32_768) {
        let y = if px < last {
            let middle = (px as f64 + 0.5) / scale;
            Some(frame.y(values.at(frame.beat_at(middle))))
        } else {
            None
        };
        match (run, y) {
            (Some((_, was)), Some(y)) if (was - y).abs() < 1e-6 => {}
            _ => {
                if let Some((from, was)) = run {
                    let (x0, x1) = (from as f64 / scale, px as f64 / scale);
                    scene.rect((x0, was, x1 - x0, bottom - was), faint(c.text, 0.07));
                }
                run = y.map(|y| (px, y));
            }
        }
    }
    // 6. The curve: each line cut where it leaves what's in sight and where the highlight starts and ends.
    let points = shape.points();
    let colour_at = |beat: f64| match lit {
        Some(h) if beat > h.start && beat < h.end => c.accent,
        _ => c.text,
    };
    let (from, to) = (frame.view.start, (frame.view.start + frame.view.span).min(length));
    let stretch = |scene: &mut Scene, a: f64, b: f64, value: &dyn Fn(f64) -> f64, bent: bool| {
        let (a, b) = (a.max(from), b.min(to));
        if b - a <= 1e-9 {
            return;
        }
        let mut cuts = vec![a, b];
        if let Some(h) = lit {
            cuts.extend([h.start, h.end].into_iter().filter(|e| *e > a && *e < b));
        }
        cuts.sort_by(f64::total_cmp);
        for pair in cuts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            // A bent line as short straight ones, about three points long each.
            let steps = if bent { ((b - a) * frame.beat_width() / 3.0).ceil().clamp(1.0, 2000.0) as usize } else { 1 };
            let colour = colour_at((a + b) / 2.0);
            for s in 0..steps {
                let (t0, t1) = (a + (b - a) * s as f64 / steps as f64, a + (b - a) * (s + 1) as f64 / steps as f64);
                scene.line((frame.x(t0), frame.y(value(t0))), (frame.x(t1), frame.y(value(t1))), 2.0, colour);
            }
        }
    };
    // Before the first point and after the last one the curve holds their values.
    if let (Some(first), Some(last)) = (points.first(), points.last()) {
        stretch(&mut scene, 0.0, first.beat, &|_| first.value, false);
        stretch(&mut scene, last.beat, length, &|_| last.value, false);
    }
    for (i, pair) in points.windows(2).enumerate() {
        let (a, b) = (pair[0], pair[1]);
        if b.beat - a.beat < 1e-9 {
            // A jump: a straight drop.
            if a.beat >= from - 1e-9 && a.beat <= to + 1e-9 {
                let x = frame.x(a.beat);
                let colour = match lit {
                    Some(h) if a.beat > h.start + 1e-9 && a.beat < h.end - 1e-9 => c.accent,
                    _ => c.text,
                };
                scene.line((x, frame.y(a.value)), (x, frame.y(b.value)), 2.0, colour);
            }
            continue;
        }
        stretch(&mut scene, a.beat, b.beat, &|beat| shape.line_value(i, beat), a.bend.abs() >= 0.01);
    }
    scene
}

/// The curve's value at beats asked for from left to right, without looking through its points from the start each
/// time.
struct Columns<'a> {
    shape: &'a Shape,
    /// The last point at or before the beat last asked for.
    at: usize,
}

impl<'a> Columns<'a> {
    fn new(shape: &'a Shape) -> Columns<'a> {
        Columns { shape, at: 0 }
    }

    fn at(&mut self, beat: f64) -> f64 {
        let points = self.shape.points();
        while points.get(self.at + 1).is_some_and(|p| p.beat <= beat) {
            self.at += 1;
        }
        match points.get(self.at) {
            Some(p) if p.beat <= beat => self.shape.line_value(self.at, beat),
            Some(p) => p.value,
            None => 0.0,
        }
    }
}

/// One of the ruler's marks.
#[derive(Clone, Debug, PartialEq)]
pub struct Tick {
    /// Where, in points from the picture's left (the ruler keeps the picture's room at its sides).
    pub x: f64,
    /// Its place as bar and beat ("1", "1.2", "2"); empty when the marks are too close to number each one.
    pub label: String,
    /// At the start of a bar.
    pub bar: bool,
}

/// The ruler's marks for what's in sight of a curve `length` beats long: one at every beat, each numbered by its bar
/// and beat; zoomed out past [`NUMBERED`] points a beat, only the bars are numbered (every second, fourth... when
/// they're close too), and beats closer than [`TIGHT`] points aren't marked at all.
pub fn ruler(length: f64, frame: Frame) -> Vec<Tick> {
    if !frame.fits() {
        return Vec::new();
    }
    let frame = Frame { view: frame.view.within(length), ..frame };
    let per_bar = BAR as u64;
    let name_beats = frame.beat_width() >= NUMBERED;
    // How many bars apart the numbered ones are, so their numbers have at least 44 points each (as the
    // arrangement's ruler gives its own).
    let mut every = 1u64;
    while every as f64 * BAR * frame.beat_width() < 44.0 && every < 4096 {
        every *= 2;
    }
    let mut ticks: Vec<Tick> = marks(frame, 1.0, length)
        .into_iter()
        .map(|(i, beat)| {
            let (bar, in_bar) = (i / per_bar, i % per_bar);
            let label = match in_bar {
                0 if bar % every == 0 => format!("{}", bar + 1),
                0 => String::new(),
                _ if name_beats => format!("{}.{}", bar + 1, in_bar + 1),
                _ => String::new(),
            };
            Tick { x: frame.x(beat), label, bar: in_bar == 0 }
        })
        .collect();
    if ticks.is_empty() {
        // Beats too close to mark: a mark over each bar line the picture draws.
        let step = bar_lines(frame);
        let bars_apart = (step / BAR) as u64;
        ticks = marks(frame, step, length)
            .into_iter()
            .map(|(i, beat)| {
                let bar = i * bars_apart;
                let label =
                    if bar.is_multiple_of(every.max(bars_apart)) { format!("{}", bar + 1) } else { String::new() };
                Tick { x: frame.x(beat), label, bar: true }
            })
            .collect();
    }
    ticks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve_edit::{Edit, Point, Step, apply};
    use crate::shapes::Shape as Drawn;

    fn frame(length: f64) -> Frame {
        Frame { width: 628.0, height: 236.0, view: View::whole(length) }
    }

    fn colours() -> Colours {
        Colours {
            well: 0x101010,
            stroke: Colour::rgb(0x303030, 1.0),
            text: Colour::rgb(0xEEEEEE, 1.0),
            muted: Colour::rgb(0x888888, 1.0),
            accent: Colour::rgb(0x35D0BA, 1.0),
        }
    }

    fn pt(beat: f64, value: f64) -> Point {
        Point { beat, value, bend: 0.0 }
    }

    fn triangle() -> Shape {
        Shape::new(vec![pt(0.0, 0.0), pt(2.0, 1.0), pt(4.0, 0.0)], 4.0)
    }

    /// A drawn line: its two ends and its colour.
    type Line = ((f32, f32), (f32, f32), Colour);

    fn lines(scene: &Scene) -> Vec<Line> {
        scene
            .shapes
            .iter()
            .filter_map(|s| match *s {
                Drawn::Line { from, to, colour, .. } => Some((from, to, colour)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_beat_and_a_value_have_their_place_and_back() {
        let f = frame(4.0);
        assert_eq!(f.x(0.0), SIDE);
        assert_eq!(f.x(4.0), 628.0 - SIDE);
        assert_eq!(f.y(1.0), TOP);
        assert_eq!(f.y(0.0), 236.0 - TOP);
        assert!((f.beat_at(f.x(1.3)) - 1.3).abs() < 1e-9);
        assert!((f.value_at(f.y(0.62)) - 0.62).abs() < 1e-9);
        assert_eq!(f.value_at(-50.0), 1.0);
        assert_eq!(f.beat_width(), 150.0);
        assert!(!Frame { width: 20.0, ..f }.fits());
        assert!(!Frame { view: View { start: 0.0, span: f64::NAN }, ..f }.fits());
    }

    #[test]
    fn the_view_zooms_round_the_mouse_and_stays_inside_the_curve() {
        let whole = View::whole(8.0);
        let closer = whole.zoomed(2.0, 2.0, 8.0);
        assert_eq!(closer, View { start: 1.0, span: 4.0 });
        // The beat under the mouse stays where it was: a quarter of the way across.
        assert!(((2.0 - closer.start) / closer.span - 0.25).abs() < 1e-9);
        assert_eq!(closer.zoomed(2.0, 0.5, 8.0), whole);
        assert_eq!(whole.zoomed(2.0, 0.5, 8.0), whole, "never wider than the curve");
        assert_eq!(whole.zoomed(8.0, 1000.0, 8.0), View { start: 8.0 - CLOSEST, span: CLOSEST });
        assert_eq!(closer.moved(100.0, 8.0), View { start: 4.0, span: 4.0 });
        assert_eq!(closer.moved(-100.0, 8.0).start, 0.0);
        assert!(whole.is_whole(8.0) && !closer.is_whole(8.0));
        // A curve shorter than the closest zoom is shown whole; a broken view is the whole curve.
        assert_eq!(View::whole(0.1).zoomed(0.05, 4.0, 0.1), View::whole(0.1));
        assert_eq!(View { start: f64::NAN, span: 1.0 }.within(8.0), whole);
        // The curve got shorter under a view of its end.
        assert_eq!(View { start: 6.0, span: 2.0 }.within(4.0), View { start: 2.0, span: 2.0 });
    }

    #[test]
    fn the_curve_is_drawn_through_its_points() {
        let f = frame(4.0);
        let scene = picture(&triangle(), None, Grid::default(), f, 1.0, &colours());
        let drawn = lines(&scene);
        assert_eq!(drawn.len(), 2);
        assert_eq!(drawn[0].0, (f.x(0.0) as f32, f.y(0.0) as f32));
        assert_eq!(drawn[0].1, (f.x(2.0) as f32, f.y(1.0) as f32));
        assert_eq!(drawn[1].1, (f.x(4.0) as f32, f.y(0.0) as f32));
        assert!(drawn.iter().all(|l| l.2 == colours().text));
        // Nothing is drawn outside the picture's room at the sides.
        for s in &scene.shapes {
            if let Drawn::Rect { x, w, .. } = *s {
                assert!(x >= SIDE as f32 - 0.51 && x + w <= (628.0 - SIDE) as f32 + 0.51, "{x} {w}");
            }
        }
    }

    #[test]
    fn the_highlighted_part_is_drawn_in_the_accent_with_its_band() {
        let f = frame(4.0);
        let c = colours();
        let lit = Span::new(1.0, 3.0);
        let scene = picture(&triangle(), Some(lit), Grid::default(), f, 1.0, &c);
        let drawn = lines(&scene);
        // Each line is cut at the highlight's edge: outside in the text colour, inside in the accent.
        assert_eq!(drawn.len(), 4);
        let colours: Vec<Colour> = drawn.iter().map(|l| l.2).collect();
        assert_eq!(colours, [c.text, c.accent, c.accent, c.text]);
        assert_eq!(drawn[0].1.0, f.x(1.0) as f32);
        // The band runs the picture's full height over the highlight's time.
        let band = scene.shapes.iter().any(|s| {
            matches!(*s, Drawn::Rect { x, y, w, h, fill }
                if x == f.x(1.0) as f32 && y == 0.0 && w == (f.x(3.0) - f.x(1.0)) as f32 && h == 236.0
                    && fill == faint(c.accent, 0.07))
        });
        assert!(band);
    }

    #[test]
    fn a_jump_is_a_straight_drop_and_a_bend_many_short_lines() {
        let f = frame(4.0);
        let gate = Shape::new(vec![pt(0.0, 0.0), pt(2.0, 0.0), pt(2.0, 1.0), pt(4.0, 1.0)], 4.0);
        let drawn = lines(&picture(&gate, None, Grid::default(), f, 1.0, &colours()));
        assert_eq!(drawn.len(), 3);
        assert_eq!(drawn[1].0.0, drawn[1].1.0, "the jump goes straight up");
        let bent = Shape::new(vec![Point { beat: 0.0, value: 0.0, bend: 0.8 }, pt(4.0, 1.0)], 4.0);
        let drawn = lines(&picture(&bent, None, Grid::default(), f, 1.0, &colours()));
        assert_eq!(drawn.len(), 200, "600 points across, about 3 each");
        // The short lines follow the curve, end to end.
        for pair in drawn.windows(2) {
            assert_eq!(pair[0].1, pair[1].0);
        }
        let mid = drawn[100].0;
        assert!((f64::from(mid.1) - f.y(bent.value_at(2.0))).abs() < 0.01);
    }

    #[test]
    fn a_curve_holds_its_ends_and_only_what_is_in_sight_is_drawn() {
        // Its points don't reach its ends.
        let s = Shape::new(vec![pt(1.0, 0.2), pt(3.0, 0.8)], 4.0);
        let f = frame(4.0);
        let drawn = lines(&picture(&s, None, Grid::default(), f, 1.0, &colours()));
        assert_eq!(drawn.len(), 3);
        assert_eq!(drawn[0].0, (f.x(0.0) as f32, f.y(0.2) as f32));
        assert_eq!(drawn[1].1, (f.x(4.0) as f32, f.y(0.8) as f32));
        // Zoomed in on beats 1 to 2 of the triangle: one line, from edge to edge.
        let near = Frame { view: View { start: 1.0, span: 1.0 }, ..f };
        let drawn = lines(&picture(&triangle(), None, Grid::default(), near, 1.0, &colours()));
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].0, (SIDE as f32, near.y(0.5) as f32));
        assert_eq!(drawn[0].1, ((628.0 - SIDE) as f32, near.y(1.0) as f32));
    }

    #[test]
    fn the_fill_follows_the_curve_and_the_grid_follows_its_chips() {
        let f = frame(4.0);
        let c = colours();
        let uprights = |grid: Grid| {
            picture(&triangle(), None, grid, f, 1.0, &c)
                .shapes
                .iter()
                .filter(|s| matches!(**s, Drawn::Rect { w, h, .. } if w == 1.0 && h == 200.0))
                .count()
        };
        // Off: the beats (3) and the bars (2, each drawn twice to be brighter).
        let off = uprights(Grid { step: None, triplet: false });
        assert_eq!(off, 3 + 2 * 2);
        // 1/16: three more between each pair of beats.
        assert_eq!(uprights(Grid { step: Some(Step::Sixteenth), triplet: false }), off + 12);
        // Triplet eighths: two more between each pair of beats.
        assert_eq!(uprights(Grid { step: Some(Step::Eighth), triplet: true }), off + 8);
        // The fill: a column a pixel wide under the curve, the tallest at its peak.
        let scene = picture(&triangle(), None, Grid::default(), f, 2.0, &c);
        let fill: Vec<(f32, f32)> = scene
            .shapes
            .iter()
            .filter_map(|s| match *s {
                Drawn::Rect { w, h, fill, .. } if fill == faint(c.text, 0.07) && w == 0.5 => Some((w, h)),
                _ => None,
            })
            .collect();
        // (But for the few at the peak that share a height and are one shape.)
        assert!((1190..=1200).contains(&fill.len()), "{}", fill.len());
        let tallest = fill.iter().map(|f| f.1).fold(0.0, f32::max);
        assert!((tallest - 200.0).abs() < 1.0, "{tallest}");
        // A gate's two levels are two shapes (its low level has no height to fill), whatever the scale; and at a
        // scale that isn't whole, every column still starts and ends on a whole pixel.
        let gate = Shape::new(vec![pt(0.0, 0.5), pt(2.0, 0.5), pt(2.0, 1.0), pt(4.0, 1.0)], 4.0);
        for scale in [1.0, 1.25, 1.75, 2.0] {
            let fills: Vec<(f32, f32)> = picture(&gate, None, Grid::default(), f, scale, &c)
                .shapes
                .iter()
                .filter_map(|s| match *s {
                    Drawn::Rect { x, w, fill, .. } if fill == faint(c.text, 0.07) => Some((x, w)),
                    _ => None,
                })
                .collect();
            assert_eq!(fills.len(), 2, "at {scale}");
            assert!((fills[0].0 + fills[0].1 - fills[1].0).abs() < 1e-3, "at {scale}: they meet");
            for (x, w) in fills {
                let (left, right) = (f64::from(x) * scale, f64::from(x + w) * scale);
                assert!((left - left.round()).abs() < 1e-3 && (right - right.round()).abs() < 1e-3, "at {scale}");
            }
        }
    }

    #[test]
    fn clips_show_their_seams() {
        let f = frame(4.0);
        let (s, _) = apply(&triangle(), Some(Span::new(1.0, 2.0)), &Edit::Remember { name: "part".into() }).unwrap();
        let scene = picture(&s, None, Grid::default(), f, 1.0, &colours());
        let seams: Vec<f32> = scene
            .shapes
            .iter()
            .filter_map(|s| match *s {
                Drawn::Rect { x, h: 236.0, .. } => Some(x + 0.5),
                _ => None,
            })
            .collect();
        assert_eq!(seams, [f.x(1.0) as f32, f.x(2.0) as f32]);
        // Two clips that touch share one seam.
        let (s, _) = apply(&s, Some(Span::new(2.0, 3.0)), &Edit::Remember { name: "next".into() }).unwrap();
        let seams = picture(&s, None, Grid::default(), f, 1.0, &colours())
            .shapes
            .iter()
            .filter(|s| matches!(**s, Drawn::Rect { h: 236.0, .. }))
            .count();
        assert_eq!(seams, 3);
    }

    #[test]
    fn a_view_or_size_that_cant_be_is_drawn_safely() {
        let c = colours();
        // The curve got shorter under a view of its old end, with a highlight on.
        let past = Frame { view: View { start: 6.0, span: 2.0 }, ..frame(4.0) };
        let scene = picture(&triangle(), Some(Span::new(1.0, 2.0)), Grid::default(), past, 1.0, &c);
        assert_eq!(lines(&scene).len(), 1, "what's shown is the curve's last two beats");
        // A view from before the curve's start shows the curve from its start.
        let before = Frame { view: View { start: -1.0, span: 4.0 }, ..frame(4.0) };
        assert_eq!(
            picture(&triangle(), None, Grid::default(), before, 1.0, &c),
            picture(&triangle(), None, Grid::default(), frame(4.0), 1.0, &c)
        );
        // No size yet: an empty picture a painter can still make.
        let none = Frame { width: f64::NAN, ..frame(4.0) };
        let scene = picture(&triangle(), None, Grid::default(), none, 1.0, &c);
        assert!(scene.shapes.is_empty());
        assert_eq!(scene.pixel_size(1.0), (1, 236));
        let broken = Some(Span { start: f64::NAN, end: 2.0 });
        assert_eq!(lines(&picture(&triangle(), broken, Grid::default(), frame(4.0), 1.0, &c)).len(), 2);
    }

    #[test]
    fn the_ruler_numbers_beats_then_only_bars() {
        let labels = |length: f64, width: f64| -> Vec<String> {
            ruler(length, Frame { width, height: 236.0, view: View::whole(length) })
                .into_iter()
                .map(|t| t.label)
                .collect()
        };
        assert_eq!(labels(8.0, 628.0), ["1", "1.2", "1.3", "1.4", "2", "2.2", "2.3", "2.4", "3"]);
        // 16 bars in 628 points: 9.4 points a beat. The beats are marked but only bars numbered.
        let far = labels(64.0, 628.0);
        assert_eq!(far.len(), 65);
        assert_eq!(far[0], "1");
        assert_eq!(far[1], "");
        assert_eq!(far[4], "", "bars 37.5 points apart: every second one is numbered");
        assert_eq!(far[8], "3");
        // 64 bars, 9.4 points each: the beats are too close to mark at all. Each bar line the picture draws has its
        // mark, and every eighth is numbered.
        let farther = labels(256.0, 628.0);
        assert_eq!(farther.len(), 65);
        assert_eq!(farther[0], "1");
        assert!(farther[1..8].iter().all(String::is_empty));
        assert_eq!(farther[8], "9");
        // Zoomed in to the second bar.
        let near = ruler(8.0, Frame { width: 628.0, height: 236.0, view: View { start: 4.0, span: 2.0 } });
        assert_eq!(near.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), ["2", "2.2", "2.3"]);
        assert!(near[0].bar && !near[1].bar);
        assert_eq!(near[0].x, SIDE);
    }
}
