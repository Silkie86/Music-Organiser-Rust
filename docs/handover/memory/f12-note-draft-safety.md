---
name: f12-note-draft-safety
description: How the F12 note keeps typed words safe (draft part files, unsaved.txt, the turn, IN_THE_DRAFT, ALONE count, crash hook order) and the trade-offs accepted in the clean-up PR's reviews
metadata:
  type: project
---

Rule 17 ("F12 notes keep their draft through restarts") went through nine max-effort reviews in the clean-up PR
(#224, branch `claude/project-thread-bqy26s`, 2026-11-03/04). What a later change must keep:

**Typed words are always in at least one place:** draft.txt, unsaved.txt (failed saves, end-of-run words), a saved
note-<time>.md, the session log, or `UNSAVED` in memory (mo-ui note.rs) until a write in the draft's turn worked.

- Draft writes: `draft.txt.mo-part`, synced, renamed (Windows retries error 5/32); without the turn (crash, close)
  `draft.txt.mo-part-2`. The sweep never touches the draft, its parts, unsaved.txt or writing.lock.
- The turn (DRAFT_TURN) is taken on the low-priority "Note files" thread or a save's own thread, never the window's
  (rule 12). A crash waits at most 1 s, then writes alone.
- ALONE counts writes without the turn from before they read UNSAVED until they return. A write in its turn that sees
  ALONE > 0 (looked at after it read UNSAVED) lets nothing go (Kept::Alone); a crash that wrote alone queues
  `KeepDraft(OPENINGS)` so a write in its turn follows.
- KeepDraft(n) carries the note it's for: a newer note's words wait for that note's own write.
- IN_THE_DRAFT (newest opening written in turn): an older note's ClearDraft can't clear a newer note's words.
- A Fill for an opening that's no longer the newest reads "" (no double text).
- `save_note`: .md synced before the picture; words leave BEING_SAVED only once on disk; a failed save keeps them
  itself. KeepFailed's `shown` lets go of failed words that note showed, so they don't show twice.
- Crash hook order (mo-ui main.rs, shared with its test): log line, flush, crash file, keep the note, backtrace,
  std's report last. Log lines while `panicking()` use the stored time-zone offset and a brief lock wait.

**Accepted:** words shown twice (never lost) in rare cases, incl. (review 9) a failed save's words after a crash the
app survives + slow disk + note closed before it showed the draft; two writers without the turn race on part-2; a
power cut between sync and rename loses that write; error 32's retry is only tested on Windows CI.

Not covered (whole-app review 2026-11-03): writing.lock overwritten when the C# app holds it (mo-app note.rs:113);
caught panics still log as crashes (see [[crash-logging]]).

Tests take turns (`one_at_a_time`, shared statics); each rule was broken on purpose to see a test fail. Reviewer
models of every job order live only in old scratchpads; re-run a model when changing the turn/ALONE logic.

See [[app-cleanup-rule-28]], [[cleanup-notes-sweep-decisions]].
