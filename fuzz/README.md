# Fuzzing the file readers

## What fuzzing is

A fuzzer is a program that makes up files and feeds them to a reader, millions of times. It starts from a few good
files (the "seeds"), then changes them in small random ways: flips bits, cuts bytes off, copies one part over another.
It watches which parts of the reader's code each made-up file reaches, and keeps the files that reach new code, so over
time it gets deeper and deeper into the reader.

A project file on Solomon's disk can be damaged in any way (a crash while saving, a half-copied file, a newer DAW
version). The rule for every reader in `mo-formats` is: **it may say "this file is damaged", but it must never panic
(stop the app) or hang (never finish)**. The fuzzer is how we check that rule on files nobody would think to write by
hand.

The tool used is [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz.html), which runs LLVM's libFuzzer.

## The targets

Each "target" is a small program in `fuzz_targets/` that hands the fuzzer's bytes to one reader:

| Target          | Reader                                                                              |
|-----------------|-------------------------------------------------------------------------------------|
| `fl`            | FL Studio `.flp`: the project list's reader, the song, notes, plugin states, arrangement |
| `bitwig_decode` | Bitwig `.bwproject`: decoding the file's objects                                   |
| `bitwig_read`   | Bitwig `.bwproject`: the project list's reader, notes, plugin states, arrangement, automation |
| `studio_one`    | Studio One `.song` (a zip of XML): the project list's reader, notes, plugin states, arrangement |
| `midi_smf`      | Standard MIDI Files (`.mid`)                                                        |
| `midi_clip`     | MIDI Clip Files (`.midi2`, MIDI 2.0)                                                |
| `ableton`       | Ableton Live `.als` (gzipped XML): the project list's reader, notes, arrangement     |
| `song_shape`    | The Board's song shape and the arrangement, from an FL, Bitwig, Ableton or Studio One save |
| `midi_find`     | The MIDI page: the parts the index finds in a MIDI file or any song, and a song as it plays |
| `shapes`        | The Curves page: Serum `.XferShape`/`.shp`, LFO Tool `.fxp`, Vital, Duck, Bitwig `.bwcurve`, FL and Bitwig knob moves |
| `presets`       | Plugin presets (`.fxp`, `.fxb`, `.vstpreset`, `.bwpreset`) and Serum 1's settings in them |
| `audio`         | Sound files (WAV, AIFF, MP3, FLAC, OGG, M4A, AAC): the player, the waveform, the analysis |
| `stored`        | What the app reads back from its own databases (a part's notes, chords, a sound's held notes): a damaged database must not take down the CLAP plugin in a DAW |

Where one target reads several kinds of file, the made-up file's first byte picks the kind (its file ending), and the
rest is the file.

## How to run it

The fuzzer needs nightly Rust and cargo-fuzz (once):

```
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz
```

It runs on Linux and macOS (on Windows, use WSL). From the top of the repository:

```
# 1. Make the seeds: the made-up projects the tests build and the recipe saves (never Solomon's own files), written into
#    fuzz/corpus/<target>/.
cargo test -p mo-formats --test fuzz_seeds -- --ignored

# 2. Fuzz one target for 10 minutes. -timeout counts a file that takes over 10 seconds as a hang, and -malloc_limit_mb
#    a file that makes the reader ask for over 512 MB at once (the app would run out of memory on a full PC).
cd fuzz
cargo +nightly fuzz run fl -- -max_total_time=600 -timeout=10 -rss_limit_mb=2048 -malloc_limit_mb=512
```

The targets that read the app's databases build SQLite in, twice over, which the fuzzer's memory checker takes for a
mistake ("odr-violation") before it starts: set `ASAN_OPTIONS=detect_odr_violation=0` first (`export
ASAN_OPTIONS=detect_odr_violation=0`).

`cargo +nightly fuzz list` shows the targets. The fuzzer prints a line every so often; `exec/s` is how many files a
second it tries, and `cov` is how much of the reader's code it has reached. It keeps the interesting files it made in
`fuzz/corpus/<target>/`, so the next run carries on from there.

## When it finds something

It stops and writes the file that broke the reader to `fuzz/artifacts/<target>/crash-...` (or `timeout-...` for a hang),
with the panic message above it. Then:

```
# Make the file as small as it can be while it still breaks the reader.
cargo +nightly fuzz tmin fl artifacts/fl/crash-...
# Run the reader on it once, to see the panic again.
cargo +nightly fuzz run fl artifacts/fl/minimized-from-...
```

Fix the reader so it returns an error or a warning instead, and add a test in `crates/mo-formats/tests/` that feeds the
small file's bytes, written out in the test as a byte list (`b"..."` or `[0x46, 0x4c, ...]`), so it never comes back.
A fix that only turns a panic into an error doesn't change what the reader finds in good files, so its version stays.

## The tests that run on every change

The fuzzer needs nightly Rust and hours, so it isn't part of CI. Three kinds of test do a smaller version of the same on
every change, with the same broken files each time:

- `broken_saves.rs` (mo-formats), `broken_presets.rs` (mo-presets), `broken_sounds.rs` (mo-audio): every reader on
  copies of every test save cut short at many places and changed in a few bytes (a fixed random seed). A panic a reader
  catches itself counts too: the file still loses what it would have shown.
- `deep_saves.rs` (mo-formats): racks in racks, objects in objects and tags in tags thousands deep, read on a thread with
  a 1 MiB stack. Running out of stack ends the app at once, with no warning and nothing caught.
- `big_saves.rs` (mo-formats): saves with hundreds of tracks and tens of thousands of clips, each read in good time.

`fuzz/corpus`, `fuzz/artifacts` and `fuzz/target` are never committed (see `.gitignore`); `fuzz/target` can grow to a
few gigabytes, so delete it when you're done.
