---
name: sounds-backend-prs
description: One line per merged Sounds backend PR (#39-#196): which module each added and what its twin check showed
metadata:
  type: reference
  modified: 2026-11-03T22:44:51.019Z
---

All merged; reviews done before each merge.
- #39 mo-sounds: SoundsDb read-only (all, activity, history, map, vectors, tag_counts, hidden, matches, known_files),
  `types` (SoundTypes.cs), `music` (key and note names, Camelot).
- #46 mo-sounds::scan + mo-library scan::find_files. #51 mo-audio: WAV/AIFF, write_wav (.part then rename),
  symphonia for MP3/FLAC/M4A/OGG/AAC. #55 mo_audio::play::Player on cpal (reopen on loss, fades).
- #57 mo_audio::dsp + mo_sounds::analyse (twin: 0 differences on 13 sounds). #58 classify + dotnet_random (twin green).
- #81 learn (pure, returns changes). #87 sample_usage, usage, liking (for the screens thread). #106 keys from names.
- #115 index (VectorIndex, similar; twin green). #117 rediscover. #118 kits (copy, check, sync, rename).
- #121 duplicates (reads C#'s finds). #123/#126 galaxy (repeatable on any core count; twin compares quality).
- #154 WMA on Windows via Media Foundation. #160 read in rowid order, sort in Rust (802 s fix).
- #183 one pass for the list and both fingerprints, one connection (Everything, Everything::analysed).
- #196 (mo-library samples + mo-app sound_library) sample finder reads sounds.db lazily, `SELECT id, path` from the
  path index (46 MB not 1.9 GB), sizes by path on demand, kept until sounds.db/-wal stamp changes (failed read: 60 s).
- #219 (merged 2026-11-04, 6442530) compact sound library part 1: sounds-rust.db beside sounds.db, compact::in_step/sync.
- #236 (merged 2026-11-04, 8e0f1f3) compact sound library part 2: the Sounds page reads sounds-rust.db, brings it up to date
  after; two max reviews fixed: see [[compact-sound-library-sync]].

See [[sounds-backend-status]].
