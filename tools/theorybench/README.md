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

- **McGill Billboard** (CC0; Burgoyne, Wild and Fujinaga, ISMIR 2011): people's chord charts of US Billboard Hot 100
  songs from 1958 to 1991, with their tonic. `convert/charts.py` plays each as a bass line under block chords, and
  reads the key's mode from the tonic chord. They bring the blues and modal chords POP909 hardly has (a chord on the
  flat 7th, say), and keyfit may learn from them.
- **Rolling Stone 200** (de Clercq and Temperley's rock corpus, through ChoCo's JAMS files): 200 rock charts with
  their tonic, played the same way, for measuring only: their licence isn't one keyfit learns from. The Billboard run
  leaves out the 22 charts whose title RS200 has too, so the rock songs stay unseen. That's by title alone, so covers
  go too (they share the chords); two are other songs with the same title.

```
git clone --filter=blob:none --sparse https://github.com/boomerr1/the-mcgill-billboard-project.git billboard
cd billboard && git sparse-checkout set --no-cone '/billboard-2.0-index.csv' '/billboard-2.0-salami_chords/*'
git clone --filter=blob:none --sparse https://github.com/smashub/choco.git choco
cd choco && git sparse-checkout set --no-cone '/partitions/rock-corpus/choco/*'
python3 -I tools/theorybench/convert/charts.py jams choco/partitions/rock-corpus/choco/jams-converted \
    choco/partitions/rock-corpus/choco/meta.csv rs200 "measuring only (RS200)" path/to/bench/rs200.jsonl \
    --titles path/to/bench/rs200-titles.txt
python3 -I tools/theorybench/convert/charts.py billboard billboard/billboard-2.0-salami_chords \
    billboard/billboard-2.0-index.csv path/to/bench/billboard.jsonl --skip path/to/bench/rs200-titles.txt
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
cargo run -p mo-theorybench --release -- keyfit path/to/bench/pop909cl.jsonl [more.jsonl...] [--lambda-delta 1e-4|inf]
    [--inputs full,tune,upper,bass,piano] [--picks 8] [--bars 1,2,4,8,16] [--weigh <source>[/<input>]=<w>]
    [--dump <file>] [--against <file>] [--hand 13,14,16] [--shipped | --rescale] [--final | --write <weights.rs>]
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
- **Sources.** Several files can be read at once (`keyfit pop909cl.jsonl billboard.jsonl rs200.jsonl`). keyfit
  learns only from pieces whose licence lets the fitted weights ship (MIT, CC0, CC BY without NC, SA or ND: the
  `licence` each converter writes), and stops if no piece read has one. The rest are measured only: by the finders
  fitted without them, never in the kept-back fold, and by `--final`'s finder too. A piece only measured whose
  notes (in any key) are also in a piece learned from is left out, since its finder learned them. A piece with no
  tune (a chart) is heard as its full mix, its upper notes and its bass alone.
- **Weighting.** Each set of cases (a cut and a kind of input) counts 1 in all, however many cases it has, and whole
  songs as a full mix count 5. Counting every case the same instead lets the many one-bar windows outvote the rest:
  on folds 1 to 4 that gave one-bar windows 46.2% (against 42.3%), but songs that change key 72.0% (against 79.6%).
  Each source has sets of its own: `--weigh billboard=0.25` makes a source's sets count a quarter as much (0 counts
  it in the tables only), and `--weigh billboard/upper=0` does the same for one kind of input.

The report gives the full mix set by set as `keys` does, then a table of every kind of input against the C# app on
the same notes. Its last column ("sure") is how sure the finder was on average: above its share right, it was too
sure; below, not sure enough. Both are for the first file's source. Each other source follows in short, set by set:
how often the key, and the home note alone, was found by the C# app and by mo-harmony (a chart's key is its tonic
with a mode read from its chords, so the home note is the fairer score), and mo-harmony's log-loss.

- **Answers.** How sure each kind of answer must be at 1, 2, 4, 8, 16 and 32 bars (whole songs take the last) is
  fitted on held-out chances (`answerfit.rs`): the smallest chance at which the answers of that kind, given by at least
  30 songs, are wrong no more often than the design's target (one key 15% at 1 and 2 bars, 10% at 4 and 8, 8% at 16,
  5% for songs; a pair or a home note 10%), never rising with more music. A case is fitted at the knot at or above its
  length (a 3-bar case at 4 bars): the app draws a straight line between knots, never below the knot above, so it's
  at least as strict as the fit at every length. Each fold is graded with the thresholds fitted on the other folds'
  chances, so the thresholds never saw the songs they're graded on (the calibration behind those chances saw a little
  of every fold, so it isn't quite a clean hold-out). `--write` grades its report the same way. The report prints the
  thresholds fitted on all four folds, with how many answers each gives at each length, how many of those were wrong
  and the upper end of that share's likely range.
- **Shown as.** Each answer graded as the app shows it (`keyfind::grade`, with the fitted thresholds): one key, a
  key or its relative, a home note, or no clear key (and why). Beside each kind: how often it's right (a pair when the
  labelled key is either of the two, a home note when it's the tonic) and the chance shown with it on average. The
  "shown" and "right when shown" columns count only the answers given as one key.
- **Hand cases.** Every model fitted (each fold's, the one `--final` tests, the one `--write` writes) is read on the
  design's hand cases (`keyfind::cases`: a relative-key loop, a blues, a Dorian vamp, whole-tone scales and so on).
  The report says how many models keep each case's gate and meet its target, and what broke a gate. `--write` won't
  write weights that break one.
- **What `--write` writes.** The weights and the calibration; the thresholds, fitted on every fold's held-out
  chances; the share of notes in one scale a key needs, as built in (0.75); and a hash of every number with the
  finder version those numbers were first given with. When any number changed, that's the next version, so the tests
  fail until `KEY_FINDER_VERSION` in `keyfind/mod.rs` is bumped to match and the hand cases' readings are recorded for
  it (`hand_case_tests.rs`): the MIDI page then knows to read every part again.

### Trying a change (the design's experiments)

A change to the finder is tried on folds 1 to 4 only, against the run before it:

```
cargo run -p mo-theorybench --release -- keyfit path/to/bench/pop909cl.jsonl --dump before.tsv
(make the change)
cargo run -p mo-theorybench --release -- keyfit path/to/bench/pop909cl.jsonl --against before.tsv --hand 13,14,16
```

`--dump` writes how each case came out (right or not, and its log-loss). `--against` pairs each case with itself in
that file and prints, for every set and kind of input, the share right and the log-loss before and now, each
difference with its likely range (the songs drawn again, so a song's stretches stay together), and every set that got
worse beyond its range. The two runs must cut the same cases (the same `--bars` and `--inputs`; `--picks` only changes
which are fitted on): a dump made otherwise is refused, and cases missing from it are named rather than skipped. A run
can write its own dump to the file it's compared against. `--hand` prints each fold's model's reading of the hand cases named, to see how near a gate is.

The design keeps a change only if, on folds 1 to 4, 8-bar accuracy rises with its range above 0 (or, for a choice of
shape or calibration, log-loss falls by at least 0.01 while accuracy falls no more than 0.3 points); no set gets worse
beyond its range; and no hand case's gate breaks.

### Solomon's packs (on his computer only)

Rule 5 lets only counts and averages from Solomon's own MIDI go in code and, by his leave (2026-11-01), about twenty of
the key finder's numbers fitted on it: each stream's scale, the minor weight and the calibration. This is how, run on
his computer; his packs, and the file made from them, never leave it.

```
cargo run -p mo-theorybench --release -- packs "D:\MIDI Packs" [more folders] --out D:\Bench\packs.jsonl
cargo run -p mo-theorybench --release -- keyfit D:\Bench\packs.jsonl --inputs full --shipped --dump D:\Bench\shipped.tsv
cargo run -p mo-theorybench --release -- keyfit D:\Bench\packs.jsonl --inputs full --rescale --against D:\Bench\shipped.tsv
cargo run -p mo-theorybench --release -- keyfit D:\Bench\packs.jsonl --inputs full --rescale --final
cargo run -p mo-theorybench --release -- keyfit D:\Bench\packs.jsonl --inputs full --rescale --write crates/mo-harmony/src/keyfind/weights.rs
```

- **`packs`** reads every MIDI file under the folders as the MIDI page does (each part, its role guessed from its notes
  and names) and keeps a file only when its name gives exactly one key and no other note or chord name ("Loop 3 - C
  minor", "Keys_90_Ebmaj", "F#m"; not "Fm Bb Eb", which names chords, nor "Stab (Eb)", where a lone note may or may
  not be the key). A file whose name says nothing about notes takes the key of the nearest folder (up to three up)
  that names one. Each part is a piece, heard whole as a loop; its pack (the first folder below the one given) is its
  group, and a pack's files share a fold. Their licence says they're Solomon's own, so a plain keyfit run only
  measures them. The file can't be written inside a copy of this repository.
- **`--shipped`** scores the finder built in, as it is, on folds 1 to 4: what a refit has to beat. Its folds are drawn
  as a refit's are, so `--dump` and `--against` pair the two case by case.
- **`--rescale`** fits only the allowed numbers, on his pieces alone (anything else read is only measured): the
  built-in finder's weights for each stream times one number, and the minor weight, then the calibration. The tables
  and corrections inside those weights, the answers' thresholds and the share of notes in one scale stay as built in.
  A stream scale or calibration number that fewer than 30 pieces sound keeps its built-in number, and so does the
  minor weight unless 30 pieces are labelled major and 30 minor. The calibration's curve over bars moves only when 30
  full mixes (several parts together) have it: a part alone adds its own number to the curve, so parts alone can't
  tell the two apart, and full mixes (the Songs page) would pay for it. `--write` keeps the credits of the weights it
  scales, adds the rescale and its scales after them, and adds `packs` to `LEARNED_FROM`. A `--dump` of a run that
  read his pieces names his files, so it can't be written inside a copy of this repository either.
- **Keep rule**, as for a design change: on folds 1 to 4 against `--shipped`, the whole loops' accuracy rises with its
  range above 0, or their log-loss falls by 0.01 or more; no set gets worse beyond its range; no gate breaks. Then
  `--final` once, and `--write`. Back in the cloud, the public sets are scored with `--shipped` before and after the
  new weights, to see what the rescale costs songs.

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
