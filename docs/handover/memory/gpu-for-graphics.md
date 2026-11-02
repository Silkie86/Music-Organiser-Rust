---
name: gpu-for-graphics
description: Solomon wants ALL custom graphics (galaxy, arrangement, waveforms, piano roll...) drawn on the GPU (wgpu), CPU only as fallback
metadata:
  type: feedback
  modified: 2026-11-02T03:06:06.551Z
---

Solomon (2026-11-02 03:05): "I did originally ask for the gpu to be used for all graphics why didn't that happen?"
The spec (9.1, section 4 galaxy) says custom views are drawn with wgpu into a texture Slint shows; CPU (tiny-skia /
rayon) only as fallback and for headless screenshots/CI. The galaxy (#127..#157) and arrangement (#47, #159) were
built CPU-first and never switched; that was a miss.

**Why:** speed (450k stars, smooth glide) and it was asked for up front; phase 0 galaxy check (tools/galaxycheck,
slint::wgpu_30) proved 300k stars at 60 fps on his RTX 3060.

**How to apply:** any new custom-drawn view starts on wgpu (reuse tools/galaxycheck gpu.rs/sky.rs patterns), with the
CPU path kept for mo-screens/CI. Say plainly if something is CPU-only for now. See [[app-screens-ui]], [[rust-rebuild-rules]].
