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
| Checking before removing | the list was trusted as measured | every picked line is checked by a guard at the preview **and again** just before it goes: inside a project folder, not a project folder itself, not in or holding a sample folder, not a sound any project uses by name, not a song's newest save, really the kind the line says (a save, an autosave below the project folder, Studio One's Cache folder, audio), not a link, not on a network drive (no Recycle Bin; on a Mac or Linux, network shares mounted into a folder too) | better |
| A copy and its twin | the measure never lists both | a copy whose twin is also picked stays; at clean-up a copy only goes while its twin is still byte for byte the same | better |
| Changed since you looked | removed anyway | a file whose size or time isn't what the list measured stays, and so does anything that changed between the preview and the clean-up | better |
| Links | the path as written | paths are checked as written and as they really are (links, `..` and mapped drives worked out): a link out of the project folder, into a sample folder, or a copy whose twin is a link to it, stays; a song folder that's a link isn't the song's own | better |
| Undo | find it in the Recycle Bin yourself | after a clean-up each thing is found in the bin (one that isn't is reported: Windows removes for good, after asking, what's too big for its bin); the `Done` list, kept by the app, is what `undo` puts back; on a Mac, Finder's Put Back | better |
| Thinning, default | the setting's default is 3 hours | the same; and the newest save is never thinned even when another was written at the same moment (C# could thin it) | better |
| Removing for good | never: Recycle Bin only, Windows asks before deleting anything too big for it | the same (the `trash` library sets the same Windows flags) | same |
| Administrator step for Program Files | yes (plugin clean-up) | not yet: song clean-up never touches Program Files; it comes with plugin clean-up | not yet |

## Plugins

| What | C# | Rust | Verdict |
|---|---|---|---|
| Plugin files under linked folders | .NET's walk goes through links | the same, each real folder once, so a link back up can't loop | same |
| A family's letters | .NET `char.IsLetterOrDigit` | Rust's alphabetic or numeric: the same for every name seen; a few rare scripts' marks count as letters too | same idea, new shape |
| Folders in a data root, order | as the folder lists them | by name, so two runs give the same order | better |
| Measured when | stored with each plugin | not kept here: the caller knows when it measured (rule 23) | same idea, new shape |
| A folder naming a version | the comment says "Serum 2 Presets" is Serum 2's alone, but `Family` strips the version, so Serum and Serum 2 shared it | a folder naming a version ("Kontakt 8 Library", "Serum 2 Presets") is that version's alone; one naming none ("Kontakt Library") stays shared | better |
| A folder you added | counted for you, and for any other plugin whose guess took it too | yours alone: no other plugin's guess takes it | better |
| A plugin folder ruled out inside a maker's folder | still taken off the maker's folder, so counted nowhere | stays part of the maker's folder | better |
| A plugin file that's a link | measured as the link (nothing) | measured where it really is | better |
| Linux VST2 plugins | `.so` files not found | found | better |
| Documents | Windows' own Documents, wherever it's moved | the same (Windows' known folders) | same |
| Spare copies: same plugin? | one family anywhere inside the other ("Serum FX" took Serum's projects) | equal, or one with its maker in front ("FabFilter Pro-Q 3" and "Pro-Q 3") | better |
| Spare copies' reason | text stored with each copy | a `Why` worked out into text when shown | same idea, new shape |

## Plugin clean-up

| What | C# | Rust | Verdict |
|---|---|---|---|
| Which folders may go | the card's rules: never Program Files, your folders, a folder with a sample a project uses (by path, then by name), OneDrive online-only files, shared with a used plugin, sounds while a project uses the plugin, a still-used plugin, a folder holding one that stays | the same rules, worked out in the backend (`plugin_clean::lines`) | same idea, new shape |
| A plugin the library doesn't know | counted as having no projects when shared, as used when held | counted as used everywhere | better (safer) |
| More reasons to stay | none | a link (or reached through one), a network drive (a Mac's or Linux's network shares too), a drive or one with another drive mounted inside, your home folder or a folder the computer keeps (Documents, Music, Desktop, Downloads, AppData, OneDrive…) or one holding it, found by where they really are; any plugin file or bundle inside (not only listed ones, and a Mac's bundles are folders); a part of it that can't be read | better |
| Checked again before it goes | the card's look again (samples, OneDrive) | every rule again, against the plugin's folders as they are now, and the folder must hold just what it held when you looked: the same files, sizes and times (a preset renamed or saved again at the same size counts; the files Finder and Explorer make by themselves don't) | better |
| A folder inside another you picked | n/a (one folder at a time) | goes with it, once, and is listed as gone with it; the room freed (`freed`, and what the clean-up reports) counts everything inside once | better |
| Why a folder holding another plugin's stays | "used by" the card's plugin | names the plugin that keeps it ("Sounds stay while a project uses Omnisphere") | better |
| Undo | Recycle Bin by hand | the same `Done` list and `undo` as songs | better |
| Administrator step | yes, one question for files Windows guards | not yet (Program Files never goes in a clean-up; ProgramData folders that need it say so) | not yet |

## Uninstallers

| What | C# | Rust | Verdict |
|---|---|---|---|
| Which programs, which first | Windows' list of installed programs (both hives, 64- and 32-bit), matched by family, maker and version; a DAW's never; another version's last | the same, checked on the C# app's own test cases | same |
| Starting one | .NET's shell start, so Windows asks its administrator question when the uninstaller needs it | PowerShell's `Start-Process` (Windows' own copy), which does the same; the program and what's passed to it go in as settings, never as script, so nothing in them can run as a command | same idea, new shape |
| Waiting for it | waits for the program started; one that copies itself to Temp and starts the copy (NSIS) seems done at once | waits for it and what it starts (so also for a browser it opens, if the browser wasn't open) | better |
| How it went | not told | told: done, cancelled, or ended with a code (so it may not have finished); the plugin list is checked again either way | better |
| `%ProgramFiles%` in the command | filled in as Windows' list is read | the same | same |
| Saying no to Windows' question | told | told, in any language Windows speaks | same |

## Moving plugin data to another drive

| What | C# | Rust | Verdict |
|---|---|---|---|
| How | copy, check every file's size, rename the original to "… (old copy)", a junction in its place; undo freshens the old copy and swaps back | the same steps, and the same checks before (gone, a link already, inside a moved folder, old copy there, inside the one moved, same drive, name taken, OneDrive files only online, room plus 2 GB) | same |
| Where moves are kept | `library.db` setting `space_moved_folders` (JSON) | the same JSON, read and written by `parse_moved` and `moved_json`, so moves made by either app are known to both | same |
| A network drive picked | refused on Windows, by the drive's type | refused on every system, also when the folder picked leads there through a link | better |
| The folder picked isn't there | made | refused, so a mistyped folder isn't made | better |
| A folder of that name appears while copying | copied into | the copy is made as a new folder first, so it stops instead | better |
| A link inside worded from where it sits ("..\Shared") | kept as worded, so on Mac it would lead somewhere else from the other drive | made again leading to the same real place | better |
| A folder Windows guards (needs administrator rights) | asks Windows' administrator question and does that one step as administrator | says so and moves nothing; the administrator step comes later | not yet |

## Not yet

- The administrator step for moving a folder Windows guards, and the moved-folder checks that come after a move:
  lost, replaced or doubled moves, linking again after an update, moving home, repointing to another drive.
- Keeping measured sizes between runs. The C# app keeps them in `library.db` (`plugin_space`, `folder_space`); how the
  Rust library keeps them is for the library's database design (rule 23: each fact once, nothing stored that's quick
  to work out).
