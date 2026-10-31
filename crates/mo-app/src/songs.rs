//! The Songs page's list: one row per song (its saves grouped as the C# app groups them), newest save first, under
//! "This week", "This month", "Earlier in 2026" and one heading per year, with search and filter chips. Agreed with
//! Solomon on 2026-10-31 (no Elektron box chip).
//!
//! Reading the library and grouping happen once per load, off the window's thread; searching and filtering then only
//! walk the rows already made, so typing in the search box stays quick over 16,000 songs (rule 12).

use std::collections::BTreeSet;

use chrono::{Datelike, Days, NaiveDateTime};
use mo_formats::case;
use mo_library::songs::{self, SaveRun, Song};
use mo_library::{Library, LibraryError, STATUSES};

/// One song in the list, with the fields that belong to the song (rule 14).
#[derive(Clone, Debug)]
pub struct SongRow {
    pub song: Song,
    /// When the song was first made: its oldest save, or the date its file keeps, whichever is earlier.
    pub made: NaiveDateTime,
    /// Its working periods, oldest first.
    pub runs: Vec<SaveRun>,
    // Your fields are the song's, but the C# app stores them on the save you set them on, and a newer save starts
    // blank: each one the newest save hasn't got comes from the newest older save that has it.
    pub status: String,
    pub rating: i32,
    pub favourite: bool,
    pub musical_key: String,
    pub notes: String,
    pub tags: Vec<String>,
    /// Everything the search box looks in, upper case, one field after another.
    find: String,
}

impl SongRow {
    pub fn new(song: Song, gap_months: u32) -> SongRow {
        let made = songs::first_made(song.saves()).unwrap_or(song.latest.modified);
        let runs = songs::runs(song.saves(), gap_months);
        let mut earlier: Vec<&mo_library::Save> = song.older.iter().collect();
        earlier.sort_by_key(|s| std::cmp::Reverse(s.modified));
        let latest = &song.latest;
        let pick = |own: &str, of: fn(&mo_library::Save) -> &str| -> String {
            if !own.trim().is_empty() {
                return own.to_owned();
            }
            earlier.iter().map(|s| of(s)).find(|v| !v.trim().is_empty()).unwrap_or(own).to_owned()
        };
        let status = if latest.status != STATUSES[0] {
            latest.status.clone()
        } else {
            earlier.iter().find(|s| s.status != STATUSES[0]).map_or_else(|| latest.status.clone(), |s| s.status.clone())
        };
        let rating = if latest.rating > 0 {
            latest.rating
        } else {
            earlier.iter().find(|s| s.rating > 0).map_or(0, |s| s.rating)
        };
        let mut row = SongRow {
            made,
            runs,
            status,
            rating,
            favourite: latest.favourite || earlier.iter().any(|s| s.favourite),
            musical_key: pick(&latest.musical_key, |s| &s.musical_key),
            notes: pick(&latest.notes, |s| &s.notes),
            tags: latest.tags.clone(),
            find: String::new(),
            song,
        };
        row.find = row.search_text();
        row
    }

    fn search_text(&self) -> String {
        let l = &self.song.latest;
        let mut fields = vec![self.song.name.as_str(), &l.name, &l.folder, &self.notes, &l.daw];
        fields.extend(self.tags.iter().map(String::as_str));
        fields.extend(l.plugin_names.iter().map(String::as_str));
        case::key(&fields.join("\u{1f}"))
    }

    /// The key: the one typed in, or else the one its notes point to.
    pub fn key(&self) -> &str {
        let typed = self.musical_key.trim();
        if typed.is_empty() { &self.song.latest.detected_key } else { typed }
    }

    /// The newest run, when it's a rework: quite different from how the song first ended up.
    pub fn rework(&self) -> Option<&SaveRun> {
        self.runs.last().filter(|r| self.runs.len() > 1 && r.reworked)
    }

    /// The folder the song's newest save is in, without the path to it.
    pub fn folder_name(&self) -> &str {
        let f = self.song.latest.folder.trim_end_matches(['\\', '/']);
        f.rsplit(['\\', '/']).next().unwrap_or(f)
    }

    /// When it was last saved: "3 Aug" this year, "3 Aug 2019" before.
    pub fn short_date(&self, now: NaiveDateTime) -> String {
        let saved = self.song.latest.modified;
        if saved.year() == now.year() {
            saved.format("%-d %b").to_string()
        } else {
            saved.format("%-d %b %Y").to_string()
        }
    }

    /// The date's tooltip: when it was last saved, and when it was first made if that's another day.
    pub fn date_tip(&self) -> String {
        let saved = self.song.latest.modified;
        if self.made.date() == saved.date() {
            format!("Made {}", saved.format("%-d %b %Y"))
        } else {
            format!("Last saved {}. First made {}.", saved.format("%-d %b %Y"), self.made.format("%-d %b %Y"))
        }
    }

