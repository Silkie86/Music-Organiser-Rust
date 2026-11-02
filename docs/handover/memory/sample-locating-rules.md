---
name: sample-locating-rules
description: How the app decides a project's sample is missing (rule 13 plus the Sounds library step Solomon agreed 2026-10-31)
metadata:
  type: project
  modified: 2026-10-31T22:35:44.402Z
---

Rule 13 (Solomon, 2026-10-15): check the project's folders and the DAW's own folders before calling a sample missing.

Extended 2026-10-31 (Solomon said yes and "also check for the size of the file if that info is available"): the
locator (`mo-library/src/samples.rs`, step 7) then looks in the Sounds library (`sounds.db`, read-only):
- same file name in a folder of the same name, or
- same name anywhere only when every library sound of that name has the same byte size (copies of one sound).

No DAW saves a sample's file size: FL keeps only the path; Studio One's mediapool.xml keeps frameCount/sampleRate
(a possible later length check). Merged as PR #66; mo-app `sound_library` caches the library for 60 s.

**Why:** old saves (e.g. FL 6.0 jazzy_2.flp, 2006) point at drives long gone ("<a drive long gone>\…").
**How to apply:** don't widen the search further (whole drives, name-only guesses) without asking Solomon.
Related: [[rust-rebuild-rules]], [[rust-app-design-decisions]].
