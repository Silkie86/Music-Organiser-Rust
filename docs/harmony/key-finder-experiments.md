# Key finder experiments (design E1 to E14), folds 1 to 4 of POP909-CL

Each variant is run with `theorybench keyfit --against base.tsv` (the finder as merged in #62 to #70, corrections
penalty 1e-4), paired case by case, ranges with the songs drawn again. The design keeps a change only if 8-bar accuracy
rises with its range above 0 (or, for shape and calibration choices, log-loss falls by 0.01 with accuracy down no more
than 0.3 points), no set gets worse beyond its range, and no hand-case gate breaks.

Baseline, full mix: whole one-key songs 91.0% (log-loss 0.455), key-changing songs 72.0% (0.877), 1-bar 42.0%,
4-bar 71.6%, 8-bar 81.0% (0.837), 16-bar 84.3%, 8-bar openings 86.1%. Gates broken on some folds: 13 (2/4 keep it),
14 (1/4), 16 (3/4).

| # | Variant | 8-bar full mix | Whole songs | Worse beyond range | Gates | Kept? |
|---|---|---|---|---|---|---|
| E1 | Corrections penalty 1e-3 / 1e-2 / 0.1 / 1 / tables alone | +0.2 / -0.1 / -0.2 / -0.3 / -0.3 points; log-loss +0.05 to +0.11 | about the same; log-loss +0.03 to +0.07 | single-part inputs up to -9 points | 13 and 16 kept from 1e-2 up; 14 never | No: the design takes the largest penalty within 0.01 nats of the best, which is 1e-4 |
| E2 | Evidence size g = 1 | -0.3, log-loss +0.034 | -0.3 | upper piano songs -6.0, many log-losses | 14, 16 | No |
| E2 | n0 = 2 | -0.1, +0.010 | 0.0 | upper piano -2.4 (songs), -0.6 (8 bars) | 13, 14, 16 | No |
| E2 | n0 = 8 | 0.0, -0.006 | +0.2, +0.014 | 1- and 2-bar windows -0.4 to -0.5 | 13, 14, 16 | No |
| E2 | ln(1 + N/4) | -0.6, -0.020 | -0.2, +0.043 | 1-bar windows -0.4 to -1.4 | 3, 13, 14, 16 | No |
| E3 | Anchor gap 7 | 0.0, -0.002 | -0.2 | upper piano -4.1 (songs), -1.4 (4 bars) | 10, 13, 14 (0/4), 16 | No |
| E3 | Bass counted on bar starts only | -0.7, +0.017 | +0.3 | 1- and 2-bar windows -0.6 to -2.8 | 3 (0/4), 10, 13, 14, 16 | No |
| E3 | Flat metre (every beat 1) | -0.1, -0.001 | +0.2; key-changing songs +2.2 | only log-loss +0.000 to +0.003 | 10 breaks (3/4), 14 (0/4), 13, 16 | No: 8-bar doesn't rise, and gate 10 breaks |
| E8 | Notes counted by onsets | -1.9, +0.030 | -0.6 | tune only -3.3 (8 bars), 16-bar -1.1 | 3, 14, 16 (0/4); 13 kept | No |
| E8 | Ticks times metre weight | -0.7, +0.012 | -0.2 | 1-bar -1.0, tune only -1.2 | 10 (1/4), 14 (0/4), 16, 23 (0/4) | No |

| E7 | No ghost-note gate / ghost notes by fixed velocity | 0.0, +0.000 | 0.0 | none of note | as baseline | No: POP909's velocities don't trip the gate either way |
| E7 | Ticks times square root of velocity | +0.1, +0.000 | 0.0 | key-changing songs log-loss +0.015 to +0.019; upper piano 8 bars -0.5 | as baseline | No |
| E4 | No chord streams at all | -0.9, +0.036 | +0.2 | 1- and 2-bar windows -0.6 to -3.3 | 3, 10 (0/4), 13, 14, 16 (1/4) | No |
| E4 | Shells too (root and third, no fifth) | +0.1, -0.000 | 0.0 | upper piano -0.3 to -0.7 | 10 (0/4), 13, 16 | No: gate 10 breaks |
| E4 | **Every major and minor chord, twice on the bass** | +0.0, **-0.010** | -0.2 (log-loss -0.004) | tune-only log-loss +0.000 to +0.004 | 13 (2/4), 14 (2/4, was 1/4), 16 (4/4, was 3/4) | **Yes, judged**: upper piano +4.2 (8 bars), +7.4 (songs), +9.5 (16-bar openings); bass from the piano +1.6. PR #77 |