    /// The row's second line: "FL Studio · 140 BPM · G minor · 77 saves · reworked 2024", leaving out what isn't
    /// known.
    pub fn details_line(&self) -> String {
        let l = &self.song.latest;
        let mut parts = vec![l.daw.clone()];
        if let Some(t) = l.tempo {
            parts.push(format!("{} BPM", tempo_text(t)));
        }
        if !self.key().is_empty() {
            parts.push(self.key().to_owned());
        }
        if self.song.save_count() > 1 {
            parts.push(format!("{} saves", self.song.save_count()));
        }
        if let Some(r) = self.rework() {
            parts.push(format!("reworked {}", r.end.year()));
        }
        parts.join(" · ")
    }

    /// What's missing: "2 plugins, 1 sample, file moved", or "" when nothing is.
    pub fn missing_text(&self) -> String {
        let l = &self.song.latest;
        let mut parts = Vec::new();
        if l.missing_plugins > 0 {
            parts.push(plural(l.missing_plugins as usize, "plugin"));
        }
        if l.missing_samples > 0 {
            parts.push(plural(l.missing_samples as usize, "sample"));
        }
        if l.file_missing {
            parts.push("file moved".to_owned());
        }
        parts.join(", ")
    }

    pub fn has_missing(&self) -> bool {
        let l = &self.song.latest;
        l.missing_plugins + l.missing_samples > 0 || l.file_missing
    }

    /// Whether the search box's text finds this song. Several words must each match something, in any order, so
    /// "serum dark" finds a song using Serum with "dark" in its name or notes; a year such as 2019 also finds songs
    /// saved or made that year; a key ("G minor") must match whole.
    pub fn matches(&self, text: &str) -> bool {
        let text = text.trim();
        if text.is_empty() || self.matches_word(text) {
            return true;
        }
        let words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
        words.len() > 1 && words.iter().all(|w| self.matches_word(w))
    }

    fn matches_word(&self, word: &str) -> bool {
        self.find.contains(&case::key(word))
            || case::eq_ignore_case(self.key(), word)
            || (word.len() == 4
                && word.parse::<i32>().is_ok_and(|y| y == self.song.latest.modified.year() || y == self.made.year()))
    }
}

/// "140", "92.5", "128.25": a tempo with at most two decimals and none it doesn't need.
pub fn tempo_text(t: f64) -> String {
    let s = format!("{t:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{} {what}s", thousands(n)) }
}

/// "16,000".
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The list's time heading for a save made on `saved`: "This week", "This month", "Earlier in 2026", then one per
/// year.
pub fn when_group(saved: NaiveDateTime, now: NaiveDateTime) -> String {
    let week_ago = now.date().checked_sub_days(Days::new(7)).unwrap_or(now.date());
    if saved.date() > week_ago {
        "This week".to_owned()
    } else if saved.year() == now.year() && saved.month() == now.month() {
        "This month".to_owned()
    } else if saved.year() == now.year() {
        format!("Earlier in {}", now.year())
    } else {
        saved.year().to_string()
    }
}

/// Everything the page reads from the library in one go, on a background thread.
#[derive(Clone, Debug, Default)]
pub struct Songs {
    /// Newest save first.
    pub rows: Vec<SongRow>,
    /// How many saves the rows hold.
    pub saves: usize,
    /// The tags in use, for the Tag chip.
    pub tags: Vec<String>,
}

impl Songs {
    /// Reads the saves and groups them into songs. Fix's copies still waiting for Keep or Delete are left out: until
    /// Keep, the song opens its original. `gap_months` splits a song's saves into runs.
    pub fn load(library: &Library, gap_months: u32) -> Result<Songs, LibraryError> {
        let pending = library.pending_fixes()?;
        let mut saves = library.saves()?;
        if !pending.is_empty() {
            saves.retain(|s| !pending.contains(&s.path));
        }
        let choices = library.song_choices()?;
        let rows = songs::group_songs(&saves, &choices).into_iter().map(|g| SongRow::new(g, gap_months)).collect();
        let tags = library.all_tags()?.into_iter().map(|(t, _)| t).collect();
        Ok(Songs { rows, saves: saves.len(), tags })
    }

    /// The DAWs the songs come from, for the DAW chip, in order.
    pub fn daws(&self) -> Vec<String> {
        let set: BTreeSet<&str> = self.rows.iter().map(|r| r.song.latest.daw.as_str()).collect();
        set.into_iter().map(str::to_owned).collect()
    }

