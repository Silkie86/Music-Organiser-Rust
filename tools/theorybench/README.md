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

## Keys

```
cargo run -p mo-theorybench --release -- keys path/to/bench/pop909.jsonl [--part melody] [--aligned]
```

It finds the key of whole songs (those with one key, and those that change key, against the key labelled for the
longest time) and of 8-bar clips cut along the bar lines (against the key labelled at the middle of the clip), with
each key finder: the C# app's (copied unchanged in `src/baseline.rs`) and four published key profiles run the same way.
For each it prints:

- **right**: the share of exact answers (no answer counts as wrong), with the range it most likely lies in. The
  ranges come from drawing whole songs again at random, because the clips of one song aren't independent.
- **MIREX**: the contest score (1 exact, 0.5 a fifth up, 0.3 the relative key, 0.2 the parallel key).
- **shown** and **right when shown**: how often the confidence clears the app's cut, and how often it's right then.
- **major** and **minor**: right, split by the labelled mode.
- what the wrong answers were (a fifth up or down, the relative or parallel key, another key, no key);
- how often it's right when it answers only its surest cases (if its confidence means anything, this rises);
- against the first finder: the difference in points, with its range, and McNemar's test.

`--part` hears only the parts with that role; `--aligned` keeps only the songs the C# bench used (four beats a bar,
labelled bar lines on the MIDI's grid: 387 of POP909's songs).

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
segmentation 91.1, 1.36 chord changes a bar (the labels change 1.47 times a bar).
