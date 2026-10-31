//! An FL channel's clips as another DAW's track holds them (the C# `FlClips`).

use std::collections::HashMap;

use mo_formats::NoteEvent;
use mo_formats::fl::song::{FlAudioClip, FlChannel, FlClip};

/// A channel's clips, in order, as one track can hold them: clips that overlap in FL (on different playlist tracks)
/// become one, where notes from a muted clip are muted and a note both play is there once. A channel with notes but no
/// clips gets one from the start of the song holding its notes, in whole bars.
pub fn of(channel: &FlChannel, clips: Vec<FlClip>, ppq: u16) -> Vec<FlClip> {
    if clips.is_empty() {
        let Some(end) = channel.notes.iter().map(|n| n.start.saturating_add(n.length)).max() else {
            return Vec::new();
        };
        let bar = 4 * ppq.max(1) as i64;
        let length = bar.max(end.saturating_add(bar - 1) / bar * bar);
        return vec![FlClip {
            name: channel.name.clone(),
            start: 0,
            length,
            notes: channel.notes.clone(),
            muted: false,
            track: 0,
            on_muted_track: false,
        }];
    }
    let mut sorted = clips;
    // A stable sort, as C#'s OrderBy is: clips starting together stay in the playlist's order.
    sorted.sort_by_key(|x| x.start);
    let mut out: Vec<FlClip> = Vec::with_capacity(sorted.len());
    for clip in sorted {
        let Some(last) = out.last_mut().filter(|last| clip.start < last.start.saturating_add(last.length)) else {
            out.push(clip);
            continue;
        };
        let muted = last.muted && clip.muted;
        let part = |x: &FlClip, from: i64| {
            x.notes
                .iter()
                .map(|n| NoteEvent {
                    start: x.start.saturating_sub(from).saturating_add(n.start),
                    muted: n.muted || x.muted && !muted,
                    ..*n
                })
                .collect::<Vec<_>>()
        };
        let mut notes = part(last, last.start);
        notes.extend(part(&clip, last.start));
        // Unmuted first where two land on one key at one tick, so the one kept plays when either clip plays it.
        notes.sort_by_key(|n| (n.start, n.key, n.muted));
        notes.dedup_by_key(|n| (n.start, n.key));
        let end = last.start.saturating_add(last.length).max(clip.start.saturating_add(clip.length));
        last.length = end - last.start;
        last.notes = notes;
        last.muted = muted;
    }
    out
}

/// Whether all of a track's clips sit on muted FL playlist tracks, so the track is muted rather than they are.
pub fn all_on_muted_tracks(clips: &[FlClip]) -> bool {
    !clips.is_empty() && clips.iter().all(|x| x.on_muted_track)
}

/// [`all_on_muted_tracks`] for audio clips.
pub fn all_audio_on_muted_tracks(clips: &[FlAudioClip]) -> bool {
    !clips.is_empty() && clips.iter().all(|x| x.on_muted_track)
}

/// A track's clips with those on muted FL playlist tracks muted, unless the track itself is muted for them
/// (`track_muted`), when they keep only their own mutes.
pub fn on_muted_tracks(clips: &[FlClip], track_muted: bool) -> Vec<FlClip> {
    clips.iter().map(|x| FlClip { muted: x.muted || x.on_muted_track && !track_muted, ..x.clone() }).collect()
}

/// [`on_muted_tracks`] for audio clips.
pub fn audio_on_muted_tracks(clips: &[FlAudioClip], track_muted: bool) -> Vec<FlAudioClip> {
    clips.iter().map(|x| FlAudioClip { muted: x.muted || x.on_muted_track && !track_muted, ..x.clone() }).collect()
}