    /// The rows the filters let through, under their headings, with where each year starts for the scrubber.
    pub fn view(&self, filters: &Filters, now: NaiveDateTime) -> View {
        let mut view = View::default();
        let mut heading = String::new();
        let mut shown = 0;
        for (i, row) in self.rows.iter().enumerate().filter(|(_, r)| filters.allows(r)) {
            let h = when_group(row.song.latest.modified, now);
            if h != heading {
                let year = row.song.latest.modified.year();
                if view.years.last().is_none_or(|(y, _)| *y != year) {
                    view.years.push((year, view.entries.len()));
                }
                view.entries.push(Entry::Heading(h.clone()));
                heading = h;
            }
            view.entries.push(Entry::Song(i));
            shown += 1;
        }
        let all = self.rows.len();
        let mut count =
            if shown == all { plural(all, "song") } else { format!("{} of {}", thousands(shown), plural(all, "song")) };
        if self.saves > all {
            count.push_str(&format!(" ({} saves)", thousands(self.saves)));
        }
        view.count = count;
        view.shown = shown;
        view
    }
}

/// One line of the list: a heading, or a song (its place in [`Songs::rows`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Heading(String),
    Song(usize),
}

/// What the list shows.
#[derive(Clone, Debug, Default)]
pub struct View {
    pub entries: Vec<Entry>,
    /// Each year shown, newest first, with the line its first heading is on: the year scrubber.
    pub years: Vec<(i32, usize)>,
    /// How many songs are shown.
    pub shown: usize,
    /// "1,234 of 16,000 songs (52,000 saves)".
    pub count: String,
}

/// The search box and filter chips. They start empty each time the app opens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filters {
    pub search: String,
    pub daw: Option<String>,
    pub status: Option<String>,
    pub tag: Option<String>,
    pub favourites: bool,
    /// Only songs missing a plugin, a sample or their file.
    pub missing: bool,
    /// Archived songs are left out unless their status is picked.
    pub hide_archived: bool,
}

impl Filters {
    pub fn allows(&self, r: &SongRow) -> bool {
        let l = &r.song.latest;
        if self.daw.as_ref().is_some_and(|d| *d != l.daw) {
            return false;
        }
        match &self.status {
            Some(s) if *s != r.status => return false,
            None if self.hide_archived && r.status == "Archived" => return false,
            _ => {}
        }
        if self.tag.as_ref().is_some_and(|t| !r.tags.contains(t)) {
            return false;
        }
        if self.favourites && !r.favourite {
            return false;
        }
        if self.missing && !r.has_missing() {
            return false;
        }
        r.matches(&self.search)
    }

