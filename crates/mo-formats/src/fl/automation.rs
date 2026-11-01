//! Automation clips and the mixer's settings in an FL project: what each clip moves, its curve where the playlist puts
//! it, and the mixer inserts' faders, pans and effect slots.

use std::collections::HashMap;

use super::events::*;
use super::song::{FlAudioClip, FlClip, PlaylistItem, PlaylistTracks};

/// What an automation link (227) or a mixer setting (225) moves: the target's high word is a channel, a mixer insert's
/// slot (see [`MixerSettings`]) or the song, and its low word what on it. The insert and song ones match the ids FL's
/// scripting docs give; only channel volume, on/off and plugin settings have been seen in an automation link so far.
const TARGET_VOLUME: u32 = 0;
const TARGET_PAN: u32 = 1;
const TARGET_PITCH: u32 = 4;
const TARGET_ON_OFF: u32 = 7;
const TARGET_PLUGIN: u32 = 0x8000;
/// FL 8 saved an edit event moving a plugin's setting n as 0x2000 + n, rather than 0x8000 + n.
const FL8_PLUGIN: u32 = 0x2000;
/// FL 8 saved an edit event's value as a whole number, 2^20 for 1.
const FL8_ONE: f64 = (1u32 << 20) as f64;
pub(super) const TARGET_SLOT_ON_OFF: u32 = 0x1f00;
pub(super) const TARGET_INSERT_VOLUME: u32 = 0x1fc0;
pub(super) const TARGET_INSERT_PAN: u32 = 0x1fc1;
const TARGET_SONG: u32 = 0x4000;
const TARGET_MAIN_VOLUME: u32 = 0;
const TARGET_TEMPO: u32 = 5;

/// How far apart two points can be and still count as at the same beat: FL adds up the gaps between points, which
/// drifts.
const SAME_BEAT: f64 = 1e-4;

/// What an automation clip moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FlControl {
    /// A channel's volume, pan, pitch, on/off (its Channel Rack light) or a setting of its plugin.
    Volume,
    Pan,
    Pitch,
    PluginParameter,
    Other,
    OnOff,
    /// A mixer insert's volume, pan, an effect slot's on/off or a setting of the effect in it.
    InsertVolume,
    InsertPan,
    InsertSlotOnOff,
    InsertEffect,
    /// The song's tempo or master volume.
    Tempo,
    MainVolume,
}

/// A point on an automation curve: beats from the start of the song (or clip), and the value (0..1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlPoint {
    pub beat: f64,
    pub value: f64,
}

/// An automation clip's curve across the song, as placed in the playlist.
#[derive(Clone, Debug, PartialEq)]
pub struct FlAutomation {
    pub name: String,
    /// The channel it moves something on; `None` when it moves a mixer insert or the song.
    pub channel: Option<u32>,
    pub control: FlControl,
    /// For a plugin setting, its index.
    pub parameter: u32,
    pub points: Vec<FlPoint>,
    /// The mixer insert it moves something on.
    pub insert: Option<u32>,
    /// For an insert's effect, its slot.
    pub slot: Option<u32>,
    /// The beats where each of its playlist clips starts.
    pub starts: Vec<f64>,
    /// For edit events (knob moves recorded into a pattern): each place the pattern plays, with its moves timed from
    /// there, so a DAW with automation clips can give each its own. Empty for an automation clip, whose curve is
    /// [`FlAutomation::points`] only.
    pub parts: Vec<FlPart>,
}

/// One place a pattern with edit events plays, from its first move (FL leaves the setting alone until then) to where
/// the placement ends or a later one takes over: where that starts and how long it is (beats from the song's start),
/// and the moves, in beats from where it starts, cut to the placement as the song's curve is.
#[derive(Clone, Debug, PartialEq)]
pub struct FlPart {
    pub start: f64,
    pub length: f64,
    pub points: Vec<FlPoint>,
}

impl FlAutomation {
    /// Where the playlist clip a point belongs to starts. Where clips moving one thing meet on a beat, the points of
    /// the one starting there go last, so it has the last word.
    pub fn start_of(&self, point: FlPoint) -> f64 {
        self.starts.iter().rev().find(|&&s| s <= point.beat + SAME_BEAT).copied().unwrap_or(0.0)
    }
}

