//! The Sounds page's galaxy picture on the processor, without a window, for when there's no graphics card to draw
//! with (and for the screenshots): every star drawn as a soft dot on a picture the size of the view, spread over
//! every core. It's kept here (an optimised crate even in the launcher's debug build) because it draws half a million
//! stars.
//!
//! To look like a galaxy rather than a scatter plot, the stars give off light: each one brightens what's under it
//! (a "screen" blend, so a crowd of them glows towards white, the way a galaxy's core does), and behind them a soft
//! cloud of each crowd's colour glows like a nebula, thicker where more sounds are alike. As on the graphics card,
//! close up each star has a white-hot core in a soft glow, and with Light by on the best lit ones get it far out too.

use std::sync::{Arc, LazyLock};

use mo_sounds::StarPoint;
use rayon::prelude::*;

use crate::galaxy_grid::Crowd;
use crate::galaxy_look::{
    self as gl, DUST_GREY, DUST_LIT, DUST_MOST, FALLOFF, FAR_GLOW, FAR_HALO, GLOW, GLOW_LIT, HALO, HOT, LIT_BRIGHTEST,
    Look, MIN_LIT, UNLIT_SIZE, WHITE, lit_size, lit_weight,
};
use crate::galaxy_view::View;

/// The nebula's grid, in points: each square gathers the light of the stars in it, then it's blurred and spread.
const CLOUD: f32 = 5.0;
/// How far the nebula's light spreads, in squares (each blur pass).
const CLOUD_BLUR: usize = 3;
/// How bright the nebula is at its thickest (0 to 1).
const CLOUD_LIGHT: f32 = 0.3;
/// How many stars in a square (after the blur) make the nebula about two thirds as thick as it gets.
const CLOUD_THICK: f32 = 6.0;
/// Up to about this many stars on show, each shines fully; more and each is dimmed.
const BRIGHT_CROWD: f32 = 30_000.0;
/// Each star's glow costs the processor many times its dot, so glows only come in as the stars on the view get few
/// enough: from this many (none) to the lower one (all of it), evenly in doublings so they come in smoothly. With more
/// on the view the picture would take too long to draw, for glows too faint to see.
const GLOWS_FROM: f32 = 8_000.0;
const GLOWS_FULL: f32 = 1_500.0;

/// One pixel, its colour premultiplied by its alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// A picture drawn: what it was asked as, its size in pixels and the pixels row by row.
pub struct Drawn {
    pub id: u64,
    pub view: View,
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<Rgba>,
}

/// A picture of the stars: everything its thread needs.
pub struct Picture {
    pub id: u64,
    pub stars: Arc<Vec<Option<StarPoint>>>,
    /// How many stars are on the map (not `None`).
    pub count: usize,
    /// Each star's place in `palette`.
    pub colours: Arc<Vec<i32>>,
    pub shown: Arc<Vec<usize>>,
    /// How lit each star is (0 to 1), by its place in the list, as [`crate::galaxy_look::light`] gives it; empty
    /// draws every star alike. A lit star is bigger and brighter, on the same curve as on the graphics card.
    pub light: Arc<Vec<f32>>,
    /// The shown stars with the Light by measure from the least lit to the most ([`crate::galaxy_look::lit_order`]):
    /// the last of them glint far out, the same ones as on the graphics card. Empty with nothing lit.
    pub order: Arc<Vec<u32>>,
    /// Every sound type's colour (as `Theme.sound-types`), then grey for a sound with no type.
    pub palette: Vec<[u8; 3]>,
    pub view: View,
    pub scale: f32,
    /// How many shown stars are on the view and how crowded they are (as the map's grid finds them, as for the
    /// graphics card), which decides how bright each star can be and when the glows come in.
    pub crowd: Crowd,
    /// Each star drawn as a star, with a glow, and the best lit as points of their own far out (the graphics card's
    /// look). Off for the mini-map, which only shows the map's shape: there the glows would run into one blob.
    pub glows: bool,
}

/// The rows of the picture drawn together on one thread. A star that reaches into several bands (a big glow close up)
/// is drawn by each of them, each its own rows of it, so the bands stay short and every core gets some.
const BAND: usize = 64;

/// A glow's falloff, (1 - its distance over its reach) to the power [`FALLOFF`], by the distance squared over the reach
/// squared in [`GLOW_STEPS`] steps: looked up, not worked out (a square root and a power) for every pixel of a glow.
const GLOW_STEPS: usize = 4096;
static GLOW_FALLOFF: LazyLock<Vec<f32>> =
    LazyLock::new(|| (0..=GLOW_STEPS).map(|i| (1.0 - (i as f32 / GLOW_STEPS as f32).sqrt()).powf(FALLOFF)).collect());

/// A star to draw: where (in pixels), its size, colour and strength, and how strong its glow is (0 for none) and
/// how far it reaches (in pixels).
#[derive(Clone, Copy)]
struct Dot {
    x: f32,
    y: f32,
    r: f32,
    colour: [u8; 3],
    alpha: f32,
    glow: f32,
    halo: f32,
    /// How lit it is (0 to 1), for its glow.
    lit: f32,
}

