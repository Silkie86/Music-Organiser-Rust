//! The Ctrl+K finder: one box, from anywhere, that jumps to a song, a sound, a page or an action (and, as their pages
//! land, a preset, a MIDI part, a plugin or a setting). New in the Rust app; the C# app had nothing like it.
//!
//! Each page hands the finder a [`Source`]: what it can be found by (a name, and a few more words such as a folder),
//! a line saying what it is, and an id the page opens it by. A page adds its own source when its list is read, and
//! hands it again when the list changes; nothing here knows any page's data, so a new page needs no change here.
//!
//! Finding walks every source on the low-priority pool, all cores at once (rules 12 and 21), and keeps only the best
//! few of each kind, so a one-letter search over 450,000 sounds never builds a list of 300,000.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::sync::Arc;

use chrono::NaiveDateTime;
use rayon::prelude::*;

use crate::places::Place;
use crate::shortcuts::{Action, Shortcuts};
use crate::songs::{Songs, plural, thousands};
use crate::sounds::{self as list, SoundList};

/// What a found thing is. The order is the chips' order, and breaks ties between groups that match as well.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Song,
    Sound,
    Preset,
    MidiPart,
    Plugin,
    Setting,
    Page,
    Action,
}

impl Kind {
    pub const ALL: [Kind; 8] =
        [Kind::Song, Kind::Sound, Kind::Preset, Kind::MidiPart, Kind::Plugin, Kind::Setting, Kind::Page, Kind::Action];

    /// The group's heading and its chip.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Song => "Songs",
            Kind::Sound => "Sounds",
            Kind::Preset => "Presets",
            Kind::MidiPart => "MIDI parts",
            Kind::Plugin => "Plugins",
            Kind::Setting => "Settings",
            Kind::Page => "Pages",
            Kind::Action => "Actions",
        }
    }

    /// One of them, in small letters: "12 more songs".
    fn one(self) -> &'static str {
        match self {
            Kind::Song => "song",
            Kind::Sound => "sound",
            Kind::Preset => "preset",
            Kind::MidiPart => "MIDI part",
            Kind::Plugin => "plugin",
            Kind::Setting => "setting",
            Kind::Page => "page",
            Kind::Action => "action",
        }
    }
}

/// Things of one kind the finder can find. A page makes one from its list and hands it over; it's read from the pool's
/// threads, so it holds its own copy of what it needs, or shares it in an `Arc` (the Sounds page's list).
pub trait Source: Send + Sync {
    fn kind(&self) -> Kind;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// What it's found by first: its name, as shown.
    fn name(&self, at: usize) -> &str;
    /// More words it's found by, which count for less than the name: a folder, a DAW, tags.
    fn also(&self, _at: usize) -> &str {
        ""
    }
    /// From 0 to 1: how much to lift it among things that match as well (newer songs, sounds you use more).
    fn weight(&self, _at: usize) -> f32 {
        0.0
    }
    /// The line under its name. Only made for what's shown.
    fn detail(&self, at: usize) -> String;
    /// Its key, for an action (shown at the right, like a menu line: rule 15).
    fn key(&self, _at: usize) -> String {
        String::new()
    }
    /// What its page opens it by: a song's key, a sound's id, a place's name, an action's id.
    fn id(&self, at: usize) -> String;
    /// Whether Space plays it from the finder (sounds, MIDI parts).
    fn plays(&self) -> bool {
        false
    }
    /// The DAW Ctrl+Enter opens it in (a song: its own, or Bitwig for an FL Studio song, as Songs opens it); "" for
    /// none.
    fn opens_in(&self, _at: usize) -> &str {
        ""
    }
}

/// One thing found, ready to show and to open.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub kind: Kind,
    pub title: String,
    pub detail: String,
    pub key: String,
    pub id: String,
    pub plays: bool,
    /// The DAW Ctrl+Enter opens it in, "" for none.
    pub opens_in: String,
}

/// The best things of one kind, best first, and how many matched in all.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub kind: Kind,
    pub hits: Vec<Hit>,
    pub total: usize,
    /// The best one's score, to put the best group first.
    best: i32,
}

/// What was typed, split into words, in small letters.
#[derive(Clone, Debug, Default)]
pub struct Query {
    words: Vec<Vec<char>>,
    /// Every word run together, letters and digits only: a name that's exactly this beats the rest.
    whole: Vec<char>,
}

