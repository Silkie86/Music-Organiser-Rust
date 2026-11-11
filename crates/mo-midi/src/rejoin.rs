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
//! - they sound like one voice: their notes sit near each other, so the tune carries on;
//! - the joined phrase is a tidy length (2, 4, 8 or 16 bars), or a piece is too short to be a part on its own.
//!
//! Drums never join (a fill answering a lead is two parts), and neither do pieces in far apart registers (a bass line
//! handing to a lead is two parts). Channels named the same with " #2" and so on that play the same sample or plugin
//! are one voice first ([`voices`]): FL users copied a channel to add hits or a doubled layer.
//!
//! Everything here is from the song as the playlist lays it out ([`fl::song::extract_channels`]).

use std::collections::HashMap;

use mo_formats::{MidiChannel, NoteEvent, case};

use crate::analyser;
use crate::member::PhraseMember;
use crate::roles::Role;
use crate::{BAR, PPQ};

/// How many times two stretches must come back together, the same way, to be one phrase.
pub const MIN_REPEATS: usize = 2;
/// The score a pair of stretches needs to be joined ([`score`]).
pub const JOIN_SCORE: i32 = 5;
/// The most a phrase is joined to: longer, it's a section of the song, not a phrase (and the index would cut it into
/// pieces again, [`analyser::MAX_BARS`]).
pub const MAX_JOINED_BARS: i64 = analyser::MAX_BARS;
/// How far apart, in semitones, the middles of two pieces may be and still be one voice.
pub const MAX_REGISTER_GAP: f64 = 12.0;

/// One voice of the song: a channel, with its " #2" copies that play the same sample or plugin folded in, its notes at
/// [`PPQ`] where the playlist puts them.
#[derive(Clone, Debug, PartialEq)]
pub struct Voice {
    /// The first channel's name ("SNAP_34" for SNAP_34 and SNAP_34 #2).
    pub name: String,
    pub notes: Vec<NoteEvent>,
    /// Drums or other unpitched hits: never joined to another voice.
    pub drums: bool,
}

/// A phrase joined from several voices' stretches: the names joined ("LegacyCell #3 + M1"), its notes from its first
/// bar, where it first plays in the song (as a bar from 0), how many times it comes back, and its pieces.
#[derive(Clone, Debug, PartialEq)]
pub struct Joined {
    pub name: String,
    pub notes: Vec<NoteEvent>,
    pub first_bar: i64,
    pub repeats: usize,
    pub members: Vec<PhraseMember>,
}

