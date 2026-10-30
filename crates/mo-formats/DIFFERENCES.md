# Where the Rust readers differ from the C# ones

The C# readers are the guide, not the answer (CLAUDE.md). Each difference found is listed here and judged: **better**
(the Rust reader is right and C# was wrong), **not yet** (still to port) or **bug** (to fix in Rust).

The twin test (`tools/twin`) finds them: CI runs it on the test files, and `tools\dev\Twin test.cmd` on the whole
library.

## FL Studio

| What | C# | Rust | Judged |
|---|---|---|---|
| "Notes" count in FL 4 to 8 saves | Counts every note event's bytes ÷ 24, though FL 4 to 8 notes are 20 bytes (a pattern of 10 notes showed as 8). | Uses the same note size the note reader uses (by FL version, or told from the notes). | Better |
| FL version written as just "9" | Its major version reads as 0 when the text ends in a zero byte. | Reads 9. | Better |
| A save cut short, or damaged part-way through an event | Read up to there with no warning, as if it were whole. | Read up to there, with a warning that it ends part-way (rule 9). | Better |
| FL 9 and later notes that don't divide into 24 bytes | Guesses the record size from the notes, so a damaged FL 21 pattern could be read as 20-byte notes. | Always 24 bytes from FL 9 (rule 9: note size by FL version). | Better |
| Text in a save whose version has no minor part ("9") | Read as UTF-16, so names and sample paths came out garbled. | One byte a letter, as for any FL before 11.5. | Better |
| Samples under `%FLStudioUserData%` | Found on disk for each sample, inside the reader. | The reader never looks at the disk; reading a file from disk finds each folder once for all its samples. | Same result, faster |
| Step sequencer dots in FL 4 to 8 saves | Turned into 24-byte notes, then read with the version's 20-byte size when their bytes also divide by 20 (a multiple of five steps), jumbling them. | Notes the reader makes itself are always read as 24-byte ones. | Better |
| Channel events after the mixer's started | Each reader kept its own track of channel and mixer events, and they differed a little (the project list took channel types from mixer events; the MIDI clips took names from a mixer effect's plugin). | One walker for every reader. | Better |
| Where a mixer effect's preset sits, after an effect that saved nothing | The skipped effect's insert was kept and given to the next preset taken out, even one not in the mixer. | Only a preset taken out gets an insert. | Better |
| A song whose playlist has only audio or automation clips | Played every pattern one after another, as if the playlist were empty, so patterns never placed had notes. | Patterns play one after another only when the playlist is empty. | Better |
| An effect slot switched on and off by automation | Its curve slid between on and off. | Held, jumping between on and off as a channel's on/off does. | Better |
| MIDI clips in a save with no version | Picked each pattern's note size without the song's channels, so it could differ from the song's reading. | The same size the song uses. | Better |
| Samples on the channels inside a Layer channel | Counted as not played when only the Layer has notes. | The same, until the Layer's children are read (needs one of Solomon's saves with a Layer). | Not yet |
| Songs arranged in a later arrangement (FL 12.9 on) | Only the first arrangement is read. | The same: the twin test will show whether any of Solomon's songs need more. | Not yet |

## Bitwig Studio

On the test files the two read the same; these are the places the Rust reader was written to differ, for files the
tests don't have.

| What | C# | Rust | Judged |
|---|---|---|---|
| A group of tracks listed inside itself (a damaged file) | Walks round it for ever, until the app runs out of stack and closes. | Lists each track once. | Better |
| A plugin state in the project's zip that won't come out whole | No presets at all from the project. | That one preset is left out; the rest are taken out. | Better |
| A folder beside the project that can't be opened (in `samples`, say) | The whole project fails to read. | That folder is skipped. | Better |
| A folder linked into `samples` that links back to itself | Can go round it again and again. | Each folder is listed once, wherever the links lead. | Better |
| The order of hardware names | .NET's culture-aware order (which differs between Windows and Linux). | Close to it: spaces and punctuation before digits, digits before letters, letter case aside. Only names that differ just in punctuation could come out in another order. | Same for real names |
| A note's velocity as a MIDI 1.0 value (0 to 127) | Bitwig's 0-to-1 velocity rounded straight to 0 to 127. | Kept at MIDI 2.0's 16 bits, then rounded to 0 to 127 when a MIDI 1.0 value is wanted. About 3 in 10,000 velocities come out 1 apart (rounding twice); Bitwig's own whole-step velocities are the same. | Better (rule 22) |
