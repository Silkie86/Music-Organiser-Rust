//! What the C# analyser stored for each sound: the `analysis` column of `sounds`, as JSON with the C# property names.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::compact::{F32_THOUSANDTHS, thousandths};
use crate::music;
use crate::word::Word;

/// A small waveform's peak levels (0..1). The analyser rounds each to thousandths (`Math.Round(x, 3)` in the C# app),
/// so they're held as whole thousandths, 2 bytes each instead of a float's 4, and come back as the same floats to the
/// last bit: 145 MB less for half a million sounds. A list with any other number in it keeps its floats.
///
/// Made only by [`Peaks::new`] (or the copy's reader, from thousandths it packed), so a list of thousandths is always
/// held as thousandths, each one 1023 at most, and two `Peaks` with the same floats are equal.
#[derive(Clone, Debug, PartialEq)]
pub struct Peaks(Levels);

#[derive(Clone, Debug, PartialEq)]
enum Levels {
    Thousandths(Box<[u16]>),
    Floats(Box<[f32]>),
}

impl Default for Peaks {
    fn default() -> Peaks {
        Peaks(Levels::Thousandths(Box::default()))
    }
}

impl Peaks {
    /// `levels` held as tightly as they can be with nothing lost.
    pub fn new(levels: &[f32]) -> Peaks {
        match levels.iter().map(|&x| thousandths(x)).collect::<Option<Box<[u16]>>>() {
            Some(ks) => Peaks(Levels::Thousandths(ks)),
            None => Peaks(Levels::Floats(levels.into())),
        }
    }

    /// Levels given as whole thousandths, as the copy packs them (10 bits each, so never over 1023).
    pub(crate) fn from_thousandths(ks: Box<[u16]>) -> Peaks {
        debug_assert!(ks.iter().all(|&k| usize::from(k) < F32_THOUSANDTHS.len()));
        Peaks(Levels::Thousandths(ks))
    }

    /// The levels as whole thousandths, when that's how they're held.
    pub(crate) fn as_thousandths(&self) -> Option<&[u16]> {
        match &self.0 {
            Levels::Thousandths(ks) => Some(ks),
            Levels::Floats(_) => None,
        }
    }

    pub fn len(&self) -> usize {
        match &self.0 {
            Levels::Thousandths(ks) => ks.len(),
            Levels::Floats(xs) => xs.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The levels as floats, as the analyser gave them, one by one (no list made).
    pub fn iter(&self) -> impl Iterator<Item = f32> + Clone + '_ {
        let (ks, xs): (&[u16], &[f32]) = match &self.0 {
            Levels::Thousandths(ks) => (ks, &[]),
            Levels::Floats(xs) => (&[], xs),
        };
        ks.iter().map(|&k| F32_THOUSANDTHS[usize::from(k)]).chain(xs.iter().copied())
    }

    /// The levels as a list of floats, as the analyser gave them.
    pub fn to_vec(&self) -> Vec<f32> {
        match &self.0 {
            Levels::Thousandths(_) => self.iter().collect(),
            Levels::Floats(xs) => xs.to_vec(),
        }
    }
}

impl From<Vec<f32>> for Peaks {
    fn from(levels: Vec<f32>) -> Peaks {
        Peaks::new(&levels)
    }
}

impl Serialize for Peaks {
    /// As the C# app writes them: a list of numbers.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.iter())
    }
}

impl<'de> Deserialize<'de> for Peaks {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Peaks, D::Error> {
        Ok(Peaks::new(&Vec::<f32>::deserialize(d)?))
    }
}

/// A stretch of a sound that plays one chord.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ChordSpan {
    /// Seconds from the start.
    pub start: f64,
    pub end: f64,
    pub name: String,
}

/// One sound's analysis. Every field may be missing in an older analysis, and then reads as its default.
///
/// The features vector isn't kept here: the database holds it again in the `features` column, which
/// [`crate::SoundsDb::vectors`] reads.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SoundAnalysis {
    pub seconds: f64,
    pub sample_rate: i32,
    pub channel_count: i32,
    pub peak_db: f64,
    pub rms_db: f64,
    /// Milliseconds from the start to the peak level.
    pub attack_ms: f64,
    /// Milliseconds from the peak until the level falls 30 dB.
    pub decay_ms: f64,
    /// Spectral centre of mass in Hz: low is dark, high is bright.
    pub brightness: f64,
    /// 0 is a pure tone, 1 is white noise.
    pub noisiness: f64,
    /// 0 is mono, 1 is very wide stereo.
    pub width: f64,
    pub is_loop: bool,
    pub bpm: Option<f64>,
    /// Where the tempo came from ("acid", "name", "onsets"…).
    pub bpm_source: Word,
    /// The main pitch of a one-shot, as a MIDI note number with cents.
    pub pitch: Option<f64>,
    pub pitch_confidence: f64,
    /// 0 = C … 11 = B.
    pub key_tonic: Option<i32>,
    pub key_minor: bool,
    pub key_confidence: f64,
    pub key_source: Word,
    /// The chord's name when the whole sound is one chord (chord stabs).
    pub chord: Option<String>,
    pub chords: Vec<ChordSpan>,
    /// How much of each of the 12 notes is in the sound, C first.
    pub chroma: Vec<f64>,
    /// Character words: punchy, soft, bright, dark, wide, tonal, noisy…
    pub character: Vec<Word>,
    /// Peak levels in 0..1, for drawing a small waveform.
    pub waveform: Peaks,
}

