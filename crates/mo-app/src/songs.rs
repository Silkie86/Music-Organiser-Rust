//! The Songs page's list: one row per song (its saves grouped as the C# app groups them), newest save first, under
//! "This week", "This month", "Earlier in 2026" and one heading per year, with search and filter chips. Agreed with
//! Solomon on 2026-10-31 (no Elektron box chip).
//!
//! Reading the library and grouping happen once per load, off the window's thread; searching and filtering then only
//! walk the rows already made, so typing in the search box stays quick over 16,000 songs (rule 12).

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{Datelike, Days, NaiveDateTime};
use mo_formats::case;
use mo_library::songs::{self, SaveRun, Song};
use mo_library::{STATUSES, Save, SongChoices};

use crate::filters::{self as chips, Filter};

/// The status the Hide archived chip hides.
pub const ARCHIVED: &str = STATUSES[6];

/// The filters' names, as the page calls them when a choice is picked.
pub const DAW: &str = "daw";
pub const STATUS: &str = "status";
pub const TAG: &str = "tag";

/// One song in the list, with the fields that belong to the song (rule 14).
#[derive(Clone, Debug)]
pub struct SongRow {
    pub song: Song,
    /// When it was last saved: its newest save's date. The list is in this order and its headings and dates use it.
    /// It isn't always `song.latest`'s date: the save with the highest number is the song's latest even when an
    /// older-numbered one was saved after it.
    pub saved: NaiveDateTime,
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
        let saved = song.saves().map(|s| s.modified).max().unwrap_or(song.latest.modified);
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
            saved,
            made,
            runs,
            status,
            rating,
            favourite: latest.favourite || earlier.iter().any(|s| s.favourite),
            musical_key: pick(&latest.musical_key, |s| &s.musical_key),
            notes: pick(&latest.notes, |s| &s.notes),
            tags: if latest.tags.is_empty() {
                earlier.iter().find(|s| !s.tags.is_empty()).map_or_else(Vec::new, |s| s.tags.clone())
            } else {
                latest.tags.clone()
            },
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
        self.runs.last().filter(|r| r.reworked)
    }

    /// When it was last saved: "3 Aug" this year, "3 Aug 2019" before.
    pub fn short_date(&self, now: NaiveDateTime) -> String {
        let saved = self.saved;
        if saved.year() == now.year() {
            saved.format("%-d %b").to_string()
        } else {
            saved.format("%-d %b %Y").to_string()
        }
    }

    /// The date's tooltip: when it was last saved, and when it was first made if that's another day.
    pub fn date_tip(&self) -> String {
        let saved = self.saved;
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

    /// Whether the search finds this song. Several words must each match something, in any order, so "serum dark"
    /// finds a song using Serum with "dark" in its name or notes; a year such as 2019 also finds songs saved or made
    /// that year; a key ("G minor") must match whole.
    pub fn matches(&self, search: &Search) -> bool {
        let Some(whole) = &search.whole else { return true };
        self.matches_word(whole) || (search.words.len() > 1 && search.words.iter().all(|w| self.matches_word(w)))
    }

    fn matches_word(&self, word: &Word) -> bool {
        self.find.contains(&word.key)
            || case::eq_ignore_case(self.key(), &word.text)
            || word.year.is_some_and(|y| y == self.saved.year() || y == self.made.year())
    }
}

/// The search box's text, made ready once for every row it is tried on.
#[derive(Clone, Debug, Default)]
pub struct Search {
    /// All of it, when there is any.
    whole: Option<Word>,
    /// Each word on its own.
    words: Vec<Word>,
}

#[derive(Clone, Debug)]
struct Word {
    text: String,
    /// Upper case, as the rows' search text is.
    key: String,
    /// The year it names, when it's four digits.
    year: Option<i32>,
}

impl Word {
    fn new(text: &str) -> Word {
        let year = if text.len() == 4 { text.parse().ok() } else { None };
        Word { text: text.to_owned(), key: case::key(text), year }
    }
}

impl Search {
    pub fn new(text: &str) -> Search {
        let text = text.trim();
        if text.is_empty() {
            return Search::default();
        }
        Search {
            whole: Some(Word::new(text)),
            words: text.split(' ').filter(|w| !w.is_empty()).map(Word::new).collect(),
        }
    }
}

