//! The sounds made inside one song: each plugin's saved settings with where it sits, whether it plays, and the part
//! it plays. FL Studio and Bitwig saves can be listed; Studio One's can't (its reader doesn't take out plugin states).
//!
//! The page lists many songs, so an entry's settings can be let go once it's listed ([`SongPreset::forget_data`]);
//! exporting reads the save again.

use std::collections::BTreeMap;

use mo_formats::bitwig::Project;
use mo_formats::fl::events;
use mo_formats::fl::presets::extract_presets;
use mo_formats::fl::song::{FlSong, read_song};
use mo_formats::presets::PresetBlob;
use mo_formats::{MidiClip, NoteEvent};

use crate::role::{Basis, Guess, Part, Role, Shift, Thresholds, guess};

/// One sound made inside a song.
#[derive(Clone, Debug, PartialEq)]
pub struct SongPreset {
    /// The plugin's settings, as the reader took them out (see `mo_formats::presets::PresetBlob`).
    pub blob: PresetBlob,
    /// How big the settings are, kept when the bytes themselves are let go.
    pub size: usize,
    /// The channel or track it's on, as the page shows it ("Bass", or for an FL effect the insert's place).
    pub track: String,
    /// An instrument whose channel or track plays no notes (the page tucks these away, as the C# Presets window did
    /// with FL synths nobody played).
    pub plays_nothing: bool,
    pub guess: Guess,
    /// The fingerprint a fix to its part is kept by ([`crate::fixes::fix_key`]: the plugin, its settings and the notes
    /// it plays), kept when the bytes are let go.     /// fix to its part is kept by it, so it holds for every copy of the sound in every save.
    pub hash: String,
    /// The part you set, which wins over [`SongPreset::guess`]; the guess stays, so taking the fix away shows it again.
    pub fixed: Option<Role>,
}

impl SongPreset {
    fn new(blob: PresetBlob, track: String, plays_nothing: bool, guess: Guess, parts: &[&[NoteEvent]]) -> SongPreset {
        let hash = crate::fixes::fix_key(&blob.plugin, &blob.data, parts);
        SongPreset { size: blob.data.len(), blob, track, plays_nothing, guess, hash, fixed: None }
    }

    /// The part shown: yours when you set one, else the guess.
    pub fn role(&self) -> Option<Role> {
        self.fixed.or(self.guess.role)
    }

    /// Lets go of the settings' bytes, keeping everything the list shows.
    pub fn forget_data(&mut self) {
        self.blob.data = Vec::new();
    }

    /// Who made the plugin, as the page's filter sorts it.
    pub fn native(&self) -> bool {
        self.blob.native
    }

    /// What says which preset this is within its save, so it can be found again there (to export it): the place it
    /// sits and the plugin.
    /// An FL instrument's channel number goes in too: FL names a new channel after its plugin, so two "Serum"
    /// channels are common and each must be told apart. (A fix to its part is kept by [`SongPreset::hash`], not this.)
    pub fn place_key(&self) -> String {
        let place = if self.blob.place.is_empty() { &self.track } else { &self.blob.place };
        let channel = self.blob.channel.map(|c| format!("#{c}")).unwrap_or_default();
        format!("{}{channel}|{}", place.to_lowercase(), self.blob.plugin.to_lowercase())
    }
}

/// The sounds in an FL Studio project's bytes, with their parts guessed. `None` when it isn't an FL project.
pub fn read_fl(raw: &[u8], limits: &Thresholds) -> Option<Vec<SongPreset>> {
    let flp = events::load(raw).ok()?;
    let song = read_song(&flp);
    Some(from_fl(&song, extract_presets(&flp), limits))
}

