//! A grid over the whole galaxy map (the C# app's): the shown stars sorted into squares once, whenever the map or the
//! filters change, so the star under the mouse and how many stars are on the view, and how crowded they are, are found
//! by looking only at the squares near it, whatever the zoom. It's made off the window's thread, and the drawing
//! doesn't need it.

use mo_sounds::StarPoint;

use crate::galaxy_view::View;

/// Squares across (and down) the map, as the C# app.
pub const SIDE: usize = 192;
/// How many stars a star's gap is judged by, at the least (see [`Crowd`]).
const ROUND: u32 = 4;

/// The shown stars, by square.
#[derive(Clone, Debug, Default)]
pub struct Grid {
    /// Each star (place in the list, x, y on the map), sorted by square.
    stars: Vec<(u32, f32, f32)>,
    /// Where each square's stars start in `stars` (one more than there are squares).
    starts: Vec<u32>,
    /// How far apart each square's stars are, in doublings of a map unit (log2 of the gap): see [`Crowd`].
    gaps: Vec<f32>,
}

/// How crowded the shown stars on a view are: how many there are, and how far apart they are, judged round each star.
/// That decides how the galaxy is drawn ([`crate::galaxy_look::look`]): only painted while the stars are too close to
/// tell apart, as stars once they come apart.
///
/// Each star's gap is judged by the stars round it: its square's, if it holds at least four, or else the squares round
/// it too, out and out until there are four (3 by 3 squares, then 5 by 5, 9 by 9...), so a few stars far apart are
/// judged by the room between them, not by one square's size. The view's is the stars' gaps averaged as doublings, so
/// it goes by where the stars are, not by the view's own size: a crowded clump in an otherwise empty view is still a
/// crowd. And a few stars from a crowd at the view's edge (or a few far apart) only sway it as much as their share of
/// the view's stars, so the look never flips as the view moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crowd {
    /// How many shown stars are on the view.
    pub in_view: usize,
    /// How far apart they are, in map units: each star's gap to the stars round it, averaged as doublings.
    pub apart: f32,
}

impl Crowd {
    /// `in_view` stars spread evenly over `view` (for when there's no grid yet, and for the tests).
    pub fn even(view: &View, in_view: usize) -> Crowd {
        let unit = view.unit().max(f32::MIN_POSITIVE);
        let area = view.width.max(1.0) * view.height.max(1.0) / (unit * unit);
        Crowd { in_view, apart: (area / in_view.max(1) as f32).sqrt() }
    }
}

/// The square a map point is in, across or down.
fn square(v: f32) -> usize {
    ((v.clamp(0.0, 1.0) * SIDE as f32) as usize).min(SIDE - 1)
}

impl Grid {
    /// Sorts the stars `shown` lets through (places in the list) into the grid; a star not on the map is left out.
    pub fn new(stars: &[Option<StarPoint>], shown: &[usize]) -> Grid {
        let on_map: Vec<(u32, f32, f32)> = shown
            .iter()
            .filter_map(|&at| {
                let p = (*stars.get(at)?)?;
                Some((u32::try_from(at).ok()?, p.x, p.y))
            })
            .collect();
        let cell = |&(_, x, y): &(u32, f32, f32)| square(y) * SIDE + square(x);
        // Counted per square, then each star put straight into its square's place.
        let mut starts = vec![0u32; SIDE * SIDE + 1];
        for s in &on_map {
            starts[cell(s) + 1] += 1;
        }
        for i in 1..starts.len() {
            starts[i] += starts[i - 1];
        }
        let mut next = starts.clone();
        let mut sorted = vec![(0, 0.0, 0.0); on_map.len()];
        for s in on_map {
            let c = cell(&s);
            sorted[next[c] as usize] = s;
            next[c] += 1;
        }
        let gaps = gaps(&starts);
        Grid { stars: sorted, starts, gaps }
    }

    /// How many stars it holds.
    pub fn len(&self) -> usize {
        self.stars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stars.is_empty()
    }

