//! The Sounds page's list: which sounds the filters let through, in which order, and the words each tile and line
//! shows. Ported from the C# `SoundsViewModel` (`MakeFilter`, `MakeOrder`), with the work done the Rust way: each
//! sound's search words are lower-cased once when the list is made, and filtering and sorting spread over every core
//! (rule 21). Call [`SoundList::view`] off the window's thread; it takes well under a second for 357,000 sounds.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Datelike;
use mo_library::ProjectFile;
use mo_sounds::compact::Synced;
use mo_sounds::usage::Usage;
use mo_sounds::{Sound, SoundsError, music, rediscover, types};
use mo_space::size::size_text;
use rayon::prelude::*;

use crate::filters::{self as chips, Filter};

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

/// The filters' names, as the page calls them when a choice is picked.
pub const TYPE: &str = "type";
pub const LENGTH: &str = "length";
pub const KEY: &str = "key";
pub const TEMPO: &str = "tempo";
pub const TAG: &str = "tag";

/// The Tempo chips' choices, and the tempos each lets through (lowest, highest).
pub const TEMPOS: [(&str, Option<f64>, Option<f64>); 5] = [
    ("Up to 90", None, Some(90.0)),
    ("90 to 110", Some(90.0), Some(110.0)),
    ("110 to 130", Some(110.0), Some(130.0)),
    ("130 to 150", Some(130.0), Some(150.0)),
    ("150 and up", Some(150.0), None),
];

/// How many types are chips of their own, the most used; the rest are in the type filter's More menu.
pub const TYPE_CHIPS: usize = 9;

/// What the page's chips and boxes ask for. Empty means everything.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    /// The types picked ("Kick", "Snare"): a sound shows when it has any of them. Empty means every type.
    pub sound_types: Vec<String>,
    pub length: Length,
    /// The keys picked, each as tonic (0 = C) and minor: a sound shows when it's in any of them.
    pub keys: Vec<(i32, bool)>,
    /// With keys picked: also the keys that mix well with each (Camelot neighbours).
    pub mix_well: bool,
    /// The Tempo chips picked, by their words ([`TEMPOS`]): a sound shows when its tempo is in any of them.
    pub tempos: Vec<String>,
    /// Only sounds no project uses. Ignored until the use counts are known.
    pub unused: bool,
    /// Only sounds some project uses. Ignored until the use counts are known.
    pub used: bool,
    pub favourites: bool,
    /// The tags picked: a sound shows when it has any of them, capitals aside.
    pub tags: Vec<String>,
    /// Words that must all be in the path, type, tags or character.
    pub search: String,
    /// Only the sounds right in this folder (not in a folder inside it), as Rediscover's forgotten folders count them:
    /// one clicked there.
    pub folder: Option<String>,
    /// Only sounds from these years, first to last: the years of the songs that use them when Used is on (and the
    /// use counts are in), otherwise the year the file was added.
    pub years: Option<(i32, i32)>,
}

impl Filters {
    /// Picks a type, or takes it off again when it's already picked.
    pub fn toggle_type(&mut self, sound_type: &str) {
        chips::toggle(&mut self.sound_types, sound_type, true);
    }

    /// Picks or unpicks `choice` (as its chip or menu line says it) on the filter `chip` ([`TYPE`], [`LENGTH`],
    /// [`KEY`], [`TEMPO`], [`TAG`]). More than one type, key, tempo or tag can be picked; one length, as hits and loops
    /// together are every sound.
    pub fn pick(&mut self, chip: &str, choice: &str) {
        match chip {
            TYPE => self.toggle_type(choice),
            LENGTH => {
                let mut picked: Vec<Length> = Some(self.length).filter(|l| *l != Length::All).into_iter().collect();
                chips::toggle_as(&mut picked, choice, false, Length::name, |w| {
                    Length::ALL.into_iter().find(|l| *l != Length::All && l.name() == w)
                });
                self.length = picked.first().copied().unwrap_or_default();
            }
            KEY => {
                // In the menu's own words, however it came ("Am", "a minor"), so picking it again unpicks it.
                if let Some((tonic, minor)) = music::parse_key(choice) {
                    let words = |(t, m): (i32, bool)| music::key_name(t, m, false);
                    chips::toggle_as(&mut self.keys, &words((tonic, minor)), true, words, music::parse_key);
                }
            }
            TEMPO if TEMPOS.iter().any(|t| t.0 == choice) => chips::toggle(&mut self.tempos, choice, true),
            TAG => chips::toggle(&mut self.tags, choice, true),
            _ => {}
        }
    }

    /// The type filter's More menu's Any: unpicks the types in the menu, and keeps the ones picked on chips of their
    /// own. `types` is [`SoundList::type_counts`], as the chips were shown from.
    pub fn clear_more_types(&mut self, types: &[(String, usize)]) {
        let on_chips = &types[..types.len().min(TYPE_CHIPS)];
        self.sound_types.retain(|t| on_chips.iter().any(|(c, _)| c.eq_ignore_ascii_case(t)));
    }

    /// Picks nothing on the filter `chip`: its menu's Any, or Clear types.
    pub fn clear_chip(&mut self, chip: &str) {
        match chip {
            TYPE => self.sound_types.clear(),
            LENGTH => self.length = Length::All,
            KEY => self.keys.clear(),
            TEMPO => self.tempos.clear(),
            TAG => self.tags.clear(),
            _ => {}
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
            // Its chip shows only while it's on: a click takes it off.
            "folder" => self.folder = None,
            _ => {}
        }
        if self.used != by_use {
            self.years = None;
        }
    }

    /// Whether any filter is set (for the Clear chip). The search box doesn't count: it has its own ×. Nor does Keys
    /// that mix well with no key picked: it changes nothing then, and its chip isn't shown.
    pub fn any(&self) -> bool {
        let mix_well = self.mix_well && self.keys.is_empty();
        let none = Filters { search: self.search.clone(), mix_well, ..Filters::default() };
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
    /// Each project's file, by id, once known: the projects a sound is used in are named from it. Shared by a clone.
    projects: Option<Arc<HashMap<i64, ProjectFile>>>,
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
        SoundList {
            shared: Arc::new(Shared { sounds, hay, names }),
            used: None,
            projects: None,
            liked: None,
            liked_best: 0.0,
        }
    }

