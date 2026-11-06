---
name: app-cleanup-rule-28
description: Rule 28 (app cleans up after itself, mo_app::tidy), how old F12 notes go to the Recycle Bin, and Solomon's 2026-11-03 answers on notes, bin, old copies, backups
metadata:
  type: project
  modified: 2026-11-03T16:11:10.430Z
---

Rule 28 (Solomon, 2026-11-02): "make sure the app cleans up after itself where possible, I don't want the app filling
up my PC as I use it". Every cache, temp file and report the app writes has a limit or goes when done; a start-up sweep
(`mo_app::tidy`, low priority, ~20 s after start) clears leftovers, and `tidy::prune_logs` keeps `logs\` in bounds at
every start (also through a linked logs folder: the app writes its logs through it). It removes only plain files whose
whole name the app gives them, in the folders it writes them to; never by folder alone, never through a link inside
them. PR #172 was replaced by #224 (branch `claude/project-thread-bqy26s`, "Fix the app clean-up PR" thread), MERGED
2026-11-04 as 8a38df4 after nine max-effort reviews; Windows, Mac and Linux CI green (Windows ran every crate).
Lesson from #172's review: an earlier draft emptied `%TEMP%\MusicOrganiser-Rust` by age, and with TEMP unset that is
the user's home: never clear a folder by name or age.

Solomon's answers to the six retention questions (2026-11-03 14:45-14:48):
1. F12 notes and their pictures: remove ones about 3 months old (drafts never). Done in the clean-up PR, to the Recycle
   Bin, and only when it can take them without asking (Windows asks "permanently delete?" when the bin is off, too
   small, or on a network/USB drive: checked first via mo_space `Bin::refuses_quietly`, registry BitBucket settings).
   Age is by the time in the note's name, counted to the newest note (a clock jump can't age them all), and
   `tidy-notes.txt` in the data folder records how far the sweep went, so a note put back from the bin stays.
   A Mac keeps its notes for now (Finder can ask).
2. Items the app put in the Recycle Bin: the app never empties them itself; "leave it to me with a warning". The
   warning (what's in the bin) was given to the "Remind about old copies and half moves" thread (Space page).
3. "(old copy)" folders after Move folder: the Space page should always remind him about them (with size).
4. A folder move cut off by a crash: yes, leave a marker so the app can clear its own half copy.
5. Backup drive "Older versions": keep for ever, but give an option to remove them.
6. Files deleted from his music that stay on the backup: keep them, but give a warning.
Threads started for 3+4 ("Remind about old copies and half moves") and 5 ("Add removal option for backup versions").

How the sweep decides a note may go (Windows checks, the "Stopped:" line, tidy-notes.txt), and what is still
untested on Windows: [[cleanup-notes-sweep-decisions]]. How the F12 draft keeps typed words: [[f12-note-draft-safety]].

See [[rust-rebuild-rules]], [[space-backend]], [[music-backup-plan]].
