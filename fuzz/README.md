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

## How to run it

The fuzzer needs nightly Rust and cargo-fuzz (once):

```
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz
```

It runs on Linux and macOS (on Windows, use WSL). From the top of the repository:

```
# 1. Make the seeds: the made-up projects the tests build (never Solomon's files), written into fuzz/corpus/<target>/.
cargo test -p mo-formats --test fuzz_seeds -- --ignored

# 2. Fuzz one target for 10 minutes. -timeout counts a file that takes over 10 seconds as a hang.
cd fuzz
cargo +nightly fuzz run fl -- -max_total_time=600 -timeout=10 -rss_limit_mb=2048
```

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

`fuzz/corpus`, `fuzz/artifacts` and `fuzz/target` are never committed (see `.gitignore`); `fuzz/target` can grow to a
few gigabytes, so delete it when you're done.