    /// The stars in square (`x`, `y`).
    fn in_square(&self, x: usize, y: usize) -> &[(u32, f32, f32)] {
        let c = y * SIDE + x;
        match (self.starts.get(c), self.starts.get(c + 1)) {
            (Some(&a), Some(&b)) => &self.stars[a as usize..b as usize],
            _ => &[],
        }
    }

    /// The star nearest map point (`x`, `y`), no further than `reach` (in map units).
    pub fn nearest(&self, x: f32, y: f32, reach: f32) -> Option<usize> {
        if self.stars.is_empty() || reach <= 0.0 {
            return None;
        }
        let (x0, x1) = (square(x - reach), square(x + reach));
        let (y0, y1) = (square(y - reach), square(y + reach));
        let mut best: Option<(f32, u32)> = None;
        for gy in y0..=y1 {
            for gx in x0..=x1 {
                for &(at, sx, sy) in self.in_square(gx, gy) {
                    let d = (sx - x).powi(2) + (sy - y).powi(2);
                    // Ties go to the earlier star, so the answer is the same every time.
                    if d <= reach * reach && best.is_none_or(|(b, was)| d < b || (d == b && at < was)) {
                        best = Some((d, at));
                    }
                }
            }
        }
        best.map(|b| b.1 as usize)
    }

    /// The star an arrow key goes to from map point (`x`, `y`): the nearest one that way (`dx`, `dy`: the arrow's
    /// direction, such as (1, 0) for Right; y grows down the map). "That way" is within 45 degrees of the arrow, and at
    /// least `least` (in map units) along it, so each press visibly moves on and never lands on the star it's on, even
    /// in a crowd far out. Of those, the nearest wins, a star off to the side counting as further: its distance plus
    /// how far it is off the arrow's line. `None` when there's no star that way.
    pub fn toward(&self, x: f32, y: f32, (dx, dy): (f32, f32), least: f32) -> Option<usize> {
        let len = (dx * dx + dy * dy).sqrt();
        if self.stars.is_empty() || !len.is_finite() || len == 0.0 {
            return None;
        }
        let (ux, uy) = (dx / len, dy / len);
        let least = least.max(0.0);
        let score = |sx: f32, sy: f32| {
            let (ox, oy) = (sx - x, sy - y);
            let along = ox * ux + oy * uy;
            let aside = (ox * uy - oy * ux).abs();
            (along >= least && along > 0.0 && aside <= along).then(|| (ox * ox + oy * oy).sqrt() + aside)
        };
        // Squares out to `reach`, doubling until a star is found and nothing further out could beat it: a star outside
        // the box is more than `reach` away, so it scores more than `reach`.
        let mut reach = (least * 2.0).max(1.0 / SIDE as f32);
        loop {
            let (x0, x1) = (square(x - reach), square(x + reach));
            let (y0, y1) = (square(y - reach), square(y + reach));
            let mut best: Option<(f32, u32)> = None;
            for gy in y0..=y1 {
                for gx in x0..=x1 {
                    for &(at, sx, sy) in self.in_square(gx, gy) {
                        let Some(s) = score(sx, sy) else { continue };
                        // Ties go to the earlier star, so the answer is the same every time.
                        if best.is_none_or(|(b, was)| s < b || (s == b && at < was)) {
                            best = Some((s, at));
                        }
                    }
                }
            }
            match best {
                Some((s, at)) if s <= reach => return Some(at as usize),
                // The whole map was looked through: the best there is, or none.
                _ if reach >= 2.0 => return best.map(|b| b.1 as usize),
                _ => reach *= 2.0,
            }
        }
    }

    /// How many stars are in the box from (`left`, `top`) to (`right`, `bottom`) on the map: the squares wholly inside
    /// are counted whole, the ones on its edges star by star.
    pub fn count_in(&self, left: f32, top: f32, right: f32, bottom: f32) -> usize {
        self.crowd(left, top, right, bottom).in_view
    }