impl Query {
    pub fn new(text: &str) -> Query {
        let words: Vec<Vec<char>> = text
            .split(|c: char| c.is_whitespace() || is_separator(c))
            .filter(|w| !w.is_empty())
            .map(|w| w.chars().flat_map(char::to_lowercase).collect())
            .collect();
        let whole = words.iter().flatten().copied().filter(|c| c.is_alphanumeric()).collect();
        Query { words, whole }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
}

/// Characters a name's words are joined by: "Kick_Hard-01.wav".
fn is_separator(c: char) -> bool {
    matches!(c, '_' | '-' | '.' | '/' | '\\' | '(' | ')' | '[' | ']' | ',' | '+' | '&')
}

/// A name in small letters, with where each of its words starts. Kept and reused while one thread scores many names,
/// so scoring 450,000 sounds doesn't make 450,000 lists.
#[derive(Default)]
struct Folded {
    chars: Vec<char>,
    starts: Vec<bool>,
}

impl Folded {
    fn fill(&mut self, text: &str) {
        self.chars.clear();
        self.starts.clear();
        let mut prev: Option<char> = None;
        for c in text.chars() {
            // A word starts after a space or a separator, at a capital after a small letter ("KickHard"), and where
            // letters turn to digits or back ("Kick01").
            let start = match prev {
                None => true,
                Some(p) => {
                    !p.is_alphanumeric()
                        || (p.is_lowercase() && c.is_uppercase())
                        || (c.is_alphanumeric() && p.is_alphabetic() != c.is_alphabetic())
                }
            };
            for (i, low) in c.to_lowercase().enumerate() {
                self.chars.push(low);
                self.starts.push(start && i == 0);
            }
            prev = Some(c);
        }
    }

    /// How well a word of the search matches: at the very start, at the start of a word, anywhere inside, or (for
    /// three letters or more, no digits) as letters in order from the start of a word, like "khrd" for "Kick Hard".
    fn word_score(&self, word: &[char], fuzzy: bool) -> Option<i32> {
        let (n, w) = (&self.chars, word.len());
        if w == 0 || w > n.len() {
            return None;
        }
        let mut best: Option<i32> = None;
        for at in 0..=n.len() - w {
            if n[at..at + w] == *word {
                let s = if at == 0 {
                    100
                } else if self.starts[at] {
                    70
                } else {
                    40
                };
                best = best.max(Some(s));
                if s == 100 {
                    break;
                }
            }
        }
        if best.is_some() || !fuzzy || w < 3 || !word.iter().all(|c| c.is_alphabetic()) {
            return best;
        }
        // Letters in order, the first at the start of a word, and close together: "khrd" finds "Kick Hard", but "bass"
        // isn't spread over a long line like "Bin, where … app removes goes".
        let first = word[0];
        let close = |from: usize| {
            let mut rest = word[1..].iter().peekable();
            for &c in &n[from + 1..n.len().min(from + 3 * w)] {
                if rest.peek() == Some(&&c) {
                    rest.next();
                }
            }
            rest.peek().is_none()
        };
        (0..n.len()).any(|i| self.starts[i] && n[i] == first && close(i)).then_some(12)
    }

