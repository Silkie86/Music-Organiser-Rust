//! The Sounds page's list: which sounds the filters let through, in which order, and the words each tile and line
//! shows. Ported from the C# `SoundsViewModel` (`MakeFilter`, `MakeOrder`), with the work done the Rust way: each
//! sound's search words are lower-cased once when the list is made, and filtering and sorting spread over every core
//! (rule 21). Call [`SoundList::view`] off the window's thread; it takes well under a second for 357,000 sounds.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Datelike;
use mo_formats::dates::RealDates;
use mo_library::ProjectFile;
use mo_sounds::compact::Synced;
use mo_sounds::copies::SameAudio;
use mo_sounds::index::SoundIndex;
use mo_sounds::usage::Usage;
use mo_sounds::{Sound, SoundsError, Word, learn, music, rediscover, types};
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
    /// The file changed last first; a date that isn't real (still to come, or before 1981) last, either way round
    /// ([`RealDates`]).
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

    /// The List view's column this order goes by, as its heading says it; "" for an order no column stands for. A
    /// click on the heading picks the order, a second click turns it round (the UX review of 10 Nov, B5).
    pub fn column(self) -> &'static str {
        match self {
            Order::Name => "Sound",
            Order::Type => "Type",
            Order::Key => "Key",
            Order::Shuffled | Order::MostLiked | Order::Newest => "",
        }
    }

    pub fn from_column(column: &str) -> Option<Order> {
        Order::ALL.into_iter().find(|o| !column.is_empty() && o.column() == column)
    }
}

/// The filters' names, as the page calls them when a choice is picked.
pub const TYPE: &str = "type";
pub const LENGTH: &str = "length";
pub const KEY: &str = "key";
pub const TEMPO: &str = "tempo";
pub const TAG: &str = "tag";
/// The Hide duplicates chip's name, as the page gives it to [`Filters::toggle`]; also the setting it's kept in.
pub const HIDE_COPIES: &str = "hide-copies";
/// The Hide silent sounds line's name, as the page gives it to [`Filters::toggle`].
pub const HIDE_SILENT: &str = "hide-silent";

/// The Tempo slider's two ends in BPM, and how far a handle moves at a time. A handle at an end sets no limit there:
/// the lowest place is "however slow", the highest "however fast".
pub const TEMPO_LOW: i32 = 60;
pub const TEMPO_HIGH: i32 = 200;
pub const TEMPO_STEP: i32 = 5;
/// The bars over the Tempo slider: how many sounds there are at each tempo, one bar for every [`TEMPO_BAR`] BPM from
/// [`TEMPO_LOW`] to [`TEMPO_HIGH`]. Slower sounds are in the first bar and faster ones in the last.
pub const TEMPO_BARS: usize = 14;
const TEMPO_BAR: i32 = 10;

/// How many types are chips of their own, the most used; the rest are in the type filter's More menu.
pub const TYPE_CHIPS: usize = 9;

/// A More types menu's Any (the Type filter's and Rediscover's alike): unpicks from `picked` the types picked in the
/// menu, and keeps the ones picked on chips or tick lines of their own, in any case. `types` is
/// [`SoundList::type_counts`], as the chips were shown from, and the first `shown` of them are on show: as many as the
/// filter column has room for ([`chips_showing`]), [`TYPE_CHIPS`] at the least.
pub fn keep_first_types(picked: &mut Vec<String>, types: &[(String, usize)], shown: usize) {
    let on_chips = &types[..types.len().min(shown)];
    picked.retain(|t| on_chips.iter().any(|(c, _)| c.eq_ignore_ascii_case(t)));
}

/// The Favourites filter's "how adventurous" steps ([`Filters::like_favourites`]): 0 to this.
pub const LIKE_STEPS: u8 = 8;

/// How many sounds like each favourite of their type show beside the favourites at step `step` (0 to
/// [`LIKE_STEPS`]): none, 1, 3, 7, 15, 31, 63, 127 and 255, each step about twice as far as the last.
pub fn per_favourite(step: u8) -> u32 {
    (1u32 << step.min(LIKE_STEPS)) - 1
}

/// How far along the sounds like your favourites are, for the words by the slider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LikeKnown {
    Yes,
    Finding,
    /// They can't be found: the fingerprints couldn't be read.
    Missing,
}

/// The words by the Favourites filter's "how adventurous" slider at step `step`: "Only them" at 0, else how many like
/// each favourite show ("+3 each"), "Finding…" while they're being found, or "No fingerprints" when they can't be.
pub fn like_favourites_words(step: u8, known: LikeKnown) -> String {
    match (step, known) {
        (0, _) => "Only them".to_owned(),
        (_, LikeKnown::Finding) => "Finding…".to_owned(),
        (_, LikeKnown::Missing) => "No fingerprints".to_owned(),
        (step, LikeKnown::Yes) => format!("+{} each", per_favourite(step)),
    }
}

/// Where the slider is (0 to 1) at step `step`.
pub fn like_favourites_share(step: u8) -> f32 {
    f32::from(step.min(LIKE_STEPS)) / f32::from(LIKE_STEPS)
}

/// The step the slider at `share` (0 to 1 along it) picks, to the nearest.
pub fn like_favourites_step(share: f32) -> u8 {
    let share = if share.is_finite() { share.clamp(0.0, 1.0) } else { 0.0 };
    (share * f32::from(LIKE_STEPS)).round() as u8
}

/// Where the setting keeps how far the sounds like your favourites reach (a step, 0 to [`LIKE_STEPS`]).
pub const LIKE_FAVOURITES_KEY: &str = "sounds_like_favourites";

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
    /// The Tempo slider's lowest and highest tempo, both ends in ([`Filters::set_tempo`]). No lowest means however
    /// slow, no highest however fast; with either set, a sound with no tempo is left out.
    pub tempo: (Option<f64>, Option<f64>),
    /// Only sounds no project uses. Ignored until the use counts are known.
    pub unused: bool,
    /// Only sounds some project uses. Ignored until the use counts are known.
    pub used: bool,
    pub favourites: bool,
    /// With Favourites on: how far the sounds like them reach too, as a step from 0 (only the favourites) to
    /// [`LIKE_STEPS`] (the most adventurous): [`per_favourite`] sounds like each favourite show beside it. Not a filter
    /// of its own: it changes nothing with Favourites off, and the Clear chip leaves it as you set it.
    pub like_favourites: u8,
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
    /// Hide duplicates: of sounds whose audio is exactly the same ([`mo_sounds::copies`]), only one shows, the one to
    /// keep of those the other filters let through ([`SoundList::set_copies`]). Not a filter the Clear chip takes off:
    /// it stays as you set it.
    pub hide_copies: bool,
    /// Shows the sounds that are silent all through, or empty ([`Sound::is_silent`]), too. Off, as it starts: they're
    /// left out, and the Hide silent sounds line says how many (Solomon, 12 Nov 2026: "hide silent audio files"). Not
    /// a filter Clear filters takes off: it stays as you set it.
    pub show_silent: bool,
}

impl Filters {
    /// Picks a type, or takes it off again when it's already picked.
    pub fn toggle_type(&mut self, sound_type: &str) {
        chips::toggle(&mut self.sound_types, sound_type, true);
    }

    /// Picks or unpicks `choice` (as its chip or menu line says it) on the filter `chip` ([`TYPE`], [`LENGTH`],
    /// [`KEY`], [`TAG`]). More than one type, key or tag can be picked; one length, as hits and loops together are
    /// every sound. The tempo isn't picked from choices: it has a slider ([`Filters::set_tempo`]).
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
            TAG => chips::toggle(&mut self.tags, choice, true),
            _ => {}
        }
    }

    /// The Tempo slider's handles were put at `low` and `high` BPM. They go to the nearest step between
    /// [`TEMPO_LOW`] and [`TEMPO_HIGH`], the lower one first; a handle at an end sets no limit there, so both at
    /// their ends is every tempo, and the sounds with none.
    pub fn set_tempo(&mut self, low: i32, high: i32) {
        let step = |bpm: i32| {
            let bpm = bpm.clamp(TEMPO_LOW, TEMPO_HIGH);
            TEMPO_LOW + (bpm - TEMPO_LOW + TEMPO_STEP / 2) / TEMPO_STEP * TEMPO_STEP
        };
        let (low, high) = (step(low.min(high)), step(low.max(high)));
        self.tempo = ((low > TEMPO_LOW).then_some(f64::from(low)), (high < TEMPO_HIGH).then_some(f64::from(high)));
    }

    /// Picks nothing on the filter `chip`: its menu's Any, or Clear types.
    pub fn clear_chip(&mut self, chip: &str) {
        match chip {
            TYPE => self.sound_types.clear(),
            LENGTH => self.length = Length::All,
            KEY => self.keys.clear(),
            TEMPO => self.tempo = (None, None),
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
            HIDE_COPIES => self.hide_copies = !self.hide_copies,
            HIDE_SILENT => self.show_silent = !self.show_silent,
            // Its chip shows only while it's on: a click takes it off.
            "folder" => self.folder = None,
            _ => {}
        }
        if self.used != by_use {
            self.years = None;
        }
    }

    /// Whether anything narrows the list that Clear filters takes off: a filter, or words in the search box. Not Keys
    /// that mix well with no key picked: it changes nothing then, and its chip isn't shown. Nor Hide duplicates, Hide
    /// silent sounds, nor how far the sounds like your favourites reach: Clear filters leaves them as they are.
    pub fn any(&self) -> bool {
        let mix_well = self.mix_well && self.keys.is_empty();
        let none = Filters { search: self.search.clone(), mix_well, ..self.cleared() };
        *self != none || !self.search.trim().is_empty()
    }

    /// No filters and no search, as Clear filters leaves them, as on every page: Hide duplicates, Hide silent sounds and
    /// how far the sounds like your favourites reach stay as they are. A search left in the box went on narrowing the
    /// list after Clear filters, unseen (Solomon, 12 Nov 2026: "why are only these showing as favourites", then "same
    /// issue").
    pub fn cleared(&self) -> Filters {
        Filters {
            hide_copies: self.hide_copies,
            show_silent: self.show_silent,
            like_favourites: self.like_favourites,
            ..Filters::default()
        }
    }

    /// Whether the list shows sounds like your favourites beside them: Favourites on, a step above only them.
    pub fn wants_like_favourites(&self) -> bool {
        self.favourites && self.like_favourites > 0
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
    /// Which sounds' dates are real, as of when the list was read: the Newest order, the years and which copy is the
    /// older all go by it, so they agree for as long as the list lasts.
    dates: RealDates,
    /// How many of the sounds are silent all through, or empty: what Hide silent sounds leaves out.
    silent: usize,
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
    used: Option<Arc<HashMap<i64, Usage>>>,
    /// Each project's file, by id, once known: the projects a sound is used in are named from it. Shared by a clone.
    projects: Option<Arc<HashMap<i64, ProjectFile>>>,
    /// Each sound's Most liked score, in `sounds` order, once known.
    liked: Option<Arc<Vec<f64>>>,
    /// The best of `liked`, worked out once, as the galaxy asks every sound for its share of it.
    liked_best: f64,
    /// Which sounds are copies of each other (the same audio), once known: shared by a clone, and kept by
    /// [`SoundList::sharing`], as it's of the same sounds.
    copies: Option<Arc<Copies>>,
    /// Your say about sounds since they were read (a favourite, your own type, tags), by sound id
    /// ([`SoundList::with_say`]): over what each sound says, for the filters, the chips' counts, the orders and
    /// [`SoundList::now`]. Kept by [`SoundList::sharing`], as it's of the same sounds.
    say: Option<Arc<HashMap<i64, YourSay>>>,
    /// How close each sound is to your favourites of its type, once worked out ([`SoundList::like_favourites_in`]):
    /// for the Favourites filter's sounds like them. Kept by [`SoundList::sharing`], as it's of the same sounds.
    like: Option<Arc<LikeFavourites>>,
}

