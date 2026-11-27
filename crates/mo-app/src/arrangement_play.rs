//! Playing a song's MIDI from its arrangement on the Songs page (Solomon, 27 Nov 2026: "should be able to play the
//! midi on the arranger view so i can hear if an idea is worth working on"). The song's own plugins can't load here,
//! so each instrument plays with a General MIDI sound matched to it the way the MIDI page matches a part's
//! (`mo_midi::gm::sound_for`: its name, the instrument its name or plugin says, then the kind of part its notes
//! play), and you can pick another for it, remembered for the song ([`YourSounds`]).
//!
//! The screen logic and every word it shows; no window and no synth in it, so all of it is tested here. mo-ui makes
//! the sound (`mo_audio::gm::SongSound`, made as it plays) from what [`Voice::plays`] gives.
//!
//! What plays is what the arrangement view on show draws: every instrument track's clips with their notes, as the
//! DAW plays them. Muted tracks, muted clips and muted notes stay quiet, and so do a Studio One track's other takes;
//! audio clips and automation aren't played. An FL Studio pattern holds a part for each channel in it
//! ([`PreviewClip::parts`]): each channel is an instrument of its own, so a kick, a bass and chords in one pattern
//! each get their own sound.
//!
//! A drum sample played from one key (an FL Studio channel holding a kick plays C5 over and over) would sound as a
//! drum kit's C5, which is a bongo: the hit its name says plays instead ([`hit_named`]: "Kick" plays the kit's kick).

use std::borrow::Cow;
use std::collections::HashMap;

use mo_formats::NoteEvent;
use mo_formats::arrangement::{Arrangement, LaneKind, PreviewClip};
use mo_formats::midi::write::lengthen_steps;
use mo_midi::gm::{GmSound, PROGRAM_NAMES};
use mo_midi::{PPQ, Role};
use serde_json::Value;

/// The app's setting your sounds are kept in ([`YourSounds`]).
pub const SOUNDS_KEY: &str = "arrangement_sounds";

/// How many songs' sounds are kept: the ones changed last.
pub const SONGS_KEPT: usize = 300;

/// What plays an instrument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sound {
    /// A General MIDI sound, 0 to 127.
    Program(u8),
    /// The drum kit, each note the kit's own key.
    Kit,
    /// One hit of the drum kit (a GM drum key) for every note, whatever key it's on: for a drum sample played from
    /// one key.
    Hit(u8),
}

impl Sound {
    /// What the sound is called ("Electric Piano 1", "Drum kit", "Clap").
    pub fn name(self) -> &'static str {
        match self {
            Sound::Program(p) => PROGRAM_NAMES[usize::from(p & 0x7f)],
            Sound::Kit => "Drum kit",
            Sound::Hit(key) => HITS.iter().find(|h| h.1 == key).map_or("Drum hit", |h| h.0),
        }
    }

    /// As the setting keeps it: "p4", "kit", "h36".
    pub fn code(self) -> String {
        match self {
            Sound::Program(p) => format!("p{p}"),
            Sound::Kit => "kit".to_owned(),
            Sound::Hit(key) => format!("h{key}"),
        }
    }

    /// From [`Sound::code`]; none for what isn't one.
    pub fn from_code(code: &str) -> Option<Sound> {
        if code == "kit" {
            return Some(Sound::Kit);
        }
        if let Some(p) = code.strip_prefix('p') {
            return p.parse::<u8>().ok().filter(|p| *p < 128).map(Sound::Program);
        }
        let key = code.strip_prefix('h')?.parse::<u8>().ok()?;
        HITS.iter().any(|h| h.1 == key).then_some(Sound::Hit(key))
    }
}

/// The drum kit's single hits there are to pick, by name and GM drum key, in the order the menu shows them.
pub const HITS: [(&str, u8); 13] = [
    ("Kick", 36),
    ("Snare", 38),
    ("Rim", 37),
    ("Clap", 39),
    ("Closed hat", 42),
    ("Open hat", 46),
    ("Tom", 45),
    ("Crash", 49),
    ("Ride", 51),
    ("Tambourine", 54),
    ("Cowbell", 56),
    ("Shaker", 70),
    ("Percussion", 75),
];

