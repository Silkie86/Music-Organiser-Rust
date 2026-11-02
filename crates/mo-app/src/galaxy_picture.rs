//! The Sounds page's galaxy picture, without a window: every star drawn as a soft dot on a picture the size of the
//! view, spread over every core, and where each one landed so the one under the mouse can be found. The page shows
//! the picture; this is kept here (an optimised crate even in the launcher's debug build) because it draws half a
//! million stars.
//!
//! To look like a galaxy rather than a scatter plot, the stars give off light: each one brightens what's under it
//! (a "screen" blend, so a crowd of them glows towards white, the way a galaxy's core does), and behind them a soft
//! cloud of each crowd's colour glows like a nebula, thicker where more sounds are alike.

use std::sync::Arc;

use mo_sounds::StarPoint;
use rayon::prelude::*;

use crate::galaxy_view::View;

/// The grid the drawn stars are sorted into, for finding the one under the mouse (in pixels).
const CELL: usize = 16;
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

/// One pixel, its colour premultiplied by its alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// A picture drawn: what it was asked as, its size in pixels, the pixels row by row, and where the stars went.
pub struct Drawn {
    pub id: u64,
    pub view: View,
    /// Pixels per point it was drawn at.
    pub scale: f32,
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<Rgba>,
    pub hits: Hits,
}

/// Where each shown star was drawn, by grid cell: (place in the list, x, y) in pixels, sorted by cell, and where each
/// cell's stars start in that list.
pub struct Hits {
    columns: usize,
    rows: usize,
    stars: Vec<(usize, f32, f32)>,
    starts: Vec<usize>,
}

impl Hits {
    /// Sorts the drawn stars (place in the list, x, y in pixels, all on the picture) into a grid of `columns` by
    /// `rows` cells.
    fn new(columns: usize, rows: usize, drawn: Vec<(usize, f32, f32)>) -> Hits {
        let cell_of = |&(_, x, y): &(usize, f32, f32)| {
            ((y as usize / CELL).min(rows - 1)) * columns + (x as usize / CELL).min(columns - 1)
        };
        // Counted per cell, then each star put straight into its cell's place.
        let mut starts = vec![0; columns * rows + 1];
        for s in &drawn {
            starts[cell_of(s) + 1] += 1;
        }
        for i in 1..starts.len() {
            starts[i] += starts[i - 1];
        }
        let mut next = starts.clone();
        let mut stars = vec![(0, 0.0, 0.0); drawn.len()];
        for s in drawn {
            let cell = cell_of(&s);
            stars[next[cell]] = s;
            next[cell] += 1;
        }
        Hits { columns, rows, stars, starts }
    }

    /// The shown star nearest (`x`, `y`) in pixels, within `near` pixels.
    pub fn nearest(&self, x: f32, y: f32, near: f32) -> Option<usize> {
        let (cx, cy) = ((x / CELL as f32).floor() as isize, (y / CELL as f32).floor() as isize);
        let reach = (near / CELL as f32).ceil() as isize;
        let mut best: Option<(f32, usize)> = None;
        for gy in cy - reach..=cy + reach {
            for gx in cx - reach..=cx + reach {
                if gx < 0 || gy < 0 || gx as usize >= self.columns || gy as usize >= self.rows {
                    continue;
                }
                let cell = gy as usize * self.columns + gx as usize;
                for &(at, sx, sy) in &self.stars[self.starts[cell]..self.starts[cell + 1]] {
                    let d = (sx - x).powi(2) + (sy - y).powi(2);
                    if d <= near * near && best.is_none_or(|(b, _)| d < b) {
                        best = Some((d, at));
                    }
                }
            }
        }
        best.map(|b| b.1)
    }
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
    /// Every sound type's colour (as `Theme.sound-types`), then grey for a sound with no type.
    pub palette: Vec<[u8; 3]>,
    pub view: View,
    pub scale: f32,
}

/// The fewest rows of the picture drawn together on one thread. A band is made taller when the stars are big (a high
/// scale factor), so a star never reaches past the band below the one its top is in.
const BAND: usize = 64;

