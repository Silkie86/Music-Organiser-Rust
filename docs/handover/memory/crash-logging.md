---
name: crash-logging
description: How crashes and caught panics are logged: crash hook order and crash file limits (#224), mo-catch's catch vs catch_crash (#242, merged), its limits and how to pick one
metadata:
  type: project
  modified: 2026-11-04T06:23:32.145Z
---

Rule 17 (crashes always log) as built in the clean-up PR (#224, merged 2026-11-04 as 8a38df4):
- `set_crash_hook`/`on_crash` in mo-ui main.rs: session-log line, flush, crash file, keep the F12 note, backtrace,
  std's report last. Crash files: one per place, at most 10 places a run (`log::CrashFiles`), the app-ending crash
  always; `tidy::prune_logs` keeps the newest 30.
- Windows (MSVC): a panic in a `Drop` while an earlier panic unwinds ends the app at once, no third hook call. The
  hook treats it as non-fatal (open note's words kept, a note mid-save isn't).

Caught panics (#242, merged 2026-11-04 as 51bff85; max effort):
- Every catch goes through `crates/mo-catch`; clippy.toml forbids `std::panic::catch_unwind` and `resume_unwind`.
  `mo_catch::catch` = what a damaged file or a library can do: one `Warning [Caught]` line, counted by place, words
  cut at 1000 chars, no backtrace/crash file/note/std report. `mo_catch::catch_crash` = a bug a thread lives through:
  a crash in full. Pick catch_crash when unsure, and always for work that waits on rayon or a pool.
- Hook (`what_it_is` in main.rs): `log::cannot_unwind(info)` (reads "can_unwind: false" from PanicHookInfo's Debug;
  `can_unwind` unstable in 1.99; Debug shows no message text) or `log::std_ends_the_app` first (crash, fatal), then
  `mo_catch::panic_starting()` (marks the innermost frame unwinding; a later panic in a Drop is a crash), then the
  main-thread rule (catch_crash on main isn't fatal).
- Limits on work inside `catch`: never wait on other threads (par_iter, on_pool, thread::scope), never resume a
  panic, never let one reach `extern "C"`. A catch around a par_iter's per-item closure is fine (read_peaks).
- While #242 waited, main gained catches that it converted: #244 Space old-copies look and #248
  `background::guarded` (both catch_crash; guarded's backup plan waits on a pool), #249 arrangement read (catch),
  #174 half_copy test (catch_crash). #206 (drag-out, swash fonts) was told to use `catch`.
- Wrong-change checks: 11/11 first version, 5/5 review fixes. One hook wiring isn't testable on stable: passing
  `false` for cannot_unwind survives, as every known can't-unwind message is also in the words list.

See [[f12-note-draft-safety]], [[app-cleanup-rule-28]], [[ci-notes]].

Close crash (2026-11-05, PR #318): every debug-build close wrote crash-*.log "cannot access a Thread Local Storage
value during or after destruction" (wgpu-core debug-only snatch lock tracer TLS, hit when Slint's TLS context drops
the wgpu queue in ExitProcess). Harmless (after [App] Closed). Fix: [profile.dev.package.wgpu-core]
debug-assertions = false. Release never had it.
