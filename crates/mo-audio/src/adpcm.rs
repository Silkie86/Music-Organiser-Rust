//! ADPCM WAV files: Microsoft ADPCM (WAV format 2) and IMA ADPCM (format 0x11), the two squeezed kinds older sample
//! packs ship. Each is a run of fixed-size blocks; every block starts afresh from its own header, so a read can start
//! at any block and a damaged block can't spoil the ones after it.

use crate::AudioError;

/// Microsoft ADPCM.
pub(crate) const MS: u16 = 2;
/// IMA (DVI) ADPCM.
pub(crate) const IMA: u16 = 0x11;

/// The coefficient pairs Microsoft ADPCM uses when a file doesn't list its own.
const MS_COEFS: [(i32, i32); 7] = [(256, 0), (512, -256), (0, 0), (192, 64), (240, 0), (460, -208), (392, -232)];

/// How Microsoft ADPCM's step grows or shrinks after each nibble.
const MS_ADAPT: [i32; 16] = [230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230];

const IMA_INDEX: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

const IMA_STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107,
    118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963,
    1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894,
    6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
];

/// How an ADPCM file's blocks are laid out, from its `fmt ` chunk.
#[derive(Clone, Debug)]
pub(crate) struct Layout {
    pub format: u16,
    pub channels: usize,
    /// Bytes in one block (all channels).
    pub block_bytes: usize,
    /// Frames one whole block gives.
    pub frames_per_block: usize,
    /// Microsoft ADPCM's coefficient pairs (empty for IMA).
    coefs: Vec<(i32, i32)>,
}

impl Layout {
    /// The layout from the `fmt ` chunk's bytes, or why it can't be decoded.
    pub(crate) fn from_fmt(format: u16, channels: u16, fmt: &[u8]) -> Result<Layout, AudioError> {
        let le16 = |at: usize| fmt.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
        let channels = usize::from(channels);
        let block_bytes = usize::from(le16(12).unwrap_or(0));
        let bits = le16(14).unwrap_or(0);
        let head = if format == MS { 7 } else { 4 } * channels;
        if !(1..=2).contains(&channels) || bits != 4 || block_bytes <= head {
            return Err(AudioError::NotSupported(format!(
                "ADPCM WAV with {channels} channels, {bits}-bit, {block_bytes}-byte blocks"
            )));
        }
        let body_frames = (block_bytes - head) * 2 / channels;
        let frames_per_block = if format == MS { body_frames + 2 } else { body_frames + 1 };
        let mut coefs = Vec::new();
        if format == MS {
            // cbSize at 16, samples per block at 18, the number of pairs at 20, then the pairs.
            let n = usize::from(le16(20).unwrap_or(0)).min(256);
            for i in 0..n {
                let (Some(a), Some(b)) = (le16(22 + i * 4), le16(24 + i * 4)) else { break };
                coefs.push((i32::from(a as i16), i32::from(b as i16)));
            }
            if coefs.len() < MS_COEFS.len() {
                coefs = MS_COEFS.to_vec();
            }
        } else if block_bytes.saturating_sub(head) % (4 * channels) != 0 {
            return Err(AudioError::Format("IMA ADPCM block size doesn't fit its channels".into()));
        }
        Ok(Layout { format, channels, block_bytes, frames_per_block, coefs })
    }

    /// Frames in `data_bytes` of blocks; a cut-short last block counts the frames its bytes hold.
    pub(crate) fn frames_in(&self, data_bytes: u64) -> u64 {
        let whole = data_bytes / self.block_bytes as u64;
        let rest = (data_bytes % self.block_bytes as u64) as usize;
        whole * self.frames_per_block as u64 + self.frames_in_part(rest) as u64
    }

    fn frames_in_part(&self, bytes: usize) -> usize {
        let head = if self.format == MS { 7 } else { 4 } * self.channels;
        if bytes < head {
            return 0;
        }
        let body = bytes - head;
        if self.format == MS {
            body * 2 / self.channels + 2
        } else {
            // IMA's body comes in 4 bytes per channel at a time (8 frames).
            body / (4 * self.channels) * 8 + 1
        }
    }

    /// Decodes whole and part blocks into one list per channel, from -1 to 1.
    pub(crate) fn decode(&self, raw: &[u8]) -> Vec<Vec<f32>> {
        let frames = self.frames_in(raw.len() as u64) as usize;
        let mut out: Vec<Vec<f32>> = (0..self.channels).map(|_| Vec::with_capacity(frames)).collect();
        for block in raw.chunks(self.block_bytes) {
            if self.format == MS {
                self.ms_block(block, &mut out);
            } else {
                self.ima_block(block, &mut out);
            }
        }
        out
    }

    fn ms_block(&self, b: &[u8], out: &mut [Vec<f32>]) {
        let ch = self.channels;
        if b.len() < 7 * ch {
            return;
        }
        let i16_at = |at: usize| i32::from(i16::from_le_bytes([b[at], b[at + 1]]));
        // Per channel: predictor, then the step, the newer sample and the older one.
        let mut state: Vec<(i32, i32, i32, i32, i32)> = (0..ch)
            .map(|c| {
                let (c1, c2) = self.coefs.get(usize::from(b[c])).copied().unwrap_or((0, 0));
                (c1, c2, i16_at(ch + c * 2), i16_at(3 * ch + c * 2), i16_at(5 * ch + c * 2))
            })
            .collect();
        for (c, &(_, _, _, s1, s2)) in state.iter().enumerate() {
            out[c].push(s2 as f32 / 32768.0);
            out[c].push(s1 as f32 / 32768.0);
        }
        let mut c = 0;
        for &byte in &b[7 * ch..] {
            for nibble in [byte >> 4, byte & 0x0F] {
                let (c1, c2, delta, s1, s2) = &mut state[c];
                let signed = i32::from(nibble as i8) << 28 >> 28;
                let predicted = ((*s1 * *c1 + *s2 * *c2) >> 8) + signed * *delta;
                let s = predicted.clamp(-32768, 32767);
                *s2 = *s1;
                *s1 = s;
                *delta = ((MS_ADAPT[usize::from(nibble)] * *delta) >> 8).max(16);
                out[c].push(s as f32 / 32768.0);
                c = (c + 1) % ch;
            }
        }
    }

