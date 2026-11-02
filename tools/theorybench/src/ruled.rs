//! Fitting numbers that must keep some rules (`chordfit`): each rule a sign (a number at least 0, or at most 0) or an
//! order (one number at most another), as mo-harmony's chord reader's weights keep them ([`mo_harmony::chordread::RULES`]).
//!
//! The method is a projected quasi-Newton one: from where it is, it builds a quadratic picture of the loss from its
//! slope and a curvature it learns as it goes (BFGS, kept whole: the chord reader has 39 numbers), finds that
//! picture's lowest point among the numbers that keep the rules (by projected gradient steps, each projected back with
//! Dykstra's method), and moves towards it as far as the loss falls enough (Armijo). Every point it reads keeps the
//! rules, so every loss it reports is one the reader could ship with.

use mo_harmony::chordread::Bound;

/// The nearest numbers to `x` that keep every bound, with the numbers `fixed` holds kept as they are in `x` (Dykstra's
/// method: each bound in turn, carrying what each took away, until nothing moves).
pub fn project(x: &[f64], bounds: &[Bound], fixed: &[bool]) -> Vec<f64> {
    let n = x.len();
    let is_fixed = |i: usize| fixed.get(i).copied().unwrap_or(false);
    // What each bound took away last time, on the one or two numbers it touches.
    let mut taken: Vec<(f64, f64)> = vec![(0.0, 0.0); bounds.len()];
    let mut y = x.to_vec();
    let at = |y: &[f64], i: usize| y.get(i).copied().unwrap_or(0.0);
    for _ in 0..100_000 {
        let mut moved = 0.0f64;
        for (bound, took) in bounds.iter().zip(taken.iter_mut()) {
            match *bound {
                Bound::AtLeast0(i) | Bound::AtMost0(i) => {
                    if i >= n || is_fixed(i) {
                        continue;
                    }
                    let z = at(&y, i) + took.0;
                    let p = if matches!(bound, Bound::AtLeast0(_)) { z.max(0.0) } else { z.min(0.0) };
                    moved = moved.max((p - at(&y, i)).abs());
                    took.0 = z - p;
                    y[i] = p;
                }
                Bound::AtMost(i, j) => {
                    if i >= n || j >= n {
                        continue;
                    }
                    let (zi, zj) = (at(&y, i) + took.0, at(&y, j) + took.1);
                    let (pi, pj) = if zi <= zj {
                        (zi, zj)
                    } else {
                        match (is_fixed(i), is_fixed(j)) {
                            (false, false) => ((zi + zj) / 2.0, (zi + zj) / 2.0),
                            (true, false) => (zi, zi),
                            (false, true) => (zj, zj),
                            (true, true) => (zi, zj),
                        }
                    };
                    moved = moved.max((pi - at(&y, i)).abs()).max((pj - at(&y, j)).abs());
                    *took = (zi - pi, zj - pj);
                    y[i] = pi;
                    y[j] = pj;
                }
            }
        }
        if moved <= 1e-15 * y.iter().fold(1.0f64, |m, v| m.max(v.abs())) {
            break;
        }
    }
    settle(&mut y, bounds, fixed);
    y
}

/// Moves numbers a rounding error past a bound back onto it, until every bound holds.
fn settle(y: &mut [f64], bounds: &[Bound], fixed: &[bool]) {
    let is_fixed = |i: usize| fixed.get(i).copied().unwrap_or(false);
    for _ in 0..1000 {
        let mut broken = false;
        for bound in bounds {
            match *bound {
                Bound::AtLeast0(i) if !is_fixed(i) && y.get(i).is_some_and(|v| *v < 0.0) => {
                    y[i] = 0.0;
                    broken = true;
                }
                Bound::AtMost0(i) if !is_fixed(i) && y.get(i).is_some_and(|v| *v > 0.0) => {
                    y[i] = 0.0;
                    broken = true;
                }
                Bound::AtMost(i, j) if i < y.len() && j < y.len() && y[i] > y[j] => {
                    if is_fixed(i) {
                        y[j] = y[i];
                    } else {
                        y[i] = y[j];
                    }
                    broken = true;
                }
                _ => {}
            }
        }
        if !broken {
            return;
        }
    }
}

/// Whether some numbers keep every bound, as the reader checks them ([`Bound::holds`]).
#[cfg(test)]
pub fn keeps(x: &[f64], bounds: &[Bound]) -> bool {
    bounds.iter().all(|b| b.holds(x))
}

/// What the loss gives at some numbers: its value and its slope, and a curvature to start from (the first point's is
/// used, when given).
pub struct Reading {
    pub loss: f64,
    pub slope: Vec<f64>,
    pub curvature: Option<Vec<f64>>,
}