/// Gives steps from FL's step sequencer, which have no length, the length FL plays them for: FL plays a step until its
/// sound ends. Here that's until the next note on the same key or the end of the clip, and no longer than `longest`
/// gives for the key (how long its sample plays), when it knows.
pub fn lengthen(notes: Vec<NoteEvent>, clip_length: i64, longest: Option<&dyn Fn(u8) -> i64>) -> Vec<NoteEvent> {
    if !notes.iter().any(|n| n.step) {
        return notes;
    }
    // Each key's starts in order, so the next is found by halves: a whole song's steps can all be on one key.
    let mut starts: HashMap<u8, Vec<i64>> = HashMap::new();
    for n in &notes {
        starts.entry(n.key).or_default().push(n.start);
    }
    for on in starts.values_mut() {
        on.sort_unstable();
        on.dedup();
    }
    notes
        .into_iter()
        .map(|n| {
            if !n.step {
                return n;
            }
            let on = starts.get(&n.key).map(Vec::as_slice).unwrap_or_default();
            // Always there: it's this note's own start.
            let next = on.partition_point(|&s| s <= n.start);
            let next = on.get(next).copied().unwrap_or(clip_length);
            let mut length = next.min(clip_length).saturating_sub(n.start);
            if let Some(most) = longest {
                length = length.min(most(n.key));
            }
            NoteEvent { length: n.length.max(length), ..n }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_formats::NoteAttribute;

    fn note(start: i64, length: i64, key: u8) -> NoteEvent {
        NoteEvent { start, length, key, velocity: 50_000, muted: false, step: false, attribute: NoteAttribute::NONE }
    }

    fn clip(start: i64, length: i64, notes: Vec<NoteEvent>, muted: bool) -> FlClip {
        FlClip { name: "P".into(), start, length, notes, muted, track: 1, on_muted_track: false }
    }

    fn channel() -> FlChannel {
        FlChannel {
            index: 0,
            name: "Lead".into(),
            instrument: None,
            sample: String::new(),
            insert: 0,
            notes: vec![note(0, 96, 60), note(400, 96, 62)],
            root_key: 60,
            pitch_cents: 0,
            volume: 0.78,
            pan: 0.5,
            clips: Vec::new(),
            volume_envelope: None,
            audio_clips: Vec::new(),
            stretch: Default::default(),
            enabled: true,
            switched_by: Vec::new(),
        }
    }

    #[test]
    fn a_channel_without_clips_gets_one_in_whole_bars() {
        let c = channel();
        let made = of(&c, Vec::new(), 96);
        assert_eq!((made.len(), made[0].start, made[0].length, made[0].notes.len()), (1, 0, 768, 2));
    }

    #[test]
    fn overlapping_clips_become_one_and_a_note_both_play_is_there_once() {
        let a = clip(0, 384, vec![note(0, 96, 60), note(192, 96, 60)], false);
        let mut quiet = note(0, 96, 60);
        quiet.muted = true;
        // Starts at 192, so its note at 0 lands on the first clip's note at 192.
        let b = clip(192, 384, vec![quiet, note(96, 96, 64)], true);
        let made = of(&channel(), vec![b, a], 96);
        assert_eq!(made.len(), 1);
        assert_eq!((made[0].start, made[0].length, made[0].muted), (0, 576, false));
        let got: Vec<_> = made[0].notes.iter().map(|n| (n.start, n.key, n.muted)).collect();
        // The muted clip's other note is muted, as it would be silent in FL.
        assert_eq!(got, [(0, 60, false), (192, 60, false), (288, 64, true)]);
    }

    #[test]
    fn clips_that_only_touch_stay_apart() {
        let made = of(&channel(), vec![clip(0, 384, vec![], false), clip(384, 384, vec![], true)], 96);
        assert_eq!(made.iter().map(|c| (c.start, c.muted)).collect::<Vec<_>>(), [(0, false), (384, true)]);
    }

    #[test]
    fn steps_last_until_the_next_note_on_their_key_or_the_clip_end() {
        let mut a = note(0, 0, 60);
        a.step = true;
        let mut b = note(96, 0, 60);
        b.step = true;
        let mut c = note(0, 0, 62);
        c.step = true;
        let got = lengthen(vec![a, b, c, note(48, 10, 60)], 384, None);
        assert_eq!(got.iter().map(|n| n.length).collect::<Vec<_>>(), [48, 288, 384, 10]);
        let short = |_: u8| 30;
        let got = lengthen(vec![a], 384, Some(&short));
        assert_eq!(got[0].length, 30, "no longer than its sample plays");
    }

    #[test]
    fn a_track_with_all_its_clips_on_muted_tracks_is_muted_rather_than_they_are() {
        let mut on = clip(0, 96, vec![], false);
        on.on_muted_track = true;
        assert!(all_on_muted_tracks(std::slice::from_ref(&on)));
        assert!(!all_on_muted_tracks(&[]));
        assert!(!on_muted_tracks(std::slice::from_ref(&on), true)[0].muted);
        assert!(on_muted_tracks(std::slice::from_ref(&on), false)[0].muted);
    }
}
