//! The part guess: what each sound plays in its song (bass, lead, chords...), from the notes it played.
//!
//! The rules (spec section 3.4a): one note at a time and low is bass, higher is lead; several at once is chords, or a
//! pad when the notes are long, or stabs and plucks when they're short; fast, even single notes over several pitches
//! are an arp; a few pitches repeated on the grid are drums; very few notes, or a few very long ones, are FX. Channel,
//! track and plugin names add clues. An effect takes the role of the track it sits on (see [`crate::list`]).
//!
//! The pitch is what the listener hears ([`Shift`]): the written note moved by the DAW's transpose and, where its
//! settings can be read, the plugin's own octave. When they can't, the guess says the octave wasn't checked.
//!
//! The numbers the rules use are in [`Thresholds`]. Rule 5: they may only come from counts and averages of Solomon's
//! own MIDI, never from the notes themselves; [`NoteStats`] is that summary of one part, and nothing else is kept.

use mo_formats::NoteEvent;

/// What a sound plays in its song.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    Bass,
    Lead,
    /// Several notes at once, neither long nor short.
    Chords,
    /// Several notes at once, held long.
    Pad,
    /// Several notes at once, short: stabs and plucks.
    Stabs,
    /// Fast, even single notes moving over several pitches.
    Arp,
    Drums,
    /// Very few notes, or a few very long ones: risers, hits, sweeps.
    Fx,
}

impl Role {
    pub const ALL: [Role; 8] =
        [Role::Bass, Role::Lead, Role::Chords, Role::Pad, Role::Stabs, Role::Arp, Role::Drums, Role::Fx];

    /// The word the page shows.
    pub fn label(self) -> &'static str {
        match self {
            Role::Bass => "Bass",
            Role::Lead => "Lead",
            Role::Chords => "Chords",
            Role::Pad => "Pad",
            Role::Stabs => "Stabs / plucks",
            Role::Arp => "Arp",
            Role::Drums => "Drums",
            Role::Fx => "FX",
        }
    }

    /// A short name that doesn't change, for saving a fix ("bass", "stabs"...).
    pub fn key(self) -> &'static str {
        match self {
            Role::Bass => "bass",
            Role::Lead => "lead",
            Role::Chords => "chords",
            Role::Pad => "pad",
            Role::Stabs => "stabs",
            Role::Arp => "arp",
            Role::Drums => "drums",
            Role::Fx => "fx",
        }
    }

    /// The role a [`Role::key`] names.
    pub fn from_key(key: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.key().eq_ignore_ascii_case(key.trim()))
    }
}

/// How far the DAW and the plugin move the written notes, so the guess hears the real pitch.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shift {
    /// Semitones the DAW adds before the plugin: FL's root note (C5 minus the channel's root key) and its pitch knob;
    /// Bitwig's Note Transpose devices before the instrument.
    pub semitones: f64,
    /// Semitones the plugin itself adds (its oscillator octave or coarse tune), when its settings could be read; `None`
    /// when they couldn't, and the guess then says the octave wasn't checked.
    pub plugin: Option<f64>,
    /// The DAW's transpose was read ([`Shift::semitones`] is all of it). False where the reader doesn't read it yet
    /// (Bitwig's Note Transpose devices), and the guess then says so.
    pub transpose_checked: bool,
}

impl Shift {
    /// FL Studio's channel: a plugin plays C5 (60) when the channel's root key is played, so a root key an octave down
    /// moves everything an octave up; the pitch knob adds its cents.
    pub fn fl_channel(root_key: u8, pitch_cents: i32) -> Shift {
        Shift { semitones: 60.0 - root_key as f64 + pitch_cents as f64 / 100.0, plugin: None, transpose_checked: true }
    }

    /// The pitch heard for a written key, as a MIDI note number with a fraction.
    pub fn heard(&self, key: u8) -> f64 {
        key as f64 + self.semitones + self.plugin.unwrap_or(0.0)
    }
}

