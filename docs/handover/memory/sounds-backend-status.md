---
name: sounds-backend-status
description: Phase 3 Sounds page backend thread: mo-sounds crate state, PRs, scope limits, next pieces
metadata:
  type: project
  modified: 2026-10-31T10:35:24.581Z
---

- 2026-10-31: Sounds backend runs in its own thread (Solomon asked). Scope: backend only (mo-sounds, later mo-audio);
  stay out of studio.slint/songs.slint (Songs thread) and mo-harmony (harmony thread). Coordinate with the main
  thread before shared schema changes.
- PR #39 (MERGED 2026-10-31): new crate `mo-sounds`: SoundsDb opens sounds.db read-only (all/activity/history/map/vectors/tag_counts/
  hidden/matches/known_files), `types` = SoundTypes.cs port, `music` = key/note names + Camelot. High review done.
- Rule 23 note for when Rust writes sounds.db: C# stores the features vector twice (analysis JSON + features column).
- PR #46 (MERGED 2026-10-31): mo-sounds::scan + mo-library scan::find_files (generic walk; MIDI indexer can use it).
- PR #51 (MERGED 2026-10-31): new crate mo-audio: WAV/AIFF port + write_wav (.part then rename) + symphonia 0.6 for
  MP3/FLAC/M4A/OGG/AAC; deny.toml +MPL-2.0; ffmpeg tone fixtures; DIFFERENCES.md. Two high reviews done.
- PR #55 (MERGED 2026-10-31): mo_audio::play::Player on cpal 0.18 (output choice, reopen on loss, 10 ms buffers, 3 ms fades,
  Outputs trait with a test stand-in). CI installs libasound2-dev. High review done (10 points fixed).
- Not yet: streaming long files while they play (comes with the Sounds page).
- PR #57 (MERGED 2026-10-31; twin: 0 differences on 13 test sounds): mo_audio::dsp + mo_sounds::analyse (SoundAnalyzer port) + `twin sounds` in CI (C# helper --sounds).
- PR #58 (MERGED 2026-10-31; twin green): mo_sounds::classify (SoundClassifier port: train/predict, model JSON at sounds.db setting "type_model", read-only), dotnet_random (.NET seeded Random port), classifier cases in `twin sounds` (helper --classify, both directions). 
- PR #81 (MERGED 2026-11-01): mo_sounds::learn (update_types/resolve_type/disagreements/learn_tags; pure, return changes, no DB writes; CLAP zero-shot not yet, stored "model" types kept). Next: Rediscover, kits, duplicates, galaxy.
- PR #87 (MERGED 2026-11-01, asked by the "Build the app screens" thread, which owns all page UI): mo_library::Library::sample_usage + UsageRules (C# SampleUsageByAge), mo_sounds::usage (ApplyUsage) and mo_sounds::liking (LikedWeights, setting "liked_weights" in library.db). Expose plain functions/structs for the screens thread.
  Key/chord detection from audio: Solomon chose (2026-10-31) keys from names/root note only until the phase 6 harmony engine.
- PR #106 (MERGED 2026-11-01): key names like "C Minor"/"Bb MIN" read from file names.
- PR #115 (MERGED 2026-11-01; twin green): mo_sounds::index (VectorIndex + SoundIndex::read/similar = C# Similar), told the screens thread how to use it. 
- PR #117 (MERGED): mo_sounds::rediscover (suggest/why_nothing/years/folders); twin: C# helper --rediscover writes a sounds.db with its own SoundLibrary, Rust reads it (pattern to reuse).
- PR #118 (MERGED): mo_sounds::kits (roll/export, copy-check-sync-rename; Windows sync needs a write handle). Twin rolls kits on the same db. 
- PR #121 (MERGED): mo_sounds::duplicates::read/groups (DuplicateFinder.Groups port; reads C#'s finds via SoundsDb::duplicate_finds in one snapshot). Twin fills the finder's tables. Rust can't compute prints/matches until sounds.db writes are trusted.
- PR #123 (MERGED 2026-11-01): mo_sounds::galaxy (space/fingerprints/layout/place_new/update/out_of_date; update returns what to save, no writes). Twin: tiny libraries exact; bigger ones step by step (PCA rounding differs in last float digit, C# Vector.Sum lane order unknown, chaos after 400 epochs), settling from C#'s Init/Edges exact. Superseded by #126 (MERGED 2026-11-01, Solomon: drop C# workarounds): SplitMix64 per star per round, Jacobi-style settle (repeatable on any core count), borrowed fingerprints (Cow), place_new compares all mapped up to 50k; 40k sounds ~8 s on 4 cores; twin compares quality (neighbours kept: Rust 0.59 vs C# 0.54). All coordinator-asked pieces (Rediscover, kits, duplicates, galaxy) done; left: streaming playback, sounds.db writes once trusted.
- PR #154 (MERGED 2026-11-02, asked via coordinator: Solomon's .wma files wouldn't play): mo_audio::wma decodes WMA on Windows via Media Foundation (only unsafe in mo-audio, module-level allow); mo_audio::can_decode(path); scan lists .wma on Windows only. Mac/Linux: no WMA decoder.
- PR #160 (MERGED 2026-11-02): SoundsDb::all read 453k rows in 802 s on Solomon's PC because ORDER BY path fetched rows in random order and each row carries features/clap blobs before the later columns. Now reads in rowid order and sorts in memory with by_path (same as COLLATE NOCASE). Rule: never ORDER BY on the sounds table for whole-table reads; sort in Rust.
- No .NET in the container (install denied): the twin only runs in CI. Disk is tight: clear target/incremental.
- A decoder panic on a damaged file is caught with `mo_catch::catch` (2026-11-04): one warning line in the log, not
  a crash. Every catch goes through mo-catch (clippy.toml forbids `catch_unwind`).
- Later: shared read-only open helper with mo-library; parallel JSON parse in mo-app on the low-priority pool.

See [[rust-rebuild-status]], [[rust-rebuild-rules]].
