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
    /// The file's bytes to change: where, and the track slot written there.
    patches: Vec<(usize, u16)>,
    /// The playlist is saved one row per pattern (FL 8 and older, many FL 9 saves): its tracks can't be changed.
    pub rows: bool,
}

impl Lineup {
    /// The project's bytes with the clones lined up.
    pub fn apply(&self, raw: &[u8]) -> Vec<u8> {
        let mut out = raw.to_vec();
        for &(at, slot) in &self.patches {
            if let Some(b) = out.get_mut(at..at + 2) {
                b.copy_from_slice(&slot.to_le_bytes());
            }
        }
        out
    }

    /// One line for each pattern whose clips move: "bass #2: 3 clips from track 7 to track 2".
    pub fn summary(&self) -> Vec<String> {
        let name = |p: u32| {
            self.patterns.iter().find(|q| q.number == p).map_or_else(|| format!("Pattern {p}"), Pattern::shown)
        };
        let mut by: BTreeMap<(u32, u32, u32), usize> = BTreeMap::new();
        for m in &self.moves {
            *by.entry((m.pattern, m.from_track, m.to_track)).or_default() += 1;
        }
        by.into_iter()
            .map(|((p, from, to), n)| {
                let clips = if n == 1 { "1 clip".to_owned() } else { format!("{n} clips") };
                format!("{}: {clips} from track {from} to track {to}", name(p))
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
                    let size = notes::note_size(&e.data, flp.major, e.offset.is_none(), None);
                    p.channels.extend(notes::decode(&e.data, size).into_iter().map(|(c, _)| c));
                }
            }
            _ => {}
        }
    }
    by.into_values().collect()
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
                match found {
                    Some(p) if !p.channels.is_disjoint(&c.channels) => Some(p),
                    Some(p) => {
                        left.push(format!(
                            "{} stays: the unnamed pattern before it ({}) has none of its channels",
                            c.shown(),
                            p.shown()
                        ));
                        None
                    }
                    None => {
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

/// A pattern clip in a playlist.
struct Clip {
    /// Where its track slot is in the file.
    slot_at: usize,
    pattern: u32,
    at: u32,
    end: u32,
    slot: u16,
}

/// The clips in a playlist saved with tracks (FL 8.5 and later): None when it's saved one row per pattern (or in a
/// layout this doesn't know).
fn clips(data: &[u8], offset: usize) -> Option<Vec<Clip>> {
    let size = [88, 80, 60, 32].into_iter().find(|&s| {
        !data.is_empty() && data.len().is_multiple_of(s) && data.chunks_exact(s).all(|r| u16_at(r, 4) == PATTERN_BASE)
    })?;
    Some(
        data.chunks_exact(size)
            .enumerate()
            .filter(|(_, r)| u16_at(r, 6) > PATTERN_BASE && u32_at(r, 8) > 0)
            .map(|(i, r)| Clip {
                slot_at: offset + i * size + 12,
                pattern: (u16_at(r, 6) - PATTERN_BASE) as u32,
                at: u32_at(r, 0),
                end: u32_at(r, 0).saturating_add(u32_at(r, 8)),
                slot: u16_at(r, 12),
            })
            .collect(),
    )
}

/// Every channel or audio clip's slot and time in a playlist with tracks, so a moved clone doesn't land on one.
fn other_clips(data: &[u8]) -> Vec<(u16, u32, u32)> {
    let Some(size) = [88, 80, 60, 32].into_iter().find(|&s| !data.is_empty() && data.len().is_multiple_of(s)) else {
        return Vec::new();
    };
    data.chunks_exact(size)
        .filter(|r| u16_at(r, 6) < PATTERN_BASE && u32_at(r, 8) > 0)
        .map(|r| (u16_at(r, 12), u32_at(r, 0), u32_at(r, 0).saturating_add(u32_at(r, 8))))
        .collect()
}

/// Works out which clips move where. Changes nothing.
pub fn line_up(raw: &[u8]) -> Result<Lineup, String> {
    let flp = events::load(raw).map_err(|e| e.to_string())?;
    if flp.warning.is_some() {
        return Err("This save is damaged or cut short, so it isn't changed.".to_owned());
    }
    let mut lineup = Lineup { patterns: patterns(&flp), ..Lineup::default() };
    lineup.groups = groups(&lineup.patterns, &mut lineup.left);
    let playlists: Vec<_> = flp.events.iter().filter(|e| e.id == EV_PLAYLIST).collect();
    let group_of: HashMap<u32, usize> = lineup
        .groups
        .iter()
        .enumerate()
        .flat_map(|(g, group)| std::iter::once(group.source).chain(group.clones.iter().copied()).map(move |p| (p, g)))
        .collect();

    for (arrangement, e) in playlists.iter().enumerate() {
        let Some(offset) = e.offset else { continue };
        let Some(clips) = clips(&e.data, offset) else {
            lineup.rows = true;
            continue;
        };
        let others = other_clips(&e.data);
        let highest = clips.iter().map(|c| c.slot).chain(others.iter().map(|c| c.0)).max().unwrap_or(0);
        let top = song::top_slot(&flp.version, highest as i64);
        let track = |slot: u16| u32::try_from(top - slot as i64 + 1).unwrap_or(0);

        // Each group's track: where its source is most often, else where its first placed clone is.
        let mut target: HashMap<usize, u16> = HashMap::new();
        for (g, group) in lineup.groups.iter().enumerate() {
            let most = |p: u32| {
                let mut count: BTreeMap<u16, usize> = BTreeMap::new();
                for c in clips.iter().filter(|c| c.pattern == p) {
                    *count.entry(c.slot).or_default() += 1;
                }
                // Ties go to the higher track (the bigger slot).
                count.into_iter().max_by_key(|&(slot, n)| (n, slot)).map(|(slot, _)| slot)
            };
            let mut members = std::iter::once(group.source).chain(group.clones.iter().copied()).collect::<Vec<_>>();
            members.sort_unstable();
            if let Some(slot) = most(group.source).or_else(|| members.iter().find_map(|&p| most(p))) {
                target.insert(g, slot);
            }
        }

        // What's on each track and stays.
        let moving = |c: &Clip| group_of.get(&c.pattern).and_then(|g| target.get(g)).is_some_and(|&t| t != c.slot);
        let mut taken: HashMap<u16, Vec<(u32, u32)>> = HashMap::new();
        for c in clips.iter().filter(|c| !moving(c)) {
            taken.entry(c.slot).or_default().push((c.at, c.end));
        }
        for &(slot, at, end) in &others {
            taken.entry(slot).or_default().push((at, end));
        }
        let mut order: Vec<&Clip> = clips.iter().filter(|c| moving(c)).collect();
        order.sort_by_key(|c| (c.at, c.pattern));
        for c in order {
            let Some(&to) = group_of.get(&c.pattern).and_then(|g| target.get(g)) else { continue };
            let on = taken.entry(to).or_default();
            if on.iter().any(|&(a, b)| c.at < b && a < c.end) {
                let name = lineup.patterns.iter().find(|p| p.number == c.pattern).map(Pattern::shown);
                lineup.left.push(format!(
                    "{} at tick {} stays on track {}: something on track {} plays at the same time",
                    name.unwrap_or_else(|| format!("Pattern {}", c.pattern)),
                    c.at,
                    track(c.slot),
                    track(to)
                ));
                continue;
            }
            on.push((c.at, c.end));
            lineup.patches.push((c.slot_at, to));
            lineup.moves.push(Move {
                arrangement,
                pattern: c.pattern,
                at: c.at,
                from_track: track(c.slot),
                to_track: track(to),
            });
        }
    }
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
        let why = if lineup.rows {
            "this save keeps one playlist row per pattern (FL 9 and older), so its tracks can't be changed yet"
        } else if lineup.groups.is_empty() {
            "no pattern in it is a clone (\"name #2\")"
        } else {
            "its clones are already on their sources' tracks"
        };
        return Err(format!("Nothing was written: {why}."));
    }
    let bytes = lineup.apply(&raw);
    // The copy must read back as the same project, its clones lined up.
    let problem = |back: &[u8]| match (events::load(&raw), events::load(back)) {
        (Ok(a), Ok(b)) if a.events.len() == b.events.len() && b.warning.is_none() => match line_up(back) {
            Ok(again) if again.moves.is_empty() => None,
            Ok(_) => Some("some clones still aren't lined up".to_owned()),
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
