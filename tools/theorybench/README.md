# Theory bench

Measures the harmony engine (`mo-harmony`) on music other people labelled, beside the C# app's engine as it was, so
every change to how the app hears keys and chords is checked before it ships: run it before and after, and compare.
The C# engine is the fixed point every new idea has to beat.

## The labelled music

It never goes in the repository (`*.jsonl` is ignored) and nothing from it goes into the app. The scripts in
`convert/` turn a public dataset into one JSON object a line: a piece's notes by part, its bar grid, and the keys and
chords someone (or some program) labelled. The bench reads only that, so any dataset can be added with a converter.

- **POP909** (MIT licence): 909 Chinese pop songs as MIDI (melody, bridge and piano parts) with key, chord and beat
  labels. The labels were made by software (keys from the audio, chords from the MIDI), not by people, so POP909
  shows how two engines compare, not how often either is truly right.

```
git clone --filter=blob:none --sparse https://github.com/music-x-lab/POP909-Dataset.git pop909
cd pop909 && git sparse-checkout set --no-cone '/POP909/*/*.mid' '/POP909/*/*.txt'
pip install mido
python3 -I tools/theorybench/convert/pop909.py path/to/pop909/POP909 path/to/bench/pop909.jsonl
```

- **POP909-CL** (MIT licence; Yao, Chen, Dubnov and Berg-Kirkpatrick, ICASSP 2026): 906 of the same songs, the same
  notes, with labels checked by people: expert-reviewed chords (named by triad and seventh templates, so no 9ths or
  13ths), corrected bar lines and time signatures, and the key changes POP909 missed. It is the better test of the
  two, and its weights may ship (with credit).

```
git clone --filter=blob:none --sparse https://github.com/AndyWeasley2004/POP909-CL-Dataset.git pop909cl
cd pop909cl && git sparse-checkout set --no-cone '/POP909_processed/*' '/POP909_chord_labels/*'
python3 -I tools/theorybench/convert/pop909cl.py path/to/pop909cl path/to/bench/pop909cl.jsonl
```

## Keys

```
cargo run -p mo-theorybench --release -- keys path/to/bench/pop909.jsonl [--part melody] [--aligned] [--bars 1,2,4,8,16]
```

It finds the key of whole songs (those with one key, and those that change key, against the key labelled for the
longest time) and of 8-bar clips cut along the bar lines (against the key labelled at the middle of the clip), with
each key finder: the C# app's (copied unchanged in `src/baseline.rs`) and four published key profiles run the same way.
For each it prints:

- **right**: the share of exact answers (no answer counts as wrong), with the range it most likely lies in. The
  ranges come from drawing whole songs again at random, because the clips of one song aren't independent.
- **home note**: the right tonic, major or minor either way.
- **MIREX**: the contest score (1 exact, 0.5 a fifth up, 0.3 the relative key, 0.2 the parallel key).
- **shown** and **right when shown**: how often the confidence clears the app's cut, and how often it's right then.
- **major** and **minor**: right, split by the labelled mode.
- **ECE**: how far the confidence is from a chance of being right (0 is "80% sure" right 80% of the time), over ten
  bins of equal width, then fifteen of equal size. The C# app's confidence was never meant as a chance, so its number
  shows how much a calibrated one would add.
- **AURC**: answering only the surest cases, for every number of cases, the average share wrong (lower is better).
- **NLL** and **Brier**, for a finder that gives a chance to every key: how much its chances say about the truth.
- what the wrong answers were (a fifth up or down, the relative or parallel key, another key, no key);
- how often it's right when it answers only its surest cases (if its confidence means anything, this rises);
- against the first finder: the difference in points, with its range, and McNemar's test.

`--part` hears only the parts with that role; `--aligned` keeps only the songs the C# bench used (four beats a bar,
labelled bar lines on the MIDI's grid: 387 of POP909's songs); `--bars` sets the clip lengths (a MIDI pack's loop is
often 1 to 4 bars).

## Fitting the key finder

```
cargo run -p mo-theorybench --release -- keyfit path/to/bench/pop909cl.jsonl [--lambda-delta 1e-4|inf]
    [--inputs full,tune,upper,bass,piano] [--picks 8] [--bars 1,2,4,8,16] [--final | --write <weights.rs>]
```

`keyfit` fits mo-harmony's key finder and measures it on songs held out of each fit, beside the C# app on the same
notes.

