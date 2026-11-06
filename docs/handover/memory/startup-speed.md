---
name: startup-speed
description: "Speed up the app and its startup" thread (2026-11-04): why library.db list reads were slow, saves-list copy, graphics card once, [Start] log line, PR #266, next steps
metadata:
  type: project
  modified: 2026-11-05T03:15:00.000Z
---

Solomon 2026-11-04 07:45: efficiency push, startup first. 08:42: "Always look for the most efficient way to do things".

**Findings (his log 04:29, build 6009edd, Debug):** window first drew ~7 s after start (time to the RenderingSetup
line); Songs "library read in 47.4 s"; Sounds 673 s (pre-#236; the compact copy now, Sounds thread owns it).
- library.db `projects.info` (track details JSON, tens of KB) sits mid-row (C# schema); SQLite must walk its overflow
  pages to reach later columns, so `Library::saves()` read ~all of it: 718 MB of a 720 MB synthetic copy vs 51 MB
  without. Correlated subqueries (plugin/sample counts) add ~2 s CPU on 16k rows.
- Slint Automatic made 2 wgpu instances + ours = 3, all backends incl. GL (hidden window on Windows); debug builds had
  InstanceFlags::debugging() (wgpu from_build_config).
- Presets Sounds tab re-reads every song's newest save (~3,077 project files) on every start, no store. MIDI and Curves
  already only read changed files.

**PR #266 (branch claude/project-thread-sd0hin):** `mo_library::list_copy` (library-list-rust.db, stamp = size+ns
mtime of db and -wal + WAL header hex; keep() from main only; one read at a time, pages asking together share it;
tidy clears `.new-<n>`). gpu::look_ahead thread + WGPUConfiguration::Manual, PRIMARY backends, VALIDATION_INDIRECT_CALL
only; Automatic fallback when none or WGPU_ADAPTER_NAME. `mo_ui::startup` writes one `[Start]` line with step times.

**Bench here:** scratchpad start.sh (xvfb, XDG_DATA_HOME, synthetic library from mklib.py + C# schema.sql), lavapipe:
`SLINT_WGPU_CPU=1 VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`. Cloud window ready ~0.3-0.8 s; his PC ~10x.

**#266 merged 09:08 (44ab80e). #268 merged 09:33 (8ddf64d):** `mo_presets::sound_store` (preset-sounds.db; ns mtime+size per save, one version row = page VERSION + fl/bitwig VERSION + Thresholds Debug; away/failed saves keep old rows; damaged store remade). Bump sound_store::VERSION when list/role/octave guessing changes (rule 7). Not done: shared stamp helper with curves.db, duplicated PluginKind number map.

**DB audit done:** MIDI store/index, curves, compact sounds already fine (data blob last, indexes). MIDI list has ORDER BY over 348k rows (small).

**Round 2, PR #292 MERGED 2026-11-05 03:29 (266ea61):** `Library::saves_and_source` (Songs line says copy or why not), gpu.rs times each interface (Windows: Vulkan then DX12; WGPU_BACKEND = only that), keeps built-in card while a discrete one may come. His 15:51 log (e277a6f): graphics card 5805 ms, Songs read 104.6 s (likely first copy). Drag-to-DAW note told: #277.

**Next:** DB query + folder walk audit HANDED 2026-11-05 03:15 to "Make the app feel instant" thread; this thread keeps startup log + graphics card. Was: audit (indexes, sorts,
double reads). Read his next `[Start]` line. See [[solomon-pc-disks]], [[slint-frame-speed]], [[app-memory-use]].
