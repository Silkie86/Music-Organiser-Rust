---
name: space-old-copies-api
description: The old copies thread's API for Space part 2 (PR #174, not merged at 2026-11-03 20:32): names to call, Recycle Bin rules, half copies are permanent
metadata:
  type: project
---

Sent by the "Remind about old copies and half moves" thread on 2026-11-03 20:32, for Space part 2 ([[screens-queue]],
[[space-backend]]). PR #174 was in its 8th review round; it said the names are stable and it'll message if they change.
Check the merged code before using these. All read the disk: call off the UI thread.

- Records: read library.db settings `space_moved_folders` and `space_old_homes` read-only, then
  `folder_move::parse_moved` and `after_move::parse_old_homes`.
- List: `old_copies::remind(moved, homes, look_in, stop)` -> Reminder, biggest first. Leftover: path, bytes (now),
  what (OldCopy(i), Behind(i), Kept(OldHome), NotKept(MovedFolder), Unknown), now_in, since, stays (None = can go,
  else the sentence why not). `Reminder::title()`, `can_go()`, `bytes()` count only what can go.
- Before removing an old copy show `folder_move::only_in_old_copy(moved, all, stop)` (files only in it or newer
  there; Err = Stop).
- Remove, always to the Recycle Bin: `folder_move::remove_old_copy(moved, all, sure, bin, stop)`,
  `after_move::remove_old_home(home, moved, sure, bin, stop)`; both re-check and return a Done; `sure` skips the
  only-in-it check. Before removing, add a Kept to the old homes and keep a NotKept as a move. Unknown always stays.
  Put back: `clean::undo(done, bin)`.
- Half copies: `half_copy::half_copies(moved, also, &SystemDrives, stop)` (path, from, started, bytes, state CutOff /
  MarkerOnly / Stays(why)); `half_copy::clear(folder, moved, &SystemDrives, stop)` -> Cleared or why it stays. A clear
  is PERMANENT (no bin, no Put back: a half copy only holds copies). The page must say so.
- Recycle Bin reminder: `in_bin::remind(done, bin)`, then `title()` and `note()`.
