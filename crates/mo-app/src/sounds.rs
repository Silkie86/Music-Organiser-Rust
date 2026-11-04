//! The Sounds page's list: which sounds the filters let through, in which order, and the words each tile and line
//! shows. Ported from the C# `SoundsViewModel` (`MakeFilter`, `MakeOrder`), with the work done the Rust way: each
//! sound's search words are lower-cased once when the list is made, and filtering and sorting spread over every core
//! (rule 21). Call [`SoundList::view`] off the window's thread; it takes well under a second for 357,000 sounds.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Datelike;
use mo_sounds::usage::Usage;
use mo_sounds::{Sound, music, types};
use rayon::prelude::*;

/// Hits, loops or both (the C# Length radio buttons).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Length {
    #[default]
    All,
    /// One-shots: anything the analyser didn't call a loop.
    Hits,
    Loops,
}

impl Length {
    pub const ALL: [Length; 3] = [Length::All, Length::Hits, Length::Loops];

    pub fn name(self) -> &'static str {
        match self {
            Length::All => "Hits and loops",
            Length::Hits => "Hits",
            Length::Loops => "Loops",
        }
    }
}

/// The orders on offer. The page opens shuffled, a new order every time, to stumble on forgotten sounds (C#).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    #[default]
    Shuffled,
    MostLiked,
    Name,
    Type,
    Key,
    Newest,
}

impl Order {
    pub const ALL: [Order; 6] =
        [Order::Shuffled, Order::MostLiked, Order::Name, Order::Type, Order::Key, Order::Newest];

    /// The name the Order menu shows, which is also how the C# app wrote it.
    pub fn name(self) -> &'static str {
        match self {
            Order::Shuffled => "Shuffled",
            Order::MostLiked => "Most liked",
            Order::Name => "Name",
            Order::Type => "Type",
            Order::Key => "Key",
            Order::Newest => "Newest",
        }
    }

    pub fn from_name(name: &str) -> Order {
        Order::ALL.into_iter().find(|o| o.name() == name).unwrap_or_default()
    }
}

/// What the page's chips and boxes ask for. Empty means everything.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    /// The types picked ("Kick", "Snare"): a sound shows when it has any of them. Empty means every type.
    pub sound_types: Vec<String>,
    pub length: Length,
    /// A key as tonic (0 = C) and minor.
    pub key: Option<(i32, bool)>,
    /// With a key: also the keys that mix well with it (Camelot neighbours).
    pub mix_well: bool,
    pub bpm_from: Option<f64>,
    pub bpm_to: Option<f64>,
    /// Only sounds no project uses. Ignored until the use counts are known.
    pub unused: bool,
    /// Only sounds some project uses. Ignored until the use counts are known.
    pub used: bool,
    pub favourites: bool,
    /// One tag, matched without caring about capitals.
    pub tag: Option<String>,
    /// Words that must all be in the path, type, tags or character.
    pub search: String,
    /// Only sounds from these years, first to last: the years of the songs that use them when Used is on (and the
    /// use counts are in), otherwise the year the file was added.
    pub years: Option<(i32, i32)>,
}

impl Filters {
    /// Picks a type, or takes it off again when it's already picked.
    pub fn toggle_type(&mut self, sound_type: &str) {
        match self.sound_types.iter().position(|t| t == sound_type) {
            Some(at) => {
                self.sound_types.remove(at);
            }
            None => self.sound_types.push(sound_type.to_owned()),
        }
    }

    /// Turns one of the on/off chips on or off, by the name the page gives it: "mix-well", "unused", "used" or
    /// "favourites" (any other name changes nothing). Used and Unused are never both on. When Used goes on or off,
    /// the years picked on the year strip go too: its years then mean something else (the years of the songs that
    /// use each sound, or the year each was added), so a span picked for one isn't kept for the other.
    pub fn toggle(&mut self, chip: &str) {
        let by_use = self.used;
        match chip {
            "mix-well" => self.mix_well = !self.mix_well,
            "unused" => {
                self.unused = !self.unused;
                self.used &= !self.unused;
            }
            "used" => {
                self.used = !self.used;
                self.unused &= !self.used;
            }
            "favourites" => self.favourites = !self.favourites,
            _ => {}
        }
        if self.used != by_use {
            self.years = None;
        }
    }

    /// Whether any filter is set (for the Clear chip). The search box doesn't count: it has its own ×.
    pub fn any(&self) -> bool {
        let none = Filters { search: self.search.clone(), ..Filters::default() };
        *self != none
    }
}

/// What every list made from the same sounds shares: the sounds and what's worked out once from them.
#[derive(Default)]
struct Shared {
    sounds: Vec<Sound>,
    /// Lower-cased path, type, tags and character, one per line: what the search words are looked for in.
    hay: Vec<String>,
    /// Lower-cased names, for the name order and every order's tie-break.
    names: Vec<String>,
}

