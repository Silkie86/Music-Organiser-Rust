---
name: piano-roll-on-gpu
description: MIDI page piano roll is one picture drawn off the UI thread on the GPU (PR #261 merged 0d3aa81): mo_app::piano_roll + mo-ui piano_roll.rs, RollLabel/RollRoom, shared canvas helpers, checks, overlaps, what's next
metadata:
  type: project
  modified: 2026-11-04T07:11:57.178Z
---

Asked by the "Speed up Sounds tiles and fix freezes" thread (Solomon 2026-11-04 05:18: what can slow the screens goes
off the UI thread). Built in "Draw the app's views on the GPU", branch claude/project-thread-gra2a5. PR #261 MERGED
2026-11-04 ~07:55Z as 0d3aa81 (squash; review medium, 9 points, 1/4/6/7 fixed, 2/3/5/8/9 kept).

Design:
- Before: up to 4,000 (ROLL_NOTES) Slint Rectangles + per-key rows/keys/lines/names, drawn by the UI thread.
- `mo_app::piano_roll::picture(roll, Room, scale, Choices, &Colours) -> Picture {scene, labels}`: rows edges rounded
  to f32 so rows touch exactly; notes snapped to whole pixels at `scale`; SIDE_WITH_KEYBOARD 52 / SIDE 28 points.
- midi.slint: `MidiData.roll-keys` (int), `roll-picture` (image), `roll-labels` ([RollLabel]), callback
  `roll-laid-out(RollRoom {width, height, left: Space.xs, margin: Space.sm, name: Type.tiny})` fired on init and on
  width/height/keyboard/names/scale (`1px / 1phx`) changes. Image shown at natural size (`source.width * 1phx`).
  roll-keyboard / roll-names unchanged (the MIDI page thread will remember them).
- mo-ui piano_roll.rs: Newest thread "MIDI roll drawing", shown/SENT counters (arrangement pattern), HERE mode for
  mo-screens (`draw_here`), graphics_ready/card_gone/graphics_gone wired in gpu.rs, look_changed in apply_look.
- canvas.rs now shares `Drawing::new(scene, scale, on_card)`, `show(drawing, &CARD, &Steps)`, `refill`, `colour`,
  `rgb` (arrangement uses them too). grid.rs (Sounds tiles) still has its own copy: the tiles thread's to move.

Checks: 36 before/after shots (scratch mo-screens patch, never committed): same at 100% but corners (<=51/255) and
1/255 shine; 1 px shifts at fractional widths/scales. Ignored test `piano_roll::tests::the_card_draws_what_the_processor_draws`
(llvmpipe, max diff 19; asserts none > 32, <= 1% > 8).

#259 (MIDI thread) merged first; its tests and `a_pianos_black_keys_are_its_sharps_and_flats` both kept
(`roll_keys`/`RollKey` are gone). The MIDI thread moves its filters onto FilterPicks next.
What's next: Solomon tries the roll on his PC (real GPU untested here). See [[gpu-for-graphics]],
[[midi-page-screens]], [[off-the-ui-thread]].