/// The numbers the guess decides by. Each is a count, an average or a share, as rule 5 allows; the defaults are
/// starting points to be set from the counts and averages of Solomon's own MIDI (see [`NoteStats`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    /// A part with at most this many notes is FX.
    pub fx_most_notes: usize,
    /// One-at-a-time notes this long on average (in beats), with few notes, are FX.
    pub fx_long_beats: f64,
    /// ...and "few" is at most this many.
    pub fx_long_most_notes: usize,
    /// Notes that start together count as one chord when they're this close (in beats).
    pub together_beats: f64,
    /// A part is "several at once" when at least this share of its starts are chords.
    pub chord_share: f64,
    /// One at a time and heard below this MIDI note (on average, the middle of its notes) is bass.
    pub bass_below: f64,
    /// Within this many semitones of [`Thresholds::bass_below`], the bass/lead call is weak, so a name can overrule it.
    pub bass_doubt: f64,
    /// Chords held at least this long on average (beats) are a pad.
    pub pad_beats: f64,
    /// Chords at most this long on average (beats) are stabs or plucks.
    pub stab_beats: f64,
    /// Arp: notes start at most this far apart on average (beats)...
    pub arp_gap_beats: f64,
    /// ...that evenly (the spread of the gaps over their average)...
    pub arp_spread: f64,
    /// ...over at least this many pitches...
    pub arp_pitches: usize,
    /// ...with at least this many notes.
    pub arp_least_notes: usize,
    /// Drums: at most this many pitches...
    pub drum_most_pitches: usize,
    /// ...at least this share of notes on the sixteenth-note grid...
    pub drum_grid_share: f64,
    /// ...with at least this many notes...
    pub drum_least_notes: usize,
    /// ...and, with more than one pitch, at least this share of starts hitting two together (kick and hat).
    pub drum_together_share: f64,
}

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds {
            fx_most_notes: 3,
            fx_long_beats: 8.0,
            fx_long_most_notes: 8,
            together_beats: 1.0 / 32.0,
            chord_share: 0.3,
            // D3 (MIDI 50): a bass line rarely goes above it, a lead rarely below.
            bass_below: 50.0,
            bass_doubt: 4.0,
            pad_beats: 2.0,
            stab_beats: 0.5,
            // Sixteenths or faster: eighth-note bass and lead lines are common.
            arp_gap_beats: 0.3,
            arp_spread: 0.35,
            arp_pitches: 3,
            arp_least_notes: 12,
            drum_most_pitches: 6,
            drum_grid_share: 0.9,
            drum_least_notes: 8,
            drum_together_share: 0.15,
        }
    }
}

/// One part's notes summed up: counts, averages and shares only (rule 5), and all the guess looks at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteStats {
    /// Notes that play (muted notes left out).
    pub notes: usize,
    /// Different pitches written.
    pub pitches: usize,
    /// The middle of the heard pitches (half are below), as a MIDI note number.
    pub middle_pitch: f64,
    /// Places where notes start (notes starting together count once).
    pub starts: usize,
    /// The share of [`NoteStats::starts`] where two or more notes start together.
    pub chord_share: f64,
    /// Notes per start, on average.
    pub notes_per_start: f64,
    /// The average note length in beats, over notes that have one (FL step notes don't).
    pub average_beats: Option<f64>,
    /// The share of notes entered as FL steps (no length of their own).
    pub step_share: f64,
    /// The average gap between starts, in beats.
    pub average_gap_beats: Option<f64>,
    /// How uneven the gaps are: their spread over their average (0 is perfectly even).
    pub gap_spread: Option<f64>,
    /// The share of notes starting on the sixteenth-note grid.
    pub grid_share: f64,
}