    /// How crowded the stars in the box from (`left`, `top`) to (`right`, `bottom`) on the map are: how many are in it
    /// (counted as [`Grid::count_in`] counts them) and how far apart they are ([`Crowd`]).
    pub fn crowd(&self, left: f32, top: f32, right: f32, bottom: f32) -> Crowd {
        let none = Crowd { in_view: 0, apart: 1.0 };
        if self.stars.is_empty() || right < 0.0 || bottom < 0.0 || left > 1.0 || top > 1.0 {
            return none;
        }
        let (x0, x1) = (square(left), square(right));
        let (y0, y1) = (square(top), square(bottom));
        // The stars on the view, and their gaps (as doublings) added up.
        let (mut n, mut doublings) = (0usize, 0.0f64);
        for gy in y0..=y1 {
            for gx in x0..=x1 {
                let stars = self.in_square(gx, gy);
                let inside = gx > x0 && gx < x1 && gy > y0 && gy < y1;
                let on_view = if inside {
                    stars.len()
                } else {
                    stars.iter().filter(|&&(_, x, y)| x >= left && x <= right && y >= top && y <= bottom).count()
                };
                n += on_view;
                doublings += on_view as f64 * f64::from(self.gaps.get(gy * SIDE + gx).copied().unwrap_or(0.0));
            }
        }
        if n == 0 {
            return none;
        }
        Crowd { in_view: n, apart: (doublings / n as f64).exp2() as f32 }
    }
}

