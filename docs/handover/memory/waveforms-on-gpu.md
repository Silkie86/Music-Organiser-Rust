---
name: waveforms-on-gpu
description: Sounds waveforms on the GPU (PR #209 merged dc1b455, review follow-up #240 merged c21c4d9): pictures kept per place, phase and scale, 4 ms a frame, femtovg-exact edges, sub-pixel phase fix, look test, what's next
metadata:
  type: project
  modified: 2026-11-04T05:16:00.000Z
---

PR #209 MERGED 2026-11-04 ~04:10Z as dc1b455 (squash; thread "Draw arrangement and waveforms on GPU"). Its last
commit (sub-pixel phase) merged unreviewed (credits ran out); follow-up PR #240 (same branch) MERGED 2026-11-04
05:15Z as c21c4d9 (squash) with the review's fixes (high-effort review of that commit, then medium of the fixes, 7
points each). The branch kept #209's old commits via `merge -s ours` (force-push was refused here).

- `Waves.picture(shape, points, tint, w px, h px, line px, over-x, over-y, asked) -> WavePicture {picture, x, y}`
  (studio.slint) -> mo-ui `waves.rs`. over-x/y are fractions 0..1 (Slint sends `px - floor(px)` of its
  `absolute-position`); Rust `phase()` rounds to 64ths (>= 63.5/64 is the next whole pixel, as femtovg's
  `pixel_align_origin` ROUNDS an Image's origin; a Path is drawn fractional). The card draws the lines that far over
  in a picture ceil(w)+1 x ceil(h)+1 (`mo_app::waves::scene(.., at, ..)`); the Image sits at -x,-y phx.
- Kept (`Kept`): per Shows (path, points, tint, card generation) a Vec of pictures (size incl. scale bits, phase),
  each with `asked` and `shown`. A new picture replaces ones of the same HEIGHT not shown this frame (tile and card
  kept apart). Moving (no picture at this phase) -> `Found::Near` stand-in, stamped shown via `shown_in_place`, + ask
  again; at the same phase in 2 consecutive frames IN WHICH WAVEFORMS ASKED -> drawn. 64 MB LRU by `shown`, never
  one shown this frame or the one before. Scale change -> all cleared (`scale_is`). 4 ms a frame.
- Look = femtovg's own stroke: fringe = 1/ceil(scale) (1 px <=100%, 0.5 px 125-200%); canvas.rs `stroked()`.
- Look test: `WAVES_GPU_LOOK=<dir> cargo +1.99 test -p mo-ui --lib -- --include-ignored --nocapture waves::`
  (vs femtovg, 3 phases): within 2/255, 100% ink. Real window (scratch E2E on lavapipe + Xvfb, card vs path):
  within 2/255 at 100% and 150% incl. after scrolling, except the tooltip near the pointer.
- Accepted: 45% unplayed copy where neighbouring fringes overlap (a few /255); stand-in up to 0.5 px off while
  scrolling; one lookup per visible Waveform per scroll frame (0.67 us each, measured; review point kept).
- `gpu::prepare`: a painter whose invoke_from_event_loop closure never runs is forgotten (`OnItsWay`).

What's next: Solomon to check on his PC (scroll, pick and play, resize; `[Waveforms]` log line if the card
refused). No real-GPU timings yet (only lavapipe here). The "Speed up Sounds tiles" thread (whole-grid tile picture,
after #209) was told about the sub-pixel phase and the per-frame lookups while scrolling.

See [[gpu-for-graphics]], [[screenshot-compare-traps]].