impl NoteStats {
    /// Sums up a part's notes (in ticks at `ppq`), heard through `shift`. `None` when no note plays.
    pub fn of(notes: &[NoteEvent], ppq: u16, shift: Shift, limits: &Thresholds) -> Option<NoteStats> {
        let ppq = f64::from(ppq.max(1));
        let mut playing: Vec<&NoteEvent> = notes.iter().filter(|n| !n.muted).collect();
        if playing.is_empty() {
            return None;
        }
        playing.sort_by_key(|n| (n.start, n.key));

        let mut keys: Vec<u8> = playing.iter().map(|n| n.key).collect();
        keys.sort_unstable();
        let middle_pitch = match keys.len() {
            0 => 0.0,
            n if n % 2 == 1 => shift.heard(keys.get(n / 2).copied().unwrap_or(0)),
            n => {
                let low = shift.heard(keys.get(n / 2 - 1).copied().unwrap_or(0));
                let high = shift.heard(keys.get(n / 2).copied().unwrap_or(0));
                (low + high) / 2.0
            }
        };
        keys.dedup();

        // Starts: notes within `together_beats` of the first in a group start together.
        let together = (limits.together_beats * ppq).max(1.0);
        let mut starts: Vec<(i64, usize)> = Vec::new();
        for n in &playing {
            match starts.last_mut() {
                Some((at, count)) if (n.start - *at) as f64 <= together => *count += 1,
                _ => starts.push((n.start, 1)),
            }
        }
        let chords = starts.iter().filter(|(_, c)| *c >= 2).count();

        let timed: Vec<f64> = playing.iter().filter(|n| !n.step).map(|n| n.length.max(0) as f64 / ppq).collect();
        let average_beats = (!timed.is_empty()).then(|| timed.iter().sum::<f64>() / timed.len() as f64);
        let steps = playing.iter().filter(|n| n.step).count();

        let gaps: Vec<f64> = starts
            .windows(2)
            .map(|w| match w {
                [(a, _), (b, _)] => (b - a) as f64 / ppq,
                _ => 0.0,
            })
            .collect();
        let (average_gap_beats, gap_spread) = if gaps.is_empty() {
            (None, None)
        } else {
            let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
            let var = gaps.iter().map(|g| (g - mean).powi(2)).sum::<f64>() / gaps.len() as f64;
            (Some(mean), (mean > 0.0).then(|| var.sqrt() / mean))
        };

        let sixteenth = ppq / 4.0;
        let on_grid = playing
            .iter()
            .filter(|n| {
                let off = (n.start as f64 / sixteenth).fract();
                off.min(1.0 - off) * sixteenth <= together
            })
            .count();

        let total = playing.len() as f64;
        Some(NoteStats {
            notes: playing.len(),
            pitches: keys.len(),
            middle_pitch,
            starts: starts.len(),
            chord_share: chords as f64 / starts.len().max(1) as f64,
            notes_per_start: total / starts.len().max(1) as f64,
            average_beats,
            step_share: steps as f64 / total,
            average_gap_beats,
            gap_spread,
            grid_share: on_grid as f64 / total,
        })
    }
}

/// Where a guess came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    /// The notes it played.
    Notes,
    /// A word in its channel's, track's or plugin's name.
    Name,
    /// The part of the track it sits on (an effect).
    Track,
}

/// The guess for one sound, with why.
#[derive(Clone, Debug, PartialEq)]
pub struct Guess {
    /// `None` when there was nothing to go on.
    pub role: Option<Role>,
    pub basis: Basis,
    /// Why, in plain words, for the hover text.
    pub reason: String,
    /// The plugin's own octave was read, so the pitch is what the listener hears; false when it couldn't be (the
    /// reason says so).
    pub octave_checked: bool,
    /// A weak call, where a word in a name could have gone either way.
    pub weak: bool,
}

impl Guess {
    /// No guess, and why.
    pub fn none(reason: impl Into<String>) -> Guess {
        Guess { role: None, basis: Basis::Notes, reason: reason.into(), octave_checked: true, weak: true }
    }
}

/// Words in a name and the part they point to, checked in this order (so "Bass Drum" is drums, "Pad Bass" bass).
const NAME_CLUES: &[(&[&str], Role)] = &[
    (
        &[
            "drum",
            "drums",
            "kick",
            "snare",
            "hat",
            "hats",
            "hihat",
            "hi-hat",
            "clap",
            "perc",
            "percussion",
            "tom",
            "toms",
            "cymbal",
            "ride",
            "crash",
            "shaker",
            "rim",
            "fpc",
            "battery",
            "beat",
            "breakbeat",
        ],
        Role::Drums,
    ),
    (&["bass", "sub", "subbass", "808", "reese"], Role::Bass),
    (&["arp", "arpeggio", "arps"], Role::Arp),
    (&["pad", "pads", "strings", "atmos", "atmosphere", "drone", "choir"], Role::Pad),
    (&["pluck", "plucks", "stab", "stabs", "bell", "bells"], Role::Stabs),
    (&["chord", "chords", "keys", "piano", "rhodes", "organ", "epiano"], Role::Chords),
    (&["lead", "leads", "melody", "hook", "solo", "vox", "vocal"], Role::Lead),
    (&["fx", "sfx", "riser", "rise", "sweep", "impact", "downlifter", "uplifter", "noise", "whoosh", "hit"], Role::Fx),
];

