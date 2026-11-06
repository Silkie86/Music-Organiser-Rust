---
name: cloud-build-disk
description: Cloud session disk allowance (~39 GB) fills up with the workspace's debug build; how to keep cargo test --workspace from failing
metadata:
  type: reference
  modified: 2026-11-04T17:41:01.233Z
---

2026-11-03 (Speed up loading Sounds thread): a cloud session's writable disk is about 39 GB in all. The workspace's
`target/debug` reached 23 GB (12 GB deps, 9 GB `incremental`), and with a 5 GB test library in the scratchpad the
disk filled: `cargo test --workspace` then fails at linking and tool output is lost.

- Delete `target/debug/incremental` (safe: only rebuilds take longer) and build with `CARGO_INCREMENTAL=0` (CI does
  not use incremental builds either).
- Keep big test files (synthetic `sounds.db`, 5.1 GB) only while timing; `gen_sounds_db.py` in the thread's
  scratchpad remade it.
- A full `cargo test --workspace` build takes over 10 minutes on the 4-core VM: run it in the background with a
  timeout of an hour, not the 10-minute default.
- Never share one `CARGO_TARGET_DIR` between two checkouts of this workspace (a git worktree and the main clone):
  cargo hashes path crates by their path inside the workspace, so both write the same artifacts, and one checkout's
  build can link the other's code without rebuilding (a timing of "old" code ran the new code). After it happens,
  `cargo clean -p <crate>` in the checkout you use. Hit again 2026-11-04: wrong-change checks run in a worktree with
  the main `target/` left a wrong-change test binary that the main clone's `cargo test --workspace` ran as fresh.
  Run wrong changes in place (commit first, `git checkout -- file` after each), or clean every workspace crate after.
- Old test binaries pile up in `target/debug/deps` (5.5 GB of them by 2026-11-04): delete files there with no
  extension older than a few hours (outputs only, nothing links them).

- Two toolchains here: default `stable` is rustc 1.97, CI's clippy is `+1.99`; their builds don't share files, so
  stick to one (`cargo +1.99` for build, clippy and tests). Deleting deps rlibs by age (`-mmin +180`) can remove
  libraries still in use: cargo then rebuilds the whole tree. To free one toolchain's files, map each
  `.fingerprint/<crate>-<hash>/*.json` "rustc" value to `deps/*-<hash>.*` (freed 3.75 GB, 4 Nov).

See [[solomon-pc-disks]] for why cloud timings mislead anyway.

2026-11-04 (MIDI thread): tools/galaxycheck/target is ~8 GB on its own; deleting it plus target/debug/incremental and
big deps files older than 30 min freed ~15 GB. A test holding FakeBitwig::world()'s guard while calling world() again
deadlocks (looks like a 30-minute "hang"): drop the guard first.
