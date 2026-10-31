//! Works out what a MIDI part is: chords, a melody, a bassline, an arp or drums (the C# `PhraseAnalyser`). Its notes
//! decide (how many sound at once, how low, how fast and even), with names as a hint: a part called "Bass" or a file
//! in a "Chords" folder. It's plain counting, so it's quick enough for hundreds of thousands of parts, and it says why
//! it decided.
//!
//! Every number here is worked out as the C# app works it out, down to the order floats are added in, so a part read
//! by both apps gets the same kind, shape and hash (the twin test checks this against your `midi.db`). The key and
//! chords come from `mo-harmony` (phase 6): until then they're given by the caller, or left out.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use mo_formats::NoteEvent;
use regex::Regex;
use sha2::{Digest, Sha256};

use crate::chord::PhraseChord;
use crate::index::Key;
use crate::roles::Role;
use crate::{BAR, PPQ};

/// Parts longer than this are cut into phrases of 4 to 16 bars, where the music breathes.
pub const MAX_BARS: i64 = 16;
/// The most notes kept in one phrase.
pub const MAX_NOTES: usize = 4096;
/// How many numbers a part's shape has ([`Analysis::features`]).
pub const FEATURE_COUNT: usize = 48;
/// No note is held longer than this: 1,024 bars, over half an hour at 120 BPM, so a real note (an 80-bar pad) is
/// never cut. Some FL Studio 6 patterns say a note lasts about two billion ticks.
pub const MAX_HOLD: i64 = 1024 * BAR;
/// This many empty bars or more inside a part split it into sections, each kept on its own.
pub const GAP_BARS: i64 = 4;
/// A part taken for a melody or bassline with at least this share of its onsets struck as chords still has its
/// chords read.
pub const CHORDS_IN_A_LINE: f64 = 0.25;

/// What the page knows about one part, worked out from its notes and names.
#[derive(Clone, Debug, PartialEq)]
pub struct Analysis {
    pub role: Role,
    /// Why, in a few words, for the tooltip.
    pub role_why: String,
    pub bars: i64,
    pub lowest: u8,
    pub highest: u8,
    pub key: Option<Key>,
    /// How long each pitch class sounds, as shares adding up to 1.
    pub chroma: [f32; 12],
    pub chords: Vec<PhraseChord>,
    /// The part's shape, the same whatever key it's in, for "more like this".
    pub features: [f32; FEATURE_COUNT],
    /// The same notes give the same hash, so a pattern saved in 20 versions of a song is shown once.
    pub hash: String,
}

impl Analysis {
    /// Whether the harmony engine should read chords from this part: chords, arps and other parts, and a melody or
    /// bassline that strikes chords a good share of the time (a comping piano with a busy top line).
    pub fn wants_chords(role: Role, shape: &Shape) -> bool {
        matches!(role, Role::Chords | Role::Arp | Role::Other)
            || (role != Role::Drums && shape.chord_share >= CHORDS_IN_A_LINE)
    }
}

/// The notes at [`PPQ`], muted ones left out, a step-sequencer note given a sixteenth, moved so the phrase starts in its
/// first bar (a pattern that starts on bar 9 starts on bar 1 here). With `keep_place`, they stay where they are.
pub fn normalise<'a>(notes: impl IntoIterator<Item = &'a NoteEvent>, ppq: i64, keep_place: bool) -> Vec<NoteEvent> {
    let scale = PPQ as f64 / ppq.max(1) as f64;
    let mut list: Vec<NoteEvent> = notes
        .into_iter()
        .filter(|n| !n.muted)
        .map(|n| {
            let length = if n.step { ppq as f64 / 4.0 } else { n.length as f64 };
            NoteEvent {
                start: round(n.start as f64 * scale),
                length: round(length * scale).clamp(1, MAX_HOLD),
                step: false,
                ..*n
            }
        })
        .collect();
    // Stable, as the C# OrderBy … ThenBy.
    list.sort_by_key(|n| (n.start, n.key));
    if list.is_empty() || keep_place {
        return list;
    }
    // To the bar it starts in, counted down: a note pushed in before tick 0 (-48) starts in the bar before.
    let first = list.first().map_or(0, |n| n.start);
    let shift = first - first.rem_euclid(BAR);
    if shift != 0 {
        for n in &mut list {
            n.start -= shift;
        }
    }
    list
}

/// .NET's `(long)Math.Round(x)`: to the nearest, a half to the even one.
fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// How many bars the notes take, from tick 0: at least 1, and 0 for no notes.
pub fn bars_of(notes: &[NoteEvent]) -> i64 {
    let Some(end) = notes.iter().map(|n| n.start + n.length).max() else { return 0 };
    ((end as f64 / BAR as f64 - 0.01).ceil() as i64).max(1)
}

/// One phrase of a part, from [`split`].
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    /// Where it sits in the part ("bars 9–16", "bar 3"), or "" when it's the whole part.
    pub place: String,
    /// Its notes, moved to start in bar 1.
    pub notes: Vec<NoteEvent>,
    /// How far it was moved, in ticks, so what sits in the part can be found in the phrase.
    pub from: i64,
}