/// How close each sound of a list is to your favourites of its type ([`mo_sounds::near::reach`]), by place: how many
/// sounds like each favourite must show for it to show.
#[derive(Debug, Default)]
pub struct LikeFavourites {
    reach: Vec<u32>,
    /// Which sounds were favourites when it was worked out: one you've since made not a favourite isn't let through
    /// as one like them (its reach says 0) while it's worked out again.
    was_favourite: Vec<bool>,
    /// What it was worked out from ([`SoundList::like_key`]).
    key: u64,
}

/// Your say about a sound since it was read: what you changed, over what the sound says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct YourSay {
    pub favourite: Option<bool>,
    /// Your own type: `Some(None)` when you took yours away.
    pub own_type: Option<Option<Word>>,
    /// Its tags as they are now, sorted as the library sorts them (byte by byte).
    pub tags: Option<Vec<String>>,
}

/// A sound as it is now ([`SoundList::now`]): the sound as it was read, with your say since over it.
#[derive(Clone, Copy, Debug)]
pub struct Now<'a> {
    pub sound: &'a Sound,
    pub favourite: bool,
    /// The type shown, and where it came from ("you", "name", "learned", "model", or "").
    pub sound_type: &'a str,
    pub type_source: &'a str,
    /// Your own type, when you've given it one.
    pub own_type: Option<&'a str>,
    pub tags: &'a [String],
    /// Your say changed what the search looks for in it (its type or its tags) since its words were worked out.
    words_changed: bool,
}

impl<'a> Now<'a> {
    /// `s` with `say` over it. Your own type taken away shows the name's type again, or Unknown when the name gives
    /// none, as the C# app's `SetUserType` did.
    pub fn of(s: &'a Sound, say: Option<&'a YourSay>) -> Now<'a> {
        let mut now = Now {
            sound: s,
            favourite: s.favourite,
            sound_type: s.sound_type.as_str(),
            type_source: s.type_source.as_str(),
            own_type: s.user_type.as_deref(),
            tags: &s.tags,
            words_changed: false,
        };
        let Some(say) = say else { return now };
        now.words_changed = say.own_type.is_some() || say.tags.is_some();
        if let Some(f) = say.favourite {
            now.favourite = f;
        }
        match &say.own_type {
            Some(Some(own)) => {
                (now.sound_type, now.type_source, now.own_type) = (own.as_str(), "you", Some(own.as_str()))
            }
            Some(None) => {
                let (named, source, _) = learn::named_or_unknown(s.name_type.as_deref());
                (now.sound_type, now.type_source, now.own_type) = (named, source, None);
            }
            None => {}
        }
        if let Some(tags) = &say.tags {
            now.tags = tags;
        }
        now
    }
}

/// The sounds whose audio is exactly the same as another's, by their places in a list ([`SoundList::copies_in`]): the
/// same for every list sharing its sounds.
#[derive(Debug, Default)]
pub struct Copies {
    /// Each such sound's group, by place.
    group_of: HashMap<usize, u32>,
    /// How many sounds are copies of another (each group's sounds but one).
    copies: usize,
    /// Each group's sounds, by place.
    groups: Vec<Vec<usize>>,
    /// Each grouped sound's file size in bytes, by place, when known.
    sizes: HashMap<usize, u64>,
}

/// What told a sound to keep from its copy ([`SoundList::why_kept`]): the first of these that does, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeptFor {
    /// Your projects use it more.
    Used,
    Favourite,
    Plays,
    Tags,
    /// The older file (most likely the first).
    Older,
    /// Nothing else did: by path.
    Path,
}

/// A sound's copies (the same audio), as [`SoundList::copy_groups`] gives them: by place in the list, the one to keep
/// first, each with its file's size in bytes (0 when not known).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyGroup {
    pub sounds: Vec<(usize, u64)>,
}

impl CopyGroup {
    /// The room the copies take: every file but the one to keep.
    pub fn spare_bytes(&self) -> u64 {
        self.sounds.iter().skip(1).fold(0u64, |sum, (_, b)| sum.saturating_add(*b))
    }
}

impl Copies {
    /// How many sounds are copies of another: what Hide duplicates leaves out.
    pub fn count(&self) -> usize {
        self.copies
    }
}

/// Knows which sounds a list holds without keeping them ([`SoundList::sounds_of`]): while it's held, no other list's
/// sounds can be taken for them.
#[derive(Clone, Default)]
pub struct SoundsOf(std::sync::Weak<Shared>);

impl SoundsOf {
    /// Whether `list` holds these sounds (it shares them: a clone, or a list with the use counts in).
    pub fn is_of(&self, list: &SoundList) -> bool {
        self.0.upgrade().is_some_and(|s| Arc::ptr_eq(&s, &list.shared))
    }
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
    /// A list's use counts (one per sound your songs use, each with its years) go the same way when there are many and
    /// no other list shares them.
    fn drop(&mut self) {
        if self.used.as_ref().is_some_and(|u| u.len() >= FREE_LATER_FROM && Arc::strong_count(u) == 1)
            && !crate::background::freeing_here()
        {
            crate::background::free_later(self.used.take());
        }
    }
}

impl SoundList {
    /// The list from the library's sounds: all but the copies you've hidden as duplicates, and the files programs left
    /// in a temp folder that an older scan took in ([`mo_sounds::scan::in_temp_folder`]).
    pub fn new(sounds: Vec<Sound>) -> SoundList {
        let mut sounds: Vec<Sound> =
            sounds.into_par_iter().filter(|s| !s.hidden && !mo_sounds::scan::in_temp_folder(&s.path)).collect();
        // Leaving out the hidden copies keeps the room they took: give it back.
        sounds.shrink_to_fit();
        let hay = sounds.par_iter().map(|s| hay_of(&Now::of(s, None))).collect();
        let names = sounds.par_iter().map(|s| s.name().to_lowercase()).collect();
        let silent = sounds.par_iter().filter(|s| s.is_silent()).count();
        SoundList {
            shared: Arc::new(Shared { sounds, hay, names, dates: RealDates::now(), silent }),
            used: None,
            projects: None,
            liked: None,
            liked_best: 0.0,
            copies: None,
            say: None,
            like: None,
        }
    }

    /// A list of the same sounds, sharing them, without this one's use or Most liked yet: what a list with the use
    /// counted in starts from.
    pub fn sharing(&self) -> SoundList {
        SoundList {
            shared: self.shared.clone(),
            used: None,
            projects: None,
            liked: None,
            liked_best: 0.0,
            copies: self.copies.clone(),
            say: self.say.clone(),
            like: self.like.clone(),
        }
    }

    /// How many of its sounds are silent all through, or empty ([`Sound::is_silent`]): what Hide silent sounds leaves
    /// out while it's on.
    pub fn silent(&self) -> usize {
        self.shared.silent
    }

    /// Knows this list's sounds without keeping them.
    pub fn sounds_of(&self) -> SoundsOf {
        SoundsOf(Arc::downgrade(&self.shared))
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
        self.used = Some(Arc::new(used));
    }

    /// The file of each project that uses a sound, by id ([`mo_library::Library::sample_usage_with_projects`]), to
    /// name the projects a sound is used in.
    pub fn set_project_files(&mut self, projects: Arc<HashMap<i64, ProjectFile>>) {
        self.projects = Some(projects);
    }

    /// The file of each project that uses a sound, as [`SoundList::set_project_files`] gave them, if it has.
    pub fn project_files(&self) -> Option<&Arc<HashMap<i64, ProjectFile>>> {
        self.projects.as_ref()
    }

    /// The projects that use the sound at `i`, newest first (each one's file and when it was saved), a date that isn't
    /// real last. None while that isn't known; empty when none do.
    pub fn projects_of(&self, i: usize) -> Option<Vec<&ProjectFile>> {
        let files = self.projects.as_deref()?;
        self.used.as_ref()?;
        let ids = self.usage(i).map_or(&[][..], |u| &u.project_ids[..]);
        let mut of: Vec<&ProjectFile> = ids.iter().filter_map(|id| files.get(id)).collect();
        let real = |f: &ProjectFile| self.shared.dates.seconds(f.mtime);
        of.sort_by(|a, b| real(b).cmp(&real(a)).then(b.mtime.cmp(&a.mtime)).then_with(|| a.path.cmp(&b.path)));
        Some(of)
    }

    /// Which sounds are copies of each other (groups of ids, as [`mo_sounds::copies::find`] gives them), for Hide
    /// duplicates. Sounds not in this list are left out of their group; a group left with one sound is none.
    pub fn set_copies(&mut self, same: &SameAudio) {
        self.copies = Some(self.copies_in(same));
    }

