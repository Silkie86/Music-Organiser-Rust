//! A part of a sound picked on its waveform: the small sums the Sounds card, the Edit strip and the plugin's panel
//! share (how its length reads, where its edges snap to, the hits' marks along a waveform's foot, and a waveform's
//! lines), with no screen.

use std::fmt::Write as _;

/// How long a part lasts, as the card's handle says it: "0.47 s", "12.3 s", "1:05". Rounded before the way it's
/// written is chosen, so 9.996 s reads "10.0 s" and 59.96 s "1:00".
pub fn length_text(seconds: f64) -> String {
    let hundredths = (seconds * 100.0).round() / 100.0;
    let tenths = (seconds * 10.0).round() / 10.0;
    if tenths >= 60.0 {
        let whole = seconds.round() as u64;
        format!("{}:{:02}", whole / 60, whole % 60)
    } else if hundredths >= 10.0 {
        format!("{tenths:.1} s")
    } else {
        format!("{hundredths:.2} s")
    }
}

/// The hits at `at` (0 to 1) as short lines along a waveform's foot, a path 1000 across and 10 high. Hits closer than
/// a few tenths of a percent of the width to the last one shown are left out: on the card they'd be one mark.
pub fn marks(at: &[f64]) -> String {
    let mut path = String::new();
    let mut last = f64::NEG_INFINITY;
    for &a in at {
        let x = a * 1000.0;
        if x - last < 4.0 {
            continue;
        }
        last = x;
        let _ = write!(path, "M {x:.1} 10 L {x:.1} 0 ");
    }
    path
}

/// Where a part's edge goes with the mouse at `at`: on the nearest of `hits` (in order, in what `at` is counted in)
/// no further away than `within`, else at `at` (and always with `within` 0: Shift).
pub fn snap_to(hits: &[f64], at: f64, within: f64) -> f64 {
    if within <= 0.0 || !at.is_finite() {
        return at;
    }
    hits.iter()
        .copied()
        .filter(|h| (h - at).abs() <= within)
        .min_by(|a, b| (a - at).abs().total_cmp(&(b - at).abs()))
        .unwrap_or(at)
}

/// `steps` (a waveform's loudest samples, one a step) as `points` lines, each the loudest of its share.
pub fn lines(steps: &[f32], points: usize) -> Vec<f32> {
    (0..points)
        .map(|p| {
            let from = steps.len() * p / points;
            let to = (from + 1).max(steps.len() * (p + 1) / points).min(steps.len());
            steps.get(from..to).unwrap_or_default().iter().copied().fold(0.0, f32::max)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edge_snaps_to_the_nearest_hit_within_reach_only() {
        let hits = [0.1, 0.25, 0.3];
        assert_eq!(snap_to(&hits, 0.27, 0.05), 0.25);
        assert_eq!(snap_to(&hits, 0.29, 0.05), 0.3);
        assert_eq!(snap_to(&hits, 0.5, 0.05), 0.5, "none near");
        assert_eq!(snap_to(&hits, 0.27, 0.0), 0.27, "Shift: no snap");
        assert_eq!(snap_to(&[], 0.27, 0.05), 0.27);
    }

    #[test]
    fn a_parts_length_reads_short() {
        assert_eq!(length_text(0.46875), "0.47 s");
        assert_eq!(length_text(12.34), "12.3 s");
        assert_eq!(length_text(65.2), "1:05");
        // Rounded first, then written.
        assert_eq!(length_text(9.996), "10.0 s");
        assert_eq!(length_text(59.96), "1:00");
        assert_eq!(length_text(59.94), "59.9 s");
    }

    #[test]
    fn marks_close_together_are_one() {
        assert_eq!(marks(&[0.1, 0.1005, 0.5]), "M 100.0 10 L 100.0 0 M 500.0 10 L 500.0 0 ");
        assert_eq!(marks(&[]), "");
    }

    #[test]
    fn lines_take_the_loudest_of_their_share() {
        assert_eq!(lines(&[0.1, 0.5, 0.2, 0.9], 2), vec![0.5, 0.9]);
        // Fewer steps than lines: each line shows the step under it.
        assert_eq!(lines(&[0.1, 0.5], 4), vec![0.1, 0.1, 0.5, 0.5]);
        assert_eq!(lines(&[], 3), vec![0.0; 3]);
    }
}