    fn ima_block(&self, b: &[u8], out: &mut [Vec<f32>]) {
        let ch = self.channels;
        if b.len() < 4 * ch {
            return;
        }
        let mut state: Vec<(i32, i32)> = (0..ch)
            .map(|c| {
                let s = i32::from(i16::from_le_bytes([b[c * 4], b[c * 4 + 1]]));
                (s, i32::from(b[c * 4 + 2]).min(88))
            })
            .collect();
        for (c, &(s, _)) in state.iter().enumerate() {
            out[c].push(s as f32 / 32768.0);
        }
        // Then 4 bytes (8 frames) for each channel in turn, the low nibble first.
        for group in b[4 * ch..].chunks_exact(4 * ch) {
            for (c, bytes) in group.as_chunks::<4>().0.iter().enumerate() {
                let (s, index) = &mut state[c];
                for &byte in bytes {
                    for nibble in [byte & 0x0F, byte >> 4] {
                        let step = IMA_STEPS[*index as usize];
                        // The exact form, ((2n + 1) * step) / 8, as ffmpeg and most encoders work it out (the
                        // shift-and-add form found in old decoders drifts from what the encoder meant).
                        let mut diff = ((i32::from(nibble & 7) * 2 + 1) * step) >> 3;
                        if nibble & 8 != 0 {
                            diff = -diff;
                        }
                        *s = (*s + diff).clamp(-32768, 32767);
                        *index = (*index + IMA_INDEX[usize::from(nibble)]).clamp(0, 88);
                        out[c].push(*s as f32 / 32768.0);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(format: u16, channels: u16, block: u16) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&format.to_le_bytes());
        f.extend_from_slice(&channels.to_le_bytes());
        f.extend_from_slice(&8000u32.to_le_bytes());
        f.extend_from_slice(&4000u32.to_le_bytes());
        f.extend_from_slice(&block.to_le_bytes());
        f.extend_from_slice(&4u16.to_le_bytes());
        f
    }

    #[test]
    fn frames_per_block_follow_the_formats() {
        // The usual sizes: 2,036 frames in a 1,024-byte mono MS block, 1,017 in a 512-byte stereo IMA block.
        let ms = Layout::from_fmt(MS, 1, &fmt(MS, 1, 1024)).unwrap();
        assert_eq!(ms.frames_per_block, 2036);
        assert_eq!(ms.coefs, MS_COEFS);
        let ima = Layout::from_fmt(IMA, 2, &fmt(IMA, 2, 1024)).unwrap();
        assert_eq!(ima.frames_per_block, 1017);
        assert!(Layout::from_fmt(MS, 3, &fmt(MS, 3, 1024)).is_err());
        assert!(Layout::from_fmt(MS, 1, &fmt(MS, 1, 7)).is_err());
    }

    #[test]
    fn an_ms_block_starts_with_its_two_samples_then_follows_the_nibbles() {
        let l = Layout::from_fmt(MS, 1, &fmt(MS, 1, 9)).unwrap();
        // Predictor 0 (256, 0), step 16, newer sample 1000, older 500; then nibbles +1 and -1.
        let mut b = vec![0u8];
        for v in [16i16, 1000, 500] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&[0x1F, 0x00]);
        let out = l.decode(&b);
        let got: Vec<i32> = out[0].iter().map(|s| (s * 32768.0).round() as i32).collect();
        // 1000 + 16 = 1016, then 1016 - 16 (the step never drops under 16); zero nibbles keep the newer sample.
        assert_eq!(got, vec![500, 1000, 1016, 1000, 1000, 1000]);
    }

    #[test]
    fn an_ima_block_starts_with_its_sample_then_reads_low_nibbles_first() {
        let l = Layout::from_fmt(IMA, 1, &fmt(IMA, 1, 8)).unwrap();
        let mut b = Vec::new();
        b.extend_from_slice(&100i16.to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        // Low nibble 4 (+9 * 7 / 8 = 7 at step 7), then zero nibbles add step / 8: 1 at steps 9 and 8.
        b.extend_from_slice(&[0x04, 0, 0, 0]);
        let out = l.decode(&b);
        let got: Vec<i32> = out[0].iter().map(|s| (s * 32768.0).round() as i32).collect();
        assert_eq!(got.len(), 9);
        assert_eq!(&got[..4], &[100, 107, 108, 109]);
    }

    #[test]
    fn a_cut_short_block_gives_what_its_bytes_hold() {
        let l = Layout::from_fmt(MS, 1, &fmt(MS, 1, 1024)).unwrap();
        assert_eq!(l.frames_in(1024 + 10), 2036 + 8);
        assert_eq!(l.frames_in(1024 + 3), 2036);
        let raw = vec![0u8; 1024 + 10];
        assert_eq!(l.decode(&raw)[0].len(), 2036 + 8);
    }
}