/// The words that name a hit, looked for in this order (an open hat before a hat): each word whole, or a name's
/// word that starts with it ("Kicks 2", "Snares").
const HIT_WORDS: &[(&str, u8)] = &[
    ("kick", 36),
    ("kik", 36),
    ("bd", 36),
    ("bassdrum", 36),
    ("rim", 37),
    ("rimshot", 37),
    ("snare", 38),
    ("snr", 38),
    ("sd", 38),
    ("clap", 39),
    ("clp", 39),
    ("snap", 39),
    ("ohh", 46),
    ("oh", 46),
    ("open", 46),
    ("chh", 42),
    ("hh", 42),
    ("hat", 42),
    ("hihat", 42),
    ("hats", 42),
    ("tom", 45),
    ("crash", 49),
    ("cymb", 49),
    ("cymbal", 49),
    ("ride", 51),
    ("tamb", 54),
    ("tambourine", 54),
    ("cowbell", 56),
    ("shaker", 70),
    ("shk", 70),
    ("shkr", 70),
    ("perc", 75),
];

/// Words that start with one of these name a hit too ("Kicks", "Snares", "Claps", "Shakers"); shorter ones must
/// be the whole word ("oh" isn't "ohm").
const HIT_STEMS: &[&str] = &["kick", "snare", "clap", "tamb", "shaker", "perc", "crash", "cymb"];

/// The drum hit a name says ("Kick 2" the kick, "Chh 1 Select" the closed hat), as its GM drum key.
pub fn hit_named(name: &str) -> Option<u8> {
    let lowered = mo_formats::case::to_lower(name);
    let words: Vec<&str> = lowered.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    // "Open hat" is an open hat, not a hat: the first word of the list that's there wins, not the first in the name.
    HIT_WORDS
        .iter()
        .find(|(word, _)| {
            words.iter().any(|w| {
                let w = w.trim_end_matches(|c: char| c.is_ascii_digit());
                w == *word || (HIT_STEMS.contains(word) && w.starts_with(word))
            })
        })
        .map(|(_, key)| *key)
}

/// One instrument of the song: what it's called, the notes it plays (ticks at [`PPQ`] from the song's start, by
/// where they start), and the sound matched to it.
#[derive(Clone, Debug, PartialEq)]
pub struct Voice {
    /// Its track's name, or its FL Studio channel's.
    pub name: String,
    /// Its track's colour (0xRRGGBB), when it has one.
    pub colour: Option<u32>,
    pub notes: Vec<NoteEvent>,
    pub matched: Sound,
    /// Why that one, as a tooltip says it ("from its name: rhodes").
    pub why: String,
}

impl Voice {
    /// The notes it plays with `sound`, and the GM program and whether they go to the drum kit: a single hit plays
    /// each note on its key.
    pub fn plays(&self, sound: Sound) -> (Cow<'_, [NoteEvent]>, u8, bool) {
        match sound {
            Sound::Program(p) => (Cow::Borrowed(&self.notes), p & 0x7f, false),
            Sound::Kit => (Cow::Borrowed(&self.notes), 0, true),
            Sound::Hit(key) => (Cow::Owned(self.notes.iter().map(|n| NoteEvent { key, ..*n }).collect()), 0, true),
        }
    }
}

/// The longest a step from FL Studio's step sequencer plays (it has no length of its own: FL plays its sample to
/// its end), in ticks: a beat, or to the next note on its key when that comes first.
const STEP_LONGEST: i64 = PPQ;