    /// Whether the name, letters and digits only, is exactly `whole`.
    fn is(&self, whole: &[char]) -> bool {
        !whole.is_empty() && self.chars.iter().copied().filter(|c| c.is_alphanumeric()).eq(whole.iter().copied())
    }
}

/// How well a thing matches the search, or `None` when one of the words is in neither its name nor its other words.
fn score(q: &Query, name: &mut Folded, also: &mut Folded, source: &dyn Source, at: usize) -> Option<i32> {
    name.fill(source.name(at));
    let mut total = 0;
    let mut also_filled = false;
    for word in &q.words {
        if let Some(s) = name.word_score(word, true) {
            total += s;
            continue;
        }
        if !also_filled {
            also.fill(source.also(at));
            also_filled = true;
        }
        total += also.word_score(word, false).map(|s| if s >= 70 { 25 } else { 15 })?;
    }
    if name.is(&q.whole) {
        total += 60;
    }
    // Shorter names first among equals ("Kick" before "Kick Hard Long Tail 03"), and a nudge for the ones you use.
    total -= (name.chars.len().min(80) / 8) as i32;
    total += (source.weight(at).clamp(0.0, 1.0) * 30.0).round() as i32;
    Some(total)
}

/// The best `each` of a source, best first, and how many matched in all.
fn best_of(q: &Query, source: &dyn Source, each: usize) -> (Vec<(i32, usize)>, usize) {
    type Top = BinaryHeap<Reverse<(i32, Reverse<usize>)>>;
    fn keep(top: &mut Top, each: usize, s: i32, at: usize) {
        top.push(Reverse((s, Reverse(at))));
        if top.len() > each {
            top.pop();
        }
    }
    let one = |(mut top, mut n, mut name, mut also): (Top, usize, Folded, Folded), at: usize| {
        if let Some(s) = score(q, &mut name, &mut also, source, at) {
            keep(&mut top, each, s, at);
            n += 1;
        }
        (top, n, name, also)
    };
    let start = || (Top::new(), 0, Folded::default(), Folded::default());
    let (top, n) = if source.len() < 4096 {
        let (top, n, _, _) = (0..source.len()).fold(start(), one);
        (top, n)
    } else {
        (0..source.len()).into_par_iter().fold(start, one).map(|(top, n, _, _)| (top, n)).reduce(
            || (Top::new(), 0),
            |(mut a, n), (b, m)| {
                for Reverse((s, Reverse(at))) in b {
                    keep(&mut a, each, s, at);
                }
                (a, n + m)
            },
        )
    };
    let mut best: Vec<(i32, usize)> = top.into_iter().map(|Reverse((s, Reverse(at)))| (s, at)).collect();
    best.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    (best, n)
}

fn hit(source: &dyn Source, at: usize) -> Hit {
    Hit {
        kind: source.kind(),
        title: source.name(at).to_owned(),
        detail: source.detail(at),
        key: source.key(at),
        id: source.id(at),
        plays: source.plays(),
        opens_in: source.opens_in(at).to_owned(),
    }
}

/// Finds `text` in the sources, keeping at most `each` of every kind, the best-matching group first. `only` keeps
/// one kind. With nothing typed, it lists a kind's things in its own order (songs newest first), and with no kind
/// picked, only the pages.
pub fn find(sources: &[Arc<dyn Source>], text: &str, only: Option<Kind>, each: usize) -> Vec<Group> {
    let q = Query::new(text);
    let mut groups: Vec<Group> = Vec::new();
    for kind in Kind::ALL {
        if only.is_some_and(|k| k != kind) || (q.is_empty() && only.is_none() && kind != Kind::Page) {
            continue;
        }
        let of_kind: Vec<&Arc<dyn Source>> = sources.iter().filter(|s| s.kind() == kind).collect();
        if of_kind.is_empty() {
            continue;
        }
        // Each source's best, then the best of those across the sources of one kind.
        let mut found: Vec<(i32, usize, usize)> = Vec::new();
        let mut total = 0;
        for (i, s) in of_kind.iter().enumerate() {
            if q.is_empty() {
                found.extend((0..s.len().min(each)).map(|at| (0, i, at)));
                total += s.len();
            } else {
                let (best, n) = best_of(&q, s.as_ref(), each);
                found.extend(best.into_iter().map(|(score, at)| (score, i, at)));
                total += n;
            }
        }
        if total == 0 {
            continue;
        }
        found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        found.truncate(each);
        let best = found.first().map_or(i32::MIN, |f| f.0);
        let hits = found.into_iter().map(|(_, i, at)| hit(of_kind[i].as_ref(), at)).collect();
        groups.push(Group { kind, hits, total, best });
    }
    // The best match's group first; groups that match as well stay in the chips' order.
    groups.sort_by(|a, b| match b.best.cmp(&a.best) {
        Ordering::Equal => a.kind.cmp(&b.kind),
        o => o,
    });
    groups
}

/// The kinds there's something to find of, in the chips' order.
pub fn kinds(sources: &[Arc<dyn Source>]) -> Vec<Kind> {
    Kind::ALL.into_iter().filter(|k| sources.iter().any(|s| s.kind() == *k && !s.is_empty())).collect()
}

/// The kind Tab (or Shift+Tab, `back`) moves to from `now`: All, then each kind, then All again.
pub fn next_kind(kinds: &[Kind], now: Option<Kind>, back: bool) -> Option<Kind> {
    let at = now.and_then(|k| kinds.iter().position(|x| *x == k));
    let n = kinds.len() + 1;
    // Place 0 is All; the kinds follow.
    let place = at.map_or(0, |i| i + 1);
    let next = if back { (place + n - 1) % n } else { (place + 1) % n };
    next.checked_sub(1).and_then(|i| kinds.get(i).copied())
}

/// How many of each kind show when every kind shows, and when one does.
pub const EACH: usize = 4;
pub const EACH_OF_ONE: usize = 200;

/// How many of each kind to find: the best few of every kind, many more of one kind, and every place (but not their
/// views) before anything is typed.
pub fn each(text: &str, only: Option<Kind>) -> usize {
    match only {
        Some(_) => EACH_OF_ONE,
        None if text.trim().is_empty() => Place::ALL.len(),
        None => EACH,
    }
}

/// The things you jumped to lately, newest first, shown before anything is typed.
#[derive(Clone, Debug, Default)]
pub struct Recent {
    hits: Vec<Hit>,
}

impl Recent {
    const MOST: usize = 6;

    pub fn add(&mut self, hit: Hit) {
        self.hits.retain(|h| !(h.kind == hit.kind && h.id == hit.id));
        self.hits.insert(0, hit);
        self.hits.truncate(Recent::MOST);
    }

    pub fn hits(&self) -> &[Hit] {
        &self.hits
    }
}

/// One line of the finder's list.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Heading(String),
    Hit(Hit),
    /// "12 more songs": picking it shows only that kind.
    More(Kind, String),
    /// A quiet word under a kind's best few: "The best 200 of 4,512 songs. Type more to narrow it down."
    Note(String),
}

/// The finder's lines: each group under its heading, with a "more" line when there are more than it shows. Before
/// anything is typed (and with every kind showing), the things you jumped to lately come first.
pub fn lines(groups: &[Group], recent: &Recent, typed: bool, only: Option<Kind>) -> Vec<Line> {
    let mut out = Vec::new();
    if !typed && only.is_none() && !recent.hits().is_empty() {
        out.push(Line::Heading("Recent".to_owned()));
        out.extend(recent.hits().iter().cloned().map(Line::Hit));
    }
    for g in groups {
        out.push(Line::Heading(g.kind.name().to_owned()));
        out.extend(g.hits.iter().cloned().map(Line::Hit));
        let left = g.total.saturating_sub(g.hits.len());
        // Before anything is typed, the places are the start and their views wait for a search.
        if left > 0 && (typed || only.is_some()) {
            if only.is_some() {
                out.push(Line::Note(format!(
                    "The best {} of {}. Type more to narrow it down.",
                    g.hits.len(),
                    plural(g.total, g.kind.one())
                )));
            } else {
                let what = if left == 1 { g.kind.one().to_owned() } else { format!("{}s", g.kind.one()) };
                out.push(Line::More(g.kind, format!("{} more {what}", thousands(left))));
            }
        }
    }
    out
}