/// The part a name points to, and the word that says so. Words are matched whole ("Subtle" isn't "sub").
pub fn name_clue(name: &str) -> Option<(Role, String)> {
    let lower = name.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric() && c != '-').filter(|w| !w.is_empty()).collect();
    for (clues, role) in NAME_CLUES {
        for clue in *clues {
            let hit = if clue.contains(' ') {
                lower.contains(clue)
            } else {
                // A hyphen joins words too ("Sub-Bass", "Kick-01"), though a clue may hold one ("hi-hat").
                words.iter().any(|w| {
                    w == clue
                        || w.trim_matches('-') == *clue
                        || numbered(w, clue)
                        || w.split('-').any(|part| part == *clue || numbered(part, clue))
                })
            };
            if hit {
                return Some((*role, (*clue).to_owned()));
            }
        }
    }
    None
}

/// "bass2" or "pad01": a clue with a number after it.
fn numbered(word: &str, clue: &str) -> bool {
    word.strip_prefix(clue).is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
}

/// A part, as the guess sees it.
#[derive(Clone, Copy, Debug)]
pub struct Part<'a> {
    pub notes: &'a [NoteEvent],
    pub ppq: u16,
    pub shift: Shift,
    /// Names that may hold a clue, most telling first: the channel's or track's, then the preset's, then the
    /// plugin's.
    pub names: &'a [&'a str],
}

/// What the notes alone say, and whether the call is weak.
fn from_notes(s: &NoteStats, limits: &Thresholds) -> (Role, String, bool) {
    let n = s.notes;
    let plural = |k: usize, one: &str| if k == 1 { format!("1 {one}") } else { format!("{k} {one}s") };
    if n <= limits.fx_most_notes {
        return (Role::Fx, format!("Plays only {}", plural(n, "note")), true);
    }
    // Drums: a few pitches, short, on the grid, and either one pitch (a one-shot played on one key), FL steps, or
    // different pitches hit together in twos (kick and hat) rather than chords of three or more. A melody line moves
    // over its pitches one at a time, so it's none of these.
    let short_or_steps = s.step_share >= 0.5 || s.average_beats.is_some_and(|b| b <= limits.stab_beats);
    let hits_together = s.chord_share >= limits.drum_together_share && s.notes_per_start < 1.6;
    if n >= limits.drum_least_notes
        && s.pitches <= limits.drum_most_pitches
        && s.grid_share >= limits.drum_grid_share
        && short_or_steps
        && (s.pitches == 1 || s.step_share >= 0.5 || hits_together)
    {
        // One pitch low down could as well be a bass on the grid.
        let weak = s.pitches == 1 && s.middle_pitch < limits.bass_below;
        let what = if s.pitches == 1 { "One pitch".to_owned() } else { format!("{} pitches", s.pitches) };
        let how = if hits_together { ", hit together" } else { "" };
        return (Role::Drums, format!("{what} repeated on the grid, short{how}"), weak);
    }
    let together = s.chord_share >= limits.chord_share;
    if !together {
        if n <= limits.fx_long_most_notes && s.average_beats.is_some_and(|b| b >= limits.fx_long_beats) {
            return (Role::Fx, format!("A few long notes ({n})"), true);
        }
        // A fast low line is a rolling bass, not an arp.
        if n >= limits.arp_least_notes
            && s.middle_pitch >= limits.bass_below
            && s.pitches >= limits.arp_pitches
            && s.average_gap_beats.is_some_and(|g| g <= limits.arp_gap_beats)
            && s.gap_spread.is_some_and(|sp| sp <= limits.arp_spread)
        {
            return (Role::Arp, "Fast, even single notes over several pitches".into(), false);
        }
        let weak = (s.middle_pitch - limits.bass_below).abs() < limits.bass_doubt;
        let note = note_name(s.middle_pitch);
        return if s.middle_pitch < limits.bass_below {
            (Role::Bass, format!("One note at a time, low (around {note})"), weak)
        } else {
            (Role::Lead, format!("One note at a time, higher (around {note})"), weak)
        };
    }
    match s.average_beats {
        Some(b) if b >= limits.pad_beats => (Role::Pad, "Several notes at once, held long".into(), false),
        Some(b) if b <= limits.stab_beats => (Role::Stabs, "Several notes at once, short".into(), false),
        Some(_) => (Role::Chords, "Several notes at once".into(), true),
        None => (Role::Stabs, "Several notes at once, entered as steps".into(), true),
    }
}

/// A MIDI note number as a note name, middle C (60) as C4 ("around A#2").
pub fn note_name(pitch: f64) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let p = pitch.round().clamp(0.0, 127.0) as i32;
    let name = NAMES.get(p.rem_euclid(12) as usize).copied().unwrap_or("C");
    format!("{name}{}", p.div_euclid(12) - 1)
}