/// An automation clip's points (event 234): a 21-byte header with the count at 17, then 24 bytes a point: f64 beats
/// since the previous point, f64 value (0..1), f32 tension, u32 flags. Curves between points (tension) aren't kept;
/// the points are joined by straight lines.
pub(super) fn automation_points(d: &[u8]) -> Vec<FlPoint> {
    let mut points = Vec::new();
    if d.len() < 21 {
        return points;
    }
    let count = u32_at(d, 17) as usize;
    let mut beat = 0.0;
    for p in d.get(21..).unwrap_or_default().as_chunks::<24>().0.iter().take(count) {
        let (Some(step), Some(value)) = (f64_at(p, 0), f64_at(p, 8)) else { break };
        beat += step;
        // A step back in time is damage, as an endless or unreadable number is: the points before it are kept, so a
        // curve always runs forward (placing it relies on that).
        if !beat.is_finite() || beat < 0.0 || step < 0.0 || !value.is_finite() {
            break;
        }
        points.push(FlPoint { beat, value: value.clamp(0.0, 1.0) });
    }
    points
}

/// The mixer's settings (event 225): 12 bytes each, u32 0, u32 the setting's target (as an automation link names it)
/// and i32 its value. A setting on a mixer insert has base + 64 × insert + slot in its target's high word (slot 0 for
/// the insert's own fader and pan). The base is 0x2000 in FL 21 and 0x7000 in FL 25 and 26, so it's taken from the
/// file: the lowest high word of an insert's fader.
pub(super) struct MixerSettings {
    values: HashMap<u32, i32>,
    base: Option<u32>,
}

impl MixerSettings {
    pub fn new(data: Option<&[u8]>) -> MixerSettings {
        let mut values = HashMap::new();
        for r in data.unwrap_or(&[]).as_chunks::<12>().0 {
            values.insert(u32_at(r, 4), i32_at(r, 8));
        }
        let base = values
            .keys()
            .filter(|&&t| t & 0xFFFF == TARGET_INSERT_VOLUME && t >> 16 != TARGET_SONG)
            .map(|t| t >> 16)
            .min();
        MixerSettings { values, base }
    }

    /// A setting's value on an insert's slot, if the file has it.
    pub fn value(&self, insert: u32, slot: u32, setting: u32) -> Option<i32> {
        let high = self.base? as u64 + 64 * insert as u64 + slot as u64;
        let target = u32::try_from(high << 16 | setting as u64).ok()?;
        self.values.get(&target).copied()
    }

    /// The insert and slot a target's high word names, when it names one of the file's inserts.
    pub fn slot(&self, high: u32) -> Option<(u32, u32)> {
        let b = self.base?;
        if high < b || high == TARGET_SONG {
            return None;
        }
        let (insert, slot) = ((high - b) / 64, (high - b) % 64);
        self.value(insert, 0, TARGET_INSERT_VOLUME).map(|_| (insert, slot))
    }
}

/// The part of a curve from `from` to `from + length` (beats into the clip), moved to start at `at`. Between points the
/// value slides in a straight line, or with `hold` stays at the earlier point's until the next. Where several points
/// share a beat (a jump), the part starts from the value after the jump and ends on the value before it; a jump right
/// at its end is kept as a second point there, so what's held after the clip is where the curve ends (FL saves a
/// clip's last points at its very end).
pub(super) fn cut_curve(curve: &[FlPoint], at: f64, from: f64, length: f64, hold: bool) -> Vec<FlPoint> {
    if length <= 0.0 || curve.is_empty() {
        return Vec::new();
    }
    let value_at = |beat: f64, arriving: bool| {
        let first = curve.iter().find(|p| p.beat >= beat - SAME_BEAT);
        if arriving
            && let Some(f) = first
            && f.beat <= beat + SAME_BEAT
        {
            return f.value;
        }
        // Before the first point, the curve starts at its value.
        let Some(last) = curve.iter().rposition(|p| p.beat <= beat + SAME_BEAT) else {
            return curve.first().map_or(0.0, |p| p.value);
        };
        let Some(&a) = curve.get(last) else { return 0.0 };
        match curve.get(last + 1) {
            Some(b) if !hold && a.beat < beat - SAME_BEAT => {
                a.value + (b.value - a.value) * (beat - a.beat) / (b.beat - a.beat)
            }
            _ => a.value,
        }
    };
    let end = from + length;
    let mut out = vec![FlPoint { beat: at, value: value_at(from, false) }];
    out.extend(
        curve
            .iter()
            .filter(|p| p.beat > from + SAME_BEAT && p.beat < end - SAME_BEAT)
            .map(|p| FlPoint { beat: at + p.beat - from, value: p.value }),
    );
    let arrived = value_at(end, true);
    out.push(FlPoint { beat: at + length, value: arrived });
    let after = value_at(end, false);
    if after != arrived {
        out.push(FlPoint { beat: at + length, value: after });
    }
    out
}

