//! The MIDI page's screen logic and every word it shows: which parts the filters let through and in what order, each
//! line's words, the picked part's card (its key as the key finder reads it, its facts and its notes for the piano
//! roll), and the page's summary. No window in it, so all of it is tested here.
//!
//! The parts come from `mo_midi::Catalogue` (one entry per set of notes, the newest copy, "in 20 files"); the slow
//! work (reading the index, working out a list, reading a part's notes) is the page's to run off the window's thread.
//! Browse only: the Playground app (editing) comes later, and so do the GM sound, fitting to a song, Send to Bitwig
//! and drag out.

use std::collections::HashSet;

use chrono::{Local, TimeZone};
use mo_formats::NoteEvent;
use mo_formats::dates::RealDates;
use mo_harmony::keyfind::Why;
use mo_harmony::{Key as KeyName, KeyAnswer, KeyFact, Mode};
use mo_library::songs::thousands;
use mo_midi::{BAR, Catalogue, Entry, From, Hit, Query, Role};
use rayon::prelude::*;

use crate::filters::{self, Filter};

/// The orders the list can take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    /// The newest file first, as the catalogue keeps them; a date that isn't real (still to come, or before 1981)
    /// last, either way round ([`RealDates`]).
    #[default]
    Newest,
    Name,
    /// By key round the circle from C, major before minor; parts with no clear key last.
    Key,
    /// The longest first.
    Bars,
    /// The ones in the most files first: what you came back to most.
    Copies,
}

impl Order {
    pub const ALL: [Order; 5] = [Order::Newest, Order::Name, Order::Key, Order::Bars, Order::Copies];

    /// The list column this order goes by, as its heading says it: a click on the heading picks the order, a second
    /// click turns it round (the UX review of 10 Nov, B5).
    pub fn column(self) -> &'static str {
        match self {
            Order::Newest => "Saved",
            Order::Name => "Part",
            Order::Key => "Key",
            Order::Bars => "Bars",
            Order::Copies => "Files",
        }
    }

    /// The order as the line over the list says it: "newest first", or turned round, "oldest first".
    pub fn words(self, reversed: bool) -> &'static str {
        match (self, reversed) {
            (Order::Newest, false) => "newest first",
            (Order::Newest, true) => "oldest first",
            (Order::Name, false) => "by name",
            (Order::Name, true) => "by name, Z to A",
            (Order::Key, false) => "by key",
            (Order::Key, true) => "by key, backwards",
            (Order::Bars, false) => "longest first",
            (Order::Bars, true) => "shortest first",
            (Order::Copies, false) => "in most files first",
            (Order::Copies, true) => "in fewest files first",
        }
    }

    pub fn from_column(column: &str) -> Order {
        Order::ALL.into_iter().find(|o| o.column() == column).unwrap_or_default()
    }
}

/// Where the parts come from, as its chip names it: your projects' parts, or the MIDI folders' files. Neither picked
/// is both, as on the other pages.
pub fn from_name(from: From) -> &'static str {
    match from {
        From::Projects => "Your projects",
        From::Files => "MIDI folders",
    }
}

/// The From chips, in the order they show.
pub const FROM_CHOICES: [From; 2] = [From::Projects, From::Files];

/// The choice a From chip's name stands for.
pub fn from_of_name(name: &str) -> Option<From> {
    FROM_CHOICES.into_iter().find(|f| from_name(*f) == name)
}

/// The filters' names, as the page calls them when one is picked.
pub const KIND: &str = "kind";
pub const FROM: &str = "from";
pub const KEY: &str = "key";

/// What the page shows: the filters and what the list is ranked against.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    /// Words that must all appear (names, folders, kind, instrument, key).
    pub text: String,
    /// Only these kinds of part (none picked: every kind).
    pub roles: Vec<Role>,
    /// Only your projects' parts, or only the MIDI folders' files (none: both).
    pub from: Option<From>,
    /// Only parts the key finder puts in one of these keys (one of a pair counts too, and so does a home note's
    /// tonic); none picked: any key.
    pub keys: Vec<KeyName>,
    /// Only parts from files saved in these years, first to last.
    pub years: Option<(i32, i32)>,
    /// Only parts of at least this many notes (Settings › MIDI, [`SMALLEST_PART_KEY`]); 0 or 1: every part. Not one
    /// of the filters you pick on the page, so [`Filters::any`] leaves it out.
    pub smallest: usize,
}

impl Filters {
    /// Picks or unpicks `choice` (as its chip or menu line says it) on the filter `name` ([`KIND`], [`FROM`],
    /// [`KEY`]). More than one kind or key can be picked; From is one or none (none is both).
    pub fn toggle(&mut self, name: &str, choice: &str) {
        match name {
            KIND => filters::toggle_as(&mut self.roles, choice, true, Role::plural, role_of_plural),
            FROM => {
                let mut picked: Vec<From> = self.from.into_iter().collect();
                filters::toggle_as(&mut picked, choice, false, from_name, from_of_name);
                self.from = picked.first().copied();
            }
            KEY => {
                let mut words: Vec<String> = self.keys.iter().map(|k| key_words(*k)).collect();
                filters::toggle(&mut words, choice, true);
                self.keys = words.iter().filter_map(|w| key_of_choice(w)).collect();
            }
            _ => {}
        }
    }

    /// Picks nothing on the filter `name`: its menu's Any.
    pub fn clear(&mut self, name: &str) {
        match name {
            KIND => self.roles.clear(),
            FROM => self.from = None,
            KEY => self.keys.clear(),
            _ => {}
        }
    }

    /// Whether anything narrows the list.
    pub fn any(&self) -> bool {
        !self.text.trim().is_empty()
            || !self.roles.is_empty()
            || self.from.is_some()
            || !self.keys.is_empty()
            || self.years.is_some()
    }
}

/// What the list is ranked against instead of newest first: parts shaped like one, or the longer phrases one is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// "More like this": as a place in the catalogue's entries.
    Like(usize),
    /// "Show the whole phrase": the longer phrases this part is in.
    Within(usize),
}

/// The parts the search and the target let through, before the chips: what the chips' counts are of.
fn searched(cat: &Catalogue, f: &Filters, target: Option<Target>) -> Vec<Hit> {
    let q = Query {
        text: f.text.clone(),
        like: match target {
            Some(Target::Like(at)) => Some(at),
            _ => None,
        },
        within: match target {
            Some(Target::Within(at)) => Some(at),
            _ => None,
        },
        limit: usize::MAX,
        ..Query::default()
    };
    let mut hits = cat.search(&q).0;
    if f.smallest > 1 {
        hits.retain(|h| cat.entry(h.entry).is_some_and(|e| big_enough(e, f.smallest)));
    }
    hits
}

/// Settings' smallest MIDI part: a part with fewer notes than this is hidden on the MIDI page. This app's own (the C#
/// app always dropped parts under 3 notes).
pub const SMALLEST_PART_KEY: &str = "midi_smallest_part";
/// The smallest part when Settings hasn't picked one: the C# app's limit.
pub const SMALLEST_PART_DEFAULT: usize = 3;
/// What Settings offers, in notes.
pub const SMALLEST_PART_CHOICES: [usize; 6] = [1, 2, 3, 4, 6, 8];

/// Where a smallest part sits in [`SMALLEST_PART_CHOICES`] (the default's place for any other).
pub fn smallest_part_index(n: usize) -> usize {
    let at = |n| SMALLEST_PART_CHOICES.iter().position(|&c| c == n);
    at(n).or_else(|| at(SMALLEST_PART_DEFAULT)).unwrap_or(0)
}

/// The smallest part as kept in the settings: one of [`SMALLEST_PART_CHOICES`], else the default.
pub fn smallest_part_from(kept: Option<&str>) -> usize {
    kept.and_then(|k| k.trim().parse::<usize>().ok())
        .filter(|n| SMALLEST_PART_CHOICES.contains(n))
        .unwrap_or(SMALLEST_PART_DEFAULT)
}

/// Settings' words for a smallest part: "Every part", "2 notes".
pub fn smallest_part_words(n: usize) -> String {
    if n <= 1 { "Every part".to_owned() } else { format!("{n} notes") }
}

/// Whether a part has at least `smallest` notes.
pub fn big_enough(e: &Entry, smallest: usize) -> bool {
    usize::try_from(e.phrase.notes).unwrap_or(0) >= smallest
}

/// How many lines the whole list has with parts under `smallest` notes hidden (the same loop in other keys is one
/// line; those all have the same notes).
pub fn line_count(cat: &Catalogue, smallest: usize) -> usize {
    if smallest <= 1 {
        return cat.line_count;
    }
    (0..cat.entries.len())
        .into_par_iter()
        .filter(|&at| cat.is_line(at) && cat.entry(at).is_some_and(|e| big_enough(e, smallest)))
        .count()
}

/// Said after the page's summary when Settings hides small parts: " · 1,234 under 3 notes hidden".
pub fn hidden_text(cat: &Catalogue, lines: usize, smallest: usize) -> String {
    let hidden = cat.line_count.saturating_sub(lines);
    if hidden == 0 || smallest <= 1 {
        return String::new();
    }
    format!(" · {} under {smallest} notes hidden", thousands(hidden))
}

/// Whether a part is in the years picked (the year strip, which isn't one of the chips).
fn in_years(f: &Filters, e: &Entry, dates: &RealDates) -> bool {
    f.years.is_none_or(|(a, b)| (a..=b).contains(&year_of(dates, e.phrase.source.mtime)))
}

/// Whether each chip filter lets a part through: kind, From, key.
fn chips_pass(f: &Filters, e: &Entry) -> [bool; 3] {
    [
        f.roles.is_empty() || f.roles.contains(&e.role()),
        f.from.is_none_or(|from| (from == From::Projects) == e.is_project()),
        f.keys.is_empty() || {
            let test = key_test(e);
            f.keys.iter().any(|&k| test(k))
        },
    ]
}

/// A test of one part by every filter at once (the words, Settings' smallest part, the years and the chips), for a
/// list [`view`] doesn't make: the parts tried for a song ([`crate::midi_fits`]). What it lets through is what a list
/// of everything would show, with every copy of a loop in other keys.
pub fn passing<'a>(cat: &'a Catalogue, f: &'a Filters) -> impl Fn(&Entry) -> bool + Sync + 'a {
    let words = mo_midi::catalogue::words_of(&f.text);
    move |e| {
        big_enough(e, f.smallest) && e.matches(&words) && in_years(f, e, &cat.dates) && chips_pass(f, e) == [true; 3]
    }
}

/// The parts the filters let through, in order, as hits in the catalogue, and how many each chip's choices would
/// show. Runs on every core; call it off the window's thread (rules 12 and 21). `seed` shuffles them (a random order
/// this number decides).
pub fn view(
    cat: &Catalogue,
    f: &Filters,
    order: Order,
    reversed: bool,
    target: Option<Target>,
    seed: Option<u64>,
) -> (Vec<Hit>, ChoiceCounts) {
    let mut base = searched(cat, f, target);
    // The same loop in other keys is one line, the first the filters let through ("in 12 keys"); a whole phrase's
    // list shows each.
    let one_line = !matches!(target, Some(Target::Within(_)));
    if let Some(Target::Like(at)) = target {
        // Not the part's own notes in another key: they're what it's most like.
        let own = cat.entry(at).and_then(|e| e.key_group);
        if own.is_some() {
            base.retain(|h| cat.entry(h.entry).is_none_or(|e| e.key_group != own));
        }
    }
    let counts = counted(cat, &base, f, one_line);
    let mut hits: Vec<Hit> = base
        .into_par_iter()
        .filter(|h| cat.entry(h.entry).is_some_and(|e| in_years(f, e, &cat.dates) && chips_pass(f, e) == [true; 3]))
        .collect();
    if one_line {
        one_per_key_group(cat, &mut hits);
    }
    // Ranked by likeness, or the longest phrase first: that order is the point.
    if target.is_none() {
        sort(cat, &mut hits, order);
        if reversed {
            turn_round(cat, &mut hits, order);
        }
    }
    // A shuffle is of the plain list: likeness and length are the order a ranked list is for.
    if let Some(seed) = seed.filter(|_| target.is_none()) {
        shuffle(&mut hits, seed);
    }
    (hits, counts)
}

