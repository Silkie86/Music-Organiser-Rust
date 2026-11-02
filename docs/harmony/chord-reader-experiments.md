# Chord reader experiments

Measurements made while building the chord reader (design: `harmony/design/chord-reader-design.md`). The scripts are
in `harmony/experiments/table-recall/`; the data is the bench's scratch copies (POP909-CL, and the voice.py renders of
the Billboard charts and the sweep).

## E0. Do the candidate tables keep the right chord? (2026-11-01, cores PR)

Table-level candidate recall: for each labelled chord, is the label's core among the cores `valid` (root heard) or
`valid_rootless` (root not heard) allows for the notes sounding in its span? No hearing step yet, so "heard" is every
note that sounds in the span at all (and, to see how much presence matters, more than 10% or 25% of it). Time-weighted.

| Input | any presence | > 10% | > 25% |
|---|---|---|---|
| Billboard set A (voice.py) | 100.00% | 100.00% | 100.00% |
| Billboard set B (voice.py) | 100.00% | 99.95% | 80.26% |
| The sweep (voice.py) | 100.00% | 100.00% | 83.89% |
| POP909-CL, piano only | 91.03% | 89.00% | 74.04% |
| POP909-CL, piano and tune | 95.96% | 94.72% | 86.18% |

What it showed:

1. **Rootless m7b5 was missing.** Before m7b5 joined the rootless cores, the sweep's rootless voicings of a m7b5(9)
   lost the right chord 33% of their time (it was the only rootless miss). It was added in the cores PR, with its ♭5
   required.
2. **Real pop piano often plays open fifths.** On POP909-CL, piano only, the misses are mostly the piano playing no
   third under a major or minor label: root and fifth alone 3.1% of the time, sus shapes (1 2 5, 1 4 5) about 0.7%.
   Labels with a seventh the piano doesn't play (m7 over 1 ♭3 5, maj7 or 7 over 1 3 5) are about 0.9%. Chords
   shorter than two beats are missed 24% to 34% of their time (label edges).
3. **The design's accept sets hardly help** (91.03% to 91.11%). Letting a major or minor triad be tried on a root
   with its fifth and no third gives 96.15% (piano only) and 98.59% (piano and tune). So the design's gate
   (candidate recall at least 99% of POP909-CL time) is not met by the tables and accept sets alone. The reader PR
   has to choose: the open-fifth route in the tables, a stronger escape label (the design's single best-by-T1 escape
   ties on open fifths), or the tune in the heard set. Measure each with the bench's candidate-recall rate.
4. **Presence must count struck notes.** With "heard" as sounding for more than a quarter of the span, recall falls to
   74% (pop piano) and 80% (set B: arpeggios, Alberti). The hearing step's presence has to count a note struck in a
   broken part (the design's Ar signal), not only its held time.

## E0 again, with the hearing step (2026-11-01, candidates PR)

The same question with the reader's own hearing (`theorybench candidates`): the heard set is the design's P (a note
sounding an eighth of the stretch, chords and bass counted fully, the tune's held notes at half), the labelled ends
are moved to the nearest unit start, and labels are cut into stretches of at most two bars. Time-weighted, POP909-CL,
accompaniment / with the tune / the upper part with no bass:

| Ways in | accompaniment | with the tune | upper, no bass |
|---|---|---|---|
| the tables (played, rootless) | 89.19 | 91.34 | 84.23 |
| + the design's escape (best by the chords' notes alone) | 94.14 | 95.19 | 87.01 |
| + an escape by every note heard (bass and held tune too) | 94.27 | 94.98 | 87.03 |
| + open fifths (major and minor triads) | 95.43 | 96.25 | 89.87 |
| + open fifths, with the sevenths heard over them too | 95.82 | 96.60 | 90.13 |
| **+ open fifths and the escape (shipped)** | **96.99** | **97.43** | **91.48** |
| shipped, every tune note heard (not only held ones) | 96.99 | 98.21 | 91.48 |
| shipped, + sevenths not played (a triad heard tries its sevenths) | 97.61 | 97.89 | 92.21 |
| shipped, + rootless triads (a root not heard, its 3rd and 5th heard) | 97.61 | 97.99 | 94.59 |
| **shipped, ends within a unit of the label's** | **98.89** | **99.07** | **95.97** |
| shipped, ends within a unit, every tune note heard | 98.89 | 99.41 | 95.97 |

On the `voice.py` renders (Billboard A and B, RS200 A and B, the sweep) the shipped candidates hold the label 99.99% to
100% of the time with a bass part, and 93.6% to 99.3% (within a unit) for the upper part alone. 8.7 candidates a
stretch on POP909-CL's accompaniment, 10.0 with the tune, 23.4 in the sweep. After the code review, a stretch with its
ends moved is kept to two bars (the longest the reader scores): the upper part's 95.98 became 95.97, nothing else
moved. Labels too short to place (both ends on the same unit start) are counted now: 20 on POP909-CL, under 0.01% of
the time.

What was chosen and why:

1. **Open fifths in** (+6.2 points): the biggest gain, and musical. A root with its fifth and no third (an open fifth,
   or a sus shape) tries both triads; the scoring and the chords around choose.
2. **The escape in, counted by every note heard.** The design's escape (by the chords' notes alone) and one by every
   note heard (bass and held tune too, as presence counts them) are within 0.2 points either way; every note heard
   is kept because a chord with no third over a bass part (bass C, keys G B♭ D) needs its bass counted, or Gm7 wins
   the escape over C7. Ties go to the root nearest above the lowest note, then core order, so the escape moves with
   the music.