/// What a link's target moves, for a clip named `name` with these points.
fn moves(name: &str, target: u32, mixer: &MixerSettings, points: Vec<FlPoint>) -> FlAutomation {
    let (high, low) = (target >> 16, target & 0xFFFF);
    let plugin = if low >= TARGET_PLUGIN { low - TARGET_PLUGIN } else { low };
    let mut a = FlAutomation {
        name: name.to_owned(),
        channel: None,
        control: FlControl::Other,
        parameter: plugin,
        points,
        insert: None,
        slot: None,
        starts: Vec::new(),
        parts: Vec::new(),
    };
    if high == TARGET_SONG {
        a.parameter = low;
        a.control = match low {
            TARGET_TEMPO => FlControl::Tempo,
            TARGET_MAIN_VOLUME => FlControl::MainVolume,
            _ => FlControl::Other,
        };
    } else if let Some((insert, slot)) = mixer.slot(high) {
        a.control = match (low, slot) {
            (TARGET_INSERT_VOLUME, 0) => FlControl::InsertVolume,
            (TARGET_INSERT_PAN, 0) => FlControl::InsertPan,
            (TARGET_SLOT_ON_OFF, _) => FlControl::InsertSlotOnOff,
            (l, _) if l >= TARGET_PLUGIN => FlControl::InsertEffect,
            _ => FlControl::Other,
        };
        (a.insert, a.slot) = (Some(insert), Some(slot));
    } else {
        a.channel = Some(high);
        a.control = match low {
            TARGET_VOLUME => FlControl::Volume,
            TARGET_PAN => FlControl::Pan,
            TARGET_PITCH => FlControl::Pitch,
            TARGET_ON_OFF => FlControl::OnOff,
            l if l >= TARGET_PLUGIN => FlControl::PluginParameter,
            _ => FlControl::Other,
        };
    }
    a
}

/// Automation clips placed where the playlist puts them, cut to each clip's length, joined per clip. A link (event
/// 227) gives the clip's channel and its target. Clips that aren't in the playlist, or are muted there or on a muted
/// track, don't play, so they're left out. A clip placed several times, with gaps between, becomes one curve that
/// holds its value through the gaps. An on/off switches rather than slides, so its curve holds each point's value until
/// the next.
pub(super) fn place_automation(
    links: &[(u32, u32)],
    curves: &HashMap<u32, Vec<FlPoint>>,
    names: &HashMap<u32, String>,
    items: &[PlaylistItem],
    tracks: &PlaylistTracks,
    mixer: &MixerSettings,
    ppq: u16,
) -> (Vec<FlAutomation>, bool, usize) {
    let mut result = Vec::new();
    let mut cut = false;
    let mut placed_points = 0usize;
    if ppq == 0 {
        return (result, cut, placed_points);
    }
    let ppq = ppq as f64;
    let mut items: Vec<PlaylistItem> = items
        .iter()
        .filter(|i| i.index <= super::song::PATTERN_BASE)
        .map(|&i| PlaylistItem { muted: i.muted || tracks.at(i.slot).is_some_and(|t| t.1), ..i })
        .collect();
    items.sort_by_key(|i| i.at);
    // Each clip's placements, in time order.
    let mut placements: HashMap<u32, Vec<PlaylistItem>> = HashMap::new();
    for i in items.into_iter().filter(|i| !i.muted) {
        placements.entry(i.index).or_default().push(i);
    }
    for &(clip, target) in links {
        let Some(curve) = curves.get(&clip).filter(|c| !c.is_empty()) else { continue };
        let name = names.get(&clip).cloned().unwrap_or_else(|| format!("Automation {}", clip + 1));
        // Switches jump between on and off rather than slide.
        let hold =
            matches!(moves(&name, target, mixer, Vec::new()).control, FlControl::OnOff | FlControl::InsertSlotOnOff);
        let (placed, starts) = place_curve(
            curve,
            placements.get(&clip).map_or(&[][..], Vec::as_slice),
            ppq,
            hold,
            false,
            &mut placed_points,
            &mut cut,
            None,
        );
        if !placed.is_empty() {
            result.push(FlAutomation { starts, ..moves(&name, target, mixer, placed) });
        }
    }
    (result, cut, placed_points)
}