/// How many parts each chip's choices would show, as [`view`] counts them for a list that isn't ranked against a
/// part, without making that list: for a page whose lines come from elsewhere (the parts that fit a song,
/// [`crate::midi_fits`]) and whose filters still say what each choice would show. Runs on every core; call it off
/// the window's thread.
pub fn choice_counts(cat: &Catalogue, f: &Filters) -> ChoiceCounts {
    counted(cat, &searched(cat, f, None), f, true)
}

/// The counts of `base` (what the words and the target let through), with how many lines the whole list has.
fn counted(cat: &Catalogue, base: &[Hit], f: &Filters, one_line: bool) -> ChoiceCounts {
    let mut counts = counts(cat, base, f, one_line);
    counts.lines = line_count(cat, f.smallest);
    counts
}

/// Keeps the first hit of each set of the same notes in other keys ([`Entry::key_group`]), in the order they come.
fn one_per_key_group(cat: &Catalogue, hits: &mut Vec<Hit>) {
    let mut seen = HashSet::new();
    hits.retain(|h| cat.entry(h.entry).and_then(|e| e.key_group).is_none_or(|g| seen.insert(g)));
}

/// The row of `hits` that shows the part at `at`: its own, else the line standing for it in another key.
pub fn row_of(cat: &Catalogue, hits: &[Hit], at: usize) -> Option<usize> {
    hits.iter().position(|h| h.entry == at).or_else(|| {
        let group = cat.entry(at).and_then(|e| e.key_group)?;
        hits.iter().position(|h| cat.entry(h.entry).is_some_and(|e| e.key_group == Some(group)))
    })
}

/// How many parts each chip's choices would show, with the other filters as they are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChoiceCounts {
    /// By kind, in [`Role::ALL`]'s order.
    pub roles: Vec<usize>,
    /// By where from, in [`FROM_CHOICES`]' order.
    pub froms: Vec<usize>,
    /// By key, majors then minors (the Key menu's order).
    pub keys: Vec<usize>,
    /// How many lines the whole list has with the smallest parts hidden ([`line_count`]): the "of" in "40 of 900".
    pub lines: usize,
}

impl ChoiceCounts {
    fn empty() -> ChoiceCounts {
        ChoiceCounts {
            roles: vec![0; Role::ALL.len()],
            froms: vec![0; FROM_CHOICES.len()],
            keys: vec![0; key_list().len()],
            lines: 0,
        }
    }

    /// One chip's counts: kind 0, From 1, key 2.
    fn chip(&mut self, chip: usize) -> &mut Vec<usize> {
        match chip {
            0 => &mut self.roles,
            1 => &mut self.froms,
            _ => &mut self.keys,
        }
    }

    fn add(mut self, other: ChoiceCounts) -> ChoiceCounts {
        for (a, b) in [(&mut self.roles, other.roles), (&mut self.froms, other.froms), (&mut self.keys, other.keys)] {
            a.iter_mut().zip(b).for_each(|(a, b)| *a += b);
        }
        self
    }
}

/// Counts every chip's choices in one pass over `hits`: a part counts beside every chip when all three let it
/// through, and beside the one that doesn't when only one doesn't (picking another choice there could show it). The
/// same loop in other keys is one line (`one_line`), so it then counts once beside each choice that would show it.
fn counts(cat: &Catalogue, hits: &[Hit], f: &Filters, one_line: bool) -> ChoiceCounts {
    let keys = key_list();
    let entries = || hits.par_iter().filter_map(|h| cat.entry(h.entry).filter(|e| in_years(f, e, &cat.dates)));
    let mut c = entries()
        .filter(|e| !one_line || e.key_group.is_none())
        .fold(ChoiceCounts::empty, |mut c, e| {
            choices_of(e, f, &keys, |chip, at| {
                if let Some(n) = c.chip(chip).get_mut(at) {
                    *n += 1;
                }
            });
            c
        })
        .reduce(ChoiceCounts::empty, ChoiceCounts::add);
    if !one_line {
        return c;
    }
    // Few: a pack's loops in other keys. Each (set, chip, choice) once.
    let grouped: Vec<&Entry> = entries().filter(|e| e.key_group.is_some()).collect();
    let mut seen = HashSet::new();
    for e in grouped {
        let group = e.key_group.unwrap_or_default();
        choices_of(e, f, &keys, |chip, at| {
            if seen.insert((group, chip, at))
                && let Some(n) = c.chip(chip).get_mut(at)
            {
                *n += 1;
            }
        });
    }
    c
}

/// Each chip choice a part counts beside (see [`counts`]), as (chip, place in that chip's choices): kind 0, From 1,
/// key 2.
fn choices_of(e: &Entry, f: &Filters, keys: &[KeyName], mut add: impl FnMut(usize, usize)) {
    let ok = chips_pass(f, e);
    let fails = ok.iter().filter(|o| !**o).count();
    let counts_on = |chip: usize| fails == 0 || (fails == 1 && !ok[chip]);
    if counts_on(0)
        && let Some(at) = Role::ALL.iter().position(|&r| r == e.role())
    {
        add(0, at);
    }
    if counts_on(1) {
        let from = if e.is_project() { From::Projects } else { From::Files };
        if let Some(at) = FROM_CHOICES.iter().position(|&w| w == from) {
            add(1, at);
        }
    }
    if counts_on(2) {
        let test = key_test(e);
        for (at, &k) in keys.iter().enumerate() {
            if test(k) {
                add(2, at);
            }
        }
    }
}

/// The chips as the page shows them: each filter's choices with their counts, and what's picked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chips {
    pub kind: Filter,
    pub from: Filter,
    pub key: Filter,
}

/// The chips for these counts and picks.
pub fn chips(c: &ChoiceCounts, f: &Filters) -> Chips {
    // Before the first list is worked out there are no counts: the chips show none rather than a row of 0s.
    let counted = |words: &str, n: Option<&usize>| (words.to_owned(), n.map(|n| thousands(*n)).unwrap_or_default());
    let kinds: Vec<(String, String)> =
        Role::ALL.iter().enumerate().map(|(at, r)| counted(r.plural(), c.roles.get(at))).collect();
    let froms: Vec<(String, String)> =
        FROM_CHOICES.iter().enumerate().map(|(at, &w)| counted(from_name(w), c.froms.get(at))).collect();
    let keys: Vec<(String, String)> =
        key_list().iter().enumerate().map(|(at, &k)| counted(&key_words(k), c.keys.get(at))).collect();
    Chips {
        kind: filters::show(&kinds, &f.roles.iter().map(|r| r.plural().to_owned()).collect::<Vec<_>>()),
        from: filters::show(&froms, &f.from.map(|w| vec![from_name(w).to_owned()]).unwrap_or_default()),
        key: filters::show(&keys, &f.keys.iter().map(|k| key_words(*k)).collect::<Vec<_>>()),
    }
}

/// The kind a chip's words ("Basslines") stand for.
fn role_of_plural(words: &str) -> Option<Role> {
    Role::ALL.into_iter().find(|r| r.plural().eq_ignore_ascii_case(words))
}

fn sort(cat: &Catalogue, hits: &mut [Hit], order: Order) {
    let e = |h: &Hit| cat.entry(h.entry);
    // Every sort is stable, so equal ones stay newest first.
    match order {
        Order::Newest => {}
        Order::Name => {
            hits.par_sort_by_cached_key(|h| e(h).map(|e| mo_formats::case::key(&title(e))).unwrap_or_default())
        }
        Order::Key => hits.par_sort_by_cached_key(|h| e(h).and_then(sort_key).unwrap_or(u8::MAX)),
        Order::Bars => hits.par_sort_by_key(|h| std::cmp::Reverse(e(h).map_or(0, |e| e.phrase.bars))),
        Order::Copies => hits.par_sort_by_key(|h| std::cmp::Reverse(e(h).map_or(0, Entry::copies))),
    }
}

/// Turns a sorted list round (a second click on its heading). The parts with nothing in the column still come last:
/// by key those with no clear key, by date those whose date isn't real (oldest first doesn't start with 1970).
fn turn_round(cat: &Catalogue, hits: &mut [Hit], order: Order) {
    hits.reverse();
    match order {
        Order::Key => hits.sort_by_key(|h| cat.entry(h.entry).and_then(sort_key).is_none()),
        Order::Newest => {
            hits.sort_by_key(|h| cat.entry(h.entry).is_none_or(|e| !cat.dates.seconds(e.phrase.source.mtime)))
        }
        Order::Name | Order::Bars | Order::Copies => {}
    }
}

/// A key's place in the Key order: round the circle of fifths from C, major then its relative minor.
fn sort_key(e: &Entry) -> Option<u8> {
    let k = key_of(e)?;
    let fifths = (k.tonic * 7) % 12;
    let major_fifths = if k.minor { (fifths + 9) % 12 } else { fifths };
    Some(major_fifths * 2 + u8::from(k.minor))
}

/// A small, sure shuffle (the same seed, the same order), so Shuffle can be tested.
fn shuffle(hits: &mut [Hit], seed: u64) {
    let mut s = seed | 1;
    for i in (1..hits.len()).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let j = (s % (i as u64 + 1)) as usize;
        hits.swap(i, j);
    }
}

/// The key a part's line and the Key chip go by: the Rust key finder's when the index has its reading, else the
/// C# app's, when it was sure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShownKey {
    pub tonic: u8,
    pub minor: bool,
}

fn key_of(e: &Entry) -> Option<ShownKey> {
    if let Some(fact) = &e.phrase.key_fact {
        // The key the line shows: one key, the likelier of a pair, or a home note (as major unless it leans minor);
        // never the key a "no clear key" answer only leans to, which the line doesn't show.
        let k = match fact.answer()? {
            KeyAnswer::Key(k) | KeyAnswer::OrRelative(k) => k,
            KeyAnswer::HomeNote { tonic, leaning } => KeyName { tonic, mode: leaning.unwrap_or(Mode::Major) },
            KeyAnswer::Unclear { .. } => return None,
        };
        return Some(ShownKey { tonic: k.tonic.number(), minor: k.mode == Mode::Minor });
    }
    e.phrase.key.filter(|k| k.is_sure()).map(|k| ShownKey { tonic: k.tonic, minor: k.minor })
}

/// Whether a part is in this key: the key the reading gives, either of a pair, or a home note's tonic.
pub fn in_key(e: &Entry, key: KeyName) -> bool {
    key_test(e)(key)
}

/// [`in_key`] for many keys: the key finder's answer is worked out once, not once for each key.
fn key_test(e: &Entry) -> impl Fn(KeyName) -> bool + '_ {
    let answer = e.phrase.key_fact.as_ref().map(|fact| fact.answer());
    move |key| match &answer {
        Some(answer) => answer.as_ref().is_some_and(|a| a.names(key)),
        None => e
            .phrase
            .key
            .filter(|k| k.is_sure())
            .is_some_and(|k| k.tonic == key.tonic.number() && k.minor == (key.mode == Mode::Minor)),
    }
}

/// Every key, majors then minors, in the Key chip's order.
fn key_list() -> Vec<KeyName> {
    let mut keys: Vec<KeyName> = KeyName::ALL.to_vec();
    keys.sort_by_key(|k| (k.mode == Mode::Minor, k.tonic.number()));
    keys
}

/// A key as the Key chip writes it ("Eb", "C#m").
fn key_words(k: KeyName) -> String {
    k.short()
}

pub fn key_of_choice(name: &str) -> Option<KeyName> {
    KeyName::ALL.into_iter().find(|k| k.short().eq_ignore_ascii_case(name))
}

/// The year a file was saved in, here; 0 for a date that isn't real by `dates`, which is in no year.
pub fn year_of(dates: &RealDates, mtime: i64) -> i32 {
    dates.year(mtime).unwrap_or(0)
}

/// How many parts each year holds (by when their newest file was saved), newest first, for the year strip.
pub fn year_counts(cat: &Catalogue) -> Vec<(i32, usize)> {
    let mut counts: std::collections::BTreeMap<i32, usize> = std::collections::BTreeMap::new();
    // A line each, as the list: the same loop in other keys once.
    for (_, e) in cat.entries.iter().enumerate().filter(|(at, _)| cat.is_line(*at)) {
        *counts.entry(year_of(&cat.dates, e.phrase.source.mtime)).or_insert(0) += 1;
    }
    crate::year_strip::strip(counts)
}