- **Folds.** Each song goes to one of five folds by a hash of its name (`pop909/001`, so POP909 and POP909-CL agree),
  and songs with the same notes in another key share a fold. Fold 0 is kept back: every choice is made on folds 1 to
  4, each scored by a finder fitted on the other three. `--final` fits on folds 1 to 4, scores fold 0 once, and adds
  a line to `test-runs.md`, so how often fold 0 was looked at is on record. `--write` comes last: it scores every
  fold (fold 0 included, so it's logged too), fits on every song and writes `crates/mo-harmony/src/keyfind/weights.rs`.
- **Cases.** Whole songs (heard as songs), stretches of 1 to 16 bars cut along the bar lines (heard as windows, and
  scored against the key at their middle, as the C# bench did) and each song's first bars (heard as loops). Each is
  heard five ways: the full mix, the tune alone, the piano's upper notes, the bass line taken from the piano, and the
  piano (the parts under the tune, each with its role: a dataset's bass part stays a bass part). Only stretches inside one key region are fitted on, at most `--picks` a song for each length; all are scored.
- **Model.** Each stream's weights are a counted table (the average share of each note above the labelled tonic) times
  one trust weight, plus a correction for each note held near 0 by `--lambda-delta` (`inf`: the tables alone). The
  tables a song is fitted with are counted without its own fifth of the songs.
- **Calibration.** How sure a reading may be comes from held-out scores: a curve over the bars heard (1 to 16, flat
  past 16), offsets for the kind of input, a curve of its own that a loop adds, and an offset for a song.
- **Weighting.** Each set of cases (a cut and a kind of input) counts 1 in all, however many cases it has, and whole
  songs as a full mix count 5. Counting every case the same instead lets the many one-bar windows outvote the rest: on
  folds 1 to 4 that gave one-bar windows 46.2% (against 42.3%), but songs that change key 72.0% (against 79.6%).

The report gives the full mix set by set as `keys` does, then a table of every kind of input against the C# app on
the same notes. Its last column ("sure") is how sure the finder was on average: above its share right, it was too
sure; below, not sure enough.

- **Shown as.** Each answer graded as the app shows it (`keyfind::grade`, with the thresholds built in): one key, a
  key or its relative, a home note, or no clear key (and why). Beside each kind: how often it's right (a pair when the
  labelled key is either of the two, a home note when it's the tonic) and the chance shown with it on average. The
  "shown" and "right when shown" columns count only the answers given as one key.
- **Hand cases.** Every model fitted (each fold's, the one `--final` tests, the one `--write` writes) is read on the
  design's hand cases (`keyfind::cases`: a relative-key loop, a blues, a Dorian vamp, whole-tone scales and so on).
  The report says how many models keep each case's gate and meet its target, and what broke a gate. `--write` won't
  write weights that break one.
- **What `--write` writes.** The weights and the calibration; the thresholds and the share of notes in one scale a
  key needs, as they are (they aren't fitted yet); and a hash of every number with the finder version it belongs to.
  When any number changed, that's the next version, so the tests fail until `KEY_FINDER_VERSION` in
  `keyfind/mod.rs` is bumped to match and the hand cases' readings are recorded for it (`hand_case_tests.rs`): the
  MIDI page then knows to read every part again.

## Chords

The C# app's chord reader runs unchanged through the small C# helper in `cs/`, which needs .NET 10 and a clone of the
C# app (on Ubuntu, `apt-get install dotnet-sdk-10.0`):

```
dotnet build tools/theorybench/cs -c Release -o path/to/helper -p:MoOldRepo=path/to/Music-Organiser -p:MoBuildNative=false -p:MoBuildBitwig=false
dotnet path/to/helper/theorybench-cs.dll chords path/to/bench/pop909.jsonl --part accompaniment > cs-chords.jsonl
cargo run -p mo-theorybench --release -- chords path/to/bench/pop909.jsonl --answers "C# app=cs-chords.jsonl"
```

Each `--answers name=file` is one engine's chords for every piece; the first is the baseline the others are compared
with. The chords are scored exactly as `mir_eval.chord` scores chord recognition, so the numbers mean what published
ones mean: each stretch of time weighted by its length, pooled over all the songs.

- **root**: the right root. **majmin**: the right root and major or minor (where the label is major, minor or no
  chord). **thirds**, **triads**, **sevenths**, **tetrads**: more of the chord right each time (tensions like 9ths and
  13ths are never scored, as mir_eval leaves them out). **mirex**: at least three notes in common. **+bass**: the
  bass right as well. **bass**: the bass note alone.
- **over-cut** and **under-cut**: how much the chords read chop up the labelled ones, or run them together;
  **changes a bar**: how often the reader changes chord.

`--dump file` writes every piece's labels, chords read and scores; `python3 -I convert/check_mir_eval.py file` runs
mir_eval itself on them and prints both sets of numbers (they matched exactly on all 909 POP909 songs, 2026-10-31).

## The C# baseline on POP909 (2026-10-31)

Keys, 8-bar clips (8,850 clips from 909 songs): the C# app is right 77.7% of the time (76.0 to 79.3), MIREX 83.2;
its most common miss is the relative key (13.3% of clips). Whole songs with one key: 90.8%. The Krumhansl-Kessler
profile gets 66.0% of the clips, as the C# bench measured. Melody alone 71.9%, accompaniment alone 69.6%.

Chords, accompaniment part, all 909 songs: root 88.5%, majmin 81.5%, sevenths 79.4%, mirex 82.1%, bass 90.7%;
segmentation 91.2, 1.36 chord changes a bar (the labels change 1.47 times a bar).

## The C# baseline on POP909-CL (2026-10-31)

Against the keys and chords people checked, the C# app does a little worse than against POP909's own labels, which
were made by software much like it. Keys: whole songs with one key 87.8% (85.5 to 90.1; the Aarden-Essen profile
alone gets 87.7%), songs that change key 69.5%; clips of 1, 2, 4 and 8 bars 29.7%, 44.7%, 62.2% and 75.2%, the
relative key still the most common miss. Chords, accompaniment part, all 906 songs: root 88.0%, majmin 81.7%, sevenths
77.8%, mirex 82.9%, bass 92.8%; segmentation 93.9 (mir_eval gives the same numbers).
