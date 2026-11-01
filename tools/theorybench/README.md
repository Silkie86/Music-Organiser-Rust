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
  - `--write` keeps the credits of the weights it scales, adds the rescale and its scales after them, and adds
    `packs` to `LEARNED_FROM`. A `--dump` of a run that read his pieces names his files, so it can't be written inside
    a copy of this repository either.
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
cargo run -p mo-theorybench --release -- chords path/to/bench/pop909.jsonl --answers "C# app=cs-chords.jsonl" --part accompaniment
```

Each `--answers name=file` is one engine's chords for every piece; the first is the baseline the others are compared
with. The chords are scored exactly as `mir_eval.chord` scores chord recognition, so the numbers mean what published
ones mean: each stretch of time weighted by its length in beats, pooled over all the songs. `--per-segment` scores
them per labelled chord too (each counts once, right when right for more than half its time).

The C# app is scored by the name it shows, read back with mo-harmony (`Chord::parse`, on the root and over the bass it
gives), not by the notes it heard: "C7" names a G even when none sounds, and "Gsus4" for C/G leaves the E out, as its
page shows them. A name stands for a few notes a chord may leave out, as players read names and as mo-harmony's own
names do: the plain fifth, the 9 under an 11 or 13, and the 11 of a minor 13. Those count only where the label has
them, so "Cm13" is right for a minor 13 with no 11 and "Cm11" for a minor 7 with an 11 and no 9. A 7alt says it is
altered but not how, so it can be right as the event, never as the name.

The helper also writes, for each chord, the other names the C# app's Playground offers for the same notes (on its
other roots, at most four), so its pick list can be scored. With `--oracle-cuts` it reads each labelled chord's
stretch on its own and keeps the chord it read there for longest: its naming when the changes are given, apart from
where it would change chord. Give the bench the same `--part` as the helper, so it knows which notes were heard.

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