    /// A list of the same sounds, sharing them, without this one's use or Most liked yet: what a list with the use
    /// counted in starts from.
    pub fn sharing(&self) -> SoundList {
        SoundList { shared: self.shared.clone(), used: None, projects: None, liked: None, liked_best: 0.0 }
    }

    /// Whether `other` shares this list's sounds (it was cloned from it, or made with [`SoundList::sharing`]).
    pub fn shares_with(&self, other: &SoundList) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    /// Whether `other` lists the same sounds as this one in the same order (by id), whatever each one says now: a list
    /// read again after the C# app changed the library (a play, stars, tags) can then take this one's place with
    /// nothing on show moving.
    pub fn same_sounds_as(&self, other: &SoundList) -> bool {
        self.shares_with(other)
            || (self.len() == other.len() && self.sounds().iter().zip(other.sounds()).all(|(a, b)| a.id == b.id))
    }

    /// How your projects use each sound (by id, as [`mo_sounds::usage::usage`] gives it), for the Used chips and
    /// column.
    pub fn set_used(&mut self, used: HashMap<i64, Usage>) {
        self.used = Some(used);
    }

    /// The file of each project that uses a sound, by id ([`mo_library::Library::sample_usage_with_projects`]), to
    /// name the projects a sound is used in.
    pub fn set_project_files(&mut self, projects: Arc<HashMap<i64, ProjectFile>>) {
        self.projects = Some(projects);
    }