/// How far apart each square's stars are (see [`Crowd`]), in doublings of a map unit, from where each square's stars
/// start (`starts`, as [`Grid`] keeps them). 0 for an empty square, which is never used.
fn gaps(starts: &[u32]) -> Vec<f32> {
    let count = |c: usize| starts.get(c + 1).zip(starts.get(c)).map_or(0, |(b, a)| b - a);
    // How many stars are above and left of each corner of the squares, so a block of squares is counted at once.
    let n = SIDE + 1;
    let mut corner = vec![0u32; n * n];
    for y in 0..SIDE {
        let mut row = 0;
        for x in 0..SIDE {
            row += count(y * SIDE + x);
            corner[(y + 1) * n + x + 1] = corner[y * n + x + 1] + row;
        }
    }
    let block = |x0: usize, y0: usize, x1: usize, y1: usize| {
        corner[y1 * n + x1] + corner[y0 * n + x0] - corner[y0 * n + x1] - corner[y1 * n + x0]
    };
    let square_area = 1.0 / (SIDE * SIDE) as f32;
    (0..SIDE * SIDE)
        .map(|c| {
            if count(c) == 0 {
                return 0.0;
            }
            let (x, y) = (c % SIDE, c / SIDE);
            // The square itself, then the squares round it, out and out, until there are enough stars to judge by (or
            // it's the whole map).
            let mut reach = 0;
            loop {
                let (x0, y0) = (x.saturating_sub(reach), y.saturating_sub(reach));
                let (x1, y1) = ((x + reach + 1).min(SIDE), (y + reach + 1).min(SIDE));
                let stars = block(x0, y0, x1, y1);
                if stars >= ROUND || (x0, y0, x1, y1) == (0, 0, SIDE, SIDE) {
                    let area = ((x1 - x0) * (y1 - y0)) as f32 * square_area;
                    // The gap is the square root of each star's share of the area: half its doublings.
                    return 0.5 * (area / stars.max(1) as f32).log2();
                }
                reach = (reach * 2).max(1);
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn star(x: f32, y: f32) -> Option<StarPoint> {
        Some(StarPoint { x, y })
    }

    #[test]
    fn the_nearest_shown_star_within_reach_is_found() {
        let stars = vec![star(0.5, 0.5), star(0.52, 0.5), None, star(0.9, 0.9), star(0.501, 0.5)];
        // The fifth is the nearest but the filters leave it out.
        let grid = Grid::new(&stars, &[0, 1, 2, 3]);
        assert_eq!(grid.len(), 3);
        assert_eq!(grid.nearest(0.502, 0.5, 0.01), Some(0));
        assert_eq!(grid.nearest(0.515, 0.5, 0.01), Some(1));
        assert_eq!(grid.nearest(0.7, 0.7, 0.01), None);
        // Across squares: the star is in the next square over.
        assert_eq!(grid.nearest(0.9 - 0.006, 0.9, 0.01), Some(3));
        let all = Grid::new(&stars, &[0, 1, 2, 3, 4]);
        assert_eq!(all.nearest(0.5012, 0.5, 0.01), Some(4));
    }

    #[test]
    fn an_arrow_goes_to_the_nearest_star_that_way() {
        let stars = vec![
            star(0.5, 0.5),   // 0: where it starts
            star(0.6, 0.5),   // 1: right, on the line
            star(0.55, 0.53), // 2: right, nearer but off the line
            star(0.4, 0.5),   // 3: left
            star(0.5, 0.2),   // 4: up, far
            star(0.5, 0.501), // 5: down, too close to count as a step
            star(0.9, 0.95),  // 6: down and right, the only other one down
            star(0.51, 0.7),  // 7: down
        ];
        let shown: Vec<usize> = (0..stars.len()).collect();
        let grid = Grid::new(&stars, &shown);
        // Star 2 is 0.058 away and 0.03 off the line (0.088); star 1 is 0.1 away on it.
        assert_eq!(grid.toward(0.5, 0.5, (1.0, 0.0), 0.005), Some(2));
        assert_eq!(grid.toward(0.5, 0.5, (-1.0, 0.0), 0.005), Some(3));
        // Up is far, and found though it's many squares away.
        assert_eq!(grid.toward(0.5, 0.5, (0.0, -1.0), 0.005), Some(4));
        // Down: star 5 is less than a step on, so star 7.
        assert_eq!(grid.toward(0.5, 0.5, (0.0, 1.0), 0.005), Some(7));
        assert_eq!(grid.toward(0.5, 0.5, (0.0, 1.0), 0.0), Some(5));
        // Nothing further right of the rightmost star but star 6, which is more down than right.
        assert_eq!(grid.toward(0.9, 0.95, (1.0, 0.0), 0.005), None);
        assert_eq!(grid.toward(0.5, 0.5, (0.0, 0.0), 0.005), None);
        // Only the shown stars count.
        let some = Grid::new(&stars, &[0, 1, 3]);
        assert_eq!(some.toward(0.5, 0.5, (1.0, 0.0), 0.005), Some(1));
    }

    #[test]
    fn stars_in_a_box_are_counted_exactly() {
        let stars: Vec<Option<StarPoint>> =
            (0..100).flat_map(|i| (0..100).map(move |j| star(i as f32 / 100.0, j as f32 / 100.0))).collect();
        let shown: Vec<usize> = (0..stars.len()).collect();
        let grid = Grid::new(&stars, &shown);
        assert_eq!(grid.count_in(0.0, 0.0, 1.0, 1.0), 10_000);
        // 0.1 to 0.295: x 0.10..0.29 is 20 columns, the same down.
        assert_eq!(grid.count_in(0.1, 0.1, 0.295, 0.295), 400);
        assert_eq!(grid.count_in(2.0, 2.0, 3.0, 3.0), 0);
        assert_eq!(grid.count_in(-1.0, -1.0, 0.005, 2.0), 100);
    }

    #[test]
    fn a_crowd_is_judged_where_the_stars_are_not_by_the_views_average() {
        // A clump of 2,500 stars in a square a fiftieth of the map across (a gap of 0.0004 between them), and 100
        // stars spread over the rest of the map.
        let mut stars: Vec<Option<StarPoint>> =
            (0..50).flat_map(|i| (0..50).map(move |j| star(0.5 + i as f32 * 4e-4, 0.5 + j as f32 * 4e-4))).collect();
        stars.extend((0..10).flat_map(|i| (0..10).map(move |j| star(0.05 + i as f32 * 0.1, 0.05 + j as f32 * 0.1))));
        let shown: Vec<usize> = (0..stars.len()).collect();
        let grid = Grid::new(&stars, &shown);
        let whole = grid.crowd(0.0, 0.0, 1.0, 1.0);
        assert_eq!(whole.in_view, 2_600);
        // Spread over the whole map they'd be 0.02 apart. But nearly all of them are in the clump, so the map's gap is
        // near the clump's own, swayed a little by the hundred far apart.
        assert!(whole.apart > 4e-4 && whole.apart < 7e-4, "{whole:?}");
        // Away from the clump, the stars are as far apart as they are (0.1), within the rounding of the squares they're
        // judged over, though each square holds only one.
        let sparse = grid.crowd(0.0, 0.0, 0.4, 0.4);
        assert_eq!(sparse.in_view, 16);
        assert!(sparse.apart > 0.07 && sparse.apart < 0.15, "{sparse:?}");
        // Off the map: none.
        assert_eq!(grid.crowd(2.0, 2.0, 3.0, 3.0).in_view, 0);
    }

    #[test]
    fn a_few_stars_far_apart_are_judged_by_the_room_between_them() {
        // 49 stars 0.13 apart, each alone in its square (a 192nd of the map across): judged by the room between them.
        let stars: Vec<Option<StarPoint>> =
            (0..7).flat_map(|i| (0..7).map(move |j| star(0.11 + i as f32 * 0.13, 0.11 + j as f32 * 0.13))).collect();
        let grid = Grid::new(&stars, &(0..stars.len()).collect::<Vec<_>>());
        let all = grid.crowd(0.0, 0.0, 1.0, 1.0);
        assert_eq!(all.in_view, 49);
        assert!(all.apart > 0.1 && all.apart < 0.2, "{all:?}");
        // Two stars alone on the map are as far apart as the map shared between them.
        let two = Grid::new(&[star(0.1, 0.1), star(0.9, 0.9)], &[0, 1]).crowd(0.0, 0.0, 1.0, 1.0);
        assert!((two.apart - 0.5f32.sqrt()).abs() < 1e-6, "{two:?}");
    }

    #[test]
    fn a_few_stars_of_a_crowd_at_the_views_edge_sway_it_only_by_their_share() {
        // 150 stars 0.016 apart (three squares) and, just past the view's top right corner, a crowd of 10,000 about a
        // square across (5e-5 apart).
        let mut stars: Vec<Option<StarPoint>> =
            (0..15).flat_map(|i| (0..10).map(move |j| star(0.3 + i as f32 * 0.016, 0.4 + j as f32 * 0.016))).collect();
        let (right, top) = (0.535, 0.39);
        stars.extend(
            (0..100).flat_map(|i| (0..100).map(move |j| star(right + i as f32 * 5e-5, top + (j as f32 - 84.5) * 5e-5))),
        );
        let grid = Grid::new(&stars, &(0..stars.len()).collect::<Vec<_>>());
        let alone = grid.crowd(0.29, top, right - 1e-4, 0.56);
        assert_eq!(alone.in_view, 150);
        assert!(alone.apart > 0.012 && alone.apart < 0.024, "{alone:?}");
        // Moved right a touch, so 15 of the crowd come onto the view's corner: a tenth of its stars, so they sway it by
        // about a tenth of their doublings, not all the way to their own gap.
        let sliver = grid.crowd(0.29, top, right + 2e-5, 0.56);
        assert_eq!(sliver.in_view, 165);
        assert!(sliver.apart > alone.apart * 0.5, "{alone:?} {sliver:?}");
    }

    #[test]
    fn stars_spread_evenly_over_a_view_are_as_far_apart_as_its_area_shared_out() {
        let mut v = View::default();
        (v.width, v.height) = (800.0, 600.0);
        let c = Crowd::even(&v, 10_000);
        // 600 points by 0.92 to the map unit: the view is about 1.45 by 1.09 map units.
        let area = (800.0 / (600.0 * 0.92)) * (600.0 / (600.0 * 0.92));
        assert!((c.apart - (area / 10_000.0f32).sqrt()).abs() < 1e-6, "{c:?}");
        assert_eq!(c.in_view, 10_000);
        // Zoomed in twice as far, the same stars on show would be half as far apart on the map.
        v.zoom = 2.0;
        assert!((Crowd::even(&v, 10_000).apart * 2.0 - c.apart).abs() < 1e-6);
    }
}