/// Every sound the page can show (hidden copies left out: they only hide in the app, rule 2), with what's worked out
/// once per sound for searching and sorting.
///
/// The sounds and what's worked out from them ([`Shared`]) are shared, not copied, by a clone: a big library's list is
/// most of a gigabyte, and the page makes a second list once it knows how your songs use each sound. Only what that
/// adds (use, Most liked) is each list's own.
#[derive(Clone, Default)]
pub struct SoundList {
    shared: Arc<Shared>,
    /// How your projects use each sound, by sound id, once known (sounds no project uses are left out).
    used: Option<HashMap<i64, Usage>>,
    /// Each sound's Most liked score, in `sounds` order, once known.
    liked: Option<Vec<f64>>,
    /// The best of `liked`, worked out once, as every tile asks for its share of it.
    liked_best: f64,
}

/// A list of at least this many sounds is freed off the thread that lets it go ([`crate::background::free_later`]):
/// freeing a few thousand takes a few milliseconds, half a million over a second.
const FREE_LATER_FROM: usize = 2_000;

impl Drop for Shared {
    /// The last list sharing a big library's sounds is often let go on the window's thread (when the use counts
    /// arrive, the page swaps its list for one that has them, and whichever part of the page held the old one last
    /// lets it go there), so the sounds go to the freeing thread whole, all they hold with them, and are freed there.
    fn drop(&mut self) {
        if self.sounds.len() >= FREE_LATER_FROM && !crate::background::freeing_here() {
            crate::background::free_later(std::mem::take(self));
        }
    }
}

impl Drop for SoundList {
    /// A list's own use counts (one per sound your songs use, each with its years) go the same way when there are many.
    fn drop(&mut self) {
        if self.used.as_ref().is_some_and(|u| u.len() >= FREE_LATER_FROM) && !crate::background::freeing_here() {
            crate::background::free_later(self.used.take());
        }
    }
}

impl SoundList {
    /// The list from the library's sounds.
    pub fn new(sounds: Vec<Sound>) -> SoundList {
        let mut sounds: Vec<Sound> = sounds.into_iter().filter(|s| !s.hidden).collect();
        // Leaving out the hidden copies keeps the room they took: give it back.
        sounds.shrink_to_fit();
        let hay = sounds.par_iter().map(hay_of).collect();
        let names = sounds.par_iter().map(|s| s.name().to_lowercase()).collect();
        SoundList { shared: Arc::new(Shared { sounds, hay, names }), used: None, liked: None, liked_best: 0.0 }
    }

    /// A list of the same sounds, sharing them, without this one's use or Most liked yet: what a list with the use
    /// counted in starts from.
    pub fn sharing(&self) -> SoundList {
        SoundList { shared: self.shared.clone(), used: None, liked: None, liked_best: 0.0 }
    }

    /// Whether `other` shares this list's sounds (it was cloned from it, or made with [`SoundList::sharing`]).
    pub fn shares_with(&self, other: &SoundList) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    /// How your projects use each sound (by id, as [`mo_sounds::usage::usage`] gives it), for the Used chips and
    /// column.
    pub fn set_used(&mut self, used: HashMap<i64, Usage>) {
        self.used = Some(used);
    }

    /// Each sound's Most liked score, from `score`.
    pub fn set_liked(&mut self, score: impl Fn(&Sound) -> f64 + Sync + Send) {
        let liked: Vec<f64> = self.shared.sounds.par_iter().map(&score).collect();
        self.liked_best = liked.iter().copied().fold(0.0, f64::max);
        self.liked = Some(liked);
    }

    pub fn len(&self) -> usize {
        self.shared.sounds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shared.sounds.is_empty()
    }

    /// Every sound in the list, in its order.
    pub fn sounds(&self) -> &[Sound] {
        &self.shared.sounds
    }

    /// How your projects use every sound (by id; ones no project uses are left out), if that's known yet.
    pub fn usage_map(&self) -> Option<&HashMap<i64, Usage>> {
        self.used.as_ref()
    }

    /// How your projects use the sound at `i`, if that's known yet (`None` too when none do).
    pub fn usage(&self, i: usize) -> Option<&Usage> {
        self.used.as_ref()?.get(&self.shared.sounds.get(i)?.id)
    }

    pub fn sound(&self, i: usize) -> Option<&Sound> {
        self.shared.sounds.get(i)
    }

    /// The place in the list of the sound with this id.
    pub fn find(&self, id: i64) -> Option<usize> {
        self.shared.sounds.iter().position(|s| s.id == id)
    }

    /// How many projects use a sound, if that's known yet.
    pub fn used_in(&self, i: usize) -> Option<u32> {
        let used = self.used.as_ref()?;
        Some(self.shared.sounds.get(i).and_then(|s| used.get(&s.id)).map_or(0, |u| u.projects))
    }

    /// The Most liked score, from 0 to 1 against the best liked, if scores are known yet.
    pub fn liked_share(&self, i: usize) -> Option<f64> {
        let liked = self.liked.as_ref()?;
        let best = self.liked_best;
        Some(if best > 0.0 { liked.get(i).copied().unwrap_or(0.0) / best } else { 0.0 })
    }

