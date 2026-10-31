# Space: where Rust differs from the C# app

The C# app's `Core/Space` is the reference (`DiskSpace`, `SongSpace`, `AppFiles`, `RecycleBin`). Each difference is
listed with a verdict: **better** (on purpose), **same idea, new shape**, or **not yet**.

## Measuring

| What | C# | Rust | Verdict |
|---|---|---|---|
| Walking a folder | one thread, file by file | a level of folders at a time over every core of the pool it's called in (rule 21) | better |
| Links, OneDrive online-only files | not followed / left out of sizes | the same | same |
| `size_text` rounding | .NET formatting | the same, halves rounded up as .NET does | same |
| Stopping part way | `CancellationToken` | a stop flag; a stopped measure says `Stopped` rather than giving a part answer | same idea, new shape |
| Compact line notes | text built when measured | a `note()` worked out from the line's fields (kind, time, the save it copies) | same idea, new shape |

## Clean up

| What | C# | Rust | Verdict |
|---|---|---|---|
| Checking before removing | the list was trusted as measured | every picked line is checked by a guard at the preview **and again** just before it goes: inside a project folder, not a project folder itself, not in or holding a sample folder, not a sound any project uses by name, not a song's newest save, really the kind the line says (a save, an autosave below the project folder, Studio One's Cache folder, audio), not a link, not on a network drive (no Recycle Bin) | better |
| A copy and its twin | the measure never lists both | a copy whose twin is also picked stays; at clean-up a copy only goes while its twin is still byte for byte the same | better |
| Changed since you looked | removed anyway | a file whose size or time isn't what the list measured stays, and so does anything that changed between the preview and the clean-up | better |
| Links | the path as written | paths are checked as written and as they really are (links, `..` and mapped drives worked out): a link out of the project folder, into a sample folder, or a copy whose twin is a link to it, stays; a song folder that's a link isn't the song's own | better |
| Undo | find it in the Recycle Bin yourself | after a clean-up each thing is found in the bin (one that isn't is reported: Windows removes for good, after asking, what's too big for its bin); the `Done` list, kept by the app, is what `undo` puts back; on a Mac, Finder's Put Back | better |
| Thinning, default | the setting's default is 3 hours | the same; and the newest save is never thinned even when another was written at the same moment (C# could thin it) | better |
| Removing for good | never: Recycle Bin only, Windows asks before deleting anything too big for it | the same (the `trash` library sets the same Windows flags) | same |
| Administrator step for Program Files | yes (plugin clean-up) | not yet: song clean-up never touches Program Files; it comes with plugin clean-up | not yet |

## Not yet

- Plugin sizes (folder guessing), spare plugin copies, uninstallers, moving plugin data to another drive.
- Keeping measured sizes between runs. The C# app keeps them in `library.db` (`plugin_space`, `folder_space`); how the
  Rust library keeps them is for the library's database design (rule 23: each fact once, nothing stored that's quick
  to work out).
