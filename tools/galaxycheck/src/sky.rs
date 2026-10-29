//! The pretend sky: 300,000 stars in clusters, like a map of sounds where similar ones sit together. No real sounds are
//! in it; the numbers are made up the same way on every run, so two runs on one PC can be compared.

/// One star as the graphics card reads it: 16 bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Star {
    pub x: f32,
    pub y: f32,
    /// RGBA, one byte each, red in the lowest byte.
    pub colour: u32,
    /// Its size in pixels when the whole sky is on screen.
    pub size: f32,
}

impl Star {
    pub fn bytes(stars: &[Star]) -> Vec<u8> {
        let mut out = Vec::with_capacity(stars.len() * 16);
        for s in stars {
            out.extend_from_slice(&s.x.to_le_bytes());
            out.extend_from_slice(&s.y.to_le_bytes());
            out.extend_from_slice(&s.colour.to_le_bytes());
            out.extend_from_slice(&s.size.to_le_bytes());
        }
        out
    }
}

/// One colour per kind of sound (kick, snare, hat, bass, pad, lead, vocal, fx), like the Sounds page's chips.
const KINDS: [u32; 8] = [0xF0616D, 0xF2B33D, 0x4ACB8B, 0x5B9DF5, 0xB07CF0, 0x3FC7D9, 0xF27FB8, 0xC9D16A];

/// `count` stars in 48 clusters of different sizes, plus a thin scatter between them. Every star lies within -1..1.
pub fn make(count: usize) -> Vec<Star> {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let clusters: Vec<(f32, f32, f32, u32)> = (0..48)
        .map(|i| {
            let (x, y) = (rng.unit() * 1.6 - 0.8, rng.unit() * 1.6 - 0.8);
            let spread = 0.015 + rng.unit() * 0.09;
            (x, y, spread, KINDS[i % KINDS.len()])
        })
        .collect();
    (0..count)
        .map(|i| {
            let (x, y, colour) = if i % 10 == 0 {
                (rng.unit() * 2.0 - 1.0, rng.unit() * 2.0 - 1.0, 0x8A8F99)
            } else {
                let (cx, cy, spread, colour) = clusters[rng.below(clusters.len())];
                let (gx, gy) = rng.gauss2();
                (cx + gx * spread, cy + gy * spread, colour)
            };
            let alpha = 140 + rng.below(100) as u32;
            Star {
                x: x.clamp(-1.0, 1.0),
                y: y.clamp(-1.0, 1.0),
                colour: (colour >> 16 & 0xFF) | (colour & 0xFF00) | ((colour & 0xFF) << 16) | (alpha << 24),
                size: 1.5 + rng.unit() * 2.0,
            }
        })
        .collect()
}

/// Where the camera is: the sky point in the middle of the picture, and how far in (1 shows the whole sky).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

impl Default for View {
    fn default() -> View {
        View { x: 0.0, y: 0.0, zoom: 1.0 }
    }
}

/// The scripted test: each stage moves the camera in its own way for `STAGE_SECONDS`.
pub const STAGES: [&str; 3] = ["Whole sky", "Flying in", "Zoomed in, moving across"];
pub const STAGE_SECONDS: f32 = 6.0;
const DEEPEST: f32 = 64.0;

/// Where the camera is `t` seconds into the scripted test, and which stage that is; `None` once it's over.
pub fn scripted(t: f32) -> Option<(usize, View)> {
    let stage = (t / STAGE_SECONDS) as usize;
    let s = (t % STAGE_SECONDS) / STAGE_SECONDS;
    let target = (0.31, -0.22);
    let view = match stage {
        // The whole sky, drifting a little so every frame is new work.
        0 => {
            View { x: 0.05 * (s * std::f32::consts::TAU).sin(), y: 0.05 * (s * std::f32::consts::TAU).cos(), zoom: 1.0 }
        }
        // Flying in towards one cluster, faster and faster, like a search flying the map.
        1 => View { x: target.0 * s, y: target.1 * s, zoom: DEEPEST.powf(s) },
        // Deep in, moving across clusters.
        2 => View { x: target.0 + 0.4 * s, y: target.1 + 0.2 * (s * std::f32::consts::PI).sin(), zoom: DEEPEST },
        _ => return None,
    };
    Some((stage, view))
}

/// Frame times for one stage.
#[derive(Clone, Debug, Default)]
pub struct Timings {
    pub frames_ms: Vec<f32>,
}

impl Timings {
    /// The middle frame time, the 95th-percentile one, the slowest, and how many took longer than one 60 fps frame.
    pub fn summary(&self) -> Option<(f32, f32, f32, usize)> {
        if self.frames_ms.is_empty() {
            return None;
        }
        let mut sorted = self.frames_ms.clone();
        sorted.sort_by(f32::total_cmp);
        let at = |p: f32| sorted[((sorted.len() - 1) as f32 * p).round() as usize];
        // A frame "late" for 60 fps took over 16.7 ms, with a little room for the screen's own timing.
        let late = sorted.iter().filter(|&&ms| ms > 18.0).count();
        Some((at(0.5), at(0.95), sorted[sorted.len() - 1], late))
    }
}

/// A small, fixed random number maker, so every run draws the same sky.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn gauss2(&mut self) -> (f32, f32) {
        let r = (-2.0 * (1.0 - self.unit()).ln()).sqrt();
        let a = self.unit() * std::f32::consts::TAU;
        (r * a.cos(), r * a.sin())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sky_is_the_same_every_run_and_inside_its_square() {
        let a = make(10_000);
        assert_eq!(a, make(10_000));
        assert!(a.iter().all(|s| (-1.0..=1.0).contains(&s.x) && (-1.0..=1.0).contains(&s.y)));
        assert_eq!(Star::bytes(&a).len(), 10_000 * 16);
    }

    #[test]
    fn the_script_flies_in_and_ends() {
        assert_eq!(scripted(0.0).map(|(s, v)| (s, v.zoom)), Some((0, 1.0)));
        let (stage, deep) = scripted(STAGE_SECONDS * 2.0 - 0.01).expect("still flying");
        assert_eq!(stage, 1);
        assert!(deep.zoom > 60.0);
        assert!(scripted(STAGE_SECONDS * 3.0).is_none());
    }

    #[test]
    fn timings_say_how_many_frames_were_late() {
        let t = Timings { frames_ms: vec![16.0, 16.5, 17.0, 40.0] };
        let (middle, _, slowest, late) = t.summary().expect("some frames");
        assert_eq!((middle, slowest, late), (17.0, 40.0, 1));
    }
}
