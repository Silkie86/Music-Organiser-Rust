//! The MIDI page's screen logic and every word it shows: which parts the filters let through and in what order, each
//! line's words, the picked part's card (its key as the key finder reads it, its facts and its notes for the piano
//! roll), and the page's summary. No window in it, so all of it is tested here.
//!
//! The parts come from `mo_midi::Catalogue` (one entry per set of notes, the newest copy, "in 20 files"); the slow
//! work (reading the index, working out a list, reading a part's notes) is the page's to run off the window's thread.
//! Browse only: the Playground app (editing) comes later, and so do the GM sound, fitting to a song, Send to Bitwig
//! and drag out.

use std::collections::HashSet;

use chrono::{Datelike, Local, TimeZone};
use mo_formats::NoteEvent;
use mo_harmony::keyfind::Why;
use mo_harmony::{Key as KeyName, KeyAnswer, KeyFact, Mode};
use mo_library::songs::thousands;
use mo_midi::{BAR, Catalogue, Entry, From, Hit, Query, Role};
use rayon::prelude::*;

use crate::filters::{self, Filter};

/// The orders the list can take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    /// The newest file first, as the index keeps them.
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

    pub fn name(self) -> &'static str {
        match self {
            Order::Newest => "Newest first",
            Order::Name => "Name",
            Order::Key => "Key",
            Order::Bars => "Longest first",
            Order::Copies => "In most files",
        }
    }

    pub fn from_name(name: &str) -> Order {
        Order::ALL.into_iter().find(|o| o.name() == name).unwrap_or_default()
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
}

impl Filters {
    /// Picks a kind of part, or lets it go again.
    pub fn toggle_role(&mut self, role: Role) {
        if let Some(at) = self.roles.iter().position(|r| *r == role) {
            self.roles.remove(at);
        } else {
            self.roles.push(role);
        }
    }

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
    cat.search(&q).0
}

/// Whether a part is in the years picked (the year strip, which isn't one of the chips).
fn in_years(f: &Filters, e: &Entry) -> bool {
    f.years.is_none_or(|(a, b)| (a..=b).contains(&year_of(e.phrase.source.mtime)))
}

/// Whether each chip filter lets a part through: kind, From, key.
fn chips_pass(f: &Filters, e: &Entry) -> [bool; 3] {
    [
        f.roles.is_empty() || f.roles.contains(&e.role()),
        f.from.is_none_or(|from| (from == From::Projects) == e.is_project()),
        f.keys.is_empty() || f.keys.iter().any(|&k| in_key(e, k)),
    ]
}

/// The parts the filters let through, in order, as hits in the catalogue, and how many each chip's choices would
/// show. Runs on every core; call it off the window's thread (rules 12 and 21). `seed` shuffles them (a random order
/// this number decides).
pub fn view(
    cat: &Catalogue,
    f: &Filters,
    order: Order,
    target: Option<Target>,
    seed: Option<u64>,
) -> (Vec<Hit>, ChoiceCounts) {
    let base = searched(cat, f, target);
    let counts = counts(cat, &base, f);
    let mut hits: Vec<Hit> = base
        .into_par_iter()
        .filter(|h| cat.entry(h.entry).is_some_and(|e| in_years(f, e) && chips_pass(f, e) == [true; 3]))
        .collect();
    // Ranked by likeness, or the longest phrase first: that order is the point.
    if target.is_none() {
        sort(cat, &mut hits, order);
    }
    // A shuffle is of the plain list: likeness and length are the order a ranked list is for.
    if let Some(seed) = seed.filter(|_| target.is_none()) {
        shuffle(&mut hits, seed);
    }
    (hits, counts)
}

/// How many parts each chip's choices would show, with the other filters as they are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChoiceCounts {
    /// By kind, in [`Role::ALL`]'s order.
    pub roles: Vec<usize>,
    /// By where from, in [`FROM_CHOICES`]' order.
    pub froms: Vec<usize>,
    /// By key, in [`key_choices`]' order.
    pub keys: Vec<usize>,
}