    /// The copies of [`SoundList::set_copies`], worked out (off the window's thread) for [`SoundList::with_copies`].
    pub fn copies_in(&self, same: &SameAudio) -> Arc<Copies> {
        let places: HashMap<i64, usize> = self.shared.sounds.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
        let mut group_of = HashMap::new();
        let mut groups = Vec::new();
        let mut sizes = HashMap::new();
        let mut copies = 0;
        for group in &same.groups {
            let here: Vec<usize> = group.iter().filter_map(|id| places.get(id).copied()).collect();
            if here.len() < 2 {
                continue;
            }
            copies += here.len() - 1;
            let n = groups.len() as u32;
            for (&id, &i) in group.iter().filter_map(|id| Some((id, places.get(id)?))) {
                group_of.insert(i, n);
                if let Some(&size) = same.sizes.get(&id) {
                    sizes.insert(i, size);
                }
            }
            groups.push(here);
        }
        Arc::new(Copies { group_of, copies, groups, sizes })
    }

    /// This list with `copies` (from [`SoundList::copies_in`] on a list sharing its sounds). What it holds is shared,
    /// not copied (a few pointers).
    pub fn with_copies(&self, copies: Arc<Copies>) -> SoundList {
        let mut list = self.clone();
        list.copies = Some(copies);
        list
    }

    /// The copies `other` knows, when it shares this list's sounds and this list doesn't know them yet: a list made
    /// from the same sounds before they were found (with the use counts in) keeps what was found since.
    pub fn keep_copies_of(&mut self, other: &SoundList) {
        if self.copies.is_none() && self.shares_with(other) {
            self.copies = other.copies.clone();
        }
    }

    /// Every sound's copies, once known (empty before), the one to keep first by [`SoundList::keep_first`] (what the
    /// Sounds page shows with Hide duplicates on and no other filter), the groups whose copies take the most room
    /// first. For the Space page's Copies of the same sound: call it off the window's thread.
    pub fn copy_groups(&self) -> Vec<CopyGroup> {
        let Some(copies) = self.copies.as_deref() else { return Vec::new() };
        let mut groups: Vec<CopyGroup> = copies
            .groups
            .iter()
            .map(|g| {
                let mut places = g.clone();
                places.sort_by(|&a, &b| self.keep_first(a, b));
                CopyGroup {
                    sounds: places.into_iter().map(|i| (i, copies.sizes.get(&i).copied().unwrap_or(0))).collect(),
                }
            })
            .collect();
        groups.sort_by(|a, b| b.spare_bytes().cmp(&a.spare_bytes()).then_with(|| a.sounds.cmp(&b.sounds)));
        groups
    }

    /// Whether the sound at `i` has a copy (the same audio) that Hide duplicates may show instead.
    pub fn has_copies(&self, i: usize) -> bool {
        self.copies.as_ref().is_some_and(|c| c.group_of.contains_key(&i))
    }

    /// How many sounds Hide duplicates leaves out (every copy but one of each sound), once the copies are known.
    pub fn copies(&self) -> Option<usize> {
        self.copies.as_ref().map(|c| c.copies)
    }

    /// This list with your say about its sounds since they were read (`say`, by sound id), over what each says: for
    /// the filters, the chips' counts, the orders and [`SoundList::now`]. What it holds is shared, not copied.
    pub fn with_say(&self, say: Arc<HashMap<i64, YourSay>>) -> SoundList {
        let mut list = self.clone();
        list.say = (!say.is_empty()).then_some(say);
        list
    }

    /// How close each sound is to your favourites of its type (with your say over which are favourites and each
    /// one's type), from the fingerprints in `index`, for [`SoundList::with_like_favourites`]. Slow on a big library:
    /// call it off the window's thread.
    /// A copy (the same audio, once the copies are known) of a favourite is never one like them: it's no new sound.
    /// Nor is a sound silent all through, and one that's a favourite finds none like it: it's like nothing.
    pub fn like_favourites_in(&self, index: &SoundIndex) -> Arc<LikeFavourites> {
        use mo_sounds::near::Role;
        let was_favourite: Vec<bool> = (0..self.len()).map(|i| self.now_at(i).favourite).collect();
        // The copy groups that hold a favourite.
        let copied: std::collections::HashSet<u32> = self.copies.as_deref().map_or_else(Default::default, |c| {
            c.group_of.iter().filter(|(i, _)| was_favourite[**i]).map(|(_, g)| *g).collect()
        });
        let sounds: Vec<(i64, &str, Role)> = (0..self.len())
            .map(|i| {
                let now = self.now_at(i);
                let copy = || self.copies.as_ref().and_then(|c| c.group_of.get(&i)).is_some_and(|g| copied.contains(g));
                let role = if now.sound.is_silent() {
                    Role::Out
                } else if was_favourite[i] {
                    Role::Favourite
                } else if copy() {
                    Role::Out
                } else {
                    Role::Other
                };
                (now.sound.id, now.sound_type, role)
            })
            .collect();
        let reach = mo_sounds::near::reach(index, &sounds);
        Arc::new(LikeFavourites { reach, was_favourite, key: self.like_key() })
    }

