---
name: slint-frame-speed
description: Why every Rust app frame was slow (femtovg 0.27 rebuilt GPU pipelines each frame), the vendor/femtovg patch (PR #203), and how to bench the real window in a cloud session
metadata:
  type: project
  modified: 2026-11-03T23:48:19.843Z
---

**Cause (found 2026-11-03):** femtovg 0.27.0's wgpu renderer dropped every pipeline a render didn't bind, and Slint
1.18.1 renders twice per frame whenever a rendering notifier is set (first a clear-only flush; `Canvas::set_size`
always queues SetRenderTarget, so that render is never empty). The app always sets one (`mo_ui::gpu::connect`: GPU
sharing + [Frames] timing), so all 10-13 pipelines were rebuilt every frame. Solomon's logs: Songs ~12 ms, Sounds
23-31 ms a frame, window drawing 80% of the time while the stars twinkle.

**Fix:** PR #203 (merged 2026-11-04 as 4d827b9) vendors femtovg 0.27.0 at `vendor/femtovg` (PATCHED.md), cache cleaned only past 256 pipelines;
`[patch.crates-io]` in the root and tools/galaxycheck Cargo.toml; workspace `exclude`; rustfmt.toml there disables
formatting; `#![allow(unused_imports, dead_code)]` because CI has RUSTFLAGS=-D warnings and path deps aren't capped.
`mo-ui/tests/drawing_library.rs` fails if a lock file stops using the copy. Remove all of it once a Slint release
uses femtovg >= 0.28 (0.28 has LRU eviction). Slint 1.18.1 was the newest on 2026-11-03.

**Bench (lavapipe, numbers before -> after):** Songs 115 -> 6 ms, Sounds list 202 -> 12-16, galaxy 148-172 -> 6-8,
Backup 177 -> 7, Space 150 -> 4. Sounds tiles view still 33-37 ms: look at it next.

**How to bench the real window here:** scratch bin copied from mo-screens (sample data; kept at
scratchpad/bench/mo-framebench.rs, never committed; must sit in crates/mo-ui/src/bin to build). Needs
`apt-get install libxkbcommon-x11-0`. Run: `SLINT_WGPU_CPU=1 VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json
WGPU_BACKEND=vulkan SLINT_SCALE_FACTOR=0.5 XDG_DATA_HOME=<scratch> xvfb-run -a -s "-screen 0 3840x2160x24" <bin>`
with a 1280x720 window (same 2560x1440 layout, 1/9 of the pixels). Read the [Frames] lines from
<scratch>/MusicOrganiser/logs. Where time goes: gdb -p <pid> -batch -ex "thread 1" -ex "bt 60" in a loop
(use `pgrep -x`, not -f, or you sample xvfb-run). Count calls with address breakpoints (binary is PIE: add the load
base from /proc/<pid>/maps). Never change RUSTFLAGS locally: it rebuilds everything and the disk fills.

**Bench traps (2026-11-04, tiles thread):** the cloud env sets RUST_BACKTRACE=1, which makes wgpu-core's debug snatch
lock capture a backtrace on every lock (~19% of a frame): run benches with `env -u RUST_BACKTRACE`. Frame times on
llvmpipe are mostly the software raster; compare the window thread's own CPU per frame instead (/proc/<pid>/task/<pid>
stat over 20 s / frames). Scripts: /mnt/project-files/frames/bench/ (bench.sh, cpu.sh, count.py = gdb draw-call counts).
Tiles: see [[sounds-tiles-speed]].
