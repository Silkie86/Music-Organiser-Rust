//! Fitting the weights of a log-linear choice. Each case offers the same number of candidates (the 24 keys), every
//! candidate has the same named features, and the model picks candidate k with the chance softmax of `w · x(k)`. This
//! is the shape the research recommends for keys: one small set of weights shared by every key, so transposing the
//! notes transposes the answer exactly, and each weight says how much one named piece of evidence counts.
//!
//! The weights minimise the cases' weighted average negative log-likelihood plus an L2 penalty on each weight
//! (`l2[i] / 2 · w[i]²`, so some weights can be held nearer 0 than others). That is convex, so Newton's method finds
//! its one best answer in a few steps, and the sums are taken in a fixed order, so every run gives the same weights to
//! the last bit, however many threads it has.

use rayon::prelude::*;

/// One labelled case, as the fitter reads it: its candidates' features, the right candidate and how much it counts.
/// A case can keep its features in any form (the key cases keep twelve-note histograms and turn them for each key).
pub trait Choice: Sync {
    /// How many candidates it offers.
    fn candidates(&self) -> usize;
    /// The right candidate.
    fn truth(&self) -> usize;
    /// How much the case counts, such as 1 over the number of clips from its song, so that each song counts the same.
    fn weight(&self) -> f64;
    /// Calls `each(feature, value)` for every feature of candidate `k` that isn't 0.
    fn features(&self, k: usize, each: &mut dyn FnMut(usize, f64));
    /// A part of candidate `k`'s score that isn't fitted: what numbers held as they are give it (0 unless a case says
    /// otherwise; `keyfit --rescale` holds some).
    fn offset(&self, _k: usize) -> f64 {
        0.0
    }
}

/// The fitted weights.
#[derive(Debug)]
pub struct Fitted {
    pub weights: Vec<f64>,
    /// The penalised loss at those weights.
    pub loss: f64,
    /// Newton steps taken.
    pub steps: usize,
}

/// Each candidate's score `w · x(k)`, plus its offset.
pub fn choice_scores<C: Choice + ?Sized>(case: &C, w: &[f64]) -> Vec<f64> {
    (0..case.candidates())
        .map(|k| {
            let mut s = case.offset(k);
            case.features(k, &mut |i, v| s += v * w.get(i).copied().unwrap_or(0.0));
            s
        })
        .collect()
}

/// Softmax of scores, computed without overflow. (The key finder's own, `keyfind::chances`, adds its terms in order
/// of size so that scores moved round the keys give the same chances moved round; the fit doesn't need that, and this
/// is quicker.)
pub fn softmax(scores: &[f64]) -> Vec<f64> {
    let top = scores.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let e: Vec<f64> = scores.iter().map(|s| (s - top).exp()).collect();
    let sum: f64 = e.iter().sum();
    e.into_iter().map(|v| v / sum).collect()
}

/// `-ln` of the chance given to the right candidate.
pub fn surprise(scores: &[f64], truth: usize) -> f64 {
    let top = scores.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let log_sum = top + scores.iter().map(|s| (s - top).exp()).sum::<f64>().ln();
    log_sum - scores.get(truth).copied().unwrap_or(f64::NEG_INFINITY)
}

/// The Brier score of one answer: the squared distance between the chances and the truth (0 is sure and right, 2 is
/// sure and wrong).
pub fn brier(chances: &[f64], truth: usize) -> f64 {
    chances.iter().enumerate().map(|(k, p)| if k == truth { (1.0 - p).powi(2) } else { p * p }).sum()
}

/// Cases are summed in blocks of this many, in parallel, and the blocks added up in order.
const BLOCK: usize = 512;

/// The penalty: `l2[i] / 2 · w[i]²` summed.
fn penalty(w: &[f64], l2: &[f64]) -> f64 {
    w.iter().zip(l2).map(|(v, l)| l * v * v).sum::<f64>() / 2.0
}