    /// What how close the sounds are to your favourites is worked out from, as a number: which are favourites, each
    /// one's type (in any case), and which are copies of each other. The same number, the same answer (for the same
    /// fingerprints). Goes through every sound: call it off the window's thread.
    pub fn like_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.len().hash(&mut h);
        for i in 0..self.len() {
            let now = self.now_at(i);
            now.favourite.hash(&mut h);
            for b in now.sound_type.bytes() {
                b.to_ascii_lowercase().hash(&mut h);
            }
            0xffu8.hash(&mut h);
        }
        self.copies.as_ref().map(|c| Arc::as_ptr(c) as usize).hash(&mut h);
        h.finish()
    }

    /// The closeness this list holds, when it was worked out from what `key` says ([`SoundList::like_key`]): a list
    /// made again with nothing in it changed (a tag, the use counts) needs it worked out no more.
    pub fn like_favourites_for(&self, key: u64) -> Option<Arc<LikeFavourites>> {
        self.like.clone().filter(|l| l.key == key)
    }

    /// The closeness `other` knows, when it shares this list's sounds and this list knows none yet (a list with the
    /// use counts in, made from the same sounds).
    pub fn keep_like_favourites_of(&mut self, other: &SoundList) {
        if self.like.is_none() && self.shares_with(other) {
            self.like = other.like.clone();
        }
    }

    /// This list with `like` (from [`SoundList::like_favourites_in`] on a list sharing its sounds): the Favourites
    /// filter then shows the sounds most like them too, as far as [`Filters::like_favourites`] says.
    pub fn with_like_favourites(&self, like: Arc<LikeFavourites>) -> SoundList {
        let mut list = self.clone();
        list.like = (like.reach.len() == self.len()).then_some(like);
        list
    }

    /// Whether this list knows how close its sounds are to your favourites.
    pub fn knows_like_favourites(&self) -> bool {
        self.like.is_some()
    }

    /// Whether this list shows your say over any of its sounds ([`SoundList::with_say`]).
    pub fn has_say(&self) -> bool {
        self.say.is_some()
    }

    /// The sound at `i` as it is now: with your say since it was read over it.
    pub fn now(&self, i: usize) -> Option<Now<'_>> {
        let s = self.shared.sounds.get(i)?;
        Some(Now::of(s, self.say_of(s)))
    }

    /// The sound at `i` (in the list) as it is now.
    fn now_at(&self, i: usize) -> Now<'_> {
        let s = &self.shared.sounds[i];
        Now::of(s, self.say_of(s))
    }

    /// Your say about `s` since it was read, if any.
    fn say_of(&self, s: &Sound) -> Option<&YourSay> {
        self.say.as_ref()?.get(&s.id)
    }

    /// The copies, when `filters` hide them and they're known.
    fn hiding(&self, filters: &Filters) -> Option<&Copies> {
        self.copies.as_deref().filter(|c| filters.hide_copies && !c.group_of.is_empty())
    }

    /// Of two copies of the same sound, the one to keep showing first: the one your projects use most, then a
    /// favourite, more plays, more tags, the older file (the first, most likely), and then by path. Not stars: the
    /// heart is the one way to like a sound (Solomon's pick, 10 Nov 2026), so stars given in the old app don't pick it.
    fn keep_first(&self, a: usize, b: usize) -> std::cmp::Ordering {
        self.keep_by(a, b).0
    }

    /// [`SoundList::keep_first`], and what told the two apart (the first rule that does): one list of rules for the
    /// order and for what the Space page says of it ([`SoundList::why_kept`]).
    fn keep_by(&self, a: usize, b: usize) -> (std::cmp::Ordering, KeptFor) {
        let (na, nb) = (self.now_at(a), self.now_at(b));
        let (sa, sb) = (na.sound, nb.sound);
        let used = |s: &Sound| self.used.as_ref().and_then(|u| u.get(&s.id)).map_or(0, |u| u.projects);
        let real = |s: &Sound| self.shared.dates.local(s.added);
        let rules = [
            (KeptFor::Used, used(sb).cmp(&used(sa))),
            (KeptFor::Favourite, nb.favourite.cmp(&na.favourite)),
            (KeptFor::Plays, sb.plays.cmp(&sa.plays)),
            (KeptFor::Tags, nb.tags.len().cmp(&na.tags.len())),
            // A date that isn't real is no clue to which came first: the copies with a real date are older.
            (KeptFor::Older, (!real(sa), sa.added).cmp(&(!real(sb), sb.added))),
        ];
        if let Some((rule, by)) = rules.into_iter().find(|(_, by)| by.is_ne()) {
            return (by, rule);
        }
        (sa.path.cmp(&sb.path).then(a.cmp(&b)), KeptFor::Path)
    }

    /// Why the sound at `a` is kept before its copy at `b` ([`SoundList::keep_first`]).
    pub fn why_kept(&self, a: usize, b: usize) -> KeptFor {
        self.keep_by(a, b).1
    }

    /// The best to keep of each group's sounds in `places` (by [`SoundList::keep_first`]), by group.
    fn kept<'p>(&self, copies: &Copies, places: impl IntoIterator<Item = &'p usize>) -> HashMap<u32, usize> {
        let mut best: HashMap<u32, usize> = HashMap::new();
        for &i in places {
            if let Some(&g) = copies.group_of.get(&i) {
                best.entry(g)
                    .and_modify(|b| {
                        if self.keep_first(i, *b).is_lt() {
                            *b = i;
                        }
                    })
                    .or_insert(i);
            }
        }
        best
    }

    /// Each sound's Most liked score, from `score`.
    pub fn set_liked(&mut self, score: impl Fn(&Sound) -> f64 + Sync + Send) {
        let liked: Vec<f64> = self.shared.sounds.par_iter().map(&score).collect();
        self.liked_best = liked.iter().copied().fold(0.0, f64::max);
        self.liked = Some(Arc::new(liked));
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

    /// Which of the sounds' dates are real, as of when the list was read.
    pub fn dates(&self) -> &RealDates {
        &self.shared.dates
    }

    /// How your projects use every sound (by id; ones no project uses are left out), if that's known yet.
    pub fn usage_map(&self) -> Option<&HashMap<i64, Usage>> {
        self.used.as_deref()
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
        if let Some(copies) = self.hiding(filters) {
            let kept = self.kept(copies, &shown);
            shown.retain(|i| copies.group_of.get(i).is_none_or(|g| kept[g] == *i));
        }
        self.ordered(shown, order, seed)
    }

    /// [`SoundList::view`], and how many sounds each choice of the chips' filters would show with the other filters
    /// as they are, counted in the same pass: a sound counts beside every chip when all five let it through, and
    /// beside the one that doesn't when only one doesn't. With Hide duplicates on, the copies of a sound count once
    /// beside each choice, as picking it would show one of them.
    pub fn view_counted(&self, filters: &Filters, order: Order, seed: u64) -> (Vec<usize>, SoundCounts) {
        let test = Test::new(filters);
        let copies = self.hiding(filters);
        let (mut shown, mut tally, held) = (0..self.shared.sounds.len())
            .into_par_iter()
            .fold(
                || (Vec::new(), Tally::default(), Vec::new()),
                |(mut shown, mut tally, mut held), i| {
                    let now = self.now_at(i);
                    let ok = facets(&now, &test);
                    let fails = ok.iter().filter(|&&o| !o).count();
                    // The chips first, as they're quick: a sound two of them leave out is neither shown nor counted,
                    // so the search (the slow part) isn't tried on it.
                    if fails <= 1 && self.passes_others(i, &now, &test) {
                        if copies.is_some_and(|c| c.group_of.contains_key(&i)) {
                            // Counted with its copies, below.
                            held.push((i, ok, fails));
                        } else {
                            tally.count(&now, ok, fails);
                            if fails == 0 {
                                shown.push(i);
                            }
                        }
                    }
                    (shown, tally, held)
                },
            )
            .reduce(
                || (Vec::new(), Tally::default(), Vec::new()),
                |(mut a, mut tally, mut held), (b, other, more)| {
                    a.extend(b);
                    tally.add(other);
                    held.extend(more);
                    (a, tally, held)
                },
            );
        if let Some(copies) = copies {
            let kept = self.kept(copies, held.iter().filter(|h| h.2 == 0).map(|h| &h.0));
            shown.extend(kept.values());
            let mut by_group: HashMap<u32, Tally> = HashMap::new();
            for (i, ok, fails) in held {
                by_group.entry(copies.group_of[&i]).or_default().count(&self.now_at(i), ok, fails);
            }
            for group in by_group.into_values() {
                tally.add(group.once());
            }
        }
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
        if order == Order::Type {
            // Each sound's type (with your say over it) found once, not on every comparison.
            let mut keyed: Vec<(&str, usize)> = shown.par_iter().map(|&i| (self.now_at(i).sound_type, i)).collect();
            keyed.par_sort_unstable_by(|&(ta, a), &(tb, b)| by_letters(ta, tb).then_with(|| self.by_name(a, b)));
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
        for i in 0..self.len() {
            *exact.entry(self.now_at(i).sound_type).or_default() += 1;
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
        let mut tags: Vec<String> = (0..self.len()).flat_map(|i| self.now_at(i).tags.iter().cloned()).collect();
        tags.sort_by_key(|t| t.to_lowercase());
        tags.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        tags
    }

    fn allows(&self, i: usize, t: &Test) -> bool {
        let now = self.now_at(i);
        facets(&now, t) == [true; FACETS] && self.passes_others(i, &now, t)
    }

    /// Everything but the chips' filters lets the sound at `i` (`now`) through: Hide silent sounds, favourites, Used and
    /// Unused, the folder, the years and the search (last, as it's the slow one).
    fn passes_others(&self, i: usize, now: &Now, t: &Test) -> bool {
        let s = now.sound;
        if !t.show_silent && s.is_silent() {
            return false;
        }
        if t.favourites
            && !now.favourite
            && !self.like.as_ref().is_some_and(|like| {
                like.reach.get(i).is_some_and(|&r| r < t.per_favourite)
                    && !like.was_favourite.get(i).copied().unwrap_or(true)
            })
        {
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
                _ => self.shared.dates.local(s.added) && within(s.added.year()),
            };
            if !ok {
                return false;
            }
        }
        if t.words.is_empty() {
            return true;
        }
        // Your say changed its type or tags since its words were worked out: worked out again, for it alone.
        let changed = now.words_changed.then(|| hay_of(now));
        let hay = changed.as_deref().unwrap_or(&self.shared.hay[i]);
        t.words.iter().all(|w| hay.contains(w.as_str()))
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
                // A date that isn't real is in no year.
                for s in self.shared.sounds.iter().filter(|s| self.shared.dates.local(s.added)) {
                    *counts.entry(s.added.year()).or_default() += 1;
                }
            }
        }
        crate::year_strip::strip(counts)
    }

    /// Turns a list in `order` round (a second click on its heading; a shuffle has no other way round). By key, the
    /// sounds with no key still come last, and by date those whose date isn't real.
    pub fn turn_round(&self, shown: &mut [usize], order: Order) {
        if order == Order::Shuffled {
            return;
        }
        shown.reverse();
        let sound = |i: usize| &self.shared.sounds[i];
        match order {
            Order::Key => shown.sort_by_key(|&i| key_order(sound(i)) == i32::MAX),
            Order::Newest => shown.sort_by_key(|&i| !self.shared.dates.local(sound(i).added)),
            Order::Shuffled | Order::MostLiked | Order::Name | Order::Type => {}
        }
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
            // [`SoundList::ordered`] finds each type once.
            Order::Type => by_letters(self.now_at(a).sound_type, self.now_at(b).sound_type),
            Order::Key => key_order(sa).cmp(&key_order(sb)),
            Order::Newest => {
                let real = |s: &Sound| self.shared.dates.local(s.added);
                real(sb).cmp(&real(sa)).then(sb.added.cmp(&sa.added))
            }
        };
        by.then_with(|| self.by_name(a, b))
    }

    /// The sounds at `a` and `b` by name, then by place: the order of those any other order finds the same.
    fn by_name(&self, a: usize, b: usize) -> std::cmp::Ordering {
        self.shared.names[a].cmp(&self.shared.names[b]).then(a.cmp(&b))
    }
}

/// `a` and `b` letter by letter, lower-cased as it goes: no new strings for each of a big list's comparisons.
pub fn by_letters(a: &str, b: &str) -> std::cmp::Ordering {
    a.chars().flat_map(char::to_lowercase).cmp(b.chars().flat_map(char::to_lowercase))
}

/// The filters made ready to test many sounds: lower-cased words, the set of keys allowed.
struct Test<'a> {
    sound_types: &'a [String],
    length: Length,
    unused: bool,
    used: bool,
    favourites: bool,
    /// How many sounds like each favourite show beside them (0 for none).
    per_favourite: u32,
    tags: &'a [String],
    keys: Option<Vec<(i32, bool)>>,
    /// The lowest and highest tempo let through.
    tempo: (Option<f64>, Option<f64>),
    years: Option<(i32, i32)>,
    folder: Option<&'a str>,
    words: Vec<String>,
    show_silent: bool,
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
            per_favourite: if f.favourites { per_favourite(f.like_favourites) } else { 0 },
            tags: &f.tags,
            keys,
            tempo: f.tempo,
            years: f.years,
            folder: f.folder.as_deref(),
            words: f.search.split_whitespace().map(str::to_lowercase).collect(),
            show_silent: f.show_silent,
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

/// Which of the chips' filters let a sound (as it is `now`) through: its type, its length (hit or loop), its key, its
/// tempo and its tags.
fn facets(now: &Now, t: &Test) -> [bool; FACETS] {
    let s = now.sound;
    let a = s.analysis.as_ref();
    let is_loop = a.is_some_and(|a| a.is_loop);
    let bpm = a.and_then(|a| a.bpm);
    [
        types::is_one_of(now.sound_type, t.sound_types),
        match t.length {
            Length::All => true,
            Length::Hits => !is_loop,
            Length::Loops => is_loop,
        },
        t.keys.as_ref().is_none_or(|keys| key_of(s).is_some_and(|k| keys.contains(&k))),
        tempo_in(bpm, t.tempo),
        t.tags.is_empty() || t.tags.iter().any(|w| now.tags.iter().any(|own| own.eq_ignore_ascii_case(w))),
    ]
}

/// Whether `bpm` is in `range` (lowest, highest), both ends in. As the C# app: with a lowest tempo, a sound with none
/// is left out; with a highest, likewise.
fn tempo_in(bpm: Option<f64>, (lo, hi): (Option<f64>, Option<f64>)) -> bool {
    !(lo.is_some_and(|lo| bpm.unwrap_or(-1.0) < lo) || hi.is_some_and(|hi| bpm.is_none_or(|b| b > hi)))
}

/// Which of the Tempo slider's bars a tempo is counted in.
fn tempo_bar(bpm: f64) -> usize {
    let bar = ((bpm - f64::from(TEMPO_LOW)) / f64::from(TEMPO_BAR)).floor();
    // A tempo read wrongly (not a number, or huge) still lands in a bar.
    if bar.is_nan() { 0 } else { bar.clamp(0.0, (TEMPO_BARS - 1) as f64) as usize }
}

/// The Tempo slider as the page shows it ([`chips`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TempoPick {
    /// Where the two handles are, in BPM: [`TEMPO_LOW`] and [`TEMPO_HIGH`] when there's no limit.
    pub low: i32,
    pub high: i32,
    /// What's picked, in words: "Any tempo", "90 to 130 BPM".
    pub words: String,
    /// How tall each bar is, 0 to 1 of the tallest ([`SoundCounts::tempos`]); all flat until a list has been counted.
    pub bars: Vec<f32>,
}

