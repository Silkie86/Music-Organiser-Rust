//! Saves grouped into songs: `Song.flp`, `Song_2.flp` … `Song_77.flp` in one folder are one song, with the runs it was
//! worked on in and the songs that may be one. The C# app's `SongVersions` and `SongChoices`, rule for rule.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use chrono::{Datelike, Months, NaiveDateTime};
use mo_formats::case;
use regex::Regex;

use crate::Save;

/// Your own say on which saves are one song, over the guess from names and folders: a song joined to another, a save
/// split off into a song of its own, and songs you said are different, so the app doesn't ask again (table
/// `song_choices`).
#[derive(Clone, Debug, Default)]
pub struct SongChoices {
    joined: HashMap<String, String>,
    split: HashSet<String>,
    apart: HashSet<String>,
}

impl SongChoices {
    /// From the rows of `song_choices`: its kind ("join", "split" or "apart"), `a` and `b`.
    pub fn from_rows<'a>(rows: impl IntoIterator<Item = (&'a str, &'a str, &'a str)>) -> SongChoices {
        let mut c = SongChoices::default();
        for (kind, a, b) in rows {
            match kind {
                "join" => {
                    c.joined.insert(a.to_owned(), b.to_owned());
                }
                "split" => {
                    c.split.insert(case::to_lower(a));
                }
                "apart" => {
                    c.apart.insert(a.to_owned());
                }
                _ => {}
            }
        }
        c
    }

    pub fn is_empty(&self) -> bool {
        self.joined.is_empty() && self.split.is_empty()
    }

    /// The song a save goes under: its own when split off, else its name's song, followed to whatever that was
    /// joined to.
    pub fn key_of(&self, path: &str, folder: &str, daw: &str, name: &str) -> String {
        if self.is_split(path) { self.follow(&split_key(path)) } else { self.follow(&key(folder, daw, name)) }
    }

    /// Where a song's key ends up after the joins (a few steps at most; a loop stops after 32).
    pub fn follow(&self, key: &str) -> String {
        let mut key = key;
        for _ in 0..32 {
            match self.joined.get(key) {
                Some(to) => key = to,
                None => break,
            }
        }
        key.to_owned()
    }

    /// Whether you took this save out of its song (and maybe put it under another).
    pub fn is_split(&self, path: &str) -> bool {
        self.split.contains(&case::to_lower(path))
    }

    /// Whether you said these two songs are different.
    pub fn apart(&self, a: &str, b: &str) -> bool {
        self.apart.contains(&pair(a, b))
    }

    /// Whether the song with this key was joined to another, or a save in it split off.
    pub fn changed<'a>(&self, key: &str, paths: impl IntoIterator<Item = &'a str>) -> bool {
        self.joined.contains_key(key)
            || self.joined.values().any(|v| v == key)
            || paths.into_iter().any(|p| self.is_split(p))
    }
}

/// The song of one save split off from its song.
pub fn split_key(path: &str) -> String {
    format!("save:{}", case::to_lower(path))
}

/// Two songs' keys as one, in a set order (by UTF-16 units, as .NET's ordinal order).
pub fn pair(a: &str, b: &str) -> String {
    if a.encode_utf16().cmp(b.encode_utf16()).is_lt() { format!("{a}\u{1e}{b}") } else { format!("{b}\u{1e}{a}") }
}

/// One song and its saves: the one it opens, and the others, newest first.
#[derive(Clone, Debug)]
pub struct Song {
    pub name: String,
    pub latest: Save,
    pub older: Vec<Save>,
    /// The song's key (see [`key`] and [`SongChoices`]), for joining it to another.
    pub key: String,
    /// The same files found somewhere else as well (a project folder copied to another drive): hidden, never deleted
    /// (rule 2). Each is the same save as one in `latest` or `older`.
    pub copies: Vec<Save>,
}

impl Song {
    pub fn save_count(&self) -> usize {
        self.older.len() + 1
    }

    /// Every save, the one it opens first.
    pub fn saves(&self) -> impl Iterator<Item = &Save> {
        std::iter::once(&self.latest).chain(&self.older)
    }
}

/// Two songs that may be saves of one: their names (or what's in them) are close but not close enough to be sure.
#[derive(Clone, Debug)]
pub struct SongDoubt {
    pub a: Song,
    pub b: Song,
    pub why: String,
    /// B is one save inside A, put there by a save mark ("final", "copy", "v2", a date): is it a save of A?
    pub one_save: bool,
}

/// One working period on a song: saves less than the run gap apart. `last` is the save it ended on, which is what
/// gets opened. `reworked`: it's quite different from how the song first ended up, not just saved again.
#[derive(Clone, Debug)]
pub struct SaveRun {
    pub last: Save,
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub saves: usize,
    pub first: bool,
    pub reworked: bool,
}

impl SaveRun {
    /// "2019 original", "2024 rework", or "2023 re-save" for a run that only saved it again.
    pub fn label(&self) -> String {
        let what = if self.first {
            "original"
        } else if self.reworked {
            "rework"
        } else {
            "re-save"
        };
        format!("{} {what}", self.end.year())
    }