/// A run of one voice's notes with no empty bar in it, in bars from the song's start (`to` is the bar after its last).
#[derive(Clone, Debug)]
struct Stretch {
    voice: usize,
    from: i64,
    to: i64,
    /// Ticks: its first note's start, and where its last note ends.
    first: i64,
    last_end: i64,
    /// The notes moved to start in its first bar, and their fingerprint: a stretch that comes back the same way.
    hash: String,
    notes: Vec<NoteEvent>,
    middle: f64,
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

/// The song's voices: each channel with notes, the copies named "#2" and so on that play the same sample (or the same
/// plugin, when there's no sample) folded into the first. Notes both play (a doubled layer) are kept once.
pub fn voices(channels: &[MidiChannel]) -> Vec<Voice> {
    let mut out: Vec<Voice> = Vec::new();
    let mut by_key: HashMap<(String, String), usize> = HashMap::new();
    for c in channels.iter().filter(|c| !c.notes.is_empty()) {
        let plays = if c.sample_path.is_empty() { c.plugin.as_str() } else { c.sample_path.as_str() };
        let key = (stem(&c.name), case::to_lower(plays));
        let notes = analyser::normalise(&c.notes, i64::from(c.ppq), true);
        let drums = crate::extract::is_unpitched_plugin(&c.plugin)
            || crate::extract::is_unpitched_plugin(&c.name)
            || analyser::roles_named(&format!("{} {}", c.name, c.source)).contains(&Role::Drums);
        match by_key.get(&key).and_then(|&at| out.get_mut(at)) {
            Some(v) => {
                v.notes.extend(notes);
                v.drums |= drums;
            }
            None => {
                by_key.insert(key, out.len());
                out.push(Voice { name: c.name.clone(), notes, drums });
            }
        }
    }
    for v in &mut out {
        v.notes.sort_by_key(|n| (n.start, n.key, n.length));
        v.notes.dedup_by(|a, b| a.start == b.start && a.key == b.key && a.length == b.length);
    }
    out
}

/// A voice's notes cut wherever a whole bar goes by with no note sounding.
fn stretches(at: usize, v: &Voice) -> Vec<Stretch> {
    let mut out = Vec::new();
    let mut run: Vec<NoteEvent> = Vec::new();
    let mut sounding_until = i64::MIN;
    for n in &v.notes {
        // The next note starts after a bar that has nothing in it.
        let empty_bar = sounding_until != i64::MIN && n.start.div_euclid(BAR) > sounding_until.div_euclid(BAR) + 1;
        if empty_bar && !run.is_empty() {
            out.push(stretch_of(at, std::mem::take(&mut run)));
        }
        sounding_until = sounding_until.max(n.start + n.length - 1);
        run.push(*n);
    }
    if !run.is_empty() {
        out.push(stretch_of(at, run));
    }
    out
}

fn stretch_of(voice: usize, notes: Vec<NoteEvent>) -> Stretch {
    let first = notes.first().map_or(0, |n| n.start);
    let last_end = notes.iter().map(|n| n.start + n.length).max().unwrap_or(first);
    let from = first.div_euclid(BAR);
    let to = (last_end - 1).div_euclid(BAR) + 1;
    let moved: Vec<NoteEvent> = notes.iter().map(|n| NoteEvent { start: n.start - from * BAR, ..*n }).collect();
    let middle = notes.iter().map(|n| f64::from(n.key)).sum::<f64>() / notes.len().max(1) as f64;
    let lowest = notes.iter().map(|n| n.key).min().unwrap_or(0);
    let highest = notes.iter().map(|n| n.key).max().unwrap_or(0);
    Stretch { voice, from, to, first, last_end, hash: analyser::hash_of(&moved), notes: moved, middle, lowest, highest }
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
    // 0 when they overlap.
    let gap = b.lowest.saturating_sub(a.highest).max(a.lowest.saturating_sub(b.highest));
    if gap <= 3 {
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
pub fn join(voices: &[Voice]) -> Vec<Joined> {
    let all: Vec<Stretch> =
        voices.iter().enumerate().filter(|(_, v)| !v.drums).flat_map(|(at, v)| stretches(at, v)).collect();
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
            if !hands_over(a, b) || (a.middle - b.middle).abs() > MAX_REGISTER_GAP {
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
    let mut next: HashMap<usize, usize> = HashMap::new();
    for pairs in handovers.values() {
        let Some(&(i, j)) = pairs.first() else { continue };
        let (Some(a), Some(b)) = (all.get(i), all.get(j)) else { continue };
        if pairs.len() >= MIN_REPEATS && score(a, b, pairs.len()) >= JOIN_SCORE {
            for &(i, j) in pairs {
                // One way on from each stretch: the first found keeps it.
                next.entry(i).or_insert(j);
            }
        }
    }
    let starts_a_chain = |i: &usize| !next.values().any(|j| j == i);
    let mut heads: Vec<usize> = next.keys().copied().filter(starts_a_chain).collect();
    heads.sort_by_key(|&i| all.get(i).map_or(0, |s| s.first));
    let mut out: Vec<Joined> = Vec::new();
    let mut by_hash: HashMap<String, usize> = HashMap::new();
    for head in heads {
        let mut chain = vec![head];
        let mut at = head;
        while let Some(&j) = next.get(&at) {
            let too_long = all.get(j).zip(all.get(head)).is_some_and(|(s, h)| s.to - h.from > MAX_JOINED_BARS);
            if chain.contains(&j) || too_long {
                break;
            }
            chain.push(j);
            at = j;
        }
        let Some(first) = all.get(head) else { continue };
        let origin = first.from * BAR;
        let mut notes = Vec::new();
        let mut members = Vec::new();
        let mut names: Vec<&str> = Vec::new();
        for s in chain.iter().filter_map(|&i| all.get(i)) {
            let start = s.from * BAR - origin;
            notes.extend(s.notes.iter().map(|n| NoteEvent { start: n.start + start, ..*n }));
            // As the piece lists on its own: from its first bar.
            members.push(PhraseMember { hash: s.hash.clone(), start, length: (s.to - s.from) * BAR });
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
        vec![
            channel("Synth A", "Sytrus", lead_a),
            channel("Synth B", "3x Osc", lead_b),
            channel("Pickup", "TS404", pickup),
            channel("Answer", "SimSynth", answer),
            channel("Kick", "Sampler", kick),
            channel("SNAP", "Sampler", snap),
            channel("SNAP #2", "Sampler", snap_2),
        ]
    }

    #[test]
    fn a_lead_handed_between_synths_and_a_pickup_with_its_answer_join() {
        let voices = voices(&song());
        let joined = join(&voices);
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
        // A copy that plays another plugin is another voice.
        let v = voices(&[channel("Pad", "Sytrus", tune(0, 2, 60)), channel("Pad #2", "Harmor", tune(0, 2, 60))]);
        assert_eq!(v.len(), 2);
        assert_eq!(stem("SNARE_2319 #2"), "snare_2319");
        assert_eq!(stem("Track #two"), "track #two");
        assert_eq!(stem("#2"), "#2");
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
        assert!(join(&voices(&[channel("Bass", "Sytrus", bass), channel("Lead", "Harmor", lead)])).is_empty());
        // A hand-over heard once is two parts that happen to meet.
        let once = [channel("A", "Sytrus", tune(0, 8, 60)), channel("B", "Harmor", tune(8, 8, 64))];
        assert!(join(&voices(&once)).is_empty());
        // Far apart registers: a bass handing to a lead, again and again, stays two parts.
        let (mut low, mut high) = (Vec::new(), Vec::new());
        for cycle in 0..3 {
            low.extend(tune(cycle * 16, 8, 28));
            high.extend(tune(cycle * 16 + 8, 8, 76));
        }
        assert!(join(&voices(&[channel("Low", "Sytrus", low), channel("High", "Harmor", high)])).is_empty());
        // Drums answering a lead never join it.
        let (mut lead, mut fill) = (Vec::new(), Vec::new());
        for cycle in 0..3 {
            lead.extend(tune(cycle * 8, 7, 48));
            fill.extend(tune(cycle * 8 + 7, 1, 48));
        }
        let drums = [channel("Lead", "Sytrus", lead), channel("Snare fill", "Sampler", fill)];
        assert!(join(&voices(&drums)).is_empty());
    }
}