/// A part as phrases, with no empty bars before or after any of them. [`GAP_BARS`] or more empty bars split it into
/// sections; a section longer than [`MAX_BARS`] bars is cut where the music breathes ([`cut_points`]). Pieces with
/// the same notes are kept once. `clip_starts` are bars (from the part's start) where a clip starts, when the part was
/// joined from clips: likely places to cut. `notes` are sorted by start, as [`normalise`] gives them.
pub fn split(notes: &[NoteEvent], clip_starts: Option<&HashSet<i64>>) -> Vec<Piece> {
    if notes.is_empty() {
        return Vec::new();
    }
    // Parts that didn't come through normalise (a Bitwig song's tracks) are held to the same limit.
    let held: Vec<NoteEvent>;
    let notes = if notes.iter().any(|n| n.length > MAX_HOLD) {
        held = notes.iter().map(|n| NoteEvent { length: n.length.min(MAX_HOLD), ..*n }).collect();
        &held
    } else {
        notes
    };
    let sections = sections(notes);
    let whole = sections.len() == 1 && bars_of(notes) <= MAX_BARS;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for section in sections {
        let Some(head) = section.first() else { continue };
        let first = head.start / BAR;
        let bars = bars_of(&section) - first;
        let mut pieces: Vec<Vec<NoteEvent>> = Vec::new();
        if bars <= MAX_BARS {
            pieces.push(section);
        } else {
            let mut bounds = vec![first];
            bounds.extend(cut_points(&section, first, bars, clip_starts));
            bounds.push(first + bars);
            for w in bounds.windows(2) {
                let [from, to] = [w[0] * BAR, w[1] * BAR];
                // Notes held past the piece are cut at its end.
                let piece: Vec<NoteEvent> = section
                    .iter()
                    .filter(|n| n.start >= from && n.start < to)
                    .map(|n| NoteEvent { length: n.length.min(to - n.start), ..*n })
                    .collect();
                if !piece.is_empty() {
                    pieces.push(piece);
                }
            }
        }
        for piece in pieces {
            if !whole && piece.len() < 3 {
                continue;
            }
            let first_bar = piece.first().map_or(0, |n| n.start / BAR);
            let trimmed: Vec<NoteEvent> =
                piece.iter().take(MAX_NOTES).map(|n| NoteEvent { start: n.start - first_bar * BAR, ..*n }).collect();
            if !seen.insert(hash_of(&trimmed)) {
                continue;
            }
            let last_bar = first_bar + bars_of(&trimmed);
            let place = if whole {
                String::new()
            } else if first_bar + 1 == last_bar {
                format!("bar {last_bar}")
            } else {
                format!("bars {}–{last_bar}", first_bar + 1)
            };
            out.push(Piece { place, notes: trimmed, from: first_bar * BAR });
        }
    }
    out
}

/// The part cut wherever [`GAP_BARS`] or more bars go by with no note playing.
fn sections(notes: &[NoteEvent]) -> Vec<Vec<NoteEvent>> {
    let mut sections: Vec<Vec<NoteEvent>> = vec![Vec::new()];
    let mut playing_until = 0i64;
    for n in notes {
        let gap = n.start / BAR - (playing_until + BAR - 1) / BAR;
        if sections.last().is_some_and(|s| !s.is_empty()) && gap >= GAP_BARS {
            sections.push(Vec::new());
        }
        if let Some(s) = sections.last_mut() {
            s.push(*n);
        }
        playing_until = playing_until.max(n.start + n.length);
    }
    sections
}