    /// "Didums_77 · 3 Aug 2024 · 12 saves".
    pub fn details(&self) -> String {
        let mut parts = vec![self.last.name.clone(), self.end.format("%-d %b %Y").to_string()];
        if self.saves != 1 {
            parts.push(format!("{} saves", self.saves));
        }
        parts.join(" · ")
    }
}

/// A gap this long between two saves (Solomon's pick: 3 months) starts a new run, unless Settings says otherwise.
pub const DEFAULT_RUN_GAP_MONTHS: u32 = 3;

/// A "final" save's number: above every numbered save, so it's the one the song opens.
const FINAL_NUMBER: i32 = 1_000_000;

/// The names DAWs give a new project: "Untitled 3" and "Untitled 4" are different songs that were never named, not
/// saves of one, so a number after one of these is part of the name (FL Studio's "Untitled_2" is still a save).
const DAW_DEFAULTS: [&str; 5] = ["untitled", "untitled project", "untitled song", "new project", "new song"];

fn is_daw_default(name: &str) -> bool {
    DAW_DEFAULTS.iter().any(|d| case::eq_ignore_case(d, name))
}

/// A fixed pattern. They're all tested (see the tests), so none fails to build; if one ever did, it would match
/// nothing rather than stop the app.
struct Pattern(Option<Regex>);

impl Pattern {
    fn captures<'h>(&self, haystack: &'h str) -> Option<regex::Captures<'h>> {
        self.0.as_ref()?.captures(haystack)
    }

    fn is_match(&self, haystack: &str) -> bool {
        self.0.as_ref().is_some_and(|r| r.is_match(haystack))
    }

    fn replace<'h>(&self, haystack: &'h str, with: &str) -> std::borrow::Cow<'h, str> {
        match &self.0 {
            Some(r) => r.replace(haystack, with),
            None => std::borrow::Cow::Borrowed(haystack),
        }
    }

    fn find_iter<'r, 'h>(&'r self, haystack: &'h str) -> impl Iterator<Item = regex::Match<'h>> + use<'r, 'h> {
        self.0.iter().flat_map(move |r| r.find_iter(haystack))
    }
}

fn re(pattern: &str) -> Pattern {
    Pattern(Regex::new(pattern).ok())
}

/// "Song_12".
static NUMBERED: LazyLock<Pattern> = LazyLock::new(|| re(r"^(.+?)_([0-9]{1,4})$"));
/// "Song 12", "Song-12", "Song - 12" (up to three digits, so "Swu oct 2026" stays a name).
static SPACE_NUMBERED: LazyLock<Pattern> = LazyLock::new(|| re(r"^(.+?)(?:\s*-\s*|\s+)([0-9]{1,3})$"));
/// "track2", "FLP19": a number glued to the last word.
static GLUED_ON: LazyLock<Pattern> = LazyLock::new(|| re(r"^(.*\p{L})([0-9]{1,3})$"));
/// Fix's copies: "Didums_70 (fixed)", "Didums_70 (fixed 2)".
static FIXED_TAIL: LazyLock<Pattern> = LazyLock::new(|| re(r"(?i)(\s*\(fixed(\s+[0-9]+)?\))+$"));
/// A word that marks a save, then its number: "Song v2", "Song v2.1", "Song take 4", "Song mix_3", "Song rev 7". There
/// has to be a space, "_", "-" or "." before the word, so "Remix2" isn't read as a "mix".
static MARKED_NUMBER: LazyLock<Pattern> = LazyLock::new(|| {
    re(
        r"(?i)^(.+?)[\s_.-]+(?:v|ver|version|take|tk|mix|rev|revision|draft|alt)\s*[.#_]?\s*([0-9]{1,4})(?:\.([0-9]{1,3}))?$",
    )
});
/// The same words glued on with a capital: "SongV2", "SongTake4".
static GLUED_NUMBER: LazyLock<Pattern> =
    LazyLock::new(|| re(r"^(.*\p{Ll})(?:V|Ver|VER|Take|TAKE|Mix|MIX|Rev|REV)([0-9]{1,4})$"));
/// "Song final", "Song FINAL FINAL", "Song final2", "Song final v3": the last save, ahead of any number.
static FINAL: LazyLock<Pattern> =
    LazyLock::new(|| re(r"(?i)^(.+?)[\s_.-]+final(?:[\s_.-]*final)*(?:[\s_.-]*v?([0-9]{1,3}))?$"));
/// Copies and side saves that say nothing about order: "Song - Copy", "Song - Copy (2)", "Song (2)", "Song copy",
/// "Song wip", "Song backup", and a date: "Song 2024-03-01", "Song_20240301".
static SIDE_SAVE: LazyLock<Pattern> = LazyLock::new(|| {
    re(concat!(
        r"(?i)^(.+?)(?:\s*-\s*copy(?:\s*\([0-9]{1,3}\))?|\s+copy(?:\s*[0-9]{1,3})?|\s*\(copy\)|\s*\([0-9]{1,2}\)",
        r"|[\s_.-]+(?:wip|backup|bak|bkp)",
        r"|[\s_.-]+(?:19|20)[0-9][0-9][-_.]?(?:0[1-9]|1[0-2])[-_.]?(?:0[1-9]|[12][0-9]|3[01]))$"
    ))
});
/// A plugin's version or format on the end of its name.
static VERSION_TAIL: LazyLock<Pattern> = LazyLock::new(|| {
    re(
        r"(?i)(\s*\((x64|x86|64-?bit|32-?bit|vst3?|clap|au)\)|[\s_-]+(x64|x86|64-?bit|32-?bit|vst3?)|[\s_-]*v?\d+(\.\d+)*)$",
    )
});
static LETTER: LazyLock<Pattern> = LazyLock::new(|| re(r"\p{L}"));

