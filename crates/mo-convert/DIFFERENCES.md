# Where mo-convert differs from the C# converter

The plan (`src/plan.rs`) is the C# `BitwigProject.FromFl` and `DawProject` choices without Bitwig's objects. Where it
does something else on purpose, it's listed here and judged.

| What | C# | Rust | Judged |
|---|---|---|---|
| Note velocity | FL's velocity over 127 | The MIDI 2.0 16-bit velocity over 65535 (FL's 0 to 128 over 128, as the reader keeps it) | Better: FL's top is 128, so C# made 127 and 128 both the loudest |
| Serum 1 swapped for Serum 2 | Optional, with its sound translated | Left out | Solomon dropped it (2026-10-29) |
| Notes the template decides (a lane or on/off it can't copy, a track with no fader) | Said while writing | Said by the writer, after the plan's notes | Same notes, the template's ones last |
| Two FL clips moving one plugin setting under different names | One lane | One lane (named after the first) | Same |
| Two FL automation clips moving one thing, with a gap between them | A straight line across the gap | The value holds until the next clip, then jumps | Better: FL keeps a setting where the last clip left it |
| Third-party effects on an insert no converted channel feeds (a bus fed by sends) | Left out silently | Left out with a note | Better |
| A muted note after a step on the same key | Cuts the step short | Doesn't | Better: a muted note plays nothing in FL |
| Overlapping clips with a step and a held note on one key at one tick | Keeps whichever came first | Keeps the longer | Better |
| A channel with both notes and audio clips | Two tracks with the same name | "Vox" and "Vox 2" | Better |
| A sample shared by several channels | Read (hashed) once per channel | Read once | Better (rule 12) |
| An unreadable sample stretched to a set length (FL's Time knob) | Length guessed ignoring the Time knob | Guessed as playing at its own speed over that length | Better |
| Audio clips when the template has no audio track | Listed, the rest converted | Same (the writer lists them) | Same |
| A VST2 program that can't be turned into a bank | Saved as `.fxb` anyway, so Bitwig can't load it | Kept as `.fxp` | Better |
| A VST2 with no id saved | Kept the template plugin's id under the new name | Id 0 | Better: Bitwig never loads the template's plugin by mistake |
| A template whose Sampler has no sample, or whose patterns share a track | Failed on the first song | Turned down when it's chosen | Better |