/// Guesses a part's role: from its notes when it has some, with a name's clue taking over a weak call; from a name
/// alone when it has none.
pub fn guess(part: &Part<'_>, limits: &Thresholds) -> Guess {
    let clue = part.names.iter().find_map(|n| name_clue(n).map(|(r, w)| (r, w, *n)));
    let octave_checked = part.shift.plugin.is_some() && part.shift.transpose_checked;
    let unchecked = match (part.shift.transpose_checked, part.shift.plugin.is_some()) {
        (false, false) => " (the track's transpose and the plugin's own octave weren't checked)",
        (false, true) => " (the track's transpose wasn't checked)",
        _ => " (the plugin's own octave wasn't checked)",
    };
    let Some(stats) = NoteStats::of(part.notes, part.ppq, part.shift, limits) else {
        return match clue {
            Some((role, word, name)) => Guess {
                role: Some(role),
                basis: Basis::Name,
                reason: format!("Plays no notes; \"{name}\" says {}", word_says(&word, role)),
                octave_checked: true,
                weak: true,
            },
            None => Guess::none("Plays no notes, and its name gives no clue"),
        };
    };
    let (role, mut reason, weak) = from_notes(&stats, limits);
    // Bass and lead are told apart by pitch, so only they depend on the octave; the reason says so even when a name
    // then overrules the call.
    let pitch_matters = matches!(role, Role::Bass | Role::Lead);
    if pitch_matters && !octave_checked {
        reason += unchecked;
    }
    if let Some((named, word, name)) = clue
        && named != role
        && weak
    {
        return Guess {
            role: Some(named),
            basis: Basis::Name,
            reason: format!("{reason}, but \"{name}\" says {}", word_says(&word, named)),
            octave_checked: octave_checked || !pitch_matters,
            weak: true,
        };
    }
    Guess { role: Some(role), basis: Basis::Notes, reason, octave_checked: octave_checked || !pitch_matters, weak }
}