/// A group's text as a number: up to four ASCII digits, so it always fits.
fn number(m: Option<regex::Match<'_>>) -> Option<i32> {
    m.and_then(|m| m.as_str().parse().ok())
}

fn group<'h>(c: &regex::Captures<'h>, i: usize) -> &'h str {
    c.get(i).map_or("", |m| m.as_str())
}

/// Whether a name is one of Fix's copies: "Didums_70 (fixed)".
pub fn is_fixed_copy(name: &str) -> bool {
    FIXED_TAIL.is_match(name)
}

fn without_fixed(name: &str) -> String {
    FIXED_TAIL.replace(name, "").into_owned()
}

/// Whether what's left is a song's own name, not nothing or a DAW's "Untitled".
fn is_song(rest: &str) -> bool {
    let song = rest.trim();
    !song.is_empty() && !is_daw_default(song)
}

/// "Didums_70" → ("Didums", 70); "Didums" → ("Didums", 1). A fixed copy counts as the save it was made from. The same
/// name with a number on the end is a save of one song, whether the number is glued to the last word ("track2";
/// "fleck athens FLP19" is a save of "fleck athens FLP"), after a space, a dash or "_". The usual marks count too:
/// "Song v2.1", "Song take 4", "Song final" (the last save), and copies, side saves and dates, which keep the number of
/// what's left. A DAW's own name for a new project is never cut down to.
pub fn split(name: &str) -> (String, i32) {
    let (song, number, _) = worked_out(name);
    (song, number)
}

/// A name's song, its number and its song by the plain rule alone, worked out once per name on each thread: every
/// grouping, key and sort asks for them many times per save, and the patterns are the slow part (as the C# app's
/// `SplitCache`).
fn worked_out(name: &str) -> (String, i32, String) {
    thread_local! {
        static KNOWN: RefCell<HashMap<String, (String, i32, String)>> = RefCell::new(HashMap::new());
    }
    if let Some(known) = KNOWN.with(|k| k.borrow().get(name).cloned()) {
        return known;
    }
    let (song, number) = split_uncached(name);
    let found = (song, number, plain_song_uncached(name));
    KNOWN.with(|k| {
        let mut k = k.borrow_mut();
        if k.len() > 200_000 {
            k.clear();
        }
        k.insert(name.to_owned(), found.clone());
    });
    found
}

fn split_uncached(name: &str) -> (String, i32) {
    let mut name = without_fixed(name);
    let mut found = None;
    // Marks stack ("Song v2 final", "Song_3 - Copy"): take them off from the end, keeping the first number found.
    for _ in 0..4 {
        let Some((song, n)) = unmark(&name) else { break };
        name = song;
        found = found.or(n);
    }
    if let Some(m) = NUMBERED.captures(&name)
        && !group(&m, 1).trim().is_empty()
    {
        return (group(&m, 1).trim_end().to_owned(), found.or(number(m.get(2))).unwrap_or(1));
    }
    let tail = SPACE_NUMBERED.captures(&name).or_else(|| GLUED_ON.captures(&name));
    if let Some(t) = tail
        && is_song(group(&t, 1))
    {
        return (group(&t, 1).trim().to_owned(), found.or(number(t.get(2))).unwrap_or(1));
    }
    (name, found.unwrap_or(1))
}

/// The name with one save mark taken off its end, and the number it gave (none when it gave none).
fn unmark(name: &str) -> Option<(String, Option<i32>)> {
    if let Some(f) = FINAL.captures(name)
        && is_song(group(&f, 1))
    {
        return Some((group(&f, 1).trim().to_owned(), Some(FINAL_NUMBER + number(f.get(2)).unwrap_or(0))));
    }
    for marked in [&MARKED_NUMBER, &GLUED_NUMBER] {
        if let Some(m) = marked.captures(name)
            && is_song(group(&m, 1))
        {
            // "v2.1" is save 2: the date tells it from "v2".
            return Some((group(&m, 1).trim().to_owned(), number(m.get(2))));
        }
    }
    if let Some(s) = SIDE_SAVE.captures(name)
        && is_song(group(&s, 1))
    {
        return Some((group(&s, 1).trim().to_owned(), None));
    }
    None
}

/// The song a name is a numbered save of by the plain rule alone (a number on the end), leaving save marks such as
/// "final", "copy", "v2" or a date in the name.
fn plain_song(name: &str) -> String {
    worked_out(name).2
}

fn plain_song_uncached(name: &str) -> String {
    let name = without_fixed(name);
    if let Some(m) = NUMBERED.captures(&name)
        && !group(&m, 1).trim().is_empty()
    {
        return group(&m, 1).trim_end().to_owned();
    }
    let tail = SPACE_NUMBERED.captures(&name).or_else(|| GLUED_ON.captures(&name));
    match tail {
        Some(t) if is_song(group(&t, 1)) => group(&t, 1).trim().to_owned(),
        _ => name,
    }
}