impl Picture {
    /// Draws the picture, spread over every core: the stars are placed on the view at once, then each band of rows
    /// draws the ones that touch it. With half a million stars one thread was too slow to keep up with the wheel.
    pub fn draw(self) -> Drawn {
        let (w, h) =
            ((self.view.width * self.scale).round() as usize, (self.view.height * self.scale).round() as usize);
        let mut pixels = vec![Rgba::default(); w * h];
        let mut on = vec![false; self.stars.len()];
        for &at in self.shown.iter() {
            if let Some(o) = on.get_mut(at) {
                *o = true;
            }
        }
        // A small library gets bigger stars, so a few dozen don't look like dust; and they grow a little as the map is
        // zoomed in, so a close look shows each one.
        let n = self.count.max(1) as f32;
        let base = (200.0 / n.sqrt()).clamp(1.2, 4.5);
        let r = self.scale * (base + (self.view.zoom - 1.0).max(0.0).sqrt() * 0.5).min(6.0);
        let grey = self.palette.last().copied().unwrap_or([128, 128, 128]);
        // Light adds up, so when a great many stars are on show each gives off less, or the crowds would all burn
        // white; zoomed in, or with the filters leaving few, fewer are on show and each shines fully.
        let on_show = self.crowd.in_view.max(1) as f32;
        let bright = (0.9 * (BRIGHT_CROWD / on_show).sqrt()).clamp(0.3, 0.9);
        // How big and bright each shown star is for how lit it is (all alike with nothing on show lit), and how
        // strongly it glows: every star close up, only the best lit far out (the graphics card's look).
        let share = gl::lit_share(&self.stars, &self.shown, &self.light);
        let lighting = share > 0.0;
        let lk = gl::look(&self.view, self.crowd, share);
        let crisp = glows_in(&lk, on_show);
        let painted = 1.0 - crisp;
        // How strongly each of the best lit shows as a point of its own far out: by its rank, as on the card.
        let glint = {
            let lit = u32::try_from(self.order.len()).unwrap_or(u32::MAX);
            let (glinting, _) = gl::glinting(&lk, lit);
            let mut glint = Vec::new();
            if self.glows && lighting && painted > 0.0 && glinting > 0 {
                glint = vec![0.0f32; self.stars.len()];
                let first = (lit - glinting) as usize;
                for (k, &at) in self.order[first..].iter().enumerate() {
                    if let Some(g) = glint.get_mut(at as usize) {
                        *g = gl::glint_at(k as u32, glinting);
                    }
                }
            }
            glint
        };
        // Its radius, how strong its core is, how strongly it glows and how lit it is.
        let lit = |at: usize| -> (f32, f32, f32, f32) {
            if !lighting {
                return (r, 1.0, crisp, 0.0);
            }
            let l = self.light.get(at).copied().unwrap_or(0.0);
            let glint = glint.get(at).copied().unwrap_or(0.0);
            // As the card: far out a star that glints is a bright point, going to its close-up size as the view flies
            // in. Its core is as bright as it's lit, with the same steady variety as on the card.
            let size = gl::glint_point(r * UNLIT_SIZE * lit_size(l), glint, crisp, MIN_LIT * self.scale);
            (size, lit_weight(l) * gl::variety(at) / LIT_BRIGHTEST, crisp.max(painted * glint), l)
        };
        // The biggest a star gets, and how far it reaches: a glow reaches HALO times its star's core, a well lit
        // one's up to twice that.
        let largest = if lighting { (r * UNLIT_SIZE * lit_size(1.0)).max(MIN_LIT * self.scale * 1.6) } else { r };
        let biggest = if self.glows { largest * HALO * if lighting { 2.0 } else { 1.0 } } else { largest } + 1.0;
        let bands = h.div_ceil(BAND).max(1);
        // A left-out star is faint grey dust, fainter still with Light by on, when the unlit stars on show are dimmed
        // too (and however many crowd together, they never get past a faint haze: `dust`).
        let dust = 0.16 * if lighting { DUST_LIT } else { 1.0 };
        // The nebula's squares, lined up with the map so its light holds still as the map is dragged, and the stars
        // just past the picture's edges, whose light spreads onto it.
        let size = (CLOUD * self.scale).max(2.0);
        let (ox, oy) = self.view.to_view(0.0, 0.0, self.scale);
        let reach = 2 * CLOUD_BLUR;
        let grid = (ox.rem_euclid(size) - reach as f32 * size, oy.rem_euclid(size) - reach as f32 * size);
        let margin = biggest.max(-grid.0).max(-grid.1);
        // Every star on the picture once for each band it reaches into (marked on the first, for the nebula, which
        // counts each star once); sorted so each band's stars are together, the ones the filters leave out (drawn
        // faint) before the shown ones (drawn over them), and otherwise in list order, so stars that overlap are
        // always drawn the same way round.
        let mut placed: Vec<(usize, bool, usize, bool, Dot)> = self
            .stars
            .par_iter()
            .enumerate()
            .filter_map(|(at, star)| {
                let p = (*star)?;
                let (x, y) = self.view.to_view(p.x, p.y, self.scale);
                if x < -margin || y < -margin || x > w as f32 + margin || y > h as f32 + margin {
                    return None;
                }
                let colour = self
                    .colours
                    .get(at)
                    .and_then(|&i| usize::try_from(i).ok())
                    .and_then(|i| self.palette.get(i))
                    .copied()
                    .unwrap_or(grey);
                let shown = on[at];
                let (radius, strength, glow, l) = if shown { lit(at) } else { (r * 0.8, dust, 0.0, 0.0) };
                let alpha = bright * strength;
                let halo = radius * (FAR_HALO + (HALO + HALO * l - FAR_HALO) * crisp);
                // A glow is as strong as its star's core is lit, as on the card (an unlit star's is faint), and never
                // dimmed quite as far as the stars are in a crowd, or far out it would go.
                let glow = if self.glows { glow * strength * bright.max(0.5) } else { 0.0 };
                let dot = Dot {
                    x,
                    y,
                    r: radius,
                    colour,
                    alpha,
                    glow: glow * (FAR_GLOW + (1.0 - FAR_GLOW) * crisp),
                    halo,
                    lit: l,
                };
                // Its top and bottom edges: its glow's when it has one.
                let reach = if dot.glow > 0.02 { dot.halo.max(dot.r + 1.0) } else { dot.r + 1.0 };
                let band_of = |y: f32| (y.max(0.0) as usize / BAND).min(bands - 1);
                Some((band_of(y - reach), band_of(y + reach), shown, at, dot))
            })
            .flat_map_iter(|(top, bottom, shown, at, dot)| {
                (top..=bottom).map(move |band| (band, shown, at, band == top, dot))
            })
            .collect();
        placed.par_sort_unstable_by_key(|p| (p.0, p.1, p.2));
        let cloud = Cloud::new(&placed, w, h, size, grid);
        // Where each band's stars start.
        let starts: Vec<usize> = (0..=bands).map(|b| placed.partition_point(|p| p.0 < b)).collect();
        pixels.par_chunks_mut((w * BAND).max(1)).enumerate().for_each(|(band, rows)| {
            let y0 = band * BAND;
            cloud.light(rows, w, y0);
            for p in &placed[starts[band]..starts[band + 1]] {
                if p.1 {
                    disc(rows, w, y0, &p.4);
                } else {
                    speck(rows, w, y0, &p.4);
                }
            }
        });
        Drawn { id: self.id, view: self.view, width: w, height: h, pixels }
    }
}

