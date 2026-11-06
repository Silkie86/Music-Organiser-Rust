---
name: remember-work-between-starts
description: Plan section E thread (2026-11-05): MIDI on bulk pool #298, Bitwig prints store #300, preset-files.db #304, arrangement-peaks.db; store rules used
metadata:
  type: project
  modified: 2026-11-05T05:31:16.689Z
---

Thread "Remember work between app starts" (plan doc section E, see [[app-feel-instant]]). One PR per job, all from
branch claude/project-thread-chck3o (reset to main after each merge).

- #298 MERGED: MIDI folder walk + reading on `background::bulk_pool()` (new accessor, up to 4 threads); log line
  "[MIDI] read with N readers in X s".
- #300 MERGED: `mo_space::prints` -> `bitwig-prints.db` (path, size, ns mtime, ends print = first+last 64 KB, whole
  print). Whole look (Library found AND every root is_dir) forgets unasked rows; crashed look calls
  `prints::forget`. `remove_copies` never uses the store (always reads fresh before binning).
- #304 MERGED: `mo_presets::file_store` -> `preset-files.db`; bwpreset read+decoded once (`files::bwpreset_of` +
  `bitwig::inspect_doc`); io errors not kept. Bump `file_store::VERSION` when preset readers change (rule 7).
- #307 MERGED (84f02ea): `mo_app::arrangement::peaks_store` -> `arrangement-peaks.db` (u8 peaks, 64 MB LRU cap, incremental vacuum,
  journal_size_limit 4 MB, reader string "mo-audio, 60 s" in meta); decode on bulk pool.
All of section E done 2026-11-05. Solomon to check the new log lines ([MIDI] readers, [Space] prints kept, [Presets] files read, [Arrangement] waveforms kept) on his PC.

Store rules (agreed with "Build the new Rust databases" thread): application_id + user_version set in the SAME
transaction as the schema; remake only on SQLite NotADatabase/DatabaseCorrupt or a row that doesn't read back (no
code), never on busy/IO; foreign app id -> left alone; WAL, synchronous NORMAL, temp_store MEMORY, no mmap. Kept three
files (not one caches.db) so each crate owns and can remake its own file. Databases thread may move them to a shared
helper later.
