//! The names over the galaxy when it's zoomed out (the C# app's regions): the map is cut into squares, each square
//! takes the sound type most of its shown stars share, and touching squares with the same type become one region,
//! named just above its top. Only the biggest few are named, fading out as the view flies in, and none on top of
//! another. A click on a name flies into its region.

use std::collections::{HashMap, HashSet, VecDeque};

use mo_sounds::StarPoint;

use crate::galaxy_view::View;

/// The most regions named.
const MOST: usize = 16;
/// A star with no type, or "Unknown": it names no region.
pub const NO_KIND: u32 = u32::MAX;

/// A neighbourhood of the map where one sound type is most of the stars.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub name: String,
    /// Its middle across, and its top (map units, 0 to 1).
    pub x: f32,
    pub y: f32,
    pub count: usize,
    /// Its colour, as a place in `Theme.sound-types` (-1 grey).
    pub colour: i32,
    /// The box its stars are in: left, top, right and bottom.
    pub area: (f32, f32, f32, f32),
}

/// Finds the regions of the shown stars, biggest first. `kinds` says each star's type as a place in `names`
/// ([`NO_KIND`] for none) and `colours` each type's colour, both by the star's place in the list; `shown` is the list
/// places the filters let through.
pub fn find(
    stars: &[Option<StarPoint>],
    kinds: &[u32],
    names: &[String],
    colours: &[i32],
    shown: &[usize],
) -> Vec<Region> {
    let on_map: Vec<(StarPoint, u32)> = shown
        .iter()
        .filter_map(|&at| Some(((*stars.get(at)?)?, *kinds.get(at)?)))
        .filter(|&(_, k)| k != NO_KIND && (k as usize) < names.len())
        .collect();
    let total = on_map.len();
    if total == 0 {
        return Vec::new();
    }
    // Finer squares and bigger minimums for a big library, so its families still get their own names (as C#).
    let grid = ((total as f32).sqrt() / 20.0).clamp(10.0, 24.0) as usize;
    let least = (total / 3000).max(5);
    let cell_of = |p: StarPoint| {
        let c = |v: f32| ((v.clamp(0.0, 1.0) * grid as f32) as usize).min(grid - 1);
        (c(p.x), c(p.y))
    };
    // Each square's stars by type.
    let mut counts: HashMap<(usize, usize), HashMap<u32, usize>> = HashMap::new();
    for &(p, k) in &on_map {
        *counts.entry(cell_of(p)).or_default().entry(k).or_default() += 1;
    }
    // The type each square takes: at least three stars, and the top type half of them or more.
    let cells: HashMap<(usize, usize), u32> = counts
        .into_iter()
        .filter_map(|(cell, by_kind)| {
            let all: usize = by_kind.values().sum();
            // Ties go to the lower type number, so the answer is the same every time.
            let (&kind, &top) = by_kind.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))?;
            (all >= 3 && top * 2 >= all).then_some((cell, kind))
        })
        .collect();
    // Touching squares with the same type, joined.
    let mut region_of: HashMap<(usize, usize), usize> = HashMap::new();
    let mut joined: Vec<u32> = Vec::new();
    let mut starts: Vec<(usize, usize)> = cells.keys().copied().collect();
    starts.sort_by_key(|&(x, y)| (y, x));
    let mut seen = HashSet::new();
    for start in starts {
        if !seen.insert(start) {
            continue;
        }
        let kind = cells[&start];
        let id = joined.len();
        joined.push(kind);
        let mut queue = VecDeque::from([start]);
        while let Some((x, y)) = queue.pop_front() {
            region_of.insert((x, y), id);
            let next = [(x + 1, y), (x.wrapping_sub(1), y), (x, y + 1), (x, y.wrapping_sub(1))];
            for n in next {
                if cells.get(&n) == Some(&kind) && seen.insert(n) {
                    queue.push_back(n);
                }
            }
        }
    }
    // Each region's own stars (of its type, in its squares).
    struct Sum {
        count: usize,
        sum_x: f64,
        area: (f32, f32, f32, f32),
    }
    let mut sums: Vec<Sum> =
        joined.iter().map(|_| Sum { count: 0, sum_x: 0.0, area: (f32::MAX, f32::MAX, f32::MIN, f32::MIN) }).collect();
    for &(p, k) in &on_map {
        let Some(&id) = region_of.get(&cell_of(p)) else { continue };
        if joined[id] != k {
            continue;
        }
        let s = &mut sums[id];
        s.count += 1;
        s.sum_x += f64::from(p.x);
        s.area = (s.area.0.min(p.x), s.area.1.min(p.y), s.area.2.max(p.x), s.area.3.max(p.y));
    }
    let mut regions: Vec<Region> = sums
        .iter()
        .zip(&joined)
        .filter(|(s, _)| s.count >= least)
        .map(|(s, &k)| Region {
            name: names[k as usize].clone(),
            x: (s.sum_x / s.count as f64) as f32,
            y: s.area.1,
            count: s.count,
            colour: colours.get(k as usize).copied().unwrap_or(-1),
            area: s.area,
        })
        .collect();
    regions.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    regions.truncate(MOST);
    regions
}

/// A region's name placed on the view.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    /// Which region (its place in the list [`find`] gave).
    pub region: usize,
    pub name: String,
    /// The middle of the name's bottom edge, in points.
    pub x: f32,
    pub y: f32,
    pub colour: i32,
    /// How strongly it shows (0 to 1).
    pub fade: f32,
}