/// The loudest a silent sound gets, in dB below full scale: under any sample pack's noise floor or dither, and quieter
/// than Trim silence's quietest level (-70 dB). An empty file, or one of only zeros, measures -120 dB.
pub const SILENT_DB: f64 = -80.0;

impl SoundAnalysis {
    /// Whether the sound is silent all through, or empty: its loudest point ([`SoundAnalysis::peak_db`]) is at
    /// [`SILENT_DB`] or under. That peak is measured with the two sides mixed into one, so a stereo sound whose sides
    /// cancel out there (one is the other upside down) is still heard: nearly all that's left of it is the difference
    /// between them, which puts its width near the most there is (1.41).
    pub fn is_silent(&self) -> bool {
        self.peak_db <= SILENT_DB && self.width < 1.3
    }

    /// Reads the JSON the C# app stored. `None` when it isn't a readable analysis (the C# app then shows none too).
    pub fn from_json(json: &str) -> Option<SoundAnalysis> {
        let mut a: SoundAnalysis = serde_json::from_str(json).ok()?;
        // Reading a list doubles its room each time it fills, so with half a million sounds held the spare room adds
        // up. Each list is cut to what it holds (the waveform's `Peaks` is always made at its exact size).
        a.chroma.shrink_to_fit();
        a.character.shrink_to_fit();
        a.chords.shrink_to_fit();
        Some(a)
    }

    /// The key, short ("Ebm", "F#"), when one was found.
    pub fn key_name(&self) -> Option<String> {
        self.key_tonic.map(|t| music::key_name(t, self.key_minor, true))
    }

    /// The key's Camelot code ("8A"), when one was found.
    pub fn camelot(&self) -> Option<String> {
        self.key_tonic.map(|t| music::camelot(t, self.key_minor))
    }

    /// The main pitch as a note name ("C2"), when one was found.
    pub fn root_note_name(&self) -> Option<String> {
        // C# rounds half to even (Math.Round), and casts to int.
        self.pitch.filter(|p| p.is_finite()).map(|p| music::note_name(p.round_ties_even() as i32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_analysis_keeps_no_spare_room() {
        let points: Vec<String> = (0..160).map(|i| format!("{}", f64::from(i * 6) / 1000.0)).collect();
        let json = format!(
            r#"{{"Waveform":[{}],"Chroma":[0,0,0,0,0,0,0,0,0,0,0,0.5],"Character":["dark","wide","soft"],
                "Chords":[{{"Name":"Cm"}},{{"Name":"Fm"}},{{"Name":"G"}}]}}"#,
            points.join(",")
        );
        let a = SoundAnalysis::from_json(&json).unwrap();
        assert_eq!((a.waveform.len(), a.chroma.len(), a.character.len(), a.chords.len()), (160, 12, 3, 3));
        assert!(a.waveform.as_thousandths().is_some(), "160 thousandths held as 2 bytes each");
        assert_eq!(a.waveform.to_vec(), points.iter().map(|p| p.parse::<f32>().unwrap()).collect::<Vec<_>>());
        assert_eq!(a.chroma.capacity(), a.chroma.len());
        assert_eq!(a.character.capacity(), a.character.len());
        assert_eq!(a.chords.capacity(), a.chords.len());
    }

    #[test]
    fn peaks_come_back_as_the_same_floats_to_the_last_bit() {
        // Every thousandth, as JSON gives it (the analyser rounds to 3 places), then numbers that aren't one.
        let all: Vec<f32> = (0..=1023).map(|k| format!("{}", f64::from(k) / 1000.0).parse().unwrap()).collect();
        let held = Peaks::new(&all);
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&held.to_vec()), bits(&all));
        assert_eq!(bits(&held.iter().collect::<Vec<_>>()), bits(&all));
        assert!(held.as_thousandths().is_some(), "JSON's thousandths are the table's floats");
        for odd in [[0.5, 0.1234], [0.5, -0.001], [0.5, 1.024], [0.5, f32::NAN]] {
            let p = Peaks::new(&odd);
            assert!(p.as_thousandths().is_none());
            assert_eq!(bits(&p.to_vec()), bits(&odd));
            assert_eq!(bits(&p.iter().collect::<Vec<_>>()), bits(&odd));
        }
        assert!(Peaks::new(&[]).is_empty() && Peaks::default().is_empty());
        // Written as the C# app writes them, and read back the same.
        let json = serde_json::to_string(&held).unwrap();
        assert_eq!(serde_json::from_str::<Peaks>(&json).unwrap(), held);
        assert_eq!(serde_json::to_string(&Peaks::new(&[0.5, 1.0])).unwrap(), "[0.5,1.0]");
    }
}
