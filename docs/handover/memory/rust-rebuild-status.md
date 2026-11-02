---
name: rust-rebuild-status
description: Where the Music Organiser Rust rebuild stands (phase, repo state, agreed app map decisions, open questions)
metadata:
  type: project
  modified: 2026-10-31T13:00:00.000Z
---

- Phase 0 (PRs #1-#7) merged 2026-10-29; left: Solomon runs Galaxy check.cmd at home, Claude reads
  logs/galaxy-check-*.txt. CI uses Rust 1.99: run checks with `cargo +1.99`.
  MERGING (changed by Solomon 2026-10-29 20:43): Claude merges PRs itself once the code review is done, its points
  are fixed and CI is green. No Merge card, no waiting.
- 2026-10-30: Phase 1 (file readers: FL, Bitwig, Studio One, MIDI SMF + Clip Files, formatlab, twin) merged #8-#20,
  ultracode review fixed. Later: catch_unwind around reader calls in mo-app (overflow-checks on), fuzzing, Bitwig
  note expressions. Open "Not yet" items in crates/mo-formats/DIFFERENCES.md need Solomon's saves.
- 2026-10-31: Phase 2 (Songs page) started. mo-library (read-only library.db, C# song grouping port) merged #21;
  copied folders + renamed saves merged #22; Songs list page (search, chips, year scrubber, lazy list model)
  merged #23; song panel merged #24; Open/Show in folder/Copy path + Songs page shortcut list (C# ids projects.*)
  merged #25; Health strip merged #26 (Health strip, show-only until writing). Next: scanner/folder watch. Not yet: last_opened (read-only DB),
  tooltips (no shared tooltip part yet; rule 15 wants keys in tooltips), Older saves menu, arrangement preview.
- F12 note behaviour agreed in code: only Cancel discards; Esc/click-outside keeps the draft for next F12.
- DECIDED 2026-10-30: Slint royalty-free licence (not GPLv3). The app must show a "Made with Slint" credit (AboutSlint
  widget in an About box/Settings page). deny.toml allows LicenseRef-Slint-Royalty-free-2.0.
2026-10-31 07:31: Solomon set up the launcher at home (needed the repo zip + rustup) and is testing PR #30
(live updates). Don't push to #30 while he tests without asking. Merged #27 scanner, #28 folder watch, #29 sample locator.
2026-10-31 later: #30 merged; #31 (songs twin, copies count) waiting on CI. Galaxy check PASSED on Solomon's
RTX 3060 (60 fps all stages). Whole-library twin test: 266,466 same, 14,901 differ; all judged (DIFFERENCES.md):
only real Rust fix was FL created ms (cut like .NET AddDays, v11). Local branches style-tokens (Type/Space tokens,
proposed scale agreed by Solomon) and twin-fixes wait for #31 to merge. Solomon's PC: folder access via
remote-devices (<FL old projects folder>, MO Test, <a Studio One songs folder> granted).
2026-10-31 ~10:00: #32 (Type/Space tokens) merged. #33 = CI scope (tools/dev/ci_scope.py: PRs test only changed crates;
full on main + workflow_dispatch before ultracode) + twin fixes (FL v11 date, DIFFERENCES rows incl. FL 20 playlist
tracks: C# counts from tracks kept, not 500 = Rust Better). Local branch `tips`: shared Tip/TipLayer/TipState in
studio.slint (no PopupWindow: popups swallow clicks). Other threads now own mo-harmony, Sounds/MIDI/Presets backends
and the FL->Bitwig converter; this thread keeps page UI, shared style, readers, twin. Design system v4 has the sizes.
2026-10-31 ~13:00: arrangement preview done: readers #42 (FL v12, Bitwig/Studio One arrangement.rs), Overview drawing
#47 (mo_app::arrangement View/Frame with tiny-skia, mo-ui arrangement.rs two Newest threads, shared PointerPad/HoverNote).
Next: waveforms (audio decode + peaks cache), then the Fix list under the preview, damaged-FL older-save fallback.
Open: FL on/off ramp automation (does FL switch at 0.5?) needs Solomon/FL to check.
C# app is frozen (bug fixes + its PRs #51–#56 only) and stays the daily app until the Rust one is better.

Branch lesson (2026-10-30): after each squash merge, rebuild the work branch from origin/main and cherry-pick, then
ours-merge the old remote branch; an ours-merge of an older PR left #16 conflicting and CI never started.

App design decisions, keys and FL notes: [[rust-app-design-decisions]]. See [[rust-rebuild-rules]].
