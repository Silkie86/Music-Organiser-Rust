//! The Sounds page's galaxy picture, without a window: every star drawn as a soft dot on a picture the size of the
//! view, spread over every core, and where each one landed so the one under the mouse can be found. The page shows
//! the picture; this is kept here (an optimised crate even in the launcher's debug build) because it draws half a
//! million stars.

use std::sync::Arc;

use mo_sounds::StarPoint;
use rayon::prelude::*;

use crate::galaxy_view::View;

/// The grid the drawn stars are sorted into, for finding the one under the mouse (in pixels).
const CELL: usize = 16;

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
        let mut keyed: Vec<(usize, (usize, f32, f32))> = drawn.into_par_iter().map(|s| (cell_of(&s), s)).collect();
        keyed.par_sort_unstable_by_key(|k| k.0);
        let mut starts = vec![0; columns * rows + 1];
        for &(cell, _) in &keyed {
            starts[cell + 1] += 1;
        }
        for i in 1..starts.len() {
            starts[i] += starts[i - 1];
        }
        Hits { columns, rows, stars: keyed.into_iter().map(|k| k.1).collect(), starts }
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
    /// Each star's place in `palette`.
    pub colours: Arc<Vec<i32>>,
    pub shown: Arc<Vec<usize>>,
    /// Every sound type's colour (as `Theme.sound-types`), then grey for a sound with no type.
    pub palette: Vec<[u8; 3]>,
    pub view: View,
    pub scale: f32,
}

/// The rows of the picture drawn together on one thread. Taller than the biggest star (6 points across at a scale of up
/// to 4, and its soft edge), so a star reaches at most into the band below the one its top is in.
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
        let n = self.stars.par_iter().filter(|s| s.is_some()).count().max(1) as f32;
        let base = (200.0 / n.sqrt()).clamp(1.2, 4.5);
        let r = self.scale * (base + (self.view.zoom - 1.0).max(0.0).sqrt() * 0.5).min(6.0);
        let grey = self.palette.last().copied().unwrap_or([128, 128, 128]);
        let bands = h.div_ceil(BAND).max(1);
        // Every star on the picture with the band its top edge is in; sorted so each band's stars are together, the
        // ones the filters leave out (drawn faint) before the shown ones (drawn over them).
        let mut placed: Vec<(usize, bool, usize, Dot)> = self
            .stars
            .par_iter()
            .enumerate()
            .filter_map(|(at, star)| {
                let p = (*star)?;
                let (x, y) = self.view.to_view(p.x, p.y, self.scale);
                if x < -r || y < -r || x > w as f32 + r || y > h as f32 + r {
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
                let dot =
                    Dot { x, y, r: if shown { r } else { r * 0.8 }, colour, alpha: if shown { 0.9 } else { 0.14 } };
                let band = ((y - dot.r - 1.0).max(0.0) as usize / BAND).min(bands - 1);
                Some((band, shown, at, dot))
            })
            .collect();
        placed.par_sort_unstable_by_key(|p| (p.0, p.1));
        // Where each band's stars start, and where its shown ones start.
        let first = |band: usize, shown: bool| placed.partition_point(|p| (p.0, p.1) < (band, shown));
        let ranges: Vec<[usize; 3]> = (0..=bands).map(|b| [first(b, false), first(b, true), 0]).collect();
        // A star is never taller than a band, so it reaches at most into the band below the one its top is in: each band
        // draws its own stars and the band above's, faint ones first.
        pixels.par_chunks_mut((w * BAND).max(1)).enumerate().for_each(|(band, rows)| {
            let y0 = band * BAND;
            let mine = ranges[band];
            let next = ranges[band + 1][0];
            let above = band.checked_sub(1).map(|b| (ranges[b], ranges[b + 1][0]));
            let mut draw = |from: usize, to: usize| {
                for p in &placed[from..to] {
                    disc(rows, w, y0, &p.3);
                }
            };
            if let Some((a, end)) = above {
                draw(a[0], a[1]);
                draw(mine[0], mine[1]);
                draw(a[1], end);
            } else {
                draw(mine[0], mine[1]);
            }
            draw(mine[1], next);
        });
        let columns = w.div_ceil(CELL).max(1);
        let rows = h.div_ceil(CELL).max(1);
        let drawn: Vec<(usize, f32, f32)> = placed
            .par_iter()
            .filter(|p| p.1 && p.3.x >= 0.0 && p.3.y >= 0.0 && p.3.x < w as f32 && p.3.y < h as f32)
            .map(|p| (p.2, p.3.x, p.3.y))
            .collect();
        Drawn { id: self.id, view: self.view, width: w, height: h, pixels, hits: Hits::new(columns, rows, drawn) }
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
            let o = &mut rows[py * w + px];
            let keep = 1.0 - a;
            o.r = (d.colour[0] as f32 * a + o.r as f32 * keep) as u8;
            o.g = (d.colour[1] as f32 * a + o.g as f32 * keep) as u8;
            o.b = (d.colour[2] as f32 * a + o.b as f32 * keep) as u8;
            o.a = (255.0 * a + o.a as f32 * keep) as u8;
        }
    }
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
}