/// A star to draw: where (in pixels), its size, colour and strength.
#[derive(Clone, Copy)]
struct Dot {
    x: f32,
    y: f32,
    r: f32,
    colour: [u8; 3],
    alpha: f32,
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
        let on_show = self.shown.len().max(1) as f32 / (self.view.zoom * self.view.zoom);
        let bright = (0.9 * (BRIGHT_CROWD / on_show).sqrt()).clamp(0.3, 0.9);
        let band_rows = BAND.max((2.0 * r + 4.0).ceil() as usize);
        let bands = h.div_ceil(band_rows).max(1);
        // The nebula's squares, lined up with the map so its light holds still as the map is dragged, and the stars
        // just past the picture's edges, whose light spreads onto it.
        let size = (CLOUD * self.scale).max(2.0);
        let (ox, oy) = self.view.to_view(0.0, 0.0, self.scale);
        let reach = 2 * CLOUD_BLUR;
        let grid = (ox.rem_euclid(size) - reach as f32 * size, oy.rem_euclid(size) - reach as f32 * size);
        let margin = r.max(-grid.0).max(-grid.1);
        // Every star on the picture with the band its top edge is in; sorted so each band's stars are together, the
        // ones the filters leave out (drawn faint) before the shown ones (drawn over them), and otherwise in list
        // order, so stars that overlap are always drawn the same way round.
        let mut placed: Vec<(usize, bool, usize, Dot)> = self
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
                let dot = Dot {
                    x,
                    y,
                    r: if shown { r } else { r * 0.8 },
                    colour,
                    alpha: if shown { bright } else { bright * 0.16 },
                };
                let band = ((y - dot.r - 1.0).max(0.0) as usize / band_rows).min(bands - 1);
                Some((band, shown, at, dot))
            })
            .collect();
        placed.par_sort_unstable_by_key(|p| (p.0, p.1, p.2));
        let cloud = Cloud::new(&placed, w, h, size, grid);
        // Where each band's stars start, and where its shown ones start.
        let first = |band: usize, shown: bool| placed.partition_point(|p| (p.0, p.1) < (band, shown));
        let ranges: Vec<(usize, usize)> = (0..=bands).map(|b| (first(b, false), first(b, true))).collect();
        // A star is never taller than a band, so it reaches at most into the band below the one its top is in: each band
        // draws its own stars and the band above's, faint ones first.
        pixels.par_chunks_mut((w * band_rows).max(1)).enumerate().for_each(|(band, rows)| {
            let y0 = band * band_rows;
            cloud.light(rows, w, y0);
            let mine = ranges[band];
            let next = ranges[band + 1].0;
            let above = band.checked_sub(1).map(|b| (ranges[b], ranges[b + 1].0));
            let mut draw = |from: usize, to: usize| {
                for p in &placed[from..to] {
                    disc(rows, w, y0, &p.3);
                }
            };
            if let Some((a, end)) = above {
                draw(a.0, a.1);
                draw(mine.0, mine.1);
                draw(a.1, end);
            } else {
                draw(mine.0, mine.1);
            }
            draw(mine.1, next);
        });
        let columns = w.div_ceil(CELL).max(1);
        let rows = h.div_ceil(CELL).max(1);
        let drawn: Vec<(usize, f32, f32)> = placed
            .par_iter()
            .filter(|p| p.1 && p.3.x >= 0.0 && p.3.y >= 0.0 && p.3.x < w as f32 && p.3.y < h as f32)
            .map(|p| (p.2, p.3.x, p.3.y))
            .collect();
        let hits = Hits::new(columns, rows, drawn);
        Drawn { id: self.id, view: self.view, scale: self.scale, width: w, height: h, pixels, hits }
    }
}

/// Draws a soft-edged dot over what's there (colours premultiplied by their alpha), on the rows in `rows`: a band of
/// the picture `w` pixels wide whose first row is row `y0` of the whole picture.
fn disc(rows: &mut [Rgba], w: usize, y0: usize, d: &Dot) {
    let h = rows.len() / w.max(1);
    let (x, y, r) = (d.x, d.y - y0 as f32, d.r);
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
        }
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
    /// Each square's colour (the stars' light, averaged) and how thick it is (0 to `CLOUD_LIGHT`).
    squares: Vec<([f32; 3], f32)>,
}