/// "140", "92.5", "128.25": a tempo with at most two decimals and none it doesn't need.
pub fn tempo_text(t: f64) -> String {
    let s = format!("{t:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// "1 song", "16,000 songs".
pub fn plural(n: usize, what: &str) -> String {
    if n == 1 { format!("1 {what}") } else { format!("{} {what}s", thousands(n)) }
}

pub use mo_library::songs::thousands;

/// The list's time headings: "This week", "This month", "Earlier in 2026", then one per year.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    ThisWeek,
    ThisMonth,
    EarlierIn(i32),
    Year(i32),
}

impl When {
    /// The heading a song last saved on `saved` goes under.
    pub fn of(saved: NaiveDateTime, now: NaiveDateTime) -> When {
        let week_ago = now.date().checked_sub_days(Days::new(7)).unwrap_or(now.date());
        if saved.date() > week_ago {
            When::ThisWeek
        } else if saved.year() == now.year() && saved.month() == now.month() {
            When::ThisMonth
        } else if saved.year() == now.year() {
            When::EarlierIn(now.year())
        } else {
            When::Year(saved.year())
        }
    }
}

impl std::fmt::Display for When {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            When::ThisWeek => f.write_str("This week"),
            When::ThisMonth => f.write_str("This month"),
            When::EarlierIn(y) => write!(f, "Earlier in {y}"),
            When::Year(y) => write!(f, "{y}"),
        }
    }
}

/// Everything the page reads from the library in one go, on a background thread.
#[derive(Clone, Debug, Default)]
pub struct Songs {
    /// Last saved first.
    pub rows: Vec<SongRow>,
    /// How many saves the rows hold, leaving out the same file found again elsewhere.
    pub saves: usize,
    /// How many saves are the same file found again elsewhere (a copied project folder): hidden, never deleted.
    pub copies: usize,
    /// The tags in use, for the Tag chip.
    pub tags: Vec<String>,
    /// The program picked in Settings for each DAW that has one (the C# app's `daw_exe:` settings), for Open.
    pub daw_exes: HashMap<String, String>,
    /// Songs that may be saves of one, for the Health strip, newest first.
    pub doubts: Vec<Doubt>,
}

/// Two songs that may be saves of one: the app asks rather than guessing (the C# app's question).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doubt {
    /// The song asked about, by its place in [`Songs::rows`].
    pub a: usize,
    pub other: Other,
    pub why: String,
}

/// What a [`Doubt`] asks `a` about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Other {
    /// Another song, by its place in [`Songs::rows`].
    Song(usize),
    /// One save inside `a` that a mark put there ("final", "v2", a date): its name.
    Save(String),
}

/// The settings that turn the questions off, or on across folders: the C# app's.
pub const ASK_DOUBTS: &str = "ask_song_doubts";
pub const DOUBTS_ACROSS_FOLDERS: &str = "song_doubts_across_folders";

/// At most this many questions. The C# app stopped at 200, which in a big library left songs with no question; the
/// questions are only places and a line each, so far more fit without a cost.
const MOST_DOUBTS: usize = 5000;

impl Songs {
    /// Groups the saves into songs and puts them in order, last saved first. `asking` is whether to look for songs
    /// that may be saves of one (`Some`), and then whether also across folders.
    pub fn from_saves(
        saves: &[Save],
        choices: &SongChoices,
        tags: Vec<String>,
        gap_months: u32,
        asking: Option<bool>,
    ) -> Songs {
        let groups = songs::group_songs(saves, choices);
        let found = asking.map(|across| songs::doubt_pairs(&groups, choices, across, MOST_DOUBTS)).unwrap_or_default();
        let mut rows: Vec<(usize, SongRow)> =
            groups.into_iter().map(|g| SongRow::new(g, gap_months)).enumerate().collect();
        // Stable, so songs saved at the same moment keep the order they were grouped in.
        rows.sort_by_key(|(_, r)| std::cmp::Reverse(r.saved));
        // Where each group ended up in the list.
        let mut at = vec![0; rows.len()];
        for (place, (group, _)) in rows.iter().enumerate() {
            if let Some(a) = at.get_mut(*group) {
                *a = place;
            }
        }
        let rows: Vec<SongRow> = rows.into_iter().map(|(_, r)| r).collect();
        let doubts = found
            .into_iter()
            .filter_map(|(pair, why)| match pair {
                songs::Pair::Songs(a, b) => Some(Doubt { a: *at.get(a)?, other: Other::Song(*at.get(b)?), why }),
                songs::Pair::Save(a, save) => {
                    let row = rows.get(*at.get(a)?)?;
                    let name = row.song.saves().nth(save)?.name.clone();
                    Some(Doubt { a: *at.get(a)?, other: Other::Save(name), why })
                }
            })
            .collect();
        let saves = rows.iter().map(|r| r.song.save_count()).sum();
        let copies = rows.iter().map(|r| r.song.copies.len()).sum();
        Songs { rows, saves, copies, tags, daw_exes: HashMap::new(), doubts }
    }

