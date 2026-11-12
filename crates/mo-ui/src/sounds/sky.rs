//! The galaxy drawn by the graphics card: the painter is `mo-sky`, shared with the CLAP plugin. Here is only its
//! look test, which needs a graphics card made the way Slint makes the window's.

pub(super) use mo_sky::{Shine, Sky};

#[cfg(test)]
mod tests {
    use super::*;
    use mo_app::galaxy_view::View;
    use slint::wgpu_30::wgpu;

    /// Draws 450,000 made-up stars in clumps on whatever graphics card there is (a software one will do), made the way
    /// Slint makes the window's, to PNGs in the folder GALAXY_GPU_LOOK names, at a few zooms (GALAXY_GPU_SCALE pixels
    /// per point, 1 if not set), lit by a measure every sound has (as Length), by one only a few have (as Liked) and by
    /// nothing, over a dark page and a light one (as Paper), to judge the look by eye and to check the card takes every
    /// step. With GALAXY_GPU_TIME set it also times each view: whole frames, and a twinkle's frames, which only draw the
    /// stars again (on a software card that's the processor's time, far slower than a real card, but fine for before
    /// and after).
    ///
    /// To look at one thing closely, these narrow it: GALAXY_GPU_ZOOMS (the zooms, as "1,6,12,60,400"),
    /// GALAXY_GPU_VIEW (the view's size in points, as "1100x800"), GALAXY_GPU_LIT ("length,liked,nothing") and
    /// GALAXY_GPU_PAGES ("dark,paper"). GALAXY_GPU_MAP=blob lays the stars out as a big library's map is, most of them
    /// in one crowded blob, with the view in its middle; GALAXY_GPU_AT ("0.05,0.05") puts the view's middle elsewhere.
    /// GALAXY_GPU_FEW=318 lets only one star in 318 through the filters (Kick with Favourites on a big library).
    /// Each picture's line in the output (`LOOK ...`) says how many stars were on it and how bright it is on average (0
    /// to 255 over the page), to see brightness change with zoom.
    #[test]
    #[ignore]
    fn render() {
        use crate::canvas::testing::{card, read_back};
        use mo_app::galaxy_look::{lit_order, lit_share, look, pack};
        use mo_sounds::StarPoint;
        let Ok(out) = std::env::var("GALAXY_GPU_LOOK") else { return };
        let scale: f32 = std::env::var("GALAXY_GPU_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0);
        let timed = std::env::var("GALAXY_GPU_TIME").is_ok();
        let listed = |name: &str, all: &str| -> Vec<String> {
            std::env::var(name).unwrap_or_else(|_| all.to_owned()).split(',').map(|s| s.trim().to_owned()).collect()
        };
        let zooms: Vec<f32> =
            listed("GALAXY_GPU_ZOOMS", "1,6,12,60,400").iter().filter_map(|z| z.parse().ok()).collect();
        let size: Vec<f32> =
            listed("GALAXY_GPU_VIEW", "1100x800")[0].split('x').filter_map(|v| v.parse().ok()).collect();
        let (view_width, view_height) =
            (size.first().copied().unwrap_or(1100.0), size.get(1).copied().unwrap_or(800.0));
        let lit_kinds = listed("GALAXY_GPU_LIT", "length,liked,nothing");
        let page_kinds = listed("GALAXY_GPU_PAGES", "dark,paper");
        let blob = std::env::var("GALAXY_GPU_MAP").is_ok_and(|m| m == "blob");
        let at: Vec<f32> = listed("GALAXY_GPU_AT", "").iter().filter_map(|v| v.parse().ok()).collect();
        let (device, queue) = card().expect("a graphics card, a software one at least");
        let mut seed = 12345u64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 1_000_000) as f32 / 1_000_000.0
        };
        let mut clumps: Vec<(f32, f32, f32, i32)> =
            (0..40).map(|i| (0.1 + rnd() * 0.8, 0.1 + rnd() * 0.8, 0.01 + rnd() * 0.08, i % 8)).collect();
        if blob {
            // Most of the stars in one blob in the middle (a sixth of the map across), crowded clumps of sound-alikes
            // inside it, the rest scattered round it. Its middle is about as crowded as Solomon's library is: about
            // 13,000 stars on a 4K view (1830 by 1171 points) at 44 times.
            clumps = (0..40)
                .map(|i| {
                    let (a, r) = (rnd() * std::f32::consts::TAU, rnd().sqrt() * if i < 30 { 0.08 } else { 0.3 });
                    (0.5 + a.cos() * r, 0.5 + a.sin() * r, if i < 30 { 0.02 + rnd() * 0.04 } else { 0.05 }, i % 8)
                })
                .collect();
        }
        let (mut stars, mut colours) = (Vec::new(), Vec::new());
        for i in 0..450_000 {
            let c = clumps[i % clumps.len()];
            let (a, r) = (rnd() * std::f32::consts::TAU, (-(rnd().max(1e-6)).ln()).sqrt() * c.2);
            stars.push(Some(StarPoint {
                x: (c.0 + a.cos() * r).clamp(0.0, 1.0),
                y: (c.1 + a.sin() * r).clamp(0.0, 1.0),
            }));
            colours.push(if rnd() < 0.2 { (rnd() * 8.0) as i32 } else { c.3 });
        }
        let palette = [
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
        // Every third star left out by the filters, as a filter would; with GALAXY_GPU_FEW=n, all but one in n (as
        // Kick with Favourites lets 1,413 of 450,000 through: n of 318).
        let few: Option<usize> = std::env::var("GALAXY_GPU_FEW").ok().and_then(|n| n.parse().ok()).filter(|&n| n > 0);
        let shown: Vec<usize> = (0..stars.len()).filter(|i| few.map_or(i % 3 != 0, |n| i % n == 0)).collect();
        let grid = mo_app::galaxy_grid::Grid::new(&stars, &shown);
        let mut sky = Sky::new(&device, &queue);
        // Lit as Length lights them: every sound has a length, so each has a rank from 0 to 1. As Liked: about one
        // in two hundred, ranked among themselves, the rest without the measure.
        let length: Vec<f32> = (0..stars.len()).map(|_| rnd().max(f32::MIN_POSITIVE)).collect();
        let liked: Vec<f32> = (0..stars.len()).map(|_| if rnd() < 0.005 { rnd().max(0.01) } else { 0.0 }).collect();
        // The page's well behind the map: Graphite's and Paper's.
        let pages = [("dark", [18.0 / 255.0; 3]), ("paper", [241.0 / 255.0, 239.0 / 255.0, 235.0 / 255.0])];
        let pages: Vec<(&str, [f32; 3])> =
            pages.into_iter().filter(|(p, _)| page_kinds.iter().any(|k| k == p)).collect();
        for (lit_by, light) in [("length", length), ("liked", liked), ("nothing", Vec::new())] {
            if !lit_kinds.iter().any(|k| k == lit_by) {
                continue;
            }
            let order = lit_order(&stars, &shown, &light);
            let share = lit_share(&stars, &shown, &light);
            let shine = Shine { light_on: share > 0.0, ..Shine::default() };
            let (packed, lit) = pack(&stars, &colours, &palette, &shown, &light, &order);
            sky.set_stars(&packed, lit);
            for &zoom in &zooms {
                let mut view = View::default();
                (view.width, view.height, view.zoom) = (view_width, view_height, zoom);
                (view.cx, view.cy) = match at[..] {
                    [x, y] => (x, y),
                    _ if blob => (0.5, 0.5),
                    _ => (clumps[3].0, clumps[3].1),
                };
                let (l, t, r, b) = mo_app::galaxy_look::on_view(&view);
                let crowd = grid.crowd(l, t, r, b);
                let in_view = crowd.in_view;
                let lk = look(&view, crowd, share);
                eprintln!("{lit_by}, zoom {zoom}: {lk:?}");
                if timed {
                    // Each drawn 5 times and waited for, taking turns with a twinkle's frames, so what else the
                    // computer is doing hits both alike.
                    let mut time = |whole: bool, t: f32| {
                        if whole {
                            sky.repaint();
                        }
                        let started = std::time::Instant::now();
                        let twinkle = Shine { time: t, twinkle: 0.07, ..shine };
                        let _ = sky.draw(&view, scale, &lk, twinkle, pages.first().map_or([0.07; 3], |p| p.1));
                        device.poll(wgpu::PollType::wait_indefinitely()).expect("the card finishes");
                        started.elapsed().as_secs_f64() * 1000.0
                    };
                    time(true, 0.0);
                    let (mut whole, mut twinkle) = (0.0, 0.0);
                    for i in 0..5 {
                        whole += time(true, i as f32);
                        twinkle += time(false, i as f32 + 0.5);
                    }
                    eprintln!(
                        "TIMING lit by {lit_by}, zoom {zoom}: {:.1} ms a whole frame, {:.1} ms a twinkle's frame",
                        whole / 5.0,
                        twinkle / 5.0
                    );
                }
                for &(page, background) in &pages {
                    let picture = sky.draw(&view, scale, &lk, shine, background);
                    let mut img = read_back(&device, &queue, &picture);
                    // Saved solid, as the window shows it.
                    let mut sum = 0u64;
                    for pixel in img.pixels_mut() {
                        pixel[3] = 255;
                        sum += u64::from(pixel[0].max(pixel[1]).max(pixel[2]));
                    }
                    let mean = sum as f64 / f64::from(img.width() * img.height()).max(1.0);
                    eprintln!(
                        "LOOK {lit_by} {page} zoom {zoom} in view {in_view} apart {:.2} pt mean {mean:.2}",
                        crowd.apart * view.unit()
                    );
                    img.save(format!("{out}/galaxy-gpu-{lit_by}-{page}-{zoom}.png")).expect("saved");
                }
            }
        }
    }
}