/// What the finder says when nothing matches.
pub fn nothing_found(text: &str, only: Option<Kind>) -> String {
    match only {
        Some(k) => format!("No {}s match “{}”.", k.one(), text.trim()),
        None => format!("Nothing matches “{}”.", text.trim()),
    }
}

/// The keys line at the foot of the finder, for what's picked: Ctrl+Enter only when it'd do something, and Space once
/// the arrows have `moved` to something it plays (before that, Space is a space in the box).
pub fn hints(picked: Option<&Hit>, moved: bool) -> String {
    let mut parts = vec!["↑↓ move", "Enter open"];
    let daw = picked.filter(|h| !h.opens_in.is_empty()).map(|h| format!("Ctrl+Enter open in {}", h.opens_in));
    if let Some(daw) = &daw {
        parts.push(daw);
    }
    if moved && picked.is_some_and(|h| h.plays) {
        parts.push("Space play");
    }
    parts.extend(["Tab one kind", "Esc close"]);
    parts.join("    ")
}

/// A source made of lines kept in a list: the pages, the actions, the songs, and any page whose list is small.
#[derive(Clone, Debug, Default)]
pub struct Lines {
    pub kind: Option<Kind>,
    pub lines: Vec<Found>,
    pub plays: bool,
    /// Each line's DAW for Ctrl+Enter ([`Source::opens_in`]), in the same order; empty when none opens in one.
    pub opens_in: Vec<String>,
}

/// One thing of a [`Lines`] source.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub name: String,
    pub also: String,
    pub detail: String,
    pub key: String,
    pub id: String,
    pub weight: f32,
}

impl Source for Lines {
    fn kind(&self) -> Kind {
        self.kind.unwrap_or(Kind::Page)
    }
    fn len(&self) -> usize {
        self.lines.len()
    }
    fn name(&self, at: usize) -> &str {
        self.lines.get(at).map_or("", |l| &l.name)
    }
    fn also(&self, at: usize) -> &str {
        self.lines.get(at).map_or("", |l| &l.also)
    }
    fn weight(&self, at: usize) -> f32 {
        self.lines.get(at).map_or(0.0, |l| l.weight)
    }
    fn detail(&self, at: usize) -> String {
        self.lines.get(at).map(|l| l.detail.clone()).unwrap_or_default()
    }
    fn key(&self, at: usize) -> String {
        self.lines.get(at).map(|l| l.key.clone()).unwrap_or_default()
    }
    fn id(&self, at: usize) -> String {
        self.lines.get(at).map(|l| l.id.clone()).unwrap_or_default()
    }
    fn plays(&self) -> bool {
        self.plays
    }
    fn opens_in(&self, at: usize) -> &str {
        self.opens_in.get(at).map_or("", String::as_str)
    }
}

/// The id of a view of a page: "Sounds/Galaxy". A place on its own is just its name.
pub fn view_id(place: Place, view: &str) -> String {
    format!("{}/{view}", place.name())
}

/// The place and view (if any) a page's id stands for.
pub fn page_of(id: &str) -> Option<(Place, Option<&str>)> {
    let (name, view) = match id.split_once('/') {
        Some((n, v)) => (n, Some(v)),
        None => (id, None),
    };
    Place::ALL.into_iter().find(|p| p.name() == name).map(|p| (p, view))
}

/// Every place, with its key, then the views each page names (`views`: the Sounds page's Tiles, List, Galaxy…).
pub fn pages(shortcuts: &Shortcuts, views: &[(Place, &[&str])]) -> Lines {
    let mut lines: Vec<Found> = Place::ALL
        .into_iter()
        .map(|p| Found {
            name: p.name().to_owned(),
            also: also_of_place(p).to_owned(),
            detail: "Page".to_owned(),
            key: shortcuts.key_of(p.shortcut_id()).to_owned(),
            id: p.name().to_owned(),
            // A page beats a song or sound of the same name: typing "sounds" means the page.
            weight: 1.0,
        })
        .collect();
    for (place, names) in views {
        lines.extend(names.iter().map(|v| Found {
            name: format!("{} › {v}", place.name()),
            also: v.to_string(),
            detail: format!("A view of {}", place.name()),
            key: String::new(),
            id: view_id(*place, v),
            weight: 0.8,
        }));
    }
    Lines { kind: Some(Kind::Page), lines, ..Lines::default() }
}

