---
name: off-the-ui-thread
description: Solomon's standing ask (2026-11-04): move anything that can lock or slow the screens off the UI thread; sweep owned by the tiles-speed thread
metadata:
  type: feedback
  modified: 2026-11-04T05:22:11.386Z
---

Solomon, 2026-11-04 05:18: "anything that can lock the screens and slow down the ui should be taken off the main thread if it can".

**Why:** freezes of seconds on his PC (C: 98.8% full, RAM 88%); the window thread doing file reads, DB queries or big layouts stalls every page. See [[sounds-tiles-speed]], [[slint-frame-speed]].

**How to apply:** in every change, no file read, DB query, sort of thousands, or one-shot layout of thousands of items on the window's thread: use mo_app::background workers (Newest), virtualised lists (ListView), results back by invoke_from_event_loop. The coordinator asked the "Speed up Sounds tiles and fix freezes" thread to sweep the whole app for such work after its Galaxy sweep work, in small PRs off main, messaging the owning threads where a page belongs to them. Related to rule 12 in [[rust-rebuild-rules]].

**Sweep done 2026-11-04:** Sounds, MIDI, Ctrl+K lists were already on Newest workers. Fixed: Presets filtering one pass on a worker (#263, 170 -> 39 ms on 100k lines, debug); big one-read files (Bitwig copies look, hashes) open with sequential scan (#252); long MenuChip menus in a ListView (#245); MIDI piano roll on the GPU (#261, GPU thread). Songs' view still runs on the window's thread (small lists, not measured). Follow-up: sounds/grid.rs could use canvas.rs's shared helpers from #261. Later the same day a 961 ms freeze (3,090 page faults) during the Bitwig copies look on build e277a6f: #278 moved that look to on_bulk_pool and gave the bulk/backup-copy pools low memory priority on Windows (SetThreadInformation ThreadMemoryPriority LOW). If freezes with many page faults still come during long disk jobs, next idea: THREAD_MODE_BACKGROUND_BEGIN (very low I/O too).
