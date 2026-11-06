---
name: release-binary-size
description: Measured size of the app's Windows release .exe (main 469498e, 2026-11-05) and what each size setting saves
metadata:
  type: project
---

Measured 2026-11-05 on main 469498e, cross-built here for x86_64-pc-windows-gnu (mingw; cargo-xwin/MSVC blocked:
aka.ms is denied by the proxy). Linux build needs libasound2-dev (alsa-sys) which isn't installed.

- Today's release profile: 109 MB (unstripped), 71 MB stripped, 28.6 MB zipped. Only Windows system DLLs needed.
- lto="fat" + codegen-units=1 + strip=true (opt-level 3): 48.6 MB, 21.8 MB zipped; build ~10.5 min on 4 cores.
- Same with opt-level="s": 35.3 MB, 16.4 MB zipped (slower code: not tried for speed).
- panic="abort" is NOT an option: mo-catch relies on catching panics.
- Code (.text) is 49 MB of the unstripped 109; biggest: mo_ui (Slint-generated UI) ~5.7 MB, core/alloc generics,
  Slint core 2.7, naga/wgpu ~2.9, fonts embedded 1.8 MB. Beside the exe: FL script 20 KB (also embedded), Bitwig
  .bwextension (small; Maven can't fetch Bitwig's API here so not built).
Solomon said yes 2026-11-05: PR #320 merged (c9de295) adds [profile.dist] (inherits release, lto fat, cgu 1; no strip,
which on MSVC drops the .pdb crash logs need). release stays quick for the launcher's -Release and twin tests. dist
build checked on main: 49.7 MB stripped (mingw), 25 min here.

2026-11-05: launcher 8 (PR #318, merged a9238a4) adds -Release and a "Music Organiser Rust (release)" Desktop
shortcut; release profile now debug="line-tables-only" (pdb). Until then Solomon had only ever run Debug builds
(every rust-session log says "Build: Debug"). Not yet confirmed working on his PC.
