//! Lines up cloned patterns in an old FL Studio project: each clone goes on its source pattern's playlist track.
//!
//! FL Studio 8 and older (and many FL 9 saves) gave every pattern a row of its own in the playlist, so a song opened
//! in a newer FL gets one track per pattern, and a pattern's clones end up on tracks of their own. This writes
//! "<name> (fixed).flp" beside the project with the clones moved onto their source's track. The original is never
//! changed (rule 3).
//!
//! FL keeps no link from a clone to its source, only the name it gives the clone (checked on 21,936 of Solomon's
//! saves, 2026-11-02): a clone of "bass" is "bass #2", then "bass #3" and on; a clone of a pattern that was never named
//! is "Pattern 3 #2", where the 3 is the source's number when it was cloned (it goes stale as patterns are added before
//! it). So:
//! - "X #N" goes with the pattern named "X" (the nearest one before it).
//! - "Pattern N #M" goes with the nearest unnamed pattern before it, if they share a channel.
//! - A pattern without a "#" name stays where it is, even when another uses the same channels: "chords main" and
//!   "chords" on the same two channels aren't clones.
//! - A clone that shares no channel with its source stays too (FL's Split by channel names its patterns "X #N" as well).
//!
//! A clip only moves when nothing on its source's track plays at the same time; one that would overlap is listed and
//! stays where it is. Only the track of each clip changes: two bytes of each moved clip, so the file is otherwise the
//! same, byte for byte.
//!
//! In a save that keeps patterns on rows of their own (FL 8 and older, many FL 9 saves, some FL 11.1), each row
//! becomes a pattern clip on a track, as FL itself does when it opens one (seen in Solomon's FL 8 "midnight_33" and
//! its FL 11 re-save): the clip keeps its place, length and everything else; a pattern and its clones share a track,
//! the others get one each, in pattern order below the tracks already used. Only those bytes and the part played (set to "all of it", as FL does) change.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use mo_formats::fl::events::{self, EV_PATTERN_NAME, EV_PATTERN_NEW, EV_PATTERN_NOTES, EV_PLAYLIST, Flp};
use mo_formats::fl::{notes, song};
use mo_formats::paths;

use crate::save::save_checked_new;

/// Where FL's pattern numbers start in a playlist clip's index.
const PATTERN_BASE: u16 = 20480;

/// A pattern in the project.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pattern {
    pub number: u32,
    /// "" when the pattern was never named.
    pub name: String,
    /// The channels its notes are on.
    pub channels: BTreeSet<u32>,
}

impl Pattern {
    /// The name FL shows: "Pattern 3" for one never named.
    pub fn shown(&self) -> String {
        if self.name.is_empty() { format!("Pattern {}", self.number) } else { self.name.clone() }
    }
}

/// A pattern and its clones, all to go on one track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub source: u32,
    pub clones: Vec<u32>,
}

/// A clip moved to its source's track. Tracks count from 1 at the top, as FL shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Move {
    /// Which playlist (arrangement) it's in, from 0.
    pub arrangement: usize,
    pub pattern: u32,
    /// Where it starts, in ticks.
    pub at: u32,
    /// 0 for a pattern that was on a row of its own (an old save).
    pub from_track: u32,
    pub to_track: u32,
}

/// What lining up a project's clones changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lineup {
    pub patterns: Vec<Pattern>,
    pub groups: Vec<Group>,
    pub moves: Vec<Move>,
    /// What stays as it is, and why, in plain words.
    pub left: Vec<String>,
    /// The file's bytes to change: where, and the byte written there.
    patches: Vec<(usize, u8)>,
    /// The playlist had patterns on rows of their own (FL 8 and older, many FL 9 saves, some FL 11.1): each becomes a
    /// pattern clip on a track, as FL does when it opens such a save, with clones on their source's track.
    pub rows: bool,
}

