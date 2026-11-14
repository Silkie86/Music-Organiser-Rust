//! Painting on the curve editor's picture (step 7 of the plan Solomon approved on 14 Nov 2026): Ctrl-drag over
//! empty space paints steps, one a grid square; with Shift too (or the grid off) it draws a line by hand. And the
//! swing mark on the ruler: every second grid line dragged later or earlier. No screen here: a stroke gathers where
//! the mouse has been and gives the one edit it comes to, so a whole stroke is one step of undo.

use std::collections::BTreeMap;

use crate::curve_edit::{Edit, Grid, MOST_SWING};
use crate::curve_picture::Frame;
use crate::curve_touch::{Keys, percent};

/// How far the mouse may stray from a straight run before a drawn line keeps a point there, in points.
const DRAWN_CLOSE: f64 = 1.5;

/// A stroke in hand: steps on the grid (which square, and its value), or a line drawn freely.
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    /// The grid's step in beats; none when it's a line by hand.
    step: Option<f64>,
    cells: BTreeMap<i64, f64>,
    /// The line by hand, in the order of time: where the mouse was, in points on the picture.
    line: Vec<(f64, f64)>,
    last: Option<(f64, f64)>,
}

impl Stroke {
    /// A stroke begun with these keys on this grid: steps, or a line by hand with Shift held or the grid off.
    pub fn begin(grid: Grid, keys: Keys) -> Stroke {
        let step = if keys.shift { None } else { grid.beats() };
        Stroke { step, cells: BTreeMap::new(), line: Vec::new(), last: None }
    }

    /// Whether it draws by hand.
    pub fn by_hand(&self) -> bool {
        self.step.is_none()
    }

    /// The mouse is at `x`, `y` on the picture.
    pub fn add(&mut self, frame: Frame, x: f64, y: f64) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        match self.step {
            Some(step) => {
                let cell = |x: f64| (frame.beat_at(x) / step + 1e-9).floor() as i64;
                let value = |y: f64| frame.value_at(y).clamp(0.0, 1.0);
                let (now, v) = (cell(x), value(y));
                // The squares the mouse skipped on its way here get the values on the way.
                match self.last {
                    Some((lx, ly)) if cell(lx) != now => {
                        let (from, from_v) = (cell(lx), value(ly));
                        let n = (now - from).abs();
                        for i in 1..=n.min(4096) {
                            let k = from + (now - from).signum() * i;
                            self.cells.insert(k, from_v + (v - from_v) * i as f64 / n as f64);
                        }
                    }
                    _ => {
                        self.cells.insert(now, v);
                    }
                }
            }
            None => {
                // Going back over time it has drawn, the new way takes the old one's place.
                if let Some((lx, _)) = self.last {
                    let (low, high) = (lx.min(x), lx.max(x));
                    self.line.retain(|p| p.0 < low || p.0 > high || p.0 == lx);
                }
                self.line.retain(|p| p.0 != x);
                let at = self.line.partition_point(|p| p.0 < x);
                self.line.insert(at, (x, y));
            }
        }
        self.last = Some((x, y));
    }

    /// The edit the stroke comes to so far; none before there's anything of it.
    pub fn edit(&self, frame: Frame) -> Option<Edit> {
        match self.step {
            Some(step) => {
                let cells: Vec<(f64, f64, f64)> =
                    self.cells.iter().map(|(k, v)| (*k as f64 * step, (*k + 1) as f64 * step, *v)).collect();
                (!cells.is_empty()).then_some(Edit::Paint { cells })
            }
            None => {
                let kept = thinned(&self.line, DRAWN_CLOSE);
                let line: Vec<(f64, f64)> =
                    kept.iter().map(|&(x, y)| (frame.beat_at(x), frame.value_at(y).clamp(0.0, 1.0))).collect();
                (line.len() >= 2).then_some(Edit::Draw { line })
            }
        }
    }

    /// The read-out beside the mouse while it paints.
    pub fn words(&self, frame: Frame) -> String {
        let Some((_, y)) = self.last else { return String::new() };
        let value = percent(frame.value_at(y).clamp(0.0, 1.0));
        match self.step {
            Some(_) if self.cells.len() == 1 => format!("1 step · {value}"),
            Some(_) => format!("{} steps · {value}", self.cells.len()),
            None => format!("drawing · {value}"),
        }
    }
}

