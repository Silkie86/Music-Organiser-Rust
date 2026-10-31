# Where mo-convert differs from the C# converter

The plan (`src/plan.rs`) is the C# `BitwigProject.FromFl` and `DawProject` choices without Bitwig's objects. Where it
does something else on purpose, it's listed here and judged.

| What | C# | Rust | Judged |
|---|---|---|---|
| Note velocity | FL's velocity over 127 | The MIDI 2.0 16-bit velocity over 65535 (FL's 0 to 128 over 128, as the reader keeps it) | Better: FL's top is 128, so C# made 127 and 128 both the loudest |
| Serum 1 swapped for Serum 2 | Optional, with its sound translated | Left out | Solomon dropped it (2026-10-29) |
| Notes the template decides (a lane or on/off it can't copy, a track with no fader) | Said while writing | Said by the writer, after the plan's notes | Same notes, the template's ones last |
| Two FL clips moving one plugin setting under different names | One lane | One lane (named after the first) | Same |