/// How a fit is run.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    /// The most steps taken.
    pub steps: usize,
    /// It stops when the largest move the projected slope asks for is under this...
    pub slope: f64,
    /// ...or the loss falls by less than this (relative) three steps running.
    pub fall: f64,
}

impl Default for Plan {
    fn default() -> Plan {
        Plan { steps: 60, slope: 1e-7, fall: 1e-9 }
    }
}

/// Where a fit ended.
#[derive(Clone, Debug)]
pub struct Fitted {
    pub x: Vec<f64>,
    pub loss: f64,
    pub steps: usize,
    /// How many times the loss was read.
    pub readings: usize,
    /// Why it stopped.
    pub why: &'static str,
}

/// Finds the numbers that keep every bound with the least loss, from `x0` (moved onto the rules first), the numbers
/// `fixed` holds kept as they are. `read` gives the loss and its slope at any numbers; `say` hears a line for each step.
pub fn minimise(
    x0: &[f64],
    bounds: &[Bound],
    fixed: &[bool],
    plan: Plan,
    mut read: impl FnMut(&[f64]) -> Reading,
    mut say: impl FnMut(String),
) -> Fitted {
    let n = x0.len();
    let free = |i: usize| !fixed.get(i).copied().unwrap_or(false);
    let mut x = project(x0, bounds, fixed);
    let first = read(&x);
    let mut readings = 1;
    let (mut loss, mut slope) = (first.loss, held_slope(&first.slope, fixed));
    // The curvature: the one given (made symmetric and a little firmer, so it can be inverted), or the identity.
    let mut b = vec![0.0; n * n];
    match first.curvature.filter(|c| c.len() == n * n) {
        Some(c) => {
            let ridge = 1e-6 * (0..n).map(|i| c[i * n + i].abs()).fold(1e-12, f64::max);
            for i in 0..n {
                for j in 0..n {
                    b[i * n + j] = if free(i) && free(j) { (c[i * n + j] + c[j * n + i]) / 2.0 } else { 0.0 };
                }
                b[i * n + i] += ridge;
            }
        }
        None => {
            for i in 0..n {
                b[i * n + i] = 1.0;
            }
        }
    }
    let mut flat = 0;
    for step in 1..=plan.steps {
        let gap = projected_gap(&x, &slope, bounds, fixed);
        if gap < plan.slope {
            return Fitted { x, loss, steps: step - 1, readings, why: "the projected slope is flat" };
        }
        // The quadratic picture's lowest point among the numbers that keep the rules.
        let target = lowest(&x, &slope, &b, bounds, fixed);
        let d: Vec<f64> = target.iter().zip(&x).map(|(t, v)| t - v).collect();
        let fall_rate: f64 = slope.iter().zip(&d).map(|(g, d)| g * d).sum();
        if fall_rate >= 0.0 || d.iter().all(|v| v.abs() < 1e-15) {
            return Fitted { x, loss, steps: step - 1, readings, why: "no step lowers the loss" };
        }
        // As far towards it as the loss falls enough.
        let mut t = 1.0;
        let mut taken = None;
        for _ in 0..30 {
            let next = project(&x.iter().zip(&d).map(|(v, d)| v + t * d).collect::<Vec<_>>(), bounds, fixed);
            let r = read(&next);
            readings += 1;
            if r.loss.is_finite() && r.loss <= loss + 1e-4 * t * fall_rate {
                taken = Some((next, r));
                break;
            }
            t /= 2.0;
        }
        let Some((next, r)) = taken else {
            return Fitted { x, loss, steps: step - 1, readings, why: "no step along the way lowers the loss enough" };
        };
        let next_slope = held_slope(&r.slope, fixed);
        let s: Vec<f64> = next.iter().zip(&x).map(|(a, b)| a - b).collect();
        let y: Vec<f64> = next_slope.iter().zip(&slope).map(|(a, b)| a - b).collect();
        update(&mut b, &s, &y);
        let fell = (loss - r.loss) / loss.abs().max(1e-12);
        say(format!(
            "step {step}: loss {:.6} (fell {:.2e}), moved {:.4} (t {t}), projected slope {:.2e}",
            r.loss,
            loss - r.loss,
            s.iter().map(|v| v * v).sum::<f64>().sqrt(),
            gap
        ));
        (x, loss, slope) = (next, r.loss, next_slope);
        flat = if fell < plan.fall { flat + 1 } else { 0 };
        if flat >= 3 {
            return Fitted { x, loss, steps: step, readings, why: "the loss stopped falling" };
        }
    }
    Fitted { x, loss, steps: plan.steps, readings, why: "the most steps were taken" }
}