    /// Whether the use counts are in, so the Used chips can work.
    pub fn knows_use(&self) -> bool {
        self.used.is_some()
    }

    pub fn knows_liked(&self) -> bool {
        self.liked.is_some()
    }

    /// The sounds `filters` let through, as places in this list, in `order`. `seed` picks the shuffled order: the
    /// same seed gives the same order, and the page takes a new one each time it opens or Shuffle is pressed.
    pub fn view(&self, filters: &Filters, order: Order, seed: u64) -> Vec<usize> {
        let test = Test::new(filters);
        let mut shown: Vec<usize> =
            (0..self.shared.sounds.len()).into_par_iter().filter(|&i| self.allows(i, &test)).collect();
        if order == Order::Shuffled {
            // Each sound's place worked out once, not on every comparison.
            let mut keyed: Vec<(u64, usize)> =
                shown.par_iter().map(|&i| (shuffle_key(seed, self.shared.sounds[i].id), i)).collect();
            keyed.par_sort_unstable();
            return keyed.into_iter().map(|(_, i)| i).collect();
        }
        shown.par_sort_unstable_by(|&a, &b| self.compare(a, b, order));
        shown
    }

    /// History's list: the sounds `played` (places in this list, newest first, each with when) that `filters` let
    /// through, in the same order. The years picked on the year strip don't count, as History has no year strip.
    pub fn played<T: Copy>(&self, played: &[(usize, T)], filters: &Filters) -> Vec<(usize, T)> {
        let no_years = Filters { years: None, ..filters.clone() };
        let test = Test::new(&no_years);
        played.iter().copied().filter(|&(i, _)| i < self.shared.sounds.len() && self.allows(i, &test)).collect()
    }

    /// Each type and how many sounds have it, most first (the type chips), counting every sound the list holds.
    pub fn type_counts(&self) -> Vec<(String, usize)> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for s in self.shared.sounds.iter() {
            *counts.entry(s.sound_type.as_str()).or_default() += 1;
        }
        let mut counts: Vec<(String, usize)> = counts.into_iter().map(|(t, n)| (t.to_string(), n)).collect();
        // "Unknown" last, whatever its count: it isn't a type to pick for its sounds.
        counts.sort_by(|a, b| {
            (a.0 == types::UNKNOWN).cmp(&(b.0 == types::UNKNOWN)).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0))
        });
        counts
    }

    /// Every tag on a sound, sorted, for the Tag menu.
    pub fn tags(&self) -> Vec<String> {
        let mut tags: Vec<String> = self.shared.sounds.iter().flat_map(|s| s.tags.iter().cloned()).collect();
        tags.sort_by_key(|t| t.to_lowercase());
        tags.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        tags
    }

    fn allows(&self, i: usize, t: &Test) -> bool {
        let s = &self.shared.sounds[i];
        let analysis = s.analysis.as_ref();
        let is_loop = analysis.is_some_and(|a| a.is_loop);
        if (!t.sound_types.is_empty() && !t.sound_types.iter().any(|w| *w == s.sound_type))
            || (t.length == Length::Hits && is_loop)
            || (t.length == Length::Loops && !is_loop)
            || (t.favourites && !s.favourite)
        {
            return false;
        }
        if let Some(used) = &self.used {
            let n = used.get(&s.id).map_or(0, |u| u.projects);
            if (t.unused && n > 0) || (t.used && n == 0) {
                return false;
            }
        }
        if let Some(tag) = &t.tag
            && !s.tags.iter().any(|x| x.to_lowercase() == *tag)
        {
            return false;
        }
        if let Some(keys) = &t.keys {
            let Some(tonic) = analysis.and_then(|a| a.key_tonic) else { return false };
            let minor = analysis.is_some_and(|a| a.key_minor);
            if !keys.contains(&(tonic, minor)) {
                return false;
            }
        }
        let bpm = analysis.and_then(|a| a.bpm);
        // As the C# app: with a lowest tempo, a sound with none fails; with only a highest, likewise.
        if t.bpm_from.is_some_and(|lo| bpm.unwrap_or(-1.0) < lo)
            || t.bpm_to.is_some_and(|hi| bpm.is_none_or(|b| b > hi))
        {
            return false;
        }
        if let Some((first, last)) = t.years {
            let within = |y: i32| y >= first && y <= last;
            let ok = match (&self.used, t.used) {
                (Some(used), true) => used.get(&s.id).is_some_and(|u| u.years.iter().any(|&y| within(y))),
                _ => within(s.added.year()),
            };
            if !ok {
                return false;
            }
        }
        t.words.iter().all(|w| self.shared.hay[i].contains(w.as_str()))
    }

    /// How many sounds there are in each year, newest first, as the year filter reads them: by the years of the songs
    /// that use them when `by_use` (and the use counts are in), otherwise by the year each was added.
    pub fn year_counts(&self, by_use: bool) -> Vec<(i32, usize)> {
        let mut counts: std::collections::BTreeMap<i32, usize> = std::collections::BTreeMap::new();
        match (&self.used, by_use) {
            (Some(used), true) => {
                for s in self.shared.sounds.iter() {
                    let mut years = used.get(&s.id).map(|u| u.years.clone()).unwrap_or_default();
                    years.dedup();
                    for y in years {
                        *counts.entry(y).or_default() += 1;
                    }
                }
            }
            _ => {
                for s in self.shared.sounds.iter() {
                    *counts.entry(s.added.year()).or_default() += 1;
                }
            }
        }
        // A file with no date reads as 1970: it isn't a year anyone picks.
        counts.into_iter().rev().filter(|&(y, _)| y > 1980).collect()
    }

    fn compare(&self, a: usize, b: usize, order: Order) -> std::cmp::Ordering {
        let (sa, sb) = (&self.shared.sounds[a], &self.shared.sounds[b]);
        let by = match order {
            Order::Shuffled => std::cmp::Ordering::Equal,
            Order::MostLiked => match &self.liked {
                Some(liked) => liked[b].total_cmp(&liked[a]),
                None => std::cmp::Ordering::Equal,
            },
            Order::Name => std::cmp::Ordering::Equal,
            Order::Type => sa.sound_type.to_lowercase().cmp(&sb.sound_type.to_lowercase()),
            Order::Key => key_order(sa).cmp(&key_order(sb)),
            Order::Newest => sb.added.cmp(&sa.added),
        };
        by.then_with(|| self.shared.names[a].cmp(&self.shared.names[b])).then(a.cmp(&b))
    }
}