/// Whether a save mark ("final", "copy", "wip", "v2", a date) put this save with its song: a guess, so the app asks.
pub fn marked(name: &str) -> bool {
    let (song, _, plain) = worked_out(name);
    !case::eq_ignore_case(&plain, &song)
}

/// Whether a song's saves aren't all one name numbered (a marked save, or songs you joined). Their numbers then don't
/// count against each other, so the newest is the one saved last: an old "final" doesn't win over newer saves.
fn mixed<'a>(names: impl Iterator<Item = &'a str>) -> bool {
    let mut first = None;
    for n in names {
        let k = case::key(&plain_song(n));
        match &first {
            None => first = Some(k),
            Some(f) if *f != k => return true,
            _ => {}
        }
    }
    false
}

/// When the song was first made: its oldest save, or the date its file records (FL Studio keeps it through re-saves),
/// whichever is earlier.
pub fn first_made<'a>(saves: impl IntoIterator<Item = &'a Save>) -> Option<NaiveDateTime> {
    saves.into_iter().flat_map(|s| std::iter::once(s.modified).chain(s.created.filter(|c| c.year() >= 1995))).min()
}

/// The saves split into working periods, oldest first: a gap of `gap_months` or more between two saves starts a new
/// one. Moved files are left out, since they can't be opened.
pub fn runs<'a>(saves: impl IntoIterator<Item = &'a Save>, gap_months: u32) -> Vec<SaveRun> {
    let mut ordered: Vec<&Save> = saves.into_iter().filter(|s| !s.file_missing).collect();
    ordered.sort_by_cached_key(|s| (s.modified, split(&s.name).1));
    let mut groups: Vec<Vec<&Save>> = Vec::new();
    for save in ordered {
        let starts_new = match groups.last().and_then(|g| g.last()) {
            None => true,
            Some(last) => {
                last.modified.checked_add_months(Months::new(gap_months)).is_none_or(|end| save.modified >= end)
            }
        };
        if starts_new {
            groups.push(Vec::new());
        }
        if let Some(g) = groups.last_mut() {
            g.push(save);
        }
    }
    let original = groups.first().and_then(|g| g.last()).copied();
    groups
        .iter()
        .enumerate()
        .filter_map(|(i, g)| {
            let (first, last) = (g.first()?, g.last()?);
            Some(SaveRun {
                last: (*last).clone(),
                start: first.modified,
                end: last.modified,
                saves: g.len(),
                first: i == 0,
                reworked: i > 0 && original.is_some_and(|o| is_reworked(o, last)),
            })
        })
        .collect()
}

/// Whether `later` is quite different from `earlier`, not just saved again: plugins added or removed (a newer
/// version of the same one doesn't count), channels added or removed, the number of notes changed by more than a
/// fifth, or more than 8 bars longer or shorter. What isn't known about either save is left out.
pub fn is_reworked(earlier: &Save, later: &Save) -> bool {
    let before: HashSet<String> = earlier.plugin_names.iter().map(|p| plugin_family(p)).collect();
    let after: HashSet<String> = later.plugin_names.iter().map(|p| plugin_family(p)).collect();
    if before != after {
        return true;
    }
    if earlier.track_count >= 0 && later.track_count >= 0 && earlier.track_count != later.track_count {
        return true;
    }
    let notes_moved = i64::from(later.note_count) - i64::from(earlier.note_count);
    if earlier.note_count >= 0 && later.note_count >= 0 && notes_moved.abs() * 5 > i64::from(earlier.note_count.max(1))
    {
        return true;
    }
    earlier.length_bars >= 0 && later.length_bars >= 0 && (later.length_bars - earlier.length_bars).abs() > 8
}

/// A plugin's name without its version or format: "Serum 2" and "Serum (x64)" are both "serum".
pub fn plugin_family(name: &str) -> String {
    let mut n = name.trim().to_owned();
    loop {
        let shorter = VERSION_TAIL.replace(&n, "").trim().to_owned();
        if shorter.is_empty() || shorter == n {
            break;
        }
        n = shorter;
    }
    case::to_lower(&n)
}

/// Saves of the same song share a key: same folder (see [`song_folder`]), same DAW, same name before the number.
pub fn key(folder: &str, daw: &str, name: &str) -> String {
    let song = split(name).0;
    case::to_lower(&format!("{}\u{1f}{daw}\u{1f}{song}", folder_of_song(folder, &song)))
}

/// The folder a song's saves sit in. Bitwig keeps each project in a folder named after it, so a folder named like the
/// song counts as the folder it's in: Projects\bored_58\bored_58.bwproject and Projects\bored_57\bored_57.bwproject
/// are one song. Always without a closing separator, so a drive's root reads "C:".
pub fn song_folder(folder: &str, name: &str) -> String {
    folder_of_song(folder, &split(name).0)
}

/// [`song_folder`] for a save whose song is already known.
fn folder_of_song(folder: &str, song: &str) -> String {
    let trimmed = folder.trim_end_matches(['\\', '/']);
    let Some(cut) = trimmed.rfind(['\\', '/']) else { return trimmed.to_owned() };
    let (parent, own) = (trimmed.get(..cut).unwrap_or(""), trimmed.get(cut + 1..).unwrap_or(""));
    if case::eq_ignore_case(&split(own).0, song) { parent.to_owned() } else { trimmed.to_owned() }
}