/// The loss, its gradient and its Hessian (row by row) at `w`, all including the penalty.
fn loss_and_slopes<C: Choice>(cases: &[C], features: usize, w: &[f64], l2: &[f64]) -> (f64, Vec<f64>, Vec<f64>) {
    let f = features;
    let mut weight = 0.0;
    let mut loss = 0.0;
    let mut grad = vec![0.0; f];
    let mut hess = vec![0.0; f * f];
    // A few blocks at a time, so only that many Hessians are held at once; the blocks are still added in order, so
    // the sums are the same to the last bit however many threads there are.
    for batch in cases.chunks(BLOCK * BLOCKS_AT_ONCE) {
        let blocks: Vec<(f64, f64, Vec<f64>, Vec<f64>)> =
            batch.par_chunks(BLOCK).map(|b| block_slopes(b, f, w)).collect();
        for (bw, bl, bg, bh) in blocks {
            weight += bw;
            loss += bl;
            grad.iter_mut().zip(&bg).for_each(|(g, v)| *g += v);
            hess.iter_mut().zip(&bh).for_each(|(h, v)| *h += v);
        }
    }
    let weight = if weight > 0.0 { weight } else { 1.0 };
    let loss = loss / weight + penalty(w, l2);
    for ((g, v), l) in grad.iter_mut().zip(w).zip(l2) {
        *g = *g / weight + l * v;
    }
    for a in 0..f {
        for b in a..f {
            let v = hess[a * f + b] / weight + if a == b { l2.get(a).copied().unwrap_or(0.0) } else { 0.0 };
            hess[a * f + b] = v;
            hess[b * f + a] = v;
        }
    }
    (loss, grad, hess)
}

/// Blocks summed at once by [`loss_and_slopes`].
const BLOCKS_AT_ONCE: usize = 32;

/// One block's weight, weighted loss, gradient and Hessian (upper triangle), before the penalty.
fn block_slopes<C: Choice>(block: &[C], f: usize, w: &[f64]) -> (f64, f64, Vec<f64>, Vec<f64>) {
    let (mut weight, mut loss) = (0.0, 0.0);
    let mut grad = vec![0.0; f];
    let mut hess = vec![0.0; f * f];
    let mut mean = vec![0.0; f];
    let mut row: Vec<(usize, f64)> = Vec::new();
    for case in block {
        let cw = case.weight();
        let s = choice_scores(case, w);
        let p = softmax(&s);
        weight += cw;
        loss += cw * surprise(&s, case.truth());
        mean.iter_mut().for_each(|m| *m = 0.0);
        for (k, pk) in p.iter().enumerate() {
            row.clear();
            case.features(k, &mut |i, v| {
                if i < f {
                    row.push((i, v));
                }
            });
            for &(i, v) in &row {
                mean[i] += pk * v;
            }
            // E[x xᵀ], upper triangle; the lower half is mirrored at the end.
            for &(a, va) in &row {
                let va = pk * va * cw;
                for &(b, vb) in &row {
                    if b >= a {
                        hess[a * f + b] += va * vb;
                    }
                }
            }
        }
        case.features(case.truth(), &mut |i, v| {
            if i < f {
                grad[i] -= cw * v;
            }
        });
        let used: Vec<usize> = (0..f).filter(|&i| mean[i] != 0.0).collect();
        for &a in &used {
            grad[a] += cw * mean[a];
            let ma = mean[a] * cw;
            for &b in &used {
                if b >= a {
                    hess[a * f + b] -= ma * mean[b];
                }
            }
        }
    }
    (weight, loss, grad, hess)
}

/// The penalised loss alone.
fn loss_at<C: Choice>(cases: &[C], w: &[f64], l2: &[f64]) -> f64 {
    let parts: Vec<(f64, f64)> = cases
        .par_chunks(BLOCK)
        .map(|block| {
            block.iter().fold((0.0, 0.0), |(weight, loss), case| {
                (weight + case.weight(), loss + case.weight() * surprise(&choice_scores(case, w), case.truth()))
            })
        })
        .collect();
    let (weight, loss) = parts.iter().fold((0.0, 0.0), |(w0, l0), (w1, l1)| (w0 + w1, l0 + l1));
    let weight = if weight > 0.0 { weight } else { 1.0 };
    loss / weight + penalty(w, l2)
}