/// Where to cut a long section into phrases of 4, 8, 12 or 16 bars (the last can be shorter), as bar numbers: where
/// the music breathes rather than every 8 bars. A piece is a good phrase when the music after it plays it again, and a
/// bar line is a good place to cut when nothing is held across it, the last beat before it rests or lands back on the
/// note the section started on, or a clip starts there. Of every way to cut the section, the best placed wins. Each
/// cut costs a little, so a 32-bar loop of one 8-bar progression becomes four 8-bar pieces (kept as one).
fn cut_points(section: &[NoteEvent], first: i64, bars: i64, clip_starts: Option<&HashSet<i64>>) -> Vec<i64> {
    let Ok(count) = usize::try_from(bars) else { return Vec::new() };
    // Each bar's notes and where they start in it: equal bars play the same thing. Bars are numbered by what they
    // hold, so two bars are equal when their numbers are (the C# app compares hash codes, which only differ by chance).
    let mut content: Vec<Vec<(u8, i64)>> = vec![Vec::new(); count];
    for n in section {
        let bar = n.start / BAR - first;
        if let Some(c) = usize::try_from(bar).ok().and_then(|b| content.get_mut(b)) {
            c.push((n.key, n.start % BAR));
        }
    }
    let mut ids: HashMap<Vec<(u8, i64)>, usize> = HashMap::new();
    let bar_ids: Vec<usize> = content
        .into_iter()
        .map(|c| {
            let next = ids.len();
            *ids.entry(c).or_insert(next)
        })
        .collect();
    let start0 = section.first().map_or(0, |n| n.start);
    let home = section.iter().filter(|n| n.start - start0 < PPQ / 16).map(|n| n.key).min().unwrap_or(0) % 12;

    // How much the bars after a piece repeat it (0 to 1).
    let repeated = |from: usize, to: usize| -> f64 {
        let length = to - from;
        let compared = length.min(count - to);
        if compared == 0 {
            return 0.0;
        }
        let same = (0..compared).filter(|i| bar_ids.get(from + i) == bar_ids.get(to + i)).count();
        same as f64 / length as f64
    };

    // How good each bar line is to cut at.
    let mut good = vec![0.0f64; count];
    for (b, g) in good.iter_mut().enumerate().skip(1) {
        let at = (first + b as i64) * BAR;
        let mut score = -0.6; // each cut costs this much, so only good places beat a longer piece
        let held = section.iter().filter(|n| n.start < at && n.start + n.length > at + PPQ / 4).count();
        let sounding = section.iter().filter(|n| n.start < at && n.start + n.length > at - PPQ).map(|n| n.key).min();
        if held > 0 {
            score -= 0.6 * (held as f64 / 2.0).min(1.0);
        }
        match sounding {
            None => score += 0.3,                          // a rest before the bar line
            Some(low) if low % 12 == home => score += 0.4, // back home
            Some(_) => {}
        }
        if clip_starts.is_some_and(|c| c.contains(&(first + b as i64))) {
            score += 0.5;
        }
        *g = score;
    }

    // The best way through: best[p] is the best score for cutting the first p bars into pieces.
    let mut best = vec![f64::NEG_INFINITY; count + 1];
    let mut from = vec![0usize; count + 1];
    if let Some(b) = best.first_mut() {
        *b = 0.0;
    }
    for p in 1..=count {
        for length in 1..=(MAX_BARS as usize).min(p) {
            let size = match length {
                8 | 16 => 0.3,
                4 => 0.0,
                12 => -0.1,
                _ if p == count => -0.3,
                _ => continue,
            };
            let start = p - length;
            let before = best.get(start).copied().unwrap_or(f64::NEG_INFINITY);
            if before == f64::NEG_INFINITY {
                continue;
            }
            let place = if start > 0 { good.get(start).copied().unwrap_or(0.0) } else { 0.0 };
            let score = before + place + size + 1.2 * repeated(start, p);
            if best.get(p).is_some_and(|b| score > *b) {
                best[p] = score;
                from[p] = start;
            }
        }
    }
    let mut cuts = Vec::new();
    let mut p = from.get(count).copied().unwrap_or(0);
    while p > 0 {
        cuts.push(first + p as i64);
        p = from.get(p).copied().unwrap_or(0);
    }
    cuts.reverse();
    cuts
}

// ---- what kind of part ----

const DRUM_WORDS: &[&str] = &[
    "kick",
    "kicks",
    "snare",
    "snares",
    "hat",
    "hats",
    "hihat",
    "hihats",
    "hh",
    "clap",
    "claps",
    "drum",
    "drums",
    "perc",
    "percs",
    "percussion",
    "cymbal",
    "cymbals",
    "shaker",
    "fpc",
    "909",
    "rim",
    "rimshot",
    "tom",
    "toms",
    "ride",
    "crash",
    "kit",
    "beat",
    "beats",
    "groove",
    "grooves",
    "break",
    "breaks",
    "tops",
    "snr",
    "kck",
    "battery",
    "tamb",
    "tambo",
    "tambourine",
    "tambourines",
    "conga",
    "congas",
    "bongo",
    "bongos",
    "cowbell",
    "cowbells",
    "clave",
    "claves",
    "guiro",
    "timbale",
    "timbales",
    "cabasa",
    "triangle",
    "shakers",
    "snap",
    "snaps",
    "chh",
    "ohh",
    "openhat",
    "closedhat",
    "bd",
    "sd",
    "kickdrum",
    "snaredrum",
    "808kick",
    "drumloop",
];
/// The drum words that name a style, not a drum sound ("beat", "groove", "break" can be anything).
const STYLE_DRUM_WORDS: &[&str] = &["kit", "beat", "beats", "groove", "grooves", "break", "breaks", "tops", "battery"];
const BASS_WORDS: &[&str] =
    &["bass", "basses", "bassline", "basslines", "sub", "subs", "subbass", "reese", "808", "808s"];
const CHORD_WORDS: &[&str] =
    &["chord", "chords", "prog", "progs", "progression", "progressions", "pad", "pads", "stab", "stabs", "harmony"];
const MELODY_WORDS: &[&str] = &[
    "melody", "melodies", "melodic", "lead", "leads", "hook", "hooks", "topline", "toplines", "riff", "riffs", "line",
    "counter",
];
const ARP_WORDS: &[&str] = &["arp", "arps", "arpeggio", "arpeggios", "arpeggiated", "arpeggiator"];

/// The words of some names, in small letters, split at anything but a-z and 0-9 (the C# `NonWord` split).
fn words_of(hints: &str) -> Vec<String> {
    static NON_WORD: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new("[^a-z0-9]+").ok());
    let lowered = mo_formats::case::to_lower(hints);
    match NON_WORD.as_ref() {
        Some(re) => re.split(&lowered).map(str::to_owned).collect(),
        None => vec![lowered],
    }
}

/// The word lists as sets, made once: every part's names are looked up in them.
struct WordSets {
    drums: HashSet<&'static str>,
    drum_sounds: HashSet<&'static str>,
    bass: HashSet<&'static str>,
    chords: HashSet<&'static str>,
    melody: HashSet<&'static str>,
    arp: HashSet<&'static str>,
}

