//! Plain statistics for the bench: shares, ranges of doubt, and whether one engine really beats another.
//!
//! The clips of one song aren't independent (a song that fools a key finder fools it in most of its clips), so every
//! range here comes from drawing whole songs again at random (a cluster bootstrap), never single clips.

/// A random number maker with a fixed start (SplitMix64), so every run of the bench prints the same numbers.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A whole number from 0 up to, not including, `n` (which must be above 0).
    pub fn below(&mut self, n: usize) -> usize {
        // The bias from taking the remainder is under one in 10^15 for any list the bench draws from.
        (self.next_u64() % n as u64) as usize
    }
}

/// How many times the songs are drawn again for a range.
const DRAWS: usize = 2000;

/// The songs' totals: for each song, how many cases and the sum of each of the values per case.
fn by_song<const N: usize>(values: &[[f64; N]], song: &[usize]) -> Vec<(f64, [f64; N])> {
    let songs = song.iter().max().map_or(0, |m| m + 1);
    let mut totals = vec![(0.0, [0.0; N]); songs];
    for (v, &s) in values.iter().zip(song) {
        if let Some((n, sum)) = totals.get_mut(s) {
            *n += 1.0;
            for (t, x) in sum.iter_mut().zip(v) {
                *t += x;
            }
        }
    }
    totals.retain(|(n, _)| *n > 0.0);
    totals
}

/// The 2.5% and 97.5% points of `f` over the songs drawn again: the range the true value most likely lies in.
fn range<const N: usize>(totals: &[(f64, [f64; N])], f: impl Fn(f64, &[f64; N]) -> f64) -> (f64, f64) {
    if totals.is_empty() {
        return (0.0, 0.0);
    }
    let mut rng = Rng::new(909);
    let mut values: Vec<f64> = (0..DRAWS)
        .map(|_| {
            let mut n = 0.0;
            let mut sum = [0.0; N];
            for _ in 0..totals.len() {
                if let Some((m, s)) = totals.get(rng.below(totals.len())) {
                    n += m;
                    for (t, x) in sum.iter_mut().zip(s) {
                        *t += x;
                    }
                }
            }
            f(n, &sum)
        })
        .collect();
    values.sort_by(f64::total_cmp);
    let at = |q: f64| values.get(((DRAWS as f64 - 1.0) * q).round() as usize).copied().unwrap_or_default();
    (at(0.025), at(0.975))
}

/// The share of cases where `right` holds, with its range when the songs are drawn again.
pub fn share(right: &[bool], song: &[usize]) -> Share {
    mean(&right.iter().map(|&r| f64::from(u8::from(r))).collect::<Vec<_>>(), song)
}

/// The average of a value per case, with its range when the songs are drawn again.
pub fn mean(values: &[f64], song: &[usize]) -> Share {
    let n = values.len();
    let value = if n == 0 { 0.0 } else { values.iter().sum::<f64>() / n as f64 };
    let per_case: Vec<[f64; 1]> = values.iter().map(|&v| [v]).collect();
    let totals = by_song(&per_case, song);
    let (low, high) = range(&totals, |n, s| if n > 0.0 { s[0] / n } else { 0.0 });
    Share { value, low, high, n }
}

/// A share made of sums, such as the time a chord reader got right over the time scored, each case adding to both,
/// with its range when the songs are drawn again.
pub fn ratio(right: &[f64], total: &[f64], song: &[usize]) -> Share {
    let values: Vec<[f64; 2]> = right.iter().zip(total).map(|(&r, &t)| [r, t]).collect();
    let (r, t) = values.iter().fold((0.0, 0.0), |(a, b), v| (a + v[0], b + v[1]));
    let totals = by_song(&values, song);
    let (low, high) = range(&totals, |_, s| if s[1] > 0.0 { s[0] / s[1] } else { 0.0 });
    Share { value: if t > 0.0 { r / t } else { 0.0 }, low, high, n: values.len() }
}

/// Two engines' shares made of sums over the same cases (`total` the same for both): `b`'s minus `a`'s, and the
/// range that difference most likely lies in when the songs are drawn again.
pub fn paired_ratio(a: &[f64], b: &[f64], total: &[f64], song: &[usize]) -> (f64, f64, f64) {
    let values: Vec<[f64; 3]> = a.iter().zip(b).zip(total).map(|((&x, &y), &t)| [x, y, t]).collect();
    let (x, y, t) = values.iter().fold((0.0, 0.0, 0.0), |(p, q, r), v| (p + v[0], q + v[1], r + v[2]));
    let totals = by_song(&values, song);
    let (low, high) = range(&totals, |_, s| if s[2] > 0.0 { (s[1] - s[0]) / s[2] } else { 0.0 });
    (if t > 0.0 { (y - x) / t } else { 0.0 }, low, high)
}