/// Solves `a x = b` for a symmetric positive definite `a` (row by row) by Cholesky. `None` when `a` isn't positive
/// definite.
fn solve(a: &[f64], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    let mut l = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let sum: f64 = (0..j).map(|k| l[i * n + k] * l[j * n + k]).sum();
            if i == j {
                let d = a[i * n + i] - sum;
                if d <= 0.0 || !d.is_finite() {
                    return None;
                }
                l[i * n + i] = d.sqrt();
            } else {
                l[i * n + j] = (a[i * n + j] - sum) / l[j * n + j];
            }
        }
    }
    let mut y = vec![0.0; n];
    for i in 0..n {
        y[i] = (b[i] - (0..i).map(|k| l[i * n + k] * y[k]).sum::<f64>()) / l[i * n + i];
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        x[i] = (y[i] - (i + 1..n).map(|k| l[k * n + i] * x[k]).sum::<f64>()) / l[i * n + i];
    }
    Some(x)
}

/// Fits the weights, starting from all zeros, with an L2 penalty for each weight (`l2` as long as the weights, every
/// one above 0: it keeps the answer unique and the steps safe).
pub fn fit<C: Choice>(cases: &[C], features: usize, l2: &[f64]) -> Result<Fitted, String> {
    if features == 0 || l2.len() != features || l2.iter().any(|l| l.is_nan() || *l <= 0.0) {
        return Err("fitting needs at least one feature and an L2 penalty above 0 for each".into());
    }
    if let Some(bad) = cases.iter().position(|c| c.truth() >= c.candidates()) {
        return Err(format!("case {bad}'s right answer isn't one of its candidates"));
    }
    let mut w = vec![0.0; features];
    let mut steps = 0;
    loop {
        let (loss, grad, hess) = loss_and_slopes(cases, features, &w, l2);
        let largest = grad.iter().fold(0.0f64, |m, g| m.max(g.abs()));
        if largest < 1e-10 {
            return Ok(Fitted { weights: w, loss, steps });
        }
        if steps >= MOST_STEPS {
            return Err(format!("the fit didn't settle in {MOST_STEPS} Newton steps (largest slope {largest:e})"));
        }
        let minus: Vec<f64> = grad.iter().map(|g| -g).collect();
        let step = solve(&hess, &minus).ok_or("the Hessian isn't positive definite")?;
        let slope: f64 = grad.iter().zip(&step).map(|(g, d)| g * d).sum();
        // Backtracking: halve the step until the loss drops enough. A step that never does is never taken.
        let mut t = 1.0;
        let next = loop {
            let tried: Vec<f64> = w.iter().zip(&step).map(|(a, d)| a + t * d).collect();
            let tried_loss = loss_at(cases, &tried, l2);
            if tried_loss <= loss + 1e-4 * t * slope {
                break Some((tried, tried_loss));
            }
            if t < 1e-10 {
                break None;
            }
            t /= 2.0;
        };
        let Some((tried, tried_loss)) = next else {
            // Nothing lowers the loss any more. That's the best when the slopes are as small as rounding leaves them;
            // otherwise the fit is stuck, and its numbers mustn't be used.
            return if largest < 1e-6 {
                Ok(Fitted { weights: w, loss, steps })
            } else {
                Err(format!("the fit got stuck after {steps} Newton steps (largest slope {largest:e})"))
            };
        };
        steps += 1;
        let gain = loss - tried_loss;
        w = tried;
        if gain <= 1e-15 * loss.abs().max(1.0) {
            let (loss, _, _) = loss_and_slopes(cases, features, &w, l2);
            return Ok(Fitted { weights: w, loss, steps });
        }
    }
}

/// Newton steps a fit may take: a well-posed one settles in about ten.
const MOST_STEPS: usize = 100;

/// One case held out of a fit, for calibrating: its candidates' scores, the right one, how much it counts, what each
/// of the calibration's numbers is multiplied by for it (the key finder's `Calibration::terms`), and the sharpness the
/// numbers held as they are give it (0 when every number is fitted).
pub struct Scored {
    pub scores: Vec<f64>,
    pub truth: usize,
    pub weight: f64,
    pub terms: Vec<f64>,
    pub fixed: f64,
}