/// How strongly the stars glow on the processor's picture with `look`, with `on_show` stars on the view (0 to 1): as
/// they're drawn as stars on the card, but only from [`GLOWS_FROM`] stars on the view down, all of it by
/// [`GLOWS_FULL`]. The card's stars come in as soon as they're far enough apart to tell apart, many thousands of them;
/// on the processor the glows of so many would take too long to draw.
fn glows_in(look: &Look, on_show: f32) -> f32 {
    (1.0 - look.painted).min(gl::smooth(GLOWS_FROM.ln(), GLOWS_FULL.ln(), on_show.max(1.0).ln()))
}

/// Draws a star over what's there (colours premultiplied by their alpha), on the rows in `rows`: a band of the
/// picture `w` pixels wide whose first row is row `y0` of the whole picture. Its glow first (when it has one), then
/// its soft-edged core, white-hot in the middle.
fn disc(rows: &mut [Rgba], w: usize, y0: usize, d: &Dot) {
    let h = rows.len() / w.max(1);
    let (x, y, r) = (d.x, d.y - y0 as f32, d.r);
    if d.glow > 0.02 {
        let reach = d.halo.max(r + 1.0);
        let (top, bottom) = ((y - reach).floor().max(0.0) as usize, ((y + reach).ceil().max(0.0) as usize).min(h));
        let falloff = &*GLOW_FALLOFF;
        let steps = GLOW_STEPS as f32 / (reach * reach);
        let strength = (GLOW + GLOW_LIT * d.lit) * d.glow;
        for py in top..bottom {
            let dy = (py as f32 + 0.5 - y).powi(2);
            // Only the pixels on this row inside the glow's circle.
            let half = (reach * reach - dy).max(0.0).sqrt();
            let (x0, x1) = ((x - half).floor().max(0.0) as usize, ((x + half).ceil().max(0.0) as usize).min(w));
            for px in x0..x1 {
                let d2 = (px as f32 + 0.5 - x).powi(2) + dy;
                // The graphics card's falloff: soft from the core, gone by the glow's edge.
                let Some(&f) = falloff.get((d2 * steps) as usize) else { continue };
                let a = strength * f;
                if a > 0.004 {
                    screen(&mut rows[py * w + px], d.colour, a);
                }
            }
        }
    }
    let (x0, x1) = ((x - r - 1.0).floor().max(0.0) as usize, ((x + r + 1.0).ceil().max(0.0) as usize).min(w));
    let (top, bottom) = ((y - r - 1.0).floor().max(0.0) as usize, ((y + r + 1.0).ceil().max(0.0) as usize).min(h));
    let outer = (r + 0.5).powi(2);
    let inner = (r - 0.5).max(0.0).powi(2);
    for py in top..bottom {
        let dy = (py as f32 + 0.5 - y).powi(2);
        for px in x0..x1 {
            let d2 = (px as f32 + 0.5 - x).powi(2) + dy;
            // Only the soft edge needs the square root.
            let a = if d2 >= outer {
                continue;
            } else if d2 <= inner {
                d.alpha
            } else {
                (r + 0.5 - d2.sqrt()).clamp(0.0, 1.0) * d.alpha
            };
            screen(&mut rows[py * w + px], d.colour, a);
            // The middle heads for white.
            if d.glow > 0.02 {
                let hot = WHITE * d.glow * (-d2 / (HOT * r * r + 0.15)).exp();
                if hot > 0.004 {
                    screen(&mut rows[py * w + px], [255, 255, 255], hot);
                }
            }
        }
    }
}