static WORDS: LazyLock<WordSets> = LazyLock::new(|| {
    let set = |w: &[&'static str]| w.iter().copied().collect::<HashSet<_>>();
    WordSets {
        drums: set(DRUM_WORDS),
        drum_sounds: DRUM_WORDS.iter().copied().filter(|w| !STYLE_DRUM_WORDS.contains(w)).collect(),
        bass: set(BASS_WORDS),
        chords: set(CHORD_WORDS),
        melody: set(MELODY_WORDS),
        arp: set(ARP_WORDS),
    }
});

fn is_bass_word(w: &str) -> bool {
    WORDS.bass.contains(w) || w.ends_with("bass")
}

/// What the names say the part is: each kind the words point to.
pub fn roles_named(hints: &str) -> HashSet<Role> {
    roles_in(&words_of(hints))
}

fn roles_in(words: &[String]) -> HashSet<Role> {
    let sets = &*WORDS;
    let mut roles = HashSet::new();
    for word in words {
        let w = word.as_str();
        if w.is_empty() {
            continue;
        }
        if is_bass_word(w) {
            roles.insert(Role::Bass);
        }
        if w != "808" && w != "808s" && sets.drums.contains(w) {
            roles.insert(Role::Drums);
        }
        if sets.chords.contains(w) {
            roles.insert(Role::Chords);
        }
        if sets.melody.contains(w) {
            roles.insert(Role::Melody);
        }
        if sets.arp.contains(w) {
            roles.insert(Role::Arp);
        }
    }
    roles
}

/// Measurements of a part's notes that say what kind of part it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub notes: usize,
    pub onsets: usize,
    /// The share of onsets with three or more notes at once (not all one note in octaves).
    pub chord_share: f64,
    /// How many different notes sound at once, on average, while anything sounds.
    pub voices: f64,
    pub per_beat: f64,
    pub median_beats: f64,
    pub median_key: u8,
    pub distinct: usize,
    /// How even the gaps between notes are (1 = a steady grid, like an arpeggiator).
    pub evenness: f64,
    pub repeat_share: f64,
    /// The top line's steps, by size: 0 (the same note again) to an octave or more, as shares.
    pub intervals: [f64; 13],
    /// How many different pitch classes each bar uses, on average.
    pub notes_per_bar: f64,
}

impl Shape {
    /// The shape of notes sorted by start.
    pub fn of(notes: &[NoteEvent]) -> Shape {
        let mut shape = Shape {
            notes: notes.len(),
            onsets: 0,
            chord_share: 0.0,
            voices: 0.0,
            per_beat: 0.0,
            median_beats: 0.0,
            median_key: 0,
            distinct: 0,
            evenness: 0.0,
            repeat_share: 0.0,
            intervals: [0.0; 13],
            notes_per_bar: 0.0,
        };
        if notes.is_empty() {
            return shape;
        }
        let mut sorted: Vec<&NoteEvent> = notes.iter().collect();
        sorted.sort_by_key(|n| n.start);
        // Notes starting within a 64th of each other are struck together.
        let mut onsets: Vec<Vec<&NoteEvent>> = Vec::new();
        for n in sorted {
            match onsets.last_mut() {
                Some(o) if o.first().is_some_and(|f| n.start - f.start <= PPQ / 16) => o.push(n),
                _ => onsets.push(vec![n]),
            }
        }
        let distinct_count =
            |o: &[&NoteEvent], f: fn(u8) -> u8| o.iter().map(|n| f(n.key)).collect::<HashSet<_>>().len();
        // Three or more notes at once, not all the same note in octaves (a doubled lead is still a melody).
        let chordal =
            onsets.iter().filter(|o| distinct_count(o, |k| k) >= 3 && distinct_count(o, |k| k % 12) >= 2).count();
        shape.chord_share = chordal as f64 / onsets.len() as f64;

        // How many different notes sound at once, on average, while anything sounds (octave doublings count once).
        let mut seen = HashSet::new();
        let mut single: Vec<&NoteEvent> = notes.iter().filter(|n| seen.insert((n.start, n.key % 12))).collect();
        single.sort_by_key(|n| n.start);
        let (mut covered, mut cover_end) = (0i64, i64::MIN);
        for n in &single {
            let end = n.start + n.length;
            if end <= cover_end {
                continue;
            }
            covered += end - n.start.max(cover_end);
            cover_end = end;
        }
        let total: f64 = single.iter().fold(0.0, |sum, n| sum + n.length as f64);
        shape.voices = if covered <= 0 { 1.0 } else { total / covered as f64 };

        let beats = (bars_of(notes) * 4).max(1);
        let mut lengths: Vec<f64> = notes.iter().map(|n| n.length as f64 / PPQ as f64).collect();
        lengths.sort_by(f64::total_cmp);
        let mut keys: Vec<u8> = notes.iter().map(|n| n.key).collect();
        keys.sort_unstable();

        let gaps: Vec<f64> =
            onsets.windows(2).filter_map(|w| Some((w[1].first()?.start - w[0].first()?.start) as f64)).collect();
        if gaps.len() >= 4 {
            let mean = gaps.iter().fold(0.0, |s, g| s + g) / gaps.len() as f64;
            let var = gaps.iter().fold(0.0, |s, g| s + (g - mean) * (g - mean)) / gaps.len() as f64;
            let sd = var.sqrt();
            shape.evenness = if mean <= 0.0 { 0.0 } else { (1.0 - sd / mean).clamp(0.0, 1.0) };
        }

        let tops: Vec<u8> = onsets.iter().filter_map(|o| o.iter().map(|n| n.key).max()).collect();
        let mut intervals = [0.0f64; 13];
        for w in tops.windows(2) {
            let step = (w[1] as i32 - w[0] as i32).unsigned_abs().min(12) as usize;
            if let Some(i) = intervals.get_mut(step) {
                *i += 1.0;
            }
        }
        let steps: f64 = intervals.iter().fold(0.0, |s, i| s + i);
        shape.repeat_share = if steps == 0.0 { 1.0 } else { intervals[0] / steps };
        if steps > 0.0 {
            for i in &mut intervals {
                *i /= steps;
            }
        }
        shape.intervals = intervals;

        // How many different notes each bar uses: an arp keeps to its chord, a tune wanders through the scale.
        let mut per_bar: HashMap<i64, u16> = HashMap::new();
        for n in notes {
            *per_bar.entry(n.start / BAR).or_default() |= 1 << (n.key % 12);
        }
        let used: u32 = per_bar.values().map(|m| m.count_ones()).sum();
        shape.notes_per_bar = used as f64 / per_bar.len() as f64;

        shape.onsets = onsets.len();
        shape.per_beat = onsets.len() as f64 / beats as f64;
        shape.median_beats = lengths.get(lengths.len() / 2).copied().unwrap_or(0.0);
        shape.median_key = keys.get(keys.len() / 2).copied().unwrap_or(0);
        keys.dedup();
        shape.distinct = keys.len();
        shape
    }
}