/// Every instrument the arrangement plays, each with the notes it plays and a sound matched to it, in the order
/// their tracks come. Instruments named alike (two tracks called "Keys") play as one, with one sound. None for a
/// song with no notes that play.
pub fn voices(a: &Arrangement) -> Vec<Voice> {
    let mut order: Vec<String> = Vec::new();
    let mut by_name: HashMap<String, (Option<u32>, String, Vec<NoteEvent>)> = HashMap::new();
    let ticks = |beats: f64| (beats * PPQ as f64).round() as i64;
    for lane in &a.lanes {
        if lane.muted || lane.disabled || lane.layer || matches!(lane.kind, LaneKind::Automation | LaneKind::Group) {
            continue;
        }
        for clip in lane.clips.iter().filter(|c| !c.muted && !c.notes.is_empty()) {
            let (start, end) = (ticks(clip.start), ticks(clip.end()));
            for (part, notes) in clip_parts(clip) {
                let name = clip.parts.get(usize::from(part)).filter(|_| !clip.parts.is_empty()).unwrap_or(&lane.name);
                let placed: Vec<NoteEvent> = notes
                    .filter(|n| !n.muted && n.start >= 0.0 && n.start < clip.length)
                    .filter_map(|n| {
                        let on = start + ticks(n.start);
                        let off = (start + ticks(n.start + n.length)).min(end);
                        (on < end).then_some(NoteEvent {
                            start: on,
                            length: (off - on).max(1),
                            key: n.key.min(127),
                            velocity: n.velocity,
                            muted: false,
                            step: n.step,
                            attribute: mo_formats::NoteAttribute::NONE,
                        })
                    })
                    .collect();
                if placed.is_empty() {
                    continue;
                }
                let placed = lengthen_steps(placed, end, Some(&|_| STEP_LONGEST));
                let entry = by_name.entry(name.clone()).or_insert_with(|| {
                    order.push(name.clone());
                    (lane.colour, lane.detail.clone(), Vec::new())
                });
                entry.2.extend(placed);
            }
        }
    }
    order
        .into_iter()
        .filter_map(|name| {
            let (colour, detail, mut notes) = by_name.remove(&name)?;
            notes.sort_by_key(|n| (n.start, n.key));
            let (matched, why) = matched(&name, &detail, &notes);
            Some(Voice { name, colour, notes, matched, why })
        })
        .collect()
}

/// A clip's notes, one list for each of its parts (one, for a clip that has none).
fn clip_parts(clip: &PreviewClip) -> Vec<(u16, impl Iterator<Item = &mo_formats::arrangement::PreviewNote>)> {
    let mut parts: Vec<u16> = clip.notes.iter().map(|n| if clip.parts.is_empty() { 0 } else { n.part }).collect();
    parts.sort_unstable();
    parts.dedup();
    parts
        .into_iter()
        .map(|part| (part, clip.notes.iter().filter(move |n| clip.parts.is_empty() || n.part == part)))
        .collect()
}

/// The sound for an instrument called `name`, played by `detail` (its plugin or sample, as the track says), that
/// plays `notes`: as the MIDI page matches a part's, and why.
fn matched(name: &str, detail: &str, notes: &[NoteEvent]) -> (Sound, String) {
    let shape = mo_midi::analyser::Shape::of(notes);
    let hints = format!("{name} {detail}");
    let (role, _) = mo_midi::analyser::role(&shape, notes, &hints, false);
    let family = mo_midi::instruments::guess([name, detail], false, None).map(|guess| guess.family);
    let drummy = role == Role::Drums || family.is_some_and(|f| mo_midi::gm::for_family(f).is_none());
    let one_key = notes.windows(2).all(|w| w[0].key == w[1].key);
    if drummy || (one_key && hit_named(name).is_some()) {
        // A drum sample played from one key: the hit its name says. A whole kit's part keeps its keys.
        if one_key && let Some(key) = hit_named(name) {
            return (Sound::Hit(key), format!("a drum sample: {} by its name", Sound::Hit(key).name().to_lowercase()));
        }
        return (Sound::Kit, "a drum part".to_owned());
    }
    let family = family.filter(|f| mo_midi::gm::for_family(*f).is_some());
    let sound: GmSound = mo_midi::gm::sound_for(role, false, None, &[name, detail], family);
    (Sound::Program(sound.program), sound.why_text())
}