impl Lineup {
    /// The project's bytes with the clones lined up.
    pub fn apply(&self, raw: &[u8]) -> Vec<u8> {
        let mut out = raw.to_vec();
        for &(at, byte) in &self.patches {
            if let Some(b) = out.get_mut(at) {
                *b = byte;
            }
        }
        out
    }

    /// A pattern's name as FL shows it.
    pub fn name_of(&self, p: u32) -> String {
        self.patterns.iter().find(|q| q.number == p).map_or_else(|| format!("Pattern {p}"), Pattern::shown)
    }

    /// One line for each pattern whose clips move: "bass #2: 3 clips from track 7 to track 2" ("from its own row" for
    /// an old save's row).
    pub fn summary(&self) -> Vec<String> {
        let name = |p: u32| self.name_of(p);
        let mut by: BTreeMap<(u32, u32, u32), usize> = BTreeMap::new();
        for m in &self.moves {
            *by.entry((m.pattern, m.from_track, m.to_track)).or_default() += 1;
        }
        by.into_iter()
            .map(|((p, from, to), n)| {
                let clips = if n == 1 { "1 clip".to_owned() } else { format!("{n} clips") };
                let from = if from == 0 { "its own row".to_owned() } else { format!("track {from}") };
                format!("{}: {clips} from {from} to track {to}", name(p))
            })
            .collect()
    }
}

/// The name a clone was made from: "bass" for "bass #3"; None when the name doesn't end in " #N".
pub fn clone_base(name: &str) -> Option<&str> {
    let (base, n) = name.rsplit_once(" #")?;
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && !base.is_empty()).then_some(base)
}

