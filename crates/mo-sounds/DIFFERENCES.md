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

## Types and tags learned from your library

`learn` ports `UpdateTypes`, `ResolveType`, `Disagreements` and `TrainTags`. They work out what changes and hand it
back; nothing is saved to `sounds.db` yet.

| What | C# | Rust | Judged |
|---|---|---|---|
| A type from CLAP's own words ("model", zero-shot), when the classifier isn't sure | Used once deep listening is installed. | Not yet: needs CLAP in Rust. A sound the C# app typed that way keeps its stored type until the Rust app writes. | Not yet |
| A tag's model that can't train (vectors of different lengths) | Stopped with an error. | That half suggests nothing. | Better |
| The type model can't train (vectors of different lengths, halfway through analysing again) | Stopped with an error, or used part of each vector. | Nothing changes: the saved model and every type stay. | Better |
| A sound CLAP's words typed ("model") | Worked out again each time. | Kept as stored while the classifier isn't sure (Rust can't work it out yet). | Not yet |

## Names

| What | C# | Rust | Judged |
|---|---|---|---|
| A key written with capitals ("Pad C Minor", "Keys Bb MIN") | Read as major: only small "minor"/"min" counted. | Read as minor (found by the harmony thread, 2026-11-01). A lone capital "M" still isn't minor. | Better |

## Sounds like this

`index` ports `VectorIndex` and `SoundOrganiser.Similar`; the twin test (`twin sounds`) has both apps find the
nearest sounds for the same made-up vectors.

| What | C# | Rust | Judged |
|---|---|---|---|
| Vectors of two lengths in equal numbers | Kept whichever length the dictionary listed first. | Keeps the longer (a newer analyser's). | Better (only halfway through analysing again) |
| Adding up a likeness | Several products at a time (SIMD). | One at a time, so a likeness can differ in the 7th place; the twin allows 1e-5. | Same in practice |
| Two sounds exactly as alike at the cut-off (exact copies are common) | Kept whichever each worker met first, which depends on the order the dictionary holds them. | Keeps the lower id, every time. | Better (same list each time) |
| Standardising | Sums in dictionary order. | Sums in id order, so a rebuild gives the same numbers. | Same in practice |
| "Sounds like this" when hidden copies or other types fill the first 3 × max | Stops there, so the list can come up short. | Asks for more until the list is full or the library has no more. | Better |

## Rediscover

`rediscover` ports `Rediscover` (suggest, why nothing, years, folders); the twin test (`twin sounds`) has the C# app
save a made-up library to a new sounds.db, which the Rust app reads, and both answer the same seeded questions.

| What | C# | Rust | Judged |
|---|---|---|---|
| Shuffle (seed 0) | .NET's unseeded random numbers. | The seeded ones, seeded from the clock. | Same in practice (a new list each time) |
| Sounds analysed but no vectors stored | Says every sound is used already. | Says none have been listened to yet. | Better |
| Folders of a Mac or Linux path on Windows (and back) | Splits only on that system's separator. | Splits on `\` and `/` everywhere. | Same in practice |
| Why a starred (or rated, or much played) unused sound is suggested | Can say it "sounds like" itself. | Names the closest other sound you like. | Better (bug in C#) |
| No sounds of the chosen type in the index yet (deep listening hasn't heard them) | Says every one is used already. | Says they haven't been listened to yet. | Better |
| Folders on Windows spelt with `/` and `\` | One folder. | One folder too (`/` turned into `\`), spelt with `\`. | Same |

## Kits

`kits` ports `KitBuilder` (roll and export); the twin test rolls the same seeded kits on the Rediscover library.

| What | C# | Rust | Judged |
|---|---|---|---|
| A described vibe | Turned into a vector by deep listening's model inside the roll. | The caller passes the vector (the model lives in the app); not twin-checked. | Same |
| Export | Copies straight over the pad's name. | Writes beside it (`.part`) and renames, so a stopped export leaves no half pad. | Better |
| Names on Mac or Linux | Leaves characters only Windows forbids (`<>:"\|?*`). | Turns them into "-" everywhere, so a kit copied to Windows still opens. | Better |
| Two exports of the same kit at once | Could share one folder. | Each makes its own folder, then fills it. | Better |
| A copied pad | Not checked. | Checked to be whole and on the disk before it takes its name; a failed one is removed. | Better |

## Duplicates

`duplicates` ports `DuplicateFinder.Groups`: it reads the copies the C# app's duplicate finder found and groups them
for the Space page. The twin test has the C# app group the same made-up copies in its own sounds.db.

| What | C# | Rust | Judged |
|---|---|---|---|
| A trimmed (or only similar) copy alone in its set | Described against itself: always "Longer version (x s)". | Described against the one to keep: "Trimmed: 0.5 s of 2 s". | Better (bug in C#) |
| A copy kind stored as a number ("1") | Read as that kind. | The same. | Same |

## Galaxy

`galaxy` does what `GalaxyMap` (laying the map out, placing new sounds) and the galaxy part of `SoundOrganiser`
(which fingerprints, when to lay out afresh) do, the Rust way: the parts that only kept a C# app fast and small are
gone, and the tricks that make it fast in any language stay (squashing to 32 numbers, learning that from a sample,
neighbours of neighbours for big libraries, every core at once). So a fresh map isn't star for star the C# one; the
twin test checks it's at least as good (sound-alikes kept together, new sounds placed beside theirs).

| What | C# | Rust | Judged |
|---|---|---|---|
| Random numbers | .NET's seeded `Random`. | Rust's own (SplitMix64), one sequence per star per round. | Same (a different but equally good map) |
| A library of 6,000 sounds or more | Settled by every core nudging shared stars at once, so each layout differs. | Each round, every star moves itself against where the others stood at its start, so the same library always gives the same map, however many cores. | Better |
| A library under 6,000 sounds | Settled on one core (so it would repeat). | Every core, and still repeats. | Better (faster) |
| Fingerprints | Copied for each step. | The measurements are lent straight from the index; only joined ones are made. | Better |
| Placing new sounds | Compared with a sample of 10,000 mapped sounds. | With every mapped sound when few are new (up to 50,000, within 500 million comparisons); with at least 10,000 otherwise. | Better |
| Saving the map | `UpdateGalaxy` saves it to sounds.db. | `update` hands back what to save; the app saves it once sounds.db writes are trusted. | Same, for now |
| Stopping a layout | Throws when cancelled. | Gives nothing back (`None`). | Same |