/// Draws a left-out star as grey dust (only its core, at its strength) on the rows in `rows`, as [`disc`] does: the
/// fuller of dust a pixel already is, the less each speck adds, so it never goes past [`DUST_MOST`] grey.
fn speck(rows: &mut [Rgba], w: usize, y0: usize, d: &Dot) {
    let h = rows.len() / w.max(1);
    let (x, y, r) = (d.x, d.y - y0 as f32, d.r);
    let (x0, x1) = ((x - r - 1.0).floor().max(0.0) as usize, ((x + r + 1.0).ceil().max(0.0) as usize).min(w));
    let (top, bottom) = ((y - r - 1.0).floor().max(0.0) as usize, ((y + r + 1.0).ceil().max(0.0) as usize).min(h));
    for py in top..bottom {
        let dy = (py as f32 + 0.5 - y).powi(2);
        for px in x0..x1 {
            let d2 = (px as f32 + 0.5 - x).powi(2) + dy;
            let a = (r + 0.5 - d2.sqrt()).clamp(0.0, 1.0) * d.alpha;
            if a > 0.0 {
                lay_dust(&mut rows[py * w + px], a);
            }
        }
    }
}

/// The grey of the left-out stars' dust (0 to 255).
fn dust_grey() -> u8 {
    (DUST_GREY * 255.0).round() as u8
}

/// Lays grey dust on a pixel at strength `a` (0 to 1), only as far as it's under [`DUST_MOST`] alpha: however much
/// dust lands on it, it stays a faint grey haze. (The shown stars' light is screened on after it.)
fn lay_dust(o: &mut Rgba, a: f32) {
    let room = (1.0 - o.a as f32 / (DUST_MOST * 255.0)).max(0.0);
    if room > 0.0 {
        let g = dust_grey();
        screen(o, [g, g, g], a * room);
    }
}

/// Lights a pixel with `colour` at strength `a` (0 to 1): a "screen" blend, so light only ever adds, and a lot of it
/// heads for white without spilling over. The picture's alpha follows its brightest colour, so it stays premultiplied.
fn screen(o: &mut Rgba, colour: [u8; 3], a: f32) {
    let add = |was: u8, c: u8| {
        let was = was as f32;
        (was + (c as f32 * a) * (1.0 - was / 255.0)).round().min(255.0) as u8
    };
    o.r = add(o.r, colour[0]);
    o.g = add(o.g, colour[1]);
    o.b = add(o.b, colour[2]);
    o.a = o.a.max(o.r).max(o.g).max(o.b).max(add(o.a, 255));
}

/// The nebula behind the stars: the light of the stars in each square of a coarse grid, blurred.
struct Cloud {
    columns: usize,
    rows: usize,
    /// Pixels per square.
    size: f32,
    /// Where the first square's top left corner is on the picture, in pixels (up to the blur's reach before it).
    origin: (f32, f32),
    /// Each square's colour (the shown stars' light, averaged), how thick it is (0 to `CLOUD_LIGHT`) and how thick
    /// the left-out stars' grey dust is there (0 to [`DUST_MOST`]).
    squares: Vec<([f32; 3], f32, f32)>,
}

impl Cloud {
    fn new(placed: &[(usize, bool, usize, bool, Dot)], w: usize, h: usize, size: f32, origin: (f32, f32)) -> Cloud {
        let columns = ((w as f32 - origin.0) / size).ceil() as usize + 2 * CLOUD_BLUR + 1;
        let rows = ((h as f32 - origin.1) / size).ceil() as usize + 2 * CLOUD_BLUR + 1;
        let cells = columns * rows;
        // Added up on every core: each core's share of the stars into a grid of its own, then the grids together.
        let share = placed.len().div_ceil(rayon::current_num_threads().max(1)).max(1);
        let add = |mut sums: Vec<[f32; 5]>, (_, shown, _, first, d): &(usize, bool, usize, bool, Dot)| {
            let (fx, fy) = ((d.x - origin.0) / size, (d.y - origin.1) / size);
            // Each star once: on the first band it's in.
            if !first || fx < 0.0 || fy < 0.0 || fx as usize >= columns || fy as usize >= rows {
                return sums;
            }
            let s = &mut sums[fy as usize * columns + fx as usize];
            if *shown {
                s[0] += d.colour[0] as f32;
                s[1] += d.colour[1] as f32;
                s[2] += d.colour[2] as f32;
                s[3] += 1.0;
            } else {
                // A star the filters leave out still glows a little, as grey dust.
                s[4] += 0.15;
            }
            sums
        };
        let sums = placed.par_chunks(share).map(|stars| stars.iter().fold(vec![[0.0f32; 5]; cells], add)).reduce(
            || vec![[0.0f32; 5]; cells],
            |mut a, b| {
                for (a, b) in a.iter_mut().zip(&b) {
                    for j in 0..5 {
                        a[j] += b[j];
                    }
                }
                a
            },
        );
        let mut sums = sums;
        // Blurred twice each way, which is close to a soft round spread.
        for _ in 0..2 {
            sums = blur(&sums, columns, rows, true);
            sums = blur(&sums, columns, rows, false);
        }
        let squares = sums
            .iter()
            .map(|s| {
                let dust = DUST_MOST * (1.0 - (-s[4] / CLOUD_THICK).exp());
                if s[3] <= 1e-4 {
                    return ([0.0; 3], 0.0, dust);
                }
                let colour = [s[0] / s[3], s[1] / s[3], s[2] / s[3]];
                (colour, CLOUD_LIGHT * (1.0 - (-s[3] / CLOUD_THICK).exp()), dust)
            })
            .collect();
        Cloud { columns, rows, size, origin, squares }
    }

