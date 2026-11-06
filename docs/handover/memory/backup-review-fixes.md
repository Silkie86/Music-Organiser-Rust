---
name: backup-review-fixes
description: Fixes for the 2026-11-03 whole-app review's backup findings (thread "Fix the backup review findings"): PRs #211-#228, all merged, what each does, what's left
metadata:
  type: project
  modified: 2026-11-04T01:34:10.389Z
---

2026-11-04, thread "Fix the backup review findings" (branches claude/project-thread-4ujkag*):
- #211 merged (ac7c862): plan::Listed (folder list, nearest listed folder decides; Whole = goes whatever the level),
  plan::Folders { backup, listed }. Gathered/Prepared no longer have `always`.
- #215: folders::gather(list, backup, stop) canonicalizes listed folders; links walked where they really are, files
  take nearest listed folder's spelling; link to nowhere = missing; backup left out by real path.
- ALL MERGED: #211, #215, #217, #220 (c7b0a86), #225 (46cafbc), #228. #220 rebased over #224's sweep: Settings/BackedUp keep write_whole, part_of removed.
- #225 rebased on main 2026-11-04: main's #216 already had on_bulk_pool + catch_unwind; #225 keeps on_copy_pool (build_pool), writer, finish_up. Waiting only on Solomon trying: start a backup, close the app, reopen, last run says Stopped.
- #217: copy.rs keeps big copy if new < half; parts.rs journal `.mo-backup/parts-<pid>-<ns>.txt` (swept after 6 h
  quiet); saved_since (record time moved >= 1 ms) beats 2 s window; drive case probe for places only (record and
  zips keep the computer's way); links_work probe for Windows code 5; keep_time errors ignored.
- #220: zip-failed songs copied as plain folders (run not ok); Tally struct; samples deduped + not_there cache +
  stop; links both ways; plan::newest_save pub; mo_app::backup::write_whole (pid-n temp, sync, clears own leftovers
  >= 10 min).
- #225 (stacked on #220): background::on_bulk_pool (plan, drive looks) + on_copy_pool (run), up to 4 threads;
  mo-ui backup "Backup files" writer (FILES/WRITING); backup::finish_up from main stops a run, waits 20 s.
- #228: pack.rs times-only repack (Source::Same raw-copied with new time via raw_copy_file_touch; full/readonly drive -> UpToDate; else full pack).
- Left: copy helpers still
  duplicated with mo-space folder_move, Mac NFC names, connect() reads settings on UI thread.
- Cloud build note: mo-ui needs `apt-get install libasound2-dev` to compile here.

See [[backup-older-and-gone]], [[rust-rebuild-rules]].
