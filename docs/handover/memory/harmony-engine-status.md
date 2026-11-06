---
name: harmony-engine-status
description: Phase 6 harmony engine: naming #200, review fixes #230 (v3), first fit #257 (v4), calibration fit #330 (v5); naming answers, fingerprints, next step
metadata:
  type: project
  modified: 2026-11-04T06:28:40.895Z
---

FULL HAND-OVER (2026-11-02): /mnt/project-files/harmony/handoff-2026-11-02.md. Plan in [[harmony-engine-plan]];
design + as-built notes: docs/harmony/chord-reader-design.md (repo; the /mnt/project-files/harmony/design/ copy is old).

- Merged: key finder #60-#85, weights #96 (KEY_FINDER_VERSION 5 via #185, see [[key-finder-refit]]); chord reader
  #107-#158; #162 `theorybench chordfit`; #177 gates (`--hand`, `--hold`); #200 NAMING (d0b7262, CHORD_READER_VERSION
  2); #230 REVIEW FIXES (891c2da, version 3).
- FIRST FIT (2026-11-04): #257 MERGED as b1e1754 (thread "Ship the chord reader's first fit", done). `--hand 1
  --hold 3` re-run on main a014537 (65 steps, no gate breaks; ~2 h on 4 cores). Shipped PULLED BACK: FITTED = HAND.toward(&FIRST_FIT, 0.2) (FIRST_FIT = the fit's numbers to the bit).
  0.2 = last 0.05 step where every gate (held-out too) + every answer holds with hand-set THRESHOLDS; at 0.25 no-bass
  keys 13/20 read one chord (0.803); full fit breaks held-out 15a (read as D over C). CHORD_READER_VERSION 4,
  SHIPPED_HASH 0xeb70_d957_2341_a787 (HASH_VERSION 4), READINGS v4 0xcb78_8296_e472_6c11. Named: Billboard B 79.2->84.0,
  POP909-CL+tune 68.3->71.8, sweep 70.3->72.5; only RS200 segmentation dips 0.1-0.2. Targets met 5 (36 39 41 46 56).
  Fit log, JSONs and bench outputs: /mnt/project-files/harmony/fits-2026-11-04/.
- CALIBRATION FIT (2026-11-05): #330 MERGED as 046a1e0, CHORD_READER_VERSION 5, HASH_VERSION 5
  SHIPPED_HASH 0x71d8_32ab_cada_d817, READINGS v5 0x9fdb_0822_fc5b_5762. `theorybench chordcal` fits sigma (8 numbers:
  tent over log2 bars + texture offsets; NLL 0.7794->0.7779, sigma 0.82-1.09) and thresholds (error <=10%, judged by the
  error itself). Shipped one [0.8,0.8,0.83,1.0], same never [1.0;4]; full mix kept at 0.8 (fitted 0.58 was 13.2% wrong
  on Billboard B). sigma never changes what is read (fingerprint unchanged). Outputs: /mnt/project-files/harmony/calibration-2026-11-05/.
  Decision card asked Solomon 0.80 vs 0.58 for full mix (recommended 0.80, unanswered at push).
- NEXT: move the weights further toward the fit, refitting the calibration at each pull-back step.
- Bench since #257: `chords` with no `--weights` = FITTED; `--weights` a file `{"numbers": {}}` = HAND (a file fills
  left-out numbers from HAND and says so). chordfit still starts from HAND with no `--from`.
- NAMING (Solomon 2026-11-03, final): altered notes (b9 #9 #11 b5 b13 #5) take NO brackets (C13#11, C7#9#5,
  C7b9#11); every OTHER note keeps brackets (Cm(maj7), Cm(add9), C7(no3), Csus4(add9)); no 7alt; NO slash chords
  (D/C is D7, F/G is G9sus4, Am/G is Am7, C/E is C). Reading.chord = chord read; Reading.written = as written.
  Bench scores by Chord::slash_name.
- Decode fingerprint (`show_the_fingerprints`, ignored, release) reads HAND: "300 pieces, all: 653f02ca94d8f328"
  (unchanged by #257). A speed-only change must keep it. Decoder/namer unit tests use HAND on purpose.
- CI: Rust 1.99, CARGO_INCREMENTAL=0, `cargo +1.99 clippy -p <crate> --all-targets -- -D warnings`. mo-app/mo-ui
  can't build in the cloud box (no ALSA): CI checks them. Force-push refused: after a merge
  `checkout -B <branch> origin/main && merge -s ours <old tip>`. PR CI took ~35 min for #230.
- Bench scores the C# app by the NAME it shows (7alt never right). C# weak: passing notes as tensions, walking
  bass, pedals, inversions.