    /// The projects that use the sound at `i`, newest first (each one's file and when it was saved). None while
    /// that isn't known; empty when none do.
    pub fn projects_of(&self, i: usize) -> Option<Vec<&ProjectFile>> {
        let files = self.projects.as_deref()?;
        self.used.as_ref()?;
        let ids = self.usage(i).map_or(&[][..], |u| &u.project_ids[..]);
        let mut of: Vec<&ProjectFile> = ids.iter().filter_map(|id| files.get(id)).collect();
        of.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.path.cmp(&b.path)));
        Some(of)
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
        let shown = (0..self.shared.sounds.len()).into_par_iter().filter(|&i| self.allows(i, &test)).collect();
        self.ordered(shown, order, seed)
    }

    /// [`SoundList::view`], and how many sounds each choice of the chips' filters would show with the other filters
    /// as they are, counted in the same pass: a sound counts beside every chip when all five let it through, and
    /// beside the one that doesn't when only one doesn't.
    pub fn view_counted(&self, filters: &Filters, order: Order, seed: u64) -> (Vec<usize>, SoundCounts) {
        let test = Test::new(filters);
        let (shown, tally) = (0..self.shared.sounds.len())
            .into_par_iter()
            .fold(
                || (Vec::new(), Tally::default()),
                |(mut shown, mut tally), i| {
                    let s = &self.shared.sounds[i];
                    let ok = facets(s, &test);
                    let fails = ok.iter().filter(|&&o| !o).count();
                    // The chips first, as they're quick: a sound two of them leave out is neither shown nor counted,
                    // so the search (the slow part) isn't tried on it.
                    if fails <= 1 && self.passes_others(i, s, &test) {
                        tally.count(s, ok, fails);
                        if fails == 0 {
                            shown.push(i);
                        }
                    }
                    (shown, tally)
                },
            )
            .reduce(
                || (Vec::new(), Tally::default()),
                |(mut a, mut tally), (b, other)| {
                    a.extend(b);
                    tally.add(other);
                    (a, tally)
                },
            );
        (self.ordered(shown, order, seed), tally.counts())
    }

    /// `shown` in `order`.
    fn ordered(&self, mut shown: Vec<usize>, order: Order, seed: u64) -> Vec<usize> {
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

    /// Each type and how many sounds have it, most first (the type chips), counting every sound the list holds. Types
    /// that differ only in capitals are one, as the filter matches them, named as most of its sounds have it.
    pub fn type_counts(&self) -> Vec<(String, usize)> {
        let mut exact: HashMap<&str, usize> = HashMap::new();
        for s in self.shared.sounds.iter() {
            *exact.entry(s.sound_type.as_str()).or_default() += 1;
        }
        // In lower case: the name most of its sounds have, how many have that name, and how many there are in all.
        let mut by: HashMap<String, (&str, usize, usize)> = HashMap::new();
        for (t, n) in exact {
            let one = by.entry(t.to_ascii_lowercase()).or_insert((t, 0, 0));
            if n > one.1 || (n == one.1 && t < one.0) {
                (one.0, one.1) = (t, n);
            }
            one.2 += n;
        }
        let mut counts: Vec<(String, usize)> = by.into_values().map(|(t, _, n)| (t.to_owned(), n)).collect();
        // "Unknown" last, whatever its count: it isn't a type to pick for its sounds.
        let unknown = |t: &str| t.eq_ignore_ascii_case(types::UNKNOWN);
        counts.sort_by(|a, b| unknown(&a.0).cmp(&unknown(&b.0)).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0)));
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
        facets(s, t) == [true; FACETS] && self.passes_others(i, s, t)
    }

    /// Everything but the chips' filters lets `s` (at `i`) through: favourites, Used and Unused, the folder, the years
    /// and the search (last, as it's the slow one).
    fn passes_others(&self, i: usize, s: &Sound, t: &Test) -> bool {
        if t.favourites && !s.favourite {
            return false;
        }
        if let Some(used) = &self.used {
            let n = used.get(&s.id).map_or(0, |u| u.projects);
            if (t.unused && n > 0) || (t.used && n == 0) {
                return false;
            }
        }
        if t.folder.is_some_and(|folder| !rediscover::in_folder(&s.path, folder)) {
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
        crate::year_strip::strip(counts)
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
            // Letter by letter, lower-cased as it goes: no new strings for each of a big list's comparisons.
            Order::Type => sa
                .sound_type
                .chars()
                .flat_map(char::to_lowercase)
                .cmp(sb.sound_type.chars().flat_map(char::to_lowercase)),
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
    tags: &'a [String],
    keys: Option<Vec<(i32, bool)>>,
    /// The tempos picked, lowest and highest of each.
    tempos: Vec<(Option<f64>, Option<f64>)>,
    years: Option<(i32, i32)>,
    folder: Option<&'a str>,
    words: Vec<String>,
}

impl<'a> Test<'a> {
    fn new(f: &'a Filters) -> Test<'a> {
        let keys = (!f.keys.is_empty()).then(|| {
            let mut keys: Vec<(i32, bool)> = Vec::new();
            for &(tonic, minor) in &f.keys {
                let near =
                    if f.mix_well { music::compatible_keys(tonic, minor).to_vec() } else { vec![(tonic, minor)] };
                for (t, m) in near {
                    let key = (t.rem_euclid(12), m);
                    if !keys.contains(&key) {
                        keys.push(key);
                    }
                }
            }
            keys
        });
        Test {
            sound_types: &f.sound_types,
            length: f.length,
            unused: f.unused,
            used: f.used,
            favourites: f.favourites,
            tags: &f.tags,
            keys,
            tempos: TEMPOS.iter().filter(|t| f.tempos.iter().any(|w| w == t.0)).map(|t| (t.1, t.2)).collect(),
            years: f.years,
            folder: f.folder.as_deref(),
            words: f.search.split_whitespace().map(str::to_lowercase).collect(),
        }
    }
}

/// How many of the chips' filters there are, and each one's place in [`facets`]' answer.
const FACETS: usize = 5;
const TYPE_AT: usize = 0;
const LENGTH_AT: usize = 1;
const KEY_AT: usize = 2;
const TEMPO_AT: usize = 3;
const TAG_AT: usize = 4;

/// Which of the chips' filters let `s` through: its type, its length (hit or loop), its key, its tempo and its tags.
fn facets(s: &Sound, t: &Test) -> [bool; FACETS] {
    let a = s.analysis.as_ref();
    let is_loop = a.is_some_and(|a| a.is_loop);
    let bpm = a.and_then(|a| a.bpm);
    [
        t.sound_types.is_empty() || t.sound_types.iter().any(|w| w.eq_ignore_ascii_case(s.sound_type.as_str())),
        match t.length {
            Length::All => true,
            Length::Hits => !is_loop,
            Length::Loops => is_loop,
        },
        t.keys.as_ref().is_none_or(|keys| key_of(s).is_some_and(|k| keys.contains(&k))),
        t.tempos.is_empty() || t.tempos.iter().any(|&range| tempo_in(bpm, range)),
        t.tags.is_empty() || t.tags.iter().any(|w| s.tags.iter().any(|own| own.eq_ignore_ascii_case(w))),
    ]
}

/// Whether `bpm` is in `range` (lowest, highest), both ends in. As the C# app: with a lowest tempo, a sound with none
/// is left out; with a highest, likewise.
fn tempo_in(bpm: Option<f64>, (lo, hi): (Option<f64>, Option<f64>)) -> bool {
    !(lo.is_some_and(|lo| bpm.unwrap_or(-1.0) < lo) || hi.is_some_and(|hi| bpm.is_none_or(|b| b > hi)))
}

/// A sound's key (tonic 0 to 11, minor), when the analysis found one.
fn key_of(s: &Sound) -> Option<(i32, bool)> {
    s.analysis.as_ref().and_then(|a| a.key_tonic.map(|t| (t.rem_euclid(12), a.key_minor)))
}

/// A key's place among the 24: the tonic's times two, plus one for minor.
fn key_place(tonic: i32, minor: bool) -> usize {
    tonic.rem_euclid(12) as usize * 2 + usize::from(minor)
}

/// Every key the Key filter offers, as tonic and minor: the twelve major keys, then the twelve minor ones.
fn every_key() -> impl Iterator<Item = (i32, bool)> {
    [false, true].into_iter().flat_map(|minor| (0..12).map(move |tonic| (tonic, minor)))
}

/// The Key filter's choices in words ("C major" … "B minor"), as [`Filters::pick`] reads them.
pub fn key_choices() -> Vec<String> {
    every_key().map(|(tonic, minor)| music::key_name(tonic, minor, false)).collect()
}

/// The counts made while the list is: how many sounds each choice of the chips' filters lets through, with the other
/// filters as they are. Words are kept as the sounds have them, and joined in lower case at the end.
#[derive(Default)]
struct Tally<'a> {
    types: HashMap<&'a str, usize>,
    lengths: [usize; 2],
    keys: [usize; 24],
    tempos: [usize; TEMPOS.len()],
    tags: HashMap<&'a str, usize>,
}

impl<'a> Tally<'a> {
    /// Counts `s`, which the chips' filters `ok` let through, all but `fails` of them: beside every filter's choices
    /// when they all do, and beside only the one that doesn't when one doesn't (picking another of its choices would
    /// show it). A sound two of them leave out isn't counted.
    fn count(&mut self, s: &'a Sound, ok: [bool; FACETS], fails: usize) {
        let on = |facet: usize| fails == 0 || !ok[facet];
        let a = s.analysis.as_ref();
        if on(TYPE_AT) {
            *self.types.entry(s.sound_type.as_str()).or_default() += 1;
        }
        if on(LENGTH_AT) {
            self.lengths[usize::from(a.is_some_and(|a| a.is_loop))] += 1;
        }
        if on(KEY_AT)
            && let Some((tonic, minor)) = key_of(s)
        {
            self.keys[key_place(tonic, minor)] += 1;
        }
        if on(TEMPO_AT) {
            let bpm = a.and_then(|a| a.bpm);
            for (n, t) in self.tempos.iter_mut().zip(TEMPOS) {
                if tempo_in(bpm, (t.1, t.2)) {
                    *n += 1;
                }
            }
        }
        if on(TAG_AT) {
            for (k, t) in s.tags.iter().enumerate() {
                // A tag the sound has twice (in other capitals) counts it once: picking it shows the sound once.
                if !s.tags[..k].iter().any(|other| other.eq_ignore_ascii_case(t)) {
                    *self.tags.entry(t.as_str()).or_default() += 1;
                }
            }
        }
    }

    /// Adds another part of the list's counts.
    fn add(&mut self, other: Tally<'a>) {
        for (t, n) in other.types {
            *self.types.entry(t).or_default() += n;
        }
        for (mine, theirs) in self.lengths.iter_mut().zip(other.lengths) {
            *mine += theirs;
        }
        for (mine, theirs) in self.keys.iter_mut().zip(other.keys) {
            *mine += theirs;
        }
        for (mine, theirs) in self.tempos.iter_mut().zip(other.tempos) {
            *mine += theirs;
        }
        for (t, n) in other.tags {
            *self.tags.entry(t).or_default() += n;
        }
    }

    fn counts(self) -> SoundCounts {
        let mut counts =
            SoundCounts { lengths: self.lengths, keys: self.keys, tempos: self.tempos, ..SoundCounts::default() };
        for (t, n) in self.types {
            *counts.types.entry(t.to_ascii_lowercase()).or_default() += n;
        }
        for (t, n) in self.tags {
            *counts.tags.entry(t.to_ascii_lowercase()).or_default() += n;
        }
        counts
    }
}

/// How many sounds each choice of the chips' filters would show, with the other filters as they are
/// ([`SoundList::view_counted`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SoundCounts {
    /// By type, in lower case (ASCII letters only, as the filter matches).
    pub types: HashMap<String, usize>,
    /// Hits, then loops.
    pub lengths: [usize; 2],
    /// By key: its tonic (0 = C) times two, plus one for minor.
    pub keys: [usize; 24],
    /// In [`TEMPOS`]' order.
    pub tempos: [usize; TEMPOS.len()],
    /// By tag, in lower case (ASCII letters only).
    pub tags: HashMap<String, usize>,
}

