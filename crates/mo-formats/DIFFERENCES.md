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
| The playlist track a clip is on, in FL 12.9.1 and later saves that keep fewer than 500 tracks (Garage party, Minibrutish, brazil rhyme_2) | Counts the tracks down from how many the save kept, not from 500, so no clip finds its track: every clip shows track 0, no track is muted, and patterns on muted tracks play (brazil rhyme_2 got the drums and piano its muted tracks 2 and 3 hold). | Counts down from 500 as FL does, so slot 499 is track 1, and clips on muted tracks are muted. | Better |
| An effect slot switched on and off by automation | Its curve slid between on and off. | Held, jumping between on and off as a channel's on/off does. | Better |
| MIDI clips in a save with no version | Picked each pattern's note size without the song's channels, so it could differ from the song's reading. | The same size the song uses. | Better |
| Notes in FL 4 to 8 patterns, in the MIDI, the channels and the song (about 3,600 of Solomon's old saves) | Reads the 20-byte notes as 24-byte ones, so most come out impossible: keys and velocities over 127, positions in the billions, clip lengths of 2 billion ticks, and a made-up channel ("Channel 32897") that pushes every channel after it along by one. | Reads them at their true size: every value in range (checked on 5 saves from FL 4 to 8, 2026-10-31). | Better |
| An effect slot switched on and off by a curve that ramps (a few of Solomon's FL 11 saves) | The ramp, as saved. | Each point held until the next, so a ramp that ends a clip above halfway reads as off. FL switches a slot on at half the curve: check that the held value then is right. | To check |
| The arrangement preview's playlist tracks (checked on Solomon's 11 FL saves and the test files, 2026-10-31) | Doesn't say which tracks are muted. | Marks muted tracks (the first arrangement's), so the preview can dim them. Everything else matches, apart from the rows above (notes in FL 4 to 8 saves, songs with only audio or automation in the playlist, effect slots switched by a ramp). | Better |
| The playlist track of a clip in a save from before FL 12.9.1 (Solomon's FL 11 "Fuego96 - SayWhat": 35 tracks kept, its clips on slots 189 to 198) | The song reader counted down from the tracks the save kept, so no clip found its track and mutes on those tracks didn't count; the arrangement preview counted down from 198 (or 98 when every clip fits under it). | Both count down from 198 (or 98) as the preview did, so the song and its preview agree on every clip's track (VERSION 12). | Better |
| The playlist of an FL 6.0 save (Solomon's jazzy_2, November 2006: 417 pattern clips) | Not read: FL 6.0 marks a pattern clip with 0x5000 where FL 6.0.4 to 8 write 0x6000, so the song had no clips and the preview said no patterns are in the playlist. | Reads both marks, so its 417 clips land on 33 tracks, matching FL's own playlist (VERSION 13). | Better |
| The arrangement preview "As it opens in Bitwig" | Clips on a muted playlist track drawn as playing; automation cut with its own straight-line cut. | Drawn muted, as the song reader leaves them out; the automation reader's own cut, so a jump at a clip's start shows the value after it. | Better |
| Edit events: knob moves recorded into a pattern (event 223; in 120 of Solomon's 744 projects, such as sly_15) | Not read: the moves are lost without a word. | A plugin setting's moves are read (FL 25's and FL 8's forms) and placed wherever the pattern plays, as an automation clip is, so the song has one more automation per setting moved; moves of anything else are listed as left out (VERSION 14). | Better |
| Samples on the channels inside a Layer channel | Counted as not played when only the Layer has notes. | The same, until the Layer's children are read (needs one of Solomon's saves with a Layer). | Not yet |
| Songs arranged in a later arrangement (FL 12.9 on) | Only the first arrangement is read. | The same: the twin test will show whether any of Solomon's songs need more. | Not yet |
| A VST hosted by FL 7 or 8 (wrapper version 4, such as sly_7's and sly_15's LegacyCell) | Read as having no name, id or settings, so a convert says the plugin is missing. | Reads its name, VST id and settings (FL 7 and 8 keep no file or maker). | Better |
| FL 8.5's notes (a census of 17,316 of Solomon's saves: FL 3.5 to 8.0 write 20-byte notes, 8.5 on 24) | Told apart by version, 24 from FL 9. | 24 from FL 8.5 too; before, an 8.5 pattern whose notes divided by both sizes was read as 20-byte notes, garbled (VERSION 16). | Better |
| FL 9.0 to 9.5 (and 11.1) playlists that mix pattern rows with clips in one event (902 of Solomon's saves) | Not read. | Each record read on its own kind: rows as pattern n on track n, clips on their own tracks (VERSION 16). | Better |
| FL 6 to 8 pattern clips saved as newer FL's (the pattern base, then the pattern) among the rows | Not read. | Read, on their pattern's track; the row is taken from bytes 12 and 14, as the census found (VERSION 16). | Better |

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
| A plugin state the zip says unpacks to more than 256 MB (a damaged or made-up zip) | Unpacked whole, however big, which can use up the memory. | Left out, like a state that won't come out whole. | Better |
| A reference to object 0, or to an object not read yet (a damaged file: Bitwig defines each object where it's first used) | Kept, and looked up once the whole body is read, or skipped. | The body isn't trusted past it: the project is read by the text scan, with a warning, so it has no MIDI. | Safer: a body that's wrong there is likely wrong after it too |
| A note's velocity as a MIDI 1.0 value (0 to 127) | Bitwig's 0-to-1 velocity rounded straight to 0 to 127. | Kept at MIDI 2.0's 16 bits, then rounded to 0 to 127 when a MIDI 1.0 value is wanted. About 3 in 10,000 velocities come out 1 apart (rounding twice); Bitwig's own whole-step velocities are the same. | Better (rule 22) |

## Studio One

On the test files the two read the same; these are the places the Rust reader was written to differ, for files the
tests don't have.

| What | C# | Rust | Judged |
|---|---|---|---|
| A song whose mixer has an FX Chain with a splitter (`AudioFXGraph::SplitterNode` in `Devices/audiomixer.xml`) | Its fix for Studio One's `x:` prefix turns the name into `<:SplitterNode`, so the whole mixer fails to read and no mixer effect is listed (679 of Solomon's songs). | Reads the mixer and lists every effect. | Better |
| A plugin state in the song's zip that won't come out whole | No presets at all from the song. | That one preset is left out; the rest are taken out. | Better |
| An XML file in the song's zip that won't come out whole | The song can't be read. | The rest is read, with a warning naming the file left out. | Better |
| A file in the song's zip that unpacks to more than 256 MB (a damaged or made-up zip) | Unpacked whole, however big. | Left out, as a file that won't come out whole is. | Better |
| XML whose DOCTYPE declares its own entities (`<!ENTITY`; Studio One never writes them) | Expanded, up to 10 million letters. | The file is refused as unreadable: a few KB of entities can stand for gigabytes. | Safer |
| A damaged zip that names two files the same | Reads the first of them, and lists (and counts) every copy's presets. | The zip reader keeps only the last one's bytes, and the song says so in a warning. | Same for real songs |
| A note's velocity | Rounded to MIDI 1.0's 0 to 127 as it's read. | Kept at MIDI 2.0's 16 bits (a 0-to-127 velocity from an older song by MIDI 2.0's own scaling), rounded to 0 to 127 only when a MIDI 1.0 value is wanted. As with Bitwig, a few in 10,000 of the 0-to-1 velocities come out 1 apart. | Better (rule 22) |
| The arrangement preview of a song that isn't a zip, or is cut short (checked against the C# reader on the test files and 007jah's 640 clips, 2026-10-31) | Stops with an error. | Shows an empty arrangement that says the song couldn't be read well enough to draw, as a Bitwig project or a song without its song.xml already does. Everything else matches. | Better |

## MIDI files

On the test files the two read the same; these are the places the Rust reader was written to differ, for files the
tests don't have.

| What | C# | Rust | Judged |
|---|---|---|---|
| A header or track length so damaged it points before the file's start, or past what a whole number holds | Stops with an error that isn't "not a MIDI file", so the file counts as failed. | Reads the tracks before the damage and stops there. | Better |
| A note's velocity | MIDI 1.0's 0 to 127. | Kept in MIDI 2.0's 16 bits, scaled up by MIDI 2.0's own rule, so each comes back as it was written. | Same values (rule 22) |
| MIDI 2.0 Clip Files (`.midi2`) | Not read. | Read: a part for each group and channel with notes, keeping MIDI 2.0's 16-bit velocities and note attributes. No twin to compare with, so the tests follow the MIDI Clip File and Universal MIDI Packet specifications. | New (rule 22) |
