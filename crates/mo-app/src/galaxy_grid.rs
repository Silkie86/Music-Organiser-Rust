//! A grid over the whole galaxy map (the C# app's): the shown stars sorted into squares once, whenever the map or the
//! filters change, so the star under the mouse and how many stars are on the view, and how crowded they are, are found
//! by looking only at the squares near it, whatever the zoom. It's made off the window's thread, and the drawing
//! doesn't need it.

use mo_sounds::StarPoint;

use crate::galaxy_view::View;

/// Squares across (and down) the map, as the C# app.
pub const SIDE: usize = 192;

/// The shown stars, by square.
#[derive(Clone, Debug, Default)]
pub struct Grid {
    /// Each star (place in the list, x, y on the map), sorted by square.
    stars: Vec<(u32, f32, f32)>,
    /// Where each square's stars start in `stars` (one more than there are squares).
    starts: Vec<u32>,
}

/// How crowded the shown stars on a view are: how many there are, and how far apart where they crowd. That decides how
/// the galaxy is drawn ([`crate::galaxy_look::look`]): only painted while the stars are too close to tell apart, as
/// stars once they come apart. It goes by where the stars are, not by the view's average: a crowded clump in an
/// otherwise empty view is still a crowd.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crowd {
    /// How many shown stars are on the view.
    pub in_view: usize,
    /// How far apart they are where they crowd, in map units: the gap round a star in its square of the grid, averaged
    /// over the stars on the view.
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
        Grid { stars: sorted, starts }
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

    /// How many stars are in the box from (`left`, `top`) to (`right`, `bottom`) on the map: the squares wholly inside
    /// are counted whole, the ones on its edges star by star.
    pub fn count_in(&self, left: f32, top: f32, right: f32, bottom: f32) -> usize {
        self.crowd(left, top, right, bottom).in_view
    }

    /// How crowded the stars in the box from (`left`, `top`) to (`right`, `bottom`) on the map are: how many are in it
    /// (counted as [`Grid::count_in`] counts them) and how far apart they are where they crowd. Each square's crowd
    /// is its stars over its area, all of it (so a square only just on the view isn't judged by a sliver), and the
    /// box's is the average over its stars: a square with more of them on the view counts for more.
    pub fn crowd(&self, left: f32, top: f32, right: f32, bottom: f32) -> Crowd {
        let none = Crowd { in_view: 0, apart: 1.0 };
        if self.stars.is_empty() || right < 0.0 || bottom < 0.0 || left > 1.0 || top > 1.0 {
            return none;
        }
        let (x0, x1) = (square(left), square(right));
        let (y0, y1) = (square(top), square(bottom));
        // The stars on the view, and the same each times how many share its square.
        let (mut n, mut crowded) = (0usize, 0u64);
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
                crowded += on_view as u64 * stars.len() as u64;
            }
        }
        if n == 0 {
            return none;
        }
        // A square's stars each have its area over their number round them; the gap is the square root of that.
        let square_area = 1.0 / (SIDE * SIDE) as f64;
        Crowd { in_view: n, apart: (square_area * n as f64 / crowded as f64).sqrt() as f32 }
    }
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
        // Spread over the whole map they'd be 0.02 apart; where they crowd they're far closer, within a square's
        // rounding of the clump's own gap (the clump spans a few squares of the grid, each about half full).
        assert!(whole.apart < 0.002, "{whole:?}");
        assert!(whole.apart > 2e-4, "{whole:?}");
        // Away from the clump, the stars are as far apart as they are.
        let sparse = grid.crowd(0.0, 0.0, 0.4, 0.4);
        assert_eq!(sparse.in_view, 16);
        assert!((sparse.apart - 1.0 / SIDE as f32).abs() < 1e-6, "one star to a square: {sparse:?}");
        // Off the map: none.
        assert_eq!(grid.crowd(2.0, 2.0, 3.0, 3.0).in_view, 0);
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