/// The page's filters as their chips and menus show them, each choice with how many sounds it would show: the types
/// (the most used as chips of their own, the rest in a More menu), hits or loops, the key (a menu: there are 24), the
/// tempo, and the tags (chips, or a menu when there are many).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SoundChips {
    pub types: Filter,
    pub length: Filter,
    pub key: Filter,
    pub tempo: Filter,
    pub tag: Filter,
}

/// [`SoundChips`] for the filters `f`, with the `counts` of the list they let through (`None` until a list has been
/// worked out: the choices show without numbers rather than with noughts). `types` is [`SoundList::type_counts`] (the
/// most used first: an order the other filters don't change, so a chip doesn't move under the mouse when they do)
/// and `tags` is [`SoundList::tags`]. With Keys that mix well on, a key's count is how many sounds it and the keys
/// that mix well with it hold.
pub fn chips(f: &Filters, counts: Option<&SoundCounts>, types: &[(String, usize)], tags: &[String]) -> SoundChips {
    let shown = |n: &dyn Fn(&SoundCounts) -> usize| counts.map(|c| short_count(n(c))).unwrap_or_default();
    let by_word = |map: &HashMap<String, usize>, name: &str| map.get(&name.to_ascii_lowercase()).copied().unwrap_or(0);
    let types: Vec<(String, String)> =
        types.iter().map(|(t, _)| (t.clone(), shown(&|c| by_word(&c.types, t)))).collect();
    let lengths: Vec<(String, String)> = [Length::Hits, Length::Loops]
        .iter()
        .enumerate()
        .map(|(i, l)| (l.name().to_owned(), shown(&|c| c.lengths[i])))
        .collect();
    let length: Vec<String> =
        Some(f.length).filter(|l| *l != Length::All).map(|l| l.name().to_owned()).into_iter().collect();
    let in_key = |c: &SoundCounts, tonic: i32, minor: bool| -> usize {
        if f.mix_well {
            music::compatible_keys(tonic, minor).iter().map(|&(t, m)| c.keys[key_place(t, m)]).sum()
        } else {
            c.keys[key_place(tonic, minor)]
        }
    };
    let keys: Vec<(String, String)> =
        every_key().map(|(t, m)| (music::key_name(t, m, false), shown(&|c| in_key(c, t, m)))).collect();
    let keys_picked: Vec<String> = f.keys.iter().map(|&(t, m)| music::key_name(t, m, false)).collect();
    let tempos: Vec<(String, String)> =
        TEMPOS.iter().enumerate().map(|(i, t)| (t.0.to_owned(), shown(&|c| c.tempos[i]))).collect();
    let tags: Vec<(String, String)> = tags.iter().map(|t| (t.clone(), shown(&|c| by_word(&c.tags, t)))).collect();
    SoundChips {
        types: chips::show_split(&types, &f.sound_types, TYPE_CHIPS),
        length: chips::show(&lengths, &length),
        key: chips::show(&keys, &keys_picked),
        tempo: chips::show(&tempos, &f.tempos),
        tag: chips::show(&tags, &f.tags),
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

/// Of the sounds with these `ids`, the `n` that [`Order::Shuffled`] puts first for `seed`, in that order: shown at
/// once while the rest are read, the list of them is then the start of the whole list's, whatever the filters let
/// through (a sound they'd let through that comes before the last of these would be one of them). So nothing on
/// show moves when the rest arrive.
pub fn first_shuffled(ids: &[i64], seed: u64, n: usize) -> Vec<i64> {
    let mut keyed: Vec<(u64, i64)> = ids.iter().map(|&id| (shuffle_key(seed, id), id)).collect();
    if keyed.len() > n {
        keyed.select_nth_unstable(n);
        keyed.truncate(n);
    }
    keyed.sort_unstable();
    keyed.into_iter().map(|(_, id)| id).collect()
}

/// A sound's place in the shuffled order for `seed`: SplitMix64 of the two, a good spread from a cheap sum. Two
/// sounds never share one: each step turns one number into one other.
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
    wave_path_of(peaks.iter().copied())
}

/// [`wave_path`] for peaks given one by one (a sound's `Peaks`, with no list made of them).
pub fn wave_path_of(peaks: impl Iterator<Item = f32> + Clone) -> String {
    use std::fmt::Write;
    let loudest = peaks.clone().fold(0.0f32, |m, v| m.max(v.abs())).max(f32::EPSILON);
    let mut out = String::with_capacity(peaks.size_hint().0 * 16);
    for (i, p) in peaks.enumerate() {
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

/// The card's "Used in" list shows at most this many projects, the newest: a kick can be in a thousand.
pub const PROJECTS_SHOWN: usize = 50;

/// What the card's "Used in" side shows for a sound: its words on the card's switch ("Used in 4"), a word above the
/// list, and the projects, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectsCard {
    pub title: String,
    pub note: String,
    pub lines: Vec<ProjectLine>,
}

/// One project on the card: its save's id (the Songs page opens it by that), its name, its DAW and the day it was
/// last saved ("" when that isn't known).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectLine {
    pub id: i64,
    pub name: String,
    /// As the library names it ("FL Studio"), or "" for a file no reader knows.
    pub daw: String,
    pub saved: String,
}

/// The card's "Used in" side: `own`, the projects that use the sound, newest first (None while that isn't known
/// yet); `like`, for a sound Rediscover dug out, the sound its line says it sounds like, by name, with that one's
/// projects, which are listed when none use this one.
pub fn projects_card(own: Option<&[&ProjectFile]>, like: Option<(&str, &[&ProjectFile])>) -> ProjectsCard {
    let Some(own) = own else {
        return ProjectsCard {
            title: "Used in".to_owned(),
            note: "Your projects haven't been counted yet.".to_owned(),
            lines: Vec::new(),
        };
    };
    let title = format!("Used in {}", crate::songs::thousands(own.len()));
    if !own.is_empty() {
        let note = if own.len() > PROJECTS_SHOWN { format!("The newest {PROJECTS_SHOWN}:") } else { String::new() };
        return ProjectsCard { title, note, lines: project_lines(own) };
    }
    match like {
        Some((name, theirs)) if !theirs.is_empty() => {
            let newest = if theirs.len() > PROJECTS_SHOWN {
                format!(" (the newest {PROJECTS_SHOWN} here)")
            } else {
                String::new()
            };
            let note = format!(
                "No project of yours uses it yet. {name}, which it sounds like, is in {}{newest}:",
                crate::songs::plural(theirs.len(), "project")
            );
            ProjectsCard { title, note, lines: project_lines(theirs) }
        }
        _ => ProjectsCard { title, note: "No project of yours uses it yet.".to_owned(), lines: Vec::new() },
    }
}

/// The card's lines for `projects`, newest first: the newest [`PROJECTS_SHOWN`].
fn project_lines(projects: &[&ProjectFile]) -> Vec<ProjectLine> {
    projects
        .iter()
        .take(PROJECTS_SHOWN)
        .map(|f| ProjectLine {
            id: f.id,
            name: mo_formats::paths::stem(mo_formats::paths::file_name(&f.path)).to_owned(),
            daw: mo_library::scan::daw_of(&f.path).unwrap_or_default().to_owned(),
            // A save the scan didn't date (0) shows none, rather than 1 Jan 1970.
            saved: if f.mtime > 0 { crate::midi::date_text(f.mtime) } else { String::new() },
        })
        .collect()
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

/// [`used_text`] in words that make sense without the Used column's name over them, for a line of their own in a
/// narrow list: "never used", "used in 1 project", "used in 12 projects", or "" when the counts aren't known yet.
pub fn used_words(used: Option<u32>) -> String {
    match used {
        None => String::new(),
        Some(0) => "never used".into(),
        Some(n) => format!("used in {}", used_text(Some(n))),
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

/// What the card says once the app's own copy of the sound library ([`mo_sounds::compact`]) has been made: how big it
/// is beside `sounds.db`.
pub fn copy_made_line(copy_bytes: u64, library_bytes: u64) -> String {
    format!(
        "{} beside sounds.db's {}. Sounds opens from it from now on.",
        size_text(copy_bytes),
        size_text(library_bytes)
    )
}

/// What the card says once the app's own copy of the sound library has been brought up to date with what the C# app
/// changed: made afresh from it, or nothing had changed.
pub fn copy_synced_line(s: &Synced) -> String {
    if s.made {
        format!("Made afresh from {} sounds", crate::songs::thousands(s.sounds))
    } else {
        "Nothing had changed".to_owned()
    }
}

/// What the card says once the app's own sound library has been laid out anew ([`mo_sounds::compact::LaidOut`]): how
/// big it is now, and was.
pub fn laid_out_line(before: u64, after: u64) -> String {
    format!(
        "{}, from {}. The waveforms and CLAP's fingerprints now sit apart from the list, and the fingerprints take half \
         the room, so the page reads less. \"Sounds like this\" finds the same sounds as before.",
        size_text(after),
        size_text(before)
    )
}

/// What the card says when the app's own sound library couldn't be laid out anew just now, and `why` (as
/// [`mo_sounds::compact::LaidOut::NotNow`] says it).
pub fn not_laid_out_line(why: &str) -> String {
    let mut why = why.trim_end_matches('.').to_owned();
    if let Some(first) = why.get(..1) {
        why.replace_range(..1, &first.to_uppercase());
    }
    format!("{why}. It's tried again the next time Sounds opens, and until then it's read as it is.")
}

/// What the card says when the app's own sound library was found damaged as it was laid out anew, with no `sounds.db`
/// left to make it again from.
pub fn damaged_not_laid_out_line() -> String {
    "It's damaged, and there's no sounds.db to make it again from, so it's read as it is.".to_owned()
}

/// What the card says when the app's own sound library was found damaged as it was laid out anew, and couldn't be
/// made again from `sounds.db` either: why, without the library's whole path.
pub fn not_remade_line(e: &SoundsError) -> String {
    let why = match e {
        SoundsError::Copy { why, .. } => why.clone(),
        e => e.to_string(),
    };
    format!("It's damaged, and it couldn't be made again from sounds.db ({why}), so it's read as it is.")
}

/// Why the app's own copy of the sound library couldn't be made or brought up to date, for a card: what went wrong,
/// without the copy's whole path.
pub fn copy_failed_line(e: &SoundsError) -> String {
    match e {
        SoundsError::Copy { why, .. } => {
            let mut why = why.clone();
            if let Some(first) = why.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            why
        }
        e => e.to_string(),
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
        let used = |years: Vec<i32>| Usage { projects: years.len() as u32, recent: 0, years, ..Usage::default() };
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
        assert_eq!(by(Filters { tags: vec!["dark".into()], ..Filters::default() }), vec![3]);
        assert_eq!(by(Filters { keys: vec![(5, true)], ..Filters::default() }), vec![3]);
        assert_eq!(by(Filters { keys: vec![(5, true), (9, true)], ..Filters::default() }), vec![3, 2]);
        let tempos = |t: &[&str]| Filters { tempos: t.iter().map(|w| (*w).to_owned()).collect(), ..Filters::default() };
        assert_eq!(by(tempos(&["130 to 150"])), vec![2]);
        // Both ends are in, so 90 is up to 90 and 90 to 110; a sound with no tempo is in none.
        assert_eq!(by(tempos(&["Up to 90"])), vec![4]);
        assert_eq!(by(tempos(&["90 to 110", "130 to 150"])), vec![4, 2]);
        assert_eq!(by(Filters { search: "samples LOOPS".into(), ..Filters::default() }), vec![2]);
        assert_eq!(by(Filters { search: "kick pad".into(), ..Filters::default() }), Vec::<i64>::new());
    }

    #[test]
    fn a_folder_lists_only_its_own_sounds() {
        let list = SoundList::new(vec![
            sound(1, r"I:\Samples\Kicks\Punchy.wav", "Kick", None),
            sound(2, r"i:\samples\KICKS\Deep.wav", "Kick", None),
            sound(3, r"I:\Samples\Kicks 2\Thud.wav", "Kick", None),
            sound(4, r"I:\Samples\Kicks\Old\Clap.wav", "Clap", None),
            sound(5, r"I:\Samples\Kickstarter\Hat.wav", "Hat", None),
        ]);
        let folder = |f: &str| Filters { folder: Some(f.to_owned()), ..Filters::default() };
        // The search the page used to put the folder in lets three other folders' sounds through too.
        assert_eq!(
            ids(
                &list,
                &list.view(&Filters { search: r"I:\Samples\Kicks".into(), ..Filters::default() }, Order::Name, 0)
            ),
            vec![4, 2, 5, 1, 3]
        );
        assert_eq!(ids(&list, &list.view(&folder(r"I:\Samples\Kicks"), Order::Name, 0)), vec![2, 1]);
        // The type chips count within the folder, and its chip takes it off.
        let (_, counts) = list.view_counted(&folder(r"I:\Samples\Kicks 2"), Order::Name, 0);
        assert_eq!(counts.types.get("kick"), Some(&1));
        assert_eq!(counts.types.len(), 1);
        let mut f = folder(r"I:\Samples\Kicks");
        assert!(f.any());
        f.toggle("folder");
        assert_eq!(f, Filters::default());
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
        let f = Filters { keys: vec![(9, true)], mix_well: true, ..Filters::default() };
        assert_eq!(ids(&list, &list.view(&f, Order::Name, 0)), vec![4, 2]);
        // And the key's count is the sounds in it and the keys that mix well with it.
        let (_, counts) = list.view_counted(&f, Order::Name, 0);
        let keys = chips(&f, Some(&counts), &list.type_counts(), &list.tags()).key;
        let count = |key: &str| keys.more.iter().find(|c| c.text == key).map(|c| c.count.clone());
        assert_eq!(count("A minor").as_deref(), Some("2"));
        assert_eq!(count("F minor").as_deref(), Some("1"));
        assert_eq!(keys.more_picked, "A minor");
    }

    #[test]
    fn picking_on_the_filters_chips_and_menus() {
        let mut f = Filters::default();
        f.pick(TYPE, "Kick");
        f.pick(TYPE, "Vocal");
        f.pick(KEY, "A minor");
        f.pick(KEY, "C major");
        f.pick(KEY, "Am");
        f.pick(TEMPO, "90 to 110");
        f.pick(TEMPO, "No such tempo");
        f.pick(TAG, "Dark");
        f.pick(TAG, "Warm");
        f.pick(LENGTH, "Loops");
        assert_eq!(f.sound_types, ["Kick", "Vocal"]);
        assert_eq!(f.keys, [(0, false)]);
        assert_eq!(f.tempos, ["90 to 110"]);
        assert_eq!(f.tags, ["Dark", "Warm"]);
        assert_eq!(f.length, Length::Loops);
        // One length: hits takes the place of loops, and picking it again is both.
        f.pick(LENGTH, "Hits");
        assert_eq!(f.length, Length::Hits);
        f.pick(LENGTH, "Hits");
        assert_eq!(f.length, Length::All);
        // The More menu's Any keeps the types picked on chips of their own.
        let types: Vec<(String, usize)> = (0..12).map(|n| (format!("Type {n}"), 12 - n)).collect();
        let mut more = Filters { sound_types: vec!["Type 0".into(), "Type 11".into()], ..Filters::default() };
        more.clear_more_types(&types);
        assert_eq!(more.sound_types, ["Type 0"]);
        for chip in [TYPE, LENGTH, KEY, TEMPO, TAG] {
            f.clear_chip(chip);
        }
        assert!(!f.any());
        // Keys that mix well with no key picked changes nothing, so Clear doesn't show for it.
        f.toggle("mix-well");
        assert!(f.mix_well && !f.any());
        f.pick(KEY, "F minor");
        assert!(f.any());
    }

    #[test]
    fn each_choice_counts_what_it_would_show_with_the_other_filters() {
        let list = library();
        let (shown, counts) = list.view_counted(&Filters::default(), Order::Name, 0);
        assert_eq!(shown.len(), 4);
        assert_eq!(counts.lengths, [2, 2]);
        assert_eq!(counts.tempos, [1, 1, 0, 1, 0]);
        assert_eq!(counts.tags.get("dark"), Some(&1));
        // Kick picked: the other types still count (picking one shows it too), the rest count only kicks.
        let kick = Filters { sound_types: vec!["kick".into()], ..Filters::default() };
        let (shown, counts) = list.view_counted(&kick, Order::Name, 0);
        assert_eq!(ids(&list, &shown), [1]);
        assert_eq!(counts.types.values().sum::<usize>(), 4);
        assert_eq!((counts.lengths, counts.keys.iter().sum::<usize>()), ([1, 0], 0));
        // With Loops too, the kick (a hit) counts beside Hits only, and the loops beside their types.
        let loops = Filters { length: Length::Loops, ..kick };
        let (shown, counts) = list.view_counted(&loops, Order::Name, 0);
        assert!(shown.is_empty());
        assert_eq!(counts.lengths, [1, 0]);
        assert_eq!(
            (counts.types.get("bass"), counts.types.get("pad"), counts.types.get("kick")),
            (Some(&1), Some(&1), None)
        );
        // The chips say so: the types as chips (four is few), the key in a menu (there are 24).
        let shown = chips(&loops, Some(&counts), &list.type_counts(), &list.tags());
        let words = |f: &Filter| f.chips.iter().map(|c| format!("{} {}", c.text, c.count)).collect::<Vec<_>>();
        assert_eq!(words(&shown.types), ["Bass 1", "Kick 0", "Pad 1", "Vocal 0"]);
        assert_eq!(words(&shown.length), ["Hits 1", "Loops 0"]);
        assert_eq!((shown.key.chips.len(), shown.key.more.len()), (0, 24));
        assert_eq!(words(&shown.tag), ["Dark 0"]);
        // Before the first list is worked out, the choices show with no numbers.
        let uncounted = chips(&loops, None, &list.type_counts(), &list.tags());
        assert!(uncounted.types.chips.iter().chain(&uncounted.key.more).all(|c| c.count.is_empty()));
        // A tag a sound has twice, in other capitals, counts it once.
        let mut twice = sound(5, "x.wav", "Pad", None);
        twice.tags = vec!["Dark".into(), "DARK".into()];
        let list = SoundList::new(vec![twice]);
        assert_eq!(list.view_counted(&Filters::default(), Order::Name, 0).1.tags.get("dark"), Some(&1));
    }

    #[test]
    fn used_chips_wait_for_the_counts() {
        let mut list = library();
        let f = Filters { unused: true, ..Filters::default() };
        assert_eq!(list.view(&f, Order::Name, 0).len(), 4);
        let used = |projects| Usage { projects, recent: 0, years: vec![2026], ..Usage::default() };
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
    fn the_sounds_shown_first_are_the_start_of_the_whole_shuffled_list_whatever_the_filters() {
        let types = ["Kick", "Snare", "Pad", "Loop"];
        let sounds: Vec<Sound> = (1..=2_000)
            .map(|id| {
                let key = (id % 3 != 0).then_some(((id % 12) as i32, id % 2 == 0));
                let path = format!(r"I:\Samples\Pack {}\Sound {id}.wav", id % 7);
                let mut s = sound(id * 3 + 1, &path, types[(id % 4) as usize], analysed(id % 5 == 0, None, key));
                s.added = NaiveDate::from_ymd_opt(2026, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0)).unwrap_or_default();
                s
            })
            .collect();
        let every: Vec<i64> = sounds.iter().map(|s| s.id).collect();
        let whole = SoundList::new(sounds.clone());
        let mut kicks = Filters::default();
        kicks.toggle_type("Kick");
        let mut loops = Filters::default();
        loops.pick(LENGTH, Length::Loops.name());
        let searched = Filters { search: "pack 3".into(), ..Filters::default() };
        for seed in [1, 7, 0xDEAD_BEEF] {
            let first = first_shuffled(&every, seed, 200);
            assert_eq!(first.len(), 200);
            let shown_first: Vec<Sound> = sounds.iter().filter(|s| first.contains(&s.id)).cloned().collect();
            let part = SoundList::new(shown_first);
            for filters in [&Filters::default(), &kicks, &loops, &searched] {
                let start = ids(&part, &part.view(filters, Order::Shuffled, seed));
                let all = ids(&whole, &whole.view(filters, Order::Shuffled, seed));
                assert!(!start.is_empty() && start.len() < all.len());
                assert_eq!(start, all[..start.len()]);
            }
            assert_eq!(first, ids(&part, &part.view(&Filters::default(), Order::Shuffled, seed)));
        }
        // Fewer sounds than asked for: all of them, in order.
        let few = first_shuffled(&every[..5], 7, 200);
        assert_eq!(few.len(), 5);
        assert_eq!(
            few,
            ids(&whole, &whole.view(&Filters::default(), Order::Shuffled, 7))
                .into_iter()
                .filter(|id| every[..5].contains(id))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn type_chips_count_most_first_and_unknown_last() {
        let mut sounds = vec![sound(1, "a.wav", "Kick", None), sound(2, "b.wav", "Kick", None)];
        sounds.push(sound(3, "c.wav", types::UNKNOWN, None));
        sounds.push(sound(4, "d.wav", types::UNKNOWN, None));
        sounds.push(sound(5, "e.wav", types::UNKNOWN, None));
        sounds.push(sound(6, "f.wav", "Pad", None));
        // Another sound's type in other capitals is the same type, named as most of them have it.
        sounds.push(sound(7, "g.wav", "kick", None));
        let list = SoundList::new(sounds);
        let counts = list.type_counts();
        assert_eq!(counts[0], ("Kick".to_string(), 3));
        assert_eq!(counts[1], ("Pad".to_string(), 1));
        assert_eq!(counts[2], (types::UNKNOWN.to_string(), 3));
        assert_eq!(counts.len(), 3);
        let f = Filters { sound_types: vec!["Kick".into()], ..Filters::default() };
        assert_eq!(list.view(&f, Order::Name, 0).len(), 3);
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
        assert_eq!(used_words(None), "");
        assert_eq!(used_words(Some(0)), "never used");
        assert_eq!(used_words(Some(1)), "used in 1 project");
        assert_eq!(used_words(Some(12)), "used in 12 projects");
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
    fn a_list_read_again_takes_the_old_ones_place_only_with_the_same_sounds_in_order() {
        let list =
            |ids: &[i64]| SoundList::new(ids.iter().map(|&i| sound(i, &format!("{i}.wav"), "Kick", None)).collect());
        let shown = list(&[3, 1, 2]);
        let mut played = list(&[3, 1, 2]);
        played.set_liked(|s| s.id as f64);
        assert!(shown.same_sounds_as(&played) && played.same_sounds_as(&shown));
        assert!(shown.same_sounds_as(&shown.sharing()));
        // A sound new, gone or moved in the order: the page shows the list afresh.
        for other in [list(&[3, 1, 2, 4]), list(&[3, 1]), list(&[1, 3, 2]), list(&[3, 1, 5])] {
            assert!(!shown.same_sounds_as(&other), "{:?}", other.sounds().iter().map(|s| s.id).collect::<Vec<_>>());
        }
        // A copy hidden in the C# app is left out of the list, so it counts as gone.
        let mut hidden: Vec<Sound> = (1..=3).map(|i| sound(i, &format!("{i}.wav"), "Kick", None)).collect();
        hidden[1].hidden = true;
        assert!(!list(&[1, 2, 3]).same_sounds_as(&SoundList::new(hidden)));
    }

    #[test]
    fn the_copys_cards_say_what_changed_in_plain_words() {
        assert_eq!(
            copy_made_line(1_621_000_000, 4_896_000_000),
            "1.51 GB beside sounds.db's 4.56 GB. Sounds opens from it from now on."
        );
        let synced = |made, sounds| Synced { made, sounds, adopted: false };
        assert_eq!(copy_synced_line(&synced(false, 0)), "Nothing had changed");
        assert_eq!(copy_synced_line(&synced(true, 453_885)), "Made afresh from 453,885 sounds");
        assert_eq!(
            laid_out_line(1_621_000_000, 933_000_000),
            "890 MB, from 1.51 GB. The waveforms and CLAP's fingerprints now sit apart from the list, and the \
             fingerprints take half the room, so the page reads less. \"Sounds like this\" finds the same sounds as \
             before."
        );
        assert_eq!(
            not_laid_out_line("something else has it open (a page reading it, or another copy of the app)"),
            "Something else has it open (a page reading it, or another copy of the app). It's tried again the next \
             time Sounds opens, and until then it's read as it is."
        );
        let short =
            SoundsError::Copy { path: "I:\\MusicOrganiser\\sounds-rust.db".into(), why: "only 0.4 GB is free".into() };
        assert_eq!(
            not_remade_line(&short),
            "It's damaged, and it couldn't be made again from sounds.db (only 0.4 GB is free), so it's read as it is."
        );
        let full = SoundsError::Copy {
            path: "C:\\Users\\Solomon\\AppData\\Local\\MusicOrganiser\\sounds-rust.db".into(),
            why: "only 3.1 GB is free on its disk".into(),
        };
        assert_eq!(copy_failed_line(&full), "Only 3.1 GB is free on its disk");
        let read = SoundsError::Open { path: "sounds.db".into(), why: "it's locked".into() };
        assert_eq!(copy_failed_line(&read), read.to_string());
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

    fn project(id: i64, path: &str, mtime: i64) -> ProjectFile {
        ProjectFile { id, path: path.to_owned(), mtime }
    }

    #[test]
    fn the_card_lists_the_projects_that_use_a_sound_newest_first() {
        let a = project(7, r"K:\Projects\Didums_77.flp", 1_760_000_000);
        let b = project(3, r"K:\Projects\Old one.bwproject", 1_600_000_000);
        // Not known yet.
        let card = projects_card(None, None);
        assert_eq!((card.title.as_str(), card.lines.len()), ("Used in", 0));
        // Its own, newest first, by name without the ending, with its DAW and the day each was saved.
        let card = projects_card(Some(&[&a, &b][..]), None);
        assert_eq!(card.title, "Used in 2");
        assert_eq!(card.note, "");
        let names: Vec<(i64, &str, &str)> =
            card.lines.iter().map(|l| (l.id, l.name.as_str(), l.daw.as_str())).collect();
        assert_eq!(names, [(7, "Didums_77", "FL Studio"), (3, "Old one", "Bitwig Studio")]);
        assert_eq!(card.lines[0].saved, crate::midi::date_text(1_760_000_000));
        // A save the scan didn't date shows no day, rather than 1 Jan 1970.
        let undated = project(9, r"K:\Projects\Undated.flp", 0);
        assert_eq!(projects_card(Some(&[&undated][..]), None).lines[0].saved, "");
        // None of its own: the sound it sounds like has some.
        let card = projects_card(Some(&[][..]), Some(("VEH1 FX - 37", &[&a][..])));
        assert_eq!(card.title, "Used in 0");
        assert_eq!(card.note, "No project of yours uses it yet. VEH1 FX - 37, which it sounds like, is in 1 project:");
        assert_eq!(card.lines.len(), 1);
        // Neither has any.
        let card = projects_card(Some(&[][..]), Some(("VEH1 FX - 37", &[][..])));
        assert_eq!((card.note.as_str(), card.lines.len()), ("No project of yours uses it yet.", 0));
        // A sound in more projects than the card lists: the newest.
        let many: Vec<ProjectFile> =
            (0..1_200).map(|n| project(n, &format!("K:\\P\\{n}.flp"), 1_700_000_000 - n)).collect();
        let many: Vec<&ProjectFile> = many.iter().collect();
        let card = projects_card(Some(many.as_slice()), None);
        assert_eq!((card.title.as_str(), card.note.as_str()), ("Used in 1,200", "The newest 50:"));
        assert_eq!(card.lines.len(), PROJECTS_SHOWN);
        let card = projects_card(Some(&[][..]), Some(("VEH1 FX - 37", many.as_slice())));
        assert!(card.note.ends_with("is in 1,200 projects (the newest 50 here):"), "{}", card.note);
    }

    #[test]
    fn the_list_names_a_sounds_projects_newest_first_once_both_are_known() {
        let mut list = SoundList::new(vec![sound(1, "a.wav", "Kick", None), sound(2, "b.wav", "Kick", None)]);
        // Nothing known yet.
        assert_eq!(list.projects_of(0), None);
        list.set_used(HashMap::from([(1, Usage { projects: 3, project_ids: vec![10, 11, 12], ..Usage::default() })]));
        assert_eq!(list.projects_of(0), None, "the files aren't known yet");
        // Project 12's file has gone: it isn't listed.
        let files = HashMap::from([(10, project(10, "old.flp", 100)), (11, project(11, "new.flp", 300))]);
        list.set_project_files(Arc::new(files));
        let of = list.projects_of(0).unwrap_or_default();
        assert_eq!(of.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["new.flp", "old.flp"]);
        // One no project uses: none.
        assert_eq!(list.projects_of(1).map(|of| of.len()), Some(0));
        // A list sharing its sounds starts without them, as it does without the use.
        assert_eq!(list.sharing().projects_of(0), None);
    }
}
