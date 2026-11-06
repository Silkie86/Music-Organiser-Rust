---
name: compact-sound-library-sync
description: Compact sound library: part 1 #219 and part 2 #236 (the page reads sounds-rust.db) both MERGED 2026-11-04; how sync/in_step, damage, disk room and refresh work
metadata:
  type: project
  modified: 2026-11-04T05:58:00.000Z
---

Part 1 = PR #219, MERGED 2026-11-04 (6442530). Part 2 = PR #236, MERGED 2026-11-04 ~06:20 UTC (squash 8e0f1f3).
Built xhigh; review 1 (max, 9 points: 8 fixed, point 6 = first-start hand-over after indexes/fsync, declined) and
review 2 (max, 14 points, all fixed). Solomon's home check: first start makes the copy (slow once), later fast.
81d9755: galaxy::places_moved (called from move_places) remaps the sweep's played/waiting, drops held if gone (#250).
Solomon's F12 note 2026-11-04 04:12: Sounds read took 725 s on his PC (old code, competing with Songs, MIDI, Bitwig
copies jobs on full C:). That's what #236 fixes; FIRST start after merge still slow (makes the copy).

How it works (mo_sounds::compact):
- `SoundsDb::stamp` = db len@mtime(ns), -wal len@mtime ("-" if empty), -shm bytes 16..40. TEST TRAP: Linux file
  times move by a coarse tick (~4 ms): a save right after a sync, with the C# connection closed (no -shm), keeps the
  same stamp, so in_step stays true. Tests that need "behind" must let time pass; sync() itself compares row hashes.
- read_for_page: copy opens + all() ok -> PageRead::Copy{behind}; all() damaged -> remake_reading; else sync_reading
  (copy made on the page's own pass). Page (mo-ui read_all): fingerprints from copy; damaged -> remake_reading
  (made Err -> Step::Source(sounds.db) for galaxy/History); behind -> bring_up_to_date card, then Refreshed{moved,
  retyped} only if Synced::changed_the_list(); Index again only if Synced.prints > 0 or analysed count changed.
- Damage = SQLite CORRUPT/NOTADB or IOERR_READ/SHORT_READ (db::is_damage). Update stopping on damage ->
  copy_damaged() runs PRAGMA quick_check(1) on the copy: damaged -> make afresh; ok -> sounds.db is the damaged one,
  copy kept (removing it would leave the page nothing).
- Disk room: make checks library/2 + ROOM_LEFT (1 GiB) first; both make and update watch the disk while writing
  (compact::Room, every 256 sounds and between tables) and stop below 1 GiB free. No up-front "copy size" check for
  updates (it blocked one-play updates on a 1-2.6 GB-free disk).
- Refresh (mo-ui take_refreshed, testable): sets list/places/types/played (main #234 split History into `played`
  and filtered `history`), move_places remaps shown/history/rediscover/to_pick/picked/playing; galaxy::forget when
  moved or retyped, else galaxy::list_changed (drops light, set_for, in-flight packs). After a refresh Sounds like
  this is asked again (answers carry their list; alike_found drops another list's), Rediscover again only if its
  answer was on its way and sounds moved.
- LESSON: rusqlite's bundled SQLite has foreign keys ON; the copy's write connection sets foreign_keys = OFF.
- Ratings 0 and 1 take no record bytes (serial types 8/9): size-stable test saves use 2 <-> 3.

Numbers (453,885-sound test library, cloud): 5.14 GB -> 1.51 GB; list 3.9 s vs 10.7 s cold; list+prints
6.2-6.4 s vs 11.5-13.3 s; first start with make 28-33 s; update after one play 9-10 s; in_step < 1 ms.

See [[compact-sound-library]], [[solomon-pc-disks]], [[sounds-backend-status]], [[cloud-build-disk]].