After E4 (against E4, from the PR's review):

| # | Variant | 8-bar full mix | Whole songs | Worse beyond range | Gates | Kept? |
|---|---|---|---|---|---|---|
| E4b | The chord on another's third (Am7's C, Cmaj7's Em) doesn't count unless on the bass | -0.0, +0.000 | 0.0; key-changing +1.1 | 1- and 2-bar windows -0.2 to -0.3, log-loss +0.002 to +0.004 | 14 passes 4/4, but only because the plain Dm7 G7 Cmaj7 A7 reads further toward D minor (C major 0.15-0.23, was 0.30-0.52) | No |
| E4d | That chord counts half | +0.1 (0.0 to +0.2), +0.000 | 0.0 | 4-bar openings piano log-loss +0.002 | as E4; plain twin leans further to D minor | No |
| E4c | A root with both thirds off the bass is neither major nor minor | 0.0, +0.000 | 0.0 | none | as E4 | No: no measurable change |

| E5 | Tune streams: on strong beats (4, 2) and phrase ends (rest of a beat after a note, or the last note) | +0.2, -0.002 | -0.2; tune-only songs +2.8 | 8-bar openings -0.6; 1-bar tune only -0.5 | 13 breaks (0/4) | No |
| E5 | Strong beats only | +0.1, -0.003 | 0.0; key-changing +1.1 | 8-bar openings -0.6; 1- and 2-bar tune only -0.4 | as E4 | No |
| E5 | Phrase ends only | -0.0, +0.001 | -0.2 | 1-bar tune only -0.2 | 13 breaks (0/4) | No |
| E5 | Tune on every beat (4, 2, 1) and phrase ends | +0.2, -0.002 | -0.2 | 1-bar tune only -0.6 | 13 breaks (0/4) | No |
| E5 | Phrase ends after half a beat of rest | +0.2, -0.004 | +0.2 | 1-bar tune only -0.7 | 13 breaks (0/4) | No |
| E5 | Phrase ends, the last note not counted | -0.1, -0.004 | -0.3 | 1-bar tune only -0.4; piano 8 bars -0.1 | as E4 | No |

| E5 | A loop's first bass uses the song's first-bass weights (tied) | +0.0, +0.000 | +0.2; key-changing +2.2 | upper piano songs -1.6, bass from the piano songs -2.1 | as E4 | No |
| E6 | Bass arriving a fifth down (V to I) and dominant-seventh chords, as streams | -0.1, -0.010 | -0.5; key-changing log-loss -0.060 | upper piano 4 bars -1.0, openings log-loss +0.009 to +0.013 | 10 breaks (0/4), 14 (1/4) | No |
| E6 | Bass arriving a fifth down only | +0.1 (-0.3 to +0.5), -0.010 | +0.2; key-changing +3.2; bass from the piano 8 bars +0.8 | upper piano 2 and 4 bars -0.5 and -0.9 | as E4 | No: the design keeps bass moves off unless accuracy rises; upper piano gets worse |
| E6 | Dominant-seventh chords only | -0.2, -0.001 | -0.5 | small log-loss rises | 10 breaks (2/4) | No |


E11, the calibration (accuracy can't change; judged by log-loss and calibration error, ECE, in each set):

| Variant | What happened | Kept? |
|---|---|---|
| No offsets for the kind of input | ECE worse by 0.01 or more in 24 sets (tune-only songs 0.169 to 0.308), better in 9 | No: the offsets stay |
| No loop curve | Openings much worse (1-bar openings ECE 0.093 to 0.211, log-loss +0.091) | No: the loop curve stays |
| No song offset | Songs slightly better, 16-bar windows worse (ECE 0.056 to 0.099) | No: the song offset stays |
| A song offset for each kind of input (3 more numbers) | Single-part songs much better calibrated (tune only ECE 0.169 to 0.084, bass 0.129 to 0.047; log-loss -0.027 and -0.033), but full-mix 8- and 16-bar windows log-loss +0.005, beyond their range | No by the strict rule; worth another look if song-level "how sure" matters |


E13 and E14 (against E4):

| # | Variant | 8-bar full mix | Whole songs | Worse beyond range | Kept? |
|---|---|---|---|---|---|
| E13 | Folds 1 to 4 drawn by artist (POP909's index: 909 songs, 462 artists; fold 0 unchanged) | -0.1 | -0.3 | bass-from-the-piano songs -2.2 | Folds stay by song: by-artist scores are within 1 point (the design's test), so artists don't leak much |
| E14 | Openings (loops) count twice | -0.2, +0.008 | -0.3 | 1- and 2-bar windows -0.3 to -0.5 | No |
| E14 | Full-mix sets count twice | -0.1, -0.008 | 0.0, -0.007 | single-part log-loss +0.003 to +0.011; bass 4 bars -0.4 | No |

A separate set of weights for whole songs (E14's second half) wasn't built: whole songs already beat the C# app by 3.3
points (90.9% against 87.5%), and it would double the weights.

E10, home tables and weights from chord charts (against E4; folds 1 to 4). The McGill Billboard charts (CC0; 712
songs after dropping 150 duplicates and the 22 that are also in RS200) and the Rolling Stone 200 charts (RS200; rock;
measuring only, never fitted on) are played as a bass line under block chords. Their keys are the charts' tonics,
with the mode read from the tonic chord. 75 RS200 songs share an artist with a Billboard song (no shared songs).
Today's finder (fitted on POP909-CL only) already finds the RS200 home note in 91.0% of whole songs and 72.2% of
8-bar windows (the C# app: 75.3% and 59.7%).

| Variant | POP909-CL: 8-bar, 1-bar, upper-piano songs | RS200 home note: songs, 8-bar, 16-bar | Hand cases | Kept? |
|---|---|---|---|---|
| Charts in the tables only (the design's E10) | -0.1, 0.0, +0.5; log-loss +0.001 to +0.005 in 17 sets | 91.0, 71.8, 73.2 | as E4 | No: no gain |
| Learned from, sets counting 0.1 | 0.0, -0.5, -1.4 | 92.2, 73.8, 75.5 | 14 passes 4/4; 9 still a coin flip | No by the strict rule (11 sets worse) |
| ... counting 0.25 | -0.1, -1.0, -3.6 | 94.0, 74.6, 76.3 | 14 passes 4/4; 9 reads C (0.48 to 0.52) | No by the strict rule (14 sets worse); the best trade |
| ... counting 0.5 | -0.4, -1.6, -4.7 | 94.6, 75.1, 77.6 | 14 passes; 9 reads C (0.52 to 0.55); blues C 0.78 to 0.81 | No by the strict rule (17 sets worse) |
| ... counting 1 | -0.5, -2.5, -7.3 | 95.2, 75.4, 77.8 | 14 passes | No (21 sets worse) |
| 0.5, the charts' full mix only | -0.1, -1.1, -5.5 | 94.6, 74.6, 76.7 | 14 passes | No; RS200 upper-piano songs -5.4 |
| 0.5, no upper-piano sets | -0.1, -1.2, -6.2 | 94.6, 74.7, 77.0 | 14 passes | No |

Learning from the charts trades pop for rock: RS200 (never fitted on) gains 3 to 5 points and much better log-loss
(-0.1 to -0.3) in every set, the Mixolydian vamp reads C and gate 14 holds on every fold; POP909-CL loses about 1
point on 1- and 2-bar windows and 3 to 7 on single-part whole songs, while its whole songs, 8- and 16-bar windows
and openings stay level or rise. No weight passes "POP909-CL not worse", so this is Solomon's call (asked
2026-11-01).

E9, colour parts (against E4, and on top of E10 at 0.25). Each key also scores as a colour part: Mixolydian for a
major key (the tune, harmony and bass notes on the 7th and the lowered 7th swap weights), Dorian (6th and lowered
6th) and harmonic minor (7th and lowered 7th) for a minor key; a key's score is the log of the sum over its parts.
The parts' base rates and a change to the minor weight are fitted after the weights, on the same cases (they come
out near -3, about 1 in 20, on every fold, with or without the charts).

| Variant | POP909-CL | RS200 home note: songs, 8-bar | Hand cases | Kept? |
|---|---|---|---|---|
| Colour parts, pop-only weights | level; bass-only songs -0.6, key-changing songs +1.1; 5 sets worse, 7 in log-loss | 90.4 (-0.6), 72.4 (+0.2) | 9 still a coin flip (F 0.46 / C 0.46 on fold 1); 14 3/4 | No |
| Colour parts on top of E10 (0.25) | as E10 at 0.25, within 0.4 | 94.0 (0.0), 75.1 (+0.5) | 9 reads C a little more (0.50 to 0.54, was 0.48 to 0.52); no target newly met | No |

The parts change almost nothing: pop rarely needs them, and the chord and bass streams already decide the modal
cases. As the design expected, the modal fixes need E10, not E9, so the finder stays without them.

Gate 13 (2026-11-01). The tune alone in C minor (C minor pentatonic, starting and ending on C) has C minor on top on
every fold (0.62 to 0.71), but its runner-up gate ("Eb major or G minor") broke on 2 of 4 folds in the baseline and in
every variant since: F minor and Eb major sit within 0.01 of each other (fold 2: F minor 0.107, Eb major 0.099). Three
independent judges (music theory, product, test integrity) all found the gate inconsistent: it allowed G minor but not
F minor, though both hold all five notes and are a fifth from C minor, and C, the long first and last note, is F
minor's fifth. Four classic key profiles (Krumhansl-Kessler, Temperley, Aarden-Essen, Albrecht-Shanahan) never put
Eb major in their top five; most put F minor and G minor there. So the gate now allows Eb major, F minor or G minor
second and never a key alone but C minor; Eb major second and the answer "C minor or Eb major" stay as targets (met on
2 of 4 folds). Two judges also proposed giving the pair answer by its sum whatever the relative's rank (on folds 2
and 4, C minor and Eb major together are 0.73 to 0.75, above the pair's 0.63, but the pair isn't given only because
F minor is second); that is a change for every song, so it's tried as an experiment of its own (A1).

A1, the pair answer by its sum (against E4; answers only, the chances don't change). "Key or its relative" is
given when the two keys together are likely enough, wherever the relative ranks; when a home note is likely enough too,
the likelier of the two is given. Every kind stays within its target at every length, and case 13 says "C minor or Eb
major" on all four folds, but:

| Set (full mix) | No clear key | Key or its relative: given, right |
|---|---|---|
| 4-bar windows | 24.8% -> 10.5% | 63.2%, 93.8% -> 77.7%, 92.0% |
| 8-bar windows | 9.9% -> 1.6% | 50.1%, 94.9% -> 58.6%, 92.8% |
| Whole songs that change key | 24.7% -> 6.5% | 38.7%, 97.2% -> 62.4%, 84.5% |
| 4-bar openings | 24.6% -> 8.4% | 33.6%, 92.6% -> 49.9%, 88.9% |

The pairs' thresholds rise from 0.01 to 0.30 to 0.63 (9 to 16 bars: 0.30), and pairs are wrong more often (3 to 4 bars
8.6% -> 9.7%, 9 to 16 bars 4.5% -> 9.7%, songs 3.5% -> 8.1%). The target is judged over all the pairs given, so a
pair with a chance near a threshold of 0.30 gets through, and the chances are calibrated: such a pair is right about a
third of the time. A pair can also hide a third key as likely as the relative (C major 0.6, G major 0.3: "C major or
A minor"). On 4-bar windows, the pairs it adds are right about 84% of the time, where a "no clear key" leaning the same
way was given before. Not kept: it trades sure answers for more answers, against "very precise". Gate 13 no
longer needs it. (If answers should come more often, the place to change is how a threshold is fitted, judging the
answers just above it rather than all of them.)

Kept so far: E4 (every chord, judged). Every other built choice holds. The E4 code rewritten with pitch-class sets
gives the same answer on every case (checked case by case).

## The final check and the shipped weights (2026-11-01)

Design frozen: E4 (every chord), corrections penalty 1e-4, gate 13 as above, POP909-CL learned from, Billboard learned
from at a quarter weight (Solomon's choice), RS200 measured only. `keyfit --final` was run once, at commit 0e91907: one
finder fitted on folds 1 to 4 scored fold 0 (179 POP909-CL songs and 141 Billboard charts that no experiment had looked
at) and the 197 RS200 charts. Fold 0 against what folds 1 to 4 predicted (full mix, mo-harmony against the C# app):

| Set | Folds 1 to 4 (727 songs) | Fold 0 (179 songs), the final check |
|---|---|---|
| 1-bar windows | 41.2% vs 29.4% (+11.8) | 42.8% vs 31.0% (+11.8, +10.3 to +13.2) |
| 2-bar windows | 56.2% vs 44.2% (+12.0) | 58.7% vs 46.5% (+12.2, +10.3 to +14.0) |
| 4-bar windows | 71.2% vs 61.8% (+9.4) | 73.4% vs 63.6% (+9.7, +7.6 to +11.9) |
| 8-bar windows | 80.9% vs 75.2% (+5.7) | 81.5% vs 75.4% (+6.1, +3.6 to +8.5) |
| 16-bar windows | 84.9% vs 81.2% (+3.7) | 85.0% vs 81.5% (+3.5, +1.2 to +6.0) |
| 1 / 2 / 4-bar openings | +23.3 / +14.2 / +14.1 | +14.3 / +14.5 / +12.4 (all real) |
| 8 / 16-bar openings | +7.4 / +4.4 | +6.1 / +1.7 (16 bars: -1.7 to +5.6) |
| Whole songs with one key | 91.0% vs 87.5% (+3.5) | 89.6% vs 88.9% (+0.7, -2.1 to +3.5) |
| Whole songs that change key | 72.0% vs 67.7% (+4.3) | 71.4% vs 74.3% (-2.9, 35 songs, -11.4 to +5.7) |

The windows came out within half a point of the prediction, so the choices weren't tuned to the songs they were made
on. Whole songs gain less on fold 0 (within both ranges). The known cost of the charts shows again: the upper piano
alone is 4 to 6 points behind the C# app on whole songs and on 8 and 16-bar openings (ranges -10.4 to +2.1,
-10.7 to +1.7 and -11.7 to +0.0 on 179 songs).
When one key is shown on an 8-bar window it's right 92.8% of the time (said 91.0%); "a key or its relative", 95.8%.
Billboard's fold 0 (141 charts): whole songs 85.3% (C# 67.4%), 8-bar windows 62.5% (50.9%). RS200, never learned from:
whole songs 92.2% (74.1%), tonic 94.0% (75.3%); 8-bar windows 74.3% (57.7%). Every gate holds; targets 6, 7, 9, 12 and
14 are still unmet.

`keyfit --write` then fitted on every song (calibration and thresholds from all five folds held out in turn). It was
run four times: the first two wrote headers that needed a fix (which songs the summary scores; a blank line clippy
asks for), and the fourth came after the code review's floor (below). The weights and calibration were the same to
the last bit every time. Shipped as KEY_FINDER_VERSION 4. With every song, case 8's target ("a blues-scale tune alone,
ending on C") is no longer met; every gate still holds. Every run that looked at fold 0 is listed in
`tools/theorybench/test-runs.md` (a commit marked -dirty had changes not yet committed).

The code review found the pair's threshold at the grid's 0.01 from 8 bars: "C minor or Eb major" was given whenever
the relative came second, however unlikely the two together. Measured on folds 1 to 4 (8-bar windows, full mix), the
pairs are right about as often as their chance says: 0.5 to 0.6, 5 pairs, 60% right; 0.6 to 0.7, 22, 73%; 0.7 to 0.8,
130, 86%; 0.8 to 0.9, 564, 92%; 0.9 or more, 3,029, 96%. No full-mix pair was below 0.5. So answerfit now fits no
threshold below an even chance (an answer names something likelier than not). On every fold held out in turn, that
drops the few pairs below 0.5, all from a part heard alone (about 0.2% of the cases from 5 bars up), and the pairs
given are wrong a little less often (5 to 8 bars 7.4% to 7.3%, songs 4.3% to 4.1%). Shipped pair thresholds: never at
1 and 2 bars, 0.61 at 4, 0.5 from 8. A threshold of 1 now means never even when the chances add up to exactly 1.

## Rescaled on Solomon's packs: version 5 (2026-11-03)

Rule 5's exception (Solomon, 2026-11-01): only each stream's scale, the minor weight and the calibration may be fitted
on his own MIDI. The runs were made on his computer from main 3be3d26 (finder version 4), as
`tools/theorybench/README.md` says under "Solomon's packs": `--shipped --dump`, the choosing run
`--rescale --against`, `--final` once, then `--write`. His packs gave 51,655 pieces keyed by their file or folder
names, and 81% of the whole loops are labelled minor. Every finder the rescale scored or wrote was pulled back to 0.50
or 0.55 of the way from version 4 to its fit, the last step where every hand case keeps its gate. One step further,
case 16 broke in every one ("a bass alone: C Ab Eb Bb" gave no clear key where its gate needs "C minor or Eb major").

Choosing run, folds 1 to 4, each case paired with version 4's reading of it (full mix: each piece as the MIDI page
hears it):

| Set | Version 4 | Rescaled | Difference (range) |
|---|---|---|---|
| Whole loops | 46.1% | 56.0% | +9.9 (+8.2 to +14.2) |
| Pieces cut from longer parts | 29.9% | 41.4% | +11.5 (+10.2 to +16.9) |
| 1 / 2 / 4 / 8 / 16-bar windows | 25.3 / 31.4 / 36.0 / 36.5 / 36.3% | 40.5 / 45.4 / 47.9 / 48.5 / 48.6% | +15.1 / +14.1 / +11.9 / +12.0 / +12.3 |
| 1 / 2 / 4 / 8 / 16-bar openings | 47.5 / 43.3 / 44.0 / 41.8 / 40.4% | 59.8 / 55.9 / 54.3 / 52.2 / 50.8% | +12.3 / +12.6 / +10.3 / +10.5 / +10.4 |

Every set got better beyond its range, and its log-loss fell (whole loops 2.875 to 2.127); the whole loops'
calibration error fell from 0.119 to 0.037. The gain is the minor keys': whole loops labelled minor went from 46.1% to
62.3% right, and those labelled major fell from 45.9% to 29.9%. One key is given for fewer whole loops (10.7% to 7.1%)
and is right more often when it is (78.5% to 83.1%).

The final check (`--final`: fold 0, 10 packs and 4,734 pieces no run had looked at), pulled back to 0.50: whole loops
46.9% right against the C# app's 38.4% (+8.5, +0.2 to +13.8), calibration error 0.030. Every set is ahead of the C#
app, though for the 4, 8 and 16-bar windows and the 8 and 16-bar openings the range crosses 0. That lead is under half
of folds 1 to 4's (+19.7 on whole loops), and with 10 packs its range is wide; how version 4 does on fold 0 isn't known,
since only `--final` and `--write` look at it.

`--write` fitted on every piece and pulled back to 0.55 (case 16 breaks at 0.60). Its report, each fold held out in
turn and pulled back to 0.50 or 0.55: whole loops 55.0% (C# app 36.5%). Shipped as KEY_FINDER_VERSION 5:

- Stream scales: tune 0.673, harmony 0.656, bass 0.612, major chords 0.725, minor chords 0.748, a loop's first bass
  0.982; a song's first and last bass 1 (packs have no songs).
- Minor weight: -0.488 to 0.621.
- Calibration: melody only +0.612 to +0.485, bass only +0.342 to +0.372, accompaniment +0.377 to +0.330; what a loop
  adds at 1, 2, 4, 8 and 16 bars +0.394, +0.196, +0.029, -0.059, +0.018 to +0.422, +0.181, -0.059, -0.163, -0.173. The
  curve over bars and a song's offset stay (only full mixes may move the curve, and packs have none), and so do the
  answers' thresholds and the share of notes in one scale.

Hand cases: every gate holds. Case 8's target ("a blues-scale tune alone, ending on C") is met again and case 13's ("a
tune alone in C minor with no sixth") no longer is; targets 6, 7, 9, 12, 13 and 14 are unmet.

### What it costs songs

The public sets scored with `--shipped` (folds 1 to 4, nothing fitted) by version 4, then by version 5, each case paired
with itself. Version 4 learned from POP909-CL and Billboard, but its POP909-CL scores here are within a point of its
held-out ones (`--write`'s report, each fold held out in turn), and RS200 was never learned from. Full mix; Billboard
and RS200 by the key (a chart's tonic, with a mode read from its chords):

| Set | POP909-CL | Billboard | RS200 |
|---|---|---|---|
| Whole songs with one key | 91.3% to 91.3% (+0.0) | 86.1% to 81.7% (-4.4) | 91.0% to 89.8% (-1.2, -4.8 to +2.4) |
| 1-bar windows | 41.3% to 38.2% (-3.1) | 43.9% to 25.6% (-18.3) | 53.4% to 32.1% (-21.3) |
| 4-bar windows | 71.5% to 63.5% (-8.0) | 59.1% to 47.4% (-11.7) | 70.7% to 53.5% (-17.2) |
| 8-bar windows | 81.4% to 72.0% (-9.5) | 66.7% to 51.6% (-15.1) | 74.2% to 54.1% (-20.1) |
| 16-bar windows | 85.1% to 76.3% (-8.8) | 70.9% to 56.3% (-14.6) | 75.6% to 53.4% (-22.2) |
| 4-bar openings | 80.6% to 78.4% (-2.2) | 78.0% to 73.8% (-4.2) | 84.5% to 85.1% (+0.5) |

A part alone pays most. POP909-CL's tune alone: whole songs 86.6% to 63.1% (-23.5; the C# app 86.4%), 8-bar windows
74.4% to 56.1% (-18.2; C# 68.8%). On POP909-CL every set of windows is worse beyond its range for every kind of input,
and so are most openings; whole songs as a full mix, the piano or the bass keep their accuracy within its range, a
little less sure (full mix log-loss 0.399 to 0.433). On 8-bar windows version 5 is now behind the C# app (72.0%
against 75.2%), and on 16-bar windows too (76.3% against 81.2%).

Why: about 42% of these songs' keys are minor, against 81% of the packs' loops. The minor weight now favours a minor
key in everything, and every stream's weights are about a third smaller, so a stretch's notes count for less against
it. Most of the new wrong answers are a major key's relative minor: on 8-bar windows the relative, either way, went
from 11.7% to 20.3% of the cases, major keys from 85.1% to 60.3% right and minor keys from 76.3% to 88.0%. Two of
mo-midi's tests show it on a tune alone: a plain C major tune of six bars went from "C major" (87% sure) to no clear
key leaning C major (42%, A minor second), and an A minor tune from "A minor or C major" to no clear key leaning A
minor.

Who pays: today only the MIDI page reads keys, from his packs, where version 5 is better. The Projects page and the
Playground will read keys of songs; before they do, version 5 should be measured on his own songs, and songs given
version 4's numbers (a second set beside the packs') if those suit them better.