fn tempo_pick(f: &Filters, counts: Option<&SoundCounts>) -> TempoPick {
    let whole = |bpm: f64| bpm.round() as i32;
    let words = match f.tempo {
        (None, None) => "Any tempo".to_owned(),
        (Some(low), None) => format!("{} BPM and up", whole(low)),
        (None, Some(high)) => format!("Up to {} BPM", whole(high)),
        (Some(low), Some(high)) if low == high => format!("{} BPM", whole(low)),
        (Some(low), Some(high)) => format!("{} to {} BPM", whole(low), whole(high)),
    };
    let tallest = counts.and_then(|c| c.tempos.iter().copied().max()).unwrap_or(0).max(1);
    let bars = (0..TEMPO_BARS).map(|i| counts.map_or(0.0, |c| c.tempos[i] as f32 / tallest as f32)).collect();
    TempoPick { low: f.tempo.0.map_or(TEMPO_LOW, whole), high: f.tempo.1.map_or(TEMPO_HIGH, whole), words, bars }
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
    tempos: [usize; TEMPO_BARS],
    tags: HashMap<&'a str, usize>,
}

impl<'a> Tally<'a> {
    /// Counts `s`, which the chips' filters `ok` let through, all but `fails` of them: beside every filter's choices
    /// when they all do, and beside only the one that doesn't when one doesn't (picking another of its choices would
    /// show it). A sound two of them leave out isn't counted.
    fn count(&mut self, now: &Now<'a>, ok: [bool; FACETS], fails: usize) {
        let on = |facet: usize| fails == 0 || !ok[facet];
        let s = now.sound;
        let a = s.analysis.as_ref();
        if on(TYPE_AT) {
            *self.types.entry(now.sound_type).or_default() += 1;
        }
        if on(LENGTH_AT) {
            self.lengths[usize::from(a.is_some_and(|a| a.is_loop))] += 1;
        }
        if on(KEY_AT)
            && let Some((tonic, minor)) = key_of(s)
        {
            self.keys[key_place(tonic, minor)] += 1;
        }
        if on(TEMPO_AT)
            && let Some(bpm) = a.and_then(|a| a.bpm)
        {
            self.tempos[tempo_bar(bpm)] += 1;
        }
        if on(TAG_AT) {
            let tags = now.tags;
            for (k, t) in tags.iter().enumerate() {
                // A tag the sound has twice (in other capitals) counts it once: picking it shows the sound once.
                if !tags[..k].iter().any(|other| other.eq_ignore_ascii_case(t)) {
                    *self.tags.entry(t.as_str()).or_default() += 1;
                }
            }
        }
    }

    /// Every count of one at most: the counts of a sound's copies, as one sound. Types and tags that differ only in
    /// capitals are one, as the filters match them.
    fn once(mut self) -> Tally<'a> {
        let one_each = |words: HashMap<&'a str, usize>| {
            let mut seen = std::collections::HashSet::new();
            words.into_keys().filter(|w| seen.insert(w.to_ascii_lowercase())).map(|w| (w, 1)).collect()
        };
        self.types = one_each(std::mem::take(&mut self.types));
        self.tags = one_each(std::mem::take(&mut self.tags));
        for n in self.lengths.iter_mut().chain(self.keys.iter_mut()).chain(self.tempos.iter_mut()) {
            *n = (*n).min(1);
        }
        self
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
    /// By tempo: the Tempo slider's bars ([`TEMPO_BARS`]), slowest first. A sound with no tempo is in none.
    pub tempos: [usize; TEMPO_BARS],
    /// By tag, in lower case (ASCII letters only).
    pub tags: HashMap<String, usize>,
}

/// The page's filters as their chips and menus show them, each choice with how many sounds it would show: the types
/// (the most used as chips of their own, the rest in a More menu), hits or loops, the key (a menu: there are 24), the
/// tempo (a slider, with bars for how many sounds there are at each tempo), and the tags (chips, or a menu when there
/// are many).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoundChips {
    pub types: Filter,
    pub length: Filter,
    pub key: Filter,
    pub tempo: TempoPick,
    pub tag: Filter,
}

/// [`SoundChips`] for the filters `f`, with the `counts` of the list they let through (`None` until a list has been
/// worked out: the choices show without numbers rather than with noughts). `types` is [`SoundList::type_counts`] (the
/// most used first: an order the other filters don't change, so a chip doesn't move under the mouse when they do)
/// and `tags` is [`SoundList::tags`]. With Keys that mix well on, a key's count is how many sounds it and the keys
/// that mix well with it hold.
pub fn chips(f: &Filters, counts: Option<&SoundCounts>, types: &[(String, usize)], tags: &[String]) -> SoundChips {
    chips_showing(f, counts, types, tags, TYPE_CHIPS)
}

/// [`chips`] with the first `type_lines` types on show and the rest in the More menu: for a filter column that shows
/// as many types as it has room for (Solomon, 11 Nov 2026: "the more types should fill if there is more space").
pub fn chips_showing(
    f: &Filters,
    counts: Option<&SoundCounts>,
    types: &[(String, usize)],
    tags: &[String],
    type_lines: usize,
) -> SoundChips {
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
    let tags: Vec<(String, String)> = tags.iter().map(|t| (t.clone(), shown(&|c| by_word(&c.tags, t)))).collect();
    SoundChips {
        types: chips::show_split(&types, &f.sound_types, type_lines),
        length: chips::show(&lengths, &length),
        key: chips::show(&keys, &keys_picked),
        tempo: tempo_pick(f, counts),
        tag: chips::show(&tags, &f.tags),
    }
}

fn hay_of(now: &Now) -> String {
    let s = now.sound;
    let mut hay = format!("{}\n{}", s.path, now.sound_type);
    for t in now.tags {
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

/// The setting Hide duplicates is kept in, in the app's own settings ("on" or "off").
pub const HIDE_COPIES_KEY: &str = "sounds_hide_duplicates";

/// The setting Hide silent sounds is kept in, in the app's own settings ("on" or "off"): on until you turn it off.
pub const HIDE_SILENT_KEY: &str = "sounds_hide_silent";

/// The setting Trim silence is kept in, in the app's own settings ("on" or "off"): when it's on, a sound plays
/// without the silence at its start and end (`mo_audio::trim`); the file is never changed.
pub const TRIM_SILENCE_KEY: &str = "sounds_trim_silence";
/// The setting the level Trim silence counts as silence is kept in: dB below full scale, one of
/// `mo_audio::trim::LEVELS_DB` ("-60").
pub const TRIM_LEVEL_KEY: &str = "sounds_trim_level_db";

/// The page's view as you left it ("Tiles", "List", "Galaxy"...), whether Autoplay is on ("1" or "0") and how many
/// sounds Rediscover digs out: kept in the app's own settings as you change them (never in `library.db`), and until
/// you have, the C# app's own under the same names (`crate::app_settings::FROM_CSHARP`, review G-3).
pub const VIEW_KEY: &str = "sounds_view";
pub const AUTOPLAY_KEY: &str = "sound_lab_autoplay";
pub const REDISCOVER_COUNT_KEY: &str = "rediscover_count";

/// How far finding the copies for Hide duplicates has got.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CopiesFound {
    /// Not looked for: Hide duplicates hasn't been on with these sounds.
    #[default]
    NotLooked,
    /// Files being read: `done` of `of` (both 0 while what's known already is being looked up).
    Looking { done: usize, of: usize },
    /// Found: how many sounds are copies of another (what the switch hides).
    Found(usize),
    /// The files couldn't be looked at (the log says why).
    Failed,
}

/// The Hide duplicates chip's words: what it does, then how far finding the copies has got.
pub fn hide_copies_text(found: CopiesFound) -> String {
    const NAME: &str = "Hide duplicates";
    match found {
        CopiesFound::NotLooked => NAME.to_owned(),
        CopiesFound::Looking { of: 0, .. } => format!("{NAME} · checking"),
        CopiesFound::Looking { done, of } => format!("{NAME} · checking {}%", done * 100 / of),
        CopiesFound::Found(0) => format!("{NAME} · none"),
        CopiesFound::Found(n) => format!("{NAME} · {}", short_count(n)),
        CopiesFound::Failed => format!("{NAME} · couldn't check"),
    }
}

/// The Hide silent sounds line's words: what it does, and how many of the sounds are silent all through or empty
/// ([`SoundList::silent`]).
pub fn hide_silent_text(silent: usize) -> String {
    const NAME: &str = "Hide silent sounds";
    if silent == 0 { format!("{NAME} · none") } else { format!("{NAME} · {}", short_count(silent)) }
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
    facts_line_now(&Now::of(s, None))
}

/// [`facts_line`] for a sound as it is `now` (with your own type for it since it was read).
pub fn facts_line_now(now: &Now) -> String {
    let s = now.sound;
    let mut parts = vec![now.sound_type.to_owned()];
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
/// projects, which are listed when none use this one. Each line's day goes by `dates`, the list's
/// ([`SoundList::dates`]), as the projects' order did.
pub fn projects_card(
    dates: &RealDates,
    own: Option<&[&ProjectFile]>,
    like: Option<(&str, &[&ProjectFile])>,
) -> ProjectsCard {
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
        return ProjectsCard { title, note, lines: project_lines(dates, own) };
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
            ProjectsCard { title, note, lines: project_lines(dates, theirs) }
        }
        _ => ProjectsCard { title, note: "No project of yours uses it yet.".to_owned(), lines: Vec::new() },
    }
}

