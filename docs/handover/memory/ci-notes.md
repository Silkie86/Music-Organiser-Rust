---
name: ci-notes
description: GitHub Actions CI quirks for the Rust repo: main-push Mac job cancelled with no runner (re-run once), PR CI tests the merge ref
metadata:
  type: reference
  modified: 2026-11-04T15:40:00.000Z
---

- 2026-11-03: on two main pushes in a row (#176 run 41680826773, #178 run 41689498900) the `macos-latest` job never
  got a runner and was cancelled at exactly 15 minutes (no steps, no output), so the run showed "failure" while
  Windows, Linux, screenshots and twin test were green. One "Re-run failed jobs" (`actions_run_trigger`
  rerun_failed_jobs) got a Mac runner and passed (#178's run, attempt 2, full tests in ~12 min). PR runs' Mac jobs
  weren't hit. So a red main run whose only failed job is a cancelled Mac job with no runner is not a code failure:
  re-run it once before judging. #176's old run stays red, but 58e0a58's green run includes #176's code.
- PR CI checks out `refs/pull/N/merge` (the PR merged into main as main was at push time; the "What changed" job's
  checkout log says "Merge <head> into <main>"), and `tools/dev/ci_scope.py` picks the crates. To be sure a squash
  merge is what CI tested: `git merge-tree --write-tree <main> <head>` equals `<squash>^{tree}`.
- CI runs clippy from `dtolnay/rust-toolchain@stable` (1.99 on 2026-11-04) but the container's default toolchain is
  1.97 (1.99 is installed too): run `cargo +1.99 clippy -p <crate> --all-targets -- -D warnings` before pushing. #219
  failed on all three OSes from a 1.99-only lint (clippy::chunks_exact_to_as_chunks) that 1.97 doesn't know.
- The galaxy check's own `tools/galaxycheck/Cargo.lock` is often left behind when a crate gains a dependency
  (`half` #201, `indexmap` #208/#218, `known-folders` #212): building the check then rewrites it. CI doesn't use
  `--locked`, so nothing goes red; add the lines in the next PR that touches that lock, or in the whole-app review.
- Reusing a thread's branch after its PR merged (2026-11-04, #242 on #224's branch): the permission check refused
  `git push --force-with-lease` ("Git Destructive"). What worked: rebase the work on main, then
  `git merge -s ours origin/<branch>` (tree unchanged, the old merged commits kept) and a plain push. The PR's
  diff is only the new work; its commit list shows the old commits too, so say so in the body and squash-merge.
- 2026-11-04: mo-ui's `the_page_reads_the_apps_copy_and_brings_it_up_to_date` (#236) failed once in a full
  `cargo test -p mo-ui` run (line 2596, the first start's steps) with the disk 93% full; it passed alone and in two
  more full runs. Not traced: if it fails in CI, look at that test's shared fixture names first.

- 2026-11-04 ~14:45-15:30: no GitHub Mac runner for ANY run (#275 twice, main after #251, #255): each macOS job
  cancelled at 15 min with runner_id 0. #275 was merged on Solomon's "Merge all" with Windows, Linux and screenshots
  green and no Mac-only code; check main's next Mac run once runners are back.

See [[rust-rebuild-rules]], [[cloud-container-tools]], [[backup-older-and-gone]].

- 2026-11-05 CI timing (last 100 runs): PR median 16.5 min, main 21. Windows is the slowest job: on PR run
  41977522556 cache save 3.75 min, tests 9 min (8m21 compiling, 40 s running; mo-ui alone ~5.5 min), galaxy 4.7 min.
  Mac ~14 min when it gets a runner; 4 Nov afternoon it got none (cancelled at 15 min, twice on #?/run 41875869671).
  PR #310: rust-cache `save-if` main only + galaxy check as its own Windows job. Next ideas: sccache for workspace
  crates, Mac non-blocking on PRs (Solomon's choice).
- 2026-11-05 DECISION (Solomon): Mac is advice only on PRs (continue-on-error in #310, CLAUDE.md line): merge a PR
  with Mac red/cancelled once all else is green; on main Mac counts, fix a Mac failure there at once.