/// One group per song, in the order their first save comes in `saves` (newest first, as the library lists them).
/// Beyond the C# app, the same file found in two places (a copied project folder) is one save, so their songs are one,
/// and the extra copy is kept aside in [`Song::copies`]. The
/// save a song opens is the highest-numbered file still on disk (ties go to the most recently saved); folder copies
/// reset dates, numbers survive. `choices` win over the guess.
pub fn group_songs(saves: &[Save], choices: &SongChoices) -> Vec<Song> {
    let ids: Vec<Option<FileId>> = saves.iter().map(same_file).collect();
    // A copy of a save you split off goes with it, not back into the song it came from.
    let split_ids: HashMap<&FileId, String> = saves
        .iter()
        .zip(&ids)
        .filter(|(s, _)| choices.is_split(&s.path))
        .filter_map(|(s, id)| Some((id.as_ref()?, choices.follow(&split_key(&s.path)))))
        .collect();
    let mut order: Vec<String> = Vec::new();
    let mut by_key: HashMap<String, Vec<(&Save, Option<&FileId>)>> = HashMap::new();
    for (s, id) in saves.iter().zip(&ids) {
        let k = match id.as_ref().and_then(|id| split_ids.get(id)) {
            Some(own) => own.clone(),
            None => choices.key_of(&s.path, &s.folder, &s.daw, &s.name),
        };
        by_key
            .entry(k)
            .or_insert_with_key(|k| {
                order.push(k.clone());
                Vec::new()
            })
            .push((s, id.as_ref()));
    }
    // The same file in two places is one save, so their songs are one (unless you said otherwise).
    for (k, to) in copied_groups(&order, &by_key, choices) {
        if let Some(moved) = by_key.remove(&k)
            && let Some(into) = by_key.get_mut(&to)
        {
            into.extend(moved);
        }
    }
    order
        .into_iter()
        .filter_map(|k| {
            let mut g = by_key.remove(&k)?;
            let is_mixed = mixed(g.iter().map(|(s, _)| s.name.as_str()));
            g.sort_by_cached_key(|(s, _)| {
                let n = split(&s.name).1;
                (
                    s.file_missing,
                    std::cmp::Reverse(if is_mixed { 0 } else { n }),
                    std::cmp::Reverse(s.modified),
                    // "Didums_70 - Copy" saved with Didums_70: the original.
                    marked(&s.name),
                    std::cmp::Reverse(n),
                    // Saved the same second: the fix came after.
                    std::cmp::Reverse(FIXED_TAIL.is_match(&s.name)),
                )
            });
            let ordered: Vec<&Save> = g.iter().map(|(s, _)| *s).collect();
            let name = song_name(&k, &ordered);
            // Of the same file in several places, the one you organised stays (rated, tagged, noted, opened), else
            // the one the library found first; the others are its copies.
            let mut best: HashMap<&FileId, &Save> = HashMap::new();
            for (s, id) in &g {
                if let Some(id) = id {
                    best.entry(id)
                        .and_modify(|b| {
                            if original(s) > original(b) {
                                *b = s
                            }
                        })
                        .or_insert(s);
                }
            }
            let (mut kept, mut copies) = (Vec::new(), Vec::new());
            for (s, id) in &g {
                match id.and_then(|id| best.get(id)) {
                    Some(b) if !std::ptr::eq(*b, *s) => copies.push((*s).clone()),
                    _ => kept.push((*s).clone()),
                }
            }
            let mut kept = kept.into_iter();
            let latest = kept.next()?;
            Some(Song { name, latest, older: kept.collect(), key: k, copies })
        })
        .collect()
}

/// What makes two saves the same file in two places: the same name, DAW, size, tempo, counts of notes, bars, channels
/// and plugins, and the date the project was made. A save that's gone, empty, not read, or named as a DAW names a new
/// project ("Untitled" from one template) can't be told apart, so it has none.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct FileId {
    name: String,
    daw: String,
    size: i64,
    tempo: Option<u64>,
    counts: [i32; 3],
    plugins: usize,
    created: Option<NaiveDateTime>,
}

