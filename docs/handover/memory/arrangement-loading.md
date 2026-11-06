---
name: arrangement-loading
description: Songs arrangement shows right after the save is read; audio files found after on the "Arrangement samples" thread (SampleFinder keeps folders 60 s), FL factory fast path, [Arrangement] log lines; PR state and what to check next
metadata:
  type: project
  modified: 2026-11-04T05:20:41.718Z
---

Solomon's F12 note 2026-11-04 04:34 (Songs, build 6009edd): "still loading arrangement slowly" on rainforest (FL,
"Reading the arrangement..." stuck). Routed to the "Draw arrangement and waveforms on GPU" thread.

Cause: `mo_app::arrangement::read` parsed the save AND located every audio part (SampleLocator: candidates, beside,
project tree walk, DAW content trees, Sounds library) before posting; a new locator each song, so trees were re-read;
the next song's parse waited on the one "Arrangement" Newest thread.

Fix: PR #249 MERGED 2026-11-04 ~05:55Z as 196241c (squash; review medium, 10 points, 2 and 9 kept; a Windows CI
fail fixed in e92b1ab: factory path joined a folder at a time). #242 and #206 threads told the overlaps are theirs:
- `arr::read(path)` parses only; `Preview.found: Option<..>` (None = still looking; Some(empty) at once with no audio).
- mo-ui: second Newest thread "Arrangement samples": `arr::find_samples(views, path, locator, wanted)` checks
  `wanted` (SHOWN == asked) between files, then posts `samples_found`, then peaks. A tree read under way can't be
  stopped (review point kept).
- `arr::SampleFinder` keeps one locator for FOLDERS_FRESH (60 s), like live.rs SAMPLES_FRESH. Sounds library itself is
  already kept app-wide by `sound_library::shared`.
- View `looking_for_files()` / `set_found()` / `file_here()`: no "not on this computer" marks until looked.
- `SampleLocator::locate` tries `fl_factory_file` first: "%FLStudioFactoryData%\rest" -> a content folder ending
  Data\Patches\Packs, 3 levels up, join rest; else old search. `is_daw_content` = starts with '%' (one rule).
- Log: "Read the arrangement of X in N s" (>= 1 s); "Looked for the N audio files of X in T, F found: read <folder>
  (<n> files and folders) in T; ...; the Sounds library in T" or "Stopped looking ..." (>= 1 s).
  `SampleLocator::tree_reads()` (latest 64, numbered), `trees_read()`, `library_read()`.

Overlaps: #242 (mo_catch) changes the catch_unwind line in `read` and in `read_peaks` (1-line conflicts); #206
(drag-out) uses View.found in pick.rs: a drag during the search would carry no audio (needs a `looked` check).

What's next: Solomon's next log should show where the time goes on his PC; if the parse itself is slow, look at the
FL arrangement reader. See [[waveforms-on-gpu]], [[gpu-for-graphics]], [[solomon-pc-disks]].
