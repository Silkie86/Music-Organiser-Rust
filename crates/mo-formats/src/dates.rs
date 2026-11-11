//! Whether to believe the date a file says it was saved. A file's date is whatever its disk, or the zip or computer it
//! came from, kept, and some are plainly wrong: a MIDI pack whose files all say 31 Dec 2027, a sound dated 2107, or a
//! file with no date at all, which reads as 1 Jan 1970 (or 1 Jan 1980 out of a zip). A list in date order puts a date
//! that isn't real after all the real ones, whichever way round it goes (as a line with nothing in its column comes
//! last), and the date isn't shown as the day the file was saved.

use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};

/// The first year a real date can be in. Before it are the dates of a file with none: 1970, and 1980 (the first a zip
/// or a FAT disk can keep). The year strips start after 1980 too.
pub const FIRST_YEAR: i32 = 1981;

/// How far past now a real date can be: a day, so a file from a clock a few hours on (another time zone, or a zip that
/// kept local time) is still believed.
const SLACK: i64 = 24 * 60 * 60;

/// Which dates are real, as of when it was made. Made once for a whole list (and kept with it, so that its order, its
/// dates and its years agree for as long as it lasts), so that each date is then two comparisons. The default is as
/// of now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RealDates {
    /// The first and the last real moment, in Unix seconds.
    first: i64,
    last: i64,
    /// The last real moment as local time.
    last_local: NaiveDateTime,
}

impl RealDates {
    /// The real dates as of now.
    pub fn now() -> RealDates {
        RealDates::at(Utc::now().timestamp())
    }

    /// The real dates as of `now`, in Unix seconds.
    pub fn at(now: i64) -> RealDates {
        let last = now.saturating_add(SLACK);
        let first_day =
            NaiveDate::from_ymd_opt(FIRST_YEAR, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0)).unwrap_or_default();
        // Midnight in this computer's time zone (in UTC if its clocks skipped that moment).
        let first =
            Local.from_local_datetime(&first_day).earliest().map_or(first_day.and_utc().timestamp(), |t| t.timestamp());
        let last_local = match Local.timestamp_opt(last, 0).single() {
            Some(t) => t.naive_local(),
            // Past either end of the calendar.
            None if last > 0 => NaiveDateTime::MAX,
            None => NaiveDateTime::MIN,
        };
        RealDates { first, last, last_local }
    }

    /// Whether a date in Unix seconds, as a file's modified time is kept, is real.
    pub fn seconds(&self, seconds: i64) -> bool {
        (self.first..=self.last).contains(&seconds)
    }

    /// Whether a date in local time is real.
    pub fn local(&self, at: NaiveDateTime) -> bool {
        at.year() >= FIRST_YEAR && at <= self.last_local
    }

    /// The local year of a date in Unix seconds, when it's real: a date that isn't is in no year.
    pub fn year(&self, seconds: i64) -> Option<i32> {
        if !self.seconds(seconds) {
            return None;
        }
        Local.timestamp_opt(seconds, 0).single().map(|t| t.year())
    }
}

impl Default for RealDates {
    fn default() -> RealDates {
        RealDates::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 11 Nov 2026, 12:00 UTC.
    const NOW: i64 = 1_794_398_400;
    const HOUR: i64 = 60 * 60;
    const DAY: i64 = 24 * HOUR;

    fn local(seconds: i64) -> NaiveDateTime {
        Local.timestamp_opt(seconds, 0).unwrap().naive_local()
    }

    #[test]
    fn a_date_is_real_from_1981_to_a_day_from_now() {
        let real = RealDates::at(NOW);
        let day = |y: i32, m: u32, d: u32| NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(12, 0, 0).unwrap();
        // A file with no date (1970, or a zip's 1980), and a file that couldn't be read (-1).
        for no_date in [0, -1, day(1980, 1, 1).and_utc().timestamp(), day(1980, 12, 30).and_utc().timestamp()] {
            assert!(!real.seconds(no_date), "{no_date}");
            assert!(!real.local(local(no_date)), "{no_date}");
        }
        for date in [day(1981, 1, 2), day(2019, 6, 1), local(NOW), local(NOW + 2 * HOUR), local(NOW + DAY - HOUR)] {
            assert!(real.local(date), "{date}");
        }
        for seconds in [day(1981, 1, 2).and_utc().timestamp(), NOW - 10 * 365 * DAY, NOW, NOW + 2 * HOUR, NOW + DAY] {
            assert!(real.seconds(seconds), "{seconds}");
        }
        // Still to come: tomorrow evening, the pack's 31 Dec 2027, the sound's 2107.
        for future in [NOW + DAY + HOUR, day(2027, 12, 31).and_utc().timestamp(), day(2107, 3, 1).and_utc().timestamp()]
        {
            assert!(!real.seconds(future), "{future}");
            assert!(!real.local(local(future)), "{future}");
        }
        assert!(!real.local(NaiveDateTime::MAX) && !real.seconds(i64::MAX) && !real.seconds(i64::MIN));
        // In a year only when real.
        assert_eq!(real.year(day(2019, 6, 1).and_utc().timestamp()), Some(2019));
        assert_eq!((real.year(0), real.year(day(2027, 12, 31).and_utc().timestamp())), (None, None));
    }

    #[test]
    fn real_dates_move_on_with_the_clock() {
        // What's still to come today is real a few days on.
        let later = NOW + 3 * DAY;
        assert!(!RealDates::at(NOW).seconds(later) && RealDates::at(NOW + 2 * DAY).seconds(later));
        assert!(
            RealDates::now().seconds(Local::now().timestamp()) && RealDates::default().seconds(Utc::now().timestamp())
        );
        // Far ends of the clock don't overflow.
        assert!(RealDates::at(i64::MAX).seconds(NOW) && RealDates::at(i64::MAX).local(local(NOW)));
        assert!(!RealDates::at(i64::MIN).seconds(NOW) && !RealDates::at(i64::MIN).local(local(NOW)));
    }
}
