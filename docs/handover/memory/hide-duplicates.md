---
name: hide-duplicates
description: Sounds page "Hide duplicates" switch (PR #344): strict = same decoded audio bit for bit, sound-copies.db cache, how it's wired, Solomon's ask
metadata:
  type: project
  modified: 2026-11-05T21:47:11.499Z
---

Solomon 2026-11-05 20:47: no separate duplicates page; a Hide duplicates toggle on Sounds, "really strict so real
duplicates get hidden not similar sounding". Thread "Hide duplicate sounds on Sounds", branch
claude/project-thread-j68n0z, PR #344 MERGED 2026-11-05 22:12 (5738066).

- Strict: same decoded audio (rate, channels, frames, f32 bits) via SHA-256; WAV/AIFF only when
  `mo_audio::reads_exactly` (not 32-bit int / 64-bit float / fractional AIFF rate), else byte print; non-WAV decoded
  whole up to 120 s, longer -> bytes.
- Candidates only: analysis key (seconds ms, rate, channels, peak 0.1 dB) shared with another sound.
- Cache `sound-copies.db` beside the library (app id MOSC, layout 1), path+size+mtime; pruned to candidates.
  Foreign/locked file left alone; own damaged/old layout remade.
- mo-app SoundList copies (Arc, kept by sharing/keep_copies_of); keep-first = most used, fav, plays, tags,
  older (stars dropped 10 Nov, Heart only). mo-ui sounds/copies.rs: look only while on, bulk pool, ONE_LOOK mutex.
  Setting `sounds_hide_duplicates`.
- Space's copies line still reads the old C# finds; not switched to this finder (could be a follow-up).

See [[space-sound-copies]], [[sounds-backend-status]].