/// The sound each instrument plays with: yours where you picked one, else the one matched to it.
pub fn sound_of(voice: &Voice, yours: &HashMap<String, Sound>) -> Sound {
    yours.get(&voice.name).copied().unwrap_or(voice.matched)
}

/// The sounds you picked for each song's instruments, by the song's key (`mo_library::Song::key`, the same for
/// every save of it), the song changed last at the end. Kept in the app's own settings ([`SOUNDS_KEY`]) as a JSON
/// list of `[song, {instrument: code}]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct YourSounds {
    songs: Vec<(String, HashMap<String, Sound>)>,
}

impl YourSounds {
    /// From the setting; what can't be read is left out (a damaged setting is no sounds picked, not an error: it's
    /// only a choice of sound, written over on the next pick).
    pub fn from_setting(text: Option<&str>) -> YourSounds {
        let Some(Ok(Value::Array(list))) = text.map(serde_json::from_str::<Value>) else {
            return YourSounds::default();
        };
        let songs = list
            .iter()
            .filter_map(|entry| {
                let [Value::String(song), Value::Object(sounds)] = entry.as_array()?.as_slice() else { return None };
                let sounds: HashMap<String, Sound> = sounds
                    .iter()
                    .filter_map(|(name, code)| Some((name.clone(), Sound::from_code(code.as_str()?)?)))
                    .collect();
                (!sounds.is_empty()).then(|| (song.clone(), sounds))
            })
            .collect();
        YourSounds { songs }
    }

    pub fn setting(&self) -> String {
        let list: Vec<Value> = self
            .songs
            .iter()
            .map(|(song, sounds)| {
                let mut names: Vec<(&String, &Sound)> = sounds.iter().collect();
                names.sort_by_key(|(name, _)| *name);
                let map: serde_json::Map<String, Value> =
                    names.into_iter().map(|(name, sound)| (name.clone(), Value::String(sound.code()))).collect();
                Value::Array(vec![Value::String(song.clone()), Value::Object(map)])
            })
            .collect();
        Value::Array(list).to_string()
    }

    /// What you picked for `song`'s instruments.
    pub fn of(&self, song: &str) -> HashMap<String, Sound> {
        self.songs.iter().find(|(s, _)| s == song).map(|(_, sounds)| sounds.clone()).unwrap_or_default()
    }

    /// Picks `sound` for `song`'s instrument `name`, or none to go back to the one matched to it. The song moves to
    /// the end, and the songs changed longest ago go past [`SONGS_KEPT`].
    pub fn pick(&mut self, song: &str, name: &str, sound: Option<Sound>) {
        let at = self.songs.iter().position(|(s, _)| s == song);
        let mut sounds = at.map(|at| self.songs.remove(at).1).unwrap_or_default();
        match sound {
            Some(sound) => {
                sounds.insert(name.to_owned(), sound);
            }
            None => {
                sounds.remove(name);
            }
        }
        if !sounds.is_empty() {
            self.songs.push((song.to_owned(), sounds));
        }
        if self.songs.len() > SONGS_KEPT {
            let over = self.songs.len() - SONGS_KEPT;
            self.songs.drain(..over);
        }
    }
}

/// The groups the sounds are picked from: General MIDI's sixteen families of eight, then the drums.
pub const FAMILIES: [&str; 17] = [
    "Piano",
    "Bells",
    "Organ",
    "Guitar",
    "Bass",
    "Strings",
    "Ensemble",
    "Brass",
    "Reed",
    "Pipe",
    "Synth lead",
    "Synth pad",
    "Synth FX",
    "World",
    "Percussive",
    "Sound FX",
    "Drums",
];