/// The slope with the fixed numbers' parts left out.
fn held_slope(slope: &[f64], fixed: &[bool]) -> Vec<f64> {
    slope.iter().enumerate().map(|(i, g)| if fixed.get(i).copied().unwrap_or(false) { 0.0 } else { *g }).collect()
}

/// How far a projected slope step moves the numbers, the largest move: 0 where nothing that keeps the rules lowers the
/// loss (the fit's end).
fn projected_gap(x: &[f64], slope: &[f64], bounds: &[Bound], fixed: &[bool]) -> f64 {
    let moved: Vec<f64> = x.iter().zip(slope).map(|(v, g)| v - g).collect();
    project(&moved, bounds, fixed).iter().zip(x).map(|(p, v)| (p - v).abs()).fold(0.0, f64::max)
}

/// The lowest point of the quadratic picture `slope · d + ½ dᵀ B d` (d from `x`) among the numbers that keep the
/// rules: projected gradient steps, sped up (FISTA), each 1 over the largest curvature B can have.
fn lowest(x: &[f64], slope: &[f64], b: &[f64], bounds: &[Bound], fixed: &[bool]) -> Vec<f64> {
    let n = x.len();
    // Gershgorin: no eigenvalue of B is larger than its largest row of sizes.
    let largest = (0..n).map(|i| (0..n).map(|j| b[i * n + j].abs()).sum::<f64>()).fold(1e-12, f64::max);
    let step = 1.0 / largest;
    let slope_at = |y: &[f64]| -> Vec<f64> {
        (0..n).map(|i| slope[i] + (0..n).map(|j| b[i * n + j] * (y[j] - x[j])).sum::<f64>()).collect()
    };
    let (mut y, mut z) = (x.to_vec(), x.to_vec());
    let mut t = 1.0f64;
    for _ in 0..20_000 {
        let g = slope_at(&z);
        let next = project(&z.iter().zip(&g).map(|(v, g)| v - step * g).collect::<Vec<_>>(), bounds, fixed);
        let moved = next.iter().zip(&y).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
        let t_next = (1.0 + (1.0 + 4.0 * t * t).sqrt()) / 2.0;
        z = next.iter().zip(&y).map(|(a, b)| a + (t - 1.0) / t_next * (a - b)).collect();
        y = next;
        t = t_next;
        if moved < 1e-13 * y.iter().fold(1.0f64, |m, v| m.max(v.abs())) {
            break;
        }
    }
    y
}