/// Edit events: knob moves recorded into patterns (event 223), placed where the playlist puts each pattern, as
/// automation clips are. Each is 12 bytes: u32 the tick in the pattern, u32 what it moves (as an automation link's
/// target), f32 the value (0..1); worked out from Solomon's sly_15 (FL 25.2.5), where they move LegacyCell's settings
/// on every tick, and checked against FL's Edit events window for its Encoder1. FL 8 saved the same moves with the
/// target's low word 0x2000 + n and the value as a whole number (2^20 for 1); found in sly_15 saved in FL 8's format,
/// where each value matched the FL 25 save's. Before a pattern's first edit event its setting is left where it was,
/// so each placement's part starts there. Only plugin settings are carried, as only they have been seen; the patterns
/// with edit events moving anything else (or placed nowhere they could be, in a song without a playlist) come back to
/// be listed as left out. `placed_points` is how many points the automation clips placed already, as the limit is
/// shared. Returns the moves, those patterns, and whether the song held too many points.
pub(super) fn place_edit_events(
    raw: &std::collections::BTreeMap<u32, Vec<u8>>,
    channel_names: &HashMap<u32, String>,
    items: &[PlaylistItem],
    tracks: &PlaylistTracks,
    mixer: &MixerSettings,
    ppq: u16,
    mut placed_points: usize,
) -> (Vec<FlAutomation>, Vec<u32>, bool) {
    let (mut result, mut left, mut cut) = (Vec::new(), Vec::new(), false);
    if ppq == 0 {
        return (result, raw.keys().copied().collect(), cut);
    }
    let ppq = ppq as f64;
    for (&pattern, data) in raw {
        let mut placements: Vec<PlaylistItem> = items
            .iter()
            .filter(|i| i.index == super::song::PATTERN_BASE + pattern)
            .map(|&i| PlaylistItem { muted: i.muted || tracks.at(i.slot).is_some_and(|t| t.1), ..i })
            .filter(|i| !i.muted)
            .collect();
        placements.sort_by_key(|i| i.at);
        // Each thing moved, in the order first met, with its points.
        let mut moved: Vec<(u32, Vec<FlPoint>)> = Vec::new();
        let (records, _) = data.as_chunks::<12>();
        for r in records {
            let (Some(tick), Some(target), Some(bits)) = (u32_le(r, 0), u32_le(r, 4), u32_le(r, 8)) else { continue };
            let (target, value) = match target & 0xFFFF {
                low @ FL8_PLUGIN..TARGET_PLUGIN => {
                    (target & 0xFFFF_0000 | (TARGET_PLUGIN + low - FL8_PLUGIN), bits as f64 / FL8_ONE)
                }
                _ => (target, f32::from_bits(bits) as f64),
            };
            if !value.is_finite() {
                continue;
            }
            let point = FlPoint { beat: tick as f64 / ppq, value: value.clamp(0.0, 1.0) };
            match moved.iter_mut().find(|(t, _)| *t == target) {
                Some((_, points)) => points.push(point),
                None => moved.push((target, vec![point])),
            }
        }
        let mut all_carried = !items.is_empty();
        for (target, mut points) in moved {
            let what = moves("", target, mixer, Vec::new());
            let Some(channel) = what.channel.filter(|_| what.control == FlControl::PluginParameter) else {
                all_carried = false;
                continue;
            };
            points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
            let mut parts = Vec::new();
            let (placed, starts) =
                place_curve(&points, &placements, ppq, false, true, &mut placed_points, &mut cut, Some(&mut parts));
            if placed.is_empty() {
                continue;
            }
            // Where a placement starts before an earlier one ends, it takes over from there, as in the curve.
            for k in 1..parts.len() {
                let next = parts.get(k).map_or(f64::INFINITY, |p| p.start);
                if let Some(part) = parts.get_mut(k - 1)
                    && part.start + part.length > next + SAME_BEAT
                {
                    part.length = next - part.start;
                    let length = part.length;
                    part.points.retain(|p| p.beat < length - SAME_BEAT);
                }
            }
            parts.retain(|p| !p.points.is_empty() && p.length > SAME_BEAT);
            let channel_name =
                channel_names.get(&channel).cloned().unwrap_or_else(|| format!("Channel {}", channel + 1));
            let name = format!("{channel_name} - setting {} (edit events)", what.parameter);
            result.push(FlAutomation { starts, parts, ..moves(&name, target, mixer, placed) });
        }
        if !all_carried {
            left.push(pattern);
        }
    }
    (result, left, cut)
}