/// A share (0 to 1) or an average, how many cases it's over, and the range it most likely lies in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Share {
    pub value: f64,
    pub low: f64,
    pub high: f64,
    pub n: usize,
}

/// Two engines on the same cases: how much better `b` is than `a`, and whether that's more than chance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paired {
    /// `b`'s share right minus `a`'s.
    pub diff: f64,
    /// The range the difference most likely lies in, when the songs are drawn again.
    pub low: f64,
    pub high: f64,
    /// Cases only `a` got right, and cases only `b` did.
    pub only_a: usize,
    pub only_b: usize,
    /// McNemar's exact test on those two counts, treating every case as independent: how likely a split this uneven
    /// is if neither engine were better. The range above is the one to trust, since it keeps a song's clips together.
    pub p: f64,
}

pub fn paired(a: &[bool], b: &[bool], song: &[usize]) -> Paired {
    let n = a.len().min(b.len());
    let values: Vec<[f64; 2]> =
        a.iter().zip(b).map(|(&x, &y)| [f64::from(u8::from(x)), f64::from(u8::from(y))]).collect();
    let right_a = values.iter().map(|v| v[0]).sum::<f64>();
    let right_b = values.iter().map(|v| v[1]).sum::<f64>();
    let diff = if n == 0 { 0.0 } else { (right_b - right_a) / n as f64 };
    let totals = by_song(&values, song);
    let (low, high) = range(&totals, |n, s| if n > 0.0 { (s[1] - s[0]) / n } else { 0.0 });
    let only_a = a.iter().zip(b).filter(|&(&x, &y)| x && !y).count();
    let only_b = a.iter().zip(b).filter(|&(&x, &y)| !x && y).count();
    Paired { diff, low, high, only_a, only_b, p: mcnemar(only_a, only_b) }
}

/// McNemar's exact test, two-sided: the chance of `b` against `c` or a more uneven split, when each case that only
/// one engine gets right is a coin toss between them.
pub fn mcnemar(b: usize, c: usize) -> f64 {
    let n = b + c;
    if n == 0 {
        return 1.0;
    }
    // The binomial terms C(n, i) / 2^n for i = 0 to the smaller count, worked out as logarithms so a few thousand
    // cases don't run below the smallest number a float holds.
    let mut term = -(n as f64) * std::f64::consts::LN_2;
    let mut terms = vec![term];
    for i in 1..=b.min(c) {
        term += ((n - i + 1) as f64).ln() - (i as f64).ln();
        terms.push(term);
    }
    let top = terms.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let sum: f64 = terms.iter().map(|t| (t - top).exp()).sum();
    (2.0 * (top + sum.ln()).exp()).min(1.0)
}

/// Of the answers kept when only the most confident share `coverage` of all cases is answered (the rest left
/// blank), the share that are right. `answers` is each case's confidence (`None` when the engine gave no answer)
/// and whether it was right.
pub fn right_when_most_sure(answers: &[(Option<f64>, bool)], coverage: f64) -> Option<f64> {
    let mut sure: Vec<(f64, bool)> = answers.iter().filter_map(|&(c, r)| c.map(|c| (c, r))).collect();
    // Most confident first; a stable sort keeps ties in case order, so runs repeat.
    sure.sort_by(|a, b| b.0.total_cmp(&a.0));
    let keep = (coverage * answers.len() as f64).round() as usize;
    if keep == 0 || keep > sure.len() {
        return None;
    }
    let kept = sure.get(..keep)?;
    Some(kept.iter().filter(|(_, r)| *r).count() as f64 / keep as f64)
}

/// How far the confidences are from being chances: the answers are put in ten bins by confidence, and each bin's
/// share right is compared with its average confidence (expected calibration error, weighted by the bin's size). 0 is
/// a confidence that means exactly what it says ("80% sure" right 80% of the time); a finder with no answers gives
/// `None`. Cases with no answer are left out.
pub fn calibration_error(answers: &[(Option<f64>, bool)]) -> Option<f64> {
    let mut bins = [(0.0f64, 0.0f64, 0.0f64); 10];
    let mut n = 0.0;
    for &(sure, right) in answers {
        let Some(c) = sure else { continue };
        let c = c.clamp(0.0, 1.0);
        let b = ((c * 10.0) as usize).min(9);
        if let Some((count, conf, hits)) = bins.get_mut(b) {
            *count += 1.0;
            *conf += c;
            *hits += f64::from(u8::from(right));
        }
        n += 1.0;
    }
    (n > 0.0).then(|| bins.iter().filter(|b| b.0 > 0.0).map(|(_, conf, hits)| (hits - conf).abs() / n).sum::<f64>())
}