/// Other words a place is found by: the C# app's names for it, and what it holds.
fn also_of_place(p: Place) -> &'static str {
    match p {
        Place::Songs => "projects saves",
        Place::Sounds => "sound lab samples",
        Place::Presets => "patches curves",
        Place::Midi => "playground clips",
        Place::Plugins => "vst clap",
        Place::Space => "disk drives storage clean",
        Place::Backup => "copy drive",
        Place::Settings => "options preferences",
    }
}

/// Actions, each with the key it has now. Going to a place is left out: the pages cover it.
pub fn actions<'a>(shortcuts: &Shortcuts, list: impl IntoIterator<Item = &'a Action>) -> Lines {
    let lines = list
        .into_iter()
        .filter(|a| !a.id.starts_with("app.go"))
        .map(|a| Found {
            name: a.what.to_owned(),
            also: String::new(),
            detail: "Action".to_owned(),
            key: shortcuts.key_of(a.id).to_owned(),
            id: a.id.to_owned(),
            weight: 0.5,
        })
        .collect();
    Lines { kind: Some(Kind::Action), lines, ..Lines::default() }
}

/// Every song, newest save first, found by its name and by its save's file name, folder, DAW and tags.
pub fn songs(songs: &Songs, now: NaiveDateTime) -> Lines {
    let n = songs.rows.len().max(1) as f32;
    let opens_in = songs
        .rows
        .iter()
        .map(|r| if r.song.latest.daw == "FL Studio" { "Bitwig".to_owned() } else { r.song.latest.daw.clone() })
        .collect();
    let lines = songs
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let l = &r.song.latest;
            let mut also = vec![l.name.as_str(), l.folder.as_str(), l.daw.as_str()];
            also.extend(r.tags.iter().map(String::as_str));
            let mut detail = r.details_line();
            let date = r.short_date(now);
            if !date.is_empty() {
                detail = format!("{detail} · {date}");
            }
            Found {
                name: r.song.name.clone(),
                also: also.join(" "),
                detail,
                key: String::new(),
                id: r.song.key.clone(),
                // Newer songs and favourites first among equals.
                weight: (1.0 - i as f32 / n) * 0.6 + if r.favourite { 0.4 } else { 0.0 },
            }
        })
        .collect();
    Lines { kind: Some(Kind::Song), lines, plays: false, opens_in }
}

/// Every sound the Sounds page lists, shared with it rather than copied: found by its name and its folder.
pub struct Sounds(pub Arc<SoundList>);

impl Source for Sounds {
    fn kind(&self) -> Kind {
        Kind::Sound
    }
    fn len(&self) -> usize {
        self.0.len()
    }
    fn name(&self, at: usize) -> &str {
        self.0.sound(at).map_or("", |s| s.name())
    }
    fn also(&self, at: usize) -> &str {
        self.0.sound(at).map_or("", |s| s.folder())
    }
    fn weight(&self, at: usize) -> f32 {
        let used = self.0.used_in(at).map_or(0.0, |n| (n as f32 / 5.0).min(1.0));
        let liked = self.0.liked_share(at).unwrap_or(0.0) as f32;
        used.max(liked)
    }
    fn detail(&self, at: usize) -> String {
        let Some(s) = self.0.sound(at) else { return String::new() };
        let mut parts = vec![list::facts_line(s)];
        parts.retain(|p| !p.is_empty());
        if !s.folder().is_empty() {
            parts.push(format!("in {}", s.folder()));
        }
        if let Some(n) = self.0.used_in(at).filter(|n| *n > 0) {
            parts.push(format!("used in {}", plural(n as usize, "project")));
        }
        parts.join(" · ")
    }
    fn id(&self, at: usize) -> String {
        self.0.sound(at).map(|s| s.id.to_string()).unwrap_or_default()
    }
    fn plays(&self) -> bool {
        true
    }
}

/// Every plugin the Plugins page lists, found by its name, its maker and its format. Its id is its line's name key and
/// own ([`plugin_of`] splits it), so the page can find it again after a fresh read.
pub fn plugins(plugins: &crate::plugins::Plugins) -> Lines {
    let most = plugins.rows.iter().map(|r| r.songs).max().unwrap_or(0).max(1) as f32;
    let lines = plugins
        .rows
        .iter()
        .map(|r| {
            let songs = match r.songs {
                0 => "No song uses it".to_owned(),
                n => format!("used in {}", plural(n, "song")),
            };
            let own = if r.own.is_empty() { String::new() } else { format!("{}'s own", r.own) };
            let installed = if r.installed { "" } else { "not installed here" };
            let detail = [r.vendor.as_str(), r.format.as_str(), own.as_str(), songs.as_str(), installed]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            Found {
                name: r.name.clone(),
                also: [r.vendor.as_str(), r.format.as_str(), r.own.as_str()].join(" "),
                detail,
                key: String::new(),
                id: format!("{}\u{1f}{}", r.key, r.own),
                // The plugins your songs use most first among equals.
                weight: r.songs as f32 / most,
            }
        })
        .collect();
    Lines { kind: Some(Kind::Plugin), lines, ..Lines::default() }
}