3. **Not taken: rootless triads** (+0.6, and +3.1 with no bass): they let a wrong root in wherever a third with a fifth
   above sounds, which the design's validity rule keeps out on purpose (the judges' "hard validity" point). Measure
   again as an experiment if the upper-part numbers matter.
4. **Not taken: sevenths not played** (+0.5): a label's seventh the piano never plays can't be read from the notes; the
   reader would never choose it.
5. **Not taken: every tune note in the heard set** (+0.8 with the tune): it would let short passing tune notes say
   which chord it is. The design's held-note rule stays; the gain is small within a unit (+0.3).
6. **The design's gate** ("candidate recall at least 99% of POP909-CL time") is met within a unit with the tune
   (99.07%) and nearly with the accompaniment alone (98.89%). The misses left are mostly stretches whose notes don't
   hold the label: one or two notes under a full chord label, a maj7 label over a plain triad, a sus2 label over an
   open fifth, a seventh label over its triad.

## E1. How much to trust the counted tables (2026-11-01, counted tables PR)

The counted tables (`theorybench tables`, README "The chord reader's counted tables") give P1 and P2 their numbers;
the two trusts say how much each counts, a beat. Until chordfit fits them they are set by hand, on the design's hand
cases read one stretch at a time (as `score_tests` reads them): each pair of trusts is scored by the hand cases'
smallest lead (the wanted reading's score less the best rival's, a beat; below 0 a case fails). `cargo test -p
mo-harmony show_the_trusts -- --ignored --nocapture` prints the whole grid (t_core 0 to 0.3 and t_ten 0 to 0.1, by
0.01). On the shipped tables (each core counted from the sources that name it):

| t_core, by t_ten | 0 | 0.02 | 0.04 | 0.06 | 0.08 | 0.10 |
|---|---|---|---|---|---|---|
| 0 | 0.10 | 0.07 | 0.01 | −0.06 | −0.13 | −0.20 |
| 0.05 | 0.06 | 0.14 | 0.10 | 0.04 | −0.03 | −0.10 |
| 0.10 | −0.03 | 0.15 | 0.19 | 0.13 | 0.06 | −0.01 |
| 0.15 | −0.12 | 0.06 | 0.23 | 0.20 | 0.15 | 0.08 |
| 0.20 | −0.21 | −0.03 | 0.15 | **0.26** | 0.22 | 0.16 |
| 0.25 | −0.30 | −0.12 | 0.06 | 0.07 | 0.07 | 0.07 |
| 0.30 | −0.38 | −0.23 | −0.22 | −0.22 | −0.21 | −0.21 |

What it showed:

1. **The two priors balance each other.** The commonness prior alone soon breaks case 21 with no bass (C E F♯ A): C6
   overtakes F♯m7b5. The tension prior holds it back, as labels almost never name a ♯11 on a 6 chord. Together they
   widen the hand cases' smallest lead from 0.10 a beat to 0.26 at t_core 0.2, t_ten 0.06 (shipped). The best pairs
   lie on a ridge (t_ten about 0.3 × t_core).
2. **What limits each.** Too much t_ten breaks case 33 (G C F over D reads Fsus2, not Dm11; E A D G with no bass
   drops Em7 to third) and G7alt (A♭m6 first). Too much t_core breaks case 24 (C♯dim7 reads C7: dim7 is the rarest
   core of all) and 21 with no bass (Am above F♯m7b5).
3. **What changes at the shipped trusts.** First readings: C before Cmaj7 for C E G over a bass walking C B; F (F/G)
   before G7sus4 for F A C over G, and the same in its two transpositions (the design accepts either); and on a single
   bar, B D G over a struck C reads G, not Csus2. Leads that grow most: B D G over a held C (G against Csus2, 0.11 to
   1.29 a beat), C E G over G (0.10 to 1.20), D F♯ A over C (1.00 to 1.96), G7/F (0.80 to 1.66). Leads that shrink
   most: the rare cores (C+ 1.77 to 0.79, C♯dim7 1.23 to 0.35), Cmaj7 under a tune running C D E (1.92 to 1.37) and
   C6 over C (0.65 to 0.28).
4. **Case 20's G7alt** (F B E♭ B♭ with no bass) is now kept (8th), so its gate keeps both D♭7 and G7.
5. **The rootless gates** (cases 12, 13 and 20 in the top two or three) are met at no pair of trusts up to 0.5 each
   (at best G7 comes 6th, the bar of case 12 with Fmaj7 or Dm9 has Dm7 4th): on these voicings with no bass, a reading
   on a played root explains as much of the notes whatever the priors say. They wait for the pair term (pass B), which
   reads the chords around.
6. **Labels aren't playing.** f_ten counts how often chart writers name a tension, not how often it's played:
   POP909-CL never names a 9, and Billboard names one on 9.4% of minor 7s. A tension prior counted from what's played
   (the piano's notes under each label) is likely higher; the design counts labels so the renderer has no part in the
   tables. Experiment C10 (counted tables against free weights) and chordfit's own trusts will say how much this
   matters.
7. **A source counts only for what it can name** (code review). The first tables counted every core from both sources,
   so the 6, m6, 7sus4 and power chords, which POP909-CL's labels can't name, came out at half their share. On those
   tables the best trusts were t_core 0.15, t_ten 0.04 (smallest lead 0.26 too); counted from the sources that name
   them, the ridge moved to 0.2 and 0.06, and C6 over C no longer limits t_core (C♯dim7 and case 21 with no bass
   do).