/// A kind of part, as one of them is called: "Bassline", "Melody".
pub fn role_label(role: Role) -> &'static str {
    match role {
        Role::Chords => "Chords",
        Role::Melody => "Melody",
        Role::Bass => "Bassline",
        Role::Arp => "Arp",
        Role::Drums => "Drums",
        Role::Other => "Something else",
    }
}

/// A kind's colour, the Sounds page's colour for the same kind of sound (Keys, Lead, Bass, Vocal, Kick), so the two
/// pages agree: a place in the theme's sound type colours, -1 for grey (something else).
pub fn role_colour(role: Role) -> i32 {
    match role {
        Role::Chords => 11,
        Role::Melody => 9,
        Role::Bass => 8,
        Role::Arp => 14,
        Role::Drums => 0,
        Role::Other => -1,
    }
}

/// The line's title: the track or clip's name, else the file's.
pub fn title(e: &Entry) -> String {
    let t = e.title();
    if t.is_empty() { e.source_name().to_owned() } else { t }
}

/// Where it's from: "Night drive 12 · FL Studio" for a song, "MIDI file in Dark Keys" for a file; with " · in 20
/// files" when other files have the same notes.
pub fn from_text(e: &Entry) -> String {
    let source = &e.phrase.source;
    let mut text = if e.is_project() {
        [e.source_name(), source.daw.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
    } else {
        let folder = source
            .path
            .rfind(['\\', '/'])
            .and_then(|cut| source.path.get(..cut))
            .map(mo_formats::paths::file_name)
            .unwrap_or_default();
        if folder.is_empty() { "MIDI file".to_owned() } else { format!("MIDI file in {folder}") }
    };
    if e.copies() > 1 {
        text.push_str(&format!(" · in {} files", thousands(e.copies())));
    }
    match e.in_keys {
        Some(mo_midi::InKeys::Keys(n)) => text.push_str(&format!(" · in {n} keys")),
        Some(mo_midi::InKeys::Octaves(n)) => text.push_str(&format!(" · in {n} octaves")),
        None => {}
    }
    text
}

/// The key as a line shows it: "Cm", "Cm or Eb" for a pair, "D?" for a home note whose major or minor isn't clear;
/// nothing for no clear key.
pub fn key_text(e: &Entry) -> String {
    match &e.phrase.key_fact {
        Some(fact) => match fact.answer() {
            Some(KeyAnswer::Key(k)) => k.short(),
            Some(KeyAnswer::OrRelative(k)) => format!("{} or {}", k.short(), k.relative().short()),
            Some(KeyAnswer::HomeNote { tonic, .. }) => {
                format!("{}?", KeyName { tonic, mode: Mode::Major }.tonic_name())
            }
            _ => String::new(),
        },
        None => e.key_text(),
    }
}

/// The line's details: "Piano · Chords · Cm · 8 bars · 64 notes", with what's unknown left out (and the instrument
/// when it only says the kind again).
pub fn details(e: &Entry) -> String {
    let role = role_label(e.role());
    let instrument = e.instrument.as_ref().map(|i| i.family.name()).filter(|n| {
        // "Bass" says nothing "Bassline" doesn't.
        let (n, r) = (n.to_lowercase(), role.to_lowercase());
        !r.starts_with(&n) && n != e.role().name().to_lowercase()
    });
    let key = key_text(e);
    let bars = bars_text(e.phrase.bars);
    let notes = format!("{} notes", thousands(e.phrase.notes.max(0) as usize));
    [instrument.unwrap_or(""), role, key.as_str(), bars.as_str(), notes.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

pub fn bars_text(bars: i32) -> String {
    match bars {
        ..=0 => String::new(),
        1 => "1 bar".to_owned(),
        n => format!("{n} bars"),
    }
}

/// Said in the accent colour under a line: how it's joined to other parts, else "".
pub fn phrase_text(e: &Entry) -> String {
    if e.is_joined() {
        return format!("Joined from {} clips", e.clip_count());
    }
    match e.within.len() {
        0 => String::new(),
        1 => "Part of a longer phrase".to_owned(),
        n => format!("Part of {n} longer phrases"),
    }
}

/// The date its newest file was saved: "3 Nov 2026"; nothing for a date that isn't real by `dates` (one still to come,
/// as a pack whose files all say 31 Dec 2027, or one before 1981, as a file with no date has).
pub fn date_text(dates: &RealDates, mtime: i64) -> String {
    if !dates.seconds(mtime) {
        return String::new();
    }
    Local.timestamp_opt(mtime, 0).single().map(|t| t.format("%-d %b %Y").to_string()).unwrap_or_default()
}

/// A MIDI key's name, middle C (60) as C4: the MIDI standard's numbers (FL calls it C5, Bitwig C3).
pub fn note_name(key: u8) -> String {
    let name = mo_harmony::names::FLATS_BUT_F_SHARP.get(usize::from(key % 12)).copied().unwrap_or("C");
    format!("{name}{}", i32::from(key / 12) - 1)
}

/// "C2 to G4"; one note's name when it's all one key.
pub fn range_text(lowest: u8, highest: u8) -> String {
    if lowest >= highest { note_name(lowest) } else { format!("{} to {}", note_name(lowest), note_name(highest)) }
}

/// The key finder's reading, as the card says it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeyCard {
    /// "C minor", "C minor or Eb major", "D, major or minor", "No clear key".
    pub answer: String,
    /// "72% sure", or "" when there's no answer to be sure of.
    pub sure: String,
    /// Why, in a sentence, and what it's next likeliest to be.
    pub why: String,
}

/// The card's words for a reading; `None` (no pitched notes, or drums): there's no key to read.
pub fn key_card(fact: Option<&KeyFact>, drums: bool) -> KeyCard {
    let Some((fact, answer)) = fact.and_then(|f| Some((f, f.answer()?))) else {
        let why = if drums { "Drums have no key." } else { "There are no pitched notes to read a key from." };
        return KeyCard { answer: "No key".to_owned(), sure: String::new(), why: why.to_owned() };
    };
    let sure = format!("{:.0}% sure", fact.shown_chance() * 100.0);
    // The next likeliest key, unless it's the other half of the pair the answer already names.
    let next = fact
        .runner_up()
        .filter(|k| !matches!(answer, KeyAnswer::OrRelative(a) if a.relative() == *k))
        .map(|k| format!(" Next likeliest: {}.", k.name()))
        .unwrap_or_default();
    match answer {
        KeyAnswer::Key(k) => KeyCard { answer: k.name(), sure, why: format!("The notes keep to {}.{next}", k.name()) },
        KeyAnswer::OrRelative(k) => KeyCard {
            answer: format!("{} or {}", k.name(), k.relative().name()),
            sure,
            why: format!("The notes fit both, and which note is home isn't clear; {} is likelier.{next}", k.name()),
        },
        KeyAnswer::HomeNote { tonic, leaning } => {
            let home = KeyName { tonic, mode: Mode::Major }.tonic_name();
            let leans = match leaning {
                Some(Mode::Minor) => " It leans minor.",
                Some(Mode::Major) => " It leans major.",
                None => "",
            };
            KeyCard {
                answer: format!("{home}, major or minor"),
                sure,
                why: format!("{home} is home, but the notes don't say major or minor (no third, or both).{leans}"),
            }
        }
        KeyAnswer::Unclear { why, leaning } => {
            let because = match why {
                Why::TooFewNotes => "There are too few notes to tell.",
                Why::NotTonal => "The notes keep to no one scale.",
                Why::Spread => "It could be one of several keys that aren't a pair.",
            };
            let likeliest = leaning.map(|k| format!(" The likeliest is {}.", k.name())).unwrap_or_default();
            KeyCard { answer: "No clear key".to_owned(), sure: String::new(), why: format!("{because}{likeliest}") }
        }
    }
}

/// The card's chord chart, as a lead sheet writes it: "Am7 | Dm9 G7 | Cmaj7 | –", a bar each ("–" where the chord
/// before holds on, "N.C." for no chord, "?" after a chord the reader isn't sure of), spelled in `key` (the card's
/// key, for spelling only). With why it reads as it does. Bars are 4/4, as everywhere on the MIDI page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChordChart {
    pub bars: String,
    pub why: String,
}

/// The chart of a chart reading (see [`ChordChart`]): at most [`mo_midi::chart::MOST_BARS`] bars, then "…" for a part
/// that goes on; none to show for no reading (drums, or no pitched notes).
pub fn chord_chart(read: Option<&mo_midi::chart::Read>, key: Option<KeyName>) -> ChordChart {
    use mo_harmony::ChordAnswer;
    let Some(read) = read.filter(|r| !r.reading.spans.is_empty()) else { return ChordChart::default() };
    let reading = &read.reading;
    let names = reading.names(key);
    let bar_of = |at: i64| usize::try_from((at - reading.start).div_euclid(BAR)).unwrap_or(0);
    let count = (bar_of(reading.end - 1) + 1).min(mo_midi::chart::MOST_BARS);
    let mut bars: Vec<Vec<String>> = vec![Vec::new(); count];
    // The chord last written (its name, and whether it's a guess), so one held on isn't written again.
    let mut last: Option<(String, bool)> = None;
    let (mut unsure, mut implied, mut chords) = (0, 0, 0);
    for (at, (span, name)) in reading.spans.iter().zip(&names).enumerate() {
        // A stretch the reader isn't sure of is written as its likeliest reading, even when the chart shows none there.
        let guess = span.answer == ChordAnswer::Unclear;
        let shown = name.map(|n| n.name()).or_else(|| {
            guess.then(|| span.readings.first().map(|r| reading.spell(&names, at, r.chord, key).name())).flatten()
        });
        let this = (shown.clone().unwrap_or_else(|| "N.C.".to_owned()), guess && shown.is_some());
        if last.as_ref().is_some_and(|(n, _)| *n == this.0) {
            continue;
        }
        let bar = bar_of(span.start);
        let Some(cell) = bars.get_mut(bar) else { break };
        // A change after the bar line, with the chord before held over it: that one is written first in the bar.
        let bar_start = reading.start + bar as i64 * BAR;
        if cell.is_empty()
            && span.start > bar_start
            && let Some((n, g)) = &last
        {
            cell.push(if *g { format!("{n}?") } else { n.clone() });
        }
        cell.push(if this.1 { format!("{}?", this.0) } else { this.0.clone() });
        if shown.is_some() {
            chords += 1;
            unsure += usize::from(this.1);
            implied += usize::from(span.answer == ChordAnswer::Implied);
        }
        last = Some(this);
    }
    let mut text = bars.iter().map(|b| if b.is_empty() { "–".to_owned() } else { b.join(" ") }).collect::<Vec<_>>();
    if read.goes_on {
        text.push("…".to_owned());
    }
    let why = if chords == 0 {
        "The reader hears no chord in it.".to_owned()
    } else if implied == chords {
        "Only one line plays, so these are the chords it implies.".to_owned()
    } else if unsure > 0 {
        let what = if unsure == 1 { "One chord is a guess".to_owned() } else { format!("{unsure} chords are guesses") };
        format!("{what} (marked ?): the notes could be read another way.")
    } else {
        String::new()
    };
    ChordChart { bars: text.join(" | "), why }
}

/// The key a card's chart is spelled in: the key the reading names, or leans to.
pub fn chart_key(fact: Option<&KeyFact>) -> Option<KeyName> {
    fact.and_then(KeyFact::answer).and_then(|a| a.leaning_key())
}

/// Whether a part's clip is a piece cut from a longer stretch (its name ends " · bars 9–16"), which the key finder
/// hears as a window, not a loop.
pub fn is_cut(clip: &str) -> bool {
    mo_midi::index::is_cut(clip)
}

/// One note on the piano roll, each number 0 to 1 of the roll: left, top, width, height; and how hard it's played
/// (0.4 to 1, for how strongly it's drawn); and its key (60 = C4), for its name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RollNote {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub strength: f32,
    pub key: u8,
}

/// Whether `key` is a black key on a piano (Db, Eb, F#, Ab, Bb).
pub fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

/// The piano roll of a part: its notes, its bar lines (0 to 1 across), the lowest and highest key it shows, and how
/// many notes were left off (past [`ROLL_NOTES`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Roll {
    pub notes: Vec<RollNote>,
    pub bars: Vec<f32>,
    pub low: u8,
    pub high: u8,
    pub left_off: usize,
}

/// The most notes a roll draws: plenty for a part, and the card stays quick for a whole song's worth.
pub const ROLL_NOTES: usize = 4000;
/// The fewest keys a roll shows top to bottom, so a part of three notes isn't drawn as three fat bars.
const ROLL_KEYS: u8 = 24;

