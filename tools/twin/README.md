# Twin test

Both apps read the same FL Studio projects, and every place where they read them differently is listed (spec
section 11.2). The C# app's readers run unchanged, through the small C# helper in `cs/`. Each difference gets judged
in `crates/mo-formats/DIFFERENCES.md`: the Rust reader is better, or it's a bug to fix.

## At home, on the whole library

Double-click `tools\dev\Twin test.cmd` in the Rust app's folder
(`%LOCALAPPDATA%\MusicOrganiser\run-latest-rust\app`). It builds the C# helper from the C# app's code (the C#
launcher's copy), takes the folders from the library and reads every `.flp` in them with both apps. It only reads.

It writes two files to `%LOCALAPPDATA%\MusicOrganiser\logs`:

- `twin-test-<date>.txt`: the summary. Only counts and field names, nothing from the songs, so it's safe to share.
- `twin-report-<date>.txt`: every difference, file by file, with what each app found. It names songs and samples,
  so it stays on the PC.

## In CI

The test files (`crates/mo-formats/tests/twin_fixtures.rs`) are read by both apps on every push. A difference not
in `expected.txt` fails the run: judge it in DIFFERENCES.md, then add it with `--write-expect`.

```
MO_TWIN_FIXTURES=/tmp/twin cargo test -p mo-formats --test twin_fixtures
dotnet build tools/twin/cs -c Release -o /tmp/twin-cs -p:MoOldRepo=<C# repo> -p:MoBuildNative=false -p:MoBuildBitwig=false
cargo run -p mo-twin -- run --cs /tmp/twin-cs/twin-cs.dll /tmp/twin --expect tools/twin/expected.txt
```

`cargo run -p mo-twin -- dump <file>` prints what the Rust readers find in one file.
