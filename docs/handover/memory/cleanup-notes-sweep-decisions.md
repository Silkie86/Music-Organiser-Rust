---
name: cleanup-notes-sweep-decisions
description: How the start-up sweep decides an old F12 note may go to the Recycle Bin (Windows checks, the Stopped line, tidy-notes.txt, tidy-notes.lock), as settled in the 3rd-5th reviews of the clean-up PR
metadata:
  type: project
  modified: 2026-11-03T19:02:50.942Z
---

Decided in the max-effort reviews of the clean-up PR (2026-11-03), in mo_app::tidy and mo_space::clean:
- Windows: notes go only from a DRIVE_FIXED drive whose bin is on (policies NoRecycleFiles / RecycleBinSize in HKLM
  and HKCU; per-volume NukeOnDelete / MaxCapacity under HKCU ...\Explorer\BitBucket\Volume\{GUID}) and that has room:
  held (SHQueryRecycleBinW, read at most every 5 s) + all old notes' bytes <= 90% of the max (policy % of the drive,
  else MaxCapacity MB, else drive/20). The sum is `bin_decides`, a plain function tested on every OS.
- trash is pinned `=5.2.9` (mo-space Cargo.toml). On Windows its `Error::Unknown` only ever means "Some operations
  were aborted" (a "no", or an error skipped silently under FOF_NOERRORUI): matched by variant, not text, as
  KEPT_BY_YOU. HRESULTs are compared whole: cancel codes (0x800704C7, 0x80004004, 0x80270000/1) and the copy
  engine's Recycle Bin codes (0x80270035-3A) are KEPT_BY_YOU too; 0x80070005 NEEDS_ADMIN (0x80070078 is "not
  implemented", not access denied); 0x80070020 and 0x80270027 IN_USE. Only for `Unknown` (Windows didn't say why)
  does `problem_with` call `why_kept` (DELETE access, full sharing: error 32 = IN_USE, 5 = NEEDS_ADMIN: the note
  stays for good, the rest wait for the next start). Otherwise a "Stopped:" line goes in tidy-notes.txt and the
  session log warns. Also a stop: STILL_THERE (trash said Ok but the file is still there: maybe deleted for good
  while held open), the canary (first file a start sends isn't in the bin after), or a note gone after any failed
  send (with the lock it can't be another copy). A note gone before its send is passed over quietly.
- tidy-notes.lock (data folder, empty, stays for good: removing a lock file lets two holders exist) is held with
  File::try_lock while one copy goes through the notes; another copy leaves them ("another copy of the app is going
  through them now"). Made with create_new; an existing one opened (write access, for NFS) only if a plain file; on
  Windows opened without FILE_SHARE_DELETE so nobody can remove it while held.
- tidy-notes.txt: first line = time up to which notes were looked at. Written whole (.mo-part, sync_all, rename);
  re-read just before each note is written down (after refuses_quietly) and at the end (never writes an earlier
  time back). A note's own refusal (ONLINE_ONLY, CANT_LOOK) marks it and goes on; the bin's reasons return. The stop
  write is tried 3 times, 200 ms apart, else the log says notes may go again. A time after now: nothing goes.
- Accepted (safe side): same-second "-2" notes can be skipped and stay; Storage Sense can empty the bin after 30
  days (Windows' own setting: told Solomon in the PR, no code); sync_all has no test (the retry, `tried`, does).
- Untested on Windows (needs Solomon's PC): the registry/room check, which HRESULTs IFileOperation really returns,
  the session log opened without FILE_SHARE_DELETE, and whether "the Recycle Bin is corrupted" is auto-answered
  under FOF_NO_UI. CI's Windows runner does run why_kept's and the lock's tests.

See [[app-cleanup-rule-28]], [[space-backend]].
