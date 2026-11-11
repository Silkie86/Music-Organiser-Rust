//! The small waveform on each line of the sound list.
//!
//! The app's sound library keeps each sound's waveform apart from the list (a few hundred bytes each), so the plugin
//! reads only the ones for the lines on show, on a background thread, and keeps the newest few hundred as pictures
//! ([`Kept`]). A picture is white with see-through edges: the panel tints it (grey, or the accent for the picked line).

use mo_sounds::Peaks;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::{HashMap, HashSet, VecDeque};

/// A line's waveform on the panel, in logical pixels (before the screen's scaling): as panel.slint lays it out.
pub const WIDTH: f64 = 120.0;
pub const HEIGHT: f64 = 30.0;

/// How many pictures are kept: more than a tall screen's lines, and a few screens back (about 10 MB at most on a
/// 4K screen, far less on others).
pub const MOST_KEPT: usize = 512;

/// The waveform as a picture `width` by `height` pixels: one upright line per pixel across, about the middle, as tall
/// as the loudest peak it stands for next to the loudest of all (so every click shows, and a quiet sound still fills
/// its line, as the app draws them). `None` for no waveform or no room.
pub fn picture(peaks: &Peaks, width: u32, height: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let n = peaks.len();
    if n == 0 || width == 0 || height == 0 {
        return None;
    }
    let levels: Vec<f32> = peaks.iter().map(|p| if p.is_finite() { p.abs() } else { 0.0 }).collect();
    let loudest = levels.iter().fold(0.0f32, |m, &v| m.max(v)).max(f32::EPSILON);
    let (w, h) = (width as usize, height as usize);
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    let pixels = buffer.make_mut_slice();
    let middle = h as f32 / 2.0;
    for x in 0..w {
        let from = x * n / w;
        let to = ((x + 1) * n / w).clamp(from + 1, n);
        let level = levels[from..to].iter().fold(0.0f32, |m, &v| m.max(v)) / loudest;
        // At least half a pixel each way: silence is still a thin line.
        let half = (level * middle).clamp(0.5, middle);
        let (top, bottom) = (middle - half, middle + half);
        for y in 0..h {
            let covered = (bottom.min(y as f32 + 1.0) - top.max(y as f32)).clamp(0.0, 1.0);
            pixels[y * w + x] = Rgba8Pixel { r: 255, g: 255, b: 255, a: (covered * 255.0).round() as u8 };
        }
    }
    Some(buffer)
}

/// How many peaks a waveform made from a sound as it was played has: as many as a wide screen draws.
pub const PEAKS: usize = 480;

/// A waveform of audio as it was read (a list of samples per channel): at most `most` peaks, each the loudest sample
/// of its part in any channel. For a sound the library has no waveform for.
pub fn peaks(channels: &[Vec<f32>], most: usize) -> Peaks {
    let frames = channels.iter().map(Vec::len).min().unwrap_or(0);
    let points = frames.min(most);
    let levels: Vec<f32> = (0..points)
        .map(|i| {
            let (from, to) = (i * frames / points, ((i + 1) * frames / points).max(i * frames / points + 1));
            channels
                .iter()
                .flat_map(|c| &c[from..to.min(frames)])
                .fold(0.0f32, |m, v| if v.is_finite() { m.max(v.abs()) } else { m })
        })
        .collect();
    Peaks::new(&levels)
}

/// The pictures kept, by sound, and the ones to read next. `P` is the picture (the panel's is a Slint image).
pub struct Kept<P> {
    /// `None`: read, and the sound has no waveform.
    pictures: HashMap<i64, Option<P>>,
    /// The order they came in: the oldest go first.
    order: VecDeque<i64>,
    /// Asked for by a line and not read yet: the line's row and its sound, in the order asked.
    wanted: Vec<(usize, i64)>,
    /// Being read now.
    reading: Vec<(usize, i64)>,
    /// In `wanted` or `reading`: asked once only.
    asked: HashSet<i64>,
    /// Counts the times everything was forgotten: an answer to an older asking is dropped.
    round: u64,
}

