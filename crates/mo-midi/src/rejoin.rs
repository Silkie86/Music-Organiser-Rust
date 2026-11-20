//! Phrases an old FL Studio song spreads over several channels, joined back into one part.
//!
//! Old FL projects often hand a phrase from one channel to another: a lead played by one synth for 8 bars, then by
//! another for the next 8, or a one-note pickup on one channel answered on another. Each channel on its own is half a
//! phrase. Whole song sections repeat too (drums and all), so repeating alone doesn't make a phrase: two channels'
//! stretches are joined only when every clue agrees well enough, weighed together ([`score`]):
//!
//! - they come back together, the same way, at least twice (the more often, the stronger);
//! - one hands over to the other: the second starts in the bar after the first stops (or later in its last bar),
//!   and neither channel plays while the other has the phrase;
//! - they sound like one voice: their ranges meet (or their middles are near), a line hands to a line and chords to
//!   chords, and where two could take over (a lead and a pad doubling it) the one the tune steps to most smoothly does;
//! - the joined phrase is a tidy length (2, 4, 8 or 16 bars), or a piece is too short to be a part on its own.
//!
//! Drums never join (a fill answering a lead is two parts), and neither do pieces in far apart registers (a bass line
//! handing to a lead is two parts). Channels named the same with " #2" and so on that play the same sample or plugin
//! are one voice first ([`voices`]): FL users copied a channel to add hits or a doubled layer.
//!
//! Everything here is from the song as the playlist lays it out ([`fl::song::extract_channels`]), counted in the
//! song's own bars: a bar of 3/4 is three beats here, so a phrase is from the bar FL's playlist shows it in, and four
//! bars of a waltz are a tidy length. The notes of a phrase stay as every part in the library has them, in ticks from
//! the bar line it starts on (where the song's bars are longer than 4/4's and it comes in late in its first bar, from
//! whole bars of 4/4 after that line: [`anchor_of`]).

use std::collections::{HashMap, HashSet};

use mo_formats::{MidiChannel, NoteEvent, case};

use crate::analyser;
use crate::member::PhraseMember;
use crate::roles::Role;
use crate::{BAR, PPQ};

/// What joining makes of the same song: raised when that changes, so that FL songs (the only ones joined) are read
/// again once, and nothing else is ([`crate::extract::project_version`]; rule 7). 2: a hand-over too long to join is
/// no way on (its first stretch was listed again, as "joined" with one piece, and its second could start no phrase).
/// 3 (the 19 November 2026 review's MP-7 and MP-8): two layers that hand over to the same synth join the same way
/// every time (which one won was left to chance each time the phrase came back, so the phrase was often lost); a
/// hand-over two layers play in unison counts once each time it comes back; and bars are the song's own (a song in
/// 3/4 or 6/8 was counted in bars of 4/4: the bar a phrase is from was wrong, and fewer phrases joined).
pub const VERSION: u32 = 3;

/// How many times two stretches must come back together, the same way, to be one phrase.
pub const MIN_REPEATS: usize = 2;
/// The score a pair of stretches needs to be joined ([`score`]).
pub const JOIN_SCORE: i32 = 6;
/// The most a phrase is joined to, in the song's bars: longer, it's a section of the song, not a phrase. Nor is it
/// longer than as many bars of 4/4, where the song's bars are longer than those: the index would cut it into pieces
/// again ([`analyser::MAX_BARS`], [`fits`]).
pub const MAX_JOINED_BARS: i64 = analyser::MAX_BARS;
/// How far apart, in semitones, the middles of two pieces may be and still be one voice.
pub const MAX_REGISTER_GAP: f64 = 12.0;
/// The most pieces one phrase is joined from: more, and it's a section passed back and forth, not a phrase.
pub const MAX_PIECES: usize = 4;

/// One voice of the song: a channel, with its " #2" copies that play the same sample or plugin folded in, its notes at
/// [`PPQ`] where the playlist puts them.
#[derive(Clone, Debug, PartialEq)]
pub struct Voice {
    /// The first channel's name ("SNAP_34" for SNAP_34 and SNAP_34 #2).
    pub name: String,
    /// Its name as its copies share it ([`stem`]).
    pub stem: String,
    pub notes: Vec<NoteEvent>,
    /// Drums, or a one-shot sample played on one or two keys: never joined to another voice.
    pub drums: bool,
    /// It plays a sample, not a plugin.
    pub sample: bool,
}

/// A phrase joined from several voices' stretches: the names joined ("LegacyCell #3 + M1"), its notes from its first
/// bar, where it first plays in the song (as one of the song's own bars, from 0), how many times it comes back, and
/// its pieces.
#[derive(Clone, Debug, PartialEq)]
pub struct Joined {
    pub name: String,
    pub notes: Vec<NoteEvent>,
    pub first_bar: i64,
    pub repeats: usize,
    pub members: Vec<PhraseMember>,
}

/// A run of one voice's notes with no empty bar in it, in the song's own bars from its start (`to` is the bar after
/// its last).
#[derive(Clone, Debug)]
struct Stretch {
    voice: usize,
    from: i64,
    to: i64,
    /// Ticks: its first note's start, and where its last note ends.
    first: i64,
    last_end: i64,
    /// The tick its notes are counted from where it lists on its own ([`anchor_of`]).
    anchor: i64,
    /// The notes moved to start in its first bar (from `anchor`), and their fingerprint: a stretch that comes back
    /// the same way.
    hash: String,
    notes: Vec<NoteEvent>,
    middle: f64,
    /// How many of its notes sound together with another (0 to 1), and whether that's most: chords, not a line.
    together: f32,
    chordal: bool,
    /// The top note it starts on.
    first_key: u8,
    lowest: u8,
    highest: u8,
}

/// The name a channel copy shares with the channel it was copied from: "SNAP_34 #2" is "snap_34".
fn stem(name: &str) -> String {
    let trimmed = name.trim();
    let base = match trimmed.rsplit_once(" #") {
        Some((base, n)) if !base.is_empty() && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => base,
        _ => trimmed,
    };
    case::to_lower(base.trim())
}