/// The place of the drums among [`FAMILIES`].
pub const DRUMS: usize = 16;

/// The sounds of family `family` ([`FAMILIES`]), in the order the menu shows them.
pub fn family_sounds(family: usize) -> Vec<Sound> {
    if family >= DRUMS {
        return std::iter::once(Sound::Kit).chain(HITS.iter().map(|h| Sound::Hit(h.1))).collect();
    }
    let first = (family.min(15) * 8) as u8;
    (first..first + 8).map(Sound::Program).collect()
}

/// The family a sound is in ([`FAMILIES`]).
pub fn family_of(sound: Sound) -> usize {
    match sound {
        Sound::Program(p) => usize::from(p & 0x7f) / 8,
        Sound::Kit | Sound::Hit(_) => DRUMS,
    }
}

/// What the sound line says for an instrument: the sound, and why when it's the one matched to it, or that you
/// picked it.
pub fn sound_tip(voice: &Voice, sound: Sound) -> String {
    if sound == voice.matched {
        format!("{} · {}", sound.name(), voice.why)
    } else {
        format!("{} · your pick (matched: {})", sound.name(), voice.matched.name())
    }
}

/// The tempo the song plays at: its own, else 120.
pub fn bpm(a: &Arrangement) -> f64 {
    if a.tempo.is_finite() && a.tempo >= 20.0 && a.tempo <= 999.0 { a.tempo } else { 120.0 }
}

/// Seconds into the song at `beat`, and the beat at `seconds`, at its tempo ([`bpm`]).
pub fn seconds_at(a: &Arrangement, beat: f64) -> f64 {
    beat.max(0.0) * 60.0 / bpm(a)
}

pub fn beat_at(a: &Arrangement, seconds: f64) -> f64 {
    seconds.max(0.0) * bpm(a) / 60.0
}

/// The play button's tooltip: what it plays, or that there's nothing to. `voices`: how many instruments, once
/// they're matched.
pub fn play_tip(voices: Option<usize>) -> String {
    match voices {
        None => {
            "Play the song's MIDI with General MIDI sounds, from the playhead (click the ruler to move it)".to_owned()
        }
        Some(0) => "No MIDI to play in this song".to_owned(),
        Some(1) => {
            "Play the song's MIDI with a General MIDI sound, from the playhead (click the ruler to move it)".to_owned()
        }
        Some(voices) => format!(
            "Play the song's MIDI, its {voices} instruments with General MIDI sounds, from the playhead (click the \
             ruler to move it)"
        ),
    }
}

/// The sounds chip's words: how many instruments, and how many you changed.
pub fn sounds_badge(voices: &[Voice], yours: &HashMap<String, Sound>) -> String {
    voices.len().to_string()
        + &match voices.iter().filter(|v| yours.get(&v.name).is_some_and(|s| *s != v.matched)).count() {
            0 => String::new(),
            n => format!(" · {n} yours"),
        }
}

#[cfg(test)]
mod tests {
    use mo_formats::arrangement::{PreviewLane, PreviewNote, TimeSignature};

    use super::*;

    fn note(start: f64, length: f64, key: u8) -> PreviewNote {
        PreviewNote { start, length, key, velocity: 50_000, ..PreviewNote::default() }
    }

    fn lane(name: &str, clips: Vec<PreviewClip>) -> PreviewLane {
        let mut lane = PreviewLane::new(name, LaneKind::Instrument);
        lane.clips = clips;
        lane
    }

    fn clip(start: f64, length: f64, notes: Vec<PreviewNote>) -> PreviewClip {
        PreviewClip { start, length, notes, ..PreviewClip::default() }
    }