impl<P> Default for Kept<P> {
    fn default() -> Self {
        Kept {
            pictures: HashMap::new(),
            order: VecDeque::new(),
            wanted: Vec::new(),
            reading: Vec::new(),
            asked: HashSet::new(),
            round: 0,
        }
    }
}

impl<P: Clone> Kept<P> {
    /// The picture for the sound on `row`, if it's been read; if not, it's read next.
    pub fn picture(&mut self, row: usize, sound: i64) -> Option<P> {
        if let Some(kept) = self.pictures.get(&sound) {
            return kept.clone();
        }
        if self.asked.insert(sound) {
            self.wanted.push((row, sound));
        }
        None
    }

    /// The sounds to read now, with this asking's round: none while a read is under way. Only the newest
    /// [`MOST_KEPT`] asked for (the lines on show are among them); older ones are asked again if their line comes
    /// back.
    pub fn to_read(&mut self) -> Option<(u64, Vec<i64>)> {
        if !self.reading.is_empty() || self.wanted.is_empty() {
            return None;
        }
        let late = self.wanted.len().saturating_sub(MOST_KEPT);
        for (_, sound) in self.wanted.drain(..late) {
            self.asked.remove(&sound);
        }
        self.reading = std::mem::take(&mut self.wanted);
        Some((self.round, self.reading.iter().map(|&(_, sound)| sound).collect()))
    }

    /// Takes in what a read found (`round` is the one [`Kept::to_read`] gave). The rows to draw again, each with the
    /// sound it was asked for (the list may have changed since: the caller checks).
    pub fn take_in(&mut self, round: u64, found: Vec<(i64, Option<P>)>) -> Vec<(usize, i64)> {
        if round != self.round {
            return Vec::new();
        }
        let read = std::mem::take(&mut self.reading);
        let mut found: HashMap<i64, Option<P>> = found.into_iter().collect();
        for &(_, sound) in &read {
            self.asked.remove(&sound);
            // One the read didn't give has no waveform (or the library couldn't be read): not asked again.
            if self.pictures.insert(sound, found.remove(&sound).flatten()).is_none() {
                self.order.push_back(sound);
            }
        }
        while self.order.len() > MOST_KEPT {
            if let Some(old) = self.order.pop_front() {
                self.pictures.remove(&old);
            }
        }
        read
    }

    /// Whether the sound was read and has no waveform in the library.
    pub fn has_none(&self, sound: i64) -> bool {
        matches!(self.pictures.get(&sound), Some(None))
    }

    /// Gives a picture to a sound the library has none for ([`Kept::has_none`]): one made from the audio itself, as
    /// it was played. Nothing changes for any other sound.
    pub fn give(&mut self, sound: i64, picture: P) {
        if let Some(kept @ None) = self.pictures.get_mut(&sound) {
            *kept = Some(picture);
        }
    }

