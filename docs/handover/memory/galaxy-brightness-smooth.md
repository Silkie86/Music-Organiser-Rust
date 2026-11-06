---
name: galaxy-brightness-smooth
description: Galaxy brightness made smooth with zoom (Solomon's 2026-11-03 F12 notes x44/x69/x87) in PR #198, and the 1.4 s freeze fix #187: cause, new rule, constants, how to check
metadata:
  type: project
  modified: 2026-11-03T23:03:30.983Z
---

Thread "Smooth galaxy brightness and stop freeze" (2026-11-03), from Solomon's F12 notes 19:20-19:21
(copies in /mnt/project-files/notes/app-notes-2026-11-03/): "dim stars" at x69 (6,146 in view), "suddenly gets
brighter" at x87 (3,865), "brightness should be smooth as you zoom in" at x44 (13,009). His 4K screen: galaxy about
1830x1171 points at scale 1.5, 453,885 sounds.

**Freeze, merged #187 (2094974):** "Galaxy: take the sorted stars" 1390 ms was an old SoundList freed on the window
thread. `background::free_later` (one low-priority "Freeing" thread, catch_unwind per item), `SoundList::drop` hands
over lists of 2,000+ (FREE_LATER_FROM) whole via mem::take, `freeing_here()` stops a hand-over loop.

**Brightness, PR #198, MERGED 2026-11-04 (squash f5e9dbf):** cause was `galaxy_look::look` switching painted -> crisp by COUNT in view
(smooth 5,000 -> 1,500), so a big screen's thousands of well-apart stars stayed dim painted dots, then crisp stars
(~30x the light) faded in at once. New rule:
- `Grid::crowd` -> `Crowd { in_view, apart }`. Review fixes (commit 0d033e1): each square's gap (log2, in
  `Grid::gaps`, made in Grid::new) is judged with >= 4 stars (ROUND): own square, else 3x3, 5x5, 9x9... windows (a
  summed-count table); the view's apart = stars' log2 gaps averaged (geometric mean), so a sliver of a dense crowd at
  the edge sways it only by its share. The old count-weighted mean let 15 edge stars flip 150 crisp stars to paint,
  and a lone star in a square was capped at 1/192 apart (small libraries stayed painted). `Crowd::even` = no grid.
- `look(view, crowd, share)`: crossfade by apart in points (STARS_FROM 2.5 pt, one doubling); `bright` =
  smooth(0, 3, doublings past); radius = ROOM 0.22 * sqrt(star_light(0)/star_light(bright)) * apart, capped 1.9*grow;
  DOT 0.45. Shader (sky.rs): core = strength * mix(painted_dot, close, bright); lit size, halo, glow follow bright;
  camera floats 17-18 are bright, gain.
- CPU picture: `glows_in` from 8,000 (none) to 1,500 in view (log ramp via gl::smooth), min'd with the card's
  crisp (40,000 made the no-card picture ~1.7x slower mid-zoom for invisible glows).
- Known: on a clumpy map where the view's mix changes fast (flying into a corner) `painted` can move ~0.1 per 1% zoom,
  but the crossfade is brightness-matched (card mean moved < 0.3/255 per step); per-star crowd in the shader would be
  the full fix but makes lone outskirt stars crisp at x1 (look change), so not done.
- Result (made-up blob map as crowded as his): Nothing starlight before x44 0.9 / x69 0.3 / x87 10.8 / x110 14.6,
  after 14.8 / 14.1 / 14.1 / 13.8; joins the old close-up look by x110-x130. Length has a smooth hump to ~51 at x69
  (close-up 42-49): accepted, mention if he finds Length too bright mid-zoom.
- How to check: software Vulkan render, `mo_ui` `sounds::sky::tests::render --ignored` with GALAXY_GPU_LOOK=dir,
  GALAXY_GPU_MAP=blob, GALAXY_GPU_VIEW=1830x1171, GALAXY_GPU_SCALE=1.5, GALAXY_GPU_ZOOMS, _LIT, _PAGES; prints
  `LOOK ... mean M` (dark page is 18, so starlight = M - 18).
- Pictures and chart: /mnt/project-files/screenshots/galaxy-brightness/.

What's next (2026-11-04, credits low, no new code until Solomon says): Solomon tries the galaxy at home at x44,
x69, x87 with Light by Nothing, Liked and Length. If Length looks too bright mid-zoom (the ~51 hump at x69), lower
its lit growth there; if some views still switch fast, the full fix is a per-star crowd in the shader. Thread stays
open until he's happy. Related: [[galaxy-star-look]],
[[gpu-for-graphics]].