impl ChoiceCounts {
    fn empty() -> ChoiceCounts {
        ChoiceCounts { roles: vec![0; Role::ALL.len()], froms: vec![0; FROM_CHOICES.len()], keys: vec![0; 24] }
    }

    fn add(mut self, other: ChoiceCounts) -> ChoiceCounts {
        for (a, b) in [(&mut self.roles, other.roles), (&mut self.froms, other.froms), (&mut self.keys, other.keys)] {
            a.iter_mut().zip(b).for_each(|(a, b)| *a += b);
        }
        self
    }
}

/// Counts every chip's choices in one pass over `hits`: a part counts beside every chip when all three let it
/// through, and beside the one that doesn't when only one doesn't (picking another choice there could show it).
fn counts(cat: &Catalogue, hits: &[Hit], f: &Filters) -> ChoiceCounts {
    let keys = key_list();
    hits.par_iter()
        .filter_map(|h| cat.entry(h.entry).filter(|e| in_years(f, e)))
        .fold(ChoiceCounts::empty, |mut c, e| {
            let ok = chips_pass(f, e);
            let fails = ok.iter().filter(|o| !**o).count();
            let counts_on = |chip: usize| fails == 0 || (fails == 1 && !ok[chip]);
            if counts_on(0)
                && let Some(at) = Role::ALL.iter().position(|&r| r == e.role())
            {
                c.roles[at] += 1;
            }
            if counts_on(1) {
                c.froms[usize::from(!e.is_project())] += 1;
            }
            if counts_on(2) {
                for (at, &k) in keys.iter().enumerate() {
                    if in_key(e, k) {
                        c.keys[at] += 1;
                    }
                }
            }
            c
        })
        .reduce(ChoiceCounts::empty, ChoiceCounts::add)
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
    let counted = |words: &str, n: Option<&usize>| (words.to_owned(), thousands(n.copied().unwrap_or(0)));
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
    Role::ALL.into_iter().find(|r| r.plural() == words)
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
    if let Some(fact) = &e.phrase.key_fact {
        return fact.answer().is_some_and(|a| a.names(key));
    }
    e.phrase
        .key
        .filter(|k| k.is_sure())
        .is_some_and(|k| k.tonic == key.tonic.number() && k.minor == (key.mode == Mode::Minor))
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

/// The Key chip's choices: every key, majors then minors, as the list writes them ("Eb", "C#m").
pub fn key_choices() -> Vec<String> {
    key_list().into_iter().map(KeyName::short).collect()
}

pub fn key_of_choice(name: &str) -> Option<KeyName> {
    KeyName::ALL.into_iter().find(|k| k.short() == name)
}

/// The year a file was saved in, here.
pub fn year_of(mtime: i64) -> i32 {
    Local.timestamp_opt(mtime, 0).single().map_or(0, |t| t.year())
}

/// How many parts each year holds (by when their newest file was saved), newest first, for the year strip.
pub fn year_counts(cat: &Catalogue) -> Vec<(i32, usize)> {
    let mut counts: std::collections::BTreeMap<i32, usize> = std::collections::BTreeMap::new();
    for e in &cat.entries {
        *counts.entry(year_of(e.phrase.source.mtime)).or_insert(0) += 1;
    }
    counts.into_iter().rev().filter(|(y, _)| *y > 1980).collect()
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

/// The date its newest file was saved: "3 Nov 2026".
pub fn date_text(mtime: i64) -> String {
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

/// Whether a part's clip is a piece cut from a longer stretch (its name ends " · bars 9–16"), which the key finder
/// hears as a window, not a loop.
pub fn is_cut(clip: &str) -> bool {
    clip.rsplit(" · ").next().is_some_and(|last| {
        last.strip_prefix("bars ")
            .is_some_and(|r| r.contains('–') && r.chars().next().is_some_and(|c| c.is_ascii_digit()))
    })
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

/// One key of the roll's keyboard, top to bottom: its row (top and height, 0 to 1 of the roll), whether it's a black
/// key, and its name ("C4").
#[derive(Clone, Debug, PartialEq)]
pub struct RollKey {
    pub y: f32,
    pub h: f32,
    pub black: bool,
    pub name: String,
}

/// Whether `key` is a black key on a piano (Db, Eb, F#, Ab, Bb).
pub fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

/// The keyboard down the side of a roll from `low` to `high`, highest at the top, as [`roll`] lays its rows out.
pub fn roll_keys(low: u8, high: u8) -> Vec<RollKey> {
    if high < low {
        return Vec::new();
    }
    let keys = f32::from(high - low + 1);
    (low..=high)
        .rev()
        .map(|key| RollKey {
            y: f32::from(high - key) / keys,
            h: 1.0 / keys,
            black: is_black(key),
            name: note_name(key),
        })
        .collect()
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

/// The page's summary: "12,345 MIDI parts from 3,210 files · 2,000 copies kept as one", with where the parts and
/// their keys come from.
pub fn summary(cat: &Catalogue, from_cs: bool) -> String {
    if cat.entries.is_empty() {
        return "No MIDI found yet".to_owned();
    }
    let files: HashSet<&str> = cat
        .entries
        .iter()
        .flat_map(|e| std::iter::once(&e.phrase.source).chain(&e.other_sources))
        .map(|s| s.path.as_str())
        .collect();
    let mut text = format!("{} MIDI parts from {} files", thousands(cat.entries.len()), thousands(files.len()));
    let copies = cat.phrase_count.saturating_sub(cat.entries.len());
    if copies > 0 {
        let word = if copies == 1 { "copy" } else { "copies" };
        text.push_str(&format!(" · {} {word} kept as one", thousands(copies)));
    }
    if from_cs {
        text.push_str(" · keys in the list from the old app");
    }
    text
}

/// Under the toolbar: what the list holds, in what order, of how many.
pub fn result_text(shown: usize, all: usize, order: Order, target: Option<Target>, shuffled: bool) -> String {
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
        None if shown == all => format!("{n} parts, {}", order.name().to_lowercase()),
        None => format!("{n} of {} parts, {}", thousands(all), order.name().to_lowercase()),
    }
}

/// The card's facts, as (name, value): Bars, Notes, Range, Saved.
pub fn facts(e: &Entry) -> Vec<(&'static str, String)> {
    let p = &e.phrase;
    let mut out = vec![("BARS", p.bars.max(0).to_string()), ("NOTES", thousands(p.notes.max(0) as usize))];
    if p.notes > 0 && !p.drums {
        out.push(("RANGE", range_text(p.lowest, p.highest)));
    }
    out.push(("SAVED", date_text(p.source.mtime)));
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
        mo_library::songs::group_songs(&readable, choices).iter().map(|song| project(&song.latest)).collect()
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

/// A part as a MIDI clip for Bitwig: its heard notes (muted ones left out) at [`mo_midi::PPQ`], on one track named
/// after it, lasting `bars` (or to its last note, if that's later), starting where its first bar starts.
pub fn part_clip(name: &str, notes: &[NoteEvent], bars: i32) -> mo_formats::MidiClip {
    let heard: Vec<NoteEvent> = notes.iter().filter(|n| !n.muted).copied().collect();
    let first = heard.iter().map(|n| n.start).min().unwrap_or(0).min(0);
    let moved: Vec<NoteEvent> = heard.into_iter().map(|n| NoteEvent { start: n.start - first, ..n }).collect();
    let last = moved.iter().map(|n| n.start + n.length).max().unwrap_or(0);
    let end = (i64::from(bars.max(1)) * BAR).max(last);
    let mut clip = mo_formats::MidiClip {
        name: name.to_owned(),
        ppq: u16::try_from(mo_midi::PPQ).unwrap_or(96),
        tracks: Vec::new(),
        end: Some(end),
    };
    clip.track(name).extend(moved);
    clip
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
mod tests {
    use std::sync::Arc;

    use mo_midi::{Source, SourceKind, StoredPhrase};

    use super::*;

    fn phrase(path: &str, part: &str, role: Role, hash: &str, mtime: i64) -> StoredPhrase {
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
        view(cat, f, o, t, seed).0
    }

    fn titles(cat: &Catalogue, hits: &[Hit]) -> Vec<String> {
        hits.iter().map(|h| title(cat.entry(h.entry).unwrap())).collect()
    }

    #[test]
    fn kinds_can_be_picked_together() {
        let cat = catalogue();
        let mut f = Filters::default();
        assert_eq!(list(&cat, &f, Order::Newest, None, None).len(), 3);
        f.toggle_role(Role::Bass);
        assert_eq!(titles(&cat, &list(&cat, &f, Order::Newest, None, None)), ["Bass"]);
        f.toggle_role(Role::Drums);
        assert_eq!(titles(&cat, &list(&cat, &f, Order::Newest, None, None)), ["Bass", "Kick"]);
        f.toggle_role(Role::Bass);
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
        let y = year_of(1_700_000_000);
        let f = Filters { years: Some((y, y)), ..Filters::default() };
        assert_eq!(titles(&cat, &list(&cat, &f, Order::Newest, None, None)), ["Kick"]);
        assert!(year_counts(&cat).first().is_some_and(|(year, n)| *year == year_of(1_760_000_000) && *n == 2));
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
        // A shuffle keeps every part and is the same for the same seed.
        let one = list(&cat, &f, Order::Newest, None, Some(7));
        assert_eq!(one, list(&cat, &f, Order::Newest, None, Some(7)));
        assert_eq!(one.len(), 3);
        // A ranked list keeps its ranking, shuffled or not.
        let at = cat.find("A").unwrap();
        let ranked = list(&cat, &f, Order::Newest, Some(Target::Like(at)), None);
        assert_eq!(list(&cat, &f, Order::Newest, Some(Target::Like(at)), Some(7)), ranked);
        for o in Order::ALL {
            assert_eq!(Order::from_name(o.name()), o);
        }
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
        assert!(summary(&cat, true).starts_with("3 MIDI parts from 4 files · 1 copy kept as one"));
        assert_eq!(result_text(2, 3, Order::Newest, None, false), "2 of 3 parts, newest first");
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
        let (hits, counts) = view(&cat, &f, Order::Newest, None, None);
        assert_eq!(titles(&cat, &hits), ["Piano", "Kick"]);
        assert_eq!(counts.froms, [1, 2]);
        let drums = Role::ALL.iter().position(|r| *r == Role::Drums).unwrap();
        let bass = Role::ALL.iter().position(|r| *r == Role::Bass).unwrap();
        assert_eq!((counts.roles[drums], counts.roles[bass]), (1, 0));
        let c = key_list().iter().position(|k| key_words(*k) == "C").unwrap();
        assert_eq!(counts.keys[c], 1);
        let shown = chips(&counts, &f);
        assert!(shown.from.chips.iter().any(|c| c.text == "MIDI folders" && c.on && c.count == "2"));
        f.clear(FROM);
        assert!(!f.any());
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
    fn the_keyboard_runs_top_down_with_its_black_keys() {
        let keys = roll_keys(59, 63);
        let names: Vec<&str> = keys.iter().map(|k| k.name.as_str()).collect();
        assert_eq!(names, ["Eb4", "D4", "Db4", "C4", "B3"]);
        let black: Vec<bool> = keys.iter().map(|k| k.black).collect();
        assert_eq!(black, [true, false, true, false, false]);
        assert!((keys[3].y - 0.6).abs() < 1e-6 && (keys[3].h - 0.2).abs() < 1e-6);
        // The rows match the notes': C4 in a roll from 59 to 63 sits where roll() puts it.
        assert!(roll_keys(5, 4).is_empty());
        assert!(!is_black(65) && is_black(66));
    }
}