    /// Forgets every picture and asking (the screen's scaling changed: they're the wrong size).
    pub fn forget(&mut self) {
        let round = self.round + 1;
        *self = Kept { round, ..Kept::default() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_as_tall_as_each_part_is_loud() {
        // Two peaks over four pixels: the loud half fills the height, the half as loud fills half of it.
        let picture = picture(&Peaks::new(&[1.0, -0.5]), 4, 8).expect("a waveform");
        let alpha = |x: usize, y: usize| picture.as_slice()[y * 4 + x].a;
        assert_eq!((0..8).map(|y| alpha(0, y)).collect::<Vec<_>>(), [255; 8]);
        assert_eq!((0..8).map(|y| alpha(3, y)).collect::<Vec<_>>(), [0, 0, 255, 255, 255, 255, 0, 0]);
        assert_eq!(alpha(1, 0), 255);
        assert_eq!(alpha(2, 0), 0);
    }

    #[test]
    fn silence_is_a_thin_line_and_nothing_is_no_picture() {
        let quiet = picture(&Peaks::new(&[0.0, 0.0]), 2, 4).expect("a waveform");
        assert_eq!(quiet.as_slice().iter().map(|p| p.a).collect::<Vec<_>>(), [0, 0, 128, 128, 128, 128, 0, 0]);
        assert!(picture(&Peaks::default(), 10, 10).is_none());
        assert!(picture(&Peaks::new(&[1.0]), 0, 10).is_none());
    }

    #[test]
    fn more_pixels_than_peaks_still_draws_every_pixel() {
        let picture = picture(&Peaks::new(&[1.0, 1.0, 1.0]), 10, 2).expect("a waveform");
        assert!(picture.as_slice().iter().all(|p| p.a == 255));
    }

    #[test]
    fn pictures_are_read_once_and_kept() {
        let mut kept = Kept::<u8>::default();
        assert_eq!(kept.picture(0, 7), None);
        assert_eq!(kept.picture(0, 7), None);
        assert_eq!(kept.picture(1, 8), None);
        let (round, ids) = kept.to_read().expect("two to read");
        assert_eq!(ids, [7, 8]);
        // Asked while the read is under way: waits its turn.
        assert_eq!(kept.picture(2, 9), None);
        assert_eq!(kept.to_read(), None);
        // 8 has no waveform.
        assert_eq!(kept.take_in(round, vec![(7, Some(1))]), [(0, 7), (1, 8)]);
        assert_eq!((kept.picture(0, 7), kept.picture(1, 8)), (Some(1), None));
        assert_eq!(kept.to_read().map(|(_, ids)| ids), Some(vec![9]));
    }

    #[test]
    fn a_sound_with_no_waveform_can_be_given_one() {
        let mut kept = Kept::<u8>::default();
        kept.picture(0, 7);
        kept.picture(1, 8);
        // Not read yet: nothing to give to.
        assert!(!kept.has_none(8));
        kept.give(8, 5);
        let (round, _) = kept.to_read().expect("two to read");
        kept.take_in(round, vec![(7, Some(1))]);
        assert!(kept.has_none(8) && !kept.has_none(7));
        kept.give(8, 5);
        kept.give(7, 9);
        assert_eq!((kept.picture(0, 7), kept.picture(1, 8)), (Some(1), Some(5)));
    }

    /// The loudest of each part of a sound, for [`picture`].
    #[test]
    fn peaks_of_audio_as_it_was_read() {
        let left = [0.0, 0.5, -1.0, 0.25];
        let right = [0.1, -0.75, 0.5, 0.0];
        assert_eq!(peaks(&[left.to_vec(), right.to_vec()], 2).to_vec(), [0.75, 1.0]);
        assert_eq!(peaks(&[left.to_vec()], 400).len(), 4);
        assert!(peaks(&[], 400).is_empty() && peaks(&[Vec::new()], 400).is_empty());
    }

    #[test]
    fn only_the_newest_are_kept_and_read() {
        let mut kept = Kept::<u8>::default();
        for i in 0..MOST_KEPT + 3 {
            kept.picture(i, i as i64);
        }
        let (round, ids) = kept.to_read().expect("some to read");
        assert_eq!((ids.len(), ids[0]), (MOST_KEPT, 3));
        kept.take_in(round, ids.iter().map(|&i| (i, Some(0))).collect());
        // One more pushes the oldest out; the ones dropped before the read are asked again.
        kept.picture(0, 9999);
        kept.picture(1, 0);
        let (round, ids) = kept.to_read().expect("two to read");
        assert_eq!(ids, [9999, 0]);
        kept.take_in(round, vec![(9999, Some(1)), (0, Some(1))]);
        assert_eq!(kept.pictures.len(), MOST_KEPT);
        assert!(!kept.pictures.contains_key(&3) && !kept.pictures.contains_key(&4));
    }

    #[test]
    fn an_answer_from_before_a_forget_is_dropped() {
        let mut kept = Kept::<u8>::default();
        kept.picture(0, 1);
        let (round, _) = kept.to_read().expect("one to read");
        kept.forget();
        assert_eq!(kept.take_in(round, vec![(1, Some(1))]), []);
        assert_eq!(kept.picture(0, 1), None);
        assert!(kept.to_read().is_some());
    }
}
