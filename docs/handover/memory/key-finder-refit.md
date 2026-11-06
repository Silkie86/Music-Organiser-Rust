---
name: key-finder-refit
description: Key finder refit on Solomon's packs (keyfit --rescale): version 5 shipped (#185), his choice, how to rescale again, the keycheck command
metadata:
  type: project
  modified: 2026-11-03T20:18:32.472Z
---

Refit = rule 5's exception (stream scales, minor weight, calibration only). Packs at <MIDI packs folder>. Thread "Refit
the key finder on MIDI packs" (cmsg_018WW9Fe6y3WLLS9ZVAUVttKAMVG6nmqmRLRPi3gfowfHg) runs it on his PC (RC). It is
Solomon's own session: other threads can't message it (message_thread refuses); go through the coordinator.
Plan in [[harmony-engine-status]].

- 2026-11-03 trial: 81% of whole loops minor; the plain rescale broke 12 of 29 hand-case gates, so PR #179
  (merged 3be3d26) pulls every rescaled finder back toward the built-in one (same t for all numbers, largest t
  where every hand case passes its gate). His run: t 0.55.
- VERSION 5 SHIPPED 2026-11-03 via PR #185 (squash c429c68): weights.rs from his PC, KEY_FINDER_VERSION 5.
  Packs folds 1-4: whole loops 46.1% -> 56.0%, but loops labelled major 45.9% -> 29.9% (C G Am F and a C major
  scale now read "A minor or C major"; Am Dm E Am still sure A minor). Public songs: whole songs lose little
  (POP909-CL 0, RS200 1.2, Billboard 4.4 points) but 4-16 bar windows lose 8-22 points; a tune alone pays most.
  Numbers in docs/harmony/key-finder-experiments.md.
- Solomon chose "Ship it now" on a decision card: the MIDI page gets v5; BEFORE the Songs page reads keys, measure
  v5 on his own songs, and songs may keep version 4's numbers (noted in docs/handover/README.md). No page reads
  keys yet (mo-app uses neither mo-midi nor mo-harmony), so v5 has no visible effect until the hookup.
- Rescaling again: keyfit --rescale refuses while LEARNED_FROM has "packs"; put v4 back first with
  `git show 3be3d26:crates/mo-harmony/src/keyfind/weights.rs > crates/mo-harmony/src/keyfind/weights.rs`
  (tools/theorybench/README.md "Rescaling again").
- Solomon asked "how can check the key finder?" (MIDI page isn't built). Built `theorybench keycheck`, MERGED
  2026-11-03 via PR #188 (squash b0a1760). On his PC, after `git pull` in his checkout:
  `cargo run -p mo-theorybench --release -- keycheck "<MIDI packs folder>" --changed` (or a pack folder; `--counts` for
  the counts only). Per part: the name's key, version 4's answer, version 5's, marked better/worse/changed; counts
  of agreement with the names, minor/major names apart, whole loops/cut pieces apart. Prints only, writes nothing.
  Version 4's numbers live in tools/theorybench/src/keyfind_v4.rs (test checks hash 0x5e5cbe9fd9d3e96c);
  mo_midi::keys::hear gives what a part is heard as, read with any Model via keyfind::read_heard. On all of
  <MIDI packs folder> its whole-loops line should be near the refit's 46.1% -> 56.0% (all folds, v5 in-sample).