    /// Whether any filter is set (the Clear button shows).
    pub fn any(&self) -> bool {
        *self != Filters { hide_archived: self.hide_archived, ..Filters::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use mo_library::{Save, SongChoices};

    fn day(y: i32, m: u32, d: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(12, 0, 0).unwrap()
    }

    fn save(name: &str, saved: NaiveDateTime) -> Save {
        Save {
            name: name.into(),
            path: format!(r"C:\beats\{name}.flp"),
            daw: "FL Studio".into(),
            folder: r"C:\beats".into(),
            modified: saved,
            ..Save::default()
        }
    }

    fn songs(saves: &[Save]) -> Songs {
        let rows = songs::group_songs(saves, &SongChoices::default()).into_iter().map(|g| SongRow::new(g, 3)).collect();
        Songs { rows, saves: saves.len(), tags: Vec::new() }
    }

    #[test]
    fn headings_follow_the_newest_save() {
        let now = day(2026, 10, 31);
        assert_eq!(when_group(day(2026, 10, 28), now), "This week");
        assert_eq!(when_group(day(2026, 10, 2), now), "This month");
        assert_eq!(when_group(day(2026, 3, 1), now), "Earlier in 2026");
        assert_eq!(when_group(day(2019, 3, 1), now), "2019");

        let s = songs(&[
            save("Didums_77", day(2026, 10, 30)),
            save("Night", day(2026, 3, 1)),
            save("Didums", day(2019, 1, 1)),
            save("Old", day(2019, 1, 1)),
            save("Older", day(2017, 1, 1)),
        ]);
        let v = s.view(&Filters::default(), now);
        let lines: Vec<String> = v
            .entries
            .iter()
            .map(|e| match e {
                Entry::Heading(h) => format!("# {h}"),
                Entry::Song(i) => s.rows[*i].song.name.clone(),
            })
            .collect();
        assert_eq!(lines, ["# This week", "Didums", "# Earlier in 2026", "Night", "# 2019", "Old", "# 2017", "Older"]);
        assert_eq!(v.years, [(2026, 0), (2019, 4), (2017, 6)]);
        assert_eq!(v.count, "4 songs (5 saves)");
    }

    #[test]
    fn search_finds_words_in_any_order_a_year_and_a_whole_key() {
        let mut dark = save("Dark Matter", day(2019, 5, 1));
        dark.plugin_names = vec!["Serum".into()];
        dark.musical_key = "G minor".into();
        let s = songs(&[dark, save("Sunrise", day(2024, 1, 1))]);
        let found = |text: &str| -> Vec<String> {
            let f = Filters { search: text.into(), ..Filters::default() };
            s.rows.iter().filter(|r| f.allows(r)).map(|r| r.song.name.clone()).collect()
        };
        assert_eq!(found("serum dark"), ["Dark Matter"]);
        assert_eq!(found("MATTER"), ["Dark Matter"]);
        assert_eq!(found("2019"), ["Dark Matter"]);
        assert_eq!(found("g minor"), ["Dark Matter"]);
        assert!(found("minor").is_empty());
        assert_eq!(found("").len(), 2);
        assert_eq!(found("beats").len(), 2);
    }

    #[test]
    fn song_fields_come_from_the_newest_save_that_has_them() {
        let newest = save("Didums_3", day(2025, 3, 1));
        let older = Save {
            rating: 4,
            notes: "needs a drop".into(),
            status: "Mixing".into(),
            ..save("Didums_2", day(2025, 2, 1))
        };
        let oldest = Save { rating: 2, favourite: true, ..save("Didums", day(2025, 1, 1)) };
        let s = songs(&[newest, older, oldest]);
        let r = &s.rows[0];
        assert_eq!((r.rating, r.notes.as_str(), r.status.as_str(), r.favourite), (4, "needs a drop", "Mixing", true));
    }

    #[test]
    fn filters_narrow_the_list() {
        let a = Save { favourite: true, ..save("Alpha", day(2025, 1, 1)) };
        let b = Save { status: "Archived".into(), missing_samples: 2, ..save("Bravo", day(2025, 1, 2)) };
        let c = Save { daw: "Bitwig Studio".into(), tags: vec!["dark".into()], ..save("Charlie", day(2025, 1, 3)) };
        let s = songs(&[c, b, a]);
        let names = |f: Filters| -> Vec<String> {
            s.view(&f, day(2026, 1, 1))
                .entries
                .iter()
                .filter_map(|e| if let Entry::Song(i) = e { Some(s.rows[*i].song.name.clone()) } else { None })
                .collect()
        };
        assert_eq!(names(Filters { favourites: true, ..Filters::default() }), ["Alpha"]);
        assert_eq!(names(Filters { missing: true, ..Filters::default() }), ["Bravo"]);
        assert_eq!(names(Filters { daw: Some("Bitwig Studio".into()), ..Filters::default() }), ["Charlie"]);
        assert_eq!(names(Filters { tag: Some("dark".into()), ..Filters::default() }), ["Charlie"]);
        assert_eq!(names(Filters { hide_archived: true, ..Filters::default() }), ["Charlie", "Alpha"]);
        assert_eq!(
            names(Filters { hide_archived: true, status: Some("Archived".into()), ..Filters::default() }),
            ["Bravo"]
        );
        assert_eq!(s.view(&Filters { favourites: true, ..Filters::default() }, day(2026, 1, 1)).count, "1 of 3 songs");
        assert!(!Filters { hide_archived: true, ..Filters::default() }.any());
        assert!(Filters { search: "x".into(), ..Filters::default() }.any());
        assert_eq!(s.daws(), ["Bitwig Studio", "FL Studio"]);
    }

    #[test]
    fn row_text() {
        let mut d = save("Didums_77", day(2024, 8, 3));
        d.tempo = Some(140.0);
        d.detected_key = "G minor".into();
        d.missing_plugins = 1;
        d.missing_samples = 3;
        let s = songs(&[d, save("Didums", day(2019, 1, 10))]);
        let r = &s.rows[0];
        assert_eq!(r.details_line(), "FL Studio · 140 BPM · G minor · 2 saves");
        assert_eq!(r.missing_text(), "1 plugin, 3 samples");
        assert_eq!(r.short_date(day(2024, 12, 1)), "3 Aug");
        assert_eq!(r.short_date(day(2026, 1, 1)), "3 Aug 2024");
        assert_eq!(r.date_tip(), "Last saved 3 Aug 2024. First made 10 Jan 2019.");
        assert_eq!(r.folder_name(), "beats");
        assert_eq!(
            (tempo_text(92.5), tempo_text(128.25), thousands(16000), thousands(999)),
            ("92.5".into(), "128.25".into(), "16,000".into(), "999".into())
        );
    }
}