/// The song's voices: each channel with notes, with its copies named "#2" and so on folded in when they play the same
/// sample (a copy made to add hits). Two plugin channels are two voices, even of one plugin (each has its own sound),
/// unless a copy plays exactly the same notes (a doubled layer): then it's kept once.
pub fn voices(channels: &[MidiChannel]) -> Vec<Voice> {
    let mut out: Vec<Voice> = Vec::new();
    let mut by_sample: HashMap<(String, String), usize> = HashMap::new();
    for c in channels.iter().filter(|c| !c.notes.is_empty()) {
        let stem = stem(&c.name);
        let mut notes = analyser::normalise(&c.notes, i64::from(c.ppq), true);
        notes.sort_by_key(|n| (n.start, n.key, n.length));
        let drums = crate::extract::is_unpitched_plugin(&c.plugin)
            || crate::extract::is_unpitched_plugin(&c.name)
            || analyser::roles_named(&format!("{} {}", c.name, c.source)).contains(&Role::Drums);
        let folded = if c.sample_path.is_empty() {
            // A layer: a copy of a plugin channel playing the very same notes.
            out.iter().position(|v| v.stem == stem && same_notes(&v.notes, &notes))
        } else {
            by_sample.get(&(stem.clone(), case::to_lower(&c.sample_path))).copied()
        };
        match folded.and_then(|at| out.get_mut(at)) {
            Some(v) => {
                v.notes.extend(notes);
                v.drums |= drums;
            }
            None => {
                if !c.sample_path.is_empty() {
                    by_sample.insert((stem.clone(), case::to_lower(&c.sample_path)), out.len());
                }
                out.push(Voice { name: c.name.clone(), stem, notes, drums, sample: !c.sample_path.is_empty() });
            }
        }
    }
    for v in &mut out {
        v.notes.sort_by_key(|n| (n.start, n.key, n.length));
        v.notes.dedup_by(|a, b| a.start == b.start && a.key == b.key && a.length == b.length);
        // A one-shot: a sample (a hit, a kick, a vocal shot) played on one or two keys all song. Never half a tune. A
        // synth playing one note can be: a pickup.
        if !v.sample {
            continue;
        }
        let mut keys: Vec<u8> = v.notes.iter().map(|n| n.key).collect();
        keys.sort_unstable();
        keys.dedup();
        v.drums |= keys.len() <= 2;
    }
    out
}

/// Whether two channels play the same notes (where, which key, how long), however loud.
fn same_notes(a: &[NoteEvent], b: &[NoteEvent]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.start == y.start && x.key == y.key && x.length == y.length)
}

/// A voice's notes cut wherever a whole bar of the song (`bar` ticks) goes by with no note sounding.
fn stretches(at: usize, v: &Voice, bar: i64) -> Vec<Stretch> {
    let mut out = Vec::new();
    let mut run: Vec<NoteEvent> = Vec::new();
    let mut sounding_until = i64::MIN;
    for n in &v.notes {
        // The next note starts after a bar that has nothing in it.
        let empty_bar = sounding_until != i64::MIN && n.start.div_euclid(bar) > sounding_until.div_euclid(bar) + 1;
        if empty_bar && !run.is_empty() {
            out.push(stretch_of(at, std::mem::take(&mut run), bar));
        }
        sounding_until = sounding_until.max(n.start + n.length - 1);
        run.push(*n);
    }
    if !run.is_empty() {
        out.push(stretch_of(at, run, bar));
    }
    out
}

/// How many notes sound together with another for at least a sixteenth: chords played in by hand start a little
/// apart, but they ring together; a line's notes barely touch.
fn sounding_together(notes: &[NoteEvent]) -> usize {
    const LEAST: i64 = PPQ / 4;
    // Sorted by start: only the notes starting before this one ends can ring with it.
    notes
        .iter()
        .enumerate()
        .filter(|&(at, n)| {
            let ends = n.start + n.length;
            let before = notes.get(..at).unwrap_or_default().iter().rev().take(64);
            let after = notes.get(at + 1..).unwrap_or_default().iter().take_while(|m| m.start < ends);
            before.chain(after).any(|m| m.start.max(n.start) + LEAST <= (m.start + m.length).min(ends))
        })
        .count()
}

/// The tick a stretch's notes are counted from where it lists on its own: the song's bar line it starts on (its bar
/// `from`, of `bar` ticks). Where the song's bars are longer than 4/4's and its first note comes a bar of 4/4 or more
/// after that line, it's that many whole bars of 4/4 on, as every part in the library starts in its first bar of 4/4
/// ([`analyser::normalise`]).
fn anchor_of(first: i64, from: i64, bar: i64) -> i64 {
    let line = from * bar;
    line + (first - line).div_euclid(BAR) * BAR
}

fn stretch_of(voice: usize, notes: Vec<NoteEvent>, bar: i64) -> Stretch {
    let first = notes.first().map_or(0, |n| n.start);
    let last_end = notes.iter().map(|n| n.start + n.length).max().unwrap_or(first);
    let from = first.div_euclid(bar);
    let to = (last_end - 1).div_euclid(bar) + 1;
    let anchor = anchor_of(first, from, bar);
    let moved: Vec<NoteEvent> = notes.iter().map(|n| NoteEvent { start: n.start - anchor, ..*n }).collect();
    let middle = notes.iter().map(|n| f64::from(n.key)).sum::<f64>() / notes.len().max(1) as f64;
    let lowest = notes.iter().map(|n| n.key).min().unwrap_or(0);
    let highest = notes.iter().map(|n| n.key).max().unwrap_or(0);
    let together = sounding_together(&notes) as f32 / notes.len().max(1) as f32;
    let chordal = together > 0.5;
    let top_at = |start: i64| notes.iter().filter(|n| n.start == start).map(|n| n.key).max().unwrap_or(0);
    let first_key = top_at(first);
    Stretch {
        voice,
        from,
        to,
        first,
        last_end,
        anchor,
        hash: analyser::hash_of(&moved),
        notes: moved,
        middle,
        together,
        chordal,
        first_key,
        lowest,
        highest,
    }
}

/// Whether `b` takes over from `a`: it starts once `a`'s last note has ended, in `a`'s last bar or the bar after.
fn hands_over(a: &Stretch, b: &Stretch) -> bool {
    a.voice != b.voice && b.first >= a.last_end && (b.from == a.to || b.from + 1 == a.to)
}

/// How tight a hand-over is: the second starts within a beat of the first's end.
fn tight(a: &Stretch, b: &Stretch) -> bool {
    b.first - a.last_end <= PPQ
}

/// Whether any of one voice's stretches plays in the bars `from..to`.
fn plays_in(stretches: &[&Stretch], from: i64, to: i64) -> bool {
    stretches.iter().any(|s| s.from < to && s.to > from)
}

/// Each time one stretch hands to another the same way, as places in the song's stretches.
type Pairs = Vec<(usize, usize)>;