/// The roll of these notes (at [`mo_midi::PPQ`]), `bars` long (or as long as the notes, if longer). Muted notes
/// don't play, so they aren't drawn.
pub fn roll(notes: &[NoteEvent], bars: i32) -> Roll {
    let heard: Vec<&NoteEvent> = notes.iter().filter(|n| !n.muted && n.length > 0).collect();
    let Some(lo) = heard.iter().map(|n| n.key).min() else { return Roll::default() };
    let hi = heard.iter().map(|n| n.key).max().unwrap_or(lo);
    // Room above and below, centred, at least two octaves.
    let spare = ROLL_KEYS.saturating_sub(hi - lo + 1);
    let low = lo.saturating_sub(spare / 2 + 1);
    let high = (hi + (spare - spare / 2) + 1).min(127).max(low + 1);
    let first = heard.iter().map(|n| n.start).min().unwrap_or(0).min(0);
    let end = heard.iter().map(|n| n.start + n.length).max().unwrap_or(BAR);
    let length = (i64::from(bars.max(1)) * BAR).max(end - first).max(1) as f32;
    let keys = f32::from(high - low + 1);
    let mut drawn: Vec<RollNote> = heard
        .iter()
        .take(ROLL_NOTES)
        .map(|n| RollNote {
            x: (n.start - first) as f32 / length,
            y: f32::from(high - n.key) / keys,
            w: n.length as f32 / length,
            h: 1.0 / keys,
            strength: 0.4 + 0.6 * f32::from(n.velocity) / 65_535.0,
            key: n.key,
        })
        .collect();
    drawn.iter_mut().for_each(|n| n.w = n.w.min(1.0 - n.x));
    let bar_count = (length / BAR as f32).ceil() as i64;
    // A line at each bar, or each 4 bars once they'd crowd together.
    let step = if bar_count > 32 { 4 } else { 1 };
    let bar_lines = (1..bar_count).filter(|b| b % step == 0).map(|b| (b * BAR) as f32 / length).collect();
    Roll { notes: drawn, bars: bar_lines, low, high, left_off: heard.len().saturating_sub(ROLL_NOTES) }
}

/// The page's summary: "12,345 MIDI parts from 3,210 files · 2,000 copies kept as one".
pub fn summary(cat: &Catalogue) -> String {
    if cat.entries.is_empty() {
        return "No MIDI found yet".to_owned();
    }
    let files: HashSet<&str> = cat
        .entries
        .iter()
        .flat_map(|e| std::iter::once(&e.phrase.source).chain(&e.other_sources))
        .map(|s| s.path.as_str())
        .collect();
    let mut text = format!("{} MIDI parts from {} files", thousands(cat.line_count), thousands(files.len()));
    // The same notes again, and the same loop in other keys.
    let copies = cat.phrase_count.saturating_sub(cat.line_count);
    if copies > 0 {
        let word = if copies == 1 { "copy" } else { "copies" };
        text.push_str(&format!(" · {} {word} kept as one", thousands(copies)));
    }
    text
}

/// Under the toolbar: what the list holds, in what order, of how many.
pub fn result_text(
    shown: usize,
    all: usize,
    order: Order,
    reversed: bool,
    target: Option<Target>,
    shuffled: bool,
) -> String {
    if shown == 0 {
        return String::new();
    }
    let n = thousands(shown);
    match target {
        // The pill beside it names the part.
        Some(Target::Like(_)) => format!("{n} parts, most alike first"),
        Some(Target::Within(_)) => {
            format!("{n} longer {}, longest first", if shown == 1 { "phrase" } else { "phrases" })
        }
        None if shuffled => format!("Shuffled · {n} of {}", thousands(all)),
        None if shown == all => format!("{n} parts, {}", order.words(reversed)),
        None => format!("{n} of {} parts, {}", thousands(all), order.words(reversed)),
    }
}

/// The card's facts, as (name, value): Bars, Notes, Range, Saved.
pub fn facts(e: &Entry, dates: &RealDates) -> Vec<(&'static str, String)> {
    let p = &e.phrase;
    let mut out = vec![("BARS", p.bars.max(0).to_string()), ("NOTES", thousands(p.notes.max(0) as usize))];
    if p.notes > 0 && !p.drums {
        out.push(("RANGE", range_text(p.lowest, p.highest)));
    }
    let saved = date_text(dates, p.source.mtime);
    out.push(("SAVED", if saved.is_empty() { "Unknown".to_owned() } else { saved }));
    out
}

/// What the instrument guess says and why: "Piano: its track is called Keys".
pub fn instrument_text(e: &Entry) -> String {
    e.instrument.as_ref().map(|i| format!("{}: {}", i.family.name(), i.why)).unwrap_or_default()
}

/// The other files with these notes, newest first, as names, at most `most` (and how many more).
pub fn other_files(e: &Entry, most: usize) -> (Vec<String>, usize) {
    let names = e.other_sources.iter().take(most).map(|s| mo_formats::paths::file_name(&s.path).to_owned()).collect();
    (names, e.other_sources.len().saturating_sub(most))
}

/// The SoundFont you picked to play parts with (the C# app's setting); none: the one in the app's folder.
pub const SOUNDFONT_KEY: &str = "midi_soundfont";
/// Where the C# app downloaded GeneralUser GS, in the app's folder: `sounds\GeneralUser-GS.sf2`.
pub const SOUNDFONT_FOLDER: &str = "sounds";
pub const SOUNDFONT_FILE: &str = "GeneralUser-GS.sf2";
/// The tempo a part plays at when its file doesn't say.
pub const PLAY_TEMPO: f64 = 120.0;

/// The SoundFont parts play with: the one picked, while it's there, else GeneralUser GS in the app's folder, else
/// none.
pub fn soundfont_path(chosen: Option<&str>, app_folder: &std::path::Path) -> Option<std::path::PathBuf> {
    let chosen = chosen.map(str::trim).filter(|c| !c.is_empty()).map(std::path::PathBuf::from);
    let usual = app_folder.join(SOUNDFONT_FOLDER).join(SOUNDFONT_FILE);
    chosen.filter(|c| c.is_file()).or_else(|| usual.is_file().then_some(usual))
}

/// The General MIDI sound a part plays with, matched to it (`mo_midi::gm`): drums, a MIDI file's own program, its
/// names (track, part, file), the instrument finder's answer, then its kind.
pub fn gm_sound(e: &Entry) -> mo_midi::gm::GmSound {
    let p = &e.phrase;
    let file = std::path::Path::new(&p.source.path).file_stem().and_then(|s| s.to_str()).unwrap_or("");
    // A project's channel has no program change of its own to go by.
    let program = if e.is_project() { None } else { p.program };
    mo_midi::gm::sound_for(
        e.role(),
        p.drums,
        program,
        &[&p.track_name, &p.part, &p.clip, file],
        e.instrument.as_ref().map(|i| i.family),
    )
}

/// A part's tempo, as [`read_tempo`] found it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tempo {
    /// Its song's, from the song library, or its MIDI file's first tempo.
    Own(f64),
    /// Its song or file gives none: it plays at [`PLAY_TEMPO`].
    NotGiven,
    /// Its song or file couldn't be read just now (a drive not plugged in): it plays at [`PLAY_TEMPO`].
    Unread,
    /// The song library hasn't read its project yet: it plays at [`PLAY_TEMPO`] until it has.
    NotListed,
}

impl Tempo {
    /// The beats a minute it plays at.
    pub fn bpm(self) -> f64 {
        match self {
            Tempo::Own(t) => t,
            Tempo::NotGiven | Tempo::Unread | Tempo::NotListed => PLAY_TEMPO,
        }
    }
}

/// A MIDI file bigger than this isn't opened again just for its tempo (a pack's loops are a few KB).
pub const TEMPO_FILE_LIMIT: u64 = 16 * 1024 * 1024;

/// A tempo the loop can play at: not one a damaged file gives (0, or thousands).
pub fn usable_tempo(tempo: Option<f64>) -> Option<f64> {
    tempo.filter(|t| t.is_finite() && *t >= 20.0 && *t <= 400.0)
}

/// Reads the tempo of the file a part is from: a project's from the song library at `library` (by its path, letter
/// case aside, as every save's tempo is kept there), a MIDI file's from the file itself (its first tempo). It opens
/// a file, so never on the window's thread.
pub fn read_tempo(source: &mo_midi::Source, library: &std::path::Path) -> Tempo {
    if let Some(t) = usable_tempo(source.tempo) {
        return Tempo::Own(t);
    }
    match source.kind {
        mo_midi::SourceKind::Project => {
            match mo_library::Library::open_read_only(library).and_then(|l| l.save_at(&source.path)) {
                Ok(Some(save)) => usable_tempo(save.tempo).map_or(Tempo::NotGiven, Tempo::Own),
                Ok(None) => Tempo::NotListed,
                Err(_) => Tempo::Unread,
            }
        }
        mo_midi::SourceKind::File => {
            let path = std::path::Path::new(&source.path);
            match std::fs::metadata(path) {
                Ok(m) if m.len() <= TEMPO_FILE_LIMIT => {}
                Ok(_) => return Tempo::NotGiven,
                Err(_) => return Tempo::Unread,
            }
            let Ok(raw) = std::fs::read(path) else { return Tempo::Unread };
            tempo_in_midi(&raw, path)
        }
    }
}

/// A MIDI file's first tempo, from its bytes. A damaged file can't stop the app: it reads as unread.
pub fn tempo_in_midi(raw: &[u8], path: &std::path::Path) -> Tempo {
    match mo_catch::catch(std::panic::AssertUnwindSafe(|| mo_formats::midi::read_from(raw, path))) {
        Ok(Ok(parts)) => usable_tempo(parts.iter().find_map(|p| p.tempo)).map_or(Tempo::NotGiven, Tempo::Own),
        _ => Tempo::Unread,
    }
}

/// What the card says about the sound: "Electric Piano 1 · from its name: rhodes · 128 BPM". With no tempo yet (it's
/// being read), no BPM, rather than a made-up one.
pub fn sound_line(sound: &mo_midi::gm::GmSound, tempo: Option<Tempo>) -> String {
    let head = format!("{} · {}", sound.name(), sound.why_text());
    let bpm = |t: f64| (t * 10.0).round() / 10.0;
    match tempo {
        None => head,
        Some(Tempo::Own(t)) => format!("{head} · {} BPM", bpm(t)),
        Some(Tempo::NotGiven) => format!("{head} · {} BPM, as it gives no tempo", bpm(PLAY_TEMPO)),
        Some(Tempo::Unread) => format!("{head} · {} BPM, as its tempo couldn't be read", bpm(PLAY_TEMPO)),
        Some(Tempo::NotListed) => {
            format!("{head} · {} BPM, as the song library hasn't read this save yet", bpm(PLAY_TEMPO))
        }
    }
}

/// A part's heard notes (muted ones left out) and how long it loops for, in ticks at [`mo_midi::PPQ`]: its bars, or
/// to the end of the bar its last note starts in when that's later, and a note held a hair past the loop's end stops
/// there. The part's clip, by the one rule a search judges it by too ([`mo_midi::analyser::clip_of`]).
pub fn loop_notes(notes: &[NoteEvent], bars: i32) -> (Vec<NoteEvent>, i64) {
    mo_midi::analyser::clip_of(notes, i64::from(bars))
}

/// The metronome's clicks, General MIDI drums: a high wood block on each bar's first beat, a low one on the others.
pub const METRONOME_BAR_KEY: u8 = 76;
pub const METRONOME_BEAT_KEY: u8 = 77;

/// A metronome for a loop `length` ticks long, as drum notes: a click on every beat (4 to a bar), the bar's first
/// louder. Played with the part, on the SoundFont's drums, so it keeps exact time with it at any speed.
pub fn metronome_notes(length: i64) -> Vec<NoteEvent> {
    let beat = mo_midi::PPQ;
    (0..(length.max(0) + beat - 1) / beat)
        .map(|at| {
            let bar = at % 4 == 0;
            NoteEvent {
                start: at * beat,
                length: beat / 4,
                key: if bar { METRONOME_BAR_KEY } else { METRONOME_BEAT_KEY },
                velocity: if bar { 65_535 } else { 45_000 },
                muted: false,
                step: false,
                attribute: mo_formats::NoteAttribute::default(),
            }
        })
        .collect()
}