/// The same, with bins of equal size instead of equal width (`bins` of them, the most confident last): every bin has
/// enough answers to say something, so a finder whose answers bunch up at one confidence can't hide in an empty bin.
pub fn calibration_error_equal_mass(answers: &[(Option<f64>, bool)], bins: usize) -> Option<f64> {
    let mut sure: Vec<(f64, bool)> = answers.iter().filter_map(|&(c, r)| c.map(|c| (c.clamp(0.0, 1.0), r))).collect();
    if sure.is_empty() || bins == 0 {
        return None;
    }
    sure.sort_by(|a, b| a.0.total_cmp(&b.0));
    let n = sure.len();
    let mut error = 0.0;
    for b in 0..bins {
        let Some(bin) = sure.get(b * n / bins..(b + 1) * n / bins) else { continue };
        if bin.is_empty() {
            continue;
        }
        let conf: f64 = bin.iter().map(|(c, _)| c).sum();
        let hits = bin.iter().filter(|(_, r)| *r).count() as f64;
        error += (hits - conf).abs() / n as f64;
    }
    Some(error)
}

/// The area under the risk-coverage curve: answering only the k surest cases, for every k, the average share wrong.
/// 0 is a finder whose wrong answers are all less sure than its right ones and that is never wrong; lower is better.
/// Cases with no answer count as wrong and least sure.
pub fn risk_coverage_area(answers: &[(Option<f64>, bool)]) -> Option<f64> {
    if answers.is_empty() {
        return None;
    }
    let mut sure: Vec<(f64, bool)> =
        answers.iter().map(|&(c, r)| (c.unwrap_or(f64::NEG_INFINITY), r && c.is_some())).collect();
    sure.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (mut wrong, mut area) = (0.0, 0.0);
    for (k, (_, right)) in sure.iter().enumerate() {
        wrong += f64::from(u8::from(!right));
        area += wrong / (k + 1) as f64;
    }
    Some(area / sure.len() as f64)
}

/// The upper end of the likely range (95%) of an error share, from each song's cases and errors. It's a Wilson bound
/// on the share with the cases counted as fewer when a song's cases go wrong together (the design effect, from how
/// far each song's errors stray from the share), and as no more than the songs when there's no error to measure that
/// by: a share measured on a few songs is never taken as sure. 1 when there are no cases.
pub fn error_upper(per_song: &[(f64, f64)]) -> f64 {
    let songs = per_song.iter().filter(|(n, _)| *n > 0.0).count() as f64;
    let (n, errors) = per_song.iter().fold((0.0, 0.0), |(a, b), (n, e)| (a + n, b + e));
    if n <= 0.0 {
        return 1.0;
    }
    let p = errors / n;
    let n_eff = if p > 0.0 && p < 1.0 && songs > 1.0 {
        let spread: f64 = per_song.iter().map(|(ns, es)| (es - p * ns).powi(2)).sum();
        let variance = spread / (n * n) * songs / (songs - 1.0);
        let design_effect = variance / (p * (1.0 - p) / n);
        n / design_effect.max(1.0)
    } else {
        songs.min(n)
    };
    let z2 = 1.96f64 * 1.96;
    let centre = p + z2 / (2.0 * n_eff);
    let half = (z2 * (p * (1.0 - p) / n_eff + z2 / (4.0 * n_eff * n_eff))).sqrt();
    ((centre + half) / (1.0 + z2 / n_eff)).min(1.0)
}

/// "78.2%".
pub fn pct(x: f64) -> String {
    format!("{:.1}%", 100.0 * x)
}