/// What [`group_songs`] knows a save's file by: two saves with equal ones are the same file in two places. A save it
/// can't tell apart has none.
pub fn file_id(s: &Save) -> Option<impl Eq + std::hash::Hash + use<>> {
    same_file(s)
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

fn same_file(s: &Save) -> Option<FileId> {
    if s.file_missing || s.size <= 0 || s.note_count < 0 || s.track_count < 0 || is_daw_default(split(&s.name).0.trim())
    {
        return None;
    }
    Some(FileId {
        name: case::key(&s.name),
        daw: s.daw.clone(),
        size: s.size,
        tempo: s.tempo.map(f64::to_bits),
        counts: [s.note_count, s.length_bars, s.track_count],
        plugins: s.plugin_names.len(),
        created: s.created,
    })
}

/// How much you've organised a save, then how early the library found it (a lower id): the higher, the more it's the
/// original of a file found twice.
fn original(s: &Save) -> (bool, std::cmp::Reverse<i64>) {
    let organised = s.rating > 0
        || s.favourite
        || !s.tags.is_empty()
        || !s.notes.is_empty()
        || !s.musical_key.is_empty()
        || s.last_opened.is_some()
        || s.status != crate::STATUSES[0];
    (organised, std::cmp::Reverse(s.id))
}

/// Which groups go into which: (a group, the group it joins) for each group that holds the same file as an earlier one
/// (newest first). A save you split off stays on its own, and no two songs you kept apart end up together.
fn copied_groups(
    order: &[String],
    by_key: &HashMap<String, Vec<(&Save, Option<&FileId>)>>,
    choices: &SongChoices,
) -> Vec<(String, String)> {
    let mut first_with: HashMap<&FileId, usize> = HashMap::new();
    let mut into: Vec<usize> = (0..order.len()).collect();
    let mut members: Vec<Vec<usize>> = (0..order.len()).map(|i| vec![i]).collect();
    let root = |into: &[usize], mut i: usize| {
        while let Some(&up) = into.get(i) {
            if up == i {
                break;
            }
            i = up;
        }
        i
    };
    let key = |i: usize| order.get(i).map_or("", String::as_str);
    for (i, k) in order.iter().enumerate() {
        if k.starts_with("save:") {
            continue;
        }
        for id in by_key.get(k).into_iter().flatten().filter_map(|(_, id)| *id) {
            let Some(&j) = first_with.get(id) else {
                first_with.insert(id, i);
                continue;
            };
            let (a, b) = (root(&into, i), root(&into, j));
            if a == b {
                continue;
            }
            let (ma, mb) = (members.get(a).cloned().unwrap_or_default(), members.get(b).cloned().unwrap_or_default());
            if ma.iter().any(|&x| mb.iter().any(|&y| choices.apart(key(x), key(y)))) {
                continue;
            }
            let (to, from) = (a.min(b), a.max(b));
            if let Some(slot) = into.get_mut(from) {
                *slot = to;
            }
            let moved = members.get_mut(from).map(std::mem::take).unwrap_or_default();
            if let Some(m) = members.get_mut(to) {
                m.extend(moved);
            }
        }
    }
    (0..order.len())
        .filter_map(|i| {
            let r = root(&into, i);
            (r != i).then(|| (key(i).to_owned(), key(r).to_owned()))
        })
        .collect()
}

/// A group's name: the name of the song the others were joined to (one of its own saves names it), else the newest's.
fn song_name(key_: &str, ordered: &[&Save]) -> String {
    ordered
        .iter()
        .find(|s| key(&s.folder, &s.daw, &s.name) == key_)
        .or(ordered.first())
        .map(|s| split(&s.name).0)
        .unwrap_or_default()
}

/// Songs that may be saves of one song, for you to say: in the same folder and DAW, names that differ by a short word
/// or a letter or two once numbers, spaces and marks are left out ("track nd 1" and "track 2", "Sunrise" and
/// "Sunrize"); with `across_folders` also the same name in different folders; and saves a mark put with their song.
/// Beyond the C# app: songs in one folder with nearly the same plugins, channels and tempo (a save renamed), and the
/// same name in two folders that hold copies of the same file. Pairs you said are different aren't asked again. Newest
/// first, at most `most`.
pub fn doubts(songs: &[Song], choices: &SongChoices, across_folders: bool, most: usize) -> Vec<SongDoubt> {
    doubt_pairs(songs, choices, across_folders, most)
        .into_iter()
        .filter_map(|(pair, why)| match pair {
            Pair::Songs(i, j) => {
                Some(SongDoubt { a: songs.get(i)?.clone(), b: songs.get(j)?.clone(), why, one_save: false })
            }
            Pair::Save(gi, si) => {
                let g = songs.get(gi)?;
                let save = g.saves().nth(si)?;
                let b = Song {
                    name: save.name.clone(),
                    latest: save.clone(),
                    older: Vec::new(),
                    key: split_key(&save.path),
                    copies: Vec::new(),
                };
                Some(SongDoubt { a: g.clone(), b, why, one_save: true })
            }
        })
        .collect()
}

/// A doubt by place, without copying its songs: the two songs by place in the list (the newer first), or for a marked
/// save, the song and the save's place in [`Song::saves`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pair {
    Songs(usize, usize),
    Save(usize, usize),
}