    /// Lights the rows in `rows` (a band `w` pixels wide starting at row `y0`) with the nebula, smoothly between
    /// the squares' middles.
    fn light(&self, rows: &mut [Rgba], w: usize, y0: usize) {
        let at = |x: usize, y: usize| self.squares[y.min(self.rows - 1) * self.columns + x.min(self.columns - 1)];
        for (i, row) in rows.chunks_mut(w.max(1)).enumerate() {
            let fy = (((y0 + i) as f32 + 0.5 - self.origin.1) / self.size - 0.5).max(0.0);
            let (gy, ty) = (fy as usize, fy.fract());
            for (x, o) in row.iter_mut().enumerate() {
                let fx = ((x as f32 + 0.5 - self.origin.0) / self.size - 0.5).max(0.0);
                let (gx, tx) = (fx as usize, fx.fract());
                let corners = [
                    (at(gx, gy), (1.0 - tx) * (1.0 - ty)),
                    (at(gx + 1, gy), tx * (1.0 - ty)),
                    (at(gx, gy + 1), (1.0 - tx) * ty),
                    (at(gx + 1, gy + 1), tx * ty),
                ];
                let mut colour = [0.0f32; 3];
                let mut thick = 0.0;
                let mut dust = 0.0;
                for ((c, t, d), k) in corners {
                    dust += d * k;
                    let k = k * t;
                    thick += k;
                    for j in 0..3 {
                        colour[j] += c[j] * k;
                    }
                }
                if dust > 1e-3 {
                    let g = dust_grey();
                    screen(o, [g, g, g], dust);
                }
                if thick <= 1e-3 {
                    continue;
                }
                let colour = colour.map(|c| (c / thick).round().min(255.0) as u8);
                screen(o, colour, thick);
            }
        }
    }
}

