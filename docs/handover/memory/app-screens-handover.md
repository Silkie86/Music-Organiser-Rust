---
name: app-screens-handover
description: Handover 2026-11-02 05:10 of the "Build the app screens" thread: what's merged, PR #164 open, Solomon's queued asks and notes
metadata:
  type: project
  modified: 2026-11-02T05:06:57.650Z
---

Handover from the "Build the app screens" thread (2026-11-02 ~05:10 UTC). Background in [[app-screens-ui]], [[gpu-for-graphics]], [[rust-rebuild-rules]].

**Merged:** #161 galaxy on wgpu (gpu.rs, sounds/sky.rs, mo_app::galaxy_look, galaxy_grid). #163 read-ahead while sweeping (40 ms, 128 KB, 4 files) + mini-map (MapInset in studio.slint, shown zoom > 1.25).

**Open: PR #164** on branch `claude/project-thread-uptbg1` (the other session pushed its own review fixes as e76c5c0, CI green; my fixes 7d51116 were overwritten, diff in /mnt/project-files/screenshots/year-strip/review-fixes-7d51116.patch: no strip on History, Used toggle clears years, strip fits short windows, year counts + grid cached, YearRange `any` removed): year strip (`YearRange`/`YearBar` in studio.slint) on Sounds and Songs (replaces Songs' YearMark/YearStart scrubber; Filters.years in mo-app sounds.rs/songs.rs; year_counts newest first), galaxy "Light by" (LightBy enum in galaxy_look, flag byte 1..255 = how lit, shader grows lit stars), Used column 96px. Local fmt/clippy/tests passed. Quick review done and fixed; galaxy check passed. STILL TO DO: carry over the missing fixes from the patch, then merge and reset the branch (fetch main, checkout -B, merge -s ours old branch, push). Light by isn't on the CPU fallback (galaxy_picture) yet: follow-up.

**Solomon's queued asks (not started):**
1. Arrangement + waveforms on the GPU (he wants ALL graphics on wgpu; CPU only fallback).
2. Drag sounds (tiles, list rows) into a DAW with a nice animation (lift/dim, pulse on drop); also drag MIDI and audio clips out of the arrangement and tracks (MIDI written as .mid, audio = its sample).
3. "Send to Bitwig" on the Sounds side card, like the old app.
4. Show the ⋯ actions directly on every Sounds card (there's room), not only in the menu.
5. Sounds side card clipped on the right at 4K/ultrawide.
6. Year strip should apply to every list page (Sounds + Songs done in #164; MIDI, Presets, Plugins etc. next).

**F12 notes not yet handled (%LOCALAPPDATA%\MusicOrganiser\notes):**
- 03:53 and 04:35 log: Sounds read takes 833 s for 453,885 sounds; Songs library read 15-87 s. The launcher runs a DEBUG build (mo-ui not optimised).
- 04:25 "overlapping txt": while sounds load, the galaxy empty state draws "Listening to your sounds first..." on top of "Try fewer filters..." (two subtitle texts at once).
- 05:04 "galaxy looks a lot better but still not responsive, I still feel a lag; check the old project, it did a report to find the bottleneck". Log: Freeze 332 ms on Sounds "in no marked step, so drawing or laying out". Look in C# Music-Organiser for its perf report/profiling (frame timing report) and port it; measure before guessing.
- Earlier log items: 4480 ms Songs freeze at 03:07, 459 ms key.
