---
name: harmony-engine-status
description: Phase 6 harmony engine: what is merged, the hand-over of 2026-11-02 (next PR plan, key refit on Solomon's PC, open naming answers)
metadata:
  type: project
  modified: 2026-11-02T05:02:29.474Z
---

Thread "Build the harmony engine" WRAPPED 2026-11-02 ~05:10; Solomon hands the work to his other cloud session.
FULL HAND-OVER (plan, resume steps, RC brief word for word): /mnt/project-files/harmony/handoff-2026-11-02.md.
Plan in [[harmony-engine-plan]]; design + as-built notes: /mnt/project-files/harmony/design/chord-reader-design.md.

- Merged: part 1 #45; key finder #60-#85, weights #96 (KEY_FINDER_VERSION 4, its `--final` used up); chord reader
  #107-#158 (CHORD_READER_VERSION 1, SHIPPED = HAND weights); #162 `theorybench chordfit` + ChordFitter (squash).
- First chord fit NOT shipped: gains roots/kinds/inversions on every set, but 27 hand-case gates break and extensions
  fall (loss sees root+core only). README "The chord reader's first fit" (numbers predate the review fixes).
- NEXT PR (not started in code): public `chordread::cases` like keyfind::cases (Writer with RefCell slips so case
  text stays; Book::read(&Model) verdicts by Step; fit_cases() labels from Check::Reads), chordfit prints broken
  gates, then the hand cases as a separate fit term mu * mean(-ln P) (`--hand`). Fingerprint must stay
  "300 pieces, all: 57bda66c6f357eee". Then C12 mix, lambda/sigma/thresholds, speed step 2, mo-app, Chord check,
  ultracode whole-app review (rule 19).
- Key refit on Solomon's PC (rule 5 exception, numbers only): RC session ENDED 05:00 for the hand-over; it was
  waiting for where his MIDI packs are; nothing ran on them. Restart RC with the brief in the hand-over doc, then
  cloud PR: weights.rs, KEY_FINDER_VERSION 5, hand_case_tests.rs, test-runs.md line, public sets --shipped
  before/after.
- WAITING ON SOLOMON: (1) MIDI pack folders; (2) naming picks (C# names stay until yes): C13(#11)/C9(#11) not (b5)
  when a 9 or 13 sounds, C7(b5) kept for the bare shell; Eb/E not Eb/Fb; C7(#9,#5) and C7(b9,#11) instead of
  C7alt. Then a chord.rs PR with a version bump (rule 7).
- Bench data, renders, C# answers, fits were in scratch only: rebuild from tools/theorybench/README.md.
- CI: Rust 1.99, CARGO_INCREMENTAL=0, `cargo +1.99 clippy -p <crate> --all-targets -- -D warnings`. Force-push
  refused: after a merge `checkout -B <branch> origin/main && merge -s ours`. Merge needs the full SHA.
- Decode fingerprint: `show_the_fingerprints` (ignored) with DUMP=path to compare number by number.
- Bench scores the C# app by the NAME it shows (7alt never right). C# weak: passing notes as tensions, walking
  bass, pedals, inversions.
