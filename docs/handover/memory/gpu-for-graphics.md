---
name: gpu-for-graphics
description: Solomon wants ALL custom graphics (galaxy, arrangement, waveforms, piano roll...) drawn on the GPU (wgpu), CPU only as fallback; the shared painter (Scene + canvas.rs), gpu::Painter/prepare, teardown and E2E facts
metadata:
  type: feedback
  modified: 2026-11-03T23:18:46.697Z
---

Solomon (2026-11-02 03:05): "I did originally ask for the gpu to be used for all graphics why didn't that happen?"
Spec 9.1: custom views draw with wgpu into a texture Slint shows; CPU (tiny-skia/rayon) only as fallback and for
mo-screens/CI. Galaxy moved first; arrangement #184 MERGED 2026-11-03 (0d8633c); Sounds waveforms = PR 2 (same
thread "Draw arrangement and waveforms on GPU", branch claude/project-thread-gra2a5 restarted from main).

**Why:** speed and it was asked for up front (galaxycheck: 300k stars at 60 fps on his RTX 3060).
**How to apply:** any new custom view starts on wgpu, CPU path kept for mo-screens/CI. See [[app-screens-ui]].

Shared painter (#184):
- `mo_app::shapes::Scene` (points; Rect sharp, Rounded/Line soft, butt ends) worked out off the UI thread.
  `pack(scale)` = 32 bytes a shape; Rect edges packed in whole pixels by `crisp` (the ONE pixel rule; the shader only
  multiplies by `frame.fit`). `paint(scale)` = tiny-skia fallback. `usable_scale` is the one scale check.
- mo-ui `canvas.rs`: one instanced draw, premultiplied over, two alternating Rgba8Unorm textures -> `Image::try_from`.
- Slint's device: `downlevel_webgl2_defaults().using_resolution(adapter)`: no storage buffers, stride <= 255,
  max_buffer_size 256 MB -> at most 1M shapes a part. femtovg shows a wgpu texture as STRAIGHT alpha.
- Arrangement notes in the same pixel row that touch are merged (`Runs`): 100k notes -> <500 shapes, same pixels.
  The drawing thread numbers pictures (`SENT`); the window skips any older than the newest sent.
- `gpu::Painter<T>` {made (generation, T), refused, preparing}: `card()`, `on()`, `refused(on)` (drops only a painter
  made on that card), `gone()`. `gpu::prepare(app, name, &SLOT, make, ready)` builds it on a background thread; the
  processor draws meanwhile; `ready` redraws.
- Teardown (RenderingTeardown, also fired when the renderer drops during app drop, when `weak.upgrade()` fails):
  every view's `card_gone()` lets its painter go WITHOUT the app, then `graphics_gone(app)` if the app is there.
  Reason: wgpu-core debug builds keep a thread-local (snatch lock trace); dropping wgpu objects during thread-local
  destruction aborts on exit. Never keep wgpu objects in a thread-local past teardown.

Checks:
- Look test (llvmpipe): `ARRANGEMENT_GPU_LOOK=<dir> cargo +1.99 test -p mo-ui --lib -- --include-ignored
  --nocapture canvas:: arrangement:: gpu:: sky::`.
- Real-window E2E (scratch only, never commit): Xvfb :97, `SLINT_WGPU_CPU=1`, `XDG_RUNTIME_DIR=/tmp/claude-0/xdg`,
  `apt-get install libxkbcommon-x11-0`; an example built from mo-screens' sample data (crates/mo-ui/examples/,
  delete before clippy --all-targets), shots with ImageMagick `import -window root`; `E2E_QUIT=1` = quit_event_loop
  path, else hide() like the close button.

PR 2 (Sounds waveforms, #209): `Waves.picture` pure callback, a OneColour canvas picture (RGB = tint, alpha =
coverage) kept per sound, its lines stroked exactly as femtovg strokes the Path: see [[waveforms-on-gpu]]. A pure
callback runs DURING the first frame, before RenderingSetup's deferred graphics_ready: picture() calls gpu::prepare
itself and draws the path until the painter is made. Comparing screenshots: [[screenshot-compare-traps]].

MIDI piano roll (#261 merged, 0d3aa81): one picture off the UI thread; canvas.rs shares Drawing/show/refill/colour/rgb: see
[[piano-roll-on-gpu]].
