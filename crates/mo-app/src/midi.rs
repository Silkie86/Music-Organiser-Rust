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

/// Where the parts come from, as the From chip names it.
pub fn from_name(from: Option<From>) -> &'static str {
    match from {
        None => "",
        Some(From::Projects) => "Your projects",
        Some(From::Files) => "MIDI files",
    }
}

pub fn from_of_name(name: &str) -> Option<From> {
    [From::Projects, From::Files].into_iter().find(|f| from_name(Some(*f)) == name)
}

/// What the page shows: the filters and what the list is ranked against.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    /// Words that must all appear (names, folders, kind, instrument, key).
    pub text: String,
    /// Only these kinds of part (none picked: every kind).
    pub roles: Vec<Role>,
    pub from: Option<From>,
    /// Only parts the key finder puts in this key (one of a pair counts too, and so does a home note's tonic).
    pub key: Option<KeyName>,
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

    /// Whether anything narrows the list.
    pub fn any(&self) -> bool {
        !self.text.trim().is_empty()
            || !self.roles.is_empty()
            || self.from.is_some()
            || self.key.is_some()
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

/// The parts the filters let through, in order, as hits in the catalogue. Runs on every core; call it off the
/// window's thread (rules 12 and 21). `seed` shuffles them (a random order this number decides).
pub fn view(cat: &Catalogue, f: &Filters, order: Order, target: Option<Target>, seed: Option<u64>) -> Vec<Hit> {
    let q = Query {
        text: f.text.clone(),
        // One kind is the catalogue's own cheap test; several are tested below.
        role: if f.roles.len() == 1 { f.roles.first().copied() } else { None },
        from: f.from,
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
    let (hits, _) = cat.search(&q);
    let mut hits: Vec<Hit> = hits
        .into_par_iter()
        .filter(|h| {
            cat.entry(h.entry).is_some_and(|e| {
                (f.roles.is_empty() || f.roles.contains(&e.role()))
                    && f.key.is_none_or(|k| in_key(e, k))
                    && f.years.is_none_or(|(a, b)| (a..=b).contains(&year_of(e.phrase.source.mtime)))
            })
        })
        .collect();
    // Ranked by likeness, or the longest phrase first: that order is the point.
    if target.is_none() {
        sort(cat, &mut hits, order);
    }
    if let Some(seed) = seed {
        shuffle(&mut hits, seed);
    }
    hits
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
        let k = fact.answer()?.leaning_key()?;
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

/// The Key chip's choices: every key, majors then minors, as the list writes them ("Eb", "C#m").
pub fn key_choices() -> Vec<String> {
    let mut keys: Vec<KeyName> = KeyName::ALL.to_vec();
    keys.sort_by_key(|k| (k.mode == Mode::Minor, k.tonic.number()));
    keys.into_iter().map(KeyName::short).collect()
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
    let Some(answer) = fact.and_then(KeyFact::answer) else {
        let why = if drums { "Drums have no key." } else { "There are no pitched notes to read a key from." };
        return KeyCard { answer: "No key".to_owned(), sure: String::new(), why: why.to_owned() };
    };
    let fact = fact.copied().unwrap_or(KeyFact([0; 8]));
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
/// (0.4 to 1, for how strongly it's drawn).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RollNote {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub strength: f32,
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

    fn titles(cat: &Catalogue, hits: &[Hit]) -> Vec<String> {
        hits.iter().map(|h| title(cat.entry(h.entry).unwrap())).collect()
    }

    #[test]
    fn kinds_can_be_picked_together() {
        let cat = catalogue();
        let mut f = Filters::default();
        assert_eq!(view(&cat, &f, Order::Newest, None, None).len(), 3);
        f.toggle_role(Role::Bass);
        assert_eq!(titles(&cat, &view(&cat, &f, Order::Newest, None, None)), ["Bass"]);
        f.toggle_role(Role::Drums);
        assert_eq!(titles(&cat, &view(&cat, &f, Order::Newest, None, None)), ["Bass", "Kick"]);
        f.toggle_role(Role::Bass);
        assert_eq!(f.roles, [Role::Drums]);
        assert!(f.any());
    }

    #[test]
    fn key_from_and_years_narrow_it() {
        let cat = catalogue();
        let f = Filters { key: key_of_choice("Am"), ..Filters::default() };
        assert_eq!(titles(&cat, &view(&cat, &f, Order::Newest, None, None)), ["Bass"]);
        let f = Filters { from: Some(From::Files), ..Filters::default() };
        assert_eq!(titles(&cat, &view(&cat, &f, Order::Newest, None, None)), ["Piano", "Kick"]);
        let y = year_of(1_700_000_000);
        let f = Filters { years: Some((y, y)), ..Filters::default() };
        assert_eq!(titles(&cat, &view(&cat, &f, Order::Newest, None, None)), ["Kick"]);
        assert!(year_counts(&cat).first().is_some_and(|(year, n)| *year == year_of(1_760_000_000) && *n == 2));
    }

    #[test]
    fn orders() {
        let cat = catalogue();
        let f = Filters::default();
        assert_eq!(titles(&cat, &view(&cat, &f, Order::Bars, None, None)), ["Piano", "Bass", "Kick"]);
        assert_eq!(titles(&cat, &view(&cat, &f, Order::Copies, None, None)), ["Bass", "Piano", "Kick"]);
        // C major, then A minor (its relative, the same place round the circle); no key last.
        assert_eq!(titles(&cat, &view(&cat, &f, Order::Key, None, None)), ["Piano", "Bass", "Kick"]);
        assert_eq!(titles(&cat, &view(&cat, &f, Order::Name, None, None)), ["Bass", "Kick", "Piano"]);
        // A shuffle keeps every part and is the same for the same seed.
        let one = view(&cat, &f, Order::Newest, None, Some(7));
        assert_eq!(one, view(&cat, &f, Order::Newest, None, Some(7)));
        assert_eq!(one.len(), 3);
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
    }
}