/// A plugin's name key and own, from its id in [`plugins`].
pub fn plugin_of(id: &str) -> (&str, &str) {
    id.split_once('\u{1f}').unwrap_or((id, ""))
}

/// Every sound and preset file the Presets page lists, shared with it rather than copied: found by its name, then its
/// plugin. Its id is the line's song save and sound key, or the preset file ([`crate::presets::audition_id`]), so a pick
/// kept in Recent still opens the same sound after the page reads its lines again in another order.
pub struct Presets(pub Arc<Vec<crate::presets::Item>>);

impl Source for Presets {
    fn kind(&self) -> Kind {
        Kind::Preset
    }
    fn len(&self) -> usize {
        self.0.len()
    }
    fn name(&self, at: usize) -> &str {
        self.0.get(at).map_or("", |i| i.row.name.as_str())
    }
    fn also(&self, at: usize) -> &str {
        self.0.get(at).map_or("", |i| i.row.plugin.as_str())
    }
    fn detail(&self, at: usize) -> String {
        let Some(i) = self.0.get(at) else { return String::new() };
        let from = if i.is_file() { "Preset file".to_owned() } else { format!("in {}", i.song) };
        [i.row.plugin.as_str(), from.as_str(), i.row.place.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }
    fn id(&self, at: usize) -> String {
        self.0.get(at).map(crate::presets::audition_id).unwrap_or_default()
    }
}

/// Every MIDI part the MIDI page lists, found by its title, then where it's from. Its id is its notes' fingerprint, so
/// the page finds it again after the index is read again. Parts under `smallest` notes are left out, as the page hides
/// them (Settings' smallest part).
pub fn midi_parts(cat: &mo_midi::Catalogue, smallest: usize) -> Lines {
    // A line each, as the MIDI page lists them: the same loop in other keys once, found by those files' names too, as
    // the page's own search finds them ("Pad Loop F#m" when "Pad Loop Am" heads the group).
    let mut others: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    for (at, e) in cat.entries.iter().enumerate() {
        if let Some(head) = e.key_group.filter(|&g| g != at) {
            let words = others.entry(head).or_default();
            for name in [crate::midi::title(e).as_str(), e.source_name()] {
                words.push(' ');
                words.push_str(name);
            }
        }
    }
    let lines = cat
        .entries
        .iter()
        .enumerate()
        .filter(|(at, e)| cat.is_line(*at) && crate::midi::big_enough(e, smallest))
        .map(|(at, e)| {
            let mut from = crate::midi::from_text(e);
            let details = crate::midi::details(e);
            Found {
                name: crate::midi::title(e),
                detail: [details.as_str(), from.as_str()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
                also: {
                    if let Some(words) = others.remove(&at) {
                        from.push_str(&words);
                    }
                    from
                },
                key: String::new(),
                id: e.phrase.hash.clone(),
                // Notes found in many files first among equals.
                weight: ((e.copies() as f32 - 1.0) / 10.0).clamp(0.0, 1.0),
            }
        })
        .collect();
    Lines { kind: Some(Kind::MidiPart), lines, ..Lines::default() }
}

/// The Settings page's tabs, each found by what's on it too ("theme" finds General). The id is the tab's name.
pub fn settings() -> Lines {
    use crate::settings::Tab;
    let lines = Tab::ALL
        .into_iter()
        .map(|t| Found {
            name: format!("Settings › {}", t.name()),
            also: format!("{} {}", t.name(), settings_words(t)),
            detail: t.summary().to_owned(),
            key: String::new(),
            id: t.name().to_owned(),
            weight: 0.6,
        })
        .collect();
    Lines { kind: Some(Kind::Setting), lines, ..Lines::default() }
}

/// What's set on a Settings tab, in words you might type for it.
fn settings_words(t: crate::settings::Tab) -> &'static str {
    use crate::settings::Tab;
    match t {
        Tab::General => "theme colour accent font text size start page sound output pace speed pointer",
        Tab::Folders => "folders projects samples sounds midi plugins add remove",
        Tab::Daws => "bitwig fl studio script extension link install",
        Tab::Shortcuts => "keys keyboard hotkeys",
        Tab::Developer => "version logs files",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shortcuts::APP_ACTIONS;

    fn lines(kind: Kind, names: &[&str]) -> Arc<dyn Source> {
        Arc::new(Lines {
            kind: Some(kind),
            lines: names.iter().map(|n| Found { name: n.to_string(), id: n.to_string(), ..Found::default() }).collect(),
            ..Lines::default()
        })
    }

    fn titles(g: &Group) -> Vec<&str> {
        g.hits.iter().map(|h| h.title.as_str()).collect()
    }

    #[test]
    fn the_start_of_a_name_beats_the_start_of_a_word_beats_inside() {
        let s = lines(Kind::Sound, &["Big Kick", "Kickback", "Snare kick 2", "Sidekick", "Hat"]);
        let g = find(&[s], "kick", None, 10);
        assert_eq!(g.len(), 1);
        assert_eq!(titles(&g[0]), ["Kickback", "Big Kick", "Snare kick 2", "Sidekick"]);
        assert_eq!(g[0].total, 4);
    }

    #[test]
    fn every_word_must_match_in_any_order() {
        let s = lines(Kind::Sound, &["Kick_Hard_01", "Kick Soft", "Hard Snare"]);
        let g = find(&[s], "hard kick", None, 10);
        assert_eq!(titles(&g[0]), ["Kick_Hard_01"]);
    }

    #[test]
    fn letters_in_order_find_a_name_but_count_for_less() {
        let s = lines(Kind::Sound, &["Kick Hard", "khrd loop"]);
        let g = find(&[s], "khrd", None, 10);
        assert_eq!(titles(&g[0]), ["khrd loop", "Kick Hard"]);
        // Letters spread over a long name don't count.
        assert!(
            find(&[lines(Kind::Action, &["Open the Recycle Bin, where everything goes"])], "bass", None, 10).is_empty()
        );
        // Two letters are too few to guess at.
        assert!(find(&[lines(Kind::Sound, &["Kick Hard"])], "kh", None, 10).is_empty());
    }

    #[test]
    fn words_join_at_capitals_and_digits() {
        let mut f = Folded::default();
        f.fill("KickHard01");
        assert_eq!(f.word_score(&['h', 'a', 'r', 'd'], false), Some(70));
        assert_eq!(f.word_score(&['0', '1'], false), Some(70));
        assert_eq!(f.word_score(&['i', 'c', 'k'], false), Some(40));
    }

    #[test]
    fn a_name_that_is_exactly_the_search_comes_first() {
        let s = lines(Kind::Song, &["Night Drive Extended", "Night Drive", "Nightdrive"]);
        let g = find(&[s], "night drive", None, 10);
        assert_eq!(titles(&g[0])[..2], ["Night Drive", "Nightdrive"]);
    }

    #[test]
    fn other_words_find_it_too_but_below_the_name() {
        let s: Arc<dyn Source> = Arc::new(Lines {
            kind: Some(Kind::Sound),
            lines: vec![
                Found { name: "Thump".into(), also: "Kicks".into(), id: "1".into(), ..Found::default() },
                Found { name: "Kick 1".into(), also: "Drums".into(), id: "2".into(), ..Found::default() },
            ],
            plays: true,
            opens_in: Vec::new(),
        });
        let g = find(&[s], "kick", None, 10);
        assert_eq!(titles(&g[0]), ["Kick 1", "Thump"]);
        assert!(g[0].hits[0].plays);
    }

    #[test]
    fn the_best_group_comes_first_and_each_keeps_its_best_few() {
        let pages = lines(Kind::Page, &["Sounds", "Songs"]);
        let sounds = lines(Kind::Sound, &["Sounds of rain", "Rain sounds 1", "Rain sounds 2", "Rain sounds 3"]);
        let g = find(&[sounds.clone(), pages.clone()], "sounds", None, 2);
        assert_eq!(g.iter().map(|g| g.kind).collect::<Vec<_>>(), [Kind::Page, Kind::Sound]);
        assert_eq!(titles(&g[1]).len(), 2);
        assert_eq!(g[1].total, 4);
        // One kind only.
        let g = find(&[sounds, pages], "sounds", Some(Kind::Sound), 200);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].kind, Kind::Sound);
    }