/// The General MIDI drum notes, played like drums: a part using kick, snare and hats from these is a drum pattern.
/// Those notes are also C2 to Bb2, where plenty of basslines and low chords sit, so the notes alone aren't enough:
/// drums keep the hats going, hit with the kick and snare or in between them, where a bassline only passes through Bb
/// now and then and chords stack three notes or more.
fn looks_like_gm_drums(keys: &HashSet<u8>, notes: &[NoteEvent], s: &Shape) -> bool {
    let any = |set: &[u8]| keys.iter().any(|k| set.contains(k));
    if !keys.iter().all(|k| (35..=81).contains(k)) || !any(&[35, 36]) || !any(&[38, 39, 40]) || !any(&[42, 44, 46]) {
        return false;
    }
    if s.chord_share >= 0.35 {
        return false;
    }
    let mut at: HashMap<i64, HashSet<u8>> = HashMap::new();
    for n in notes {
        at.entry(n.start).or_default().insert(n.key);
    }
    let together = at.values().filter(|k| k.len() >= 2).count() as f64 / at.len() as f64;
    let hats = notes.iter().filter(|n| matches!(n.key, 42 | 44 | 46)).count() as f64 / notes.len() as f64;
    hats >= 0.25 && together >= 0.15 || hats >= 0.4
}

/// What a part is for, and why, from its shape, notes and names. `drums`: known to be drums (MIDI channel 10, or a
/// drum plugin).
pub fn role(s: &Shape, notes: &[NoteEvent], hints: &str, drums: bool) -> (Role, String) {
    let (role, why) = role_and_reason(s, notes, hints, drums);
    (role, why.into_owned())
}

fn role_and_reason(s: &Shape, notes: &[NoteEvent], hints: &str, drums: bool) -> (Role, Cow<'static, str>) {
    if drums {
        return (Role::Drums, "Drums by where it's played (MIDI channel 10 or a drum plugin)".into());
    }
    if s.notes == 0 {
        return (Role::Other, "".into());
    }
    let split = words_of(hints);
    let mut named = roles_in(&split);
    let keys: HashSet<u8> = notes.iter().map(|n| n.key).collect();
    let words: HashSet<String> = split.into_iter().collect();
    let tuned = [Role::Bass, Role::Chords, Role::Melody, Role::Arp];

    // "808" names a bass when it plays a tune and a drum when it only hits.
    if s.distinct == 1
        && (words.contains("808") || words.contains("808s"))
        && !words.iter().any(|w| w != "808" && w != "808s" && is_bass_word(w))
    {
        named.remove(&Role::Bass);
        named.insert(Role::Drums);
    }
    // A name that says a tuned part (and not drums) wins over the drum notes test.
    let named_tuned = !named.contains(&Role::Drums) && tuned.iter().any(|r| named.contains(r));
    if !named_tuned && looks_like_gm_drums(&keys, notes, s) {
        return (Role::Drums, "Uses the General MIDI kick, snare and hat notes, played like drums".into());
    }

    let (from_notes, why) = from_notes(s);
    if named.is_empty() {
        return (from_notes, why.into());
    }
    if named.contains(&from_notes) {
        return (from_notes, format!("{why}, and its name agrees").into());
    }
    if named.contains(&Role::Drums) && s.distinct <= 3 && s.chord_share < 0.2 {
        return (Role::Drums, "Named like a drum part".into());
    }
    // Named after a drum sound and nothing tuned ("Tamb", "Chh Selector Grid"): drums, whatever notes play it, unless
    // it plays far more notes than a kit has.
    let sound_word = words.iter().any(|w| WORDS.drum_sounds.contains(w.as_str()));
    if sound_word && !tuned.iter().any(|r| named.contains(r)) && s.distinct <= 12 {
        return (Role::Drums, "Named after a drum sound".into());
    }
    // A name wins unless the notes plainly say otherwise.
    for role in [Role::Chords, Role::Arp, Role::Bass, Role::Melody] {
        if !named.contains(&role) {
            continue;
        }
        let contradicts = match role {
            Role::Chords => s.chord_share < 0.1 && s.voices < 1.4,
            Role::Bass => s.chord_share > 0.5 || s.median_key > 64,
            _ => s.chord_share > 0.6,
        };
        if !contradicts {
            return (
                role,
                match role {
                    Role::Arp => "Named as an arp",
                    Role::Chords => "Named as chords",
                    Role::Bass => "Named as a bass",
                    _ => "Named as a melody",
                }
                .into(),
            );
        }
    }
    (from_notes, why.into())
}