/// [`doubts`] by place: the same questions, in the same order, with why each is asked.
pub fn doubt_pairs(songs: &[Song], choices: &SongChoices, across_folders: bool, most: usize) -> Vec<(Pair, String)> {
    // Each song's folder and skeleton, worked out once rather than for every pair.
    let folders: Vec<String> =
        songs.iter().map(|g| case::to_lower(&song_folder(&g.latest.folder, &g.latest.name))).collect();
    let named: Vec<(usize, Vec<u16>)> = songs
        .iter()
        .enumerate()
        .filter(|(_, g)| !is_daw_default(g.name.trim()))
        .map(|(i, g)| (i, skeleton(&g.name)))
        .filter(|(_, s)| s.len() >= 3)
        .collect();
    let mut found: Vec<(NaiveDateTime, Pair, String)> = Vec::new();
    let mut add = |i: usize, j: usize, why: String| {
        let (Some(a), Some(b)) = (songs.get(i), songs.get(j)) else { return };
        if a.key != b.key && !choices.apart(&a.key, &b.key) {
            // The newer song first (its name is the first offered), or the one with more saves.
            let a_first = if a.latest.modified != b.latest.modified {
                a.latest.modified > b.latest.modified
            } else {
                a.save_count() >= b.save_count()
            };
            let (i, j, newer) = if a_first { (i, j, a) } else { (j, i, b) };
            found.push((newer.latest.modified, Pair::Songs(i, j), why));
        }
    };
    let folder = |i: usize| folders.get(i).map_or("", String::as_str);
    let daw = |i: usize| songs.get(i).map_or("", |g| g.latest.daw.as_str());
    for bucket in buckets(&named, |(i, _)| (folder(*i), daw(*i))) {
        for (n, (a, sa)) in bucket.iter().enumerate() {
            for (b, sb) in bucket.iter().skip(n + 1) {
                if let Some(why) = close(sa, sb) {
                    add(*a, *b, why);
                }
            }
        }
    }
    // Renamed saves ("Untitled_5" saved as "Sunset"): songs in one folder with nearly the same plugins, channels,
    // tempo and number of notes. Only songs with the same tempo are compared, which keeps it quick in a big folder; a
    // song that looks like three or more others is a template's child, not a renamed save, so it isn't asked about.
    let by_content: Vec<(usize, HashSet<String>)> = songs
        .iter()
        .enumerate()
        .filter(|(_, g)| g.latest.tempo.is_some() && g.latest.track_count >= 0 && g.latest.note_count >= 0)
        .map(|(i, g)| (i, g.latest.plugin_names.iter().map(|p| plugin_family(p)).collect::<HashSet<_>>()))
        .filter(|(_, plugins)| plugins.len() >= 3)
        .collect();
    let tempo = |i: usize| songs.get(i).and_then(|g| g.latest.tempo).map_or(0, |t| (t * 2.0).round() as i64);
    let skeletons: HashMap<usize, &Vec<u16>> = named.iter().map(|(i, s)| (*i, s)).collect();
    let mut alike: Vec<(usize, usize)> = Vec::new();
    for bucket in buckets(&by_content, |(i, _)| (folder(*i), daw(*i), tempo(*i))) {
        for (n, (a, pa)) in bucket.iter().enumerate() {
            for (b, pb) in bucket.iter().skip(n + 1) {
                let (Some(ga), Some(gb)) = (songs.get(*a), songs.get(*b)) else { continue };
                let (la, lb) = (&ga.latest, &gb.latest);
                // Names that are close were asked about above.
                let named_alike = match (skeletons.get(a), skeletons.get(b)) {
                    (Some(sa), Some(sb)) => sa == sb || close(sa, sb).is_some(),
                    _ => false,
                };
                let shared = pa.intersection(pb).count();
                let notes_moved = i64::from(la.note_count) - i64::from(lb.note_count);
                if !named_alike
                    && shared * 5 >= (pa.len() + pb.len() - shared) * 4
                    && la.track_count.abs_diff(lb.track_count) <= 1
                    && notes_moved.abs() * 5 <= i64::from(la.note_count.max(lb.note_count).max(1))
                {
                    alike.push((*a, *b));
                }
            }
        }
    }
    let mut looks_like: HashMap<usize, usize> = HashMap::new();
    for (a, b) in &alike {
        *looks_like.entry(*a).or_default() += 1;
        *looks_like.entry(*b).or_default() += 1;
    }
    for (a, b) in alike {
        if looks_like.get(&a).is_some_and(|&n| n <= 2) && looks_like.get(&b).is_some_and(|&n| n <= 2) {
            add(a, b, "They use nearly the same plugins, channels, tempo and notes".into());
        }
    }
    // Folders that hold the same file are copies of each other (every pair of them): songs of the same name in them
    // are asked about.
    let mut copied: HashMap<String, HashSet<String>> = HashMap::new();
    for g in songs.iter().filter(|g| !g.copies.is_empty()) {
        let mut places: HashMap<FileId, Vec<String>> = HashMap::new();
        for s in g.saves().chain(&g.copies) {
            if let Some(id) = same_file(s) {
                places.entry(id).or_default().push(case::to_lower(&song_folder(&s.folder, &s.name)));
            }
        }
        for folders in places.values().filter(|f| f.len() > 1) {
            for x in folders {
                copied.entry(x.clone()).or_default().extend(folders.iter().filter(|y| *y != x).cloned());
            }
        }
    }
    let copies_of = |a: &str, b: &str| copied.get(a).is_some_and(|f| f.contains(b));
    if across_folders || !copied.is_empty() {
        for bucket in buckets(&named, |(i, s)| (daw(*i), s.clone())) {
            for (n, (a, _)) in bucket.iter().enumerate() {
                for (b, _) in bucket.iter().skip(n + 1) {
                    let (fa, fb) = (folder(*a), folder(*b));
                    if fa == fb {
                        continue;
                    }
                    if copies_of(fa, fb) {
                        add(*a, *b, "Same name in a copied folder".into());
                    } else if across_folders {
                        add(*a, *b, "Same name in different folders".into());
                    }
                }
            }
        }
    }
    // Saves a mark put with their song ("Night Drive final" with Night Drive_2 … _14): asked once each. A song whose
    // saves all carry the same mark ("Night Drive v2", "v3") has nothing to ask.
    for (gi, g) in songs.iter().enumerate().filter(|(_, g)| g.save_count() > 1) {
        let plain: Vec<String> = g.saves().map(|s| case::key(&plain_song(&s.name))).collect();
        for (si, (save, p)) in g.saves().zip(&plain).enumerate() {
            if !marked(&save.name)
                || choices.is_split(&save.path)
                || choices.apart(&g.key, &split_key(&save.path))
                || plain.iter().all(|q| q == p)
            {
                continue;
            }
            let name = without_fixed(&save.name);
            let mark = after_prefix(&name, &g.name).map(|m| m.trim_matches([' ', '_', '-', '.'])).unwrap_or("");
            let why = if mark.is_empty() {
                format!("It was put with {} by a save mark in its name", g.name)
            } else {
                format!("It was put with {} because its name adds \"{mark}\"", g.name)
            };
            found.push((g.latest.modified, Pair::Save(gi, si), why));
        }
    }
    found.sort_by_key(|(newest, _, _)| std::cmp::Reverse(*newest));
    found.truncate(most);
    found.into_iter().map(|(_, pair, why)| (pair, why)).collect()
}

