//! The year strip on Sounds, Songs, MIDI, Plugins and Presets: a column of years, newest at the top, to drag over to
//! pick a span of years.

use chrono::Datelike as _;

/// The years on a strip, newest first, from how many there are in each year (in any order). A file with no date reads
/// as 1970, and one whose date is broken can be years ahead (a sound dated 2107 on Solomon's PC, 5 Nov 2026): neither
/// is a year anyone picks, so neither is on the strip. What's in them shows while no years are picked, and goes while
/// some are.
pub fn strip(counts: impl IntoIterator<Item = (i32, usize)>) -> Vec<(i32, usize)> {
    strip_in(counts, chrono::Local::now().year())
}

/// [`strip`] in `this_year`.
fn strip_in(counts: impl IntoIterator<Item = (i32, usize)>, this_year: i32) -> Vec<(i32, usize)> {
    let mut years: Vec<(i32, usize)> =
        counts.into_iter().filter(|&(year, _)| year > 1980 && year <= this_year).collect();
    years.sort_unstable_by_key(|&(year, _)| std::cmp::Reverse(year));
    years
}

/// The years picked on the strip, first to last, when places `a` to `b` of it are dragged over (`a` at or above
/// `b`). `years` is the strip, newest first, each year with its count. The strip runs newest first, so its first
/// place picked is the last year. `None` when a place is off the strip.
pub fn year_span(years: &[(i32, usize)], a: i32, b: i32) -> Option<(i32, i32)> {
    let at = |i: i32| years.get(usize::try_from(i).ok()?).map(|y| y.0);
    Some((at(b)?, at(a)?))
}

/// The way back: where the years picked (`span`, first to last, as the filter holds them) are on the strip, as places
/// in `years` top first, from the newest year on the strip that's in the span to the oldest. `None` when nothing is
/// picked, or no year on the strip is in the span.
pub fn picked_places(years: &[(i32, usize)], span: Option<(i32, i32)>) -> Option<(i32, i32)> {
    let (first, last) = span?;
    let inside = |&(year, _): &(i32, usize)| year >= first && year <= last;
    let place = |at: usize| i32::try_from(at).ok();
    Some((place(years.iter().position(inside)?)?, place(years.iter().rposition(inside)?)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_strip_has_no_year_before_dates_or_after_this_one() {
        // No date (1970) and a broken one years ahead (2107) aren't on it; the rest run newest first.
        let counts = [(2019, 5), (2107, 1), (1970, 3), (2026, 2), (2024, 1)];
        assert_eq!(strip_in(counts, 2026), [(2026, 2), (2024, 1), (2019, 5)]);
        // Next year isn't on it until it comes.
        assert_eq!(strip_in([(2027, 1), (2026, 4)], 2026), [(2026, 4)]);
        assert_eq!(strip_in([(2027, 1), (2026, 4)], 2027), [(2027, 1), (2026, 4)]);
        assert_eq!(strip_in([], 2026), []);
    }

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

    #[test]
    fn the_years_picked_are_found_on_the_strip_top_first() {
        let strip = [(2026, 3), (2024, 1), (2023, 2), (2019, 5)];
        assert_eq!(picked_places(&strip, Some((2023, 2024))), Some((1, 2)));
        // The way back gives the same years.
        assert_eq!(year_span(&strip, 1, 2), Some((2023, 2024)));
        // A span reaching past the strip's years starts and ends at the ones it has.
        assert_eq!(picked_places(&strip, Some((2020, 2030))), Some((0, 2)));
        assert_eq!(picked_places(&strip, Some((2019, 2019))), Some((3, 3)));
        // Nothing picked, or no year on the strip in the span.
        assert_eq!(picked_places(&strip, None), None);
        assert_eq!(picked_places(&strip, Some((2020, 2022))), None);
        assert_eq!(picked_places(&[], Some((2020, 2022))), None);
    }
}