fn from_notes(s: &Shape) -> (Role, &'static str) {
    if s.chord_share >= 0.35 || s.voices >= 2.5 {
        return (Role::Chords, "Mostly three or more notes at once");
    }
    if s.distinct == 1 {
        return if s.median_key <= 52 {
            (Role::Bass, "One low note, repeated")
        } else {
            (Role::Other, "One note, repeated")
        };
    }
    if s.median_key <= 50 && s.chord_share < 0.2 {
        return (Role::Bass, "Single notes, low down");
    }
    if s.onsets >= 8
        && s.chord_share < 0.15
        && s.per_beat >= 1.5
        && s.median_beats <= 0.5
        && s.evenness >= 0.6
        && s.repeat_share < 0.5
        && s.distinct >= 3
        && s.notes_per_bar <= 4.5
    {
        return (Role::Arp, "Fast, evenly spaced notes of one chord at a time");
    }
    (Role::Melody, "Single notes, one after another")
}

/// What a part is, from its notes (at [`PPQ`], as [`normalise`] gives them) and names. `hints` are names that may say
/// what it is: its track, pattern, file and folders. `drums`: known to be drums. `key` and `chords` come from the
/// harmony engine; without a key, the shape's scale is counted from the part's strongest note.
///
/// As the C# app, chords are only kept for a part that plays them ([`Analysis::wants_chords`]): a melody's or a
/// bassline's notes go by one at a time.
pub fn analyse(notes: &[NoteEvent], hints: &str, drums: bool, key: Option<Key>, chords: Vec<PhraseChord>) -> Analysis {
    let shape = Shape::of(notes);
    let (role, role_why) = role(&shape, notes, hints, drums);
    let chords = if Analysis::wants_chords(role, &shape) { chords } else { Vec::new() };
    let chroma = chroma_of(notes);
    // As the C# app: no key for drums, or for fewer than 4 notes or 3 pitch classes.
    let key =
        key.filter(|_| role != Role::Drums && notes.len() >= 4 && chroma.iter().filter(|c| **c > 0.0).count() >= 3);
    Analysis {
        role,
        role_why,
        bars: bars_of(notes),
        lowest: notes.iter().map(|n| n.key).min().unwrap_or(0),
        highest: notes.iter().map(|n| n.key).max().unwrap_or(0),
        key,
        chroma,
        chords,
        features: features(&shape, notes, &chroma, key),
        hash: hash_of(notes),
    }
}

/// How long each pitch class sounds (each note counted from a sixteenth to two bars), as shares adding up to 1. In
/// 32-bit floats, added up as the C# app adds them.
pub fn chroma_of(notes: &[NoteEvent]) -> [f32; 12] {
    let mut chroma = [0f32; 12];
    for n in notes {
        if let Some(c) = chroma.get_mut(n.key as usize % 12) {
            *c += (n.length as f64 / BAR as f64).clamp(1.0 / 16.0, 2.0) as f32;
        }
    }
    // .NET adds floats up in a double.
    let total = chroma.iter().fold(0f64, |s, c| s + *c as f64) as f32;
    if total > 0.0 {
        for c in &mut chroma {
            *c /= total;
        }
    }
    chroma
}

