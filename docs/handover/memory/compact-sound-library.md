---
name: compact-sound-library
description: Design of the Rust sound library sounds-rust.db (the app's own since 2026-11-05): format 2 tables, exact encodings, page size, laying out format 1 anew, verified SQLite facts
metadata:
  type: project
  modified: 2026-11-05T11:07:16.656Z
---

Solomon chose "Yes, beside the old one" (2026-11-03, see [[small-database-files]]). Since 2026-11-05 it's the app's
own library (own_since, see [[new-rust-databases]]); sounds.db only read to make it.

- File: `sounds-rust.db` beside `sounds.db`. Ids are the old file's ids, so tags/events/maps/matches/hidden keep
  meaning the same sounds. 32 KB pages, WAL.
- FORMAT 2 (step 5a, 2026-11-05): `sounds` rows = C# columns + analysis as columns (no waveform) + `clap_numbers`
  (count of CLAP floats; NULL none, 0 not whole floats) with partial index `sounds_clap WHERE clap_numbers IS NOT
  NULL` (heard-count reads the index only); `sound_waves(sound_id, v)` packed waveform; `sound_features` raw;
  `sound_clap` = index::held = unit() then to_half, 2 bytes LE (bit-identical CLAP index: CLAP isn't standardised or
  weighted); whole C# tables + sound_prints(exact). No sound_hashes, no `hash:` keys. A copy behind sounds.db is made
  afresh (keep old until new is whole, let_go_of = rw open+read+close, then -wal/-shm must be gone).
- FORMAT 1 (FORMERLY): waveform last analysis column, raw CLAP, sound_hashes(row,features,clap,clap_len bytes).
  Read as is by SoundsDb (Layout::Formerly); `compact::lay_out` (page calls it after fingerprints, db dropped, card)
  copies it to .part-<pid> via ATTACH (rw), row-by-row bit compare + integrity_check, sync, rename only if no
  -wal/-shm (LAYING_OUT held only around check+rename; SoundsDb::open_read_only holds it through its first read).
  Unknown table -> left as is. Damaged -> page remakes from sounds.db if there. Verified against a copy made by the
  real format-1 code (1120fd8): identical to a fresh format-2 copy.
- Encodings: waveform and chroma packed as thousandths (10 bits, only when they decode to the very same value, else
  raw floats); chords packed binary (JSON floats aren't exact); character JSON.

Verified facts:
- A REAL column turns -0.0 into 0; a column with no type keeps -0.0 (C# Math.Round gives -0.0), so analysis columns
  have no declared type. quote() can't show -0.0: compare in Rust with to_bits.
- 2 KB CLAP rows waste half a 4 KB page; 32768 pages chosen.
- SQLite skips writing an UPDATE that sets identical values (file mtime unchanged): tests must really change a value.
- Bundled SQLite has foreign keys ON by default: copy connections set foreign_keys = OFF.
- sound_prints has ix_sound_prints_exact, so exact hashes read from the index alone.

See [[sounds-backend-status]], [[solomon-pc-disks]], [[compact-sound-library-sync]] (format 1 history).