/// The points of a line that say its shape, in order: those further than `close` from the straight run between
/// their neighbours kept (Ramer, Douglas and Peucker's way), the ends always.
fn thinned(line: &[(f64, f64)], close: f64) -> Vec<(f64, f64)> {
    if line.len() < 3 {
        return line.to_vec();
    }
    let mut keep = vec![false; line.len()];
    keep[0] = true;
    keep[line.len() - 1] = true;
    let mut runs = vec![(0usize, line.len() - 1)];
    while let Some((a, b)) = runs.pop() {
        let ((ax, ay), (bx, by)) = (line[a], line[b]);
        let run = (bx - ax).hypot(by - ay).max(1e-9);
        let far = (a + 1..b)
            .map(|i| (i, ((line[i].0 - ax) * (by - ay) - (line[i].1 - ay) * (bx - ax)).abs() / run))
            .max_by(|l, r| l.1.total_cmp(&r.1));
        if let Some((i, d)) = far
            && d > close
        {
            keep[i] = true;
            runs.push((a, i));
            runs.push((i, b));
        }
    }
    line.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

/// What the hint line says with the mouse over empty space and the paint key held.
pub fn hint(grid: Grid, keys: Keys, mac: bool) -> (&'static str, &'static str) {
    let by_hand = keys.shift || grid.beats().is_none();
    let does = if by_hand { "drag draws a line by hand" } else { "drag paints steps, one a grid square" };
    let more = match (by_hand, grid.beats().is_some(), mac) {
        (false, _, _) => "with Shift: by hand",
        (true, true, _) => "let go of Shift: steps on the grid",
        (true, false, _) => "turn the grid on to paint steps",
    };
    (does, more)
}

/// A swing mark dragged from `from_x` to `x`: how far every second line of a grid `step` beats apart moves, of a
/// step (later above 0), never past [`MOST_SWING`]. With Shift it moves freely; else it catches on no swing and on
/// triplet swing (a third) when it's close.
pub fn swing(frame: Frame, step: f64, keys: Keys, from_x: f64, x: f64) -> f64 {
    if !step.is_finite() || step <= 0.0 || !x.is_finite() || !from_x.is_finite() {
        return 0.0;
    }
    let amount = ((frame.beat_at(x) - frame.beat_at(from_x)) / step).clamp(-MOST_SWING, MOST_SWING);
    if keys.shift {
        return amount;
    }
    const CATCH: f64 = 0.04;
    [0.0, 1.0 / 3.0, -1.0 / 3.0, 0.5, -0.5].into_iter().find(|c| (amount - c).abs() < CATCH).unwrap_or(amount)
}

/// Swing in words, as drum machines count it: 50% is straight, 67% triplet swing.
pub fn swing_words(amount: f64) -> String {
    let percent = (50.0 * (1.0 + amount)).round();
    if amount.abs() < 1e-9 {
        "no swing".to_owned()
    } else if (amount.abs() - 1.0 / 3.0).abs() < 1e-9 {
        format!("swing {percent:.0}% · triplet{}", if amount < 0.0 { ", early" } else { "" })
    } else {
        format!("swing {percent:.0}%{}", if amount < 0.0 { " · early" } else { "" })
    }
}

/// Where the swing marks stand, in beats: on every second line of a grid `step` beats apart, over `span` (the
/// highlight, or the whole curve), counted from its start. None when there are too many to tell apart.
pub fn swing_marks(step: f64, start: f64, end: f64, frame: Frame) -> Vec<f64> {
    if !step.is_finite() || step <= 0.0 || frame.beat_width() * step < 6.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut at = start + step;
    while at < end - 1e-9 && out.len() < 512 {
        out.push(at);
        at += 2.0 * step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve_edit::{Shape, Step, apply};
    use crate::curve_picture::View;
    use mo_formats::bitwig::curve::CurvePoint;

    fn frame() -> Frame {
        Frame { width: 428.0, height: 236.0, view: View::whole(4.0) }
    }

    fn grid(step: Step) -> Grid {
        Grid { step: Some(step), triplet: false, dotted: false }
    }

    fn ctrl() -> Keys {
        Keys { ctrl: true, ..Keys::default() }
    }

    #[test]
    fn a_stroke_paints_a_step_in_each_square_it_crosses() {
        let f = frame();
        let mut s = Stroke::begin(grid(Step::Quarter), ctrl());
        assert!(s.edit(f).is_none());
        s.add(f, f.x(0.5), f.y(0.2));
        // Straight to the fourth beat: the two squares between are filled on the way.
        s.add(f, f.x(3.5), f.y(0.8));
        let Some(Edit::Paint { cells }) = s.edit(f) else { panic!("steps") };
        assert_eq!(cells.len(), 4);
        for (i, (start, end, value)) in cells.iter().enumerate() {
            assert!((start - i as f64).abs() < 1e-9 && (end - start - 1.0).abs() < 1e-9);
            assert!((value - (0.2 + 0.2 * i as f64)).abs() < 0.01, "{value}");
        }
        assert_eq!(s.words(f), "4 steps · 80%");
        // Back over a square: its new value.
        s.add(f, f.x(2.5), f.y(0.1));
        let Some(Edit::Paint { cells }) = s.edit(f) else { panic!("steps") };
        assert!((cells[2].2 - 0.1).abs() < 0.01);
        // The whole stroke is one edit the rules take.
        let flat = [CurvePoint { x: 0.0, y: 0.5, bend: 0.0 }, CurvePoint { x: 1.0, y: 0.5, bend: 0.0 }];
        let shape = Shape::from_curve(&flat, 4.0);
        let (after, _) = apply(&shape, None, &Edit::Paint { cells }).expect("painted");
        assert!((after.value_at(0.5) - 0.2).abs() < 0.01 && (after.value_at(2.5) - 0.1).abs() < 0.01);
    }

    #[test]
    fn a_line_by_hand_keeps_only_the_points_that_say_its_shape() {
        let f = frame();
        let mut s = Stroke::begin(grid(Step::Quarter), Keys { ctrl: true, shift: true, alt: false });
        assert!(s.by_hand());
        // Up in a straight run, then down: three points say it all.
        for i in 0..=20 {
            s.add(f, f.x(1.0) + f64::from(i) * 4.0, f.y(0.2) - f64::from(i) * 3.0);
        }
        for i in 1..=20 {
            s.add(f, f.x(1.0) + 80.0 + f64::from(i) * 4.0, f.y(0.2) - 60.0 + f64::from(i) * 3.0);
        }
        let Some(Edit::Draw { line }) = s.edit(f) else { panic!("a line") };
        assert_eq!(line.len(), 3, "{line:?}");
        assert!(line.windows(2).all(|w| w[0].0 < w[1].0));
        // Back over what it drew: the new way takes its place, and time still runs one way.
        s.add(f, f.x(1.0) + 40.0, f.y(0.9));
        let Some(Edit::Draw { line }) = s.edit(f) else { panic!("a line") };
        assert!(line.windows(2).all(|w| w[0].0 < w[1].0), "{line:?}");
        assert!(line.iter().any(|p| (p.1 - 0.9).abs() < 0.01));
        // With the grid off, Ctrl alone draws by hand.
        assert!(Stroke::begin(Grid { step: None, triplet: false, dotted: false }, ctrl()).by_hand());
    }

    #[test]
    fn the_swing_mark_catches_on_triplet_swing_and_stops_at_three_quarters() {
        let f = frame();
        let x = |beat: f64| f.x(beat);
        let none = Keys::default();
        assert!((swing(f, 1.0, none, x(1.0), x(1.34)) - 1.0 / 3.0).abs() < 1e-9);
        assert!((swing(f, 1.0, Keys { shift: true, ..none }, x(1.0), x(1.34)) - 0.34).abs() < 1e-6);
        assert!((swing(f, 1.0, none, x(1.0), x(1.2)) - 0.2).abs() < 1e-6);
        assert!((swing(f, 1.0, none, x(1.0), x(3.0)) - MOST_SWING).abs() < 1e-9);
        assert!((swing(f, 1.0, none, x(1.0), x(0.5)) + 0.5).abs() < 1e-9);
        assert_eq!(swing(f, 0.0, none, x(1.0), x(2.0)), 0.0);
        assert_eq!(swing_words(0.0), "no swing");
        assert_eq!(swing_words(1.0 / 3.0), "swing 67% · triplet");
        assert_eq!(swing_words(0.2), "swing 60%");
        assert_eq!(swing_words(-0.2), "swing 40% · early");
        assert_eq!(swing_marks(1.0, 0.0, 4.0, f), [1.0, 3.0]);
        assert!(swing_marks(1.0 / 64.0, 0.0, 4.0, f).is_empty(), "too close to tell apart");
    }

    #[test]
    fn the_hint_says_what_the_paint_key_does() {
        let g = grid(Step::Sixteenth);
        assert_eq!(hint(g, ctrl(), false).0, "drag paints steps, one a grid square");
        assert_eq!(hint(g, Keys { ctrl: true, shift: true, alt: false }, false).0, "drag draws a line by hand");
        assert_eq!(
            hint(Grid { step: None, triplet: false, dotted: false }, ctrl(), false).1,
            "turn the grid on to paint steps"
        );
    }
}