/// How many times a hand-over comes back: once for each place in the song it starts at. Two layers playing the same
/// notes under two names both hand over there (or are both handed to), which is the phrase once, not twice.
fn times(pairs: &[(usize, usize)], all: &[Stretch]) -> usize {
    let mut places: Vec<i64> = pairs.iter().filter_map(|&(i, _)| all.get(i)).map(|a| a.first).collect();
    places.sort_unstable();
    places.dedup();
    places.len()
}

/// The largest number both divide by.
const fn gcd(a: i64, b: i64) -> i64 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// The hash a stretch's notes have where the page lists them on their own, as one of the song's patterns, when they
/// are one: `listed` are the hashes of the song's own parts.
///
/// A pattern lists from the bar of 4/4 its first note is in, counted from the pattern's own start. In 4/4 that's the
/// bar line of the song the stretch is counted from (a clip starts on a bar line), so its hash here is the one. In
/// any other time it needn't be: a waltz's clip on the song's bar 18 starts three beats into a bar of 4/4, and a
/// pattern that rests for its first bar lists with that rest. So the stretch is tried from each beat of a bar of
/// 4/4 that a clip on one of the song's bar lines can put it on (four of them in 3/4), and the first that is one of
/// the song's parts is it. None is: it isn't a pattern on its own (two clips back to back, or a clip cut short), and
/// keeps the hash it has from the song's bar line, which links it to nothing.
fn listed_hash(s: &Stretch, bar: i64, listed: &HashSet<String>) -> String {
    let step = gcd(bar, BAR).max(1);
    if step == BAR || listed.contains(&s.hash) {
        return s.hash.clone();
    }
    // Where its first note is now, in its bar of 4/4, and where a clip `back` of the song's bars earlier puts it.
    let now = s.first - s.anchor;
    (1..BAR / step)
        .take(MOST_PLACES)
        .map(|back| (now + back * bar).rem_euclid(BAR) - now)
        .map(|by| {
            analyser::hash_of(&s.notes.iter().map(|n| NoteEvent { start: n.start + by, ..*n }).collect::<Vec<_>>())
        })
        .find(|hash| listed.contains(hash))
        .unwrap_or_else(|| s.hash.clone())
}

/// The most places in a bar of 4/4 a stretch is tried at ([`listed_hash`]): every sixteenth, which is more than any
/// time signature a song can have asks for.
const MOST_PLACES: usize = 16;

/// Whether the stretches from `first` to `last` are no more than is joined to one phrase ([`MAX_JOINED_BARS`]): of
/// the song's bars (`bar` ticks each), and of bars of 4/4 from where the phrase's notes start, so the index never
/// cuts it again.
fn fits(first: &Stretch, last: &Stretch, bar: i64) -> bool {
    last.to - first.from <= MAX_JOINED_BARS && last.to * bar - first.anchor <= MAX_JOINED_BARS * BAR
}

/// Whether two stretches' ranges overlap, or come within 3 semitones of each other.
fn ranges_meet(a: &Stretch, b: &Stretch) -> bool {
    b.lowest.saturating_sub(a.highest).max(a.lowest.saturating_sub(b.highest)) <= 3
}

/// Whether two stretches sit near enough to be one voice: their ranges meet, or their middles are within
/// [`MAX_REGISTER_GAP`] (a lead climbing on from where the last synth topped out has a higher middle).
fn near_enough(a: &Stretch, b: &Stretch) -> bool {
    ranges_meet(a, b) || (a.middle - b.middle).abs() <= MAX_REGISTER_GAP
}

/// How far the tune jumps where one stretch hands to the next, in semitones: from the first's range (a lead often ends
/// low and climbs back), 0 when the second starts inside it.
fn step(a: &Stretch, b: &Stretch) -> u8 {
    b.first_key.saturating_sub(a.highest).max(a.lowest.saturating_sub(b.first_key))
}

/// How unlike two stretches' textures are (0 to 100): a line taking over from a line is 0, a pad of chords 80 or so.
fn unlike(a: &Stretch, b: &Stretch) -> u8 {
    ((a.together - b.together).abs() * 100.0).round() as u8
}

/// How strongly two stretches that hand over, coming back together `repeats` times, read as one phrase (see the
/// module's notes). [`JOIN_SCORE`] or more joins them.
fn score(a: &Stretch, b: &Stretch, repeats: usize) -> i32 {
    let mut score = match repeats {
        0 | 1 => return 0,
        2 => 2,
        3 => 3,
        _ => 4,
    };
    if tight(a, b) {
        score += 1;
    }
    // The tune carries on: one's range meets the other's, or comes within a few semitones.
    if ranges_meet(a, b) {
        score += 1;
    }
    // A line hands to a line, chords to chords: a lead taken over by a pad of chords is two parts.
    if a.chordal == b.chordal {
        score += 1;
    }
    if matches!(b.to - a.from, 2 | 4 | 8 | 16) {
        score += 1;
    }
    // A piece too short to be a part on its own is most likely half of one.
    if a.notes.len() < 3 || b.notes.len() < 3 {
        score += 1;
    }
    score
}