    /// The questions about the song at `row`.
    pub fn doubts_of(&self, row: usize) -> impl Iterator<Item = &Doubt> {
        self.doubts.iter().filter(move |d| d.a == row || d.other == Other::Song(row))
    }

    /// The DAWs the songs come from, for the DAW chip, in order.
    pub fn daws(&self) -> Vec<String> {
        let set: BTreeSet<&str> = self.rows.iter().map(|r| r.song.latest.daw.as_str()).collect();
        set.into_iter().map(str::to_owned).collect()
    }

    /// How many songs were last saved in each year, newest first, for the year strip (every song, whatever the
    /// filters, so the strip doesn't shrink to the years picked).
    pub fn year_counts(&self) -> Vec<(i32, usize)> {
        let mut counts: std::collections::BTreeMap<i32, usize> = std::collections::BTreeMap::new();
        for r in &self.rows {
            *counts.entry(r.saved.year()).or_default() += 1;
        }
        counts.into_iter().rev().collect()
    }

    /// The rows the filters let through, under their headings, with where each year starts; and how many songs each
    /// DAW, status and tag would show with the other filters as they are, counted in the same pass (a song counts
    /// beside every chip when all three let it through, and beside the one that doesn't when only one doesn't).
    pub fn view(&self, filters: &Filters, now: NaiveDateTime) -> View {
        let mut view = View::default();
        let mut heading = None;
        let mut shown = 0;
        let search = Search::new(&filters.search);
        let mut daws: HashMap<&str, usize> = HashMap::new();
        let mut tags: HashMap<&str, usize> = HashMap::new();
        for (i, row) in self.rows.iter().enumerate().filter(|(_, r)| filters.passes_others(r, &search)) {
            let ok = filters.chips_pass(row);
            let fails = ok.iter().filter(|&&o| !o).count();
            let counts_on = |chip: usize| fails == 0 || (fails == 1 && !ok[chip]);
            if counts_on(0) {
                *daws.entry(row.song.latest.daw.as_str()).or_default() += 1;
            }
            if counts_on(1)
                && let Some(at) = STATUSES.iter().position(|s| *s == row.status)
            {
                view.counts.statuses[at] += 1;
            }
            if counts_on(2) {
                for t in &row.tags {
                    *tags.entry(t.as_str()).or_default() += 1;
                }
            }
            if fails > 0 {
                continue;
            }
            let h = When::of(row.saved, now);
            if heading != Some(h) {
                view.entries.push(Entry::Heading(h));
                heading = Some(h);
            }
            view.entries.push(Entry::Song(i));
            shown += 1;
        }
        for (daw, n) in daws {
            *view.counts.daws.entry(daw.to_ascii_lowercase()).or_default() += n;
        }
        for (tag, n) in tags {
            *view.counts.tags.entry(tag.to_ascii_lowercase()).or_default() += n;
        }
        let all = self.rows.len();
        let mut count =
            if shown == all { plural(all, "song") } else { format!("{} of {}", thousands(shown), plural(all, "song")) };
        // The C# app counts a copied folder's files again: saying how many are hidden keeps the two counts adding up.
        match (self.saves > all, self.copies) {
            (true, 0) => count.push_str(&format!(" ({} saves)", thousands(self.saves))),
            (true, c) => {
                count.push_str(&format!(" ({} saves, {} hidden as copies)", thousands(self.saves), thousands(c)))
            }
            (false, 0) => {}
            (false, c) => count.push_str(&format!(" ({} hidden as copies)", thousands(c))),
        }
        view.count = count;
        view.shown = shown;
        view
    }
}

/// One line of the list: a heading, or a song (its place in [`Songs::rows`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Heading(When),
    Song(usize),
}

/// What the list shows.
#[derive(Clone, Debug, Default)]
pub struct View {
    pub entries: Vec<Entry>,
    /// How many songs are shown.
    pub shown: usize,
    /// "1,234 of 16,000 songs (52,000 saves)".
    pub count: String,
    /// The numbers beside the filters' choices.
    pub counts: SongCounts,
}

