# Where the Rust sound library differs from the C# one

The C# app is the guide, not the answer (CLAUDE.md). Each difference is listed here and judged: **better** (Rust is
right and C# was wrong), **not yet** (still to port) or **bug** (to fix in Rust).

## Scanning sample folders

| What | C# | Rust | Judged |
|---|---|---|---|
| MP3, FLAC, M4A, OGG and AAC on a Mac or Linux | Left out: only Windows' decoders read them. | Listed on every system (the Rust app decodes them itself). | Better |
| WMA files | Read through Windows. | Left out, and kept like any file the app can't read: the sound keeps what you gave it. | Not yet (needs a WMA decoder) |
| A sound outside every listed sample folder | Forgotten at the next scan, with its tags, stars, plays and galaxy place. | Kept; the page shows only sounds under the listed folders. A folder taken off the list for a while (or a list that failed to load) loses nothing. | Better |
| A file dated before 1970 | Stored with a negative modified time. | The same (the shared folder walk used to read it as 0, so it was listened to again each scan). | Same |

## Analysis

`analyse` is a step-for-step port of the C# `SoundAnalyzer.cs` and `Dsp.cs` (down to where C# adds in single
precision). The twin test (`twin sounds`, run in CI) has both apps analyse the same made-up test sounds and lists every
measure that differs.

| What | C# | Rust | Judged |
|---|---|---|---|
| Key found in the audio (chroma against key profiles) | Done for tonal loops and sounds over 1.5 s with no key in the name. | Waits for the harmony engine (phase 6), which Solomon wants rebuilt rather than copied. Until then such a sound gets its root note's key (one-shots) or none (loops). Sounds already in sounds.db keep the C# key. | Not yet (Solomon, 2026-10-31: keys from names only until the harmony engine) |
| Chord name (stabs) and chords over time (loops) | Template matching and a smoothing pass. | Left to the harmony engine, as above. | Not yet |
| The mel filters for a sample rate | Made once and kept, behind a lock. | The same. | Same |
| The chroma's lowest bin at a sample rate over 400 kHz | Read one place before the start, and failed. | Starts one bin higher. | Better (no real file has that rate) |
| Where pitch is looked for in a long, high-rate sound with a long silent start (192 kHz, ~56 s of silence) | Worked out in 32-bit whole numbers, which wrapped, so it looked in the wrong place. | Worked out in 64-bit, so it looks just after the silence. | Better |
| A damaged header with a sample rate under 60 | YIN read past its buffer and the analysis failed. | No pitch is looked for; the rest is measured. | Better |

## Type classifier

`classify` is a step-for-step port of the C# `SoundClassifier.cs`, down to .NET's seeded `Random` (for the balancing
and the training order) and single-precision sums. The twin test (`twin sounds`) has both apps train on the same
made-up examples, compares every weight, then has Rust read the C# model and judge the same probes.

| What | C# | Rust | Judged |
|---|---|---|---|
| The order of the types in a model | Sorted by the current culture's rules. | Sorted ignoring case, then lower case first. The same for every type name the app uses; a home-made name that differs only in punctuation could sort differently, which changes nothing but the order they're stored in. | Same in practice |
| Training on vectors of different lengths | Stopped with an error, or quietly used the first part of the longer ones. | Refused with a clear reason. | Better (never happens: one space at a time) |
| A saved model whose sizes don't fit together | Read, then failed when used. | Treated as no model. | Better |
| When it was trained | A local time. | Kept as the text .NET wrote, and written the same way. | Same |
| A model with a number that isn't finite (from a sound whose analysis went wrong) | Its writer refused, so the old model stayed. | Not saved, so the old model stays. | Same |