/// The numbers "more like this" compares, all the same whatever key the part is in: how chordal, busy, long-noted and
/// low it is, the size of its steps, its scale (from its key's root) and where in the bar its notes fall.
pub fn features(s: &Shape, notes: &[NoteEvent], chroma: &[f32; 12], key: Option<Key>) -> [f32; FEATURE_COUNT] {
    let mut f = [0f32; FEATURE_COUNT];
    if s.notes == 0 {
        return f;
    }
    let (lo, hi) = (notes.iter().map(|n| n.key).min().unwrap_or(0), notes.iter().map(|n| n.key).max().unwrap_or(0));
    f[0] = (s.voices / 4.0).min(1.0) as f32;
    f[1] = s.chord_share as f32;
    f[2] = (s.per_beat / 4.0).min(1.0) as f32;
    f[3] = (s.median_beats / 2.0).min(1.0) as f32;
    f[4] = ((s.median_key as f64 - 24.0) / 72.0).clamp(0.0, 1.0) as f32;
    f[5] = ((hi - lo) as f64 / 36.0).min(1.0) as f32;
    for (i, v) in s.intervals.iter().enumerate() {
        f[6 + i] = *v as f32;
    }
    let max = chroma.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    // The first strongest, as .NET's IndexOf finds it.
    let strongest = chroma.iter().position(|c| *c == max).unwrap_or(0);
    let tonic = key.map_or(strongest, |k| k.tonic as usize);
    let top = max.max(1e-6);
    for i in 0..12 {
        f[19 + i] = chroma[(tonic + i) % 12] / top;
    }
    let mut grid = [0f32; 16];
    for n in notes {
        // A start before 0 counts in the bar before.
        let slot = (n.start % BAR + BAR) % BAR * 16 / BAR;
        if let Some(g) = usize::try_from(slot).ok().and_then(|s| grid.get_mut(s)) {
            *g += 1.0;
        }
    }
    let gmax = grid.iter().copied().fold(f32::NEG_INFINITY, f32::max).max(1.0);
    for i in 0..16 {
        f[31 + i] = grid[i] / gmax;
    }
    f[47] = if key.is_some_and(|k| k.minor) { 1.0 } else { 0.0 };
    f
}

/// The hash of some notes: the first 8 bytes of a SHA-256 of each note's start, length (32-bit, little-endian) and key,
/// in order of start then key, as 16 capital hex letters, as the C# app makes it. The same notes give the same hash
/// whatever their velocity.
pub fn hash_of(notes: &[NoteEvent]) -> String {
    let mut sorted: Vec<&NoteEvent> = notes.iter().collect();
    sorted.sort_by_key(|n| (n.start, n.key));
    let mut sha = Sha256::new();
    for n in sorted {
        // As the C# app's (int) casts: the low 32 bits.
        sha.update((n.start as i32).to_le_bytes());
        sha.update((n.length as i32).to_le_bytes());
        sha.update([n.key]);
    }
    sha.finalize().iter().take(8).map(|b| format!("{b:02X}")).collect()
}