/// The card's lines for `projects`, newest first: the newest [`PROJECTS_SHOWN`].
fn project_lines(dates: &RealDates, projects: &[&ProjectFile]) -> Vec<ProjectLine> {
    projects
        .iter()
        .take(PROJECTS_SHOWN)
        .map(|f| ProjectLine {
            id: f.id,
            name: mo_formats::paths::stem(mo_formats::paths::file_name(&f.path)).to_owned(),
            daw: mo_library::scan::daw_of(&f.path).unwrap_or_default().to_owned(),
            // A save the scan didn't date (0) shows none, rather than 1 Jan 1970.
            saved: crate::midi::date_text(dates, f.mtime),
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
/// mended (from its backup copy, or made again from `sounds.db`) either: why, without the library's whole path.
pub fn not_remade_line(e: &SoundsError) -> String {
    let why = match e {
        SoundsError::Copy { why, .. } | SoundsError::Damaged { why, .. } => why.clone(),
        e => e.to_string(),
    };
    format!("It's damaged, and it couldn't be mended ({why}), so it's read as it is.")
}

/// What the card says when the app's own sound library was found damaged and put back from its backup copy made at
/// `made` (`mo_sounds::compact::backup`), with what could still be read of it since (`salvaged`): from when, and that
/// the damaged one is kept a while.
pub fn put_back_line(made: chrono::DateTime<chrono::Local>, salvaged: Option<&mo_sounds::compact::Salvaged>) -> String {
    let since = match salvaged.map(|s| (s.kept, s.unread)) {
        None | Some((0, 0)) => "",
        Some((0, _)) => " (nothing since could still be read of it)",
        Some((_, 0)) => ", with everything since",
        Some(_) => ", with everything since that could still be read of it",
    };
    format!(
        "It was damaged, so it was put back from its backup copy of {}{since}. The damaged one is kept beside it for a \
         week, as {}.",
        made.format("%-d %b %H:%M"),
        mo_sounds::compact::backup::damaged_of(std::path::Path::new(mo_sounds::compact::FILE_NAME)).display()
    )
}

/// What the card says when the app's own sound library was found damaged with no backup copy to put it back from
/// (`unused`: why not, as [`Synced::backup_unused`] says it), so it was made again from `sounds.db` with what could
/// still be read of it ([`mo_sounds::compact::Salvaged`]): how much of it was kept, and that the damaged one is kept a
/// while.
pub fn salvaged_line(salvaged: &mo_sounds::compact::Salvaged, unused: Option<&str>) -> String {
    let kept = match (salvaged.kept, salvaged.unread) {
        (0, 0) => "nothing in it differed from sounds.db",
        (0, _) => "none of it could still be read, so it's as sounds.db has it",
        (_, 0) => "with all of it: your favourites, tags, plays and the sounds the scan added",
        _ => "with all of it that could still be read (the rest is as sounds.db has it)",
    };
    let unused = unused.unwrap_or("there's no backup copy");
    format!(
        "It was damaged, and {unused}, so it was made again from sounds.db, {kept}. The damaged one is kept beside it \
         for a week, as {}.",
        mo_sounds::compact::backup::damaged_of(std::path::Path::new(mo_sounds::compact::FILE_NAME)).display()
    )
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
    use chrono::{NaiveDate, TimeZone};
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
    fn favourites_show_the_sounds_most_like_them_as_far_as_asked() {
        use mo_sounds::index::VectorIndex;
        // Kicks 1 to 4, 1 the favourite; 2 most like it, then 3, then 4. A snare like the kick, never shown for it.
        let mut fav = sound(1, r"I:\Kicks\Fav.wav", "Kick", analysed(false, None, None));
        fav.favourite = true;
        let list = SoundList::new(vec![
            fav,
            sound(2, r"I:\Kicks\Close.wav", "Kick", analysed(false, None, None)),
            sound(3, r"I:\Kicks\Further.wav", "Kick", analysed(false, None, None)),
            sound(4, r"I:\Kicks\Far.wav", "Kick", analysed(false, None, None)),
            sound(5, r"I:\Snares\Like the kick.wav", "Snare", analysed(false, None, None)),
        ]);
        let vectors = [(1, [1.0, 0.0]), (2, [0.95, 0.05]), (3, [0.6, 0.4]), (4, [0.0, 1.0]), (5, [1.0, 0.0])];
        let vectors = vectors.iter().map(|(id, v)| (*id, v.to_vec())).collect();
        let index = SoundIndex { features: VectorIndex::clap(&vectors), clap: None, deep: false };
        let shown = |list: &SoundList, step: u8| {
            let f = Filters { favourites: true, like_favourites: step, ..Filters::default() };
            let mut by_view = ids(list, &list.view(&f, Order::Name, 0));
            let mut by_counted = ids(list, &list.view_counted(&f, Order::Name, 0).0);
            by_view.sort_unstable();
            by_counted.sort_unstable();
            assert_eq!(by_view, by_counted);
            by_view
        };
        // Not known yet: only the favourite, however far asked.
        assert_eq!(shown(&list, 3), [1]);
        let like = list.with_like_favourites(list.like_favourites_in(&index));
        assert!(like.knows_like_favourites() && like.shares_with(&list));
        assert_eq!(shown(&like, 0), [1]);
        assert_eq!(shown(&like, 1), [1, 2]);
        assert_eq!(shown(&like, 2), [1, 2, 3, 4]);
        // With Favourites off, every sound, as before.
        let off = Filters { like_favourites: 2, ..Filters::default() };
        assert_eq!(like.view(&off, Order::Name, 0).len(), 5);
        assert!(!off.any() && !off.wants_like_favourites());
        // Clear keeps how far, as it keeps Hide duplicates.
        let on = Filters { favourites: true, like_favourites: 2, ..Filters::default() };
        assert!(on.any() && on.wants_like_favourites());
        assert_eq!(on.cleared(), off);
        // Kept by a list sharing the sounds; one for other sounds isn't taken.
        assert!(like.sharing().knows_like_favourites());
        let mut with_use = list.sharing();
        with_use.keep_like_favourites_of(&like);
        assert!(with_use.knows_like_favourites());
        assert!(with_use.like_favourites_for(list.like_key()).is_some());
        // The favourite made not one: it shows no more while the closeness is worked out again, and the key differs.
        let say = HashMap::from([(1, YourSay { favourite: Some(false), ..YourSay::default() })]);
        let unhearted = like.with_say(Arc::new(say));
        assert!(unhearted.like_favourites_for(unhearted.like_key()).is_none());
        assert!(!shown(&unhearted, 2).contains(&1));
        // A copy of the favourite (the same audio) is no sound like it: 2 is left out, 3 and 4 come in its place.
        let same = SameAudio { groups: vec![vec![1, 2]], ..Default::default() };
        let mut copied = list.clone();
        copied.set_copies(&same);
        let copied = copied.with_like_favourites(copied.like_favourites_in(&index));
        assert_eq!(shown(&copied, 1), [1, 3]);
        assert!(
            !SoundList::new(Vec::new()).with_like_favourites(list.like_favourites_in(&index)).knows_like_favourites()
        );
        assert_eq!(
            (per_favourite(0), per_favourite(1), per_favourite(LIKE_STEPS), per_favourite(99)),
            (0, 1, 255, 255)
        );
        assert_eq!(like_favourites_words(0, LikeKnown::Finding), "Only them");
        assert_eq!(like_favourites_words(2, LikeKnown::Finding), "Finding…");
        assert_eq!(like_favourites_words(2, LikeKnown::Missing), "No fingerprints");
        assert_eq!(like_favourites_words(2, LikeKnown::Yes), "+3 each");
        assert_eq!((like_favourites_share(0), like_favourites_share(4), like_favourites_share(99)), (0.0, 0.5, 1.0));
        assert_eq!(
            [0.0, 0.06, 0.07, 0.5, 1.0, 2.0, -1.0, f32::NAN].map(like_favourites_step),
            [0, 0, 1, 4, LIKE_STEPS, LIKE_STEPS, 0, 0]
        );
    }

    #[test]
    fn your_say_since_the_sounds_were_read_shows_through_the_filters_counts_and_orders() {
        let list = library();
        let place = |id: i64| list.find(id).unwrap();
        let (kick, bass, vocal) = (place(1), place(2), place(3));
        let view = |list: &SoundList, f: Filters, order: Order| ids(list, &list.view(&f, order, 0));
        let counted = |list: &SoundList, f: Filters| ids(list, &list.view_counted(&f, Order::Name, 0).0);
        let favourites = Filters { favourites: true, ..Filters::default() };
        assert_eq!(view(&list, favourites.clone(), Order::Name), [3]);
        // The kick a favourite now, with a tag; the vocal not, and its tag gone; the bass yours as a Pad.
        let say = HashMap::from([
            (1, YourSay { favourite: Some(true), tags: Some(vec!["dusty".into()]), ..YourSay::default() }),
            (3, YourSay { favourite: Some(false), tags: Some(Vec::new()), ..YourSay::default() }),
            (2, YourSay { own_type: Some(Some("Pad".into())), ..YourSay::default() }),
        ]);
        let now = list.with_say(Arc::new(say));
        assert!(list.shares_with(&now));
        assert_eq!(view(&now, favourites.clone(), Order::Name), [1]);
        assert_eq!(counted(&now, favourites), [1]);
        let k = now.now(kick).unwrap();
        assert_eq!((k.favourite, k.tags, k.sound_type), (true, &["dusty".to_owned()][..], "Kick"));
        let b = now.now(bass).unwrap();
        assert_eq!((b.sound_type, b.type_source, b.own_type), ("Pad", "you", Some("Pad")));
        // The sound as it was read stays as it was.
        assert_eq!(now.sound(bass).unwrap().sound_type, "Bass");
        assert!(now.now(vocal).unwrap().tags.is_empty());
        // The type and tag chips, their counts, the search and the type order all go by your say.
        let pads = Filters { sound_types: vec!["pad".into()], ..Filters::default() };
        assert_eq!(view(&now, pads.clone(), Order::Name), [4, 2]);
        assert_eq!(now.type_counts(), [("Pad".to_owned(), 2), ("Kick".to_owned(), 1), ("Vocal".to_owned(), 1)]);
        assert_eq!(now.tags(), ["dusty"]);
        let dusty = Filters { tags: vec!["Dusty".into()], ..Filters::default() };
        assert_eq!(view(&now, dusty.clone(), Order::Name), [1]);
        assert_eq!(view(&now, Filters { search: "DUSTY".into(), ..Filters::default() }, Order::Name), [1]);
        assert_eq!(view(&now, Filters { search: "dark".into(), ..Filters::default() }, Order::Name), Vec::<i64>::new());
        assert_eq!(view(&now, Filters { search: "pad".into(), ..Filters::default() }, Order::Name), [4, 2]);
        let (_, counts) = now.view_counted(&Filters::default(), Order::Name, 0);
        let chips = chips(&Filters::default(), Some(&counts), &now.type_counts(), &now.tags());
        let tags = chips.tag.chips.iter().chain(&chips.tag.more);
        assert!(tags.clone().any(|c| c.text == "dusty" && c.count == "1"), "{:?}", tags.collect::<Vec<_>>());
        assert_eq!(view(&now, Filters::default(), Order::Type), [1, 4, 2, 3]);
        // Your own type taken away: the name's type again, or Unknown when the name gives none.
        let mut named = sound(5, r"I:\S\Clap 1.wav", "Clap", None);
        (named.user_type, named.name_type, named.type_source) =
            (Some("Snare".into()), Some("Clap".into()), "you".into());
        let mut unnamed = sound(6, r"I:\S\x.wav", "Snare", None);
        (unnamed.user_type, unnamed.type_source) = (Some("Snare".into()), "you".into());
        let two = SoundList::new(vec![named, unnamed]);
        let taken = YourSay { own_type: Some(None), ..YourSay::default() };
        let two = two.with_say(Arc::new(HashMap::from([(5, taken.clone()), (6, taken)])));
        let shown = |i: usize| two.now(i).map(|n| (n.sound_type, n.type_source, n.own_type));
        assert_eq!((shown(0), shown(1)), (Some(("Clap", "name", None)), Some(("Unknown", "", None))));
        // Nothing to say: as it was read.
        let none = list.with_say(Arc::new(HashMap::new()));
        assert_eq!(view(&none, Filters { favourites: true, ..Filters::default() }, Order::Name), [3]);
        // Of two copies, the favourite now is the one kept.
        assert_eq!(now.why_kept(kick, vocal), KeptFor::Favourite);
    }

    #[test]
    fn the_files_a_program_left_in_a_temp_folder_stay_out_of_the_list() {
        // FL Studio's blank renders, which an older scan took for closed hats (Solomon, 12 Nov 2026).
        let list = SoundList::new(vec![
            sound(1, r"C:\Users\silki\AppData\Local\Temp\Image-Line\{1FD56D3D}_U.wav", "Closed hat", None),
            sound(2, r"I:\Samples\Temp\Closed Hat 01.wav", "Closed hat", None),
        ]);
        assert_eq!(list.len(), 1);
        assert_eq!(list.sound(0).map(|s| s.id), Some(2));
    }

    #[test]
    fn silent_sounds_are_left_out_until_you_ask_for_them() {
        use mo_sounds::index::VectorIndex;
        // Solomon, 12 Nov 2026: "hide silent audio files". 2 is silent all through and 3 empty; 4 only whispers
        // (-70 dB); 5's two sides cancel out in the mix its peak is measured on, but it's heard in stereo; 6 isn't
        // analysed yet.
        let level = |peak_db: f64, width: f64| Some(SoundAnalysis { peak_db, width, ..SoundAnalysis::default() });
        let empty = Some(SoundAnalysis { seconds: 0.0, peak_db: -120.0, ..SoundAnalysis::default() });
        let list = SoundList::new(vec![
            sound(1, r"I:\S\1 Kick.wav", "Kick", level(-6.0, 0.0)),
            sound(2, r"I:\S\2 Kick.wav", "Kick", level(-120.0, 0.0)),
            sound(3, r"I:\S\3 Kick.wav", "Kick", empty),
            sound(4, r"I:\S\4 Kick.wav", "Kick", level(-70.0, 0.0)),
            sound(5, r"I:\S\5 Kick.wav", "Kick", level(-120.0, 1.414)),
            sound(6, r"I:\S\6 Kick.wav", "Kick", None),
        ]);
        assert_eq!(list.silent(), 2);
        let hidden = Filters::default();
        assert_eq!(ids(&list, &list.view(&hidden, Order::Name, 0)), [1, 4, 5, 6]);
        let (shown, counts) = list.view_counted(&hidden, Order::Name, 0);
        assert_eq!(ids(&list, &shown), [1, 4, 5, 6]);
        assert_eq!(counts.types.get("kick"), Some(&4));
        // Shown when you ask; Clear filters leaves the line as you set it, and it's no filter of its own.
        let mut asked = hidden.clone();
        asked.toggle(HIDE_SILENT);
        assert!(asked.show_silent && !asked.any());
        assert_eq!(ids(&list, &list.view(&asked, Order::Name, 0)), [1, 2, 3, 4, 5, 6]);
        assert_eq!(Filters { favourites: true, ..asked.clone() }.cleared(), asked);
        assert_eq!(hide_silent_text(0), "Hide silent sounds · none");
        assert_eq!(hide_silent_text(2), "Hide silent sounds · 2");
        // Nor is one among the sounds like your favourites, even shown and with a fingerprint like a favourite's.
        let mut fav = sound(7, r"I:\S\7 Kick.wav", "Kick", level(-6.0, 0.0));
        fav.favourite = true;
        let three = SoundList::new(vec![
            fav,
            sound(8, r"I:\S\8 Kick.wav", "Kick", level(-120.0, 0.0)),
            sound(9, r"I:\S\9 Kick.wav", "Kick", level(-6.0, 0.0)),
        ]);
        let vectors = [(7, [1.0, 0.0]), (8, [1.0, 0.0]), (9, [0.6, 0.4])];
        let vectors = vectors.iter().map(|(id, v)| (*id, v.to_vec())).collect();
        let index = SoundIndex { features: VectorIndex::clap(&vectors), clap: None, deep: false };
        let like = three.with_like_favourites(three.like_favourites_in(&index));
        let far = Filters { favourites: true, like_favourites: LIKE_STEPS, show_silent: true, ..Filters::default() };
        assert_eq!(ids(&like, &like.view(&far, Order::Name, 0)), [7, 9]);
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
    fn hide_duplicates_shows_one_of_each_sounds_copies_the_best_to_keep_of_those_the_filters_let_through() {
        let mut sounds: Vec<Sound> = (1..=5).map(|i| sound(i, &format!(r"I:\S\{i} Kick.wav"), "Kick", None)).collect();
        // 1, 2 and 3 are the same audio; 3 is the favourite; 2 is a snare by its name (its type differs).
        sounds[2].favourite = true;
        sounds[1].sound_type = "Snare".into();
        let mut list = SoundList::new(sounds);
        assert_eq!(list.copies(), None);
        list.set_copies(&SameAudio { groups: vec![vec![1, 2, 3], vec![5, 99]], ..SameAudio::default() });
        // The group with a sound not in the list is none.
        assert_eq!(list.copies(), Some(2));
        let ids = |v: &[usize]| v.iter().map(|&i| list.sound(i).unwrap().id).collect::<Vec<_>>();
        let off = Filters::default();
        let on = Filters { hide_copies: true, ..Filters::default() };
        assert_eq!(ids(&list.view(&off, Order::Name, 0)), [1, 2, 3, 4, 5]);
        assert_eq!(ids(&list.view(&on, Order::Name, 0)), [3, 4, 5]);
        let (shown, counts) = list.view_counted(&on, Order::Name, 0);
        assert_eq!(ids(&shown), [3, 4, 5]);
        // Kick: 1 and 3 count once, with 4 and 5; Snare: 2 would show if picked.
        assert_eq!(counts.types.get("kick"), Some(&3));
        assert_eq!(counts.types.get("snare"), Some(&1));
        assert_eq!(counts.lengths, [3, 0]);
        // Only snares: 2 shows, though 3 is the one kept when all show.
        let snares = Filters { sound_types: vec!["Snare".into()], ..on.clone() };
        assert_eq!(ids(&list.view(&snares, Order::Name, 0)), [2]);
        assert_eq!(ids(&list.view_counted(&snares, Order::Name, 0).0), [2]);
        // The search finds the copy its words are in.
        let two = Filters { search: "2 kick".into(), ..on.clone() };
        assert_eq!(ids(&list.view(&two, Order::Name, 0)), [2]);
        // A list sharing these sounds keeps their copies; Clear leaves the switch on.
        assert_eq!(list.sharing().copies(), Some(2));
        let mut busy = Filters { favourites: true, ..on.clone() };
        assert!(busy.any());
        busy = busy.cleared();
        assert!(busy.hide_copies && !busy.any());
        busy.toggle(HIDE_COPIES);
        assert!(!busy.hide_copies);
    }

    #[test]
    fn copies_count_once_beside_a_tag_however_its_capitals_go_and_a_list_with_use_keeps_them() {
        let mut sounds: Vec<Sound> = (1..=2).map(|i| sound(i, &format!("{i}.wav"), "Kick", None)).collect();
        sounds[0].tags = vec!["Dark".into()];
        sounds[1].tags = vec!["dark".into()];
        let mut list = SoundList::new(sounds);
        let with_use = list.sharing();
        list.set_copies(&SameAudio { groups: vec![vec![1, 2]], ..SameAudio::default() });
        let on = Filters { hide_copies: true, ..Filters::default() };
        let (shown, counts) = list.view_counted(&on, Order::Name, 0);
        assert_eq!(shown.len(), 1);
        assert_eq!(counts.tags.get("dark"), Some(&1));
        assert_eq!(counts.types.get("kick"), Some(&1));
        assert!(list.has_copies(0) && list.has_copies(1));
        // A list made from the same sounds before the copies were found takes them from the one that knows them.
        let mut with_use = with_use;
        assert_eq!(with_use.copies(), None);
        with_use.keep_copies_of(&list);
        assert_eq!(with_use.copies(), Some(1));
        let mut other = SoundList::new(vec![sound(1, "1.wav", "Kick", None), sound(2, "2.wav", "Kick", None)]);
        other.keep_copies_of(&list);
        assert_eq!(other.copies(), None);
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
        let tempo = |low: i32, high: i32| {
            let mut f = Filters::default();
            f.set_tempo(low, high);
            f
        };
        assert_eq!(by(tempo(130, 150)), vec![2]);
        // Both ends are in, so 90 is up to 90 and 90 and up; a sound with no tempo is in neither.
        assert_eq!(by(tempo(TEMPO_LOW, 90)), vec![4]);
        assert_eq!(by(tempo(90, TEMPO_HIGH)), vec![4, 2]);
        // Both handles at their ends is every sound, the ones with no tempo too.
        assert_eq!(by(tempo(TEMPO_LOW, TEMPO_HIGH)), vec![4, 3, 1, 2]);
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
        // The handles go to the nearest step, the lower one first, and stay on the slider.
        f.set_tempo(112, 88);
        f.pick(TAG, "Dark");
        f.pick(TAG, "Warm");
        f.pick(LENGTH, "Loops");
        assert_eq!(f.sound_types, ["Kick", "Vocal"]);
        assert_eq!(f.keys, [(0, false)]);
        assert_eq!(f.tempo, (Some(90.0), Some(110.0)));
        let mut ends = Filters::default();
        ends.set_tempo(20, 400);
        assert!(!ends.any());
        ends.set_tempo(TEMPO_LOW, 120);
        assert_eq!(ends.tempo, (None, Some(120.0)));
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
        keep_first_types(&mut more.sound_types, &types, TYPE_CHIPS);
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
        // One sound at 90 BPM and one at 140: the bars are 10 BPM wide from 60.
        assert_eq!(counts.tempos[3], 1);
        assert_eq!(counts.tempos.iter().sum::<usize>(), 2);
        let pick = chips(&Filters::default(), Some(&counts), &[], &[]).tempo;
        assert_eq!((pick.low, pick.high, pick.words.as_str()), (TEMPO_LOW, TEMPO_HIGH, "Any tempo"));
        assert_eq!((pick.bars.len(), pick.bars[3]), (TEMPO_BARS, 1.0));
        let mut slow = Filters::default();
        slow.set_tempo(TEMPO_LOW, 100);
        assert_eq!(chips(&slow, None, &[], &[]).tempo.words, "Up to 100 BPM");
        slow.set_tempo(90, 130);
        assert_eq!(chips(&slow, None, &[], &[]).tempo.words, "90 to 130 BPM");
        slow.set_tempo(150, TEMPO_HIGH);
        assert_eq!(chips(&slow, None, &[], &[]).tempo.words, "150 BPM and up");
        assert_eq!((tempo_bar(12.0), tempo_bar(999.0), tempo_bar(f64::NAN)), (0, TEMPO_BARS - 1, 0));
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
    fn a_column_with_room_shows_more_types_before_the_more_menu() {
        // Twelve types, the most used first.
        let types: Vec<(String, usize)> = (0..12).map(|i| (format!("Type {i:02}"), 100 - i)).collect();
        let f = Filters { sound_types: vec!["Type 10".into(), "Type 03".into()], ..Filters::default() };
        let split = |lines: usize| {
            let shown = chips_showing(&f, None, &types, &[], lines).types;
            (shown.chips.len(), shown.more.len())
        };
        // As ever by default; then as many as there's room for, and no menu once they all fit.
        assert_eq!(chips(&f, None, &types, &[]).types, chips_showing(&f, None, &types, &[], TYPE_CHIPS).types);
        assert_eq!((split(TYPE_CHIPS), split(11), split(12), split(40)), ((9, 3), (11, 1), (12, 0), (12, 0)));
        // The menu's Any unpicks only what's in the menu as it's shown.
        let mut picked = f.sound_types.clone();
        keep_first_types(&mut picked, &types, 11);
        assert_eq!(picked, ["Type 10", "Type 03"]);
        keep_first_types(&mut picked, &types, 9);
        assert_eq!(picked, ["Type 03"]);
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
        // Turned round (a second click on a heading); a shuffle stays as it is.
        let mut turned = list.view(&all, Order::Newest, 0);
        list.turn_round(&mut turned, Order::Newest);
        assert_eq!(ids(&list, &turned), vec![1, 2, 3, 4]);
        let mut shuffled = list.view(&all, Order::Shuffled, 7);
        list.turn_round(&mut shuffled, Order::Shuffled);
        assert_eq!(shuffled, list.view(&all, Order::Shuffled, 7));
        assert_eq!(Order::from_column("Type"), Some(Order::Type));
        assert_eq!(Order::from_column(""), None);
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
    fn clear_takes_off_the_search_too() {
        let andy = Filters { search: "andy".into(), favourites: true, hide_copies: true, ..Filters::default() };
        assert!(Filters { search: "andy".into(), ..Filters::default() }.any());
        assert!(!Filters { search: "  ".into(), ..Filters::default() }.any());
        assert!(Filters { favourites: true, ..Filters::default() }.any());
        assert_eq!(andy.cleared(), Filters { hide_copies: true, ..Filters::default() });
        assert!(!andy.cleared().any());
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
        let synced = |made, sounds| Synced { made, sounds, ..Synced::default() };
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
            "It's damaged, and it couldn't be mended (only 0.4 GB is free), so it's read as it is."
        );
        let made = chrono::Local.with_ymd_and_hms(2026, 11, 6, 21, 14, 30).unwrap();
        assert_eq!(
            put_back_line(made, None),
            "It was damaged, so it was put back from its backup copy of 6 Nov 21:14. The damaged one is kept beside \
             it for a week, as sounds-rust.db.damaged."
        );
        let since = |kept, unread| put_back_line(made, Some(&mo_sounds::compact::Salvaged { kept, unread }));
        assert_eq!(since(0, 0), put_back_line(made, None));
        assert!(since(4, 0).contains("of 6 Nov 21:14, with everything since. The damaged one"));
        assert!(since(4, 1).contains("of 6 Nov 21:14, with everything since that could still be read of it. The"));
        assert!(since(0, 1).contains("of 6 Nov 21:14 (nothing since could still be read of it). The"));
        let salvaged = |kept, unread| salvaged_line(&mo_sounds::compact::Salvaged { kept, unread }, None);
        assert_eq!(
            salvaged(12, 0),
            "It was damaged, and there's no backup copy, so it was made again from sounds.db, with all of it: your \
             favourites, tags, plays and the sounds the scan added. The damaged one is kept beside it for a week, as \
             sounds-rust.db.damaged."
        );
        let unusable = mo_sounds::compact::Salvaged { kept: 3, unread: 0 };
        assert!(
            salvaged_line(&unusable, Some("the backup copy can't be used: it's damaged"))
                .starts_with("It was damaged, and the backup copy can't be used: it's damaged, so it was made again")
        );
        assert!(salvaged(12, 3).contains("with all of it that could still be read (the rest is as sounds.db has it)."));
        assert!(salvaged(0, 1).contains("none of it could still be read, so it's as sounds.db has it."));
        assert!(salvaged(0, 0).contains("nothing in it differed from sounds.db."));
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
        let d = RealDates::now();
        let card = projects_card(&d, None, None);
        assert_eq!((card.title.as_str(), card.lines.len()), ("Used in", 0));
        // Its own, newest first, by name without the ending, with its DAW and the day each was saved.
        let card = projects_card(&d, Some(&[&a, &b][..]), None);
        assert_eq!(card.title, "Used in 2");
        assert_eq!(card.note, "");
        let names: Vec<(i64, &str, &str)> =
            card.lines.iter().map(|l| (l.id, l.name.as_str(), l.daw.as_str())).collect();
        assert_eq!(names, [(7, "Didums_77", "FL Studio"), (3, "Old one", "Bitwig Studio")]);
        assert_eq!(card.lines[0].saved, crate::midi::date_text(&d, 1_760_000_000));
        // A save the scan didn't date shows no day, rather than 1 Jan 1970.
        let undated = project(9, r"K:\Projects\Undated.flp", 0);
        assert_eq!(projects_card(&d, Some(&[&undated][..]), None).lines[0].saved, "");
        // None of its own: the sound it sounds like has some.
        let card = projects_card(&d, Some(&[][..]), Some(("VEH1 FX - 37", &[&a][..])));
        assert_eq!(card.title, "Used in 0");
        assert_eq!(card.note, "No project of yours uses it yet. VEH1 FX - 37, which it sounds like, is in 1 project:");
        assert_eq!(card.lines.len(), 1);
        // Neither has any.
        let card = projects_card(&d, Some(&[][..]), Some(("VEH1 FX - 37", &[][..])));
        assert_eq!((card.note.as_str(), card.lines.len()), ("No project of yours uses it yet.", 0));
        // A sound in more projects than the card lists: the newest.
        let many: Vec<ProjectFile> =
            (0..1_200).map(|n| project(n, &format!("K:\\P\\{n}.flp"), 1_700_000_000 - n)).collect();
        let many: Vec<&ProjectFile> = many.iter().collect();
        let card = projects_card(&d, Some(many.as_slice()), None);
        assert_eq!((card.title.as_str(), card.note.as_str()), ("Used in 1,200", "The newest 50:"));
        assert_eq!(card.lines.len(), PROJECTS_SHOWN);
        let card = projects_card(&d, Some(&[][..]), Some(("VEH1 FX - 37", many.as_slice())));
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

    #[test]
    fn a_date_that_isnt_real_comes_last_either_way() {
        // A sound dated 2107 (a broken date), one with no date (it reads as 1970), and two real ones.
        let day = |y: i32| NaiveDate::from_ymd_opt(y, 3, 1).and_then(|d| d.and_hms_opt(12, 0, 0)).unwrap();
        let mut sounds: Vec<Sound> = (1..=4).map(|id| sound(id, &format!("{id}.wav"), "Kick", None)).collect();
        for (s, y) in sounds.iter_mut().zip([2107, 2019, 1970, 2024]) {
            s.added = day(y);
        }
        let mut list = SoundList::new(sounds);
        let newest = list.view(&Filters::default(), Order::Newest, 0);
        assert_eq!(ids(&list, &newest), vec![4, 2, 1, 3]);
        let mut oldest = newest.clone();
        list.turn_round(&mut oldest, Order::Newest);
        assert_eq!(ids(&list, &oldest), vec![2, 4, 3, 1]);
        // It's in no year: not on the strip, and no span of years lets it through.
        assert_eq!(list.year_counts(false), [(2024, 1), (2019, 1)]);
        let every_year = Filters { years: Some((1, 9999)), ..Filters::default() };
        assert_eq!(ids(&list, &list.view(&every_year, Order::Newest, 0)), vec![4, 2]);

        // Nor is it a clue to which copy came first: of the same audio, the one with a real date is the older.
        let mut copies = list.sharing();
        copies.set_copies(&SameAudio { groups: vec![vec![1, 2, 3]], ..SameAudio::default() });
        let at = |id: i64| copies.find(id).unwrap();
        assert_eq!(copies.why_kept(at(2), at(3)), KeptFor::Older);
        assert!(copies.keep_first(at(2), at(3)).is_lt() && copies.keep_first(at(2), at(1)).is_lt());
        let on = Filters { hide_copies: true, ..Filters::default() };
        assert_eq!(ids(&copies, &copies.view(&on, Order::Newest, 0)), vec![4, 2]);

        // The card's projects too: one dated 2099 comes after the real ones, as one the scan didn't date does.
        list.set_used(HashMap::from([(1, Usage { projects: 3, project_ids: vec![10, 11, 12], ..Usage::default() })]));
        let files = HashMap::from([
            (10, project(10, "undated.flp", 0)),
            (11, project(11, "2019.flp", 1_559_390_400)),
            (12, project(12, "2099.flp", 4_102_401_600)),
        ]);
        list.set_project_files(Arc::new(files));
        let of = list.projects_of(0).unwrap_or_default();
        assert_eq!(of.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["2019.flp", "2099.flp", "undated.flp"]);
        let card = projects_card(list.dates(), Some(&of), None);
        assert_eq!(card.lines.iter().map(|l| l.saved.is_empty()).collect::<Vec<_>>(), [false, true, true]);
    }
}