/// How many songs each DAW, status and tag would show, with the other filters as they are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SongCounts {
    /// By DAW, its name in lower case (ASCII letters only, as the filter matches).
    pub daws: HashMap<String, usize>,
    /// By status, in [`STATUSES`]' order.
    pub statuses: [usize; STATUSES.len()],
    /// By tag, in lower case (ASCII letters only).
    pub tags: HashMap<String, usize>,
}

/// The page's filters as their chips and menus show them: the DAW, the status and the tag, each choice with how many
/// songs it would show. More than one of each can be picked; a short list is chips of its own, a long one (the tags,
/// often) a menu that scrolls.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SongChips {
    pub daw: Filter,
    pub status: Filter,
    pub tag: Filter,
}

impl Songs {
    pub fn chips(&self, f: &Filters, counts: &SongCounts) -> SongChips {
        let n = |map: &HashMap<String, usize>, name: &str| {
            thousands(map.get(&name.to_ascii_lowercase()).copied().unwrap_or(0))
        };
        let daws: Vec<(String, String)> = self
            .daws()
            .into_iter()
            .map(|d| {
                let c = n(&counts.daws, &d);
                (d, c)
            })
            .collect();
        let statuses: Vec<(String, String)> =
            STATUSES.iter().zip(counts.statuses).map(|(s, c)| ((*s).to_owned(), thousands(c))).collect();
        let tags: Vec<(String, String)> = self.tags.iter().map(|t| (t.clone(), n(&counts.tags, t))).collect();
        SongChips {
            daw: chips::show(&daws, &f.daws),
            status: chips::show(&statuses, &f.statuses),
            tag: chips::show(&tags, &f.tags),
        }
    }
}

/// The search box and filter chips. They start empty each time the app opens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filters {
    pub search: String,
    /// Only songs last saved in one of these DAWs; every DAW when none is picked.
    pub daws: Vec<String>,
    /// Only songs with one of these statuses; when none is picked, every status (but archived ones when Hide archived
    /// is on).
    pub statuses: Vec<String>,
    /// Only songs with one of these tags.
    pub tags: Vec<String>,
    pub favourites: bool,
    /// Only songs missing a plugin, a sample or their file.
    pub missing: bool,
    /// Archived songs are left out unless their status is picked.
    pub hide_archived: bool,
    /// Only songs last saved in these years, first to last (the year strip).
    pub years: Option<(i32, i32)>,
    /// Only the songs with a save among these, and what the chip calls them ("Using Serum"): the Plugins page's
    /// Show in Songs.
    pub only: Option<(String, HashSet<i64>)>,
}

impl Filters {
    /// Whether the song gets through; `search` is [`Search::new`] of this `search` text, made once for every row.
    pub fn allows(&self, r: &SongRow, search: &Search) -> bool {
        self.chips_pass(r) == [true; 3] && self.passes_others(r, search)
    }

    /// Which of the three chips let the song through: its DAW, its status (or Hide archived, when no status is
    /// picked) and its tags.
    fn chips_pass(&self, r: &SongRow) -> [bool; 3] {
        let daw = &r.song.latest.daw;
        [
            self.daws.is_empty() || self.daws.iter().any(|d| d.eq_ignore_ascii_case(daw)),
            if self.statuses.is_empty() {
                !(self.hide_archived && r.status == ARCHIVED)
            } else {
                self.statuses.iter().any(|s| s.eq_ignore_ascii_case(&r.status))
            },
            self.tags.is_empty() || self.tags.iter().any(|t| r.tags.iter().any(|own| own.eq_ignore_ascii_case(t))),
        ]
    }

    /// Picks or unpicks `choice` (as its chip or menu line says it) on the filter `chip` ([`DAW`], [`STATUS`],
    /// [`TAG`]): more than one of each can be picked.
    pub fn toggle(&mut self, chip: &str, choice: &str) {
        match chip {
            DAW => chips::toggle(&mut self.daws, choice, true),
            STATUS => chips::toggle(&mut self.statuses, choice, true),
            TAG => chips::toggle(&mut self.tags, choice, true),
            _ => {}
        }
    }

    /// Picks nothing on the filter `chip`: its menu's Any.
    pub fn clear_chip(&mut self, chip: &str) {
        match chip {
            DAW => self.daws.clear(),
            STATUS => self.statuses.clear(),
            TAG => self.tags.clear(),
            _ => {}
        }
    }