/// How far one press of the stepper's minus or plus moves the speed, in BPM (as the curve editor's).
pub const BPM_STEP: f64 = 5.0;

/// The speed a part plays at, `by` steps up or down (the stepper's minus and plus): to the next multiple of
/// [`BPM_STEP`] that way (92 goes to 95 or 90), kept to what a part can play at ([`usable_tempo`]'s 20 to 400).
pub fn step_bpm(bpm: f64, by: i32) -> f64 {
    let steps = bpm / BPM_STEP;
    let on = if by > 0 { steps.floor() } else { steps.ceil() };
    ((on + f64::from(by)) * BPM_STEP).clamp(20.0, 400.0)
}

/// The stepper's words for a speed: "120 BPM", "92.5 BPM".
pub fn bpm_text(bpm: f64) -> String {
    let tenths = (bpm * 10.0).round() / 10.0;
    if tenths.fract() == 0.0 { format!("{tenths:.0} BPM") } else { format!("{tenths:.1} BPM") }
}

/// Where a loop made again at another speed starts, in seconds, so it goes on from the same beat rather than from
/// the top: `position` seconds into the loop at `was` BPM is that many beats in, at `now` BPM, round a loop
/// `seconds` long.
pub fn resume_seconds(position: f64, was: f64, now: f64, seconds: f64) -> f64 {
    if !(seconds > 0.0 && was > 0.0 && now > 0.0 && position.is_finite()) {
        return 0.0;
    }
    (position.max(0.0) * was / now).rem_euclid(seconds)
}

/// The C# app's settings in `library.db` the reading follows: the MIDI library folders (a JSON list), and "1" to
/// read every save of a song, not only its newest.
pub const FOLDERS_KEY: &str = "midi_folders";
pub const OLDER_SAVES_KEY: &str = "midi_older_saves";

/// The files the index should hold: the newest save of each song (or every save, `older`) that a reader takes and
/// that was there and readable at the last scan, grouped into songs as the Songs page groups them (Fix's copies still
/// waiting for Keep or Delete, `pending`, and DAW autosaves left out, as there); and every MIDI file the MIDI
/// folders' walk found.
pub fn wanted(
    mut saves: Vec<mo_library::Save>,
    pending: &mo_library::PathSet,
    choices: &mo_library::SongChoices,
    older: bool,
    files: &[mo_library::scan::FoundFile],
) -> Vec<mo_midi::indexer::Wanted> {
    use mo_midi::indexer::Wanted;
    saves.retain(|s| {
        !s.file_missing
            && s.error.is_none()
            && !pending.contains(&s.path)
            && !mo_library::scan::is_autosave_file(mo_formats::paths::file_name(&s.path))
            && mo_midi::extract::project_version(&s.path).is_some()
    });
    let readable = saves;
    let project = |s: &mo_library::Save| Wanted {
        path: s.path.clone(),
        kind: mo_midi::SourceKind::Project,
        mtime: s.mtime,
        size: s.size,
    };
    let mut out: Vec<Wanted> = if older {
        readable.iter().map(project).collect()
    } else {
        let real = RealDates::now();
        mo_library::songs::group_songs(&readable, choices, &real).iter().map(|song| project(&song.latest)).collect()
    };
    out.extend(files.iter().map(|f| Wanted {
        path: f.path.clone(),
        kind: mo_midi::SourceKind::File,
        mtime: f.mtime,
        size: f.size,
    }));
    out
}

/// What the activity card says when a reading ends.
pub fn read_done_text(s: &mo_midi::indexer::Summary) -> (String, String) {
    if s.stopped {
        return ("Stopped reading MIDI".to_owned(), "It carries on from there next time".to_owned());
    }
    if s.read == 0 {
        let rekeyed = if s.rekeyed > 0 { format!("; {} keys read again", thousands(s.rekeyed)) } else { String::new() };
        return ("MIDI up to date".to_owned(), format!("Nothing new to read{rekeyed}"));
    }
    let files = if s.read == 1 { "1 file".to_owned() } else { format!("{} files", thousands(s.read)) };
    let parts = if s.parts == 1 { "1 part".to_owned() } else { format!("{} parts", thousands(s.parts)) };
    let mut detail = format!("{files}, {parts}");
    if s.failed > 0 {
        detail.push_str(&format!("; {} couldn't be read", thousands(s.failed)));
    }
    ("MIDI read".to_owned(), detail)
}

/// The progress line while reading: "Reading MIDI: 1,204 of 30,112 · Dark keys 01.mid", then "Reading keys again:
/// 500 of 8,000" once the files are done and the index reads keys again from the notes it holds. The indexer says
/// which by its calls: the files' stage ends with a call that names no file and has every file done (or is the first
/// call, when there were no files to read); after that, every call is about keys.
#[derive(Clone, Debug, Default)]
pub struct ReadProgress {
    calls: usize,
    keys: bool,
}

impl ReadProgress {
    pub fn text(&mut self, done: usize, total: usize, path: &str) -> String {
        let first = self.calls == 0;
        self.calls += 1;
        if self.keys || (first && path.is_empty()) {
            self.keys = true;
            return format!("Reading keys again: {} of {}", thousands(done), thousands(total));
        }
        if path.is_empty() && done >= total {
            // The files are done; what comes next is keys.
            self.keys = true;
        }
        let name = mo_formats::paths::file_name(path);
        let mut text = format!("Reading MIDI: {} of {}", thousands(done), thousands(total));
        if !name.is_empty() {
            text.push_str(&format!(" · {name}"));
        }
        text
    }
}

/// The folder in the export folder where parts sent to Bitwig are written, as the C# MIDI page's.
pub const EXPORT_SUBFOLDER: &str = "MIDI browser";

/// A part as a MIDI clip for a DAW (Send to Bitwig, Play in Bitwig, the drag): its heard notes (muted ones left out)
/// at [`mo_midi::PPQ`], on one track named after it, starting where its first bar starts. It lasts whole bars: `bars`
/// of them, by the same rule as the loop the app plays and a search judges ([`mo_midi::analyser::clip_of`]), so a
/// last note held a hair over the last bar line stops at the line and the clip goes round on it in the DAW. (A file
/// that ended a few ticks after the line would loop a few ticks late each time round, or come out a bar longer.)
pub fn part_clip(name: &str, notes: &[NoteEvent], bars: i32) -> mo_formats::MidiClip {
    let heard: Vec<NoteEvent> = notes.iter().filter(|n| !n.muted).copied().collect();
    let first = heard.iter().map(|n| n.start).min().unwrap_or(0).min(0);
    let moved: Vec<NoteEvent> =
        heard.into_iter().map(|n| NoteEvent { start: n.start.saturating_sub(first), ..n }).collect();
    // Notes moved later to start at the clip's start may fill more bars than the part's count: the clip has the
    // bars they fill.
    let (moved, end) = mo_midi::analyser::clip_of(&moved, i64::from(bars));
    let mut clip = mo_formats::MidiClip {
        name: name.to_owned(),
        ppq: u16::try_from(mo_midi::PPQ).unwrap_or(96),
        tracks: Vec::new(),
        end: Some(end),
    };
    clip.track(name).extend(moved);
    clip
}

/// The start of the MIDI files Play in Bitwig writes in the system's Temp folder: "MO Play 4120-3.mid" (the app's
/// process id and a count). Bitwig reads one into a clip on the selected track, which shows that name, and the file
/// goes again at once; the start-up sweep clears one a crash left ([`is_play_file`]).
pub const PLAY_FILE_PREFIX: &str = "MO Play ";

/// A new name for a Play in Bitwig file: "MO Play <process id>-<count>.mid".
pub fn play_file_name() -> String {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{PLAY_FILE_PREFIX}{}-{n}.mid", std::process::id())
}

/// A new name for an audition's Bitwig preset in Temp, made from a song's sound for the Presets page's Audition: "MO
/// Play <process id>-<count>.bwpreset". It goes once Bitwig has loaded it, and the sweep knows it as a play file.
pub fn play_preset_name() -> String {
    let mid = play_file_name();
    format!("{}.bwpreset", mid.strip_suffix(".mid").unwrap_or(&mid))
}

/// Whether a file's whole name is one [`play_file_name`] or [`play_preset_name`] gives (rule 28: the sweep removes
/// only those).
pub fn is_play_file(name: &str) -> bool {
    let Some(numbers) = name
        .strip_prefix(PLAY_FILE_PREFIX)
        .and_then(|r| r.strip_suffix(".mid").or_else(|| r.strip_suffix(".bwpreset")))
    else {
        return false;
    };
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    numbers.split_once('-').is_some_and(|(pid, n)| digits(pid) && digits(n))
}

/// A part as FL Studio plays it on its selected channel ([`mo_dawlink::fl`]): its heard notes, round and round over
/// the same loop as Play loop's ([`loop_notes`]), at `tempo`.
pub fn fl_part(notes: &[NoteEvent], bars: i32, tempo: f64) -> mo_dawlink::fl::Part {
    let (heard, length) = loop_notes(notes, bars);
    mo_dawlink::fl::Part { notes: heard, ppq: mo_midi::PPQ, length, tempo }
}

/// Whether `name` is one [`write_part`] gives a part while it's being written: `<name>.<process>-<count>.part`.
pub fn is_half_sent(name: &str) -> bool {
    let Some(rest) = name.strip_suffix(".part") else { return false };
    let Some((before, tail)) = rest.rsplit_once('.') else { return false };
    let Some((app, write)) = tail.split_once('-') else { return false };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    !before.is_empty() && digits(app) && digits(write)
}

