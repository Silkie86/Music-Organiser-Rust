//! One chord of a part, as the index keeps it (the C# `PhraseChord`): its root, every pitch class it sounds and its
//! bass note. Keeping the notes rather than a name lets two chords be compared by how many notes they share, so Am7
//! and C are close and Am and F# are not.
//!
//! Naming chords ("Am9", "G13(b9)") is the harmony engine's job (`mo-harmony`, phase 6), so it isn't here.

use std::fmt;

/// One chord: root and bass as pitch classes (0 = C … 11 = B), and the pitch classes it sounds as a 12-bit set
/// (bit 0 = C). A bass that's only a bass (the C of D/C) isn't in `mask`; [`PhraseChord::sounding`] adds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PhraseChord {
    pub root: u8,
    pub mask: u16,
    pub bass: u8,
}

impl PhraseChord {
    /// A chord with its root in the bass.
    pub fn new(root: u8, mask: u16) -> PhraseChord {
        PhraseChord { root: root % 12, mask: mask & 0xFFF, bass: root % 12 }
    }

    /// Every note that sounds, the bass included.
    pub fn sounding(self) -> u16 {
        (self.mask | 1 << (self.bass % 12)) & 0xFFF
    }

    /// The chord moved up (or down, for a negative number) by some semitones.
    pub fn transposed(self, semitones: i32) -> PhraseChord {
        PhraseChord {
            root: mod12(self.root as i32 + semitones),
            mask: rotate(self.mask, semitones),
            bass: mod12(self.bass as i32 + semitones),
        }
    }

    /// 0 to 1: the share of notes the two have in common, a quarter more when they're built on the same root (at most
    /// 1). Rhythm plays no part; only which notes sound.
    pub fn similarity(self, other: PhraseChord) -> f64 {
        let (a, b) = (self.sounding(), other.sounding());
        let union = (a | b).count_ones();
        if union == 0 {
            return 0.0;
        }
        let shared = (a & b).count_ones() as f64 / union as f64;
        (shared + if self.root == other.root { 0.25 } else { 0.0 }).min(1.0)
    }

    /// The chords as `midi.db` keeps them: each as [`Display`](fmt::Display) writes it, with spaces between.
    pub fn encode(chords: &[PhraseChord]) -> String {
        chords.iter().map(ToString::to_string).collect::<Vec<_>>().join(" ")
    }

    /// The chords from `midi.db`'s text. A piece that doesn't read as a chord is left out, as the C# app does; so is
    /// one whose root is past B or whose notes run past 12 bits, which the C# app never writes.
    pub fn decode(text: &str) -> Vec<PhraseChord> {
        text.split(' ').filter(|p| !p.is_empty()).filter_map(PhraseChord::parse).collect()
    }

    fn parse(part: &str) -> Option<PhraseChord> {
        let (root, rest) = part.split_once(':')?;
        let (mask, bass) = match rest.split_once('/') {
            Some((mask, bass)) => (mask, Some(bass)),
            None => (rest, None),
        };
        let root: u8 = root.parse().ok().filter(|r| *r < 12)?;
        let mask = u16::from_str_radix(mask, 16).ok().filter(|m| *m <= 0xFFF)?;
        // As the C# app: a bass that doesn't read leaves the chord with its root in the bass.
        let bass = bass.and_then(|b| b.parse::<u8>().ok()).filter(|b| *b < 12).unwrap_or(root);
        Some(PhraseChord { root, mask, bass })
    }
}

/// "9:291" (A with C, E and G), with "/4" after it for a bass that isn't the root (over E).
impl fmt::Display for PhraseChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{:03x}", self.root, self.mask)?;
        if self.bass != self.root {
            write!(f, "/{}", self.bass)?;
        }
        Ok(())
    }
}

/// A 12-bit pitch-class set turned round by some semitones (up for a positive number).
pub fn rotate(mask: u16, semitones: i32) -> u16 {
    let s = mod12(semitones) as u32;
    let mask = (mask & 0xFFF) as u32;
    if s == 0 { mask as u16 } else { (((mask << s) | (mask >> (12 - s))) & 0xFFF) as u16 }
}

fn mod12(n: i32) -> u8 {
    n.rem_euclid(12) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    const A_MINOR: u16 = 1 << 9 | 1 << 0 | 1 << 4;

    #[test]
    fn writes_and_reads_as_the_csharp_app() {
        let am = PhraseChord::new(9, A_MINOR);
        assert_eq!(am.to_string(), "9:211");
        let over_e = PhraseChord { bass: 4, ..am };
        assert_eq!(over_e.to_string(), "9:211/4");
        let text = PhraseChord::encode(&[am, over_e]);
        assert_eq!(text, "9:211 9:211/4");
        assert_eq!(PhraseChord::decode(&text), vec![am, over_e]);
        // Upper-case hex reads too (.NET's HexNumber takes both), and junk is left out.
        assert_eq!(PhraseChord::decode("  9:2A1 x 3: :12 12:001 0:1000 "), vec![PhraseChord::new(9, 0x2A1)]);
        // A bass that doesn't read is the root.
        assert_eq!(PhraseChord::decode("9:211/x"), vec![am]);
        assert!(PhraseChord::decode("").is_empty());
    }

    #[test]
    fn transposes_round_the_octave() {
        let am = PhraseChord::new(9, A_MINOR);
        let bm = am.transposed(2);
        assert_eq!((bm.root, bm.bass), (11, 11));
        assert_eq!(bm.mask, 1 << 11 | 1 << 2 | 1 << 6);
        assert_eq!(bm.transposed(-2), am);
        assert_eq!(am.transposed(12), am);
        assert_eq!(rotate(0b1, -1), 1 << 11);
    }

    #[test]
    fn likeness_counts_shared_notes() {
        let am = PhraseChord::new(9, A_MINOR);
        let c = PhraseChord::new(0, 1 | 1 << 4 | 1 << 7);
        let fsharp = PhraseChord::new(6, 1 << 6 | 1 << 10 | 1 << 1);
        assert_eq!(am.similarity(am), 1.0);
        assert!((am.similarity(c) - 0.5).abs() < 1e-9); // C and E shared of A, C, E, G
        assert_eq!(am.similarity(fsharp), 0.0);
        let empty = PhraseChord { root: 0, mask: 0, bass: 0 };
        assert!(empty.similarity(empty) > 0.0); // the bass alone still sounds
        // A chord made by hand past B can't stop the app.
        let odd = PhraseChord { root: 0, mask: 0xFFFF, bass: 200 };
        assert_eq!(odd.sounding(), 0xFFF);
        assert!(odd.similarity(am) > 0.0);
    }
}