    fn song(lanes: Vec<PreviewLane>) -> Arrangement {
        Arrangement {
            view: "test",
            tempo: 128.0,
            signature: TimeSignature::FOUR_FOUR,
            lanes,
            markers: Vec::new(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn each_track_plays_its_clips_notes_from_the_songs_start_and_what_is_muted_stays_quiet() {
        let chords = (0..4).flat_map(|bar| [60, 64, 67].map(|k| note(f64::from(bar) * 4.0, 4.0, k))).collect();
        let mut muted_note = note(1.0, 1.0, 72);
        muted_note.muted = true;
        let mut muted_clip = clip(32.0, 16.0, vec![note(0.0, 1.0, 50)]);
        muted_clip.muted = true;
        let mut muted_lane = lane("Muted", vec![clip(0.0, 4.0, vec![note(0.0, 1.0, 40)])]);
        muted_lane.muted = true;
        let a = song(vec![
            lane("Keys", vec![clip(16.0, 16.0, chords), muted_clip]),
            // A note past its clip's end isn't heard, and one running past it is cut there.
            lane("Lead", vec![clip(0.0, 4.0, vec![note(0.0, 8.0, 72), note(4.5, 1.0, 74), muted_note])]),
            muted_lane,
        ]);
        let voices = voices(&a);
        assert_eq!(voices.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), ["Keys", "Lead"]);
        let keys = &voices[0];
        assert_eq!(keys.notes.len(), 12);
        assert_eq!((keys.notes[0].start, keys.notes[0].length), (16 * PPQ, 4 * PPQ));
        assert_eq!(keys.matched, Sound::Program(4), "chords on an electric piano: {}", keys.why);
        let lead = &voices[1];
        assert_eq!(lead.notes.len(), 1);
        assert_eq!((lead.notes[0].start, lead.notes[0].length, lead.notes[0].key), (0, 4 * PPQ, 72));
        assert_eq!(voices[0].plays(Sound::Program(0)).1, 0);
    }

    #[test]
    fn each_channel_of_an_fl_pattern_is_an_instrument_and_a_one_key_drum_sample_plays_its_hit() {
        let mut pattern = clip(0.0, 16.0, Vec::new());
        pattern.parts = vec!["Kick 2".into(), "Sub bass".into(), "Clap 1 Select".into()];
        for beat in 0..16 {
            pattern.notes.push(PreviewNote { part: 0, step: true, ..note(f64::from(beat), 0.01, 60) });
        }
        pattern.notes.push(PreviewNote { part: 1, ..note(0.0, 8.0, 36) });
        pattern.notes.push(PreviewNote { part: 1, ..note(8.0, 8.0, 41) });
        for beat in [1, 3, 5, 7] {
            pattern.notes.push(PreviewNote { part: 2, step: true, ..note(f64::from(beat), 0.01, 60) });
        }
        let a =
            song(vec![lane("Track 1", vec![pattern.clone()]), lane("Track 2", vec![clip(16.0, 16.0, pattern.notes)])]);
        let voices = voices(&a);
        let names: Vec<&str> = voices.iter().map(|v| v.name.as_str()).collect();
        // The second track's clip has no parts: it's the track's own.
        assert_eq!(names, ["Kick 2", "Sub bass", "Clap 1 Select", "Track 2"]);
        assert_eq!(voices[0].matched, Sound::Hit(36));
        assert_eq!(voices[0].plays(voices[0].matched).0.iter().map(|n| n.key).collect::<Vec<_>>(), vec![36; 16]);
        // A step plays a beat, or to the next note on its key.
        assert_eq!(voices[0].notes[0].length, PPQ);
        assert_eq!(voices[1].matched, Sound::Program(38), "{}", voices[1].why);
        assert_eq!(voices[2].matched, Sound::Hit(39));
        assert_eq!(voices[2].why, "a drum sample: clap by its name");
    }

    #[test]
    fn names_say_which_hit() {
        for (name, key) in [
            ("Kick", Some(36)),
            ("Kicks 1 Select", Some(36)),
            ("Snare 2 Select", Some(38)),
            ("Chh 1 Select", Some(42)),
            ("OHH 1 Select", Some(46)),
            ("Open hat", Some(46)),
            ("Hi hat", Some(42)),
            ("Cymb 2 Select", Some(49)),
            ("Tamb 3", Some(54)),
            ("Shakers 4 Select", Some(70)),
            ("Perc 1", Some(75)),
            ("Clap", Some(39)),
            ("Rhodes", None),
            ("Ohm pad", None),
            ("Kickback lead", Some(36)),
        ] {
            assert_eq!(hit_named(name), key, "{name}");
        }
    }

    #[test]
    fn your_sounds_are_kept_by_song_and_the_oldest_go_first() {
        let mut yours = YourSounds::default();
        yours.pick("song a", "Keys", Some(Sound::Program(0)));
        yours.pick("song a", "Kick", Some(Sound::Hit(36)));
        yours.pick("song b", "Drums", Some(Sound::Kit));
        let text = yours.setting();
        let back = YourSounds::from_setting(Some(&text));
        assert_eq!(back, yours);
        assert_eq!(back.of("song a").get("Kick"), Some(&Sound::Hit(36)));
        assert!(back.of("song c").is_empty());
        // Back to the matched one: gone, and a song with none left goes too.
        let mut back = back;
        back.pick("song b", "Drums", None);
        assert!(back.of("song b").is_empty());
        assert!(!back.setting().contains("song b"));
        for n in 0..SONGS_KEPT + 5 {
            back.pick(&format!("s{n}"), "Keys", Some(Sound::Program(1)));
        }
        assert!(back.of("song a").is_empty());
        assert_eq!(back.of(&format!("s{}", SONGS_KEPT + 4)).get("Keys"), Some(&Sound::Program(1)));
        // What can't be read is no pick.
        for bad in ["oops", "{}", r#"[["a", {"Keys": "p999", "Kick": "h12"}], [1, 2]]"#] {
            assert_eq!(YourSounds::from_setting(Some(bad)), YourSounds::default(), "{bad}");
        }
        assert_eq!(YourSounds::from_setting(None), YourSounds::default());
    }

    #[test]
    fn every_sound_has_a_family_and_a_code() {
        for family in 0..FAMILIES.len() {
            for sound in family_sounds(family) {
                assert_eq!(family_of(sound), family);
                assert_eq!(Sound::from_code(&sound.code()), Some(sound));
                assert!(!sound.name().is_empty());
            }
        }
        assert_eq!(family_sounds(0)[4].name(), "Electric Piano 1");
        assert_eq!(family_sounds(DRUMS).len(), 1 + HITS.len());
    }

    #[test]
    fn play_says_what_it_plays() {
        assert_eq!(play_tip(Some(0)), "No MIDI to play in this song");
        assert!(play_tip(Some(6)).contains("its 6 instruments"));
        assert!(play_tip(None).starts_with("Play the song's MIDI"));
        let v = |name: &str, matched| Voice {
            name: name.into(),
            colour: None,
            notes: Vec::new(),
            matched,
            why: String::new(),
        };
        let voices = [v("Keys", Sound::Program(4)), v("Kick", Sound::Hit(36))];
        let mut yours = HashMap::new();
        assert_eq!(sounds_badge(&voices, &yours), "2");
        yours.insert("Keys".to_owned(), Sound::Program(0));
        assert_eq!(sounds_badge(&voices, &yours), "2 · 1 yours");
        assert_eq!(sound_of(&voices[0], &yours), Sound::Program(0));
        assert_eq!(
            sound_tip(&voices[0], Sound::Program(0)),
            "Acoustic Grand Piano · your pick (matched: Electric Piano 1)"
        );
    }

    #[test]
    fn the_tempo_turns_beats_into_seconds() {
        let a = song(Vec::new());
        assert!((seconds_at(&a, 128.0) - 60.0).abs() < 1e-9);
        assert!((beat_at(&a, 30.0) - 64.0).abs() < 1e-9);
        let odd = Arrangement { tempo: f64::NAN, ..a };
        assert_eq!(bpm(&odd), 120.0);
    }
}