fn word_says(word: &str, role: Role) -> String {
    format!("{} (\"{word}\")", role.label().to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_formats::NoteAttribute;

    const PPQ: u16 = 96;

    fn n(start: i64, length: i64, key: u8) -> NoteEvent {
        NoteEvent { start, length, key, velocity: 40000, muted: false, step: false, attribute: NoteAttribute::NONE }
    }

    fn guess_of(notes: &[NoteEvent], shift: Shift, names: &[&str]) -> Guess {
        guess(&Part { notes, ppq: PPQ, shift, names }, &Thresholds::default())
    }

    fn checked() -> Shift {
        Shift { semitones: 0.0, plugin: Some(0.0), transpose_checked: true }
    }

    /// A bar of eighth notes cycling through `keys`, `bars` times.
    fn line(keys: &[u8], bars: i64, length: i64) -> Vec<NoteEvent> {
        (0..bars * 8).map(|i| n(i * 48, length, keys[i as usize % keys.len()])).collect()
    }

    #[test]
    fn low_single_notes_are_bass_and_high_ones_lead() {
        let bass = line(&[36, 36, 43, 41], 4, 40);
        assert_eq!(guess_of(&bass, checked(), &[]).role, Some(Role::Bass));
        let lead = line(&[72, 74, 76, 79, 77], 4, 40);
        let g = guess_of(&lead, checked(), &[]);
        assert_eq!(g.role, Some(Role::Lead));
        assert!(g.octave_checked && !g.reason.contains("octave"));
    }

    #[test]
    fn the_heard_pitch_decides_not_the_written_one() {
        // Written high, but FL's root key three octaves up plays it 36 semitones down.
        let notes = line(&[72, 74, 76, 79, 77], 4, 40);
        let shift = Shift::fl_channel(96, 0);
        assert_eq!(shift.semitones, -36.0);
        let g = guess_of(&notes, shift, &[]);
        assert_eq!(g.role, Some(Role::Bass));
        assert!(!g.octave_checked && g.reason.contains("octave wasn't checked"), "{}", g.reason);
        // The plugin's own octave counts too, once it's read.
        let g = guess_of(&notes, Shift { semitones: -36.0, plugin: Some(36.0), transpose_checked: true }, &[]);
        assert_eq!(g.role, Some(Role::Lead));
    }

    #[test]
    fn chords_split_by_length() {
        let chord = |at: i64, len: i64| [n(at, len, 60), n(at, len, 64), n(at, len, 67)];
        let pad: Vec<_> = (0..4).flat_map(|i| chord(i * 384, 384)).collect();
        assert_eq!(guess_of(&pad, checked(), &[]).role, Some(Role::Pad));
        let stabs: Vec<_> = (0..8).flat_map(|i| chord(i * 96 + 48, 24)).collect();
        assert_eq!(guess_of(&stabs, checked(), &[]).role, Some(Role::Stabs));
        let chords: Vec<_> = (0..8).flat_map(|i| chord(i * 96, 96)).collect();
        assert_eq!(guess_of(&chords, checked(), &[]).role, Some(Role::Chords));
    }

    #[test]
    fn fast_even_notes_over_several_pitches_are_an_arp() {
        let arp: Vec<_> = (0..32).map(|i| n(i * 24, 20, [60, 64, 67, 72][i as usize % 4])).collect();
        assert_eq!(guess_of(&arp, checked(), &[]).role, Some(Role::Arp));
    }

    #[test]
    fn a_few_pitches_on_the_grid_are_drums() {
        // Hats on every sixteenth, a kick with every fourth.
        let mut beat: Vec<_> = (0..16).map(|i| n(i * 24, 12, 42)).collect();
        beat.extend((0..4).map(|i| n(i * 96, 12, 36)));
        let g = guess_of(&beat, checked(), &[]);
        assert_eq!(g.role, Some(Role::Drums));
        assert!(g.reason.starts_with("2 pitches") && g.reason.ends_with("hit together"), "{}", g.reason);
        // One key, over and over: a one-shot.
        let one: Vec<_> = (0..16).map(|i| n(i * 48, 12, 60)).collect();
        assert_eq!(guess_of(&one, checked(), &[]).role, Some(Role::Drums));
        // FL step notes count as short.
        let steps: Vec<_> = (0..16).map(|i| NoteEvent { step: true, length: 0, ..n(i * 24, 0, 60) }).collect();
        assert_eq!(guess_of(&steps, checked(), &[]).role, Some(Role::Drums));
    }

    #[test]
    fn very_few_or_long_lonely_notes_are_fx() {
        assert_eq!(guess_of(&[n(0, 96, 60)], checked(), &[]).role, Some(Role::Fx));
        let long: Vec<_> = (0..4).map(|i| n(i * 4000, 3000, 60)).collect();
        assert_eq!(guess_of(&long, checked(), &[]).role, Some(Role::Fx));
    }

    #[test]
    fn muted_notes_dont_count() {
        let notes = vec![NoteEvent { muted: true, ..n(0, 96, 40) }];
        assert_eq!(guess_of(&notes, checked(), &[]).role, None);
    }

    #[test]
    fn a_name_overrules_only_a_weak_call() {
        // Around B2: just under the bass/lead line, so "Lead" in the name wins.
        let middle = line(&[46, 48, 47, 49], 4, 40);
        let g = guess_of(&middle, checked(), &["Lead 2"]);
        assert_eq!((g.role, g.basis), (Some(Role::Lead), Basis::Name));
        // Clearly low: the notes win, whatever the name.
        let low = line(&[31, 33, 36], 4, 40);
        assert_eq!(guess_of(&low, checked(), &["Lead 2"]).role, Some(Role::Bass));
        // No notes: the name is all there is.
        let g = guess_of(&[], checked(), &["Sub Bass"]);
        assert_eq!((g.role, g.basis), (Some(Role::Bass), Basis::Name));
        assert_eq!(guess_of(&[], checked(), &["Serum"]).role, None);
    }

    #[test]
    fn name_clues_match_whole_words() {
        assert_eq!(name_clue("Bass Drum").map(|c| c.0), Some(Role::Drums));
        assert_eq!(name_clue("Subtle keys").map(|c| c.0), Some(Role::Chords));
        assert_eq!(name_clue("pad01").map(|c| c.0), Some(Role::Pad));
        assert_eq!(name_clue("Hi-Hat loop").map(|c| c.0), Some(Role::Drums));
        assert_eq!(name_clue("Sub-Bass").map(|c| c.0), Some(Role::Bass));
        assert_eq!(name_clue("Kick-01").map(|c| c.0), Some(Role::Drums));
        assert_eq!(name_clue("Serum"), None);
    }

    #[test]
    fn roles_have_keys_that_come_back() {
        for r in Role::ALL {
            assert_eq!(Role::from_key(r.key()), Some(r));
        }
        assert_eq!(Role::from_key("nope"), None);
        assert_eq!(note_name(60.0), "C4");
        assert_eq!(note_name(46.0), "A#2");
    }
}
