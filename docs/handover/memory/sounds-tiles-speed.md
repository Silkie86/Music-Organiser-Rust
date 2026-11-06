---
name: sounds-tiles-speed
description: Sounds tiles view frame cost (2026-11-04): measured per-tile costs, plan for one GPU picture for the whole tile grid, freeze diagnostics PR #227
metadata:
  type: project
---

Thread "Speed up Sounds tiles and fix freezes" (branch claude/project-thread-qiahq4).

**Tiles cost (window thread CPU per frame, ~110 tiles on a 2560x1440 layout, llvmpipe):** main ~112 ms frame time (paths each frame), with #209
(waveforms as GPU pictures) 27 ms vs List 12 ms. Per frame tiles ask femtovg for ~490 fills (170 concave = the tile's
rounded border ring) vs 70 for List; glyph runs about equal (246 vs 219). Taking parts away: border -2 ms, waveform +
liked bar -5, dot + heart -3, tile background/ListLine/texts the rest (text only: 16.7). No single culprit.
**Plan:** after #209 merges, draw every tile's shapes (background, border, dot, heart, waveform, liked bar) as one
GPU picture for the visible grid (mo_app::shapes Scene + canvas.rs), Slint keeps only the words and touch areas.

**Freezes (8 s "in no marked step", 2026-11-03 19:43/19:47, Vulkan RTX 3060, after quiet spells):** cause unknown;
PR #227 adds diagnostics: window thread stack sampled at 1 s and 5 s (Windows x64, stack copied while suspended,
dbghelp under Rust's "Local\RustBacktraceMutex<pid hex>" lock), page faults while stuck, last window event
(minimised = Resized(0); winit has no Occluded on Windows). Log excerpt: /mnt/project-files/frames/freeze-log-2026-11-03.txt.
Next: read Solomon's next [Freeze] lines and fix the cause they name.
#209 notes: canvas draw fringe arg (1/ceil(scale)), mo_app::waves::lines + waves::scene, keep gpu::prepare OnItsWay, single grid picture keeps fractional positions.

**#227 merged 2026-11-04 04:24** (squash 2be7393). Windows names needed SymInitializeW with invade=0, then SymSetSearchPathW
adding the exe's own folder (default path is only the cwd), then SymRefreshModuleList. Solomon's 04:22 note: 6.5 s
freeze on Galaxy while "Looking for Bitwig copies" ran (and 340 ms on MIDI earlier); asked him to restart the launcher
so the next [Freeze] lines carry stacks + page faults. Guess (unchecked): disk/RAM pressure from that job.

**Tiles picture (branch claude/sounds-tiles-picture, off main after #209 = dc1b455):** mo-ui/src/sounds/grid.rs +
TileGrid global in sounds.slint; tile parts at fixed places (name/facts heights from probe Texts); picture draws the
Well too (bg behind tiles is Theme.well, not background); rounded shapes use femtovg fringe (canvas.rs shader `f`).
Hovered/playing tile draws its own dot/wave/bar over its shade. Bench: still 32.4 -> 14.9 ms (List 13.2), scrolling
43.6 -> 22.7. Look check: <=354 px differ (>2) per screen, only dot/bar-end edges (max 34). Bench hacks needed (not
committed): gpu.rs allow Cpu adapter if SLINT_WGPU_CPU, BENCH_SEED in new_seed, BENCH_NO_GRID in grid::picture;
scratch look.sh/cpu.sh/bench in the session scratchpad and /mnt/project-files/frames/bench/.
