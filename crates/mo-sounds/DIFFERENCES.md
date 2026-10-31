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
