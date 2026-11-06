---
name: library-db-writers
description: Which Rust code may write library.db and Solomon's OKs: folders.rs (2026-11-03), moves + Plugins folder fixes, export folder (2026-11-05)
metadata:
  type: project
  modified: 2026-11-05T01:27:11.115Z
---

library.db is shared with the C# app and opened read-only except by named writers in mo-library:
- `mo_library::folders` (Settings › Folders): Solomon's OK 2026-11-03.
- `mo_library::moves::add_moved_folder` (space_moved_folders, C# JSON shape; Plugins Move, Space reuses it) and the
  Plugins page's "Not its / Add a folder" fixes: Solomon chose "Yes, both" on 2026-11-05 01:23 UTC.
- The export folder (`export_folder`, Settings › Folders Exports card, screens thread): asked "The Rust Settings page
  has no export folder row. Should I add one?", Solomon 2026-11-05 20:48 UTC "Yeh there should be".
Every writer: one IMMEDIATE transaction, re-read strictly (refuse what can't be read, never treat as empty), change,
read back, commit; never makes a library or table; busy -> LibraryError::Busy. Update folders.rs's header when a new
writer is added. Any other new writer still needs Solomon's OK. See [[plugins-page]], [[space-backend]].
songs.db (the app's own, after step 2) is written by mo_library::song_writes (scans/watch reads) and the settings writers above; plan doc step 3, Solomon's C#-dead decision 2026-11-05 covers it. library.db itself is never written by song_writes.
