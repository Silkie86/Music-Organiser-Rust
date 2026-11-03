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
  write weights that break one. A rescale's models are pulled back until they keep every gate (see Solomon's packs,
  below), so they're read as pulled back.
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
  and names), walking the folders as the app does (a folder given inside another is read once, hidden folders passed
  by). It keeps a file only when its name gives exactly one key and no other note or chord name ("Loop 3 - C minor",
  "Keys_90_Ebmaj", "F#m"; not "Fm Bb Eb", which names chords, nor "Stab (Eb)" or "Lost in A Dream", where a lone note
  may or may not be the key). Words that only look like notes are words: "R&B", "D&B", "Who I Am", "E Piano". A lone
  note after "in" or "of" is a key only at the end of the name or before a number ("Pad in F 90bpm"). A file whose
  name says nothing about notes takes the key of the nearest folder (up to three up) that names one.
- Each part is a piece, heard whole as the MIDI page hears it: as a loop, or as a window when the page cut it from a
  part longer than 16 bars. Its pack (the first folder below the one given; the files right inside a folder given are
  one pack) is its group, and a pack's files share a fold. The summary warns when there are fewer than 10 packs or one
  holds more than a third of the pieces, since a refit would have little to learn from and check on. Their licence
  says they're Solomon's own, so a plain keyfit run only measures them. The file can't be written inside a copy of
  this repository.
- **`--shipped`** scores the finder built in, as it is, on folds 1 to 4: what a refit has to beat. Its folds are drawn
  as the run it's compared with draws them (a rescale's when his pieces are read, a plain run's when they aren't), so
  `--dump` and `--against` pair the two case by case.