impl Cloud {
    fn new(placed: &[(usize, bool, usize, Dot)], w: usize, h: usize, size: f32, origin: (f32, f32)) -> Cloud {
        let columns = ((w as f32 - origin.0) / size).ceil() as usize + 2 * CLOUD_BLUR + 1;
        let rows = ((h as f32 - origin.1) / size).ceil() as usize + 2 * CLOUD_BLUR + 1;
        let cells = columns * rows;
        // Added up on every core: each core's share of the stars into a grid of its own, then the grids together.
        let share = placed.len().div_ceil(rayon::current_num_threads().max(1)).max(1);
        let add = |mut sums: Vec<[f32; 4]>, (_, shown, _, d): &(usize, bool, usize, Dot)| {
            let (fx, fy) = ((d.x - origin.0) / size, (d.y - origin.1) / size);
            if fx < 0.0 || fy < 0.0 || fx as usize >= columns || fy as usize >= rows {
                return sums;
            }
            // A star the filters leave out still glows a little, as it's drawn faint.
            let weight = if *shown { 1.0 } else { 0.15 };
            let s = &mut sums[fy as usize * columns + fx as usize];
            s[0] += d.colour[0] as f32 * weight;
            s[1] += d.colour[1] as f32 * weight;
            s[2] += d.colour[2] as f32 * weight;
            s[3] += weight;
            sums
        };
        let sums = placed.par_chunks(share).map(|stars| stars.iter().fold(vec![[0.0f32; 4]; cells], add)).reduce(
            || vec![[0.0f32; 4]; cells],
            |mut a, b| {
                for (a, b) in a.iter_mut().zip(&b) {
                    for j in 0..4 {
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
                if s[3] <= 1e-4 {
                    return ([0.0; 3], 0.0);
                }
                let colour = [s[0] / s[3], s[1] / s[3], s[2] / s[3]];
                (colour, CLOUD_LIGHT * (1.0 - (-s[3] / CLOUD_THICK).exp()))
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
                for ((c, t), k) in corners {
                    let k = k * t;
                    thick += k;
                    for j in 0..3 {
                        colour[j] += c[j] * k;
                    }
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
fn blur(sums: &[[f32; 4]], columns: usize, rows: usize, across: bool) -> Vec<[f32; 4]> {
    let r = CLOUD_BLUR as isize;
    let k = 1.0 / (2 * r + 1) as f32;
    let mut out = vec![[0.0f32; 4]; sums.len()];
    out.par_chunks_mut(columns.max(1)).enumerate().for_each(|(y, line)| {
        for (x, cell) in line.iter_mut().enumerate() {
            let mut acc = [0.0f32; 4];
            for d in -r..=r {
                let (sx, sy) = if across { (x as isize + d, y as isize) } else { (x as isize, y as isize + d) };
                if sx < 0 || sy < 0 || sx as usize >= columns || sy as usize >= rows {
                    continue;
                }
                let s = sums[sy as usize * columns + sx as usize];
                for j in 0..4 {
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
    fn the_star_under_the_mouse_is_the_nearest_within_reach() {
        let hits = Hits::new(4, 4, vec![(3, 20.0, 20.0), (1, 5.0, 5.0), (2, 18.0, 5.0)]);
        assert_eq!(hits.nearest(6.0, 5.0, 8.0), Some(1));
        assert_eq!(hits.nearest(15.0, 5.0, 8.0), Some(2));
        assert_eq!(hits.nearest(40.0, 40.0, 8.0), None);
    }

    #[test]
    fn a_disc_covers_its_middle_and_not_far_away() {
        let mut buf = vec![Rgba::default(); 100];
        let dot = Dot { x: 5.0, y: 5.0, r: 1.5, colour: [200, 100, 50], alpha: 1.0 };
        disc(&mut buf, 10, 0, &dot);
        assert_eq!(buf[5 * 10 + 5].a, 255);
        assert_eq!(buf[0].a, 0);
    }

    #[test]
    fn a_disc_lands_on_the_right_rows_of_a_band() {
        // The band holds rows 4 to 7 of the picture; the dot sits on row 5, the band's second row.
        let mut band = vec![Rgba::default(); 40];
        let dot = Dot { x: 5.0, y: 5.5, r: 1.0, colour: [200, 100, 50], alpha: 1.0 };
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
            palette: vec![[200, 100, 50], [110, 110, 110]],
            view,
            scale: 1.0,
        };
        let d = picture.draw();
        // Beside the crowd, between the stars' own discs, the nebula glows in their colour.
        let near = d.pixels[(100 + 8) * d.width + 104];
        assert!(near.r > 3 && near.r > near.b, "{near:?}");
        // A corner far from it is dark.
        assert_eq!(d.pixels[5 * d.width + 5], Rgba::default());
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
            palette: vec![[200, 100, 50], [110, 110, 110]],
            view,
            scale: 8.0,
        };
        let d = picture.draw();
        let (w, middle) = (d.width, d.width / 2);
        // The middle column is drawn all the way from a little above the middle to a little below.
        for y in middle - 30..middle + 30 {
            assert!(d.pixels[y * w + middle].a > 200, "row {y}");
        }
        assert_eq!(d.hits.nearest(middle as f32, middle as f32, 8.0), Some(0));
    }
}

#[cfg(test)]
mod look {
    use super::*;

    /// Draws 450,000 made-up stars in clumps to a PPM file named by GALAXY_LOOK, to judge the look by eye.
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
        let p = Picture {
            id: 1,
            stars: Arc::new(stars),
            count: n,
            colours: Arc::new(colours),
            shown: Arc::new((0..n).collect()),
            palette,
            view,
            scale: 1.0,
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
