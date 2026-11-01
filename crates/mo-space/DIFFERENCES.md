# Space: where Rust differs from the C# app

The C# app's `Core/Space` is the reference (`DiskSpace`, `SongSpace`, `AppFiles`, `RecycleBin`). Each difference is
listed with a verdict: **better** (on purpose), **same idea, new shape**, or **not yet**.

## Measuring

| What | C# | Rust | Verdict |
|---|---|---|---|
| Walking a folder | one thread, file by file | a level of folders at a time over every core of the pool it's called in (rule 21) | better |
| Links, OneDrive online-only files | not followed / left out of sizes | the same | same |
| A file with two names (a hard link) | counted once for each name | a folder's size (`measure`) counts it once on a Mac or Linux; Windows can't tell from here, and the other walks (a song's space report, Clean up's look) still count each name | better |
| `size_text` rounding | .NET formatting | the same, halves rounded up as .NET does | same |
| Stopping part way | `CancellationToken` | a stop flag, looked at before each folder is read and every 256 entries in one; a stopped measure says `Stopped` rather than giving a part answer | same idea, new shape |
| Compact line notes | text built when measured | a `note()` worked out from the line's fields (kind, time, the save it copies) | same idea, new shape |

## Clean up

| What | C# | Rust | Verdict |
|---|---|---|---|
| Checking before removing | the list was trusted as measured | every picked line is checked by a guard at the preview **and again** just before it goes: inside a project folder, not a project folder itself, not in or holding a sample folder, not a sound any project uses by name, not a song's newest save, really the kind the line says (a save, an autosave below the project folder, Studio One's Cache folder, audio), not a link, not on a network drive (no Recycle Bin; on a Mac or Linux, network shares mounted into a folder too) | better |
| A copy and its twin | the measure never lists both | a copy whose twin is also picked stays; at clean-up a copy only goes while its twin is still byte for byte the same | better |
| Changed since you looked | removed anyway | a file whose size or time isn't what the list measured stays, and so does a file that changed between the preview and the clean-up (even at the same size). Studio One's Cache folder is compared by the folder itself only (a file rewritten deep inside doesn't count): it's rebuilt anyway | better |
| Links | the path as written | paths are checked as written and as they really are (links, `..` and mapped drives worked out): a link out of the project folder, into a sample folder, or a copy whose twin is a link to it, stays; a song folder that's a link isn't the song's own | better |
| Undo | find it in the Recycle Bin yourself | after a clean-up each thing is found in the bin (one that isn't is reported: Windows removes for good, after asking, what's too big for its bin); the `Done` list, kept by the app, is what `undo` puts back; on a Mac, Finder's Put Back. Windows lists a file without its ending when Explorer hides endings (as it does at first): it's still found, by the ending of the file the bin keeps, and comes back under its whole name. Undoing a clean-up again never puts back something binned later from the same place | better |
| Thinning, default | the setting's default is 3 hours | the same; and the newest save is never thinned even when another was written at the same moment (C# could thin it) | better |
| Removing for good | never: Recycle Bin only, Windows asks before deleting anything too big for it | the same (the `trash` library sets the same Windows flags) | same |
| What the Recycle Bin can't keep | each clean-up checked network drives and OneDrive files itself; a USB stick or memory card wasn't checked, so Windows asked to delete for good | each clean-up still checks them itself, and the one route every clean-up sends through (`Sending::send`) checks once more: anything on a network drive or (on Windows) a USB stick or memory card, or a OneDrive file that's only online (or a folder holding one, on Windows; a folder in it that can't be read, such as one whose list of files is still only online while OneDrive isn't running, counts as can't tell, so it stays), stays with why, whichever clean-up it came from (a moved folder's old copy on a network drive now stays: delete it yourself) | better |
| Sending, whichever clean-up | n/a | the one route also never sends a link (the bin would take the link, but the administrator step where it leads); sends one thing reached two ways once; and once you press Stop, what hasn't gone stays. What waits for the administrator step is checked once more just before Windows asks: still not a link, and still where it was really (a folder on the way that leads elsewhere now keeps it) | better |
| Files Windows guards | binned in one administrator step (plugin clean-up), as listed | the same for songs and plugins: one administrator step at the end for all of them, so Windows asks once. Each is checked again just before Windows asks, and the step leaves anything that changed while it asked; what's gone afterwards is what counts as removed (see [Administrator step](#administrator-step)) | better |

## Plugins

| What | C# | Rust | Verdict |
|---|---|---|---|
| Plugin files under linked folders | .NET's walk goes through links | the same, each real folder once, so a link back up can't loop | same |
| A family's letters | .NET `char.IsLetterOrDigit` | Rust's alphabetic or numeric: the same for every name seen; a few rare scripts' marks count as letters too | same idea, new shape |
| Folders in a data root, order | as the folder lists them | by name, so two runs give the same order | better |
| Measured when | stored with each plugin | not kept here: the caller knows when it measured (rule 23) | same idea, new shape |
| A folder naming a version | the comment says "Serum 2 Presets" is Serum 2's alone, but `Family` strips the version, so Serum and Serum 2 shared it | a folder naming a version ("Kontakt 8 Library", "Serum 2 Presets") is that version's alone; one naming none ("Kontakt Library") stays shared | better |
| A folder you added | counted for you, and for any other plugin whose guess took it too (twice) | shared with any other plugin whose guess takes it, or that has it added too, so it's counted once and stays on one's clean-up card while the other is used; rule it out for the other to make it yours alone. One you added inside another counted folder (a maker's, say) is taken off that one, so it's counted once | better |
| A plugin folder ruled out inside a maker's folder | still taken off the maker's folder, so counted nowhere | stays part of the maker's folder | better |
| A plugin file that's a link | measured as the link (nothing) | measured where it really is; a link to a file the plugin has listed already isn't counted again | better |
| Linux VST2 plugins | `.so` files not found | found | better |
| Documents | Windows' own Documents, wherever it's moved | the same (Windows' known folders) | same |
| Spare copies: same plugin? | one family anywhere inside the other ("Serum FX" took Serum's projects); versions ignored, so Ozone 9's projects counted Ozone 11's VST2 | by name as the Plugins page groups them (endings like "x64" and "(VST3)" ignored, but versions not: Ozone 9 and Ozone 11, TR-808 and TR-909 are different plugins), equal, or one with its maker in front ("FabFilter Pro-Q 3" and "Pro-Q 3") | better |
| Spare copies' reason | text stored with each copy | a `Why` worked out into text when shown | same idea, new shape |
| Is Pro Tools here (are AAX copies spare)? | its usual folder | its usual folder; on Windows also any Pro Tools among the installed programs (another drive, or another name); on a Mac any Pro Tools app ("Pro Tools First", …) in the Applications folders or a folder in them. One left listed after it was removed still counts, so AAX copies stay | better |
| Removing spare copies | found again against your projects as they are now, then to the Recycle Bin, Windows asking once for Program Files | the same, and each is also checked just before it goes: still there, a plugin file or bundle of its format, not a link, not only online in OneDrive, not on a network drive, holding just what it held when found (each file, its size and time, and where it really is: an update since, even one of the same size, or a folder on the way that leads elsewhere now, keeps it); and the copy your projects use must still be there and hold something here (a link whose file has gone, an empty bundle left behind or one only online doesn't count). When it isn't spare any more, it says why (Pro Tools may be installed since, or a project saved since). One file picked twice by two paths (a linked folder) goes once. A copy that can't go (a link, on a network drive, only online, unreadable) is still listed, with `can_remove` false. The ones Windows guards are checked again just before it asks, and once more by the administrator step | better |

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
| Administrator step | yes, one question for files Windows guards | the same, one question; Program Files still never goes in a clean-up (the card's rules), so it's for folders Windows guards elsewhere, such as ProgramData. A picked folder inside one that waits for the question goes with it, or, if that one stays after all (you said no), is tried on its own | same |

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
| Room on the other drive | the folder's size as measured, plus 2 GB, on the drive the path names | counted again (the size measured earlier only when it can't be), each file and folder as the drive keeps it, in whole pieces (128 KB each on a big exFAT drive, so a folder of many small presets takes far more there), leaving out the old copies of folders moved from inside it (they aren't copied), plus 2 GB, on the drive the folder really goes to (a link on the way is followed). The same when moving back or linking again | better |
| A big file while it copies | copied in one go: Stop waits for its end, and the bar doesn't move meanwhile | a file of 64 MB or more goes 8 MB at a time, so Stop takes hold within a piece and the bar moves through it. Smaller ones are copied by the system in one go. On Windows a big file keeps its marks (read-only, hidden) as before; on a Mac its extended attributes (Finder tags, say) aren't copied | better |
| Where moves are kept | `library.db` setting `space_moved_folders` (JSON) | the same JSON, read and written by `parse_moved` and `moved_json`, so moves made by either app are known to both | same |
| A network drive picked | refused on Windows, by the drive's type | refused on every system, also when the folder picked leads there through a link | better |
| The folder picked isn't there | made | refused, so a mistyped folder isn't made | better |
| A folder of that name appears while copying | copied into | the copy is made as a new folder first, so it stops instead | better |
| Files saved while it copies | only those in folders not yet copied go across | a second pass after the copy takes what was added or changed meanwhile | better |
| A file that goes while it copies (a plugin's lock file) | stops the move | passed over | better |
| Two names that differ only in capitals, to a drive that doesn't tell them apart | the second one stops the copy | the same | same |
| A link inside worded from where it sits ("..\Shared") | kept as worded, so on Mac it would lead somewhere else from the other drive | made again leading to the same real place | better |
| A link inside leading to another place in the moved folder | made again to the old place, so after moving back it can lead to the other drive | leads to the same place in the copy, wherever the copy ends up | better |
| A link inside that leads nowhere now (a drive not plugged in) | made again | made again (on Windows a folder link as a junction; a file link only where Windows allows one, otherwise left out and named by the check before the old copy is deleted) | same |
| Deleting the old copy | when the link reaches the copy | also only when nothing is only in the old copy, or newer there (`only_in_old_copy`, which the app shows first; you can still say you're sure), and when the old copies of folders moved from inside it earlier still have working links. An old copy that's a link now stays, and says so (where it leads is left alone) | better |
| Moving back with the old copy there | copies files that are newer on the other drive into it | copies files that are new or differ in size or time (an update can bring an older time), each under a temporary name first, so one that fails leaves the old file whole; the old copy's own newer files are kept in "… (newer files from the old copy)" beside the folder, never replaced; a second pass takes what was saved meanwhile; only with room for it plus 2 GB. Stopped or failing part way, it says the old copy was partly brought up to date and where its newer files are; trying again keeps them in the same folder, and `kept_beside` lists that folder meanwhile | better |
| Moving back once the old copy is deleted | copies the folder back | also checks this drive has room for it plus 2 GB, and that the link still leads to the copy the move made | better |
| Moving a folder whose link an update replaced with a real folder | moves it again, and the first copy is forgotten | refused, pointing at linking again (`after_move::link_again`), which puts it right | better |
| A folder Windows won't rename (access denied) | asks Windows' administrator question and does that one step as administrator; if even that's refused, "Windows wouldn't allow it even as administrator" | the same, and Windows asks before copying too (a move, or linking again after an update): the folder is renamed and straight back as administrator, so a folder even the administrator can't rename (the DAW has a file in it open) is found before a long copy, not after it. Then the copy is made as you, and Windows asks again for the swap (and once for moving back). If you say no at either question, nothing has moved (a copy already made goes). If even the administrator is refused, it says that something is probably using it and to close the DAW, the plugin and any window showing it (Windows refuses for that too). On a Mac or Linux (no question to ask) it says so before copying anything | better (two questions, but no wasted copy) |
| Something fails half-way and the folder can't get its name back | can say nothing was moved while the folder is left as "… (old copy)" or without its link | says what to rename back, and keeps every copy that holds the files | better |

## After a move

| What | C# | Rust | Verdict |
|---|---|---|---|
| Lost, replaced by an update, doubled by a later move | found the same way | the same (`lost`, `replaced`, `doubled`); the Rust move keeps links inside as links, so it never makes doubled ones itself | same |
| Folders left behind | `library.db` setting `space_old_homes` (JSON) | the same JSON (`parse_old_homes`, `old_homes_json`), plus where its files went (`Copy`) and what it holds (`Kind`), written only when there's something to say; the C# app skips over them when it reads | same, and more |
| Linking again after an update | new and newer files join the copy on the other drive, then the folder is set aside and the link goes back | files that are new or differ join it, each under a temporary name first; every file of the copy that's replaced is kept first in "… (kept from before)" beside it and listed back (the update's file may be newer, but yours was the one in use); a second pass takes what was saved meanwhile; only with room for it; a stop or failure partway says what's where | better |
| What linking again set aside or kept | the folder set aside is listed to delete | the same, at its real place (a second copy inside a moved folder is on the other drive); the files kept from before are listed too (and found beside the copy, `kept_beside`, even after a try that stopped partway), and since there's nothing to check them against they go only once you say you're sure | better |
| Deleting a folder left behind | when no moved folder or link is still in it | the same, and only when nothing is only in it, or newer there (`only_in_old_home`, shown first; you can still say you're sure), and not when a link is in its place now (removing that frees nothing) | better |
| Moving the folder the moved folders live in | renamed on the same drive, then every link pointed at the new place, outer ones first; copied and checked to another drive | to another drive the same, with the checks a move has (a link, inside itself, network drive, name taken, OneDrive, room) and a second pass. On the same drive the working links are pointed first (inner ones first, while they can still be reached), then one rename, then each is checked: if a link won't change, the rename fails or a link doesn't reach, it's all put back and nothing has moved. Not while a moved folder's own link (or its old copy) is in it, as it would go with the rename. Folders left behind that were in it follow it | better |
| Pointing links after you moved it yourself | each link pointed at the new place; a real folder where a link was is a problem, and its record stays at the old place | the same, but a real folder's record follows its copy, so it shows as replaced by an update and linking again puts it right (and the old place isn't held up by it). Folders left behind that were in it follow it | better |
| Pointing a link again that won't change | asks Windows' administrator question, once for each level of links (outer ones first) | the same; if even that fails, says so, and the old link stays (or it says which link to make). Moving the folder they live in on the same drive doesn't ask: when Windows won't rename it, or a link it guards would stop it, nothing moves and it says to rename it in File Explorer and then point the links at it here | same |

## Administrator step

The app starts itself again as administrator for one step and waits, as the C# app's `AdminStep` does: `--move-folder
check|swap|unswap|relink` and `--recycle-bin`, with a list in Temp, a result file and the list's fingerprint. Each app only
ever starts itself, so the two don't need to share the shape of these.

| What | C# | Rust | Verdict |
|---|---|---|---|
| Starting it | .NET's shell start with `runas` | PowerShell's `Start-Process -Verb RunAs` (Windows' own copy), the program and what's passed to it going in as settings, never as script. Windows is only asked once the app's start-up has run `run_step`, so the copy started as administrator can't be the whole app, hidden | same idea, new shape |
| Saying no | told | told, recognised by Windows' own wording of "cancelled" in whatever language it speaks (Windows PowerShell passes it on only as text; the uninstaller had the same slip, now fixed too) | same |
| Quoting what it's started with | each part put in quotes, so a path ending in `\` ran into the part after it | Windows' own rules (backslashes before a quote doubled) | better |
| Swapping a folder and swapping it back | the paths on the step's command line | in a list with its fingerprint, as links and bins are | same idea, new shape |
| The list | written over whatever was there | written only as a new file, and read once; a list that doesn't match its fingerprint, isn't text, or a result file already there does nothing | better |
| What it's handed | the paths as the app has them | each path's folder as it really is (a move recorded through a link to ProgramData is handed the real ProgramData), and the Temp folder by its real name, not Windows' short one | better |
| Before each line | binned, swapped or relinked as listed (a link's target checked) | checked again: a path reached through a link is left alone (so a link put in meanwhile can't steer it), and so is a result file; a link isn't binned; something to bin must still hold just what it held when the app checked it, just before Windows asked (each file inside, its size and time); a check or a swap only while the folder is still a real folder (and, for a swap, the copy a folder), an unswap only while the link still leads to the copy, and the "(old copy)" always beside the folder | better |
| How it went | the step's word for a link or a bin; the folder looked at for a swap | what's there afterwards for all of them: the link leads to the copy and the old copy is a real folder, the link leads to the new place, the file is gone; anything else says what's where | better |
| A line it didn't get to | "skipped" | stays, and says the step didn't get to it | same |
| Binned as another account | Windows asks a standard account for an administrator's password, and the step's files go to that account's Recycle Bin: "may have been removed for good" | says it went to the Recycle Bin as administrator and may be in that account's bin | better |

## Bitwig's multisample copies

| What | C# | Rust | Verdict |
|---|---|---|---|
| Finding them | not done | each project's `multi-samples` copies that are the same as a Library multisample byte for byte (SHA-256; only same-size files are read, a Library file once, and again only when its size or time changed; one that can't be read is tried again next time). Names only break ties, never decide: the Library can hold a "-2" copy that differs. A copy that's a link, or a `multi-samples` folder that's a link, is left out; a project's or template's own copies (anything in a `multi-samples` folder, also through a link) never count as Library files, so a copy is never pointed at another project's copy or at itself. A Library folder moved to another drive with a link in its place is read where it really is, its files keeping their paths through the link (a link further inside isn't followed, as in every walk), and a file reached two ways is listed and read once | new (Solomon, 31 Oct 2026) |
| What happens to them | not done | a "(fixed)" project pointing at the Library file is written first (the converter); only the copies it changed are offered for the Recycle Bin, after you've opened the fixed project (`remove_copies`). Each is checked again just before it goes: the fixed project is there and is a "(fixed)" one (never the original); the copy is a multisample file (not a link) in that project's own `multi-samples` folder (not a link either), on a drive with a Recycle Bin; the file used instead is in a Library folder, isn't a project's copy and isn't the same file on disk (a hard link, on a Mac or Linux); no other Bitwig file in the project folder names the copy (the converter's check, read again); and the two are still the same byte for byte, each Library file read once. So the same sound always stays in the Library: the one exception to rule 2, Solomon's choice (31 Oct 2026). Undo puts a copy back | new |

## Not yet

- Undo of something the administrator step binned: `undo` can only put it back where Windows lets you write, so it says
  it couldn't; Explorer's Restore asks Windows' question itself.
- Keeping measured sizes between runs. The C# app keeps them in `library.db` (`plugin_space`, `folder_space`); how the
  Rust library keeps them is for the library's database design (rule 23: each fact once, nothing stored that's quick
  to work out).
