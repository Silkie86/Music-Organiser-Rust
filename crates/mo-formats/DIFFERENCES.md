# Where the Rust readers differ from the C# ones

The C# readers are the guide, not the answer (CLAUDE.md). Each difference found is listed here and judged: **better**
(the Rust reader is right and C# was wrong), **not yet** (still to port) or **bug** (to fix in Rust).

## FL Studio

| What | C# | Rust | Judged |
|---|---|---|---|
| "Notes" count in FL 4 to 8 saves | Counts every note event's bytes ÷ 24, though FL 4 to 8 notes are 20 bytes (a pattern of 10 notes showed as 8). | Uses the same note size the note reader uses (by FL version, or told from the notes). | Better |
| Samples a save holds but doesn't play (`unplayed`) | Filled in from the song's channels and playlist. | Empty until the song reader (notes, playlist, channels) is ported. | Not yet |
| FL version written as just "9" | Its major version reads as 0 when the text ends in a zero byte. | Reads 9. | Better |
| A save cut short, or damaged part-way through an event | Read up to there with no warning, as if it were whole. | Read up to there, with a warning that it ends part-way (rule 9). | Better |
| FL 9 and later notes that don't divide into 24 bytes | Guesses the record size from the notes, so a damaged FL 21 pattern could be read as 20-byte notes. | Always 24 bytes from FL 9 (rule 9: note size by FL version). | Better |
| Text in a save whose version has no minor part ("9") | Read as UTF-16, so names and sample paths came out garbled. | One byte a letter, as for any FL before 11.5. | Better |
| Samples under `%FLStudioUserData%` | Found on disk for each sample, inside the reader. | The reader never looks at the disk; reading a file from disk finds each folder once for all its samples. | Same result, faster |
