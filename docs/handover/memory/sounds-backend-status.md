---
name: sounds-backend-status
description: Sounds backend (mo-sounds, mo-audio): scope, lessons that hold for every change, what's done and what's next
metadata:
  type: project
  modified: 2026-11-03T22:44:46.184Z
---

Scope (Solomon, 2026-10-31): backend only (mo-sounds, mo-audio; mo-app/mo-library helpers when asked); page UI
belongs to the screens thread; stay out of mo-harmony. Ask the coordinator before shared schema changes.

Lessons that hold:
- Whole-table reads of `sounds`: never `ORDER BY` (random row fetches; each row carries ~4 KB of analysis and
  fingerprints before its later columns); read in rowid order and sort in Rust (#160).
- Never read sounds.db on several connections at once: no faster on Solomon's slow, nearly full C:, and it starved
  the Songs read (#183). Low disk priority (THREAD_MODE_BACKGROUND_BEGIN) rejected too: starvation risk.
- A covering index read is the cheap way to get paths: `SELECT id, path FROM sounds` reads the 46 MB path index, not
  1.89 GB of rows (test library like his).
- No .NET in the container: the twin only runs in CI. Keep cloud disk free ([[cloud-build-disk]]).
- A decoder panic is caught with `mo_catch::catch` (#242, 2026-11-04): one `Warning [Caught]` line in the session log, not a crash.
- C# stores the features vector twice (analysis JSON + features column): store once when Rust writes.

Done: everything the coordinator asked for (read, scan, analyse, classify, learn, usage, index, Rediscover, kits,
duplicates, galaxy, WMA, the one-pass read #183 merged 2026-11-03). PR list: [[sounds-backend-prs]].

Next ("Speed up loading Sounds" thread, 2026-11-03): #196 (sample finder) merged 2026-11-03 about 23:20 UTC. Now the compact Rust
sound library ([[compact-sound-library]], [[compact-sound-library-sync]], [[small-database-files]]): part 1 mo-sounds
(format, build/refresh from the old file, same reads) = PR #219, merged 2026-11-04; part 2 the page/galaxy/History
switch = PR #236, merged 2026-11-04 (8e0f1f3). Could follow: mo-app sound_library (sample finder) still reads
sounds.db's path index (46 MB, cheap). Later: streaming playback of long files, sounds.db
writes once trusted.

See [[rust-rebuild-status]], [[rust-rebuild-rules]], [[solomon-pc-disks]].
