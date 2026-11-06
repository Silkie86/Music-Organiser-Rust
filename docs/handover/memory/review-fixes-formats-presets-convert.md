---
name: review-fixes-formats-presets-convert
description: 2026-11-04 whole-app review fixes for formats/presets/convert/formatlab: PRs, curves.db new tables, deferred items
metadata:
  type: project
  modified: 2026-11-04T01:47:50.542Z
---

Whole-app review (2026-11-03) fixes, areas formats-fl, formats-bitwig, formats-other, presets, convert, formatlab.
- Merged 2026-11-04: #208 (FL reader, VERSION 17), #218 (Studio One/MP3 tags/other), #213 (Bitwig reader VERSION 4),
  #226 (convert), #229 (Format Lab).
- #222 (presets/curves.db) merged 2026-11-04 03:35 after main was merged into it (kept main's #224 sweep and Bitwig
  staging, dropped its own curves::tidy). All six PRs of this thread are merged; the thread is done.
- NEXT (when credits allow): wire the deferred items below; the twin tool's panic catch is the most useful.
- Solomon 2026-11-04 02:29: credits nearly gone; no new code, only get open PRs merged.
- curves.db: the Presets page test builds (branch claude/project-thread-6oc5bc) already made curves.db with tables
  curve_saves / curve_sources. Rule 6 => those are left alone; the reader now uses NEW tables curve_reads (with a
  version column) and curve_places. A test-build curves.db is just read again once.
- mo_app::tidy (rule 28 sweep) landed in #224. Not yet wired: mo_convert::save::is_left_behind (convert ".saving"
  leftovers; each save clears its own target's) and curve files of curves no longer in the library.
- Deferred (not done): relink::still_named batch form (perf, Space page); tools/twin catch (now `mo_catch::catch`: clippy forbids catch_unwind since #242) and library-copy.db
  leftover; FL slide-note finding (needs a real save); old-wrapper cp1252; Bitwig encode carrying a damaged zip;
  Studio One MIDI ignoring transpose; arrangement page falls back to 120 BPM for tempo 0 (convert uses 140).
