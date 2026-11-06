---
name: backup-older-and-gone
description: Backup keeps Older versions for ever (remove on request, #175 merged) and files deleted from music (warning strip, #178 merged); verified facts and follow-ups
metadata:
  type: project
  modified: 2026-11-03T18:05:00.000Z
---

Solomon 2026-11-03: answer 5 "keep for ever but give option to remove" (backup drive's Older versions); answer 6
"keep it but give a warning" (files deleted from his music stay on the backup). Thread "Add removal option for backup
versions". Follows [[music-backup-plan]] (that file is over 4 KB; new backup facts go here).

- #175 merged 2cdb272 (2026-11-03): `mo_backup::older` (read, remove, freed, Why), `mo_app::backup_older`, Backup page
  Older versions card, "Remove older versions…" MenuButton, question, red "Remove them" / Cancel. `MoreButton` now
  inherits `MenuButton` (`menu-at-right`). `Ran.kept_older`: card re-read after a run only when it kept some.
- Verified (std source, 1.90): `fs::remove_file` on Windows removes read-only files by POSIX delete with
  IGNORE_READONLY (NTFS only; mark left alone, so other hard-link names keep it). FAT/exFAT need the mark cleared:
  `remove_version` does that only for unshared files. Freed room counts a hard-linked file once (nlink / winapi-util).
- Review point left on purpose (#175): if every file in Older versions went some other way, empty run folders stay.
- #178 merged 58e0a58 (2026-11-03, answer 6; PR CI green on all 3 OSes): song zips keep files gone from your folders, copied RAW
  (`ZipWriter::raw_copy_file` from `by_index_raw`, never re-compressed); a kept file that reads back wrong is let through
  only when its raw bytes' SHA-256 equals the old zip's (damaged before, kept as it was, `Damaged { kept: true }`); a list
  line whose entry is missing is dropped (`kept: false`). A zip whose list can't be read or isn't the app's is NEVER
  packed over (`Unlisted::{CantRead, NotOurs}`); `pack_names(songs, &owners(&lists(..)))` skips names other songs' or
  unknown zips have (fixes the deleted-song same-title clash). `mo_backup::gone::find(backup, present, missing,
  &mut Holdings, stop)`: Holdings caches record + zip lists by (len, mtime); unsure = under a missing listed folder, on
  a drive with a missing folder and no present file (letter swap), behind links, drive not there; Unix volume roots
  (/Volumes/x, /media/u/x, /run/media/u/x, /mnt/x) count only when mounted (st_dev differs). Newest version named.
  `daw_made` needs a DAW project file (`daw_of`). UI: no gone look while a run writes (`drive_busy`), put_gone only on
  change, rows are `ListLine` (click = Show on the backup). Run problems now logged (first 50).
- Follow-up noticed, not done: `copy::put_in_place` clears a shared dest's read-only mark (edge case). zip64 raw copy
  of a real >4 GiB entry untested here (disk); a small large-form entry is tested.

See [[rust-rebuild-rules]], [[cloud-container-tools]].