fn u32_le(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// A curve (in beats from its clip's start) placed at each of `placements`, cut to each one's length, joined: the curve
/// across the song and the beats where each placement starts. `placed_points` counts the points held across all the
/// song's automation, and `cut` is set when it passes the limit. With `from_first`, nothing is placed before the
/// curve's first point (edit events leave a setting alone until the first one), rather than holding its value there.
/// `parts`, when given, gets each placement's own part of the curve, from its first point to where the placement ends,
/// timed from that first point (the same points as in the curve, so they don't count towards the limit again).
#[allow(clippy::too_many_arguments)]
fn place_curve(
    curve: &[FlPoint],
    placements: &[PlaylistItem],
    ppq: f64,
    hold: bool,
    from_first: bool,
    placed_points: &mut usize,
    cut: &mut bool,
    mut parts: Option<&mut Vec<FlPart>>,
) -> (Vec<FlPoint>, Vec<f64>) {
    let mut placed: Vec<FlPoint> = Vec::new();
    let mut starts = Vec::new();
    for item in placements {
        let mut part =
            cut_curve(curve, item.at as f64 / ppq, (item.offset as i64) as f64 / ppq, item.length as f64 / ppq, hold);
        if from_first && let Some(c) = curve.first() {
            let begins = (item.at as f64 + (c.beat * ppq) - (item.offset as i64) as f64) / ppq;
            part.retain(|p| p.beat >= begins - SAME_BEAT);
        }
        let (Some(&first), Some(&last)) = (part.first(), part.last()) else { continue };
        // Each placement keeps a copy of its part of the curve, so a clip placed very many times stops at the
        // limit (shared by all the song's automation, as it's all held at once).
        *placed_points = placed_points.saturating_add(part.len());
        if *placed_points > super::song::MAX_PLACED_NOTES {
            *cut = true;
            break;
        }
        starts.push(first.beat);
        if let Some(parts) = parts.as_deref_mut() {
            let end = (item.at as f64 + item.length as f64) / ppq;
            let points = part.iter().map(|p| FlPoint { beat: p.beat - first.beat, value: p.value }).collect();
            parts.push(FlPart { start: first.beat, length: end - first.beat, points });
        }
        // A clip placed over an earlier placement takes over from where it starts to where it ends; if the
        // earlier one runs on past that, it carries on from there.
        let mut rest = Vec::new();
        if let Some(pl) = placed.last().copied()
            && pl.beat > first.beat + SAME_BEAT
        {
            // Only the end of the curve so far is cut again, so a clip placed over and over costs what it
            // replaces, not the whole curve each time. The curve runs forward, so the points before that end are
            // the same either way.
            if pl.beat > last.beat + SAME_BEAT {
                let tail = placed.get(tail_from(&placed, last.beat)..).unwrap_or_default();
                rest = cut_curve(tail, last.beat, last.beat, pl.beat - last.beat, hold);
            }
            let k = keep_up_to(&placed, first.beat);
            let tail = placed.get(k..).unwrap_or_default();
            let from = tail.first().map_or(first.beat, |p| p.beat);
            let kept = cut_curve(tail, from, from, first.beat - from, hold);
            placed.truncate(k);
            placed.extend(kept);
        }
        // Between clips FL holds the value the last one ended on, then jumps to where the next starts.
        if let Some(&pl) = placed.last()
            && pl.beat < first.beat - SAME_BEAT
            && pl.value != first.value
        {
            placed.push(FlPoint { beat: first.beat, value: pl.value });
        }
        // A clip straight after another starts where that one ended: the shared point is kept once.
        if let Some(&pl) = placed.last()
            && (pl.beat - first.beat).abs() <= SAME_BEAT
            && pl.value == first.value
        {
            part.remove(0);
        }
        placed.extend(part);
        if let (Some(r), Some(pl)) = (rest.first(), placed.last())
            && r.value == pl.value
        {
            rest.remove(0);
        }
        placed.extend(rest);
    }
    (placed, starts)
}

/// Where to start cutting a forward-running curve again so that what's found at `beat` is what cutting all of it
/// finds: the last point well before it (or the start).
fn tail_from(curve: &[FlPoint], beat: f64) -> usize {
    curve.partition_point(|p| p.beat < beat - SAME_BEAT).saturating_sub(1)
}

/// Where to cut a forward-running curve that a clip placed at `beat` takes over from: a point well before it with no
/// other point on its beat after it (so cutting again from there starts on that same point), or the start.
fn keep_up_to(curve: &[FlPoint], beat: f64) -> usize {
    let mut k = tail_from(curve, beat);
    while k > 0 && curve.get(k + 1).is_some_and(|next| curve.get(k).is_some_and(|p| next.beat <= p.beat + SAME_BEAT)) {
        k -= 1;
    }
    k
}

/// A channel's on/off (its Channel Rack light) as automation switches it: each point's value holds until the next,
/// 0.5 and above is on, and before the first point it's at the first point's value.
pub(super) struct OnOff {
    points: Vec<FlPoint>,
    ppq: f64,
}

impl OnOff {
    pub fn new(points: Vec<FlPoint>, ppq: u16) -> OnOff {
        OnOff { points, ppq: ppq.max(1) as f64 }
    }

    /// How many points come at or before a beat, counting from the first while they do. The points are sorted by the
    /// tick each is on, so a search skips those on earlier ticks rather than walking every point for every note.
    fn upto(&self, beat: f64) -> usize {
        let tick = |b: f64| (b * self.ppq).round_ties_even();
        let start = self.points.partition_point(|p| tick(p.beat) < tick(beat));
        let rest = self.points.get(start..).unwrap_or_default();
        start + rest.iter().take_while(|p| p.beat <= beat + SAME_BEAT).count()
    }

    fn on(&self, beat: f64) -> bool {
        // The last point at or before the beat; before the first, the first. With no points at all, it stays on.
        let at = self.upto(beat).checked_sub(1).and_then(|i| self.points.get(i)).or(self.points.first());
        at.is_none_or(|p| p.value >= 0.5)
    }

    /// Whether it's `on` (or off) all the way from one tick to another.
    fn throughout(&self, from: i64, to: i64, on: bool) -> bool {
        let (a, b) = (from as f64 / self.ppq, to as f64 / self.ppq);
        let inside = self.points.get(self.upto(a)..).unwrap_or_default();
        self.on(a) == on && inside.iter().take_while(|p| p.beat < b - SAME_BEAT).all(|p| (p.value >= 0.5) == on)
    }

    /// A clip muted when the channel is off all the way through it, else with the notes that start while it's off
    /// muted (FL's switch stops notes starting, not sound).
    pub fn mute(&self, mut clip: FlClip) -> FlClip {
        if self.throughout(clip.start, clip.start + clip.length, false) {
            clip.muted = true;
        } else {
            for n in &mut clip.notes {
                if !n.muted && !self.on((clip.start + n.start) as f64 / self.ppq) {
                    n.muted = true;
                }
            }
        }
        clip
    }

    /// An audio clip muted when the channel is off all the way through it, else marked when it's off part of the way.
    pub fn mute_audio(&self, mut clip: FlAudioClip) -> FlAudioClip {
        if self.throughout(clip.start, clip.start + clip.length, false) {
            clip.muted = true;
        } else if !self.throughout(clip.start, clip.start + clip.length, true) {
            clip.partly_off = true;
        }
        clip
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(beat: f64, value: f64) -> FlPoint {
        FlPoint { beat, value }
    }

    #[test]
    fn a_cut_starts_and_ends_on_the_curve() {
        let curve = [p(0.0, 0.0), p(4.0, 1.0)];
        assert_eq!(cut_curve(&curve, 10.0, 1.0, 2.0, false), [p(10.0, 0.25), p(12.0, 0.75)]);
        assert_eq!(cut_curve(&curve, 10.0, 1.0, 2.0, true), [p(10.0, 0.0), p(12.0, 0.0)]);
        // A jump at the end is kept as a second point there.
        let jump = [p(0.0, 0.0), p(2.0, 0.5), p(2.0, 1.0)];
        assert_eq!(cut_curve(&jump, 0.0, 0.0, 2.0, false), [p(0.0, 0.0), p(2.0, 0.5), p(2.0, 1.0)]);
        assert!(cut_curve(&curve, 0.0, 0.0, 0.0, false).is_empty());
    }

    /// How clips placed over each other were joined before only the end of the curve was cut again: the whole curve
    /// so far was cut each time.
    fn place_whole(curve: &[FlPoint], parts: &[(f64, f64, f64)], hold: bool) -> Vec<FlPoint> {
        let mut placed: Vec<FlPoint> = Vec::new();
        for &(at, from, length) in parts {
            let mut part = cut_curve(curve, at, from, length, hold);
            let (Some(&first), Some(&last)) = (part.first(), part.last()) else { continue };
            let mut rest = Vec::new();
            if let (Some(&p0), Some(&pl)) = (placed.first(), placed.last())
                && pl.beat > first.beat + SAME_BEAT
            {
                if pl.beat > last.beat + SAME_BEAT {
                    rest = cut_curve(&placed, last.beat, last.beat, pl.beat - last.beat, hold);
                }
                placed = cut_curve(&placed, p0.beat, p0.beat, first.beat - p0.beat, hold);
            }
            if let Some(&pl) = placed.last()
                && pl.beat < first.beat - SAME_BEAT
                && pl.value != first.value
            {
                placed.push(FlPoint { beat: first.beat, value: pl.value });
            }
            if let Some(&pl) = placed.last()
                && (pl.beat - first.beat).abs() <= SAME_BEAT
                && pl.value == first.value
            {
                part.remove(0);
            }
            placed.extend(part);
            if let (Some(r), Some(pl)) = (rest.first(), placed.last())
                && r.value == pl.value
            {
                rest.remove(0);
            }
            placed.extend(rest);
        }
        placed
    }

    #[test]
    fn edit_events_give_a_part_for_each_placement_from_its_first_move_and_a_later_one_takes_over() {
        // Pattern 1's edit events on channel 0's setting 2: 0.25 one beat in, 0.75 three beats in (ppq 96).
        let record = |tick: u32, value: f32| {
            [tick.to_le_bytes(), (0x8002u32).to_le_bytes(), value.to_bits().to_le_bytes()].concat()
        };
        let raw = std::collections::BTreeMap::from([(1u32, [record(96, 0.25), record(288, 0.75)].concat())]);
        let pattern = super::super::song::PATTERN_BASE + 1;
        // Placed at beat 0 for 8 beats, and again at beat 2 (over the first) for 4 beats starting 2 beats in.
        let items = [
            PlaylistItem { index: pattern, at: 0, length: 768, offset: 0.0, muted: false, slot: 0 },
            PlaylistItem { index: pattern, at: 192, length: 384, offset: 192.0, muted: false, slot: 0 },
        ];
        let (moved, left, cut) = place_edit_events(
            &raw,
            &HashMap::new(),
            &items,
            &PlaylistTracks::default(),
            &MixerSettings::new(None),
            96,
            0,
        );
        assert!(left.is_empty() && !cut);
        type Part = (f64, f64, Vec<(f64, f64)>);
        let parts: Vec<Part> = moved[0]
            .parts
            .iter()
            .map(|p| (p.start, p.length, p.points.iter().map(|q| (q.beat, q.value)).collect()))
            .collect();
        // The first runs from its first move (beat 1) until the second takes over at beat 2. The second plays the
        // pattern from 2 beats in, where the moves are already sliding from 0.25 towards 0.75 (0.5 there).
        assert_eq!(parts.len(), 2);
        assert_eq!((parts[0].0, parts[0].1), (1.0, 1.0));
        assert_eq!(parts[0].2, [(0.0, 0.25)]);
        assert_eq!((parts[1].0, parts[1].1), (2.0, 4.0));
        assert_eq!(parts[1].2, [(0.0, 0.5), (1.0, 0.75), (4.0, 0.75)]);
    }

    #[test]
    fn cutting_only_the_end_again_places_clips_as_cutting_the_whole_curve_did() {
        let curve = [p(0.0, 0.0), p(1.0, 1.0), p(1.0, 0.25), p(2.5, 0.75), p(4.0, 0.5)];
        let ppq = 96u16;
        for hold in [false, true] {
            // A spread of placements over and inside each other, at whole and half beats.
            let mut items = Vec::new();
            for i in 0..40i64 {
                let at = (i * 37 % 23) * 48;
                let length = (1 + i * 13 % 7) * 48;
                let offset = (i * 5 % 4) as f64 * 48.0;
                items.push(PlaylistItem { index: 0, at, length, offset, muted: false, slot: 0 });
            }
            let mut sorted = items.clone();
            sorted.sort_by_key(|i| i.at);
            let parts: Vec<(f64, f64, f64)> =
                sorted.iter().map(|i| (i.at as f64 / 96.0, i.offset / 96.0, i.length as f64 / 96.0)).collect();
            let expected = place_whole(&curve, &parts, hold);
            let name = if hold { "Mute" } else { "Volume" };
            let target = if hold { TARGET_SLOT_ON_OFF } else { TARGET_INSERT_VOLUME };
            let mixer = MixerSettings::new(None);
            let held = matches!(
                moves(name, target, &mixer, Vec::new()).control,
                FlControl::OnOff | FlControl::InsertSlotOnOff
            );
            let (placed, cut, _) = place_automation(
                &[(0, target)],
                &HashMap::from([(0, curve.to_vec())]),
                &HashMap::from([(0, name.to_owned())]),
                &items,
                &PlaylistTracks::default(),
                &mixer,
                ppq,
            );
            assert!(!cut);
            let expected = if held == hold { expected } else { place_whole(&curve, &parts, held) };
            assert_eq!(placed[0].points, expected, "hold {held}");
        }
    }

    #[test]
    fn the_mixers_base_comes_from_the_file() {
        // FL 25: base 0x7000. The master's fader, insert 2's fader and the on/off of insert 2's slot 3.
        let mut d = Vec::new();
        for (high, setting, value) in [
            (0x7000u32, TARGET_INSERT_VOLUME, 12800i32),
            (0x7000 + 128, TARGET_INSERT_VOLUME, 9000),
            (0x7000 + 131, TARGET_SLOT_ON_OFF, 0),
        ] {
            d.extend(0u32.to_le_bytes());
            d.extend((high << 16 | setting).to_le_bytes());
            d.extend(value.to_le_bytes());
        }
        let m = MixerSettings::new(Some(&d));
        assert_eq!(m.value(2, 0, TARGET_INSERT_VOLUME), Some(9000));
        assert_eq!(m.value(2, 3, TARGET_SLOT_ON_OFF), Some(0));
        assert_eq!(m.value(1, 0, TARGET_INSERT_VOLUME), None);
        assert_eq!(m.slot(0x7000 + 131), Some((2, 3)));
        assert_eq!(m.slot(0x7000 + 64), None, "insert 1 isn't in the file");
        assert_eq!(m.slot(0x0003), None, "a channel");
    }
}