impl Choice for Scored {
    fn candidates(&self) -> usize {
        self.scores.len()
    }

    fn truth(&self) -> usize {
        self.truth
    }

    fn weight(&self) -> f64 {
        self.weight
    }

    /// Candidate k's features are its score times each term, so the fitted weights are the calibration's numbers
    /// and `softmax(score · sharpness)` is the model.
    fn features(&self, k: usize, each: &mut dyn FnMut(usize, f64)) {
        let s = self.scores.get(k).copied().unwrap_or(0.0);
        for (i, t) in self.terms.iter().enumerate() {
            if *t != 0.0 && s != 0.0 {
                each(i, s * t);
            }
        }
    }

    fn offset(&self, k: usize) -> f64 {
        self.fixed * self.scores.get(k).copied().unwrap_or(0.0)
    }
}

/// The calibration that makes held-out scores' chances honest: the numbers `c` minimising the weighted negative
/// log-likelihood of `softmax(scores · (c · terms))`. The loss is convex in them (the scores' log-sum-exp is convex in
/// the sharpness, and the sharpness is linear in the numbers), so the fitter finds its minimum; a whisker of penalty
/// keeps numbers no case uses at 0. Plain temperature scaling is the case with one term, always 1.
pub fn calibration(cases: &[Scored], terms: usize) -> Result<Vec<f64>, String> {
    Ok(fit(cases, terms, &vec![1e-6; terms])?.weights)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Rng;

    /// A case with every feature written out: one row of the problem's feature count per candidate.
    struct Case {
        x: Vec<f64>,
        truth: usize,
        weight: f64,
        features: usize,
    }

    impl Choice for Case {
        fn candidates(&self) -> usize {
            self.x.len().checked_div(self.features).unwrap_or(0)
        }

        fn truth(&self) -> usize {
            self.truth
        }

        fn weight(&self) -> f64 {
            self.weight
        }

        fn features(&self, k: usize, each: &mut dyn FnMut(usize, f64)) {
            if let Some(row) = self.x.chunks_exact(self.features.max(1)).nth(k) {
                for (i, v) in row.iter().enumerate() {
                    if *v != 0.0 {
                        each(i, *v);
                    }
                }
            }
        }
    }

    /// Each candidate's score `w · x(k)`.
    fn scores(x: &[f64], features: usize, w: &[f64]) -> Vec<f64> {
        x.chunks_exact(features).map(|row| row.iter().zip(w).map(|(a, b)| a * b).sum()).collect()
    }

    fn uniform(rng: &mut Rng) -> f64 {
        (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Cases drawn from a known model: 6 candidates, 3 features each.
    fn drawn(w: &[f64], n: usize, seed: u64) -> Vec<Case> {
        let mut rng = Rng::new(seed);
        (0..n)
            .map(|_| {
                let x: Vec<f64> = (0..6 * w.len()).map(|_| uniform(&mut rng) * 2.0 - 1.0).collect();
                let p = softmax(&scores(&x, w.len(), w));
                let u = uniform(&mut rng);
                let mut acc = 0.0;
                let truth = p.iter().position(|pk| {
                    acc += pk;
                    u < acc
                });
                Case { x, truth: truth.unwrap_or(5), weight: 1.0, features: w.len() }
            })
            .collect()
    }

    #[test]
    fn known_weights_are_found_again() {
        let truth = [1.5, -0.7, 0.3];
        let cases = drawn(&truth, 20_000, 7);
        let fitted = fit(&cases, 3, &[1e-6; 3]).unwrap();
        for (got, want) in fitted.weights.iter().zip(truth) {
            assert!((got - want).abs() < 0.1, "{:?}", fitted.weights);
        }
        assert!(fitted.steps < 20, "{} steps", fitted.steps);
        // The same cases give the same weights to the last bit.
        assert_eq!(fit(&cases, 3, &[1e-6; 3]).unwrap().weights, fitted.weights);
        // A strong penalty pulls the weights towards 0, and a penalty on one weight pulls only that one.
        let shrunk = fit(&cases, 3, &[10.0; 3]).unwrap();
        assert!(shrunk.weights.iter().zip(&fitted.weights).all(|(s, f)| s.abs() < f.abs()));
        let one = fit(&cases, 3, &[1e-6, 1e3, 1e-6]).unwrap();
        assert!(one.weights[1].abs() < 0.01 && (one.weights[0] - fitted.weights[0]).abs() < 0.2, "{:?}", one.weights);
    }

    #[test]
    fn a_case_whose_answer_isnt_a_candidate_is_refused() {
        let past_the_end = [Case { x: vec![1.0, 2.0], truth: 2, weight: 1.0, features: 1 }];
        assert!(fit(&past_the_end, 1, &[1.0]).is_err());
        assert!(fit::<Case>(&[], 1, &[0.0]).is_err());
        assert!(fit::<Case>(&[], 1, &[1.0, 1.0]).is_err());
        assert!(fit::<Case>(&[], 0, &[]).is_err());
    }

    #[test]
    fn overconfident_scores_are_cooled() {
        // Scores three times too sure: the right sharpness is about a third.
        let w = [1.0, -1.0, 0.5];
        let cases: Vec<Scored> = drawn(&w, 20_000, 11)
            .into_iter()
            .map(|c| Scored {
                scores: scores(&c.x, 3, &w).iter().map(|s| s * 3.0).collect(),
                truth: c.truth,
                weight: 1.0,
                terms: vec![1.0],
                fixed: 0.0,
            })
            .collect();
        let c = calibration(&cases, 1).unwrap();
        assert!((c[0] - 1.0 / 3.0).abs() < 0.03, "{c:?}");
    }

    #[test]
    fn sharpness_can_differ_by_kind() {
        // Small cases three times too sure, big ones right: each kind gets its own sharpness.
        let w = [1.0, -1.0, 0.5];
        let mut cases = Vec::new();
        for (i, c) in drawn(&w, 40_000, 13).into_iter().enumerate() {
            let small = i % 2 == 0;
            let too_sure = if small { 3.0 } else { 1.0 };
            let scores = scores(&c.x, 3, &w).iter().map(|s| s * too_sure).collect();
            let terms = if small { vec![1.0, 0.0] } else { vec![0.0, 1.0] };
            cases.push(Scored { scores, truth: c.truth, weight: 1.0, terms, fixed: 0.0 });
        }
        let c = calibration(&cases, 2).unwrap();
        assert!((c[0] - 1.0 / 3.0).abs() < 0.05 && (c[1] - 1.0).abs() < 0.1, "{c:?}");
    }

    #[test]
    fn numbers_held_as_they_are_still_count() {
        // The scores three times too sure, with a sharpness of 0.2 held: the number fitted adds what's missing.
        let w = [1.0, -1.0, 0.5];
        let cases: Vec<Scored> = drawn(&w, 20_000, 17)
            .into_iter()
            .map(|c| Scored {
                scores: scores(&c.x, 3, &w).iter().map(|s| s * 3.0).collect(),
                truth: c.truth,
                weight: 1.0,
                terms: vec![1.0],
                fixed: 0.2,
            })
            .collect();
        let c = calibration(&cases, 1).unwrap();
        assert!((c[0] + 0.2 - 1.0 / 3.0).abs() < 0.03, "{c:?}");
    }

    #[test]
    fn scores_and_surprise() {
        let p = softmax(&[0.0, 0.0]);
        assert_eq!(p, vec![0.5, 0.5]);
        assert!((surprise(&[0.0, 0.0], 1) - std::f64::consts::LN_2).abs() < 1e-12);
        assert_eq!(brier(&[1.0, 0.0], 0), 0.0);
        assert_eq!(brier(&[0.0, 1.0], 0), 2.0);
        // Huge scores don't overflow.
        assert!(softmax(&[1000.0, 0.0])[0] > 0.999);
    }
}