    /// Everything but the three chips lets the song through: favourites, missing, the years, the Plugins page's
    /// songs and the search (last, as it's the slow one).
    fn passes_others(&self, r: &SongRow, search: &Search) -> bool {
        if self.favourites && !r.favourite {
            return false;
        }
        if self.missing && !r.has_missing() {
            return false;
        }
        if let Some((first, last)) = self.years
            && !(first..=last).contains(&r.saved.year())
        {
            return false;
        }
        if let Some((_, saves)) = &self.only
            && !r.song.saves().chain(&r.song.copies).any(|s| saves.contains(&s.id))
        {
            return false;
        }
        r.matches(search)
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
        Songs::from_saves(saves, &SongChoices::default(), Vec::new(), 3, Some(false))
    }

    #[test]
    fn headings_follow_the_newest_save() {
        let now = day(2026, 10, 31);
        assert_eq!(When::of(day(2026, 10, 28), now).to_string(), "This week");
        assert_eq!(When::of(day(2026, 10, 2), now).to_string(), "This month");
        assert_eq!(When::of(day(2026, 3, 1), now).to_string(), "Earlier in 2026");
        assert_eq!(When::of(day(2019, 3, 1), now).to_string(), "2019");

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
        assert_eq!(v.count, "4 songs (5 saves)");
        let copied = Songs { copies: 2, ..s.clone() };
        assert_eq!(copied.view(&Filters::default(), now).count, "4 songs (5 saves, 2 hidden as copies)");
    }

