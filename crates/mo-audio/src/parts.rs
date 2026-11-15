//! Going through a whole sound file a part at a time ([`crate::read_parts`]): the decoders that give a file's sound a
//! stretch at a time (symphonia's, and Windows' for WMA) have it cut into parts of the same length here, and every part
//! is handed on through [`give`].

use std::panic::AssertUnwindSafe;

use crate::AudioError;

/// What a part goes to ([`crate::read_parts`]'s `each`): its sample rate and its samples, a channel each; it says whether
/// to go on.
pub(crate) type Each<'a> = dyn FnMut(u32, &[Vec<f32>]) -> bool + 'a;

/// How going through a file's sound ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Ended {
    /// At the end of the file (or of the stretch that matches its start, in a stream that changes part way).
    End,
    /// At a damaged or cut-short stretch, with what came before it kept.
    Damage,
    /// What it went to said to stop.
    Stopped,
}

/// Hands a part to `each`. A panic in it is a bug in the caller's work, not damage in the file: it's caught here as a
/// crash and written to the log in full ([`mo_catch::catch_crash`]), so the catch around a decoder (for what a damaged
/// file can do, one warning line) doesn't pass it off as damage, and the read ends.
pub(crate) fn give(each: &mut Each, rate: u32, part: &[Vec<f32>]) -> Result<bool, AudioError> {
    mo_catch::catch_crash(AssertUnwindSafe(|| each(rate, part))).map_err(|_| {
        AudioError::Format("The read stopped: the app tripped over what it was given (a crash, in the log)".into())
    })
}

/// A file's sound, as its decoder gives it a stretch at a time, cut into parts of the same length for `each` ([`give`]).
pub(crate) struct Parts {
    rate: u32,
    /// Frames in a part: one at the least, and never more than one read may hold.
    per: usize,
    /// The part being filled, a channel each.
    part: Vec<Vec<f32>>,
    /// Frames handed on so far.
    handed: u64,
    /// Why the read ended early, when `each` tripped ([`give`]).
    tripped: Option<AudioError>,
}

impl Parts {
    /// For a sound at `rate` frames a second with `count` channels, in parts of `part_seconds` (`as` saturates: NaN and
    /// anything below zero give a frame, infinity as many as a read may hold).
    pub(crate) fn new(rate: u32, count: usize, part_seconds: f64) -> Parts {
        let count = count.max(1);
        let most = usize::try_from(crate::read::MOST_BYTES).unwrap_or(usize::MAX) / (4 * count);
        let per = ((part_seconds * f64::from(rate)) as usize).clamp(1, most.max(1));
        // Each channel grows to a part once, and is kept that big for the parts after.
        Parts { rate, per, part: (0..count).map(|_| Vec::new()).collect(), handed: 0, tripped: None }
    }

    /// The next stretch of the sound, a channel each (as long as each other; a missing channel is left empty). False
    /// once `each` has said to stop, or tripped.
    pub(crate) fn push(&mut self, stretch: &[Vec<f32>], each: &mut Each) -> bool {
        let frames = stretch.first().map_or(0, Vec::len);
        let mut at = 0;
        while at < frames {
            let take = (self.per - self.filled()).min(frames - at);
            for (ch, s) in self.part.iter_mut().zip(stretch) {
                ch.extend_from_slice(s.get(at..at + take).unwrap_or_default());
            }
            at += take;
            if self.filled() >= self.per && !self.hand_on(each) {
                return false;
            }
        }
        true
    }

    /// How many frames have come: those handed on and those of the part being filled.
    #[cfg(windows)]
    pub(crate) fn frames(&self) -> u64 {
        self.handed + self.filled() as u64
    }