/// Items grouped by a key, each group in the order its first item comes, the groups in that order too.
fn buckets<T, K: std::hash::Hash + Eq>(items: &[T], key: impl Fn(&T) -> K) -> Vec<Vec<&T>> {
    let mut index: HashMap<K, usize> = HashMap::new();
    let mut out: Vec<Vec<&T>> = Vec::new();
    for item in items {
        let i = *index.entry(key(item)).or_insert_with(|| {
            out.push(Vec::new());
            out.len() - 1
        });
        if let Some(b) = out.get_mut(i) {
            b.push(item);
        }
    }
    out
}

/// What's left of `name` after `prefix`, letter case aside, or none when it doesn't start with it.
fn after_prefix<'a>(name: &'a str, prefix: &str) -> Option<&'a str> {
    let mut units = 0;
    let target = prefix.encode_utf16().count();
    let mut at = 0;
    for (i, c) in name.char_indices() {
        if units >= target {
            break;
        }
        units += c.len_utf16();
        at = i + c.len_utf8();
    }
    let (head, rest) = (name.get(..at)?, name.get(at..)?);
    (units == target && case::eq_ignore_case(head, prefix)).then_some(rest)
}

/// A song's name with only its letters, lower case: "Track nd" is "tracknd" (as UTF-16 units, which .NET counts). As
/// in .NET, which looks at one UTF-16 unit at a time, a letter beyond the first 65,536 characters doesn't count.
fn skeleton(song: &str) -> Vec<u16> {
    let letters: String = LETTER
        .find_iter(song)
        .flat_map(|m| m.as_str().chars())
        .filter(|c| c.len_utf16() == 1)
        .map(case::lower)
        .collect();
    letters.encode_utf16().collect()
}

/// Why two names (as skeletons) may be one song, or none when they're plainly different or the same.
fn close(a: &[u16], b: &[u16]) -> Option<String> {
    if a == b {
        // The same name, numbered differently: the rules already decided.
        return None;
    }
    let (shorter, longer) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let text = |u: &[u16]| String::from_utf16_lossy(u);
    let extra = longer.len() - shorter.len();
    if longer.starts_with(shorter) && extra <= 3 {
        return Some(format!("The names differ only by \"{}\"", text(longer.get(shorter.len()..)?)));
    }
    if longer.ends_with(shorter) && extra <= 3 {
        return Some(format!("The names differ only by \"{}\"", text(longer.get(..extra)?)));
    }
    let most = if longer.len() >= 8 { 2 } else { 1 };
    if shorter.len() >= 4 && extra <= most {
        let d = distance(a, b, most);
        if d <= most {
            return Some(
                if d == 1 { "The names are one letter apart" } else { "The names are two letters apart" }.into(),
            );
        }
    }
    None
}

/// How many letters to change, add or remove to turn one into the other, up to `most` + 1.
fn distance(a: &[u16], b: &[u16], most: usize) -> usize {
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        let i = i + 1;
        if let Some(c) = current.first_mut() {
            *c = i;
        }
        let mut best = i;
        for (j, &cb) in b.iter().enumerate() {
            let left = current.get(j).copied().unwrap_or(usize::MAX - 1) + 1;
            let up = previous.get(j + 1).copied().unwrap_or(usize::MAX - 1) + 1;
            let diag = previous.get(j).copied().unwrap_or(usize::MAX - 1) + usize::from(ca != cb);
            let v = left.min(up).min(diag);
            if let Some(c) = current.get_mut(j + 1) {
                *c = v;
            }
            best = best.min(v);
        }
        if best > most {
            return most + 1;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous.last().copied().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_builds() {
        let all = [
            &NUMBERED,
            &SPACE_NUMBERED,
            &GLUED_ON,
            &FIXED_TAIL,
            &MARKED_NUMBER,
            &GLUED_NUMBER,
            &FINAL,
            &SIDE_SAVE,
            &VERSION_TAIL,
            &LETTER,
        ];
        for p in all {
            assert!(p.0.is_some());
        }
    }
}
