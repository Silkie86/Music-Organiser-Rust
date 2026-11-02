---
name: harmony-engine-plan
description: mo-harmony crate plan and Solomon's "very precise" rule for keys/chords (2026-10-31)
metadata:
  type: project
  modified: 2026-10-31T06:32:05.847Z
---

mo-harmony starts in phase 2 with only "key from notes" (C# MusicTheory.EstimateMidiKey, 24-number profile), so the
Songs page key reader (C# ProjectKey) and later the MIDI page / Playground share one copy (Solomon's idea,
2026-10-31: don't repeat code). Phase 6 adds chord reader, key from chords, first-chord hint, voicer, reharmoniser,
harmoniser, chord-next (spec 5.1).

Solomon, 2026-10-31: "We need to be very precise with it." "I want improvement." "This was the weakest part of
the old app." Agreed improvements: heard pitch (FL root note/transpose), only notes the arrangement plays, settle
relative major/minor with bass + first/last chord, key per section + "changes key", measure against the keys Solomon
typed by hand (musical_key) at home, keep only changes that score better.
Key work was moved to phase 6 ("do this when you have access to my midi files as well"): built and tuned with the
MIDI index, measured on his projects AND his MIDI packs (counts and averages in code, rule 5). What C# got wrong most (Solomon 2026-11-01): CHORDS, not keys: extensions in jazz
and gospel, finding the root, and inversions. Part 3 (chord reader) centres on these: 9/11/13/altered, root when the
bass isn't it or no root is played (rootless), inversion vs slash chord; hand cases from jazz/gospel voicings, then
measured on his packs (chord names in file names) and Chord check.
**Why:** wrong keys/chords make every page that uses them wrong.
**How to apply:**
- High effort for building and reviewing any mo-harmony code (xhigh for the chord reader).
- REWRITE (Solomon 2026-10-31 06:55): the engine is planned again from the ground up, not ported. Solomon wasn't
  happy with the C# engine; it is the basis for the Playground app, so it must be solid. Reuse C# only for easy wins
  (proven bits). The C# results stay only as a baseline to beat in measuring, not as a design to copy.
- Twin test: score both against Solomon's hand-typed keys, and compare with the C# `detected_key` in library.db;
  list every difference to judge (better or bug) before merging.
- Hand-made test cases with known answers (scales, cadences, relative major/minor, ambiguous cases left blank).
- Keep MIDI 2.0 resolution; no rounding shortcuts.

See [[rust-rebuild-rules]], [[rust-app-design-decisions]].

ULTRACODE (Solomon, 2026-10-31 06:49): harmony is "so difficult and important" -> do mo-harmony at max effort with
ultracode (multi-agent workflows), research included. Plan for phase 6: (1) research workflow: symbolic key finding
(KK, Temperley, Albrecht-Shanahan, Bayesian/HMM key tracking, modulation), chord recognition from MIDI, functional /
jazz-gospel harmony, voice leading; datasets + licences (POP909, ChoCo, Lakh, Jazz Harmony Treebank, iReal, GiantMIDI);
adversarially verified, report to Solomon. (2) design judge panel. (3) build at xhigh. (4) multi-lens adversarial
reviews. (5) measure on held-out data + Solomon's hand-typed keys + his MIDI (counts only), old C# as baseline.
Styles (2026-10-31): C# has ONE engine; classical vs jazz/gospel is only a style setting in ChordScales
(HarmonyStyle), Harmoniser (Flavour) and Voicer (VoicingStyle). Proposed (not yet agreed) for the rewrite: one engine; reading keys/chords is
style-free; style = a set of weights for suggestions only (more styles like neo soul = new weights, not new code).

CHORD CHECK (Solomon 2026-10-31 10:10, agreed): a loop where the engine shows a few bars of his own songs/packs with
2-3 chord guesses (each playable), he clicks the right one or types his own; answers stay on his PC and become the
gold test set for tuning and measuring, round after round. Build after chord reader v1. Phase 6 runs in its own
thread "Build the harmony engine" (branch claude/project-thread-5apox1).