    #[test]
    fn the_year_strip_counts_every_song_and_picks_a_span() {
        let now = day(2026, 10, 31);
        let s = songs(&[
            save("Alpha", day(2026, 6, 1)),
            save("Bravo", day(2024, 5, 1)),
            save("Charlie", day(2023, 4, 1)),
            save("Delta", day(2023, 1, 1)),
            save("Echo", day(2019, 1, 1)),
        ]);
        assert_eq!(s.year_counts(), [(2026, 1), (2024, 1), (2023, 2), (2019, 1)]);
        let picked = Filters { years: Some((2023, 2024)), ..Filters::default() };
        assert!(picked.any());
        let v = s.view(&picked, now);
        let names: Vec<&str> = v
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Song(i) => Some(s.rows[*i].song.name.as_str()),
                Entry::Heading(_) => None,
            })
            .collect();
        assert_eq!(names, ["Bravo", "Charlie", "Delta"]);
    }

    #[test]
    fn show_in_songs_keeps_the_songs_with_one_of_its_saves() {
        let now = day(2026, 10, 31);
        let mut saves =
            [save("Alpha", day(2026, 6, 1)), save("Bravo_2", day(2024, 5, 1)), save("Bravo", day(2023, 4, 1))];
        for (i, s) in saves.iter_mut().enumerate() {
            s.id = i as i64 + 1;
        }
        let s = songs(&saves);
        // Only Bravo's older save uses the plugin: the song shows.
        let only = Filters { only: Some(("Using Serum".into(), HashSet::from([3]))), ..Filters::default() };
        assert!(only.any());
        let v = s.view(&only, now);
        let names: Vec<&str> = v
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Song(i) => Some(s.rows[*i].song.name.as_str()),
                Entry::Heading(_) => None,
            })
            .collect();
        assert_eq!(names, ["Bravo"]);
    }

    #[test]
    fn a_song_goes_by_its_last_saved_date_whichever_save_that_is() {
        let now = day(2026, 10, 31);
        // Didums_5 was opened and saved again this year, after Didums_77 was last touched.
        let s = songs(&[
            save("Alpha", day(2026, 6, 1)),
            save("Didums_5", day(2026, 5, 1)),
            save("Charlie", day(2026, 4, 1)),
            save("Didums_77", day(2019, 1, 1)),
        ]);
        let v = s.view(&Filters::default(), now);
        let names: Vec<String> = v
            .entries
            .iter()
            .map(|e| match e {
                Entry::Heading(h) => format!("# {h}"),
                Entry::Song(i) => s.rows[*i].song.name.clone(),
            })
            .collect();
        assert_eq!(names, ["# Earlier in 2026", "Alpha", "Didums", "Charlie"]);
        assert_eq!(s.rows[1].short_date(now), "1 May");
    }

    #[test]
    fn tags_come_from_the_newest_save_that_has_them() {
        let older = Save { tags: vec!["dark".into()], ..save("Didums_76", day(2025, 1, 1)) };
        let s = songs(&[save("Didums_77", day(2025, 2, 1)), older]);
        let f = Filters { tags: vec!["dark".into()], ..Filters::default() };
        assert_eq!(s.view(&f, day(2026, 1, 1)).shown, 1);
        assert_eq!(s.view(&Filters { search: "dark".into(), ..Filters::default() }, day(2026, 1, 1)).shown, 1);
    }

    #[test]
    fn search_finds_words_in_any_order_a_year_and_a_whole_key() {
        let mut dark = save("Dark Matter", day(2019, 5, 1));
        dark.plugin_names = vec!["Serum".into()];
        dark.musical_key = "G minor".into();
        let s = songs(&[dark, save("Sunrise", day(2024, 1, 1))]);
        let found = |text: &str| -> Vec<String> {
            let f = Filters { search: text.into(), ..Filters::default() };
            s.rows.iter().filter(|r| f.allows(r, &Search::new(text))).map(|r| r.song.name.clone()).collect()
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
        assert_eq!(names(Filters { daws: vec!["Bitwig Studio".into()], ..Filters::default() }), ["Charlie"]);
        assert_eq!(names(Filters { tags: vec!["dark".into()], ..Filters::default() }), ["Charlie"]);
        assert_eq!(names(Filters { hide_archived: true, ..Filters::default() }), ["Charlie", "Alpha"]);
        assert_eq!(
            names(Filters { hide_archived: true, statuses: vec!["Archived".into()], ..Filters::default() }),
            ["Bravo"]
        );
        assert_eq!(s.view(&Filters { favourites: true, ..Filters::default() }, day(2026, 1, 1)).count, "1 of 3 songs");
        assert!(!Filters { hide_archived: true, ..Filters::default() }.any());
        assert!(Filters { search: "x".into(), ..Filters::default() }.any());
        assert_eq!(s.daws(), ["Bitwig Studio", "FL Studio"]);

        // More than one of a chip: any of them. Picking a chip again unpicks it.
        let mut f = Filters::default();
        f.toggle(STATUS, "Archived");
        f.toggle(STATUS, "Idea");
        assert_eq!(names(f.clone()), ["Charlie", "Bravo", "Alpha"]);
        f.toggle(STATUS, "idea");
        assert_eq!(names(f.clone()), ["Bravo"]);
        f.clear_chip(STATUS);
        f.toggle(DAW, "FL Studio");
        f.toggle(DAW, "Bitwig Studio");
        assert_eq!(names(f.clone()).len(), 3);
        // Each choice counts the songs it would show with the other filters as they are: with FL Studio picked, the
        // DAW counts leave it out, and the statuses count only FL songs.
        let fl = Filters { daws: vec!["FL Studio".into()], hide_archived: true, ..Filters::default() };
        let view = s.view(&fl, day(2026, 1, 1));
        assert_eq!(view.shown, 1);
        assert_eq!((view.counts.daws.get("fl studio"), view.counts.daws.get("bitwig studio")), (Some(&1), Some(&1)));
        let at = |status: &str| STATUSES.iter().position(|s| *s == status).unwrap();
        // Archived counts beside its own chip although Hide archived hides it: picking it would show it.
        assert_eq!((view.counts.statuses[at("Idea")], view.counts.statuses[at("Archived")]), (1, 1));
        assert!(view.counts.tags.is_empty());
        let chips = s.chips(&fl, &view.counts);
        let daws: Vec<(&str, &str, bool)> =
            chips.daw.chips.iter().map(|c| (c.text.as_str(), c.count.as_str(), c.on)).collect();
        assert_eq!(daws, [("Bitwig Studio", "1", false), ("FL Studio", "1", true)]);
        assert_eq!(chips.status.chips.len(), STATUSES.len());
        // Charlie's tag: none of the FL songs has it, and it counts once with no DAW picked.
        let tagged = Songs { tags: vec!["dark".into()], ..s.clone() };
        let tag_count =
            |f: &Filters| tagged.chips(f, &tagged.view(f, day(2026, 1, 1)).counts).tag.chips[0].count.clone();
        assert_eq!((tag_count(&fl), tag_count(&Filters::default())), ("0".to_owned(), "1".to_owned()));
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
        assert_eq!(
            (tempo_text(92.5), tempo_text(128.25), thousands(16000), thousands(999)),
            ("92.5".into(), "128.25".into(), "16,000".into(), "999".into())
        );
    }
}
