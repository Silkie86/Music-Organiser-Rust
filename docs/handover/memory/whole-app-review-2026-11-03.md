---
name: whole-app-review-2026-11-03
description: Whole-app review of main (2026-11-03/04): report path, who owns which fixes, what's done, what's next
metadata:
  type: project
  modified: 2026-11-04T05:39:00.000Z
---

Report: /mnt/project-files/reviews/whole-app-review-2026-11-03.md. The review ran on b771bdf and its findings were
checked against b980a76. Of 256 findings, 232 held up (1 high, 27 medium, 204 low), plus 8 from a threads review.

Who owns what (as of 2026-11-04 02:30):
- Harmony findings: done in #200 and #230 (harmony thread).
- Crash files and tidy: the clean-up thread, on the #172 replacement branch claude/project-thread-bqy26s. After that
  and #216 merge, a follow-up guard will tell a caught panic from a crash.
- Backup, readers, and Space/library: the coordinator was asked (2026-11-04 ~00:00) to start one fix thread for each.
  Check list_threads to see whether they started.
- Background and UI findings: the "Carry on from the hand-over report" thread.
  - #216 MERGED 2026-11-04 02:32 (6e10278): Newest workers catch panics, on_bulk_pool for backup, two sound readers.
  - #233 MERGED 2026-11-04 04:18: Songs one 62 px line height (Show it), Open in Bitwig and Space look up
    "Bitwig Studio".
  - #241 MERGED 2026-11-04 05:17: held arrow with Autoplay paced by sound_step_ms (mo_app::sounds::Stepping, event.repeat).
  - #248 MERGED 2026-11-04 05:38 (bdc6b99): background::guarded; kit roll, song panel detail, backup plan answer their page on a panic.
    The caught-panics thread (#242, mo-catch) said it converts guarded to mo_catch::catch_crash on its side.
  - #255 MERGED 2026-11-04 21:53 (d10bbaa): lows batch 1 (run gap setting version_gap_months, shell Starter thread
    so DAWs aren't nice 19 on Linux/Mac, data folder ignores empty/relative env, arrangement NaN guards, muted label,
    Type sort). Left: sweep_timing take overlap (diagnostics only). Lows done so far: app-core + app-pages areas.

Solomon 2026-11-04 02:29 paused new code (credits); 03:36 credits back, work resumed.
Merge hold 2026-11-05 08:03, lifted 08:08 ("you can merge if you want now").

Next, when work resumes:
- Space measuring still runs on on_pool (move it to on_bulk_pool?): the screens thread moved the app files walk
  (2026-11-05, PR after #305); Bitwig Fix still on on_pool (mostly CPU, kept).
- Work through the 204 lows nobody owns (next areas: ui-slint, ui-rest). ui-sounds (18) taken by the screens thread 2026-11-05.
- Then the screens queue and the smaller jobs from docs/handover/CONTINUE-REPORT.md.
- Solomon still owes the [Sweep]/[Frames] lines from a galaxy sweep, for the galaxy lag fix.

See [[rust-rebuild-rules]], [[outside-cloud-session]].
