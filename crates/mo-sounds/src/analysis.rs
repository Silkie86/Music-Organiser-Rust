//! What the C# analyser stored for each sound: the `analysis` column of `sounds`, as JSON with the C# property names.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::music;
use crate::word::Word;

/// A small waveform's peak levels (0..1). The analyser rounds each to thousandths (`Math.Round(x, 3)` in the C# app),
/// so they're held as whole thousandths, 2 bytes each instead of a float's 4, and come back as the same floats to the
/// last bit: 145 MB less for half a million sounds. A list with any other number in it keeps its floats.
#[derive(Clone, Debug, PartialEq)]
pub enum Peaks {
    Thousandths(Box<[u16]>),
    Floats(Box<[f32]>),
}

impl Default for Peaks {
    fn default() -> Peaks {
        Peaks::Thousandths(Box::default())
    }
}

impl Peaks {
    /// `levels` held as tightly as they can be with nothing lost.
    pub fn new(levels: &[f32]) -> Peaks {
        match levels.iter().map(|&x| thousandths(x)).collect::<Option<Box<[u16]>>>() {
            Some(ks) => Peaks::Thousandths(ks),
            None => Peaks::Floats(levels.into()),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Peaks::Thousandths(ks) => ks.len(),
            Peaks::Floats(xs) => xs.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The levels as floats, as the analyser gave them.
    pub fn to_vec(&self) -> Vec<f32> {
        match self {
            Peaks::Thousandths(ks) => ks.iter().map(|&k| F32_THOUSANDTHS[usize::from(k)]).collect(),
            Peaks::Floats(xs) => xs.to_vec(),
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
        self.to_vec().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Peaks {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Peaks, D::Error> {
        Ok(Peaks::new(&Vec::<f32>::deserialize(d)?))
    }
}

/// Every whole number of thousandths from 0 to 1023 as the float the analyser makes of it: what [`Peaks`] holds them
/// as, looked up rather than worked out (and so the same float every time).
pub(crate) const F32_THOUSANDTHS: [f32; 1024] = {
    let mut table = [0.0; 1024];
    let mut k = 0;
    while k < 1024 {
        table[k] = (k as f64 / 1000.0) as f32;
        k += 1;
    }
    table
};

/// `x` as a whole number of thousandths, when it is exactly one (as the table gives it), up to 1023.
fn thousandths(x: f32) -> Option<u16> {
    let k = (f64::from(x) * 1000.0).round();
    if !(0.0..=1023.0).contains(&k) {
        return None;
    }
    let k = k as u16;
    (F32_THOUSANDTHS[usize::from(k)].to_bits() == x.to_bits()).then_some(k)
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

impl SoundAnalysis {
    /// Reads the JSON the C# app stored. `None` when it isn't a readable analysis (the C# app then shows none too).
    pub fn from_json(json: &str) -> Option<SoundAnalysis> {
        let mut a: SoundAnalysis = serde_json::from_str(json).ok()?;
        // Reading a list doubles its room each time it fills, so the 160 waveform points got room for 256: with half a
        // million sounds held, that spare room alone was about 200 MB. Each list is cut to what it holds.
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
        assert!(matches!(a.waveform, Peaks::Thousandths(_)), "160 thousandths held as 2 bytes each");
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
        assert!(matches!(held, Peaks::Thousandths(_)), "JSON's thousandths are the table's floats");
        for odd in [[0.5, 0.1234], [0.5, -0.001], [0.5, 1.024], [0.5, f32::NAN]] {
            let p = Peaks::new(&odd);
            assert!(matches!(p, Peaks::Floats(_)));
            assert_eq!(bits(&p.to_vec()), bits(&odd));
        }
        assert!(Peaks::new(&[]).is_empty() && Peaks::default().is_empty());
        // Written as the C# app writes them, and read back the same.
        let json = serde_json::to_string(&held).unwrap();
        assert_eq!(serde_json::from_str::<Peaks>(&json).unwrap(), held);
        assert_eq!(serde_json::to_string(&Peaks::new(&[0.5, 1.0])).unwrap(), "[0.5,1.0]");
    }
}