/// Numbers from 0 to 1 as bytes (0 to 255), as `midi.db` keeps them: plenty for comparing, at a quarter of the size.
/// With `scale_to_max`, the largest becomes 255 (for pitch-class shares, only ever compared as proportions).
pub fn bytes(values: &[f32], scale_to_max: bool) -> Vec<u8> {
    let max = if scale_to_max { values.iter().copied().fold(f32::NEG_INFINITY, f32::max).max(1e-9) } else { 1.0 };
    values.iter().map(|v| (f64::from(v / max * 255.0)).round_ties_even().clamp(0.0, 255.0) as u8).collect()
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use mo_formats::{NoteAttribute, velocity};

    fn note(start: i64, length: i64, key: u8) -> NoteEvent {
        NoteEvent {
            start,
            length,
            key,
            velocity: velocity::from_7(100),
            muted: false,
            step: false,
            attribute: NoteAttribute::NONE,
        }
    }

    fn analyse_named(notes: &[NoteEvent], hints: &str) -> Analysis {
        analyse(notes, hints, false, None, Vec::new())
    }

    #[test]
    fn normalise_moves_to_bar_one_and_resolution() {
        let mut muted = note(0, 10, 50);
        muted.muted = true;
        let mut step = note(960 * 9, 0, 60);
        step.step = true;
        let notes = [note(960 * 8 + 480, 480, 62), step, muted, note(960 * 8, 240, 60)];
        let n = normalise(&notes, 960, false);
        assert_eq!(
            n.iter().map(|n| (n.start, n.length, n.key)).collect::<Vec<_>>(),
            [(0, 24, 60), (48, 48, 62), (96, 24, 60)]
        );
        // A note before tick 0 starts in the bar before.
        let n = normalise(&[note(-48, 96, 60)], 96, false);
        assert_eq!(n[0].start, 336);
        // Banker's rounding, as .NET's Math.Round: 0.5 → 0, 1.5 → 2.
        let n = normalise(&[note(5, 5, 60), note(15, 15, 61)], 960, true);
        assert_eq!((n[0].start, n[0].length, n[1].start, n[1].length), (0, 1, 2, 2));
        // No note holds for millions of bars.
        assert_eq!(normalise(&[note(0, i64::from(i32::MAX), 60)], 96, false)[0].length, MAX_HOLD);
    }

    #[test]
    fn bars() {
        assert_eq!(bars_of(&[]), 0);
        assert_eq!(bars_of(&[note(0, 10, 60)]), 1);
        assert_eq!(bars_of(&[note(0, BAR, 60)]), 1);
        assert_eq!(bars_of(&[note(0, BAR + 3, 60)]), 1, "a hair over the bar line is still one bar");
        assert_eq!(bars_of(&[note(0, BAR + 4, 60)]), 2);
    }

    #[test]
    fn hash_matches_the_csharp_app() {
        // Worked out with the C# PhraseAnalyser.HashOf for the same notes.
        let notes = [note(0, 96, 60), note(96, 96, 64), note(192, 192, 67)];
        let mut sha = Sha256::new();
        for (s, l, k) in [(0i32, 96i32, 60u8), (96, 96, 64), (192, 192, 67)] {
            sha.update(s.to_le_bytes());
            sha.update(l.to_le_bytes());
            sha.update([k]);
        }
        let want: String = sha.finalize()[..8].iter().map(|b| format!("{b:02X}")).collect();
        assert_eq!(hash_of(&notes), want);
        assert_eq!(hash_of(&notes).len(), 16);
        // Order and velocity don't matter.
        let mut loud = [notes[2], notes[0], notes[1]];
        loud[0].velocity = 1;
        assert_eq!(hash_of(&loud), want);
    }

    #[test]
    fn kinds_of_part() {
        let triad = |at: i64, root: u8| [note(at, 384, root), note(at, 384, root + 4), note(at, 384, root + 7)];
        let chords: Vec<NoteEvent> =
            [60u8, 65, 67, 60].iter().enumerate().flat_map(|(i, r)| triad(i as i64 * 384, *r)).collect();
        assert_eq!(analyse_named(&chords, "").role, Role::Chords);
        assert_eq!(analyse_named(&chords, "Pad").role_why, "Mostly three or more notes at once, and its name agrees");

        let bass: Vec<NoteEvent> = (0..8).map(|i| note(i * 96, 90, [36, 36, 43, 41][i as usize % 4])).collect();
        assert_eq!(analyse_named(&bass, "").role, Role::Bass);

        let tune: Vec<NoteEvent> =
            [72u8, 74, 76, 79, 76, 74, 72, 71].iter().enumerate().map(|(i, k)| note(i as i64 * 144, 100, *k)).collect();
        assert_eq!(analyse_named(&tune, "").role, Role::Melody);
        // A name wins unless the notes plainly say otherwise.
        assert_eq!(analyse_named(&tune, "Sub bass").role, Role::Melody, "a bass doesn't sit this high");
        assert_eq!(analyse_named(&tune, "Lead").role_why, "Single notes, one after another, and its name agrees");

        let arp: Vec<NoteEvent> = (0..16).map(|i| note(i * 24, 20, [60, 64, 67, 72][i as usize % 4])).collect();
        assert_eq!(analyse_named(&arp, "").role, Role::Arp);

        let drums: Vec<NoteEvent> =
            (0..8).flat_map(|i| [note(i * 48, 24, 42), note(i * 48, 24, if i % 2 == 0 { 36 } else { 38 })]).collect();
        assert_eq!(analyse_named(&drums, "").role, Role::Drums);
        assert_eq!(analyse(&tune, "", true, None, Vec::new()).role, Role::Drums);
        // Named after a drum sound.
        assert_eq!(analyse_named(&tune, "Tamb").role, Role::Drums);
        // An 808 that only hits is a drum; one that plays a tune is a bass.
        let hits: Vec<NoteEvent> = (0..4).map(|i| note(i * 96, 48, 36)).collect();
        assert_eq!(analyse_named(&hits, "808").role, Role::Drums);
        assert_eq!(analyse_named(&bass, "808").role, Role::Bass);
    }

    #[test]
    fn chroma_and_features() {
        let notes = [note(0, 384, 60), note(0, 384, 64), note(384, 384, 67), note(768, 384, 67)];
        let c = chroma_of(&notes);
        assert!((c.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!((c[7] - 0.5).abs() < 1e-6);
        assert_eq!(bytes(&c, true)[7], 255);
        assert_eq!(bytes(&c, true)[0], 128);
        let a = analyse(&notes, "", false, None, Vec::new());
        assert_eq!(a.features[19], 1.0, "without a key, the scale starts on the strongest note (G)");
        let k = analyse(&notes, "", false, Some(Key { tonic: 0, minor: true, confidence: 1.0 }), Vec::new());
        assert_eq!((k.features[19], k.features[47]), (0.5, 1.0));
        assert_eq!(k.features[31], 1.0, "both notes on the first sixteenth");
    }

    #[test]
    fn long_parts_are_cut_where_they_breathe() {
        // One 8-bar progression of eight different chords played four times: cut into four 8-bar pieces, kept as one.
        // (Where halves of it repeat, the C# cut can choose 4-bar pieces at the end; this port does the same.)
        let mut notes = Vec::new();
        for round in 0..4 {
            for bar in 0..8 {
                let at = (round * 8 + bar) * BAR;
                let root = [60u8, 62, 64, 65, 67, 69, 71, 59][bar as usize];
                notes.extend([note(at, BAR, root), note(at, BAR, root + 4), note(at, BAR, root + 7)]);
            }
        }
        let pieces = split(&notes, None);
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].place, "bars 1–8");
        assert_eq!(bars_of(&pieces[0].notes), 8);

        // Two parts with 4 empty bars between them are two sections.
        let gap: Vec<NoteEvent> = (0..3)
            .map(|i| note(i * 96, 96, 60 + i as u8))
            .chain((0..3).map(|i| note(6 * BAR + i * 96, 96, 70 + i as u8)))
            .collect();
        let pieces = split(&gap, None);
        assert_eq!(pieces.iter().map(|p| p.place.as_str()).collect::<Vec<_>>(), ["bar 1", "bar 7"]);
        assert_eq!(pieces[1].from, 6 * BAR);
        assert_eq!(pieces[1].notes[0].start, 0);

        // A short part is whole.
        let pieces = split(&gap[..3], None);
        assert_eq!(pieces[0].place, "");
    }
}
