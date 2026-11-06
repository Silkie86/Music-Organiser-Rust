---
name: data-folder-move
description: Moving the app's data folder off C: (Solomon chose I:, 2026-11-05): PR #299 merged, how the pointer/copy/switch works, what Solomon still has to do
metadata:
  type: project
  modified: 2026-11-05T05:12:25.353Z
---

PR #299 merged 2026-11-05 (ee2a7b2). Settings > General > "Where the app keeps its data" > Move to another drive.
Picking I:\ copies to I:\MusicOrganiser in the background (SHA-256 checked, file times kept so stamps/copies stay
valid), and the switch happens at the next start (mo_app::data_move::finish_at_start, before the log starts; at most
1 GB caught up before the window opens, more = background catch-up, then the next start).

- Pointer: %LOCALAPPDATA%\MusicOrganiser-folder.txt (beside the first folder, not in it). Followed only to a folder
  holding moved-in.json (which names the old folder: paths::moved_from()). Pending: MusicOrganiser-moving.txt.
- Old C: folder is never changed (gets "MOVED - the app now uses another folder.txt"); Solomon removes it himself.
- Not copied: run-latest-rust/run-latest (launcher stays on C:), *.lock, *.mo-part. -wal/-shm ARE copied (sounds
  stamp reads -shm). Links copied through. LEFT_BEHIND is now pairs (old file, replacement) via left_behind(dir, name): PR #303 leaves sounds.db(-wal/-shm) behind when sounds-rust.db is there; library.db/midi.db to follow.
- Launcher v7 reads the pointer (Get-AppData) for notes\writing.lock.
- Every data path must go through paths::data_dir() (resolved once per run).

Read-only look done 2026-11-05 07:35: ~11.1 GB to copy (sounds.db 4.56, sounds-rust 1.52, library 0.92, midi 0.89,
curves.db 0.85, models 0.63, midi-index 0.49, logs 0.44 incl. a 438 MB twin-report), no links; keyrefit\repo is a
repo clone (Cargo.lock kept since #315). #315 merged (e79350b) 2026-11-05 07:57; Solomon told to press Move. Not confirmed run yet: check his next log ([Data folder] lines).
See [[app-feel-instant]], [[solomon-pc-disks]].

Deleting the old C: folder (told Solomon 2026-11-05 08:10): wait ~a week; not until the archive step has taken the old
library.db/sounds.db/midi.db (sounds.db may exist only on C: after #303). Keep for good: C:\...\MusicOrganiser\run-latest-rust
(launcher) and %LOCALAPPDATA%\MusicOrganiser-folder.txt (pointer). I promised to tell him when it's safe.