/// The insert an FL effect's place names ("Insert 3 · Vocals" is 3, "Master" or a renamed "Master · Bus" 0), as
/// `extract_presets` writes it.
fn insert_of(place: &str) -> Option<u32> {
    if place == "Master" || place.starts_with("Master · ") {
        return Some(0);
    }
    let rest = place.strip_prefix("Insert ")?;
    rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// FL Studio: instruments get their channel's guess; an effect gets the part of the channels feeding its insert
/// (the one with the most notes when they play different parts).
pub fn from_fl(song: &FlSong, presets: Vec<PresetBlob>, limits: &Thresholds) -> Vec<SongPreset> {
    // Each channel's guess, once.
    let mut by_channel: BTreeMap<u32, (Guess, usize, &str)> = BTreeMap::new();
    let mut notes_of: BTreeMap<u32, &[NoteEvent]> = BTreeMap::new();
    for c in &song.channels {
        let preset_name = c.instrument.as_ref().and_then(|i| i.state.as_ref()).map_or("", |s| s.name.as_str());
        let plugin = c.instrument.as_ref().map_or("", |i| i.name.as_str());
        let names = [c.name.as_str(), preset_name, plugin];
        // A channel switched off in the Channel Rack plays nothing, whatever its clips hold.
        let notes: &[NoteEvent] = if c.enabled { &c.notes } else { &[] };
        let part = Part { notes, ppq: song.ppq, shift: Shift::fl_channel(c.root_key, c.pitch_cents), names: &names };
        let playing = notes.iter().filter(|n| !n.muted).count();
        by_channel.insert(c.index, (guess(&part, limits), playing, c.name.as_str()));
        notes_of.insert(c.index, notes);
    }

    presets
        .into_iter()
        .map(|blob| {
            if let Some(ch) = blob.channel {
                let (g, playing, name) = by_channel
                    .get(&ch)
                    .map(|(g, p, n)| (g.clone(), *p, (*n).to_owned()))
                    .unwrap_or_else(|| (Guess::none("Its channel wasn't found"), 0, blob.name.clone()));
                let plays_nothing = playing == 0;
                let notes = notes_of.get(&ch).copied().unwrap_or_default();
                return SongPreset::new(blob, name, plays_nothing, g, &[notes]);
            }
            let insert = insert_of(&blob.place);
            // What it hears: the notes of every channel feeding its insert (none for the master).
            let heard: Vec<&[NoteEvent]> = match insert {
                Some(at) if at > 0 => song
                    .channels
                    .iter()
                    .filter(|c| c.insert == at)
                    .filter_map(|c| notes_of.get(&c.index).copied())
                    .collect(),
                _ => Vec::new(),
            };
            let g = match insert {
                Some(0) => Guess::none("On the master, so it hears the whole song"),
                Some(at) => {
                    let feeding: Vec<_> = song
                        .channels
                        .iter()
                        .filter(|c| c.insert == at)
                        .filter_map(|c| by_channel.get(&c.index))
                        .collect();
                    track_guess(&feeding, &format!("Insert {at}"))
                }
                None => Guess::none("Where it sits wasn't found"),
            };
            let track = if blob.place.is_empty() { blob.name.clone() } else { blob.place.clone() };
            // An effect plays no notes of its own, so it's never "unused".
            SongPreset::new(blob, track, false, g, &heard)
        })
        .collect()
}

/// An effect's guess from the parts feeding where it sits: the role with the most notes.
fn track_guess(feeding: &[&(Guess, usize, &str)], place: &str) -> Guess {
    let mut notes_by_role: BTreeMap<Role, (usize, Vec<&str>)> = BTreeMap::new();
    for (g, playing, name) in feeding {
        if let Some(role) = g.role
            && *playing > 0
        {
            let e = notes_by_role.entry(role).or_default();
            e.0 += playing;
            e.1.push(name);
        }
    }
    let Some((role, (_, names))) = notes_by_role.iter().max_by_key(|(r, (n, _))| (*n, std::cmp::Reverse(**r))) else {
        return Guess::none(if feeding.is_empty() {
            format!("Nothing plays into {place}")
        } else {
            format!("What plays into {place} gives no guess")
        });
    };
    let mut reason = format!("Sits on {place}, fed by {}", names.join(", "));
    if notes_by_role.len() > 1 {
        // The other parts too, most notes first.
        let mut others: Vec<_> = notes_by_role.iter().filter(|(r, _)| *r != role).collect();
        others.sort_by_key(|(r, (n, _))| (std::cmp::Reverse(*n), **r));
        let others: Vec<String> =
            others.iter().map(|(r, (_, ns))| format!("{} ({})", ns.join(", "), r.label().to_lowercase())).collect();
        reason += &format!(" ({} plays the most notes there), and {}", role.label().to_lowercase(), others.join(", "));
    }
    // Only as sure as what it took its part from.
    let weak = notes_by_role.len() > 1 || feeding.iter().any(|(g, _, _)| g.role == Some(*role) && g.weak);
    let octave_checked = feeding.iter().filter(|(g, _, _)| g.role == Some(*role)).all(|(g, _, _)| g.octave_checked);
    Guess { role: Some(*role), basis: Basis::Track, reason, octave_checked, weak }
}

/// Bitwig: every preset on a track (instrument or effect) gets that track's guess. The reader names a preset
/// "Track - Preset" and its clips by track, so they're matched by the track's name. Note Transpose devices aren't read
/// yet, so the guess says the transpose wasn't checked.
pub fn from_bitwig(project: &Project, limits: &Thresholds) -> Vec<SongPreset> {
    from_bitwig_parts(project.presets(), &project.midi(), limits)
}

/// [`from_bitwig`], from what the reader took out.
pub fn from_bitwig_parts(presets: Vec<PresetBlob>, clips: &[MidiClip], limits: &Thresholds) -> Vec<SongPreset> {
    // Each track's notes, its clips laid one after another so they don't overlap, and where the last one ends.
    let mut tracks: Vec<(String, Vec<NoteEvent>, u16)> = Vec::new();
    let mut ends: Vec<i64> = Vec::new();
    for clip in clips {
        for (name, notes) in &clip.tracks {
            let at = match tracks.iter().position(|(n, _, _)| n == name) {
                Some(at) => at,
                None => {
                    tracks.push((name.clone(), Vec::new(), clip.ppq));
                    ends.push(0);
                    tracks.len() - 1
                }
            };
            let (Some((_, all, ppq)), Some(end)) = (tracks.get_mut(at), ends.get_mut(at)) else { continue };
            let scale =
                |t: i64| if clip.ppq == *ppq || clip.ppq == 0 { t } else { t * i64::from(*ppq) / i64::from(clip.ppq) };
            let offset = *end;
            all.extend(notes.iter().map(|n| NoteEvent {
                start: offset + scale(n.start),
                length: scale(n.length),
                ..*n
            }));
            *end = notes.iter().map(|n| offset + scale(n.start) + scale(n.length).max(0)).fold(*end, i64::max);
        }
    }
    // Longest names first, so "Bass 2" isn't taken for "Bass".
    let mut by_length: Vec<usize> = (0..tracks.len()).collect();
    by_length.sort_by_key(|&i| std::cmp::Reverse(tracks.get(i).map_or(0, |t| t.0.len())));

    let mut guesses: BTreeMap<String, Guess> = BTreeMap::new();
    presets
        .into_iter()
        .map(|blob| {
            let track = by_length
                .iter()
                .filter_map(|&i| tracks.get(i))
                .find(|(name, _, _)| blob.name.strip_prefix(name.as_str()).is_some_and(|r| r.starts_with(" - ")));
            let shift = Shift { semitones: 0.0, plugin: None, transpose_checked: false };
            let (track_name, notes, ppq) = match track {
                Some((name, notes, ppq)) => (name.clone(), notes.as_slice(), *ppq),
                None => (blob.name.split(" - ").next().unwrap_or_default().to_owned(), &[][..], 960),
            };
            let g = guesses
                .entry(track_name.clone())
                .or_insert_with(|| {
                    let names = [track_name.as_str(), blob.name.as_str(), blob.plugin.as_str()];
                    guess(&Part { notes, ppq, shift, names: &names }, limits)
                })
                .clone();
            // Only an instrument can be unused; the Bitwig reader doesn't say which presets are instruments yet, so an
            // effect on an audio track isn't taken for an unused synth.
            let plays_nothing = blob.kind == mo_formats::PluginKind::Instrument && notes.iter().all(|n| n.muted);
            SongPreset::new(blob, track_name, plays_nothing, g, &[notes])
        })
        .collect()
}

/// Your fixes win over every guess: `fixes` maps a [`SongPreset::hash`] to the part you set (see
/// [`crate::fixes::RoleFixes`]). A sound with no fix there loses any fix it had, so this also shows a cleared one.
pub fn apply_fixes(presets: &mut [SongPreset], fixes: &std::collections::HashMap<String, Role>) {
    for p in presets {
        p.fixed = fixes.get(&p.hash).copied();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_insert_from_an_effects_place() {
        assert_eq!(insert_of("Master"), Some(0));
        assert_eq!(insert_of("Insert 3"), Some(3));
        assert_eq!(insert_of("Insert 12 · Vocals"), Some(12));
        assert_eq!(insert_of(""), None);
        assert_eq!(insert_of("Inserted"), None);
        assert_eq!(insert_of("Master · Bus"), Some(0), "a renamed master");
    }
}