- **`--rescale`** fits only the allowed numbers, on his pieces alone (anything else read is only measured): the
  built-in finder's weights for each stream times one number, and the minor weight, then the calibration, each held
  near its built-in value by the penalty. The tables and corrections inside those weights, the answers' thresholds and
  the share of notes in one scale stay as built in.
  - A stream scale or calibration number that fewer than 30 pieces sound keeps its built-in number, and so does the
    minor weight unless 30 pieces are labelled major and 30 minor. Pieces with the same notes in other keys (a loop
    sold in every key) count once there, and share one piece's weight in the fit.
  - The calibration's curve over bars moves only when 30 full mixes (several parts together) have it: a part alone
    adds its own number to the curve, so parts alone can't tell the two apart, and full mixes (the Songs page) would
    pay for it.
  - It stops if his pieces fall in fewer than two of folds 1 to 4. The ranges draw a pack's files again together, as
    one, since they're alike.
  - **Pulled back.** Every finder a rescale scores or writes (each fold's in a choosing run, the one `--final` scores,
    the one `--write` writes, and each fold's in `--write`'s report) is pulled back toward the finder built in: each
    weight and calibration number goes a share t of the way from its built-in value to the one fitted, the same t for
    all of them, and the thresholds stay as built in. t is the largest of 1, 0.95, 0.9 and so on down to 0 at which
    every hand case keeps its gate, so a fit that keeps every gate stays as it is (t = 1). The run prints t for each
    finder, with the scales it was pulled back to and the gates it broke one step further, then scores the pieces held
    out with the numbers pulled back. A calibration is fitted on scores of the fits, as before, then pulled back with
    the weights it goes with: the one a choosing run prints for all four folds is as fitted, before the pull-back, as
    `--write` fits the one it writes, and the answers printed are the thresholds built in as they come out on the
    chances the pieces were scored with, pulled back. Why: on his packs (2026-11-03, folds 1 to 4), nearly all minor
    (81% of the whole loops), the plain rescale moved the minor weight from -0.49 to between 1.6 and 1.9 and the scales
    of the tune, harmony, bass and chord streams to between 0.25 and 0.64, and broke the gates of 12 of the 29 hand
    cases that have one (mostly a major key pushed below a minor one, C major below A or D minor, Eb major below F
    minor, and the answer turned to no clear key). Pulled back (t 0.50 or 0.55 in each fold), every gate held, and the
    whole loops were right 56.0% of the time, against 53.3% for the plain rescale and 46.1% for the finder built in;
    every set got better beyond its range. Each number pulled back lies between its built-in value and the one fitted on
    his packs, so it's still one of the numbers rule 5's exception allows.
  - What to watch for. t is found from the fit back, so a finder can keep every gate at t and break one again further
    back, nearer the finder built in: the run says so ("but further back, at 0.40, nearer the finder built in, a gate
    breaks again"), and the finder it stopped at still keeps every gate. How sure a reading is comes from the weights
    and the calibration multiplied, so a finder part of the way can be surer than either end; the held-out report
    measures each finder as pulled back, so it would show there (in that trial, the whole loops' calibration error was
    0.037 pulled back, against 0.107 for the plain rescale and 0.119 for the finder built in). Pulled back part of the
    way, the numbers written are a step from breaking a gate, so a later change to the finder or to a hand case may
    break one: mo-harmony's tests say which, and the rescale is run again.
  - `--write` keeps the credits of the weights it scales, adds the rescale, how far it was pulled back and the scales
    of the numbers written after them, and adds `packs` to `LEARNED_FROM`. Pulled back all the way (t = 0), it writes
    nothing, since only the numbers built in keep every gate. A `--dump` of a run that read his pieces names his
    files, so it can't be written inside a copy of this repository either.
- **Keep rule**, as for a design change: on folds 1 to 4 against `--shipped`, the whole loops' accuracy rises with its
  range above 0, or their log-loss falls by 0.01 or more; no set gets worse beyond its range; no gate breaks (the
  pull-back sees to that one). Then `--final` once, and `--write`. Back in the cloud, the public sets are scored with
  `--shipped` before and after the new weights, to see what the rescale costs songs.
- **Shipped as version 5** (2026-11-03): whole loops 46.1% to 56.0% right on folds 1 to 4, every set better beyond its
  range, though the loops labelled major fell from 45.9% to 29.9%. On the public songs, whole songs lose little
  (POP909-CL nothing, RS200 1.2 points, Billboard 4.4), but stretches of 1 to 16 bars lose 3 to 22 points, and a tune
  alone more. The numbers, and who pays, are in `docs/harmony/key-finder-experiments.md`.
- **Rescaling again.** `--rescale` won't start while the weights built in were rescaled already: it would scale the
  scales, and `--final` would spend a look at fold 0 on weights `--write` won't write. So a later rescale starts from
  version 4's, the plain fit: put them back first (`git show 3be3d26:crates/mo-harmony/src/keyfind/weights.rs >
  crates/mo-harmony/src/keyfind/weights.rs`), build, then run the steps above. The runs then name the finder built in
  "version 5, with version 4's numbers", in the report and in `test-runs.md`. Their `--shipped` scores version 4; to
  compare with what ships, make the dump before putting version 4's weights back.

## Chords

The C# app's chord reader runs unchanged through the small C# helper in `cs/`, which needs .NET 10 and a clone of the
C# app (on Ubuntu, `apt-get install dotnet-sdk-10.0`):

```
dotnet build tools/theorybench/cs -c Release -o path/to/helper -p:MoOldRepo=path/to/Music-Organiser -p:MoBuildNative=false -p:MoBuildBitwig=false
dotnet path/to/helper/theorybench-cs.dll chords path/to/bench/pop909.jsonl --part accompaniment > cs-chords.jsonl
cargo run -p mo-theorybench --release -- chords path/to/bench/pop909.jsonl --answers "C# app=cs-chords.jsonl" --part accompaniment
```

Each `--answers name=file` is one engine's chords for every piece; the first is the baseline the others are compared
with. The chords are scored exactly as `mir_eval.chord` scores chord recognition, so the numbers mean what published
ones mean: each stretch of time weighted by its length in beats, pooled over all the songs. `--per-segment` scores
them per labelled chord too (each counts once, right when right for more than half its time).

The C# app is scored by the name it shows (and mo-harmony by the name its chart shows), read back with mo-harmony
(`Chord::parse`, on the root and over the bass it gives), not by the notes it heard: "C7" names a G even when none
sounds, and "Gsus4" for C/G leaves the E out, as its page shows them. A name stands for a few notes a chord may leave
out, as players read names and as mo-harmony's own names do: the plain fifth, the 9 under an 11 or 13, and the 11 of a
minor 13. Those count only where the label has them, so "Cm13" is right for a minor 13 with no 11 and "Cm11" for a minor
7 with an 11 and no 9. A 7alt says it is altered but not how, so it can be right as the event, never as the name.

The helper also writes, for each chord, the other names the C# app's Playground offers for the same notes (on its
other roots, at most four), so its pick list can be scored. With `--oracle-cuts` it reads each labelled chord's
stretch on its own and keeps the chord it read there for longest: its naming when the changes are given, apart from
where it would change chord. Give the bench the same `--part` as the helper, so it knows which notes were heard.

`--engine rust` runs mo-harmony's chord reader in the bench, on the same notes: the parts `--part` gives (all but
drums when left out), each with the role its dataset gives (melody, bass, chords), as the MIDI page hears a file's
parts. `--roles off` hears them all as one part with no role, as the C# app does. Each piece is read whole, as a song
(or as a loop or a window: Solomon's packs), and each stretch's chord is scored by the name its chart shows, as the C#
app's are, with its next two readings as its list. `--oracle-cuts` gives it the labelled changes (a pin at each
label's ends, so each label is read as one chord), as the helper's `--oracle-cuts` does for the C# app. `--timing`
reads on one core, one piece after another, and says how long a read takes, a piece and 16 bars of it (the design's
budget: 3 ms for a dense 16-bar part, 99 reads in 100):

```
cargo run -p mo-theorybench --release -- chords path/to/bench/pop909cl.jsonl --answers "C# app=cs-chords.jsonl" --engine rust --part accompaniment --timing
```

Then come the scores the chord reader's design adds (harmony/design/chord-reader-design.md), over the time the labels
name a chord. Each label is read whole, ninths, elevenths and thirteenths and all (mir_eval drops them), as
`convert/charts.py` plays them (a 13 with no 11):

- **notes+bass**: every note the label names and the bass, on any root. **name**: the same on the right root, with the
  same kind of bass (D7/C is not D/C), the whole chord. **event**: the root, the third, the seventh (or a 6), an altered
  fifth and the bass and its kind, tensions left out.
  **lenient**: the event, where a plain major label also takes a maj7 or 6, a minor one an m7 or m6 and a sus4 a 7sus4
  (POP909-CL labels by triad templates while the piano plays more). Each also per labelled chord.
- **Each extension**: precision, recall and F1 for the 6 (of a chord with no seventh), b9, 9, #9, 11, #11, b13 and 13,
  wherever they are: a hit is the same degree on the same root, so a reading on a wrong root misses every extension.
  Also the altered tones pooled, and the F1 averaged over the extensions the labels have (macro F1), with its range.
- **The root among the same notes**: where the notes and bass were read right, how often the root was too (C6 or Am7,
  Fmaj7/D or Dm9), and the commonest other roots taken.
- **The bass kind**: root position, inversion (the bass one of the chord's notes: C/E, G7/F) or slash (only a bass:
  D/C), the label's against the reading's, with the inversions read in root position.
- **The root no part plays**: root accuracy where the label's root sounds in none of the parts heard (rootless
  voicings), with and without a bass part.
- **Lists**: how often the labelled chord, as an event, is an engine's first reading, among its first two or three, and
  the mean reciprocal rank.

- **root**: the right root. **majmin**: the right root and major or minor (where the label is major, minor or no
  chord). **thirds**, **triads**, **sevenths**, **tetrads**: more of the chord right each time (tensions like 9ths and
  13ths are never scored, as mir_eval leaves them out). **mirex**: at least three notes in common. **+bass**: the
  bass right as well. **bass**: the bass note alone.
- **over-cut** and **under-cut**: how much the chords read chop up the labelled ones, or run them together;
  **changes a bar**: how often the reader changes chord.

`--by tag` splits the chords named whole by a tag `convert/voice.py` gives each chord it plays (below): set, recipe,
texture, tune, push, chart or quality, and context and role in its sweep. Given twice, it splits by both:
`--by recipe --by texture`.

`--dump file` writes every piece's labels, chords read and scores; `python3 -I convert/check_mir_eval.py file` runs
mir_eval itself on them and prints both sets of numbers (they matched exactly on all 909 POP909 songs, 2026-10-31, and
on POP909-CL pooled in beats, 2026-11-01).

### The chord reader's candidates

mo-harmony's chord reader scores only the readings the notes let in for each stretch of music (its candidates), so a
chord missing from them can never be read. `candidates` measures how often the labelled chord is among them:

```
cargo run -p mo-theorybench --release -- candidates path/to/bench/pop909cl.jsonl --inputs piano,full,upper --misses misses.tsv
```

Each labelled chord is lined up with the units the reader hears and cut into stretches of at most two bars, the longest
it scores; a label so short that both its ends land on the same unit start is left out, and the report says how much
time those take. The rows add the ways in one at a time: the tables (the root played with the notes that say which chord
it is, or a rootless seventh chord), open fifths (a root heard with its fifth and no third tries the major and the minor
triad), and the escape (the label whose own notes sound longest of all the rest): the reader's candidates. The last row
moves the labelled chord's ends by up to a unit (keeping to two bars), as the reader finds its own ends: what's missed
even then, it can't read, and `--misses` lists those stretches. On POP909-CL the design's accept sets apply (a major
label also takes a maj7 or 6, and so on, as in **lenient** above).

### The chord reader's counted tables

The chord reader's priors read counted tables (harmony/design/chord-reader-design.md, P1 to P3): how often each kind
of chord is the labelled one (f_core), how often a chord of each kind names each tension it can carry (f_ten), and how
the labelled chords move from one to the next (f_motion). `tables` counts them from the labels alone, never from what a
reader hears:

```
cargo run -p mo-theorybench --release -- tables path/to/bench/pop909cl.jsonl path/to/bench/billboard.jsonl --write crates/mo-harmony/src/chordread/tables.rs
```

- **In beats**, on folds 1 to 4 of the pieces whose licence lets numbers ship (fold 0 stays kept back, drawn as keyfit
  draws it). Each source counts as much as any other (its beats scaled to the sources' mean), and half a beat is
  added to each cell, so nothing is ever impossible.
- **f_motion in changes of chord**: a label read as another root or core than the one before it (C to C/E, or G7 to
  G9, is no change), counted by how far its root moves up and by the group of each chord (a dominant 7 or 7sus4; a
  diminished or augmented chord, whose altered fifth says which it is: dim, m7b5, dim7, +; or any other). No chord, a
  label that can't be read, or one that lasts no time, breaks the line. Only the sources whose labels name every kind
  of chord count: one that can't name a 6 or a 7sus4 reads C to C6 as no change, and G7sus4 to C as G7 or Gsus4 to C.
  Each source counts as much as any other (its changes scaled to the sources' mean), half a change is added to each
  cell, and each group's 36 shares add up to 1.
- **A source counts only for what its labels can say.** Each kind of chord is counted from the sources whose labels
  name it, and the shares are then scaled to add up to 1: POP909-CL has no 6, m6, 7sus4 or power chord, so it says
  nothing about how common they are. f_ten comes only from the sources whose labels name a 9 on at least 1% of their
  time (Billboard 5.2%): POP909-CL labels by triad and seventh templates, so its labels never say a tension is played,
  even where the piano plays one, and one stray label can't change that.
- **A dominant 11** (Harte "11": a major third and an 11 with a ♭7, and no ♯9) is counted as the 9sus4 players voice,
  as `voice.py` plays it.
- Give it the labelled sources themselves, not `voice.py`'s renders of the same charts, which would count Billboard's
  labels twice. Without `--write` it only prints the counts and the tables.

### Fitting the chord reader (`chordfit`)

```
cargo run -p mo-theorybench --release -- chordfit path/to/bench/pop909cl.jsonl path/to/voiced/billboard-A.jsonl --out fitted.json
cargo run -p mo-theorybench --release -- chords path/to/bench/pop909cl.jsonl --engine rust --part accompaniment --weights fitted.json --folds 1
```

`chordfit` fits the chord reader's 39 numbers (harmony/design/chord-reader-design.md, "Model, training and data"): the
weight of each piece of evidence (T1 to E8), the trusts in the two template priors (P1, P2) and in the pair term, and
w_N, what a stretch read as no chord scores. The counted tables stay as `tables` counted them.

- **Folds.** Drawn as keyfit draws them, from each piece's name (fold 0 kept back), with one difference: a piece only
  measured goes where a piece learned from with its name goes (`voice.py`'s set B, only measured, where set A's render
  of the same chart goes), or else one with its notes in some key, or else where its own name goes; so a fit is never
  measured on a chart it learned. It learns from folds 2 to 4 (`--train`) of the pieces whose licence lets fitted
  numbers ship, and measures fold 1 (`--test`). `chords --folds 1` scores the same pieces, but for one case it says
  when it meets: a chart learned from that shares its notes with one named before it goes to that one's fold, and its
  renders read without it go where their own name goes.
- **How each piece is heard.** A dataset's song (POP909-CL) four ways (`--inputs`), all from its own piano's notes:
  the piano (`piano`); with the tune (`full`); with the piano's bass line as a bass part (`split`: the notes the
  reader would hear as the bass, each the lowest sounding with nothing of another pitch class within a fifth above it
  at some note's start, or alone and carrying on the bass before it, so a figure played alone stays in the rest); and
  the rest of the piano with no bass (`upper`), where the roots often aren't played. A piece `voice.py` played is heard as given, each part with its
  role (set A, CC0, is learned from). Each reading counts 1 unless `--weigh` says otherwise (`pop909cl/upper=0` drops
  one way), so a Billboard song, heard one way, counts a quarter of a POP909-CL song heard four: the design's weight
  for set A.
- **What a label accepts.** Its root and core. On POP909-CL, whose labels are a template's triads while the piano
  often plays more, a major label also accepts a maj7 or 6, a minor one an m7 or m6, and a sus4 a 7sus4. A dominant 11
  also accepts the 9sus4 `voice.py` plays it as; a diminished seventh accepts its four roots and an augmented triad
  its three (the same notes, which the reader names after it decodes). No chord, a label that can't be read, or a
  chord the reader can't read (a root alone) leaves its stretch free.
- **What it makes likelier.** For each piece, the chance the reader gives the ways of reading it that the labels
  allow, among every way it can read it: each reading one its stretch accepts (reaching at most a unit, half a beat,
  into a stretch labelled otherwise), changing chord only within a unit of a labelled change. A labelled change with
  no place within a unit where the reader can change chord (the labels' time is off, or the music holds on through
  it) moves to the nearest place it can: 0.6% of POP909-CL's labelled ends. The report's **loss per bar** is minus
  the log of that chance, per bar of music, over the pieces (each weighed), plus a pull towards the numbers it starts
  from (`--lambda`, 1e-3): lower is better, 0 would be every labelled reading certain.
- **How.** Projected quasi-Newton: each step makes a quadratic picture of the loss from its slope and a curvature it
  learns as it goes (started from the slopes' outer products, piece by piece), finds that picture's lowest point
  among the numbers that keep every rule the weights keep (`chordread::RULES`: signs and orders, such as T2 between 0
  and T1), and moves towards it as far as the loss falls enough. Every point it reads keeps the rules. The pieces'
  sums are added in their order, so a run gives the same bits on any number of threads. At most 40 steps
  (`--steps`), fewer when the loss falls by less than a hundred-thousandth of itself three steps running;
  `--fix P1,P2` holds numbers where they start; `--from` starts from a file a fit wrote.
- **The report.** The loss at each step; each number at the start and fitted, and which sit on a rule's bound; the
  loss per bar of each source and way of hearing, at the start and fitted, on the pieces learned from and on those
  held out; and a `SET=` line that reads the hand cases with the fitted numbers (`cargo test -p mo-harmony --release
  show_the_cases_read_whole -- --ignored --nocapture`, and `show_the_grid`). `--out` writes the numbers, by name, for
  `chords --weights` and `chordfit --from`.
- **The hand cases' gates.** After every fit, mo-harmony's hand cases (`chordread::cases`) are read with the numbers it
  started from and the fitted ones, one stretch at a time, read whole and named: the report gives how many gates break
  at each step, every gate the fitted numbers break (with what they read), and how many of the design's targets are met
  (naming those the fit newly meets or loses); the `--out` file says which gates break ("gates"), as weights that break
  one aren't shipped. The cases the design holds out (G\*: 6, 15, 32, 34 and 35, gates and the target 15e) are never
  learned from nor looked at here, nor counted among the targets; the tests check them.
- **Learning from the hand cases.** `--hand μ` adds the hand cases to the aim: each music the cases read whole (but
  the held-out ones) is one piece, labelled where its gates say what to read: what a stretch must read (the cases
  only a whole read decides, and each stretch case's first reading), or the two readings that must come first (one
  label accepting either: it makes the two likelier between them, which is less than the gate asks, as one of them
  may still be starved). A gate that only asks for readings to be kept says nothing about what's read, so it's
  checked but not learned from. The aim adds μ times the pieces' mean −ln P(labels) to the data's loss per bar, its
  slope and curvature added the same way. `--hold n` keeps the gates as straight-line rules instead: at the start,
  each piece's labels must keep a chance of at least 0.5 (or what they have, if less) as the slope there sees it (a
  half-space, kept by the projection with the signs and orders). After each fit, for at most n more rounds, every
  rule is drawn again where the fit got to: a piece whose gates still break (found by its music, whichever way the
  gate reads it) must get likelier, to a chance of 0.75, then 0.875, but never more than it had at the start (from
  the hand-set weights every gate holds there; starting `--from` numbers that break gates, it says so, as those may
  stay broken); every other keeps that chance, or what it has if less. The rounds stop early when every gate holds,
  when no gate that breaks reads music the fit learns from (8 named cases have music of their own, and cases 12, 13
  and 20 only ask for readings to be kept: their gates are checked, never held), when the pieces whose gates break
  are already as likely as they're asked to be (a gate wanting two readings on top can break that way), or when the
  rules leave no numbers that keep them all. Each round takes at most `--steps` steps; the `--out` file says how
  many rounds were fitted and why they stopped. The two can go together.

## Charts played as music (`convert/voice.py`)

`charts.py` plays Billboard's and RS200's charts as block chords over the label's bass: every note the label names,
held. Music is rarely that tidy, so `voice.py` plays the same charts again the way a pianist and a bass player might,
and labels each chord with exactly what it plays. The bench can then tell which voicings and bass lines throw a reader.

```
python3 -I tools/theorybench/convert/voice.py charts path/to/bench/billboard.jsonl path/to/bench/billboard-A.jsonl --set A
python3 -I tools/theorybench/convert/voice.py charts path/to/bench/billboard.jsonl path/to/bench/billboard-B.jsonl --set B
python3 -I tools/theorybench/convert/voice.py sweep path/to/bench/sweep.jsonl
python3 -I tools/theorybench/convert/voice.py check path/to/bench/billboard-A.jsonl
python3 -I tools/theorybench/convert/test_voice.py
```

- **Voicings, one recipe a section (8 bars).** Set A: close root position, the chart's own inversions, drop 2, shells
  (1-3-7 or 1-7-3 with the tensions on top) and gospel (an octave bass under a close right hand). Set B: rootless A
  and B, upper-structure triads, quartal and So What, arpeggios and Alberti, two-handed spreads and clusters. A recipe
  that doesn't fit a chord (a rootless voicing needs a seventh and a 9 or 13) gives way to one that does.
- **Bass, one texture a section:** a held root, a root-fifth two-feel, a walking line (with half-step approaches), a
  pedal on the key's tonic, the chart's own inversions, or no bass.
- **A tune** in about half the sections, with passing and neighbour notes; and **pushes**: a fifth of the chord changes
  come an eighth early in the keys, the bass still on the beat.
- **The labels say what is played.** A pedal makes a slash chord, a held root takes the chart's inversion away, and
  Harte's 11 (a third under an 11th, which players avoid) is played and named as a 9sus4. The fifth may be left out and
  a rootless voicing leaves out the root, but the label keeps both, as a chart would.
- **Tags:** every note says what it is (chord, ext, passing, neighbour, approach, walking or push), and every chord its
  set, recipe, texture, tune, push, the chart's own label and the quality played, for `chords --by`; in the sweep also
  its context and role (the chord swept, or the ii or V before it).
- **The sweep:** 45 chord qualities (sixths, ninths, elevenths, thirteenths, altered dominants, sus, diminished and
  augmented chords) in every voicing that fits them, over every bass texture, on all 12 roots, alone and as the I of a
  ii-V-I: 5,423 pieces.
- **check** plays every labelled chord back against its label: every note is in it, every label note sounds (but a
  fifth or a rootless root), and the bass is where the texture puts it. `charts` and `sweep` run it on what they write;
  every chord in all five files plays as labelled. `test_voice.py` checks the recipes and textures, and reads every
  label with `charts.py` too, which was written apart.

Every choice comes from a hash of the song and the section, so each run writes the same files. Set A keeps the chart's
licence (Billboard's may be learned from, at a quarter weight, as Solomon chose for charts); set B and the sweep are
for measuring only, so the reader is tested on voicings it never learned from.

## The C# baseline on POP909 (2026-10-31)

Keys, 8-bar clips (8,850 clips from 909 songs): the C# app is right 77.7% of the time (76.0 to 79.3), MIREX 83.2;
its most common miss is the relative key (13.3% of clips). Whole songs with one key: 90.8%. The Krumhansl-Kessler
profile gets 66.0% of the clips, as the C# bench measured. Melody alone 71.9%, accompaniment alone 69.6%.

Chords, accompaniment part, all 909 songs: root 88.5%, majmin 86.7%, sevenths 84.3%, mirex 86.9%, bass 90.7%;
segmentation 91.2, 1.36 chord changes a bar (the labels change 1.47 times a bar). Until 2026-11-01 the bench scored
the notes the C# app heard rather than the name it shows (majmin 81.5%, sevenths 79.4%, mirex 82.1%): it hears notes
its names leave out, such as passing notes.

## The C# baseline on POP909-CL (2026-10-31)

Against the keys and chords people checked, the C# app does a little worse than against POP909's own labels, which
were made by software much like it. Keys: whole songs with one key 87.8% (85.5 to 90.1; the Aarden-Essen profile
alone gets 87.7%), songs that change key 69.5%; clips of 1, 2, 4 and 8 bars 29.7%, 44.7%, 62.2% and 75.2%, the
relative key still the most common miss. Chords, accompaniment part, all 906 songs, by the names it shows: root 88.0%,
majmin 86.6%, sevenths 82.4%, mirex 87.9%, bass 92.8%; segmentation 93.9 (mir_eval gives the same numbers). By the
notes it heard, as first measured: majmin 81.7%, sevenths 77.9%, mirex 82.9%.

## The C# baseline on the chord scores (2026-11-01)

What Solomon found most wrong in the C# app was chords: extensions, roots and inversions. Measured before any new code,
by the names it shows (#107 first measured the notes it heard):

- **POP909-CL, accompaniment part:** notes+bass 75.9% (74.8 to 76.9), name 74.9%, event 78.6%, lenient 80.4%. With
  the changes given (`--oracle-cuts`), notes+bass is 3.7 points higher (3.4 to 4.0). Where the notes and bass were
  right, the root was too 98.6% of the time. Inversions are read in root position 73.0% of their time. Where the
  label's root sounds in no part (1.5% of the time), the root is right 3.7% of it. The labelled chord is the first
  reading 78.6% of the time and among the first three 80.1%. Its labels name no extensions. Piano and tune together:
  notes+bass 55.1% (tune notes go into the names), lenient event 77.4%.
- **Billboard charts, played as block chords over the label's bass (CC0):** notes+bass 94.2% (93.3 to 95.0), name
  90.8%, event 91.1%. Extensions, F1: 6 68.1% (precision 55.3%), b9 59.6%, 9 82.9%, #9 98.3%, 11 74.0%, #11 82.7%,
  b13 41.8% (precision 28.8%), 13 75.1%; altered tones pooled 85.4%; macro F1 72.8% (67.4 to 77.0). Nearly all its
  false 6ths (95.5% of their time) and most of its false b13s (68.1%) are readings on another root, mostly
  inversions: Dm/F read as F6, Bm/A as D6/A, Fmaj7/A as Am(b13). Inversions are read in root position 59.7% of their
  time. Slash chords (2.7% of the time) are never written as slashes: 69.0% are read with the bass as the root and
  31.0% as the chord with the bass added (D7/C for D/C). Where the notes and bass were right, the root was too 97.0%
  of the time; the commonest other root is C/D read as D9sus4 (0.7% of the chord time).
- **RS200 rock charts, played the same way (measuring only):** notes+bass 98.6%, name 97.4%; inversions read in root
  position 49.2% of their time.

The notes+bass and name numbers were checked with a separate Python line-up that reads the labels with `charts.py`
(the same numbers on all three sets and on the played charts below, pooled in beats, with the bass kind in the name
and the notes a name may leave out).

## The C# baseline on charts played as music (2026-11-01)

The same charts played by `voice.py`, and its sweep. Played like music, the C# app names a chord whole about 20 points
less often than as block chords, and most of the tensions it names aren't in the chord.

- **Billboard, set A (the voicings a reader may learn from):** notes+bass 77.6% (76.7 to 78.4), name 71.7%, event 74.4%
  (block chords: 94.2%, 90.8%, 91.1%). Extensions: macro F1 33.6% (29.9 to 36.3); of the tensions it names, only 46.5%
  of its 9ths are in the chord, 12.2% of its 11ths and 13.5% of its 13ths. Where it names a tension the chord hasn't, on
  the right root, the note is the tune's neighbour (28.0%) and passing notes (10.4%), the walking bass's passing (23.1%)
  and approach notes (17.1%), a note of the next chord pushed early (5.2%), or one from the chord before or after, where
  its reading runs over the change (15.3%). Inversions are read in root position 77.8% of their time. Given the changes,
  it does no better (name 0.9 points lower).
  - By bass (name): a held root 93.6%, two-feel 92.5%, the chart's inversions 88.1%, no bass 75.4%, a pedal 42.9%
    (its notes and bass right 71.7%: the pedal is taken as the root), walking 38.9%.
  - By voicing: shells 76.0%, gospel 74.1%, the chart's inversions 73.6%, close 68.1%, drop 2 66.9%. With a tune
    68.2%, without 75.3%. Pushed changes make no difference (71.9% against 71.7%).
- **Billboard, set B (measuring only):** notes+bass 67.3% (66.2 to 68.2), name 62.9%, event 66.7%; macro F1 26.2%.
  Alberti 47.8%, arpeggios 55.7%, spreads 70.6%, clusters 74.3%; the rootless, upper-structure, quartal and So What
  voicings (2.6% of the time: most chart chords have no 9 or 13) 46.7% to 58.9%. Walking 29.3%, a pedal 37.5%.
- **RS200, played the same way:** set A notes+bass 81.2%, name 75.1%; set B 72.5% and 68.2% (block chords: 98.6% and
  97.4%).
- **The sweep (45 qualities, every voicing and bass):** every chord, notes+bass 63.5% (62.5 to 64.4), name 59.1%,
  event 61.5%; macro F1 56.8% (55.6 to 57.9); inversions read in root position 63.6%. Counting only the chords swept,
  not the ii and V before them (plain m7 and 7 chords; `--by role`): notes+bass 59.4%, name 53.6%, event 57.1%; on
  their own 51.3%, as the I of a ii-V-I 55.6%. The splits below are of the chords swept too (`--by role` with
  `--by texture` or `--by recipe`):
  - By bass: a held root 77.6%, a pedal (in a ii-V-I, on the I's own root) 77.7%, two-feel 65.0%, no bass 54.7%,
    the chord's inversions 32.3%, walking 26.5%. Over that pedal, the V is never named right and the ii 29.3% of the
    time.
  - By voicing: spread 69.5%, gospel 68.1%, cluster 67.8%, shells 65.7%, close 65.5%, the chord's inversions 65.4%,
    So What 60.5%, drop 2 60.4%, quartal 52.5%, upper structures 37.6%, rootless 34.4% and 34.1%, arpeggios 27.9%,
    Alberti 14.2%.
  - By quality, best: major 79.0%, minor 76.7%, minor-major 7th 76.3%, maj7 74.8%, 7 73.7%. Worst: a dominant 9 or 13
    with a #11 21.2% and 19.0% (it writes the #11 as a b5 when no fifth sounds, as mo-harmony's own names do; the
    labels write #11, as charts do), minor add 9 3.8% (read as Ebmaj13/C for Cm(add9): the right notes from the wrong
    root, 66.5% notes+bass), 7(#9,b13) and 7(b9,b13) 0.0% (named 7alt, which doesn't say which alterations; the event
    is right 55.8% and 57.8%), and power chords 1.5% (its changes come a bar late, so C5 then F5 is read as Csus4).

## The chord reader's candidates (2026-11-01)

The labelled chord among the reader's candidates, as a share of the time (`candidates`; the experiments behind the
choice are in harmony/experiments/chord-reader-experiments.md, E0):

| POP909-CL | accompaniment | with the tune | upper part, no bass |
|---|---|---|---|
| the tables alone | 89.19 | 91.34 | 84.23 |
| + open fifths | 95.43 | 96.25 | 89.87 |
| + the escape: the reader's candidates | 96.99 | 97.43 | 91.48 |
| the reader's, ends within a unit | 98.89 | 99.07 | 95.97 |

On every set `voice.py` plays (Billboard and RS200, sets A and B, and the sweep), every row is 99.99% or more for the
accompaniment and with the tune, and 93.6% to 99.3% within a unit for the upper part alone. There are 8 to 15 candidates
a stretch on average (23 in the sweep's dense chords). Labels too short to place are rare: 20 on POP909-CL, under 0.01%
of the time, and at most one a set elsewhere. What POP909-CL misses even with the ends moved is mostly a stretch whose
notes don't hold the chord: one or two notes under a full label, a maj7 label over a plain triad, a sus2 label over an
open fifth. With no bass part, the chords' roots often aren't played at all: that's most of the upper part's misses.

## The chord reader's counted tables (2026-11-01)

`tables` on POP909-CL (727 pieces on folds 1 to 4) and the Billboard charts (571), shipped as mo-harmony's
`chordread::COUNTED`:

- **f_core, the share of labelled time:** major 52.4%, minor 23.9%, minor 7 6.6%, dominant 7 6.3%, major 7 2.4%, 7sus4
  2.0%, sus4 1.7%, power chords 1.7%, 6 1.1%, sus2 1.0%, diminished 0.3%, m6 0.25%, m7b5 0.2%, and augmented, m(maj7)
  and dim7 under 0.1% each. The 6, m6, 7sus4 and power chords come from Billboard alone; counted from both sources,
  they came out at half that.
- **f_ten, from Billboard's labels:** a 9 is named on 68% of 7sus4 chords, 23% of 6 chords, 20% of major 7s, 9.4% of
  minor 7s and 6.2% of dominant 7s (a ♯9 7.2%, a 13 3.7%, a ♭9 1.2%); a ♭13 on a minor 7 almost never (0.04%).
- **f_motion, from Billboard's 53,491 changes** (POP909-CL's labels can't name a 6, m6, 7sus4 or power chord). Each
  share is of the changes from one group, to a chord of the group named. Of the 6,232 from a dominant, 42.4% go up a
  fourth to another chord (V to I) and 8.7% up a fourth to another dominant (a chain of dominants), while a tritone
  sub falling a semitone is rare in this music (0.3%). Of the 331 from a diminished or augmented chord, 15.0% rise a
  semitone to another chord and 8.5% fall one, against 1.6% of the other chords' changes rising a semitone. Of the
  46,928 from any other chord, 21.4% go up a fourth to another such chord, 20.2% a fifth and 14.4% a tone.

The reader trusts them a little, a beat: 0.2 for f_core and 0.06 for f_ten, where the design's hand cases' smallest lead
is largest (0.26 a beat, against 0.10 trusting neither; `cargo test -p mo-harmony show_the_trusts -- --ignored
--nocapture` prints the grid). Labels name tensions far less often than players play them, so f_ten makes every tension
cost a little; trusted much more, it breaks hand cases (G C F over D reads Fsus2, not Dm11). No trust up to 0.5 brings
the rootless voicings with no bass (the design's cases 12, 13 and 20) into the top two or three: those wait for the pair
term, which reads the chords around. The experiments are in harmony/experiments/chord-reader-experiments.md, E1.

## mo-harmony beside the C# app (2026-11-02)

mo-harmony's chord reader as it stands, before chordfit fits anything (its weights set by hand), on the same notes as
the C# app and scored the same way, by the names each shows (`chords --engine rust`). Played like music, it names the
chord whole 10 to 19 points more often than the C# app. On POP909-CL's piano part it is 2 points behind; with the tune
heard as a tune, 14 points ahead.

- **Charts played as music (`voice.py`):**
  - Billboard set A: notes+bass 89.0% against 77.6% (+11.4, +10.8 to +12.1), name 84.8% against 71.7%, event 85.4%
    against 74.4%; extensions' macro F1 53.2% against 33.6% (+19.6). Inversions are read in root position 36.9% of their
    time (C#: 77.8%), root position as an inversion or slash 0.5% (6.4%). The labelled chord is the first reading 85.4%
    of the time and among the first three 90.9% (74.4% and 79.5%).
    - By bass (name): walking 78.3% (C#: 38.9%), a pedal 72.2% (42.9%), no bass 82.1% (75.4%), the chart's inversions
      93.1% (88.1%), a held root 96.5% (93.6%). The root-fifth two-feel is its one loss, 86.6% (92.5%): over the fifth
      it often reads a new root for the bar's second half (C over G as Gsus4(add6), 4.1% of the two-feel time).
    - By voicing, every recipe gains, shells most (89.5% against 76.0%). With a tune 85.3% (68.2%), without 84.4%
      (75.3%): the tune no longer goes into the names. Pushed changes 83.7% (71.9%).
    - Given the changes (`--oracle-cuts`), name 88.9% against 70.9%.
  - Billboard set B: notes+bass 84.6% against 67.3% (+17.3), name 80.9% against 62.9%; macro F1 43.9% against 26.2%;
    inversions read in root position 19.6% (75.8%). Alberti 79.0% (47.8%), arpeggios 75.2% (55.7%), spreads 84.5%
    (70.6%), clusters 84.9% (74.3%); rootless 62.3% and 53.6% (49.8% and 46.7%), upper structures 68.1% (58.9%),
    quartal 70.5% (49.2%), So What 75.3% (49.3%). Walking 59.3% (29.3%), a pedal 71.2% (37.5%), two-feel 83.5% (71.2%).
    Given the changes, name 89.1% against 63.7%.
  - RS200: set A notes+bass 94.5% against 81.2% (+13.3), set B 90.7% against 72.5% (+18.2).
- **The sweep:** every chord, notes+bass 73.5% against 63.5% (+10.1), name 69.4% against 59.1%; macro F1 61.1% against
  56.8%. The chords swept: name 60.8% against 53.6%; the ii and V before them 88.1% and 82.2% (72.3% and 66.0%).
  - By bass (the chords swept, name): walking 54.3% (26.5%), the chord's inversions 47.4% (32.3%), held 83.7% (77.6%), a
    pedal 82.7% (77.7%), no bass 58.4% (54.7%); the two-feel falls, 49.3% (65.0%), its fifth taken as the root.
  - By voicing: Alberti 34.7% (14.2%), quartal 67.9% (52.5%), the rootless voicings 46.7% and 46.6% (34.4% and 34.1%),
    upper structures 46.4% (37.6%); shells the same (65.3%, 65.7%); arpeggios fall, 22.6% (27.9%): each piece of the
    arpeggio is read on its own, so a 7 chord is a triad until its seventh comes.
  - By quality, the most gained: power chords 65.5% (1.5%), m(add9) 64.2% (3.8%), 7(#9,b13) 29.9% and 7(b9,b13) 18.2%
    (0.0%: a 7alt never names them), 7(b13) 68.8% (48.6%), 7(#11) 75.5% (56.7%), m9 62.9% (45.6%), 9(#11) 37.0%
    (21.2%). Lost: maj7(#5) 42.1% (73.4%), m7(11) 54.4% (66.5%), maj13 51.0% (65.5%), m11 47.7% (58.3%), 9sus4 with a
    13 41.7% (51.0%), dim7 50.7% (59.2%), m6(9) 46.6% (54.7%).
  - Where the label's root sounds in no part (rootless voicings, 2.6% of the time), the root is still almost never
    right (1.7% with a bass part, 0.0% without): the design's pair term, which reads the chords around, isn't built yet.
- **Block chords (`charts.py`):** Billboard notes+bass 96.6% against 94.2% (+2.4), name 93.1% against 90.8%; macro F1
  76.6% against 72.8% (+3.8, +0.3 to +7.8), though its false b13s are as common (b13 F1 23.1% against 41.8%);
  inversions read in root position 27.3% (59.7%). Slash chords are still never written as slashes: 63.0% are read with
  the bass as the root. RS200 99.7% against 98.6%; its labels name almost no tensions, and its few b9s (0.1% of the
  time) are missed, so its macro F1 means nothing.
- **POP909-CL:**
  - The accompaniment part: notes+bass 74.0% against 75.9% (-1.9, -2.4 to -1.5), name 72.7% against 74.9%, event
    75.5% against 78.6%, lenient 79.0% against 80.4%. mir_eval: root 89.5% (+1.5), majmin+bass 83.5% (-1.8), sevenths
    79.5% (-2.9), bass 92.2% (-0.6); segmentation 92.6 (93.9), 1.38 changes a bar (labels 1.35). About half of what it
    loses is where the chords change (a labelled chord's main reading right but its edges wrong, 3.0% of the time
    against 1.9%); much of the rest is open fifths read as power chords (1.6% of the time), which POP909-CL's labels
    never name and where the C# app takes a triad.
  - Given the changes: notes+bass 79.2% against 79.7% (-0.5), root 93.2% (+5.2), bass 96.0% (+3.4).
  - Piano and tune together, each part with its role: notes+bass 69.1% against 55.1% (+14.0), name 67.8% against
    54.2%, sevenths 75.1% against 67.9%. With the roles off (every part one, as the C# app hears them), 45.4% (-9.7):
    knowing which part is the tune is worth 24 points here.
- **Speed (release, one core of a 2.1 GHz cloud machine, two runs):** a POP909-CL song's piano part reads in 37 ms in
  the middle and 74 to 81 ms 99 times in 100 (longest 94 to 124 ms); 16 bars of it in 7.6 to 7.7 ms and 13.6 to 14.8
  ms. That is about five times the budget (3 ms), as the design's speed notes found: the next speed step comes before
  the MIDI page reads with it. Sparse music is quick: RS200's block chords take 0.3 ms for 16 bars in the middle and
  1.5 ms 99 times in 100.
- **Against the design's merge rule** (decided later, on fold 0, after chordfit): on sets B and RS200 it passes every
  gate measured here (bass kind 94.7% against 82.7%; altered tones F1 53.9% against 32.2%; root and majmin+bass up
  12 to 15 points), but not rootless roots (under 90%). On POP909-CL it passes the root, not majmin+bass (-1.8),
  the bass (-0.6), sevenths (-2.9) or the segmentation (92.6, under 93.0). The two-feel, arpeggios and edges are
  where chordfit starts.

## The chord reader's first fit (2026-11-02)

`chordfit` on POP909-CL (each song heard four ways) and Billboard set A, learning from folds 2 to 4 (538 and 436
pieces) and measuring fold 1, from the hand-set numbers, `--lambda 1e-3`, 40 steps. These numbers come from the run
before the code review's fixes (how a render's fold is drawn, and the piano's bass line for `split` and `upper`), so
the same run now starts a little lower (a loss of 2.406 a bar against 2.418); run it again before leaning on them. At
the start, 7.1 million readings over 1.77 million units are kept only because the labels accept them, and 3,495 of
the 635,348 labelled ends (0.6%) move to the nearest place the reader can change chord. The loss per bar on fold 1,
held out, falls from 2.79 to 0.88 on POP909-CL's piano (full 2.71 to 0.87, split 2.61 to 0.87, upper 3.28 to 1.17)
and from 0.58 to 0.42 on set A.

Scored with `chords --weights` (fold 1 of POP909-CL and Billboard; RS200 and the sweep, never learned from, whole):

| set | majmin: C# / hand / fit | name: C# / hand / fit | extensions' macro F1: C# / hand / fit | inversions read in root position: C# / hand / fit |
|---|---|---|---|---|
| POP909-CL piano | 86.7 / 85.3 / 89.3 | 75.7 / 73.3 / 77.1 | - | 74.3 / 78.0 / 69.3 |
| POP909-CL with the tune | 84.0 / 85.4 / 90.3 | 56.1 / 68.3 / 77.1 | - | 70.8 / 77.2 / 69.5 |
| Billboard block chords | 96.2 / 98.1 / 98.8 | 91.2 / 92.4 / 91.6 | 73.2 / 71.0 / 69.5 | 59.0 / 27.2 / 18.0 |
| Billboard set A | 83.9 / 92.7 / 95.6 | 71.7 / 84.5 / 86.7 | 36.7 / 56.4 / 47.6 | 79.5 / 37.4 / 28.3 |
| Billboard set B | 75.6 / 88.0 / 95.6 | 61.7 / 79.2 / 87.1 | 28.9 / 44.8 / 49.1 | 78.0 / 19.6 / 7.3 |
| RS200 block chords | 97.6 / 98.5 / 99.6 | 97.4 / 98.2 / 99.1 | (too few) | 49.2 / 30.7 / 6.8 |
| RS200 set A | 84.5 / 96.0 / 97.8 | 75.1 / 90.9 / 92.9 | (too few) | 83.2 / 28.1 / 17.9 |
| RS200 set B | 78.2 / 91.9 / 98.0 | 68.2 / 87.4 / 94.6 | (too few) | 82.7 / 17.0 / 2.5 |
| The sweep | 65.4 / 77.1 / 77.6 | 59.1 / 69.4 / 66.8 | 56.8 / 61.1 / 52.1 | 63.6 / 40.6 / 23.4 |

- **What it gains.** Roots, chord kinds and inversions, on every set: POP909-CL's piano is now named ahead of the C#
  app (77.1% against 75.7%), and with the tune 77.1% (hand 68.3%). The hard basses gain most: on set A the two-feel
  is named 94.0% of its time (hand 84.5%) and walking 89.8% (80.3%); the sweep's segmentation goes from 84.8 to 95.0.
- **What it loses.** Extensions, and plain held chords: on set A, held chords are named 92.0% (hand 94.7%) and their
  extensions' F1 falls from 84.4% to 59.5% (the 13's recall from 96% to 34%); the sweep's name falls from 69.4% to
  66.8% and its macro F1 from 61.1% to 52.1%, behind the C# app.
- **The hand cases** (the `SET=` line): 27 gates break, none with the hand-set numbers. Cmaj9 is read as Em7 (h6, 34,
  49), C6 as Am7 (h1, h2, 22) and Fmaj9 as Am7 (22), Dm11 as G7sus4 (33), the 7alt and Db13 voicings as triads
  (17 to 19), passing diminished sevenths as dominant sevenths (6, 28, 29, 24), and walks under G13 and Cmaj7 as Em
  (37, 38).
- **Why, in the numbers.** The fit learns only what the labels show: POP909-CL's are triads and sevenths, and the
  loss counts a label's root and core, never its tensions. T1 (the label's own notes) rose from 3 to 5.23 and T2
  (tensions) fell from 1 to 0.36, so a reading that makes every note its own (Em7 on C E G B D) beats one with a
  tension (Cmaj9). R1 to R4 all went to 0, their rules' bound, so a played root no longer counts for itself; P1's
  trust rose from 0.11 to 0.34, so of two chords with the same notes the commoner core wins (Am7 over C6). Starting
  mid-bar costs far more (E5 from -1.2 to -3.11, E6 and E7 -4.27), so short chords merge (1.17 changes a bar on
  POP909-CL's piano, hand 1.32); w_N fell to -2.78. 13 of the 39 numbers sit on a rule's bound.
- **So these numbers aren't shipped.** Next, the fit keeps the hand cases' gates (on a stretch read whole, a gate is
  a margin between two readings' scores, a straight-line rule in the numbers, which the fit's projection can keep as
  it keeps the signs and orders), and the training mix is tried (the design's C12).

## The hand cases in the fit (2026-11-03)

The first fit run again (POP909-CL heard four ways and Billboard set A, learning from folds 2 to 4 and measuring fold 1,
from the hand-set numbers, `--lambda 1e-3`, 40 steps a round), now with every gate read: the 229 the fit looks at (66
one stretch at a time, 92 read whole, 71 named; the 15 the design holds out aren't). Three ways: plain (the first fit
with the code review's fixes), learning from the hand cases (`--hand 1`), and learning from them while holding their
gates (`--hand 1 --hold 3`). On 4 cores of a cloud machine a reading of every piece took 70 to 80 s, so a 40-step round
took about an hour; the third fit took 64 steps over two rounds (an hour and 40 minutes). The plain and `--hand 1` fits
ran before the second code review's fixes, which change only `--hold` and the targets counted (the design's held-out
15e, which no fit met, is now left out). The third fit's first try ended its second round crawling (steps a thousandth
long, each taking a millionth off the loss), so a round now stops once the loss falls by less than a hundred-thousandth
of itself three steps running: the same fit, bit for bit, then stops at its 24th step there.

| | plain | `--hand 1` | `--hand 1 --hold 3` |
|---|---|---|---|
| gates that break (of 229) | 58 (16, 24 and 18) | 3 (1, 1 and 1) | none (3 after its first round) |
| targets met (of 14) | 3: 22, 41 and 46 (the hand-set numbers meet 36, 39 and 46) | 4: the hand-set 3 and 41 (Cadd9 arpeggiated) | 5: those 4 and 13 (G not yet) |
| loss per bar held out: POP909-CL piano, set A | 0.883, 0.417 | 0.960, 0.413 | 0.979, 0.444 |
| the hand cases' mean −ln P | 0.34 to 1.83 | 0.34 to 0.079 | 0.34 to 0.073 |

Scored with `chords --weights` as before (fold 1 of POP909-CL and Billboard; RS200 and the sweep whole; the C# app's
scores are in the first fit's table):

| set | majmin: hand / plain / `--hand 1` / both | name: hand / plain / `--hand 1` / both | extensions' macro F1: hand / plain / `--hand 1` / both | inversions read in root position: hand / plain / `--hand 1` / both |
|---|---|---|---|---|
| POP909-CL piano | 85.3 / 89.3 / 88.8 / 88.7 | 73.3 / 77.1 / 76.3 / 76.3 | - | 78.0 / 69.3 / 76.2 / 76.0 |
| POP909-CL with the tune | 85.4 / 90.3 / 90.0 / 89.8 | 68.3 / 77.1 / 76.3 / 76.2 | - | 77.2 / 69.4 / 76.7 / 76.2 |
| Billboard block chords | 98.1 / 98.8 / 98.9 / 98.9 | 92.4 / 91.6 / 92.7 / 92.8 | 71.0 / 69.5 / 77.2 / 77.6 | 27.2 / 18.0 / 20.2 / 19.7 |
| Billboard set A | 92.7 / 95.6 / 96.3 / 96.3 | 84.5 / 86.7 / 87.9 / 87.9 | 56.4 / 47.9 / 61.6 / 62.8 | 37.4 / 28.4 / 34.9 / 34.2 |
| Billboard set B | 88.0 / 95.6 / 95.8 / 96.1 | 79.2 / 87.0 / 86.8 / 87.8 | 44.8 / 49.0 / 55.4 / 56.8 | 19.6 / 7.3 / 14.1 / 14.0 |
| RS200 block chords | 98.5 / 99.6 / 99.0 / 98.9 | 98.2 / 99.2 / 98.6 / 98.4 | (too few) | 30.7 / 6.8 / 22.7 / 24.0 |
| RS200 set A | 96.0 / 97.7 / 97.1 / 97.1 | 90.9 / 92.8 / 92.1 / 91.9 | (too few) | 28.1 / 17.9 / 25.1 / 24.9 |
| RS200 set B | 91.9 / 98.0 / 97.0 / 97.3 | 87.4 / 94.7 / 92.8 / 93.7 | (too few) | 17.0 / 2.7 / 11.2 / 10.4 |
| The sweep | 77.1 / 77.6 / 82.9 / 83.2 | 69.4 / 66.8 / 73.9 / 75.1 | 61.1 / 52.2 / 66.7 / 67.1 | 40.6 / 23.4 / 38.5 / 35.6 |

- **Learning from the hand cases does most of it.** `--hand 1` keeps nearly all the plain fit's gains on roots and chord
  kinds, and wins back what it lost on extensions: set A's extensions' F1 is 61.6% (plain 47.9%, hand 56.4%), its held
  chords' 80.3% (plain 59.5%, hand 84.4%), and the 13 is found again (recall 97.6%; plain 36.1%, hand 96.0%). The sweep,
  never learned from, is named 73.9% of its time (plain 66.8%, hand 69.4%, the C# app 59.1%). Three gates still break:
  Dm11 read as G7sus4 one stretch at a time (33), and G7sus4 read as G7 (w31 and 31).
- **Holding the gates puts those three right, for almost nothing.** The third fit breaks the same three after its first
  round; its second asks the cases they're in to be as likely as they were at the start (w31 a chance of 0.574, w33
  0.655), and every gate holds. Its scores are within a point of `--hand 1`'s, a little ahead on Billboard set B (named
  87.8%) and the sweep (75.1%), and it meets one more target (13, G not yet). 2 of its 68 rules are met with nothing to
  spare.
- **What the hand cases cost.** Against the plain fit, a little on POP909-CL (majmin 88.7% against 89.3%, name 76.3%
  against 77.1%, still ahead of the hand-set numbers and the C# app) and on RS200 (set B named 93.7% against 94.7%), and
  some inversions: POP909-CL's piano reads 76.0% of its inversions in root position (plain 69.3%, hand 78.0%, the C# app
  74.3%). The loss held out is a little higher (0.979 a bar on POP909-CL's piano, against 0.883), the price of the hand
  cases' own: their mean −ln P falls to 0.073 (the plain fit takes it from 0.34 to 1.83).
- **Why, in the numbers.** The hand cases keep T2 (tensions) up, at 3.23 beside T1's 4.98 (the plain fit 0.38 beside
  5.28), so a reading that names a tension (Cmaj9) is no longer beaten by one that makes every note its own (Em7). R1 (a
  played root) stays above 0 (0.14). Starting mid-bar costs about as much as in the plain fit (E5 −2.93 against −3.10,
  E6 and E7 −4.48 against −4.25), so chords change as often (1.17 times a bar on POP909-CL's piano, hand 1.32; the
  sweep's segmentation 93.9, hand 84.8). 8 of the 39 numbers sit on a rule's bound (T7, R3, R4, B4, B5, B6, E6 and E7;
  the plain fit 12).
- **So.** `--hand 1 --hold 3` is the first fit that breaks no gate, and it scores at or above the hand-set numbers on
  every line above (set A's held chords lose a little: extensions' F1 80.1% against 84.4%). Read with its numbers, every
  hand case holds read whole and one stretch at a time, the held-out G cases too (`show_the_grid`, with its `SET=` line
  and its own pair value as the one point of `AXES`). Shipping its numbers is a pull request of its own: `SHIPPED`
  changes, so `CHORD_READER_VERSION` goes up, and the fit is run once more on what's merged then (it ran on this
  branch's `7dd1183`).
- **Its numbers** (to three places): T1 4.977, T2 3.233, T3 −4.926, T4 −1.219, T5 −1.181, T6 0.441, T7 0.000, T8 −0.218,
  T9 −0.530, T10 −0.763, T11 −0.640, T12 −0.355, T15 0.259, R1 0.145, R2 0.448, R3 0.000, R4 0.000, L1 0.696, B1 1.098,
  B2 0.393, B3 0.163, B4 0.000, B5 −0.609, B6 −0.609, B7 1.072, B8 0.238, B9 −1.812, E1 1.436, E2 0.490, E3 −1.900, E4
  −1.901, E5 −2.934, E6 −4.478, E7 −4.478, E8 −1.545, P1 0.286, P2 0.222, pair 0.417, none −2.466.