/// Whether a name is the one FL gives a pattern never named: "Pattern 3".
fn default_name(name: &str) -> bool {
    name.strip_prefix("Pattern ").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The project's patterns: names and the channels their notes are on.
fn patterns(flp: &Flp<'_>) -> Vec<Pattern> {
    let mut by: BTreeMap<u32, Pattern> = BTreeMap::new();
    let mut current = None;
    for e in &flp.events {
        match e.id {
            EV_PATTERN_NEW => {
                let p = e.int();
                current = Some(p);
                by.entry(p).or_insert_with(|| Pattern { number: p, ..Pattern::default() });
            }
            EV_PATTERN_NAME => {
                if let Some(p) = current.and_then(|p| by.get_mut(&p)) {
                    p.name = flp.text(&e.data);
                }
            }
            EV_PATTERN_NOTES => {
                if let Some(p) = current.and_then(|p| by.get_mut(&p)) {
                    let size = notes::note_size(&e.data, (flp.major, flp.minor), e.offset.is_none(), None);
                    p.channels.extend(notes::decode(&e.data, size).into_iter().map(|(c, _)| c));
                }
            }
            _ => {}
        }
    }
    by.into_values().collect()
}

/// Whether two patterns share a channel, or neither has notes (automation only, or empty).
fn related(a: &Pattern, b: &Pattern) -> bool {
    (a.channels.is_empty() && b.channels.is_empty()) || !a.channels.is_disjoint(&b.channels)
}

/// Each clone's source, as groups. What can't be matched is said in `left`.
fn groups(patterns: &[Pattern], left: &mut Vec<String>) -> Vec<Group> {
    let mut source_of: BTreeMap<u32, u32> = BTreeMap::new();
    for c in patterns {
        let Some(base) = clone_base(&c.name) else { continue };
        let earlier = patterns.iter().filter(|p| p.number < c.number).rev();
        let source = match patterns.iter().filter(|p| p.name == base).min_by_key(|p| p.number.abs_diff(c.number)) {
            // The nearest one before it, else the nearest after.
            Some(any) => patterns.iter().rfind(|p| p.name == base && p.number < c.number).or(Some(any)),
            // A clone of a pattern never named: the nearest such before it, if they share a channel.
            None if default_name(base) => {
                let found = earlier.clone().find(|p| p.name.is_empty() || default_name(&p.name));
                // Else an earlier clone of the same "Pattern N" that shares a channel: FL renumbered the source, or
                // there's none left, but the clones still go together.
                let sibling = || {
                    patterns.iter().find(|p| p.number < c.number && clone_base(&p.name) == Some(base) && related(p, c))
                };
                match (found, sibling()) {
                    (Some(p), _) if related(p, c) => Some(p),
                    (_, Some(s)) => Some(s),
                    (Some(p), None) => {
                        left.push(format!(
                            "{} stays: the unnamed pattern before it ({}) has none of its channels",
                            c.shown(),
                            p.shown()
                        ));
                        None
                    }
                    (None, None) => {
                        left.push(format!("{} stays: no unnamed pattern comes before it", c.shown()));
                        None
                    }
                }
            }
            None => {
                left.push(format!("{} stays: there's no pattern called \"{base}\"", c.shown()));
                None
            }
        };
        let Some(source) = source else { continue };
        if !source.channels.is_empty() && !c.channels.is_empty() && source.channels.is_disjoint(&c.channels) {
            left.push(format!(
                "{} stays: it has none of {}'s channels (maybe made by Split by channel, not a clone)",
                c.shown(),
                source.shown()
            ));
            continue;
        }
        source_of.insert(c.number, source.number);
    }
    // A clone of a clone joins the first one's group.
    let root = |mut p: u32| {
        for _ in 0..patterns.len() {
            match source_of.get(&p) {
                Some(&s) => p = s,
                None => break,
            }
        }
        p
    };
    let mut by: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &c in source_of.keys() {
        let r = root(c);
        if r != c {
            by.entry(r).or_default().push(c);
        }
    }
    by.into_iter().map(|(source, clones)| Group { source, clones }).collect()
}

/// The number at `at` in a playlist record (records are always long enough).
fn u16_at(r: &[u8], at: usize) -> u16 {
    r.get(at..at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
}

/// The number at `at` in a playlist record.
fn u32_at(r: &[u8], at: usize) -> u32 {
    r.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// FL's version as numbers: (8, 5) for "8.5.0".
fn version(v: &str) -> (u32, u32) {
    let mut parts = v.split('.').map(|p| p.trim().parse::<u32>().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

/// What a playlist record holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A pattern on a row of its own (FL 11.1 and older): no track.
    Row,
    /// A pattern clip on a track.
    Pattern,
    /// A channel's clip (audio or automation) on a track.
    Channel,
}

/// One clip in a playlist.
struct Record {
    /// Where the record starts in the file.
    offset: usize,
    kind: Kind,
    pattern: u32,
    at: u32,
    end: u32,
    /// The track slot, counted down from the first track; None for a row.
    slot: Option<u16>,
}

/// The sizes a playlist record may have, most likely first, by FL's version (checked on Solomon's saves, 2026-11-02):
/// 28 bytes to FL 8.0, 32 from FL 8.5 to 20, 60 from FL 20.99 (21's beta) to 24, 80 to 25, 88 after. A length can
/// divide by more than one (480 bytes is 8 of 60 or 6 of 80), so the version's goes first and each is tried.
fn record_sizes(v: (u32, u32)) -> &'static [usize] {
    if v < (8, 5) {
        &[28]
    } else if v < (20, 99) {
        &[32]
    } else if v < (24, 2) {
        &[60, 80, 88]
    } else if v < (25, 2) {
        &[80, 60, 88]
    } else {
        &[88, 80, 60]
    }
}

/// The records of a playlist. A row is 0 then 0x5000 or 0x6000 where a clip's pattern base and index go, on slot
/// 999 − pattern; a clip has the base (0x5000) then the base + pattern, or the channel. In 28-byte records (FL 8 and
/// older) the slot is byte 12 with byte 14 above it (byte 13 is always 0x78).
fn records(data: &[u8], offset: usize, size: usize) -> Option<Vec<Record>> {
    let slot_of = |r: &[u8]| if size == 28 { u16::from(r[12]) | (u16::from(r[14]) << 8) } else { u16_at(r, 12) };
    let mut out = Vec::new();
    for (i, r) in data.chunks_exact(size).enumerate() {
        let (base, index, at, length) = (u16_at(r, 4), u16_at(r, 6), u32_at(r, 0), u32_at(r, 8));
        let slot = slot_of(r);
        let (kind, pattern, slot) = match (base, index) {
            (0, 0x5000 | 0x6000) if (1..=998).contains(&slot) => (Kind::Row, 999 - slot as u32, None),
            (PATTERN_BASE, i) if i > PATTERN_BASE => (Kind::Pattern, (i - PATTERN_BASE) as u32, Some(slot)),
            (PATTERN_BASE, _) => (Kind::Channel, 0, Some(slot)),
            // A layout this doesn't know: change nothing.
            _ => return None,
        };
        // A clip with no length is kept: a row of one still has to become a clip.
        out.push(Record { offset: offset + i * size, kind, pattern, at, end: at.saturating_add(length), slot });
    }
    Some(out)
}

/// What FL saves for the first playlist track in a project of this version: FL 10 and older have 99 tracks, FL 11 and
/// 12 199, from FL 12.9.1 500 (row saves only come from FL 11.1 and older).
fn top_slot(flp: &Flp<'_>, highest: u16) -> u16 {
    let at_least = match version(&flp.version) {
        (..=10, _) => return 98,
        // Rows alone (no clip to go by): FL 11 and 12 count from 198.
        (11 | 12, _) if highest == 0 => 99,
        _ => highest,
    };
    song::top_slot(&flp.version, at_least as i64).clamp(0, 499) as u16
}

/// Works out which clips move where. Changes nothing.
pub fn line_up(raw: &[u8]) -> Result<Lineup, String> {
    let flp = events::load(raw).map_err(|e| e.to_string())?;
    if flp.warning.is_some() {
        return Err("This save is damaged or cut short, so it isn't changed.".to_owned());
    }
    let v = version(&flp.version);
    let mut lineup = Lineup { patterns: patterns(&flp), ..Lineup::default() };
    lineup.groups = groups(&lineup.patterns, &mut lineup.left);
    let group_of: HashMap<u32, u32> = lineup
        .groups
        .iter()
        .flat_map(|g| std::iter::once(g.source).chain(g.clones.iter().copied()).map(move |p| (p, g.source)))
        .collect();
    // The pattern whose track a pattern's clips go on: its group's source, or itself.
    let unit = |p: u32| group_of.get(&p).copied().unwrap_or(p);
    let names = Lineup { patterns: lineup.patterns.clone(), ..Lineup::default() };
    let name = |p: u32| names.name_of(p);
    let mut left = Vec::new();

    for (arrangement, e) in flp.events.iter().filter(|e| e.id == EV_PLAYLIST).enumerate() {
        let Some(offset) = e.offset else { continue };
        // An empty playlist: nothing to line up.
        if e.data.is_empty() {
            continue;
        }
        let parsed = record_sizes(v).iter().find_map(|&size| {
            let fits = e.data.len().is_multiple_of(size);
            fits.then(|| records(&e.data, offset, size)).flatten().map(|r| (size, r))
        });
        let Some(records) = parsed else {
            return Err("Its playlist is saved in a way this doesn't know, so it isn't changed.".to_owned());
        };
        let (size, records) = records;
        let rows = records.iter().any(|r| r.kind == Kind::Row);
        lineup.rows |= rows;
        let highest = records.iter().filter_map(|r| r.slot).max().unwrap_or(0);
        let top = top_slot(&flp, highest);
        let track = |slot: u16| u32::from(top.saturating_sub(slot)) + 1;

        // Each group's track: where its source is most often, else where its first placed clone is. Ties go to the
        // higher track (the bigger slot).
        let mut count: BTreeMap<u32, BTreeMap<u16, usize>> = BTreeMap::new();
        for r in records.iter().filter(|r| r.kind == Kind::Pattern) {
            *count.entry(r.pattern).or_default().entry(r.slot.unwrap_or_default()).or_default() += 1;
        }
        let most = |p: u32| count.get(&p)?.iter().max_by_key(|&(&slot, &n)| (n, slot)).map(|(&slot, _)| slot);
        let mut target: BTreeMap<u32, u16> = BTreeMap::new();
        for g in &lineup.groups {
            let mut members: Vec<u32> = std::iter::once(g.source).chain(g.clones.iter().copied()).collect();
            members.sort_unstable();
            if let Some(slot) = most(g.source).or_else(|| members.iter().find_map(|&p| most(p))) {
                target.insert(g.source, slot);
            }
        }

        // What's on each track: every clip where it is now, until it moves away.
        let moving = |r: &Record| match r.kind {
            Kind::Row => true,
            Kind::Pattern => target.get(&unit(r.pattern)).is_some_and(|&t| Some(t) != r.slot),
            Kind::Channel => false,
        };
        let mut taken: BTreeMap<u16, Vec<(u32, u32)>> = BTreeMap::new();
        for r in records.iter() {
            if let Some(slot) = r.slot {
                taken.entry(slot).or_default().push((r.at, r.end));
            }
        }
        // New tracks for rows, as FL gives them when it opens such a save: in pattern order, below the tracks in use.
        let mut next = taken.keys().next().map_or(Some(top), |&s| s.checked_sub(1));
        let mut new_track = |taken: &BTreeMap<u16, Vec<(u32, u32)>>| -> Result<u16, String> {
            while let Some(s) = next {
                next = s.checked_sub(1);
                if !taken.contains_key(&s) {
                    return Ok(s);
                }
            }
            Err(format!(
                "Nothing was written: its patterns need more than the {} playlist tracks FL {}.{} has.",
                top + 1,
                v.0,
                v.1
            ))
        };
        // Extra tracks a group's clips spill onto when they play at the same time as each other (rows only).
        let mut spill: BTreeMap<u32, Vec<u16>> = BTreeMap::new();

        let mut order: Vec<&Record> = records.iter().filter(|r| moving(r)).collect();
        order.sort_by_key(|r| (unit(r.pattern), r.at, r.pattern));
        for r in order {
            let u = unit(r.pattern);
            let free = |slot: u16, taken: &BTreeMap<u16, Vec<(u32, u32)>>| {
                taken.get(&slot).is_none_or(|on| !on.iter().any(|&(a, b)| r.at < b && a < r.end))
            };
            let to = match (r.kind, target.get(&u).copied()) {
                (Kind::Pattern, Some(to)) if free(to, &taken) => to,
                (Kind::Pattern, Some(to)) => {
                    left.push(format!(
                        "{} at tick {} stays on track {}: something on track {} plays at the same time",
                        name(r.pattern),
                        r.at,
                        track(r.slot.unwrap_or_default()),
                        track(to)
                    ));
                    continue;
                }
                (_, Some(to)) if free(to, &taken) => to,
                (_, first) => {
                    let to = match first {
                        None => {
                            let to = new_track(&taken)?;
                            target.insert(u, to);
                            to
                        }
                        Some(_) => match spill.get(&u).and_then(|s| s.iter().copied().find(|&s| free(s, &taken))) {
                            Some(to) => to,
                            None => {
                                let to = new_track(&taken)?;
                                spill.entry(u).or_default().push(to);
                                to
                            }
                        },
                    };
                    if first.is_some() {
                        left.push(format!(
                            "{} at tick {} plays at the same time as another of its group, so it goes on track {}",
                            name(r.pattern),
                            r.at,
                            track(to)
                        ));
                    }
                    to
                }
            };
            if let Some(on) = r.slot.and_then(|from| taken.get_mut(&from))
                && let Some(i) = on.iter().position(|&span| span == (r.at, r.end))
            {
                on.swap_remove(i);
            }
            taken.entry(to).or_default().push((r.at, r.end));
            let [lo, hi] = to.to_le_bytes();
            if r.kind == Kind::Row {
                // Made a pattern clip, as FL does when it opens such a save: base, then base + pattern.
                let index = PATTERN_BASE + r.pattern as u16;
                lineup.patches.extend([(r.offset + 4, 0x00), (r.offset + 5, 0x50)]);
                lineup.patches.extend(index.to_le_bytes().into_iter().enumerate().map(|(i, b)| (r.offset + 6 + i, b)));
                // The part played: "all of it" (-1, -1), as FL writes it there. Rows say so as two -1.0 floats or
                // as -1 and 0x7FFFFFFF; no pattern clip in Solomon's saves has the second.
                let tail = size - 8;
                lineup.patches.extend((0..8).map(|i| (r.offset + tail + i, 0xFF)));
            }
            if size == 28 {
                lineup.patches.extend([(r.offset + 12, lo), (r.offset + 14, hi)]);
            } else {
                lineup.patches.extend([(r.offset + 12, lo), (r.offset + 13, hi)]);
            }
            lineup.moves.push(Move {
                arrangement,
                pattern: r.pattern,
                at: r.at,
                from_track: r.slot.map_or(0, track),
                to_track: track(to),
            });
        }
    }
    lineup.left.extend(left);
    Ok(lineup)
}

/// A "(fixed)" project with its clones lined up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fixed {
    pub file: PathBuf,
    pub lineup: Lineup,
}

/// Where the fixed project goes: "<name> (fixed).flp" beside it, or "(fixed 2)" and on while that's taken.
fn fixed_names(project: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    let stem = paths::stem(&project.to_string_lossy()).to_owned();
    (1..1000).map(move |n| {
        let mark = if n == 1 { "fixed".to_owned() } else { format!("fixed {n}") };
        project.with_file_name(format!("{stem} ({mark}).flp"))
    })
}

/// Writes "<name> (fixed).flp" beside `project` with its clones on their sources' tracks (copy, check, then rename;
/// never over a file that's there). The project itself isn't changed.
pub fn fix_fl_project(project: &Path) -> Result<Fixed, String> {
    let raw = std::fs::read(project).map_err(|e| format!("Couldn't read {}: {e}", project.display()))?;
    let lineup = line_up(&raw)?;
    if lineup.moves.is_empty() {
        let why = if lineup.groups.is_empty() {
            "no pattern in it is a clone (\"name #2\")".to_owned()
        } else if lineup.left.iter().any(|l| l.contains(" at tick ")) {
            format!("no clone could move without playing over something: {}", lineup.left.join("; "))
        } else {
            "its clones are already on their sources' tracks".to_owned()
        };
        return Err(format!("Nothing was written: {why}."));
    }
    let bytes = lineup.apply(&raw);
    // The copy must read back as the same project with every pattern on a track. (A second line-up may still find
    // a clip to move, one that waited for another to make room.)
    let events = events::load(&raw).map(|f| f.events.len()).map_err(|e| e.to_string())?;
    let problem = |back: &[u8]| match events::load(back) {
        Ok(b) if b.events.len() == events && b.warning.is_none() => match line_up(back) {
            Ok(again) if !again.rows => None,
            Ok(_) => Some("some patterns are still on rows".to_owned()),
            Err(e) => Some(e),
        },
        _ => Some("it doesn't read as the same FL project".to_owned()),
    };
    for name in fixed_names(project) {
        if save_checked_new(&name, &bytes, problem)? {
            return Ok(Fixed { file: name, lineup });
        }
    }
    Err("every \"(fixed …)\" name up to 999 is taken, so nothing was written".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clone_name_gives_its_source() {
        assert_eq!(clone_base("bass #2"), Some("bass"));
        assert_eq!(clone_base("Pattern 3 #12"), Some("Pattern 3"));
        assert_eq!(clone_base("bass 2"), None);
        assert_eq!(clone_base("bass #"), None);
        assert_eq!(clone_base("#2"), None);
        assert_eq!(clone_base("hit #2b"), None);
        assert!(default_name("Pattern 12"));
        assert!(!default_name("Pattern"));
        assert!(!default_name("Patterns 1"));
    }
}