/// "78.2% (76.9 to 79.4)".
pub fn pct_range(s: Share) -> String {
    format!("{:.1}% ({:.1} to {:.1})", 100.0 * s.value, 100.0 * s.low, 100.0 * s.high)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_share_on_few_songs_is_never_sure() {
        // No errors on 1,000 cases from 2 songs: as unsure as 2 cases (3.84 / 5.84), not 1,000.
        assert!((error_upper(&[(500.0, 0.0), (500.0, 0.0)]) - 3.8416 / 5.8416).abs() < 1e-12);
        // The same on 1,000 songs, one case each: about 0.4%.
        let many = vec![(1.0, 0.0); 1000];
        assert!((error_upper(&many) - 3.8416 / 1003.8416).abs() < 1e-12);
        // 10% wrong, spread evenly over 100 songs of 10 cases: the plain Wilson bound on 1,000 cases (about 12.0%).
        let even = vec![(10.0, 1.0); 100];
        assert!((error_upper(&even) - 0.1203).abs() < 1e-3, "{}", error_upper(&even));
        // 10% wrong, but all of it in 10 songs: far less sure (the cases count as about 100).
        let bunched: Vec<(f64, f64)> = (0..100).map(|i| (10.0, if i < 10 { 10.0 } else { 0.0 })).collect();
        assert!(error_upper(&bunched) > 0.16, "{}", error_upper(&bunched));
        assert_eq!(error_upper(&[]), 1.0);
    }

    #[test]
    fn mcnemar_matches_the_binomial_by_hand() {
        assert_eq!(mcnemar(0, 0), 1.0);
        assert_eq!(mcnemar(5, 5), 1.0);
        // 0 against 10: both tails of 1/1024.
        assert!((mcnemar(0, 10) - 2.0 / 1024.0).abs() < 1e-12);
        // 1 against 9: 2 × (1 + 10) / 1024.
        assert!((mcnemar(9, 1) - 22.0 / 1024.0).abs() < 1e-12);
        // Thousands of cases don't underflow.
        assert!(mcnemar(1000, 1300) > 0.0 && mcnemar(1000, 1300) < 1e-4);
    }

    #[test]
    fn ranges_hold_the_value_and_keep_songs_together() {
        // Ten songs of ten clips: five songs always right, five always wrong.
        let song: Vec<usize> = (0..100).map(|i| i / 10).collect();
        let right: Vec<bool> = (0..100).map(|i| i < 50).collect();
        let s = share(&right, &song);
        assert_eq!((s.value, s.n), (0.5, 100));
        assert!(s.low < 0.5 && s.high > 0.5);
        // With the clips spread over a hundred songs the range is narrower than with whole songs going together.
        let spread: Vec<usize> = (0..100).collect();
        let t = share(&right, &spread);
        assert!(t.high - t.low < s.high - s.low);
        // The same engine against itself: no difference at all.
        let p = paired(&right, &right, &song);
        assert_eq!((p.diff, p.low, p.high, p.only_a, p.only_b, p.p), (0.0, 0.0, 0.0, 0, 0, 1.0));
    }

    #[test]
    fn keeping_only_the_surest_answers() {
        let answers = [(Some(0.9), true), (Some(0.8), true), (Some(0.1), false), (None, false)];
        assert_eq!(right_when_most_sure(&answers, 0.5), Some(1.0));
        assert_eq!(right_when_most_sure(&answers, 0.75), Some(2.0 / 3.0));
        // More cases than it answered: no figure.
        assert_eq!(right_when_most_sure(&answers, 1.0), None);
    }

    #[test]
    fn confidence_that_means_what_it_says_has_no_calibration_error() {
        // Ten answers 80% sure, eight right; ten 30% sure, three right.
        let mut answers = Vec::new();
        for i in 0..10 {
            answers.push((Some(0.8), i < 8));
            answers.push((Some(0.3), i < 3));
        }
        assert!(calibration_error(&answers).is_some_and(|e| e < 1e-9));
        // Always 90% sure, right half the time: 0.4 off.
        let over: Vec<(Option<f64>, bool)> = (0..10).map(|i| (Some(0.9), i % 2 == 0)).collect();
        assert!(calibration_error(&over).is_some_and(|e| (e - 0.4).abs() < 1e-9));
        assert_eq!(calibration_error(&[(None, true)]), None);
        // Equal-size bins agree on honest answers, and find the same 0.4 when always 90% sure.
        assert!(calibration_error_equal_mass(&answers, 2).is_some_and(|e| e < 1e-9));
        assert!(calibration_error_equal_mass(&over, 5).is_some_and(|e| (e - 0.4).abs() < 1e-9));
    }

    #[test]
    fn risk_falls_when_the_wrong_answers_are_the_least_sure() {
        let honest = [(Some(0.9), true), (Some(0.8), true), (Some(0.2), false)];
        let backwards = [(Some(0.9), false), (Some(0.8), true), (Some(0.2), true)];
        // Honest: 0, 0, then 1/3 wrong: a ninth on average. Backwards: 1, 1/2, 1/3.
        assert!(risk_coverage_area(&honest).is_some_and(|a| (a - 1.0 / 9.0).abs() < 1e-12));
        assert!(risk_coverage_area(&backwards).is_some_and(|a| (a - (1.0 + 0.5 + 1.0 / 3.0) / 3.0).abs() < 1e-12));
        assert_eq!(risk_coverage_area(&[]), None);
    }
}