/// How strongly the names show at `zoom`: all the way out they're full, gone by 3.2 times in.
pub fn fade_at(zoom: f32) -> f32 {
    let t = ((zoom - 1.8) / (3.2 - 1.8)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// Places the regions' names on `view`, biggest first: each just above its region, moved a line up or down when
/// another name is in the way, or left out. `size` guesses a name's width and height in points.
pub fn place(regions: &[Region], view: &View, size: impl Fn(&str) -> (f32, f32)) -> Vec<Label> {
    let fade = fade_at(view.zoom);
    if fade <= 0.02 {
        return Vec::new();
    }
    // Only as many names as the galaxy's size has room for, so they name the clouds rather than cover them.
    let room = (view.unit() * 0.86 / 55.0).clamp(4.0, MOST as f32) as usize;
    let mut taken: Vec<(f32, f32, f32, f32)> = Vec::new();
    let mut labels = Vec::new();
    for (at, r) in regions.iter().enumerate().take(room) {
        let (w, h) = size(&r.name);
        let (x, y) = view.to_view(r.x, r.y, 1.0);
        let bottom = y - 8.0;
        // A line is the name's height and the room kept round it.
        let line = h + 8.0;
        let free = [0.0, -line, line, -2.0 * line].into_iter().map(|dy| bottom + dy).find(|&b| {
            let (l, t, rt, bt) = (x - w / 2.0 - 12.0, b - h - 3.0, x + w / 2.0 + 12.0, b + 3.0);
            !taken.iter().any(|o| l < o.2 && o.0 < rt && t < o.3 && o.1 < bt)
        });
        let Some(b) = free else { continue };
        if x + w / 2.0 < 0.0 || b < 0.0 || x - w / 2.0 > view.width || b - h > view.height {
            continue;
        }
        taken.push((x - w / 2.0, b - h, x + w / 2.0, b));
        labels.push(Label { region: at, name: r.name.clone(), x, y: b, colour: r.colour, fade });
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn star(x: f32, y: f32) -> Option<StarPoint> {
        Some(StarPoint { x, y })
    }

    /// Kicks in the top left, snares in the bottom right, a few stray hats among the kicks.
    fn map() -> (Vec<Option<StarPoint>>, Vec<u32>) {
        let mut stars = Vec::new();
        let mut kinds = Vec::new();
        for i in 0..60 {
            let d = (i % 10) as f32 * 0.01;
            stars.push(star(0.1 + d, 0.1 + (i / 10) as f32 * 0.01));
            kinds.push(if i % 7 == 0 { 2 } else { 0 });
            stars.push(star(0.8 + d, 0.8 + (i / 10) as f32 * 0.01));
            kinds.push(1);
        }
        stars.push(None);
        kinds.push(0);
        stars.push(star(0.5, 0.5));
        kinds.push(NO_KIND);
        (stars, kinds)
    }

    fn names() -> Vec<String> {
        ["Kick", "Snare", "Hi-hat"].map(String::from).to_vec()
    }

    #[test]
    fn each_neighbourhood_is_named_by_most_of_its_stars() {
        let (stars, kinds) = map();
        let shown: Vec<usize> = (0..stars.len()).collect();
        let regions = find(&stars, &kinds, &names(), &[4, 7, 9], &shown);
        assert_eq!(regions.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Snare", "Kick"]);
        let kick = &regions[1];
        assert_eq!((kick.count, kick.colour), (51, 4));
        assert!((kick.y - 0.1).abs() < 1e-6 && kick.x > 0.1 && kick.x < 0.2);
        assert!(kick.area.0 >= 0.1 && kick.area.2 <= 0.2);
    }

    #[test]
    fn the_filters_change_the_names() {
        let (stars, kinds) = map();
        // Only the snares let through.
        let shown: Vec<usize> = (0..stars.len()).filter(|i| kinds[*i] == 1).collect();
        let regions = find(&stars, &kinds, &names(), &[4, 7, 9], &shown);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].name, "Snare");
        assert!(find(&stars, &kinds, &names(), &[], &[]).is_empty());
    }

    #[test]
    fn names_fade_as_the_view_flies_in() {
        assert_eq!(fade_at(1.0), 1.0);
        assert!(fade_at(2.5) > 0.0 && fade_at(2.5) < 1.0);
        assert_eq!(fade_at(3.2), 0.0);
        let regions =
            vec![Region { name: "Kick".into(), x: 0.5, y: 0.5, count: 10, colour: 0, area: (0.4, 0.5, 0.6, 0.6) }];
        let mut v = View::default();
        (v.width, v.height) = (400.0, 400.0);
        assert_eq!(place(&regions, &v, |_| (40.0, 14.0)).len(), 1);
        v.zoom = 4.0;
        assert!(place(&regions, &v, |_| (40.0, 14.0)).is_empty());
    }

    #[test]
    fn names_in_the_way_move_a_line_or_are_left_out() {
        let at =
            |name: &str, x: f32| Region { name: name.into(), x, y: 0.5, count: 10, colour: 0, area: (x, 0.5, x, 0.6) };
        // Five names on the same spot: four places to try, so one is left out.
        let regions: Vec<Region> = ["A", "B", "C", "D", "E"].iter().map(|n| at(n, 0.5)).collect();
        let mut v = View::default();
        (v.width, v.height) = (800.0, 800.0);
        let labels = place(&regions, &v, |_| (40.0, 14.0));
        assert_eq!(labels.len(), 4);
        let mut ys: Vec<i32> = labels.iter().map(|l| l.y as i32).collect();
        ys.dedup();
        assert_eq!(ys.len(), 4, "each on its own line: {ys:?}");
        // A name off the view isn't placed.
        assert!(place(&[at("Far", 3.0)], &v, |_| (40.0, 14.0)).is_empty());
    }
}