/// The filters made ready to test many sounds: lower-cased words, the set of keys allowed.
struct Test<'a> {
    sound_types: &'a [String],
    length: Length,
    unused: bool,
    used: bool,
    favourites: bool,
    tag: Option<String>,
    keys: Option<Vec<(i32, bool)>>,
    bpm_from: Option<f64>,
    bpm_to: Option<f64>,
    years: Option<(i32, i32)>,
    words: Vec<String>,
}

impl<'a> Test<'a> {
    fn new(f: &'a Filters) -> Test<'a> {
        let keys =
            f.key.map(
                |(tonic, minor)| {
                    if f.mix_well { music::compatible_keys(tonic, minor).to_vec() } else { vec![(tonic, minor)] }
                },
            );
        Test {
            sound_types: &f.sound_types,
            length: f.length,
            unused: f.unused,
            used: f.used,
            favourites: f.favourites,
            tag: f.tag.as_ref().map(|t| t.to_lowercase()),
            keys,
            bpm_from: f.bpm_from,
            bpm_to: f.bpm_to,
            years: f.years,
            words: f.search.split_whitespace().map(str::to_lowercase).collect(),
        }
    }
}

fn hay_of(s: &Sound) -> String {
    let mut hay = format!("{}\n{}", s.path, s.sound_type);
    for t in &s.tags {
        hay.push('\n');
        hay.push_str(t);
    }
    if let Some(a) = &s.analysis {
        hay.push('\n');
        hay.push_str(&a.character.join(", "));
    }
    hay.to_lowercase()
}

/// Keys in note order, major before minor; sounds with no key last (C# `KeyOrder`).
fn key_order(s: &Sound) -> i32 {
    s.analysis.as_ref().and_then(|a| a.key_tonic.map(|t| t * 2 + i32::from(a.key_minor))).unwrap_or(i32::MAX)
}

/// A sound's place in the shuffled order for `seed`: SplitMix64 of the two, a good spread from a cheap sum.
fn shuffle_key(seed: u64, id: i64) -> u64 {
    let mut z = seed.wrapping_add((id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The types with a colour of their own, in the order of the Sounds palette in the style (`Theme.sound-type`). A type
/// whose name holds one of these ("Closed hat" holds "Hat") takes its colour (C# `StarColours.Type`).
const COLOURED: [&str; 19] = [
    "Kick", "808", "Snare", "Clap", "Hat", "Cymbal", "Perc", "Tom", "Bass", "Lead", "Pad", "Keys", "Pluck", "Chord",
    "Vocal", "FX", "Loop", "Guitar", "Texture",
];

/// How many spare colours follow the named ones in the palette, for other types.
const SPARE_COLOURS: usize = 4;

/// A type's colour as its place in the Sounds palette, or -1 for grey (no type, or "Unknown").
pub fn type_colour(sound_type: &str) -> i32 {
    if sound_type.is_empty() || sound_type == types::UNKNOWN {
        return -1;
    }
    let lower = sound_type.to_lowercase();
    let at = COLOURED
        .iter()
        .position(|t| t.eq_ignore_ascii_case(sound_type))
        .or_else(|| COLOURED.iter().position(|t| lower.contains(&t.to_lowercase())))
        .unwrap_or_else(|| {
            // As C#: the sum of the lower-cased letters picks a spare colour, the same every time.
            COLOURED.len() + lower.chars().map(|c| c as usize).sum::<usize>() % SPARE_COLOURS
        });
    at as i32
}

/// A sound's length for a tile or line: "0.4 s" under ten seconds, "1:07" after.
pub fn length_text(seconds: f64) -> String {
    if seconds < 10.0 {
        format!("{seconds:.1} s")
    } else {
        let whole = seconds.round() as u64;
        format!("{}:{:02}", whole / 60, whole % 60)
    }
}

/// A sound's key, short: "Fm", "Eb"; empty when it has none.
pub fn key_text(s: &Sound) -> String {
    s.analysis.as_ref().and_then(|a| a.key_tonic.map(|t| music::key_name(t, a.key_minor, true))).unwrap_or_default()
}

/// A tempo for a tile or line: "128 BPM", or "92.5 BPM" when it isn't whole.
pub fn bpm_text(bpm: f64) -> String {
    if (bpm - bpm.round()).abs() < 0.05 { format!("{} BPM", bpm.round()) } else { format!("{bpm:.1} BPM") }
}

/// A waveform as path commands for the screens' `Waveform`: one upright line per peak, drawn about the middle, in a box
/// `peaks.len()` wide and 100 high. Peaks are scaled so the loudest reaches the edges, and the quietest still shows as a
/// sliver, as the old app drew them.
pub fn wave_path(peaks: &[f32]) -> String {
    use std::fmt::Write;
    let loudest = peaks.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(f32::EPSILON);
    let mut out = String::with_capacity(peaks.len() * 16);
    for (i, p) in peaks.iter().enumerate() {
        let half = (p.abs() / loudest * 50.0).clamp(0.5, 50.0);
        let _ = write!(out, "M{}.5 {:.1}V{:.1}", i, 50.0 - half, 50.0 + half);
    }
    out
}

/// The line under a sound's name: its type, key, tempo and length, the ones it has.
pub fn facts_line(s: &Sound) -> String {
    let mut parts = vec![s.sound_type.to_string()];
    parts.push(key_text(s));
    if let Some(a) = &s.analysis {
        if let Some(b) = a.bpm {
            parts.push(bpm_text(b));
        }
        parts.push(length_text(a.seconds));
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" · ")
}

/// "Never", "1 project", "12 projects", or "" when the counts aren't known yet.
pub fn used_text(used: Option<u32>) -> String {
    match used {
        None => String::new(),
        Some(0) => "Never".into(),
        Some(1) => "1 project".into(),
        Some(n) => format!("{n} projects"),
    }
}

/// A count for a type chip: "357k", "6.1k", "812".
pub fn short_count(n: usize) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..10_000 => format!("{:.1}k", n as f64 / 1_000.0),
        _ => format!("{}k", (n as f64 / 1_000.0).round()),
    }
}

/// The C# app's setting for how long each sound plays while a held arrow key steps through them (Settings › Sounds).
pub const STEP_KEY: &str = "sound_step_ms";
/// Its choices, and the C# app's default.
pub const STEP_CHOICES: [u64; 5] = [150, 250, 400, 600, 1000];
pub const STEP_MS: u64 = 250;

/// The step time from its setting: a whole number of milliseconds above 0, else the default (as the C# app reads it).
pub fn step_ms(setting: Option<&str>) -> u64 {
    setting.and_then(|s| s.trim().parse().ok()).filter(|&ms: &u64| ms > 0).unwrap_or(STEP_MS)
}

/// Whether an arrow key may move the pick (the C# app's `MayStep`). A press always may; a key held down (its
/// repeats) waits until the sound it's on has played for the step time, so what you see and what you hear stay
/// together and no sound is skipped (Solomon's 19 Oct note). With Autoplay off nothing plays, so a held key runs
/// through the list at the keyboard's own speed.
#[derive(Debug, Default)]
pub struct Stepping {
    /// When the pick last moved.
    stepped: Option<std::time::Instant>,
}

impl Stepping {
    pub fn may_step(&mut self, autoplay: bool, held: bool, step: std::time::Duration, now: std::time::Instant) -> bool {
        if !autoplay {
            return true;
        }
        if held && self.stepped.is_some_and(|at| now.saturating_duration_since(at) < step) {
            return false;
        }
        self.stepped = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use mo_sounds::SoundAnalysis;

    #[test]
    fn a_held_arrow_steps_no_faster_than_the_step_time_with_autoplay() {
        use std::time::{Duration, Instant};
        let step = Duration::from_millis(250);
        let t = Instant::now();
        let mut s = Stepping::default();
        assert!(s.may_step(true, false, step, t));
        // The key's repeats wait for the sound to have played its step time.
        assert!(!s.may_step(true, true, step, t + Duration::from_millis(30)));
        assert!(!s.may_step(true, true, step, t + Duration::from_millis(249)));
        assert!(s.may_step(true, true, step, t + Duration::from_millis(250)));
        // A fresh press always moves.
        assert!(s.may_step(true, false, step, t + Duration::from_millis(260)));
        // Without Autoplay a held key runs at the keyboard's speed.
        assert!(s.may_step(false, true, step, t + Duration::from_millis(261)));
        assert_eq!(step_ms(Some("400")), 400);
        assert_eq!(step_ms(Some("0")), STEP_MS);
        assert_eq!(step_ms(Some("x")), STEP_MS);
        assert_eq!(step_ms(None), STEP_MS);
    }

    fn sound(id: i64, path: &str, ty: &str, analysis: Option<SoundAnalysis>) -> Sound {
        Sound {
            id,
            path: path.into(),
            error: None,
            analysis,
            sound_type: ty.into(),
            type_source: "name".into(),
            type_confidence: 1.0,
            user_type: None,
            name_type: None,
            tags: Vec::new(),
            favourite: false,
            hidden: false,
            rating: 0,
            plays: 0,
            added: NaiveDate::from_ymd_opt(2026, 1, 1 + id as u32)
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .unwrap_or_default(),
            activity: Default::default(),
        }
    }

    fn analysed(is_loop: bool, bpm: Option<f64>, key: Option<(i32, bool)>) -> Option<SoundAnalysis> {
        Some(SoundAnalysis {
            seconds: 1.5,
            is_loop,
            bpm,
            key_tonic: key.map(|k| k.0),
            key_minor: key.is_some_and(|k| k.1),
            ..SoundAnalysis::default()
        })
    }

    fn library() -> SoundList {
        let mut hidden = sound(9, r"I:\Samples\Kicks\Kick copy.wav", "Kick", None);
        hidden.hidden = true;
        let mut fav = sound(3, r"I:\Samples\Vox\Chop Fm.wav", "Vocal", analysed(false, None, Some((5, true))));
        fav.favourite = true;
        fav.tags = vec!["Dark".into()];
        SoundList::new(vec![
            sound(1, r"I:\Samples\Kicks\Punchy Kick.wav", "Kick", analysed(false, None, None)),
            sound(2, r"I:\Samples\Loops\Reese 140.wav", "Bass", analysed(true, Some(140.0), Some((9, true)))),
            fav,
            sound(4, r"I:\Samples\Pads\Airy Pad.wav", "Pad", analysed(true, Some(90.0), Some((0, false)))),
            hidden,
        ])
    }

    fn ids(list: &SoundList, shown: &[usize]) -> Vec<i64> {
        shown.iter().filter_map(|&i| list.sound(i)).map(|s| s.id).collect()
    }

    #[test]
    fn a_big_list_is_freed_off_the_thread_that_lets_it_go() {
        let sounds = |n: usize| (0..n as i64).map(|i| sound(i, &format!(r"I:\S\{i}.wav"), "Kick", None)).collect();
        let (big, small) = (SoundList::new(sounds(FREE_LATER_FROM)), SoundList::new(sounds(10)));
        // A copy still in use keeps the list: only the last copy's going frees it.
        let shared = std::sync::Arc::new(big);
        let copy = shared.clone();
        let before = crate::background::handed_over();
        drop(small);
        drop(shared);
        assert_eq!(
            crate::background::handed_over(),
            before,
            "a small list is freed here; one still in use isn't freed"
        );
        assert_eq!(copy.len(), FREE_LATER_FROM);
        drop(copy);
        assert_eq!(crate::background::handed_over(), before + 1, "the big one goes to the freeing thread");
    }

    #[test]
    fn waveforms_are_one_line_per_peak() {
        assert_eq!(wave_path(&[]), "");
        // The loudest fills the height, a silent one is a sliver, and a negative peak counts by its size.
        assert_eq!(wave_path(&[0.5, 0.0, -0.25]), "M0.5 0.0V100.0M1.5 49.5V50.5M2.5 25.0V75.0");
    }

    #[test]
    fn the_year_filter_reads_the_added_year_or_the_songs_years() {
        let year = |y: i32| NaiveDate::from_ymd_opt(y, 6, 1).and_then(|d| d.and_hms_opt(0, 0, 0)).unwrap();
        let mut sounds: Vec<Sound> = (1..=4).map(|i| sound(i, &format!(r"I:\S\{i}.wav"), "Kick", None)).collect();
        for (s, y) in sounds.iter_mut().zip([2019, 2021, 2021, 2024]) {
            s.added = year(y);
        }
        let mut list = SoundList::new(sounds);
        assert_eq!(list.year_counts(false), [(2024, 1), (2021, 2), (2019, 1)]);
        let by = |list: &SoundList, f: Filters| ids(list, &list.view(&f, Order::Newest, 0));
        assert_eq!(by(&list, Filters { years: Some((2020, 2023)), ..Filters::default() }), [2, 3]);
        // With Used on, the songs' years count instead.
        let used = |years: Vec<i32>| Usage { projects: years.len() as u32, recent: 0, years };
        list.set_used(HashMap::from([(1, used(vec![2025, 2026])), (4, used(vec![2020]))]));
        assert_eq!(list.year_counts(true), [(2026, 1), (2025, 1), (2020, 1)]);
        let used_then = Filters { used: true, years: Some((2025, 2026)), ..Filters::default() };
        assert_eq!(by(&list, used_then), [1]);
    }

    #[test]
    fn turning_used_on_or_off_lets_go_of_the_years_picked() {
        let picked = Filters { years: Some((2020, 2022)), ..Filters::default() };
        // Used on: the strip counts the songs' years now, so years picked from the years added go.
        let mut f = picked.clone();
        f.toggle("used");
        assert!(f.used);
        assert_eq!(f.years, None);
        // And off again: years picked from the songs' years go too.
        f.years = Some((2025, 2026));
        f.toggle("used");
        assert!(!f.used);
        assert_eq!(f.years, None);
        // Unused on turns Used off, so it lets go of them as well.
        let mut f = Filters { used: true, ..picked.clone() };
        f.toggle("unused");
        assert!(f.unused && !f.used);
        assert_eq!(f.years, None);
        // Unused with Used off and the other chips keep them: the strip still reads the years added.
        let mut f = picked.clone();
        for chip in ["unused", "mix-well", "favourites", "unused", "no such chip"] {
            f.toggle(chip);
        }
        assert!(!f.unused && !f.used && f.mix_well && f.favourites);
        assert_eq!(f.years, Some((2020, 2022)));
    }

    #[test]
    fn hidden_copies_are_left_out() {
        let list = library();
        assert_eq!(list.len(), 4);
        assert!(list.find(9).is_none());
    }

    #[test]
    fn filters_narrow_the_list() {
        let list = library();
        let by = |f: Filters| ids(&list, &list.view(&f, Order::Name, 0));
        assert_eq!(by(Filters::default()), vec![4, 3, 1, 2]);
        assert_eq!(by(Filters { sound_types: vec!["Kick".into()], ..Filters::default() }), vec![1]);
        let kick_or_vocal = Filters { sound_types: vec!["Kick".into(), "Vocal".into()], ..Filters::default() };
        assert_eq!(by(kick_or_vocal), vec![3, 1]);
        let mut f = Filters::default();
        f.toggle_type("Kick");
        f.toggle_type("Vocal");
        f.toggle_type("Kick");
        assert_eq!(f.sound_types, vec!["Vocal".to_string()]);
        assert_eq!(by(Filters { length: Length::Loops, ..Filters::default() }), vec![4, 2]);
        assert_eq!(by(Filters { length: Length::Hits, ..Filters::default() }), vec![3, 1]);
        assert_eq!(by(Filters { favourites: true, ..Filters::default() }), vec![3]);
        assert_eq!(by(Filters { tag: Some("dark".into()), ..Filters::default() }), vec![3]);
        assert_eq!(by(Filters { key: Some((5, true)), ..Filters::default() }), vec![3]);
        assert_eq!(by(Filters { bpm_from: Some(100.0), ..Filters::default() }), vec![2]);
        assert_eq!(by(Filters { bpm_to: Some(100.0), ..Filters::default() }), vec![4]);
        assert_eq!(by(Filters { search: "samples LOOPS".into(), ..Filters::default() }), vec![2]);
        assert_eq!(by(Filters { search: "kick pad".into(), ..Filters::default() }), Vec::<i64>::new());
    }

    #[test]
    fn history_lists_what_was_played_that_the_filters_let_through() {
        let list = library();
        // Newest first, each with when (a number here), and played more than once.
        let played: Vec<(usize, u32)> =
            [(2, 50), (3, 40), (1, 30), (2, 20)].map(|(id, at)| (list.find(id).unwrap(), at)).to_vec();
        let by = |f: Filters| -> Vec<(i64, u32)> {
            list.played(&played, &f).into_iter().map(|(i, at)| (list.sound(i).unwrap().id, at)).collect()
        };
        assert_eq!(by(Filters::default()), [(2, 50), (3, 40), (1, 30), (2, 20)]);
        assert_eq!(by(Filters { length: Length::Loops, ..Filters::default() }), [(2, 50), (2, 20)]);
        assert_eq!(by(Filters { search: "kick".into(), ..Filters::default() }), [(1, 30)]);
        // The years picked on the year strip don't count: History has no strip.
        let years = Filters { years: Some((1990, 1991)), favourites: true, ..Filters::default() };
        assert!(list.view(&years, Order::Name, 0).is_empty());
        assert_eq!(by(years), [(3, 40)]);
        // A place that isn't in this list is left out.
        assert!(list.played(&[(99, 1)], &Filters::default()).is_empty());
    }

    #[test]
    fn keys_that_mix_well_count_with_the_switch() {
        let list = library();
        // A minor's Camelot neighbours include C major (8A beside 8B).
        let f = Filters { key: Some((9, true)), mix_well: true, ..Filters::default() };
        assert_eq!(ids(&list, &list.view(&f, Order::Name, 0)), vec![4, 2]);
    }

    #[test]
    fn used_chips_wait_for_the_counts() {
        let mut list = library();
        let f = Filters { unused: true, ..Filters::default() };
        assert_eq!(list.view(&f, Order::Name, 0).len(), 4);
        let used = |projects| Usage { projects, recent: 0, years: vec![2026] };
        list.set_used(HashMap::from([(1, used(3)), (2, used(1))]));
        assert_eq!(ids(&list, &list.view(&f, Order::Name, 0)), vec![4, 3]);
        let f = Filters { used: true, ..Filters::default() };
        assert_eq!(ids(&list, &list.view(&f, Order::Name, 0)), vec![1, 2]);
        assert_eq!(list.used_in(list.find(1).unwrap_or(99)), Some(3));
    }

    #[test]
    fn orders() {
        let mut list = library();
        let all = Filters::default();
        assert_eq!(ids(&list, &list.view(&all, Order::Newest, 0)), vec![4, 3, 2, 1]);
        assert_eq!(ids(&list, &list.view(&all, Order::Type, 0)), vec![2, 1, 4, 3]);
        // C major, F minor, A minor, then no key.
        assert_eq!(ids(&list, &list.view(&all, Order::Key, 0)), vec![4, 3, 2, 1]);
        // Without scores, Most liked falls back to names.
        assert_eq!(ids(&list, &list.view(&all, Order::MostLiked, 0)), vec![4, 3, 1, 2]);
        list.set_liked(|s| s.id as f64);
        assert_eq!(ids(&list, &list.view(&all, Order::MostLiked, 0)), vec![4, 3, 2, 1]);
        assert_eq!(list.liked_share(list.find(2).unwrap_or(99)), Some(0.5));
        // Shuffled is the same for the same seed and changes with another.
        let first = list.view(&all, Order::Shuffled, 7);
        assert_eq!(first, list.view(&all, Order::Shuffled, 7));
        assert_eq!(first.len(), 4);
        let seen: std::collections::HashSet<_> = (0..20).map(|seed| list.view(&all, Order::Shuffled, seed)).collect();
        assert!(seen.len() > 1);
    }

    #[test]
    fn type_chips_count_most_first_and_unknown_last() {
        let mut sounds = vec![sound(1, "a.wav", "Kick", None), sound(2, "b.wav", "Kick", None)];
        sounds.push(sound(3, "c.wav", types::UNKNOWN, None));
        sounds.push(sound(4, "d.wav", types::UNKNOWN, None));
        sounds.push(sound(5, "e.wav", types::UNKNOWN, None));
        sounds.push(sound(6, "f.wav", "Pad", None));
        let list = SoundList::new(sounds);
        let counts = list.type_counts();
        assert_eq!(counts[0], ("Kick".to_string(), 2));
        assert_eq!(counts[1], ("Pad".to_string(), 1));
        assert_eq!(counts[2], (types::UNKNOWN.to_string(), 3));
    }

    #[test]
    fn type_colours_follow_the_c_sharp_app() {
        assert_eq!(type_colour("Kick"), 0);
        assert_eq!(type_colour("Closed hat"), 4);
        assert_eq!(type_colour("Drum loop"), 16);
        assert_eq!(type_colour(types::UNKNOWN), -1);
        assert_eq!(type_colour(""), -1);
        // "Strings & brass" has no colour of its own: one of the spare ones, the same every time.
        let spare = type_colour("Strings & brass");
        assert!((19..23).contains(&spare));
        assert_eq!(spare, type_colour("Strings & brass"));
    }

    #[test]
    fn words_for_tiles() {
        assert_eq!(length_text(0.42), "0.4 s");
        assert_eq!(length_text(67.2), "1:07");
        assert_eq!(bpm_text(128.0), "128 BPM");
        assert_eq!(bpm_text(92.5), "92.5 BPM");
        assert_eq!(used_text(None), "");
        assert_eq!(used_text(Some(0)), "Never");
        assert_eq!(used_text(Some(1)), "1 project");
        assert_eq!(short_count(812), "812");
        assert_eq!(short_count(6_140), "6.1k");
        assert_eq!(short_count(357_412), "357k");
        let list = library();
        let reese = list.sound(list.find(2).unwrap_or(99)).map(facts_line);
        assert_eq!(reese.as_deref(), Some("Bass · Am · 140 BPM · 1.5 s"));
    }

    #[test]
    fn clear_counts_filters_but_not_the_search() {
        assert!(!Filters { search: "kick".into(), ..Filters::default() }.any());
        assert!(Filters { favourites: true, ..Filters::default() }.any());
        assert_eq!(Order::from_name("Most liked"), Order::MostLiked);
        assert_eq!(Order::from_name("whatever"), Order::Shuffled);
    }

    #[test]
    fn a_list_with_the_use_shares_the_sounds_rather_than_copying_them() {
        let mut list = SoundList::new((1..=3).map(|i| sound(i, &format!("{i}.wav"), "Kick", None)).collect());
        list.set_liked(|s| s.id as f64);
        let used = list.sharing();
        assert!(used.shares_with(&list) && list.clone().shares_with(&list));
        assert!(std::ptr::eq(used.sounds().as_ptr(), list.sounds().as_ptr()));
        // Only the sounds: what the first list worked out from its use isn't carried over.
        assert!(!used.knows_liked() && list.knows_liked());
        assert!(!SoundList::new(Vec::new()).shares_with(&list));
    }
}