/// Writes a part's MIDI file into `dir` under `name` (made safe for a file name) as a new file: never over another.
/// "Keys · Pattern 3.mid", or "Keys · Pattern 3 (2).mid" when that's taken. Written beside it first and only then
/// given its name, so Bitwig never sees half a file.
pub fn write_part(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let stem = mo_formats::textscan::safe_file_name(name);
    // A name of its own for the half-written file, so two sends of the same part at once don't meet.
    static WRITING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITING.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let part = dir.join(format!("{stem}.{}-{n}.part", std::process::id()));
    let written = std::fs::OpenOptions::new().write(true).create_new(true).open(&part).and_then(|mut f| {
        f.write_all(bytes)?;
        f.sync_all()
    });
    if let Err(e) = written {
        // Only a file this made is taken away; one already there under that name belongs to someone else.
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            let _ = std::fs::remove_file(&part);
        }
        return Err(e);
    }
    for n in 1..1000 {
        let file = if n == 1 { dir.join(format!("{stem}.mid")) } else { dir.join(format!("{stem} ({n}).mid")) };
        if file.exists() {
            continue;
        }
        return match std::fs::rename(&part, &file) {
            Ok(()) => Ok(file),
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                Err(e)
            }
        };
    }
    let _ = std::fs::remove_file(&part);
    Err(std::io::Error::other(format!("there are already 999 files called {stem} in {}", dir.display())))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use mo_midi::{Source, SourceKind, StoredPhrase};

    use super::*;

    pub(crate) fn phrase(path: &str, part: &str, role: Role, hash: &str, mtime: i64) -> StoredPhrase {
        StoredPhrase {
            id: 1,
            source: Arc::new(Source {
                path: path.to_owned(),
                kind: if path.ends_with(".mid") { SourceKind::File } else { SourceKind::Project },
                daw: if path.ends_with(".mid") { "MIDI".into() } else { "FL Studio".into() },
                mtime,
                size: 100,
                tempo: None,
            }),
            part: part.to_owned(),
            clip: String::new(),
            role,
            role_why: String::new(),
            your_role: None,
            bars: 4,
            notes: 16,
            lowest: 48,
            highest: 67,
            key: None,
            key_fact: None,
            chroma: [0; 12],
            chords: Vec::new(),
            features: vec![0; 48],
            hash: hash.to_owned(),
            members: String::new(),
            program: None,
            drums: role == Role::Drums,
            track_name: String::new(),
            your_octave: None,
            shape: None,
            cut: None,
        }
    }

    fn sure(tonic: u8, minor: bool) -> Option<mo_midi::Key> {
        Some(mo_midi::Key { tonic, minor, confidence: 0.9 })
    }

    fn catalogue() -> Catalogue {
        let day = 86_400;
        let mut a = phrase("C:/Songs/Night drive 12.flp", "Bass", Role::Bass, "A", 1_760_000_000);
        a.key = sure(9, true);
        let mut b = phrase("L:/New Midi/Dark Keys/keys 01.mid", "Piano", Role::Chords, "B", 1_760_000_000 - day);
        b.key = sure(0, false);
        b.bars = 8;
        let c = phrase("L:/New Midi/Drums/groove.mid", "Kick", Role::Drums, "C", 1_700_000_000);
        // An older save with the bassline again: one entry, in 2 files.
        let a2 = phrase("C:/Songs/Night drive 11.flp", "Bass", Role::Bass, "A", 1_690_000_000);
        Catalogue::new(vec![a, b, c, a2])
    }

    fn list(cat: &Catalogue, f: &Filters, o: Order, t: Option<Target>, seed: Option<u64>) -> Vec<Hit> {
        view(cat, f, o, false, t, seed).0
    }

    fn titles(cat: &Catalogue, hits: &[Hit]) -> Vec<String> {
        hits.iter().map(|h| title(cat.entry(h.entry).unwrap())).collect()
    }

    #[test]
    fn kinds_can_be_picked_together() {
        let cat = catalogue();
        let mut f = Filters::default();
        assert_eq!(list(&cat, &f, Order::Newest, None, None).len(), 3);
        f.toggle(KIND, "Basslines");
        assert_eq!(titles(&cat, &list(&cat, &f, Order::Newest, None, None)), ["Bass"]);
        f.toggle(KIND, "Drums");
        assert_eq!(titles(&cat, &list(&cat, &f, Order::Newest, None, None)), ["Bass", "Kick"]);
        f.toggle(KIND, "Basslines");
        assert_eq!(f.roles, [Role::Drums]);
        assert!(f.any());
    }

    #[test]
    fn key_from_and_years_narrow_it() {
        let cat = catalogue();
        let f = Filters { keys: key_of_choice("Am").into_iter().collect(), ..Filters::default() };
        assert_eq!(titles(&cat, &list(&cat, &f, Order::Newest, None, None)), ["Bass"]);
        let f = Filters { from: Some(From::Files), ..Filters::default() };
        assert_eq!(titles(&cat, &list(&cat, &f, Order::Newest, None, None)), ["Piano", "Kick"]);
        let y = year_of(&cat.dates, 1_700_000_000);
        let f = Filters { years: Some((y, y)), ..Filters::default() };
        assert_eq!(titles(&cat, &list(&cat, &f, Order::Newest, None, None)), ["Kick"]);
        assert!(
            year_counts(&cat).first().is_some_and(|(year, n)| *year == year_of(&cat.dates, 1_760_000_000) && *n == 2)
        );
    }

    #[test]
    fn ctrl_k_finds_a_loop_by_its_other_keys_file_names() {
        let loop_in = |tonic: u8, hash: &str| {
            let mut p =
                phrase(&format!("L:/New Midi/Pack/Pad Loop {hash}.mid"), "Lead", Role::Melody, hash, 1_760_000_000);
            p.key = sure(tonic, true);
            p.lowest = 57 + tonic % 12;
            p.shape = Some(42);
            p
        };
        let cat = Catalogue::new(vec![loop_in(9, "Am"), loop_in(6, "Fsm")]);
        let parts: std::sync::Arc<dyn crate::finder::Source> = std::sync::Arc::new(crate::finder::midi_parts(&cat, 1));
        assert_eq!(parts.len(), 1, "one line for the loop");
        let g = crate::finder::find(&[parts], "pad loop fsm", None, 4);
        assert_eq!(g.len(), 1, "the other key's file is found by its own name");
    }

    #[test]
    fn the_metronome_and_the_speed() {
        // Two bars and a half: a click a beat, rounded up to the last beat begun, the bars' first beats high.
        let clicks = metronome_notes(2 * BAR + mo_midi::PPQ / 2);
        assert_eq!(clicks.len(), 9);
        let keys: Vec<u8> = clicks.iter().map(|n| n.key).collect();
        assert_eq!(&keys[..5], &[76, 77, 77, 77, 76]);
        assert_eq!(clicks[8].key, METRONOME_BAR_KEY);
        assert!(clicks[0].velocity > clicks[1].velocity);
        assert!(metronome_notes(0).is_empty());
        // Steps go to the next 5 BPM, and stay where a part can play.
        assert_eq!(step_bpm(120.0, 1), 125.0);
        assert_eq!(step_bpm(120.0, -1), 115.0);
        assert_eq!(step_bpm(92.0, 1), 95.0);
        assert_eq!(step_bpm(92.0, -1), 90.0);
        assert_eq!(step_bpm(400.0, 1), 400.0);
        assert_eq!(step_bpm(22.0, -1), 20.0);
        assert_eq!(bpm_text(120.0), "120 BPM");
        assert_eq!(bpm_text(92.54), "92.5 BPM");
        // At half the speed a beat lasts twice as long: 1 s in at 120 is 2 s in at 60, round an 8 s loop.
        assert_eq!(resume_seconds(1.0, 120.0, 60.0, 8.0), 2.0);
        assert_eq!(resume_seconds(5.0, 120.0, 60.0, 8.0), 2.0);
        assert_eq!(resume_seconds(f64::NAN, 120.0, 60.0, 8.0), 0.0);
        assert_eq!(resume_seconds(1.0, 120.0, 120.0, 0.0), 0.0);
    }

    #[test]
    fn parts_under_the_smallest_part_are_hidden_and_said() {
        let sized = |name: &str, hash: &str, notes: i32| {
            let mut p = phrase(&format!("L:/New Midi/{name}.mid"), name, Role::Melody, hash, 1_760_000_000);
            p.notes = notes;
            p
        };
        let cat = Catalogue::new(vec![sized("Lead", "L", 16), sized("Pickup", "P", 1), sized("Answer", "A", 2)]);
        // Every part (as the index keeps them now).
        let every = Filters { smallest: 1, ..Filters::default() };
        let (hits, counts) = view(&cat, &every, Order::Name, false, None, None);
        assert_eq!(titles(&cat, &hits), ["Answer", "Lead", "Pickup"]);
        assert_eq!(counts.lines, 3);
        assert_eq!(hidden_text(&cat, counts.lines, 1), "");
        // Settings' 3 notes: only the lead, and the list says how many it has of all there are to show.
        let three = Filters { smallest: 3, ..Filters::default() };
        assert!(!three.any(), "not one of the page's own filters");
        let (hits, counts) = view(&cat, &three, Order::Name, false, None, None);
        assert_eq!(titles(&cat, &hits), ["Lead"]);
        assert_eq!(counts.lines, 1);
        assert_eq!(counts.roles.iter().sum::<usize>(), 1, "the chips count only what can show");
        assert_eq!(choice_counts(&cat, &three), counts);
        assert_eq!(hidden_text(&cat, counts.lines, 3), " · 2 under 3 notes hidden");
        let two = Filters { smallest: 2, ..Filters::default() };
        assert_eq!(titles(&cat, &view(&cat, &two, Order::Name, false, None, None).0), ["Answer", "Lead"]);
        // What Settings keeps.
        assert_eq!(smallest_part_from(None), 3);
        assert_eq!(smallest_part_from(Some(" 1 ")), 1);
        assert_eq!(smallest_part_from(Some("8")), 8);
        for odd in ["0", "5", "-1", "lots", ""] {
            assert_eq!(smallest_part_from(Some(odd)), 3, "{odd:?}");
        }
        assert_eq!(smallest_part_words(1), "Every part");
        assert_eq!(smallest_part_words(4), "4 notes");
        assert_eq!(smallest_part_index(1), 0);
        assert_eq!(smallest_part_index(3), 2);
        assert_eq!(smallest_part_index(5), 2, "the default's place");
        // Ctrl+K finds only what the list can show.
        let parts = crate::finder::midi_parts(&cat, 3);
        assert_eq!(crate::finder::Source::len(&parts), 1);
    }

    #[test]
    fn the_same_loop_in_other_keys_is_one_line() {
        let day = 86_400;
        let loop_in = |tonic: u8, hash: &str, age: i64| {
            let mut p = phrase(
                &format!("L:/New Midi/Pack/loop {hash}.mid"),
                "Lead",
                Role::Melody,
                hash,
                1_760_000_000 - age * day,
            );
            p.key = sure(tonic, true);
            p.lowest = 57 + tonic % 12;
            p.shape = Some(42);
            p
        };
        let other = phrase("L:/New Midi/Pack/other.mid", "Pad", Role::Chords, "P", 1_700_000_000);
        let cat = Catalogue::new(vec![loop_in(9, "Am", 0), loop_in(2, "Dm", 1), loop_in(4, "Em", 2), other]);
        let f = Filters::default();
        let (hits, counts) = view(&cat, &f, Order::Newest, false, None, None);
        // The counts alone, for a page whose lines come from elsewhere, are the same.
        assert_eq!(choice_counts(&cat, &f), counts);
        let keys = |hits: &[Hit]| hits.iter().map(|h| key_text(cat.entry(h.entry).unwrap())).collect::<Vec<_>>();
        assert_eq!(titles(&cat, &hits), ["Lead", "Pad"]);
        assert_eq!(keys(&hits), ["Am", ""], "the newest stands for them");
        assert_eq!(from_text(cat.entry(hits[0].entry).unwrap()), "MIDI file in Pack · in 3 keys");
        // Counted once beside its kind, and once beside each key it's in.
        let melody = Role::ALL.iter().position(|&r| r == Role::Melody).unwrap();
        assert_eq!(counts.roles[melody], 1);
        let at = |name: &str| key_list().iter().position(|k| Some(*k) == key_of_choice(name)).unwrap();
        assert_eq!((counts.keys[at("Am")], counts.keys[at("Dm")], counts.keys[at("Cm")]), (1, 1, 0));
        // A key picked shows the loop in that key.
        let f = Filters { keys: key_of_choice("Dm").into_iter().collect(), ..Filters::default() };
        assert_eq!(keys(&list(&cat, &f, Order::Newest, None, None)), ["Dm"]);
        // A part picked in another key is shown by its line; the year strip and summary count a line each.
        let dm = cat.find("Dm").unwrap();
        assert_eq!(row_of(&cat, &hits, dm), Some(0));
        assert_eq!(year_counts(&cat).iter().map(|(_, n)| n).sum::<usize>(), 2);
        assert!(summary(&cat).starts_with("2 MIDI parts"));
        // More like this: not its own notes in another key.
        let like = list(&cat, &Filters::default(), Order::Newest, Some(Target::Like(dm)), None);
        assert_eq!(titles(&cat, &like), ["Pad"]);
    }

    #[test]
    fn the_card_writes_the_chords_as_a_chart() {
        let chord = |bar: i64, keys: &[u8]| -> Vec<NoteEvent> {
            keys.iter()
                .map(|&key| NoteEvent {
                    start: bar * BAR,
                    length: BAR,
                    key,
                    velocity: 50_000,
                    muted: false,
                    step: false,
                    attribute: mo_formats::NoteAttribute::NONE,
                })
                .collect()
        };
        // Am7, Dm7, then G7 held two bars.
        let mut notes =
            [chord(0, &[45, 60, 64, 67]), chord(1, &[50, 60, 65, 69]), chord(2, &[43, 59, 62, 65])].concat();
        notes.extend(chord(3, &[43, 59, 62, 65]));
        let read = mo_midi::chart::read(&notes, Role::Chords, false);
        let chart = chord_chart(read.as_ref(), None);
        assert_eq!(chart.bars, "Am7 | Dm7 | G7 | –");
        assert_eq!(chart.why, "");
        assert_eq!(chord_chart(None, None), ChordChart::default());
        // A change on beat 3, after Am7 held over the bar line: Am7 is written first in that bar.
        let half = |bar: i64, beats: i64, keys: &[u8]| -> Vec<NoteEvent> {
            chord(0, keys)
                .into_iter()
                .map(|n| NoteEvent { start: bar * BAR + beats * BAR / 4, length: BAR / 2, ..n })
                .collect()
        };
        let mut notes = chord(0, &[45, 60, 64, 67]);
        notes.extend(half(1, 0, &[45, 60, 64, 67]));
        notes.extend(half(1, 2, &[43, 59, 62, 65]));
        let read = mo_midi::chart::read(&notes, Role::Chords, false);
        assert_eq!(chord_chart(read.as_ref(), None).bars, "Am7 | Am7 G7");
    }

    #[test]
    fn orders() {
        let cat = catalogue();
        let f = Filters::default();
        assert_eq!(titles(&cat, &list(&cat, &f, Order::Bars, None, None)), ["Piano", "Bass", "Kick"]);
        assert_eq!(titles(&cat, &list(&cat, &f, Order::Copies, None, None)), ["Bass", "Piano", "Kick"]);
        // C major, then A minor (its relative, the same place round the circle); no key last.
        assert_eq!(titles(&cat, &list(&cat, &f, Order::Key, None, None)), ["Piano", "Bass", "Kick"]);
        assert_eq!(titles(&cat, &list(&cat, &f, Order::Name, None, None)), ["Bass", "Kick", "Piano"]);
        // Turned round (a second click on the heading); by key, the part with no clear key still comes last.
        assert_eq!(titles(&cat, &view(&cat, &f, Order::Name, true, None, None).0), ["Piano", "Kick", "Bass"]);
        assert_eq!(titles(&cat, &view(&cat, &f, Order::Key, true, None, None).0), ["Bass", "Piano", "Kick"]);
        // A shuffle keeps every part and is the same for the same seed.
        let one = list(&cat, &f, Order::Newest, None, Some(7));
        assert_eq!(one, list(&cat, &f, Order::Newest, None, Some(7)));
        assert_eq!(one.len(), 3);
        // A ranked list keeps its ranking, shuffled or not.
        let at = cat.find("A").unwrap();
        let ranked = list(&cat, &f, Order::Newest, Some(Target::Like(at)), None);
        assert_eq!(list(&cat, &f, Order::Newest, Some(Target::Like(at)), Some(7)), ranked);
        for o in Order::ALL {
            assert_eq!(Order::from_column(o.column()), o);
        }
    }

    #[test]
    fn a_date_that_isnt_real_comes_last_either_way_and_isnt_shown() {
        // Two real dates; a pack whose files say a date still to come (one of Solomon's says 31 Dec 2027); and a file
        // with no date, which reads as 1970. In the index's order (mo-midi's store puts them so).
        let cat = Catalogue::new(vec![
            phrase("L:/New Midi/a.mid", "New", Role::Melody, "A", 1_760_000_000),
            phrase("L:/New Midi/b.mid", "Old", Role::Melody, "B", 1_700_000_000),
            phrase("L:/New Midi/Pack/16BTFILL.MID", "Fill", Role::Drums, "C", 4_102_401_600),
            phrase("L:/New Midi/c.mid", "Undated", Role::Melody, "D", 0),
        ]);
        let f = Filters::default();
        assert_eq!(titles(&cat, &list(&cat, &f, Order::Newest, None, None)), ["New", "Old", "Fill", "Undated"]);
        // Oldest first: the real ones turned round, the others still last.
        assert_eq!(titles(&cat, &view(&cat, &f, Order::Newest, true, None, None).0), ["Old", "New", "Undated", "Fill"]);

        // Neither the line nor the card shows a date that isn't real as the day the file was saved, and it's in no
        // year: neither on the strip nor let through by a span of years.
        let d = &cat.dates;
        assert!(date_text(d, 4_102_401_600).is_empty() && date_text(d, 0).is_empty() && date_text(d, -1).is_empty());
        let saved = |hash: &str| facts(cat.entry(cat.find(hash).unwrap()).unwrap(), d).pop().unwrap();
        assert_eq!(saved("C"), ("SAVED", "Unknown".to_owned()));
        assert_eq!(saved("D"), ("SAVED", "Unknown".to_owned()));
        assert_eq!(saved("A"), ("SAVED", date_text(d, 1_760_000_000)));
        assert!(date_text(d, 1_760_000_000).ends_with(" 2025"), "{}", date_text(d, 1_760_000_000));
        assert_eq!((year_of(d, 4_102_401_600), year_of(d, 0)), (0, 0));
        assert_eq!(year_counts(&cat).iter().map(|(_, n)| n).sum::<usize>(), 2);
        let f = Filters { years: Some((1, 9999)), ..Filters::default() };
        assert_eq!(titles(&cat, &list(&cat, &f, Order::Newest, None, None)), ["New", "Old"]);
    }

    #[test]
    fn words_for_a_line() {
        let cat = catalogue();
        let bass = cat.entry(cat.find("A").unwrap()).unwrap();
        assert_eq!(from_text(bass), "Night drive 12 · FL Studio · in 2 files");
        assert_eq!(details(bass), "Bassline · Am · 4 bars · 16 notes");
        let keys = cat.entry(cat.find("B").unwrap()).unwrap();
        assert_eq!(from_text(keys), "MIDI file in Dark Keys");
        assert_eq!(range_text(48, 67), "C3 to G4");
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(61), "Db4");
        assert_eq!(bars_text(1), "1 bar");
        assert_eq!(summary(&cat), "3 MIDI parts from 4 files · 1 copy kept as one");
        assert_eq!(result_text(2, 3, Order::Newest, false, None, false), "2 of 3 parts, newest first");
        // The chips say the orders as Sounds and Plugins do; the line over the list says them in a sentence.
        assert_eq!(Order::ALL.map(Order::column), ["Saved", "Part", "Key", "Bars", "Files"]);
        assert!(Order::ALL.into_iter().all(|o| Order::from_column(o.column()) == o));
        assert_eq!(result_text(3, 3, Order::Bars, true, None, false), "3 parts, shortest first");
        assert_eq!(result_text(3, 3, Order::Bars, false, None, false), "3 parts, longest first");
        assert_eq!(result_text(1_200, 1_500, Order::Name, false, None, false), "1,200 of 1,500 parts, by name");
    }

    #[test]
    fn the_card_says_the_key_in_words() {
        let none = key_card(None, true);
        assert_eq!((none.answer.as_str(), none.why.as_str()), ("No key", "Drums have no key."));
        // A real reading from the key finder: a C major scale up and down, as a loop.
        let notes: Vec<NoteEvent> = [60u8, 62, 64, 65, 67, 69, 71, 72, 67, 64, 60, 55, 60]
            .iter()
            .enumerate()
            .map(|(i, &key)| NoteEvent {
                start: i as i64 * 96,
                length: 90,
                key,
                velocity: 50_000,
                muted: false,
                step: false,
                attribute: mo_formats::NoteAttribute::NONE,
            })
            .collect();
        let (_, fact) = mo_midi::keys::with_reader(|r| mo_midi::keys::read(r, &notes, Role::Melody, false));
        let card = key_card(fact.as_ref(), false);
        assert!(!card.answer.is_empty());
        assert!(card.sure.is_empty() || card.sure.ends_with("% sure"));
    }

    #[test]
    fn the_newest_save_of_each_song_and_every_midi_file_are_wanted() {
        let save = |path: &str, mtime: i64| mo_library::Save {
            path: path.to_owned(),
            name: mo_formats::paths::stem(path).to_owned(),
            daw: "FL Studio".to_owned(),
            folder: "C:\\Songs".to_owned(),
            mtime,
            ..mo_library::Save::default()
        };
        let saves = vec![
            save("C:\\Songs\\Night drive_12.flp", 300),
            save("C:\\Songs\\Night drive_11.flp", 200),
            mo_library::Save { file_missing: true, ..save("C:\\Songs\\Gone.flp", 100) },
            save("C:\\Songs\\notes.txt", 50),
        ];
        let files = vec![mo_library::scan::FoundFile { path: "L:\\New Midi\\a.mid".to_owned(), size: 99, mtime: 5 }];
        let choices = mo_library::SongChoices::default();
        let none = mo_library::PathSet::default();
        let paths = |w: Vec<mo_midi::indexer::Wanted>| w.into_iter().map(|w| w.path).collect::<Vec<_>>();
        assert_eq!(
            paths(wanted(saves.clone(), &none, &choices, false, &files)),
            ["C:\\Songs\\Night drive_12.flp", "L:\\New Midi\\a.mid"]
        );
        assert_eq!(wanted(saves.clone(), &none, &choices, true, &files).len(), 3);
        // An autosave is never a song's newest save.
        let mut with_autosave = saves;
        with_autosave.insert(0, save("C:\\Songs\\Night drive_12 (autosave).flp", 400));
        assert_eq!(paths(wanted(with_autosave, &none, &choices, false, &[]))[0], "C:\\Songs\\Night drive_12.flp");
    }

    #[test]
    fn reading_says_how_it_went() {
        let s = mo_midi::indexer::Summary { read: 3, parts: 12, failed: 1, ..Default::default() };
        assert_eq!(read_done_text(&s), ("MIDI read".to_owned(), "3 files, 12 parts; 1 couldn't be read".to_owned()));
        let s = mo_midi::indexer::Summary { stopped: true, ..Default::default() };
        assert_eq!(read_done_text(&s).0, "Stopped reading MIDI");
        assert_eq!(read_done_text(&Default::default()).0, "MIDI up to date");
        let s = mo_midi::indexer::Summary { read: 3, parts: 1, ..Default::default() };
        assert_eq!(read_done_text(&s).1, "3 files, 1 part");
        // Files, then the files' last call, then keys.
        let mut p = ReadProgress::default();
        assert_eq!(p.text(5, 10, "L:\\x\\a.mid"), "Reading MIDI: 5 of 10 · a.mid");
        assert_eq!(p.text(10, 10, ""), "Reading MIDI: 10 of 10");
        assert_eq!(p.text(500, 800, ""), "Reading keys again: 500 of 800");
        assert_eq!(p.text(800, 800, ""), "Reading keys again: 800 of 800");
        // No files to read: straight to keys.
        assert_eq!(ReadProgress::default().text(3, 9, ""), "Reading keys again: 3 of 9");
        // A stopped run's last call names no file but isn't done: still files.
        let mut p = ReadProgress::default();
        p.text(1, 10, "a.mid");
        assert_eq!(p.text(5, 10, ""), "Reading MIDI: 5 of 10");
    }

    #[test]
    fn cut_pieces_are_known_by_their_name() {
        assert!(is_cut("Pattern 3 · bars 9–16"));
        assert!(!is_cut("Pattern 3"));
        assert!(!is_cut("bars of gold"));
    }

    #[test]
    fn the_roll_fits_its_notes() {
        let n = |start, key| NoteEvent {
            start,
            length: 96,
            key,
            velocity: 65_535,
            muted: false,
            step: false,
            attribute: mo_formats::NoteAttribute::NONE,
        };
        let r = roll(&[n(0, 60), n(384, 64), NoteEvent { muted: true, ..n(96, 90) }], 2);
        assert_eq!(r.notes.len(), 2);
        assert!(r.high - r.low + 1 >= ROLL_KEYS);
        assert!(r.low < 60 && r.high > 64);
        assert_eq!(r.bars, [0.5]);
        assert!(r.notes.iter().all(|n| n.x >= 0.0 && n.x + n.w <= 1.0 && n.y >= 0.0 && n.y < 1.0));
        assert!((r.notes[0].strength - 1.0).abs() < 1e-6);
        assert_eq!(roll(&[], 4), Roll::default());
        assert_eq!(r.notes[0].key, 60);
    }

    #[test]
    fn from_chips_name_where_parts_come_from() {
        let names: Vec<&str> = FROM_CHOICES.iter().map(|f| from_name(*f)).collect();
        assert_eq!(names, ["Your projects", "MIDI folders"]);
        for f in FROM_CHOICES {
            assert_eq!(from_of_name(from_name(f)), Some(f));
        }
    }

    #[test]
    fn a_midi_files_tempo_is_its_first_one() {
        use mo_formats::midi::write::{SmfSong, SmfTrack, WriteOptions, write};
        let note = NoteEvent {
            start: 0,
            length: 96,
            key: 60,
            velocity: 50_000,
            muted: false,
            step: false,
            attribute: mo_formats::NoteAttribute::NONE,
        };
        let song = |tempo| SmfSong {
            name: "Loop".into(),
            ppq: 96,
            tempo,
            tracks: vec![SmfTrack { name: "Bass".into(), notes: vec![note], ..Default::default() }],
            ..Default::default()
        };
        let with = write(&song(Some(85.0)), &WriteOptions { tempo: true, ..Default::default() }).unwrap().bytes;
        let without = write(&song(None), &WriteOptions::default()).unwrap().bytes;
        let path = std::path::Path::new("Loop 85.mid");
        assert_eq!(tempo_in_midi(&with, path), Tempo::Own(85.0));
        assert_eq!(tempo_in_midi(&without, path), Tempo::NotGiven);
        assert_eq!(tempo_in_midi(b"MThd broken", path), Tempo::Unread);
        assert_eq!(Tempo::NotGiven.bpm(), PLAY_TEMPO);
    }

    #[test]
    fn a_tempo_that_cant_be_read_plays_at_120() {
        let missing = std::path::Path::new("/no/such/folder/library.db");
        let source = |path: &str, kind, tempo| mo_midi::Source {
            path: path.into(),
            kind,
            daw: String::new(),
            mtime: 0,
            size: 1,
            tempo,
        };
        let song = source("/no/such/folder/Song.flp", mo_midi::SourceKind::Project, None);
        assert_eq!(read_tempo(&song, missing), Tempo::Unread);
        let file = source("/no/such/folder/Loop.mid", mo_midi::SourceKind::File, None);
        assert_eq!(read_tempo(&file, missing), Tempo::Unread);
        // A tempo the index already holds (the C# app's midi.db) is used as it is, and a damaged one isn't.
        let kept = source("/no/such/folder/Loop.mid", mo_midi::SourceKind::File, Some(93.0));
        assert_eq!(read_tempo(&kept, missing), Tempo::Own(93.0));
        let damaged = source("/no/such/folder/Song.flp", mo_midi::SourceKind::Project, Some(0.0));
        assert_eq!(read_tempo(&damaged, missing), Tempo::Unread);
    }

    #[test]
    fn a_part_plays_with_its_matched_sound() {
        let cat = catalogue();
        let bass = cat.entry(cat.find("A").unwrap()).unwrap();
        let sound = gm_sound(bass);
        assert_eq!(sound.program, 38);
        assert!(gm_sound(cat.entry(cat.find("C").unwrap()).unwrap()).drums);
        assert_eq!(sound_line(&sound, None), "Synth Bass 1 · from its name: bass");
        assert_eq!(sound_line(&sound, Some(Tempo::Own(85.0))), "Synth Bass 1 · from its name: bass · 85 BPM");
        assert_eq!(
            sound_line(&sound, Some(Tempo::NotGiven)),
            "Synth Bass 1 · from its name: bass · 120 BPM, as it gives no tempo"
        );
    }

    /// One rule for a part's clip, however it leaves the page: the loop played here and in FL, the MIDI file dragged
    /// and sent, and the length a search judges it over.
    #[test]
    fn a_parts_loop_its_midi_file_and_what_a_search_judges_are_one_clip() {
        use mo_midi::analyser::{bars_of, clip_length};
        let n = |start, length| NoteEvent {
            start,
            length,
            key: 60,
            velocity: 30_000,
            muted: false,
            step: false,
            attribute: mo_formats::NoteAttribute::NONE,
        };
        let four: Vec<NoteEvent> = (0..8).map(|i| n(i * BAR / 2, BAR / 2)).collect();
        // The last note held a hair over the fourth bar line: four bars, and the note stops at the line.
        let mut over = four.clone();
        over[7].length += 3;
        // A grace note that starts in that hair: in the clip, which then has a fifth bar.
        let mut grace = four.clone();
        grace.push(n(4 * BAR + 1, 2));
        // A muted bar first: a bar of the clip.
        let mut led = vec![NoteEvent { muted: true, ..n(0, BAR) }];
        led.extend(four.iter().map(|x| NoteEvent { start: x.start + BAR, ..*x }));
        for (notes, bars) in [(&four, 4), (&over, 4), (&grace, 5), (&led, 5)] {
            // The index's count of the part.
            let counted = i32::try_from(bars_of(notes)).unwrap();
            let (heard, length) = loop_notes(notes, counted);
            let clip = part_clip("Part", notes, counted);
            assert_eq!(length, bars * BAR);
            assert_eq!((clip_length(notes), clip.end), (length, Some(length)));
            assert_eq!(clip.tracks[0].1, heard);
            assert!(heard.iter().all(|x| x.start + x.length <= length));
            assert_eq!(fl_part(notes, counted, 120.0).length, length);
        }
        assert_eq!(part_clip("Part", &over, 4).tracks[0].1[7].length, BAR / 2);
        // Notes moved later to start at the clip's start (one before tick 0) keep their lengths: the file has the
        // bars they then fill.
        let early = [n(-BAR / 4, BAR / 4), n(3 * BAR, BAR)];
        let clip = part_clip("Part", &early, 4);
        assert_eq!((clip.end, clip.tracks[0].1[1].length), (Some(5 * BAR), BAR));
    }

    #[test]
    fn a_loop_is_its_bars_or_to_its_last_note() {
        let n = |start, muted| NoteEvent {
            start,
            length: 96,
            key: 60,
            velocity: 30_000,
            muted,
            step: false,
            attribute: mo_formats::NoteAttribute::NONE,
        };
        let (heard, length) = loop_notes(&[n(0, false), n(96, true)], 2);
        assert_eq!((heard.len(), length), (1, 2 * BAR));
        assert_eq!(loop_notes(&[n(5 * BAR + 10, false)], 4).1, 6 * BAR);
        assert_eq!(loop_notes(&[], 0).1, BAR);
        // A count smaller than the bars the notes fill doesn't cut them: the loop has the bars they fill.
        let held = NoteEvent { length: 3 * BAR, ..n(0, false) };
        assert_eq!(loop_notes(&[held], 2), (vec![held], 3 * BAR));
    }

    #[test]
    fn the_soundfont_is_the_picked_one_else_the_one_in_the_app_folder() {
        let dir = std::env::temp_dir().join(format!("mo-app-soundfont-{}", std::process::id()));
        let usual = dir.join(SOUNDFONT_FOLDER).join(SOUNDFONT_FILE);
        assert_eq!(soundfont_path(None, &dir), None);
        std::fs::create_dir_all(usual.parent().unwrap()).unwrap();
        std::fs::write(&usual, b"x").unwrap();
        let picked = dir.join("mine.sf2");
        assert_eq!(soundfont_path(Some(picked.to_str().unwrap()), &dir), Some(usual.clone()));
        std::fs::write(&picked, b"x").unwrap();
        assert_eq!(soundfont_path(Some(picked.to_str().unwrap()), &dir), Some(picked));
        assert_eq!(soundfont_path(Some("  "), &dir), Some(usual));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chips_pick_and_count() {
        let cat = catalogue();
        let mut f = Filters::default();
        f.toggle(KIND, "Basslines");
        f.toggle(KIND, "Drums");
        assert_eq!(f.roles, [Role::Bass, Role::Drums]);
        f.toggle(KIND, "Basslines");
        assert_eq!(f.roles, [Role::Drums]);
        f.clear(KIND);
        // From is one or none: a second pick moves it, picking it again lets it go.
        f.toggle(FROM, "Your projects");
        f.toggle(FROM, "MIDI folders");
        assert_eq!(f.from, Some(From::Files));
        f.toggle(FROM, "MIDI folders");
        assert_eq!(f.from, None);
        f.toggle(KEY, "Am");
        f.toggle(KEY, "C");
        assert_eq!(f.keys.len(), 2);
        assert_eq!(titles(&cat, &list(&cat, &f, Order::Newest, None, None)), ["Bass", "Piano"]);
        // A chip's counts leave out its own pick, so its other choices say what picking them would add.
        f.keys.clear();
        f.toggle(FROM, "MIDI folders");
        let (hits, counts) = view(&cat, &f, Order::Newest, false, None, None);
        assert_eq!(titles(&cat, &hits), ["Piano", "Kick"]);
        assert_eq!(counts.froms, [1, 2]);
        // And so are the counts alone, whichever way the list would be ordered.
        assert_eq!(choice_counts(&cat, &f), counts);
        assert_eq!(choice_counts(&cat, &f), view(&cat, &f, Order::Name, true, None, Some(7)).1);
        let drums = Role::ALL.iter().position(|r| *r == Role::Drums).unwrap();
        let bass = Role::ALL.iter().position(|r| *r == Role::Bass).unwrap();
        assert_eq!((counts.roles[drums], counts.roles[bass]), (1, 0));
        let c = key_list().iter().position(|k| key_words(*k) == "C").unwrap();
        assert_eq!(counts.keys[c], 1);
        let shown = chips(&counts, &f);
        assert!(shown.from.chips.iter().any(|c| c.text == "MIDI folders" && c.on && c.count == "2"));
        f.clear(FROM);
        assert!(!f.any());
        // Before any list is worked out: the choices, with no count rather than 0.
        let first = chips(&ChoiceCounts::default(), &f);
        assert_eq!(first.kind.chips.len(), Role::ALL.len());
        assert!(first.kind.chips.iter().chain(&first.from.chips).all(|c| c.count.is_empty()));
        // Words that differ only in capitals pick the same key.
        f.toggle(KEY, "am");
        assert_eq!(f.keys, key_of_choice("Am").into_iter().collect::<Vec<_>>());
    }

    #[test]
    fn a_part_for_bitwig_is_its_heard_notes_from_its_first_bar() {
        let n = |start, key| NoteEvent {
            start,
            length: 96,
            key,
            velocity: 65_535,
            muted: false,
            step: false,
            attribute: mo_formats::NoteAttribute::NONE,
        };
        let clip = part_clip("Keys", &[n(-96, 60), n(384, 64), NoteEvent { muted: true, ..n(96, 90) }], 2);
        assert_eq!(clip.ppq, 96);
        assert_eq!(clip.tracks.len(), 1);
        let starts: Vec<i64> = clip.tracks[0].1.iter().map(|n| n.start).collect();
        assert_eq!(starts, [0, 480]);
        assert_eq!(clip.end, Some(2 * BAR));
        assert!(mo_formats::midi::write::write_clip(&clip).is_ok());
    }

    #[test]
    fn a_part_is_written_as_a_new_file_never_over_another() {
        let dir = std::env::temp_dir().join(format!("mo-app-midi-write-part-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = write_part(&dir, "Keys · Pattern 3", b"one").unwrap();
        let second = write_part(&dir, "Keys · Pattern 3", b"two").unwrap();
        assert_eq!(mo_formats::paths::file_name(&first.to_string_lossy()), "Keys · Pattern 3.mid");
        assert_eq!(mo_formats::paths::file_name(&second.to_string_lossy()), "Keys · Pattern 3 (2).mid");
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
        let unsafe_name = write_part(&dir, "A/B: C?", b"x").unwrap();
        assert!(unsafe_name.starts_with(&dir));
        // Nothing half-written is left beside them.
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "part"))
            .collect();
        assert!(left.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pianos_black_keys_are_its_sharps_and_flats() {
        let black: Vec<bool> = (59..=63).map(is_black).collect();
        assert_eq!(black, [false, false, true, false, true], "B3 C4 Db4 D4 Eb4");
        assert!(!is_black(65) && is_black(66));
    }

    #[test]
    fn play_files_have_names_of_their_own() {
        let name = play_file_name();
        assert!(is_play_file(&name), "{name}");
        assert_ne!(play_file_name(), name);
        assert!(is_play_file("MO Play 4120-3.mid"));
        assert!(!is_play_file("MO Play 4120.mid"));
        assert!(!is_play_file("MO Play 4120-3-1.mid"));
        assert!(!is_play_file("MO Play Bass.mid"));
        assert!(!is_play_file("MO Play 4120-3.mid.part"));
        assert!(!is_play_file("Keys \u{b7} Pattern 3.mid"));
        // An audition's Bitwig preset, made from a song's sound.
        let preset = play_preset_name();
        assert!(preset.ends_with(".bwpreset") && is_play_file(&preset), "{preset}");
        assert!(!is_play_file("MO Play 4120-3.bwpreset.part"));
        assert!(!is_play_file("Serum - Pad.bwpreset"));
    }

    #[test]
    fn fl_plays_the_loop_play_loop_plays() {
        let note = |start: i64, muted: bool| NoteEvent {
            start,
            length: 48,
            key: 60,
            velocity: 40_000,
            muted,
            step: false,
            attribute: mo_formats::NoteAttribute::NONE,
        };
        let part = fl_part(&[note(0, false), note(96, true), note(BAR * 2, false)], 2, 128.0);
        assert_eq!(part.notes.len(), 2);
        assert_eq!(part.length, BAR * 3);
        assert_eq!((part.ppq, part.tempo), (mo_midi::PPQ, 128.0));
    }
}