/// The BFGS update of the curvature B after a step `s` that changed the slope by `y`, damped (Powell) so B stays
/// positive definite whatever the step found.
fn update(b: &mut [f64], s: &[f64], y: &[f64]) {
    let n = s.len();
    let bs: Vec<f64> = (0..n).map(|i| (0..n).map(|j| b[i * n + j] * s[j]).sum()).collect();
    let sbs: f64 = s.iter().zip(&bs).map(|(a, b)| a * b).sum();
    let sy: f64 = s.iter().zip(y).map(|(a, b)| a * b).sum();
    if sbs <= 1e-300 {
        return;
    }
    let theta = if sy >= 0.2 * sbs { 1.0 } else { 0.8 * sbs / (sbs - sy) };
    let r: Vec<f64> = y.iter().zip(&bs).map(|(y, bs)| theta * y + (1.0 - theta) * bs).collect();
    let sr: f64 = s.iter().zip(&r).map(|(a, b)| a * b).sum();
    if sr <= 1e-300 {
        return;
    }
    for i in 0..n {
        for j in 0..n {
            b[i * n + j] += r[i] * r[j] / sr - bs[i] * bs[j] / sbs;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Bound::*;

    #[test]
    fn the_nearest_numbers_keeping_the_rules_are_found() {
        // 0 ≤ x1 ≤ x0, x2 ≤ 0, x3 ≤ x2.
        let bounds = [AtLeast0(1), AtMost(1, 0), AtMost0(2), AtMost(3, 2)];
        let none = [false; 4];
        let p = project(&[1.0, 2.0, 1.0, 3.0], &bounds, &none);
        // x0 and x1 meet at 1.5; x2 and x3 meet at 2, then x2 is held at 0 and x3 with it.
        let want = [1.5, 1.5, 0.0, 0.0];
        assert!(p.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-12), "{p:?}");
        assert!(keeps(&p, &bounds));
        // Numbers that keep the rules stay where they are.
        assert_eq!(project(&[2.0, 1.0, -1.0, -2.0], &bounds, &none), [2.0, 1.0, -1.0, -2.0]);
        // A fixed number stays; the others meet it.
        let p = project(&[1.0, 2.0, 1.0, 3.0], &bounds, &[true, false, false, false]);
        assert!(p.iter().zip([1.0, 1.0, 0.0, 0.0]).all(|(a, b)| (a - b).abs() < 1e-12), "{p:?}");
    }

    #[test]
    fn the_nearest_point_is_nearer_than_any_other_point_keeping_the_rules() {
        // Random points, and random points keeping the rules: none is nearer than the projection.
        let bounds = [AtLeast0(0), AtMost(1, 0), AtLeast0(1), AtMost(2, 1), AtMost0(3), AtMost(4, 3), AtMost(5, 4)];
        let mut seed = 0x1234_5678_9abc_def0u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 2000) as f64 / 500.0 - 2.0
        };
        for _ in 0..200 {
            let x: Vec<f64> = (0..6).map(|_| next()).collect();
            let p = project(&x, &bounds, &[false; 6]);
            assert!(keeps(&p, &bounds), "{p:?}");
            let far = |q: &[f64]| q.iter().zip(&x).map(|(a, b)| (a - b) * (a - b)).sum::<f64>();
            for _ in 0..50 {
                let q = project(&(0..6).map(|_| next()).collect::<Vec<_>>(), &bounds, &[false; 6]);
                assert!(far(&p) <= far(&q) + 1e-9, "{x:?}: {p:?} against {q:?}");
            }
        }
    }

    #[test]
    fn a_quadratic_bowl_is_fitted_to_its_lowest_point_keeping_the_rules() {
        // Loss Σ c_i (x_i - m_i)², with m breaking the rules: 0 ≤ x1 ≤ x0 and x2 ≤ 0, x3 ≤ x2.
        let (c, m) = ([1.0, 4.0, 0.5, 2.0], [1.0, 3.0, 2.0, -1.0]);
        let bounds = [AtLeast0(1), AtMost(1, 0), AtMost0(2), AtMost(3, 2)];
        let read = |x: &[f64]| Reading {
            loss: x.iter().zip(c).zip(m).map(|((x, c), m)| c * (x - m) * (x - m)).sum(),
            slope: x.iter().zip(c).zip(m).map(|((x, c), m)| 2.0 * c * (x - m)).collect(),
            curvature: None,
        };
        let fitted = minimise(&[0.0; 4], &bounds, &[false; 4], Plan::default(), read, |_| {});
        // x0 and x1 meet where c0 (x - 1) + c1 (x - 3) is 0, at 2.6; x2 is held at 0; x3 is free at -1.
        let want = [2.6, 2.6, 0.0, -1.0];
        assert!(fitted.x.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6), "{fitted:?}");
        // Held fixed, a number stays where it starts.
        let read = |x: &[f64]| Reading {
            loss: x.iter().zip(c).zip(m).map(|((x, c), m)| c * (x - m) * (x - m)).sum(),
            slope: x.iter().zip(c).zip(m).map(|((x, c), m)| 2.0 * c * (x - m)).collect(),
            curvature: None,
        };
        let fitted =
            minimise(&[4.0, 0.0, 0.0, 0.0], &bounds, &[true, false, false, false], Plan::default(), read, |_| {});
        assert!(fitted.x.iter().zip([4.0, 3.0, 0.0, -1.0]).all(|(a, b)| (a - b).abs() < 1e-6), "{fitted:?}");
    }

    #[test]
    fn a_curved_loss_is_fitted_from_a_curvature_given() {
        // ln(1 + e^(x0 - x1)) + ln(1 + e^(x1 - 2)) + x0²/2 ... a smooth loss with a curvature given at the start.
        let read = |x: &[f64]| {
            let (a, b) = (x[0] - x[1], x[1] - 2.0);
            let (sa, sb) = (1.0 / (1.0 + (-a).exp()), 1.0 / (1.0 + (-b).exp()));
            Reading {
                loss: (1.0 + a.exp()).ln() + (1.0 + b.exp()).ln() + x[0] * x[0] / 2.0 + x[1] * x[1] / 2.0,
                slope: vec![sa + x[0], -sa + sb + x[1]],
                curvature: Some(vec![1.25, -0.25, -0.25, 1.5]),
            }
        };
        let fitted = minimise(&[1.0, 1.0], &[], &[false; 2], Plan::default(), read, |_| {});
        let r = read(&fitted.x);
        assert!(r.slope.iter().all(|g| g.abs() < 1e-6), "{fitted:?}");
    }
}