/// The phrases the song spreads over several voices, each once (with how often it comes back), first in the song
/// first. Nothing is joined that doesn't hand over, come back together and sound like one voice ([`score`]).
///
/// `bar` is one of the song's own bars in ticks at [`PPQ`] ([`BAR`] in 4/4, three beats in 3/4 or 6/8): bars are
/// counted in it, so a phrase's first bar is the one FL's playlist shows (the 19 November 2026 review's MP-8: a
/// song in 3/4 was counted in bars of 4/4, so its bar 17 was called bar 13, and its 8-bar phrases, 6 bars of 4/4,
/// never got the point for a tidy length). A song that changes its time signature part way is counted in the one it
/// starts with all through, as it is wherever else it's read ([`fl::arrangement::bars_and_markers`]).
///
/// `listed` are the hashes of the song's own parts, as the page lists its patterns: a piece of a phrase that is one
/// of them is known by that hash, so the pattern's card can show the whole phrase ([`listed_hash`]).
pub fn join(voices: &[Voice], bar: i64, listed: &HashSet<String>) -> Vec<Joined> {
    let bar = bar.max(1);
    let all: Vec<Stretch> =
        voices.iter().enumerate().filter(|(_, v)| !v.drums).flat_map(|(at, v)| stretches(at, v, bar)).collect();
    // Every hand-over, by what the two stretches play: the same hand-over again is the phrase coming back.
    // Each voice's stretches, and the stretches starting in each bar: a song has thousands, so only the ones that
    // could take over are looked at.
    let mut of_voice: HashMap<usize, Vec<&Stretch>> = HashMap::new();
    let mut starting: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, s) in all.iter().enumerate() {
        of_voice.entry(s.voice).or_default().push(s);
        starting.entry(s.from).or_default().push(i);
    }
    let plays_in = |voice: usize, from: i64, to: i64| of_voice.get(&voice).is_some_and(|list| plays_in(list, from, to));
    let mut handovers: HashMap<(&str, &str), Vec<(usize, usize)>> = HashMap::new();
    for (i, a) in all.iter().enumerate() {
        let near = [a.to - 1, a.to].into_iter().filter_map(|bar| starting.get(&bar)).flatten();
        for &j in near {
            let Some(b) = all.get(j) else { continue };
            if !hands_over(a, b) || !near_enough(a, b) {
                continue;
            }
            // A true hand-over: neither voice plays while the other has the phrase.
            if plays_in(a.voice, b.from.max(a.to), b.to) || plays_in(b.voice, a.from, a.to.min(b.from)) {
                continue;
            }
            handovers.entry((a.hash.as_str(), b.hash.as_str())).or_default().push((i, j));
        }
    }
    // The links strong enough to join, from each stretch to the one it hands to.
    // The strongest first; of two as strong (a lead and the pad doubling it both taking over), the one most like the
    // first in texture, then the one the tune steps to most smoothly; then first in the song, so the same song always
    // joins the same way (a map has no order).
    let mut strong: Vec<(i32, (u8, u8), Pairs)> = handovers
        .into_values()
        .filter_map(|pairs| {
            let times = times(&pairs, &all);
            let &(i, j) = pairs.first()?;
            let (a, b) = (all.get(i)?, all.get(j)?);
            (times >= MIN_REPEATS).then(|| (score(a, b, times), (unlike(a, b), step(a, b)), pairs))
        })
        .filter(|(score, ..)| *score >= JOIN_SCORE)
        .collect();
    strong.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then_with(|| a.2.cmp(&b.2)));
    // One way on from each stretch: the strongest keeps it. `rank` is how strong that way on is, as its link's
    // place in `strong`.
    let mut next: HashMap<usize, usize> = HashMap::new();
    let mut rank: HashMap<usize, usize> = HashMap::new();
    for (place, (.., pairs)) in strong.iter().enumerate() {
        for &(i, j) in pairs {
            if let std::collections::hash_map::Entry::Vacant(way) = next.entry(i) {
                way.insert(j);
                rank.insert(i, place);
            }
        }
    }
    // A hand-over whose two stretches are more than is joined is no way on: the first stops there (alone it's one
    // channel's stretch, a part already where its patterns are read, and no phrase joined from several), and the
    // second is free to start a phrase with what it hands to. So every chain below is two pieces or more. The first
    // doesn't take its next strongest way on instead: its strongest is where the tune goes, and a tune that goes on
    // for longer than a phrase isn't joined to something else beside it (a pad that came in at the same bar).
    next.retain(|i, j| all.get(*i).zip(all.get(*j)).is_some_and(|(a, b)| fits(a, b, bar)));
    let taken: HashSet<usize> = next.values().copied().collect();
    let starts_a_chain = |i: &usize| !taken.contains(i);
    // The stretches that start a phrase, first in the song first. Two that start on the same tick (a lead and a layer
    // of it on another synth, or a harmony over it) and hand to the same stretch make two chains over the same
    // bars, and only the first of them is kept below: the one with the stronger way on, then the first of the song's
    // channels. So the same layer wins each time the phrase comes back, and its repeats count together. (The 19
    // November 2026 review's MP-7: they came in the order a map gave its keys, another each time. Each time the
    // phrase came back another layer could win, so neither came back twice, and the phrase was lost: about every
    // second reading of a song with a lead and its harmony handing to one synth.) A layer that starts even a tick
    // before the other is first, as it always was, whichever has the stronger way on: that never was left to chance.
    let mut heads: Vec<usize> = next.keys().copied().filter(starts_a_chain).collect();
    heads.sort_by_key(|&i| (all.get(i).map_or(0, |s| s.first), rank.get(&i).copied().unwrap_or(usize::MAX), i));
    let mut chains: Vec<Vec<usize>> = Vec::new();
    for head in heads {
        let mut chain = vec![head];
        let mut at = head;
        while let Some(&j) = next.get(&at) {
            let too_long = all.get(j).zip(all.get(head)).is_some_and(|(s, h)| !fits(h, s, bar));
            if chain.contains(&j) || too_long || chain.len() >= MAX_PIECES {
                break;
            }
            chain.push(j);
            at = j;
        }
        chains.push(chain);
    }
    // Where a chain plays in the song, in bars, and its voices.
    let span = |chain: &[usize]| {
        let from = chain.first().and_then(|&i| all.get(i)).map_or(0, |s| s.from);
        let to = chain.iter().filter_map(|&i| all.get(i)).map(|s| s.to).max().unwrap_or(from);
        (from, to)
    };
    // The longest first: a shorter chain over the same bars with one of its voices is a tail of it, not a phrase.
    chains.sort_by_key(|c| {
        let (from, to) = span(c);
        (std::cmp::Reverse(to - from), from)
    });
    let mut kept: Vec<Vec<usize>> = Vec::new();
    for chain in chains {
        let (from, to) = span(&chain);
        let voices_of = |c: &[usize]| c.iter().filter_map(|&i| all.get(i)).map(|s| s.voice).collect::<Vec<_>>();
        let mine = voices_of(&chain);
        let tail = kept.iter().any(|k| {
            let (f, t) = span(k);
            f < to && t > from && voices_of(k).iter().any(|v| mine.contains(v))
        });
        if !tail {
            kept.push(chain);
        }
    }
    kept.sort_by_key(|c| span(c).0);
    let mut out: Vec<Joined> = Vec::new();
    let mut by_hash: HashMap<String, usize> = HashMap::new();
    for chain in kept {
        let Some(first) = chain.first().and_then(|&i| all.get(i)) else { continue };
        let origin = first.anchor;
        let mut notes = Vec::new();
        let mut members = Vec::new();
        let mut names: Vec<&str> = Vec::new();
        for s in chain.iter().filter_map(|&i| all.get(i)) {
            let start = s.anchor - origin;
            notes.extend(s.notes.iter().map(|n| NoteEvent { start: n.start + start, ..*n }));
            // The piece by the hash it lists under on its own; where it sits in the phrase, from the bar line it's
            // counted from here to the bar line of the song after its last note.
            let hash = listed_hash(s, bar, listed);
            members.push(PhraseMember { hash, start, length: s.to * bar - s.anchor });
            if let Some(v) = voices.get(s.voice).filter(|v| !names.contains(&v.name.as_str())) {
                names.push(&v.name);
            }
        }
        notes.sort_by_key(|n| (n.start, n.key));
        let hash = analyser::hash_of(&notes);
        if let Some(&seen) = by_hash.get(&hash) {
            if let Some(j) = out.get_mut(seen) {
                j.repeats += 1;
            }
            continue;
        }
        by_hash.insert(hash, out.len());
        out.push(Joined { name: names.join(" + "), notes, first_bar: first.from, repeats: 1, members });
    }
    // The whole phrase must come back too, not only each of its hand-overs.
    out.retain(|j| j.repeats >= MIN_REPEATS);
    out
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn note(bar: i64, beat: i64, key: u8) -> NoteEvent {
        NoteEvent {
            start: bar * BAR + beat * PPQ,
            length: PPQ,
            key,
            velocity: 50_000,
            muted: false,
            step: false,
            attribute: mo_formats::NoteAttribute::default(),
        }
    }

    fn channel(name: &str, plugin: &str, notes: Vec<NoteEvent>) -> MidiChannel {
        MidiChannel {
            name: name.into(),
            source: plugin.into(),
            ppq: PPQ as u16,
            notes,
            plugin: plugin.into(),
            sample_path: String::new(),
            root_key: 60,
            pitch_cents: 0,
            channel: None,
        }
    }

    /// What a song in 4/4 with these channels joins.
    fn in_four(channels: &[MidiChannel]) -> Vec<Joined> {
        join(&voices(channels), BAR, &HashSet::new())
    }

    /// What a song with these voices joins, none of its pieces one of the song's patterns.
    fn join_in(voices: &[Voice], bar: i64) -> Vec<Joined> {
        join(voices, bar, &HashSet::new())
    }

    /// A tune of `bars` bars from `from`, a note each beat, climbing from `low`.
    fn tune(from: i64, bars: i64, low: u8) -> Vec<NoteEvent> {
        (0..bars).flat_map(|b| (0..4).map(move |beat| note(from + b, beat, low + (beat as u8) * 2))).collect()
    }

    /// A made-up song as an old FL project lays one out: a 16-bar lead handed from one synth to another every 24
    /// bars, a pickup on one channel answered on another every 8 bars, and drums all the way.
    fn song() -> Vec<MidiChannel> {
        let mut lead_a = Vec::new();
        let mut lead_b = Vec::new();
        for cycle in 0..3 {
            lead_a.extend(tune(cycle * 24 + 8, 8, 60));
            lead_b.extend(tune(cycle * 24 + 16, 8, 68));
        }
        let mut pickup = Vec::new();
        let mut answer = Vec::new();
        for eight in 0..6 {
            pickup.push(note(eight * 8 + 6, 3, 67));
            answer.push(note(eight * 8 + 7, 0, 72));
            answer.push(note(eight * 8 + 7, 2, 70));
        }
        let kick: Vec<NoteEvent> = (0..72).flat_map(|b| [note(b, 0, 36), note(b, 2, 36)]).collect();
        let snap: Vec<NoteEvent> = (0..18).map(|b| note(b * 4, 1, 40)).collect();
        let snap_2: Vec<NoteEvent> = (0..18).map(|b| note(b * 4 + 2, 1, 40)).collect();
        let sampled = |name: &str, notes| MidiChannel {
            sample_path: r"C:\Samples\snap_34.wav".into(),
            ..channel(name, "", notes)
        };
        vec![
            channel("Synth A", "Sytrus", lead_a),
            channel("Synth B", "3x Osc", lead_b),
            channel("Pickup", "TS404", pickup),
            channel("Answer", "SimSynth", answer),
            channel("Kick", "Sampler", kick),
            sampled("SNAP", snap),
            sampled("SNAP #2", snap_2),
        ]
    }

    #[test]
    fn a_lead_handed_between_synths_and_a_pickup_with_its_answer_join() {
        let joined = in_four(&song());
        let names: Vec<&str> = joined.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, ["Pickup + Answer", "Synth A + Synth B"]);
        let fill = &joined[0];
        assert_eq!((fill.notes.len(), fill.repeats, fill.first_bar), (3, 6, 6));
        assert_eq!(analyser::bars_of(&fill.notes), 2);
        let lead = &joined[1];
        assert_eq!((lead.notes.len(), lead.repeats, lead.first_bar), (64, 3, 8));
        assert_eq!(analyser::bars_of(&lead.notes), 16);
        assert_eq!(lead.members.len(), 2);
        assert_eq!((lead.members[1].start, lead.members[1].length), (8 * BAR, 8 * BAR));
    }

    #[test]
    fn a_lead_climbing_on_goes_to_the_next_lead_not_the_pad_doubling_it() {
        // Bars 0-7 on one synth, keys 60-75; then a lead from 75 up to 84 (its middle 13 higher) with a pad of
        // chords under it on the same bars; three times.
        let line = |from: i64, low: u8, high: u8| -> Vec<NoteEvent> {
            (0..8)
                .flat_map(|b| {
                    (0..4).map(move |beat| note(from + b, beat, low + ((b * 4 + beat) as u8 % (high - low + 1))))
                })
                .collect()
        };
        let (mut first, mut lead, mut pad) = (Vec::new(), Vec::new(), Vec::new());
        for cycle in 0..3 {
            let at = cycle * 24;
            first.extend(line(at, 60, 75));
            let mut climb = line(at + 8, 76, 84);
            climb[0].key = 75;
            lead.extend(climb);
            // Played in by hand: each chord's notes a little apart, never on one tick.
            pad.extend((0..8).flat_map(|b| {
                [60u8, 64, 67, 72].into_iter().enumerate().map(move |(i, k)| {
                    let n = note(at + 8 + b, 0, k);
                    NoteEvent { start: n.start + 5 * i as i64, length: BAR - 20, ..n }
                })
            }));
        }
        let joined = in_four(&[
            channel("Cell #3", "LegacyCell", first),
            channel("M1", "M1", lead),
            channel("Pad #2", "Wavestation", pad),
        ]);
        assert_eq!(joined.iter().map(|j| j.name.as_str()).collect::<Vec<_>>(), ["Cell #3 + M1"]);
        assert_eq!(joined[0].repeats, 3);
    }

    #[test]
    fn copies_of_a_channel_playing_the_same_sound_are_one_voice() {
        let v = voices(&song());
        assert_eq!(
            v.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
            ["Synth A", "Synth B", "Pickup", "Answer", "Kick", "SNAP"]
        );
        assert_eq!(v[5].notes.len(), 36, "SNAP and SNAP #2 take turns: one voice");
        // A doubled layer: the same notes on a copy are kept once.
        let lead = tune(0, 2, 60);
        let v = voices(&[channel("Pad", "Sytrus", lead.clone()), channel("Pad #2", "Sytrus", lead)]);
        assert_eq!((v.len(), v[0].notes.len()), (1, 8));
        // Even on another plugin: the same notes are one part, layered.
        let v = voices(&[channel("Pad", "Sytrus", tune(0, 2, 60)), channel("Pad #2", "Harmor", tune(0, 2, 60))]);
        assert_eq!(v.len(), 1);
        // Two of one plugin playing different notes are two sounds: the lead copy isn't folded into the pad.
        let v =
            voices(&[channel("Cell", "LegacyCell", tune(0, 8, 48)), channel("Cell #3", "LegacyCell", tune(8, 8, 60))]);
        assert_eq!(v.len(), 2);
        // A sample played on one key all song is a one-shot; a synth playing one note may be a pickup.
        let v = voices(&song());
        assert!(v[5].drums && !v[2].drums);
        assert_eq!(stem("SNARE_2319 #2"), "snare_2319");
        assert_eq!(stem("Track #two"), "track #two");
        assert_eq!(stem("#2"), "#2");
    }

    #[test]
    fn a_section_passed_back_and_forth_is_one_phrase_of_four_pieces_listed_once() {
        // Two synths swap every bar for a 12-bar section, three times over: four pieces at most, not the section, and
        // not again from each bar it could start on.
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for cycle in 0..3 {
            for bar in (0..12).step_by(2) {
                a.extend(tune(cycle * 16 + bar, 1, 60));
                b.extend(tune(cycle * 16 + bar + 1, 1, 62));
            }
        }
        let joined = in_four(&[channel("A", "Sytrus", a), channel("B", "Harmor", b)]);
        assert!(joined.iter().all(|j| j.members.len() <= MAX_PIECES));
        assert!(joined.iter().all(|j| j.repeats >= MIN_REPEATS));
        // No two of them over the same bars of the song.
        let mut bars: Vec<i64> = joined.iter().map(|j| j.first_bar).collect();
        bars.dedup();
        assert_eq!(bars.len(), joined.len());
    }

    #[test]
    fn repeating_alone_or_handing_over_once_joins_nothing() {
        // A whole section repeats: a bass and a lead that both play through it never hand over.
        let mut bass = Vec::new();
        let mut lead = Vec::new();
        for cycle in 0..3 {
            bass.extend(tune(cycle * 8, 8, 36));
            lead.extend(tune(cycle * 8, 8, 72));
        }
        assert!(in_four(&[channel("Bass", "Sytrus", bass), channel("Lead", "Harmor", lead)]).is_empty());
        // A hand-over heard once is two parts that happen to meet.
        let once = [channel("A", "Sytrus", tune(0, 8, 60)), channel("B", "Harmor", tune(8, 8, 64))];
        assert!(in_four(&once).is_empty());
        // Far apart registers: a bass handing to a lead, again and again, stays two parts.
        let (mut low, mut high) = (Vec::new(), Vec::new());
        for cycle in 0..3 {
            low.extend(tune(cycle * 16, 8, 28));
            high.extend(tune(cycle * 16 + 8, 8, 76));
        }
        assert!(in_four(&[channel("Low", "Sytrus", low), channel("High", "Harmor", high)]).is_empty());
        // Drums answering a lead never join it.
        let (mut lead, mut fill) = (Vec::new(), Vec::new());
        for cycle in 0..3 {
            lead.extend(tune(cycle * 8, 7, 48));
            fill.extend(tune(cycle * 8 + 7, 1, 48));
        }
        let drums = [channel("Lead", "Sytrus", lead), channel("Snare fill", "Sampler", fill)];
        assert!(in_four(&drums).is_empty());
        // Nor do one-shot samples with any name ("01 #3"), however neatly they take turns.
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for cycle in 0..4 {
            a.extend((0..4).map(|beat| note(cycle * 4, beat, 60)));
            b.extend((0..4).map(|beat| note(cycle * 4 + 1, beat, 60)));
        }
        let shots = |name: &str, notes| MidiChannel {
            sample_path: format!(r"C:\Samples\{name}.wav"),
            ..channel(name, "", notes)
        };
        assert!(in_four(&[shots("01", a), shots("01 #3", b)]).is_empty());
    }

    /// The 14 November 2026 review's MI-10: a lead's 12 bars answered by 8 on another synth, three times over. The
    /// hand-over is as strong as any, but the two together are 20 bars, more than is joined. So nothing is: the
    /// first lead's 12 bars alone were listed as a "joined" phrase of one piece.
    #[test]
    fn a_hand_over_too_long_to_join_joins_nothing() {
        // Lead A's 12 bars, Lead B's answer, and a third lead's 8 bars after it where asked, three times over.
        let section = |answer_bars: i64, third: bool| {
            let (mut a, mut b, mut c) = (Vec::new(), Vec::new(), Vec::new());
            for cycle in 0..3 {
                a.extend(tune(cycle * 32, 12, 60));
                b.extend(tune(cycle * 32 + 12, answer_bars, 62));
                if third {
                    c.extend(tune(cycle * 32 + 12 + answer_bars, 8, 64));
                }
            }
            let (a, b) = (channel("Lead A", "Sytrus", a), channel("Lead B", "Harmor", b));
            in_four(&[a, b, channel("Lead C", "3x Osc", c)])
        };
        let names = |joined: &[Joined]| joined.iter().map(|j| j.name.clone()).collect::<Vec<_>>();
        let shape = |j: &Joined| (j.members.len(), j.repeats, analyser::bars_of(&j.notes));
        assert_eq!(names(&section(8, false)), [""; 0]);
        // With an answer of 4 bars the two are 16 bars, and join as they did: both pieces, three times.
        let joined = section(4, false);
        assert_eq!(names(&joined), ["Lead A + Lead B"]);
        assert_eq!(shape(&joined[0]), (2, 3, 16));
        // The answer too long to join to Lead A still starts a phrase with the lead that takes over from it: being
        // handed to by a lead it can't join doesn't use it up.
        let joined = section(8, true);
        assert_eq!(names(&joined), ["Lead B + Lead C"]);
        assert_eq!((shape(&joined[0]), joined[0].first_bar), ((2, 3, 16), 12));
        // A joined phrase is always two pieces or more, in every song these tests make.
        assert!(in_four(&song()).iter().all(|j| j.members.len() >= 2));
    }

    /// The 19 November 2026 review's MP-7: a lead with a layer of it on another synth, both handing to the same
    /// third synth. Which layer's chain was kept was left to the order a map gave its keys, another each time the
    /// phrase came back: about every second reading neither came back twice, and the phrase was lost.
    #[test]
    fn two_layers_that_hand_to_one_synth_join_the_same_way_every_time() {
        // Eight bars on two synths at once, then eight on a third; twice. `second` is how far up the layer plays.
        let layered = |second: u8| {
            let (mut lead, mut layer, mut answer) = (Vec::new(), Vec::new(), Vec::new());
            for cycle in 0..2 {
                lead.extend(tune(cycle * 24, 8, 60));
                layer.extend(tune(cycle * 24, 8, 60 + second));
                answer.extend(tune(cycle * 24 + 8, 8, 62));
            }
            // The layer is the first channel of the song: it isn't the order of the channels that picks the lead.
            [channel("Layer", "Harmor", layer), channel("Lead", "Sytrus", lead), channel("Answer", "3x Osc", answer)]
        };
        // Each call's maps come in an order of their own, so 40 calls are every order there is, many times over.
        for _ in 0..40 {
            // A harmony a third up: the answer starts inside the lead's range and two semitones under the layer's,
            // so it's the lead the tune steps on from most smoothly.
            let joined = in_four(&layered(4));
            assert_eq!(joined.iter().map(|j| (j.name.as_str(), j.repeats)).collect::<Vec<_>>(), [("Lead + Answer", 2)]);
            assert_eq!((joined[0].first_bar, analyser::bars_of(&joined[0].notes)), (0, 16));
            // The same notes under another name: nothing tells the two apart, so the song's first channel has it.
            let joined = in_four(&layered(0));
            assert_eq!(
                joined.iter().map(|j| (j.name.as_str(), j.repeats)).collect::<Vec<_>>(),
                [("Layer + Answer", 2)]
            );
        }
    }

    /// Two layers in unison both hand over each time the phrase comes back: that's the phrase once, not twice.
    #[test]
    fn a_hand_over_two_layers_play_in_unison_counts_once_each_time_it_comes_back() {
        // Three bars answered by three, an untidy six: coming back twice isn't enough to join them (a score of 5).
        let song = |times: i64, layered: bool| {
            let (mut lead, mut answer) = (Vec::new(), Vec::new());
            for cycle in 0..times {
                lead.extend(tune(cycle * 16, 3, 60));
                answer.extend(tune(cycle * 16 + 3, 3, 62));
            }
            let layer = if layered { lead.clone() } else { Vec::new() };
            in_four(&[
                channel("Lead", "Sytrus", lead),
                channel("Layer", "Harmor", layer),
                channel("Answer", "M1", answer),
            ])
        };
        assert!(song(2, false).is_empty());
        // A layer of the lead doubled the count: twice read as four times, and joined.
        assert!(song(2, true).is_empty());
        // Three times is enough, with a layer or without, and it's said as three.
        for layered in [false, true] {
            let joined = song(3, layered);
            assert_eq!(joined.iter().map(|j| (j.name.as_str(), j.repeats)).collect::<Vec<_>>(), [("Lead + Answer", 3)]);
        }
    }

    /// A note on each beat of `bars` bars of `beats` beats each, from the song's bar `from`, climbing from `low`.
    fn tune_in(beats: i64, from: i64, bars: i64, low: u8) -> Vec<NoteEvent> {
        (0..bars * beats)
            .map(|at| NoteEvent { start: (from * beats + at) * PPQ, ..note(0, 0, low + (at % beats) as u8 * 2) })
            .collect()
    }

    /// The 19 November 2026 review's MP-8: a waltz's lead, eight bars on one synth and eight on another from the
    /// song's bar 17, twice. Counted in bars of 4/4 that was "bar 13", and its 16 bars were an untidy 12, so with
    /// only two times round it never joined.
    #[test]
    fn a_song_in_three_four_is_counted_in_its_own_bars() {
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for cycle in 0..2 {
            a.extend(tune_in(3, 16 + cycle * 32, 8, 60));
            b.extend(tune_in(3, 24 + cycle * 32, 8, 62));
        }
        let waltz = voices(&[channel("Synth A", "Sytrus", a), channel("Synth B", "3x Osc", b)]);
        let joined = join_in(&waltz, 3 * PPQ);
        assert_eq!(joined.iter().map(|j| j.name.as_str()).collect::<Vec<_>>(), ["Synth A + Synth B"]);
        let lead = &joined[0];
        assert_eq!((lead.first_bar, lead.repeats, lead.notes.len()), (16, 2, 48));
        assert_eq!(crate::extract::joined_clip(lead), "From bar 17 of the song, twice");
        // Its notes are as every part in the library has them: ticks from the bar line it starts on, the song's own
        // here (12 bars of 4/4 long). Each piece is where it sits in them, by the song's bars.
        assert_eq!((lead.notes[0].start, analyser::bars_of(&lead.notes)), (0, 12));
        let places: Vec<(i64, i64)> = lead.members.iter().map(|m| (m.start, m.length)).collect();
        assert_eq!(places, [(0, 8 * 3 * PPQ), (8 * 3 * PPQ, 8 * 3 * PPQ)]);
        // Each piece is known by the hash its notes have from their own first bar, as the page lists it.
        assert_eq!(lead.members[0].hash, analyser::hash_of(&tune_in(3, 0, 8, 60)));
        assert_eq!(lead.members[1].hash, analyser::hash_of(&tune_in(3, 0, 8, 62)));
        // As it was counted: nothing joined.
        assert!(join_in(&waltz, BAR).is_empty());

        // A pickup and its answer every five bars of the waltz (15 beats): in bars of 4/4 they fell on another beat
        // of the bar each of the four times, so they never came back "the same way" at all.
        let (mut pickup, mut answer) = (Vec::new(), Vec::new());
        for five in 0..4 {
            pickup.push(NoteEvent { start: ((five * 5 + 3) * 3 + 2) * PPQ, ..note(0, 0, 67) });
            answer.extend(tune_in(3, five * 5 + 4, 1, 70));
        }
        let fills = voices(&[channel("Pickup", "TS404", pickup), channel("Answer", "SimSynth", answer)]);
        let joined = join_in(&fills, 3 * PPQ);
        assert_eq!(
            joined.iter().map(|j| (j.name.as_str(), j.first_bar, j.repeats)).collect::<Vec<_>>(),
            [("Pickup + Answer", 3, 4)]
        );
        assert!(join_in(&fills, BAR).is_empty());
    }

    /// A piece of a phrase that is one of the song's patterns is known by the hash that pattern lists under, on
    /// whichever of the song's bars its clip sits. In a waltz that isn't always the hash it has from the song's bar
    /// line: a pattern that rests for its first bar lists with that rest in front.
    #[test]
    fn a_piece_that_is_one_of_the_songs_patterns_is_known_by_the_hash_it_lists_under() {
        // Synth A's pattern rests for a bar, then plays seven; Synth B's plays eight. Three times, 32 bars apart.
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for cycle in 0..3 {
            a.extend(tune_in(3, 17 + cycle * 32, 7, 60));
            b.extend(tune_in(3, 24 + cycle * 32, 8, 62));
        }
        let waltz = voices(&[channel("Synth A", "Sytrus", a), channel("Synth B", "3x Osc", b)]);
        // As the page lists the two patterns: from the bar of 4/4 each one's first note is in.
        let (rests_first, answer) =
            (analyser::hash_of(&tune_in(3, 1, 7, 60)), analyser::hash_of(&tune_in(3, 0, 8, 62)));
        let listed = HashSet::from([rests_first.clone(), answer.clone()]);
        let joined = join(&waltz, 3 * PPQ, &listed);
        assert_eq!(
            joined.iter().map(|j| (j.name.as_str(), j.first_bar, j.repeats)).collect::<Vec<_>>(),
            [("Synth A + Synth B", 17, 3)]
        );
        assert_eq!(joined[0].members.iter().map(|m| m.hash.as_str()).collect::<Vec<_>>(), [&rests_first, &answer]);
        // Where each piece sits in the phrase is by the song's bars all the same.
        let places: Vec<(i64, i64)> = joined[0].members.iter().map(|m| (m.start, m.length)).collect();
        assert_eq!(places, [(0, 7 * 3 * PPQ), (7 * 3 * PPQ, 8 * 3 * PPQ)]);
        // A piece that is none of the song's patterns keeps the hash it has from the song's bar line.
        let alone = join_in(&waltz, 3 * PPQ);
        assert_eq!(alone[0].members[0].hash, analyser::hash_of(&tune_in(3, 0, 7, 60)));
        assert_eq!((&alone[0].notes, alone[0].repeats), (&joined[0].notes, 3));
        // In 4/4 a clip's bar line is the pattern's own: only the one hash is ever tried.
        let bar_late = analyser::hash_of(&[note(0, 1, 60)]);
        let on_the_beat = stretch_of(0, vec![note(5, 0, 60)], BAR);
        assert_eq!(listed_hash(&on_the_beat, BAR, &HashSet::from([bar_late.clone()])), on_the_beat.hash);
        // In 3/4 the same note on a bar line of the song can be a pattern's note on its second beat.
        let in_three = stretch_of(0, vec![NoteEvent { start: 7 * 3 * PPQ, ..note(0, 0, 60) }], 3 * PPQ);
        assert_eq!(listed_hash(&in_three, 3 * PPQ, &HashSet::from([bar_late.clone()])), bar_late);
    }

    /// A hand-over counts once for each place in the song it starts at: layers starting together are one time, the
    /// same figure twice in one long bar is two.
    #[test]
    fn a_hand_over_counts_once_for_each_place_it_starts_at() {
        let six_four = 6 * PPQ;
        let at =
            |voice: usize, tick: i64| stretch_of(voice, vec![NoteEvent { start: tick, ..note(0, 0, 60) }], six_four);
        // Two layers on beat 1 of the first bar, the same note on its beat 5, and again six bars on.
        let all = [at(0, 0), at(1, 0), at(2, 4 * PPQ), at(3, 6 * six_four)];
        assert!(all.iter().all(|s| s.hash == all[0].hash));
        assert_eq!(all.iter().map(|s| s.from).collect::<Vec<_>>(), [0, 0, 0, 6]);
        assert_eq!(times(&[(0, 9), (1, 9)], &all), 1);
        assert_eq!(times(&[(0, 9), (2, 9)], &all), 2);
        assert_eq!(times(&[(0, 9), (1, 9), (2, 9), (3, 9)], &all), 3);
    }

    /// Where the song's bars are longer than 4/4's, a phrase is still no longer than the index keeps whole (16 bars
    /// of 4/4), and its notes still start in their first bar of 4/4, as every part in the library does.
    #[test]
    fn a_song_with_long_bars_joins_no_more_than_the_index_keeps_whole() {
        let five_four = 5 * PPQ;
        let handed = |bars: i64, pickup: bool| {
            let (mut a, mut b) = (Vec::new(), Vec::new());
            for cycle in 0..3 {
                let mut first = tune_in(5, cycle * 40, bars, 60);
                if pickup {
                    // It comes in on the fifth beat of its first bar: a bar of 4/4 after the song's bar line.
                    first.retain(|n| n.start >= cycle * 40 * five_four + 4 * PPQ);
                }
                a.extend(first);
                b.extend(tune_in(5, cycle * 40 + bars, bars, 62));
            }
            join_in(&voices(&[channel("Synth A", "Sytrus", a), channel("Synth B", "3x Osc", b)]), five_four)
        };
        // 8 + 8 bars of 5/4 are 20 bars of 4/4: a section, which the index would cut in pieces again.
        assert!(handed(8, false).is_empty());
        // 4 + 4 are 10: one phrase.
        let joined = handed(4, false);
        assert_eq!(joined.iter().map(|j| (j.name.as_str(), j.repeats)).collect::<Vec<_>>(), [("Synth A + Synth B", 3)]);
        assert_eq!((joined[0].notes[0].start, analyser::bars_of(&joined[0].notes)), (0, 10));
        assert_eq!(analyser::split(&joined[0].notes, None).len(), 1);
        // With the pickup the phrase's notes start a bar of 4/4 after the song's bar line, and the second piece
        // sits four of the song's bars after that line.
        let joined = handed(4, true);
        assert_eq!(joined.len(), 1);
        assert_eq!((joined[0].first_bar, joined[0].notes[0].start), (0, 0));
        assert_eq!(joined[0].members.iter().map(|m| m.start).collect::<Vec<_>>(), [0, 4 * five_four - BAR]);
        assert_eq!(joined[0].members[0].length, 4 * five_four - BAR);
        assert_eq!(analyser::split(&joined[0].notes, None).len(), 1);
    }
}
