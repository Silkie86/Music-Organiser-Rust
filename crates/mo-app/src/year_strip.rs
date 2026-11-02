//! The year strip on Sounds and Songs: a column of years, newest at the top, to drag over to pick a span of years.

/// The years picked on the strip, first to last, when places `a` to `b` of it are dragged over (`a` at or above
/// `b`). `years` is the strip, newest first, each year with its count. The strip runs newest first, so its first
/// place picked is the last year. `None` when a place is off the strip.
pub fn year_span(years: &[(i32, usize)], a: i32, b: i32) -> Option<(i32, i32)> {
    let at = |i: i32| years.get(usize::try_from(i).ok()?).map(|y| y.0);
    Some((at(b)?, at(a)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_down_the_strip_picks_the_years_oldest_first() {
        let strip = [(2026, 3), (2024, 1), (2023, 2), (2019, 5)];
        assert_eq!(year_span(&strip, 1, 2), Some((2023, 2024)));
        assert_eq!(year_span(&strip, 0, 3), Some((2019, 2026)));
        // One place picks one year.
        assert_eq!(year_span(&strip, 2, 2), Some((2023, 2023)));
        // Off the strip: nothing.
        assert_eq!(year_span(&strip, -1, 2), None);
        assert_eq!(year_span(&strip, 1, 4), None);
        assert_eq!(year_span(&[], 0, 0), None);
    }
}