    #[test]
    fn nothing_typed_lists_pages_or_a_kind_in_its_own_order() {
        let pages = lines(Kind::Page, &["Songs", "Sounds"]);
        let songs = lines(Kind::Song, &["Newest", "Older", "Oldest"]);
        let g = find(&[songs.clone(), pages.clone()], "  ", None, 4);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].kind, Kind::Page);
        let g = find(&[songs, pages], "", Some(Kind::Song), 2);
        assert_eq!(titles(&g[0]), ["Newest", "Older"]);
        assert_eq!(g[0].total, 3);
    }

    #[test]
    fn a_big_source_is_searched_on_every_core_with_the_same_answer() {
        let names: Vec<String> = (0..20_000).map(|i| format!("Loop {i:05}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let big = lines(Kind::Sound, &refs);
        let g = find(&[big], "loop 1999", None, 3);
        // 19990 to 19999 start a word; 01999 and 11999 hold it inside.
        assert_eq!(g[0].total, 12);
        assert_eq!(titles(&g[0]), ["Loop 19990", "Loop 19991", "Loop 19992"]);
    }

    #[test]
    fn tab_steps_through_the_kinds_and_back_to_all() {
        let kinds = [Kind::Song, Kind::Sound, Kind::Page];
        assert_eq!(next_kind(&kinds, None, false), Some(Kind::Song));
        assert_eq!(next_kind(&kinds, Some(Kind::Song), false), Some(Kind::Sound));
        assert_eq!(next_kind(&kinds, Some(Kind::Page), false), None);
        assert_eq!(next_kind(&kinds, None, true), Some(Kind::Page));
        assert_eq!(next_kind(&kinds, Some(Kind::Song), true), None);
        assert_eq!(next_kind(&[], None, false), None);
    }

    #[test]
    fn lines_put_recent_first_and_say_how_many_more() {
        let mut recent = Recent::default();
        let hit = |t: &str| Hit {
            kind: Kind::Song,
            title: t.into(),
            detail: String::new(),
            key: String::new(),
            id: t.into(),
            plays: false,
            opens_in: "Bitwig".into(),
        };
        recent.add(hit("A"));
        recent.add(hit("B"));
        recent.add(hit("A"));
        assert_eq!(recent.hits().iter().map(|h| h.title.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        let songs = lines(Kind::Song, &["Alpha", "Alps", "Altitude"]);
        let g = find(&[songs], "al", None, 2);
        let l = super::lines(&g, &recent, true, None);
        assert_eq!(l.first(), Some(&Line::Heading("Songs".into())));
        assert_eq!(l.last(), Some(&Line::More(Kind::Song, "1 more song".into())));
        let l = super::lines(&g, &recent, false, None);
        assert_eq!(l.first(), Some(&Line::Heading("Recent".into())));
        // Before anything is typed, no "more" line under the places.
        let pages = lines(Kind::Page, &["Songs", "Sounds", "Sounds › Galaxy"]);
        let g2 = find(&[pages], "", None, 2);
        assert!(!super::lines(&g2, &recent, false, None).iter().any(|l| matches!(l, Line::More(..))));
        assert_eq!(each("", None), Place::ALL.len());
        assert_eq!(each("k", None), EACH);
        assert_eq!(each("", Some(Kind::Song)), EACH_OF_ONE);
        let l = super::lines(&g, &recent, true, Some(Kind::Song));
        assert!(matches!(l.last(), Some(Line::Note(n)) if n.starts_with("The best 2 of 3 songs.")));
    }

    #[test]
    fn pages_and_their_views_open_by_id() {
        let s = Shortcuts::default();
        let p = pages(&s, &[(Place::Sounds, &["Tiles", "Galaxy"])]);
        let galaxy = p.lines.iter().find(|l| l.name == "Sounds › Galaxy").map(|l| l.id.clone());
        assert_eq!(galaxy.as_deref(), Some("Sounds/Galaxy"));
        assert_eq!(page_of("Sounds/Galaxy"), Some((Place::Sounds, Some("Galaxy"))));
        assert_eq!(page_of("MIDI"), Some((Place::Midi, None)));
        assert_eq!(page_of("Nowhere"), None);
        assert_eq!(p.lines[1].key, "Ctrl+2");
        // The C# app's name for a page finds it.
        let g = find(&[Arc::new(p) as Arc<dyn Source>], "sound lab", None, 4);
        assert_eq!(g[0].hits[0].title, "Sounds");
    }

    #[test]
    fn actions_show_their_key_and_leave_out_going_to_a_page() {
        let s = Shortcuts::default();
        let a = actions(&s, APP_ACTIONS);
        assert!(a.lines.iter().all(|l| !l.id.starts_with("app.go")));
        let full = a.lines.iter().find(|l| l.id == "app.fullScreen").map(|l| l.key.as_str());
        assert_eq!(full, Some("F11"));
        let g = find(&[Arc::new(a) as Arc<dyn Source>], "full screen", None, 4);
        assert_eq!(g[0].hits[0].id, "app.fullScreen");
        assert_eq!(g[0].hits[0].key, "F11");
    }

    #[test]
    fn the_keys_line_offers_only_what_works() {
        assert_eq!(hints(None, true), "↑↓ move    Enter open    Tab one kind    Esc close");
        let song = Hit {
            kind: Kind::Song,
            title: "A".into(),
            detail: String::new(),
            key: String::new(),
            id: "a".into(),
            plays: false,
            opens_in: "Bitwig".into(),
        };
        assert!(hints(Some(&song), true).contains("Ctrl+Enter open in Bitwig"));
        assert!(!hints(Some(&song), true).contains("Space"));
        let sound = Hit { plays: true, opens_in: String::new(), ..song };
        assert!(hints(Some(&sound), true).contains("Space play"));
        assert!(!hints(Some(&sound), false).contains("Space"));
    }

    #[test]
    fn nothing_found_says_what_was_looked_for() {
        assert_eq!(nothing_found(" zzz ", None), "Nothing matches “zzz”.");
        assert_eq!(nothing_found("zzz", Some(Kind::Sound)), "No sounds match “zzz”.");
    }

    #[test]
    fn a_settings_tab_is_found_by_what_is_on_it() {
        let g = find(&[Arc::new(settings()) as Arc<dyn Source>], "theme", None, 4);
        assert_eq!(g[0].kind, Kind::Setting);
        assert_eq!(g[0].hits[0].id, "General");
        let g = find(&[Arc::new(settings()) as Arc<dyn Source>], "bitwig", None, 4);
        assert_eq!(g[0].hits[0].id, "DAWs");
    }

    #[test]
    fn a_plugins_id_gives_back_its_key_and_own() {
        assert_eq!(plugin_of("serum\u{1f}"), ("serum", ""));
        assert_eq!(plugin_of("arpeggiator\u{1f}Bitwig"), ("arpeggiator", "Bitwig"));
        assert_eq!(plugin_of("old"), ("old", ""));
    }
}