    /// Hands on the last part, however full, when the sound went to its end (or to damage), and gives its whole length
    /// in seconds: what came, or after damage or a stop, what the file says (`told` frames) if that's more.
    pub(crate) fn finish(mut self, ended: Ended, told: Option<u64>, each: &mut Each) -> Result<f64, AudioError> {
        if ended != Ended::Stopped && self.tripped.is_none() && self.filled() > 0 {
            self.hand_on(each);
        }
        if let Some(e) = self.tripped {
            return Err(e);
        }
        let frames = match ended {
            Ended::End => self.handed,
            Ended::Damage | Ended::Stopped => told.unwrap_or(self.handed).max(self.handed),
        };
        Ok(frames as f64 / f64::from(self.rate.max(1)))
    }

    fn filled(&self) -> usize {
        self.part.first().map_or(0, Vec::len)
    }

    /// Hands on the part filled so far, and empties it for the next. False once `each` has said to stop, or tripped.
    fn hand_on(&mut self, each: &mut Each) -> bool {
        let go_on = match give(each, self.rate, &self.part) {
            Ok(go_on) => go_on,
            Err(e) => {
                self.tripped = Some(e);
                false
            }
        };
        self.handed += self.filled() as u64;
        self.part.iter_mut().for_each(Vec::clear);
        go_on
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each part handed on, and the length the parts give.
    fn cut(per_seconds: f64, stretches: &[usize], stop_after: usize, ended: Ended) -> (Vec<Vec<f32>>, f64) {
        let mut got: Vec<Vec<f32>> = Vec::new();
        let mut each = |rate: u32, part: &[Vec<f32>]| {
            assert_eq!(rate, 10);
            assert_eq!(part.len(), 2, "both channels");
            assert_eq!(part[0].iter().map(|v| -v).collect::<Vec<_>>(), part[1]);
            got.push(part[0].clone());
            got.len() < stop_after
        };
        let mut parts = Parts::new(10, 2, per_seconds);
        let mut next = 0.0f32;
        for &n in stretches {
            let left: Vec<f32> = (0..n).map(|i| next + i as f32).collect();
            next += n as f32;
            let right = left.iter().map(|v| -v).collect::<Vec<_>>();
            if !parts.push(&[left, right], &mut each) {
                break;
            }
        }
        let seconds = parts.finish(ended, Some(100), &mut each).unwrap();
        (got, seconds)
    }

    #[test]
    fn a_sound_is_cut_into_parts_of_the_same_length_whatever_its_stretches() {
        // Four frames a part (0.4 s at 10 a second), from stretches of 3, 6, 0 and 2 frames: the last part is short.
        let (got, seconds) = cut(0.4, &[3, 6, 0, 2], usize::MAX, Ended::End);
        assert_eq!(got, [vec![0.0, 1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0, 7.0], vec![8.0, 9.0, 10.0]]);
        assert_eq!(seconds, 1.1);
        // Stopped after the first part: no more, and the file's own length (100 frames).
        let (got, seconds) = cut(0.4, &[3, 6, 2], 1, Ended::Stopped);
        assert_eq!((got.len(), seconds), (1, 10.0));
        // Damage after 11 frames: the last part is still handed on, and the length is the file's.
        let (got, seconds) = cut(0.4, &[3, 6, 2], usize::MAX, Ended::Damage);
        assert_eq!((got.len(), seconds), (3, 10.0));
        // A part of no time at all is a frame.
        let (got, _) = cut(0.0, &[3], usize::MAX, Ended::End);
        assert_eq!(got, [vec![0.0], vec![1.0], vec![2.0]]);
    }

    #[test]
    fn a_trip_in_what_a_part_goes_to_ends_the_read_with_an_error() {
        let mut parts = Parts::new(10, 1, 0.2);
        let mut calls = 0;
        let mut each = |_: u32, _: &[Vec<f32>]| -> bool {
            calls += 1;
            panic!("a bug in the caller");
        };
        assert!(!parts.push(&[vec![0.5; 5]], &mut each), "stops at the trip");
        let e = parts.finish(Ended::End, None, &mut each).unwrap_err();
        assert!(e.to_string().contains("tripped"), "{e}");
        assert_eq!(calls, 1, "the last part isn't handed on after a trip");
    }
}
