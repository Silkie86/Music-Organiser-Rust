---
name: app-memory-use
description: "Cut the app's memory use" thread (2026-11-03/04): what holds the Rust app's RAM (Sounds page), measured numbers, merged PRs #190 #201 #260, what's left
metadata:
  type: project
  modified: 2026-11-03T23:56:49.784Z
---

Solomon (2026-11-03 19:49): "the app uses a lot of memory, can it be more efficient". His PC was at 88% of 32 GB.

Measured with a counting allocator on 453,885 synthetic sounds (probe in the thread's scratchpad, not in the repo).
Before: list ~0.9 GB, features+CLAP maps ~1.1 GB, CLAP index 0.93 GB; peak 3.17 GB, held 1.82 GB.
After #190 + #201: peak 1.89 GB, held ~1.36 GB. Galaxy pictures (u8 RGBA) and star arrays are small.

Merged:
- #190: SoundList keeps sounds/hay/names in one `Arc<Shared>` (`sharing()` for the usage list; #187's big-list
  freeing moved to `Shared`'s Drop); analysis Vecs shrink_to_fit in `from_json`.
- #201: `index::Gathered` (fixed 4 MB blocks, `push_blob`) read by `SoundsDb::everything`; `VectorIndex` kept in
  parts of CHUNK=4096 rows, built part by part freeing blocks; CLAP at half precision (`half` crate, 256 KB lookup
  table), not built at all until deep (9 in 10 heard). `get` returns `Cow`; hot loops use `dot_with`. Listed in
  mo-sounds DIFFERENCES.md. Twin test green.

Solomon DECIDED (2026-11-03 20:20, card): CLAP at half precision (memory over the last likeness digit).

- #260 (2026-11-04, Solomon "go"): `Peaks` (private enum: u16 thousandths, or floats when any value isn't one;
  lossless, shares compact.rs's F32_THOUSANDTHS/thousandths) for waveforms; `mo_sounds::Word` (interned Arc<str>,
  RwLock set, never shrinks) for sound_type/type_source/user_type/name_type/bpm_source/key_source/character.
  Held 1.36 GB -> 1.17 GB, peak 1.86 -> 1.68 GB, live allocations 6.57M -> 2.94M. Twin green.

Left, not done: the gathered CLAP blocks still exist at full precision during the list build (~0.93 GB of the peak);
paths/names per sound are still own Strings.

See [[solomon-pc-disks]], [[small-database-files]], [[sounds-backend-status]].