/// One pass of a box blur over a grid of sums, along its rows (`across`) or down its columns.
fn blur(sums: &[[f32; 5]], columns: usize, rows: usize, across: bool) -> Vec<[f32; 5]> {
    let r = CLOUD_BLUR as isize;
    let k = 1.0 / (2 * r + 1) as f32;
    let mut out = vec![[0.0f32; 5]; sums.len()];
    out.par_chunks_mut(columns.max(1)).enumerate().for_each(|(y, line)| {
        for (x, cell) in line.iter_mut().enumerate() {
            let mut acc = [0.0f32; 5];
            for d in -r..=r {
                let (sx, sy) = if across { (x as isize + d, y as isize) } else { (x as isize, y as isize + d) };
                if sx < 0 || sy < 0 || sx as usize >= columns || sy as usize >= rows {
                    continue;
                }
                let s = sums[sy as usize * columns + sx as usize];
                for j in 0..5 {
                    acc[j] += s[j];
                }
            }
            *cell = acc.map(|a| a * k);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disc_covers_its_middle_and_not_far_away() {
        let mut buf = vec![Rgba::default(); 100];
        let dot = Dot { x: 5.0, y: 5.0, r: 1.5, colour: [200, 100, 50], alpha: 1.0, glow: 0.0, halo: 0.0, lit: 0.0 };
        disc(&mut buf, 10, 0, &dot);
        assert_eq!(buf[5 * 10 + 5].a, 255);
        assert_eq!(buf[0].a, 0);
    }

    #[test]
    fn a_glowing_star_lights_round_it_and_its_middle_heads_for_white() {
        let mut plain = vec![Rgba::default(); 400];
        let mut glowing = plain.clone();
        let dot = Dot { x: 10.0, y: 10.0, r: 2.0, colour: [40, 80, 200], alpha: 0.6, glow: 0.0, halo: 0.0, lit: 0.0 };
        disc(&mut plain, 20, 0, &dot);
        disc(&mut glowing, 20, 0, &Dot { glow: 1.0, halo: 6.0, ..dot });
        // Three pixels out, past the core: only the glowing one lights it.
        assert_eq!(plain[10 * 20 + 13].a, 0);
        assert!(glowing[10 * 20 + 13].a > 0);
        // The middle is whiter: its red (the colour's weakest) is lifted most.
        assert!(glowing[10 * 20 + 10].r > plain[10 * 20 + 10].r + 40);
        // Nothing past the glow's reach.
        assert_eq!(glowing[10 * 20 + 19].a, 0);
    }

    #[test]
    fn a_disc_lands_on_the_right_rows_of_a_band() {
        // The band holds rows 4 to 7 of the picture; the dot sits on row 5, the band's second row.
        let mut band = vec![Rgba::default(); 40];
        let dot = Dot { x: 5.0, y: 5.5, r: 1.0, colour: [200, 100, 50], alpha: 1.0, glow: 0.0, halo: 0.0, lit: 0.0 };
        disc(&mut band, 10, 4, &dot);
        assert_eq!(band[10 + 5].a, 255);
        assert_eq!(band[3 * 10 + 5].a, 0);
    }

    #[test]
    fn light_adds_up_towards_white_and_never_spills_over() {
        let mut o = Rgba::default();
        screen(&mut o, [255, 100, 200], 1.0);
        assert_eq!((o.r, o.g, o.b, o.a), (255, 100, 200, 255));
        for _ in 0..40 {
            screen(&mut o, [255, 100, 200], 0.9);
        }
        assert!(o.g > 240 && o.b > 250, "a crowd glows white: {o:?}");
        let mut faint = Rgba::default();
        screen(&mut faint, [200, 100, 50], 0.2);
        assert!(faint.a >= faint.r && faint.a >= faint.g && faint.a >= faint.b, "stays premultiplied: {faint:?}");
    }

    #[test]
    fn a_crowd_of_stars_glows_round_it_and_empty_sky_stays_dark() {
        let mut view = View::default();
        (view.width, view.height) = (200.0, 200.0);
        let stars: Vec<Option<StarPoint>> =
            (0..50).map(|i| Some(StarPoint { x: 0.5 + (i % 7) as f32 * 0.002, y: 0.5 })).collect();
        let n = stars.len();
        let picture = Picture {
            id: 1,
            stars: Arc::new(stars),
            count: n,
            colours: Arc::new(vec![0; n]),
            shown: Arc::new((0..n).collect()),
            light: Arc::default(),
            order: Arc::default(),
            palette: vec![[200, 100, 50], [110, 110, 110]],
            view,
            scale: 1.0,
            crowd: Crowd::even(&view, n),
            glows: true,
        };
        let d = picture.draw();
        // Beside the crowd, between the stars' own discs, the nebula glows in their colour.
        let near = d.pixels[(100 + 8) * d.width + 104];
        assert!(near.r > 3 && near.r > near.b, "{near:?}");
        // A corner far from it is dark.
        assert_eq!(d.pixels[5 * d.width + 5], Rgba::default());
    }

    #[test]
    fn a_lit_star_is_drawn_bigger_and_brighter() {
        // Two stars far apart, the left one unlit and the right one fully lit.
        let mut view = View::default();
        (view.width, view.height) = (200.0, 100.0);
        let stars = vec![Some(StarPoint { x: 0.25, y: 0.5 }), Some(StarPoint { x: 0.75, y: 0.5 })];
        let light = vec![0.0, 1.0];
        let picture = Picture {
            id: 1,
            order: Arc::new(gl::lit_order(&stars, &[0, 1], &light)),
            stars: Arc::new(stars),
            count: 2,
            colours: Arc::new(vec![0, 0]),
            shown: Arc::new(vec![0, 1]),
            light: Arc::new(light),
            palette: vec![[200, 100, 50], [110, 110, 110]],
            view,
            scale: 4.0,
            crowd: Crowd::even(&view, 2),
            glows: true,
        };
        let d = picture.draw();
        let (w, h) = (d.width, d.height);
        let (left, y) = d.view.to_view(0.25, 0.5, 4.0);
        let (right, _) = d.view.to_view(0.75, 0.5, 4.0);
        let row = (y.round() as usize).min(h - 1);
        // How many pixels across each star's middle row are lit well above the nebula round it.
        let across = |from: usize, to: usize| (from..to).filter(|&x| d.pixels[row * w + x].r > 60).count();
        let half = ((left + right) / 2.0) as usize;
        let (unlit, lit) = (across(0, half), across(half, w));
        assert!(lit > unlit * 2, "lit {lit} pixels across, unlit {unlit}");
        let middle = |x: f32| d.pixels[row * w + (x.round() as usize).min(w - 1)];
        assert!(middle(right).r > middle(left).r, "{:?} {:?}", middle(right), middle(left));
    }

    /// How many pixels across the middle row of the star at map point (`x`, 0.5) are lit well above the sky.
    fn width_of(d: &Drawn, x: f32) -> usize {
        let (cx, cy) = d.view.to_view(x, 0.5, d.width as f32 / d.view.width);
        let row = (cy as usize).min(d.height - 1);
        let (from, to) = ((cx - 40.0).max(0.0) as usize, ((cx + 40.0) as usize).min(d.width));
        (from..to).filter(|&px| d.pixels[row * d.width + px].a > 60).count()
    }

    #[test]
    fn far_out_a_star_that_glints_is_never_drawn_smaller_than_a_less_lit_one() {
        // A thousand lit stars, all but two off the view, with the view far out (as with 450,000 on it): the best lit
        // few glint. The left star is the one just under them, the right one the best lit of all.
        let mut view = View::default();
        (view.width, view.height) = (200.0, 100.0);
        let mut stars = vec![Some(StarPoint { x: 5.0, y: 5.0 }); 1_000];
        stars[993] = Some(StarPoint { x: 0.25, y: 0.5 });
        stars[999] = Some(StarPoint { x: 0.75, y: 0.5 });
        let light: Vec<f32> = (0..1_000).map(|i| (i + 1) as f32 / 1_000.0).collect();
        let shown: Vec<usize> = (0..1_000).collect();
        let order = gl::lit_order(&stars, &shown, &light);
        let lk = gl::look(&view, Crowd::even(&view, 450_000), 1.0);
        let (glinting, _) = gl::glinting(&lk, 1_000);
        assert!(glinting > 1 && glinting < 7, "{glinting}");
        let d = Picture {
            id: 1,
            stars: Arc::new(stars),
            count: 1_000,
            colours: Arc::new(vec![0; 1_000]),
            shown: Arc::new(shown),
            light: Arc::new(light),
            order: Arc::new(order),
            palette: vec![[200, 100, 50], [110, 110, 110]],
            view,
            scale: 1.0,
            crowd: Crowd::even(&view, 450_000),
            glows: true,
        }
        .draw();
        let (under, best) = (width_of(&d, 0.25), width_of(&d, 0.75));
        assert!(under > 0 && best >= under, "the best lit {best} pixels across, the one under it {under}");
    }

    #[test]
    fn the_mini_map_draws_plain_dots() {
        // One star close up: drawn as a star it has a glow round it; on the mini-map only its dot.
        let mut view = View::default();
        (view.width, view.height) = (40.0, 40.0);
        let picture = |glows: bool| Picture {
            id: 1,
            stars: Arc::new(vec![Some(StarPoint { x: 0.5, y: 0.5 })]),
            count: 1,
            colours: Arc::new(vec![0]),
            shown: Arc::new(vec![0]),
            light: Arc::default(),
            order: Arc::default(),
            palette: vec![[200, 100, 50], [110, 110, 110]],
            view,
            scale: 1.0,
            crowd: Crowd::even(&view, 1),
            glows,
        };
        let (star, dot) = (picture(true).draw(), picture(false).draw());
        let beside = |d: &Drawn| {
            let (x, y) = d.view.to_view(0.5, 0.5, 1.0);
            d.pixels[y as usize * d.width + (x + 7.0) as usize]
        };
        assert!(beside(&star).r > beside(&dot).r + 10, "{:?} {:?}", beside(&star), beside(&dot));
    }

    #[test]
    fn a_big_star_on_a_band_edge_is_drawn_whole() {
        // At a scale of 8 the one star is 72 pixels across, taller than the fewest rows in a band, and it crosses a
        // band edge.
        let mut view = View::default();
        (view.width, view.height) = (40.0, 40.0);
        let picture = Picture {
            id: 1,
            stars: Arc::new(vec![Some(StarPoint { x: 0.5, y: 0.5 })]),
            count: 1,
            colours: Arc::new(vec![0]),
            shown: Arc::new(vec![0]),
            light: Arc::default(),
            order: Arc::default(),
            palette: vec![[200, 100, 50], [110, 110, 110]],
            view,
            scale: 8.0,
            crowd: Crowd::even(&view, 1),
            glows: true,
        };
        let d = picture.draw();
        let (w, middle) = (d.width, d.width / 2);
        // The middle column is drawn all the way from a little above the middle to a little below.
        for y in middle - 30..middle + 30 {
            assert!(d.pixels[y * w + middle].a > 200, "row {y}");
        }
    }

    #[test]
    fn a_glow_reaching_into_several_bands_is_drawn_by_each() {
        // At a scale of 8, close up, a well lit star's glow reaches over a hundred pixels each way, into several bands
        // of rows: it falls off the same way up and down, and on to its edge, wherever the bands split it.
        let mut view = View::default();
        (view.width, view.height) = (40.0, 40.0);
        let d = Picture {
            id: 1,
            stars: Arc::new(vec![Some(StarPoint { x: 0.5, y: 0.5 })]),
            count: 1,
            colours: Arc::new(vec![0]),
            shown: Arc::new(vec![0]),
            light: Arc::new(vec![1.0]),
            order: Arc::new(vec![0]),
            palette: vec![[200, 100, 50], [110, 110, 110]],
            view,
            scale: 8.0,
            crowd: Crowd::even(&view, 1),
            glows: true,
        }
        .draw();
        let (w, middle) = (d.width, d.height / 2);
        let column = |y: usize| d.pixels[y * w + w / 2];
        // Over a hundred rows from the middle each way, well past two bands of rows, it still glows.
        assert!(column(middle - 110).r > 0 && column(middle + 110).r > 0);
        // The same up as down (but for the nebula behind it, which its grid of squares puts a little off the middle):
        // a band that missed it would cut its glow off at the band's edge.
        for k in 1..140 {
            let (up, down) = (column(middle - k).r, column(middle + k - 1).r);
            assert!(up.abs_diff(down) <= 10, "{k} rows from the middle: {up} up, {down} down");
        }
    }

    #[test]
    fn with_light_by_on_the_left_out_stars_are_fainter_dust() {
        // Two stars far apart, close up: the left one left out by the filters, the right one shown.
        let mut view = View::default();
        (view.width, view.height) = (200.0, 100.0);
        let picture = |light: Vec<f32>, order: Vec<u32>| Picture {
            id: 1,
            stars: Arc::new(vec![Some(StarPoint { x: 0.25, y: 0.5 }), Some(StarPoint { x: 0.75, y: 0.5 })]),
            count: 2,
            colours: Arc::new(vec![0, 0]),
            shown: Arc::new(vec![1]),
            light: Arc::new(light),
            order: Arc::new(order),
            palette: vec![[200, 100, 50], [110, 110, 110]],
            view,
            scale: 1.0,
            crowd: Crowd::even(&view, 1),
            glows: true,
        };
        let dust = |d: &Drawn| {
            let (x, y) = d.view.to_view(0.25, 0.5, 1.0);
            d.pixels[y as usize * d.width + x as usize].a
        };
        let (off, on) = (picture(Vec::new(), Vec::new()).draw(), picture(vec![0.0, 1.0], vec![1]).draw());
        assert!(dust(&on) > 0, "still there");
        assert!(dust(&on) < dust(&off), "fainter with Light by on: {} against {}", dust(&on), dust(&off));
    }

    #[test]
    fn however_many_left_out_stars_crowd_together_they_stay_faint_grey_dust() {
        // 20,000 left-out stars in a red crowd on the left, one shown red star on the right, far out.
        let mut view = View::default();
        (view.width, view.height) = (200.0, 100.0);
        let mut stars: Vec<Option<StarPoint>> = (0..20_000)
            .map(|i| Some(StarPoint { x: 0.2 + (i % 100) as f32 * 0.0005, y: 0.45 + (i / 100) as f32 * 0.0005 }))
            .collect();
        stars.push(Some(StarPoint { x: 0.75, y: 0.5 }));
        let n = stars.len();
        let d = Picture {
            id: 1,
            stars: Arc::new(stars),
            count: n,
            colours: Arc::new(vec![0; n]),
            shown: Arc::new(vec![n - 1]),
            light: Arc::default(),
            order: Arc::default(),
            palette: vec![[230, 40, 40], [110, 110, 110]],
            view,
            scale: 1.0,
            crowd: Crowd::even(&view, 1),
            glows: true,
        }
        .draw();
        let most = (DUST_MOST * 255.0).ceil() as u8 + 2;
        let mut dusty = 0;
        for (i, px) in d.pixels.iter().enumerate() {
            if i % d.width < d.width / 2 && px.a > 0 {
                dusty += 1;
                assert!(px.a <= most, "faint: alpha {} at {i}", px.a);
                assert!(px.r.abs_diff(px.g) <= 2 && px.g.abs_diff(px.b) <= 2, "grey: {px:?} at {i}");
            }
        }
        assert!(dusty > 0, "the dust is still there");
        let (x, y) = d.view.to_view(0.75, 0.5, 1.0);
        let star = d.pixels[y as usize * d.width + x as usize];
        assert!(star.r > star.g + 40 && star.a > most, "the shown star keeps its colour: {star:?}");
    }
}

#[cfg(test)]
mod look {
    use super::*;

    /// Draws 450,000 made-up stars in clumps to a PPM file named by GALAXY_LOOK, to judge the look by eye. With
    /// GALAXY_FEW=n the filters let only one star in n through (318: as Kick with Favourites on a big library).
    #[test]
    #[ignore]
    fn render() {
        let Ok(out) = std::env::var("GALAXY_LOOK") else { return };
        let zoom: f32 = std::env::var("GALAXY_ZOOM").ok().and_then(|z| z.parse().ok()).unwrap_or(1.0);
        let mut seed = 12345u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 1_000_000) as f32 / 1_000_000.0
        };
        let mut stars = Vec::new();
        let mut colours = Vec::new();
        let clumps: Vec<(f32, f32, f32, i32)> =
            (0..40).map(|i| (0.1 + rnd() * 0.8, 0.1 + rnd() * 0.8, 0.01 + rnd() * 0.08, i % 8)).collect();
        for i in 0..450_000 {
            let c = clumps[i % clumps.len()];
            let (a, r) = (rnd() * std::f32::consts::TAU, (-(rnd().max(1e-6)).ln()).sqrt() * c.2);
            stars.push(Some(StarPoint {
                x: (c.0 + a.cos() * r).clamp(0.0, 1.0),
                y: (c.1 + a.sin() * r).clamp(0.0, 1.0),
            }));
            colours.push(if rnd() < 0.2 { (rnd() * 8.0) as i32 } else { c.3 });
        }
        let n = stars.len();
        let mut view = View::default();
        (view.width, view.height, view.zoom) = (1100.0, 800.0, zoom);
        view.cx = 0.45;
        view.cy = 0.5;
        let palette = vec![
            [242, 120, 180],
            [120, 170, 255],
            [255, 190, 80],
            [120, 220, 170],
            [190, 140, 255],
            [255, 120, 90],
            [100, 210, 230],
            [220, 220, 110],
            [110, 110, 110],
        ];
        let (l, t, r, b) = gl::on_view(&view);
        let in_view = stars.iter().flatten().filter(|p| p.x >= l && p.x <= r && p.y >= t && p.y <= b).count();
        let few: usize = std::env::var("GALAXY_FEW").ok().and_then(|f| f.parse().ok()).unwrap_or(1).max(1);
        let shown: Vec<usize> = (0..n).step_by(few).collect();
        let crowd = crate::galaxy_grid::Grid::new(&stars, &shown).crowd(l, t, r, b);
        if few == 1 {
            assert_eq!(crowd.in_view, in_view);
        }
        let p = Picture {
            id: 1,
            stars: Arc::new(stars),
            count: n,
            colours: Arc::new(colours),
            shown: Arc::new(shown),
            light: Arc::default(),
            order: Arc::default(),
            palette,
            view,
            scale: 1.0,
            crowd,
            glows: true,
        };
        let t = std::time::Instant::now();
        let d = p.draw();
        eprintln!("drawn in {:?}", t.elapsed());
        let mut ppm = format!("P6 {} {} 255\n", d.width, d.height).into_bytes();
        for px in &d.pixels {
            // Over the page's dark background.
            let bg = 14.0 * (1.0 - px.a as f32 / 255.0);
            ppm.extend([px.r as f32 + bg, px.g as f32 + bg, px.b as f32 + bg].map(|v| v.min(255.0) as u8));
        }
        std::fs::write(out, ppm).unwrap();
    }
}
