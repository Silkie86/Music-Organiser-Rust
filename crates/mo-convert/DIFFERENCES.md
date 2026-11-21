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
| A sample stretched to a set length in an FL 25 project | The Time knob's float read as a whole number, so the clip was stretched over millions of beats | Read as the float it is (see mo-formats) | Better |
| Audio clips when the template has no audio track | Listed, the rest converted | Same (the writer lists them) | Same |
| A VST2 program that can't be turned into a bank | Saved as `.fxb` anyway, so Bitwig can't load it | Kept as `.fxp` | Better |
| A VST2 with no id saved | Kept the template plugin's id under the new name | Id 0 | Better: Bitwig never loads the template's plugin by mistake |
| A template whose Sampler has no sample, or whose patterns share a track | Failed on the first song | Turned down when it's chosen | Better |
| MP3, OGG and M4A samples, and compressed WAVs | Length read through Windows' decoders | Timed by mo-audio's decoders from the file's start (whole-app review CN-10, 19 Nov 2026), so the Sampler gets the sample's real end and Bitwig its MD5; WAV, RF64, AIFF and FLAC are timed from their headers. One whose length a decoder could only guess (an MP3 with no length written in it, a bare .aac file) is left for Bitwig, and the list says the Sampler's end may need setting | Same |
| A WAV whose header says more audio than the file holds | Cut to the file | Same | Same |
| A WAV with an odd chunk left unpadded, or its audio more than 1 MB in | (not compared) | Read on: the unpadded place is taken when the padded one isn't a chunk, and chunks past the first 1 MB are looked at one by one | Better: such a sample got no length, so Bitwig asked where it was |
| A damaged FL save read on, or one too big to read whole | Converted without saying | Said first in the notes | Better (rule 9) |
| A song slower than 20 BPM (FL goes to 10) | (not compared) | Planned at 20 BPM, Bitwig's slowest (so its audio clips are timed for it), with a note, as its tempo lane already did | Better: said, never a tempo Bitwig can't play |
| A tempo a damaged save gives as 0 | (not compared) | FL's 140, with a note | Better: a 0 made clip lengths endless |
| The tempo's automation lane (whole-app review CN-2, 19 Nov 2026) | A copy of a Volume lane pointed at the tempo, its points a Volume lane's kind | Its points are the kind Bitwig saves for the tempo (class 0x387 in a list of 0x389), and the empty entry for the lane's setting gives way to the lane, as in Bitwig's own saves; the same for a Volume or Pan lane on the master. (Tempo moves recorded into FL patterns aren't carried at all yet: only a plugin's setting is read from edit events.) A template with a tempo lane of its own keeps it, and the list says so. Not yet opened in Bitwig | Better |
| The song's time signature (CN-6) | The template's stays | Set in the project, packed as Bitwig packs it (4/4, 3/4 and 6/8 are proved by saves); said when it can't be set | Better |
| The song's markers (CN-5) | Not carried; the template's own markers stay in every convert | FL's playlist markers become Bitwig cue markers, in time order, in place of the template's; the Songs preview's "As it opens in Bitwig" view shows them too | Better |
| An FL audio clip's fades (CN-3) | Not carried, not said; each clip kept the template clip's fades | Each clip is written with no fade, and a channel whose clips have fades is listed (what Bitwig's fade number counts isn't worked out yet) | Said |
| Notes panned or finely pitched in FL's piano roll (CN-4) | Lost without a word | Still not carried; the channel is listed | Said |
| A convert Bitwig may still have open, after the app restarts (CN-11) | (not compared: the C# app kept the opened names only while it ran, as this one did) | Remembered for a day in songs.db, so the next convert of the song takes the next name; and the save asks once more, as the last thing before the rename, that the old copy is still its own | Better |
| A song that changes time signature part-way | (not compared) | The project is in the one the song starts in, and the list says so | Said |
| A channel with notes and no clips, in a song not in 4/4 (CN-7) | One clip in bars of 4 beats | In the song's own bars | Better |
| A template whose sample was collected into its own folder (CN-12) | Every converted sample still says it's inside the project's folder | Never said to be inside; the places the template's file was found at before are the new file's | Better |
| The project's title and its list of collected files (Bitwig's browser, and the app's library, read them) | (not compared) | The FL song's title; no collected files (a convert's samples stay where FL has them) | Better: the template's title and samples never carry into every convert |
| Samples | Read one after another | Read on every core, after the template is checked | Faster; a bad template is said straight away |
| An earlier convert saved in Bitwig while samples were read | Replaced | Asked again just before saving, so it's kept, with its own list of what didn't come across (the new list is put in place only after the project) | Better |
| A sample that is not where FL saved it | Bitwig asks where it is | Looked for first in the project's and the DAW's folders (rule 13), and used where found, with a note | Better |
| FL Studio's own sounds (`%FLStudioFactoryData%…`) | The placeholder is written as the path, so Bitwig can't find them | Used from FL Studio's own folder on this computer, and only from there: never another file with the name (whole-app review CN-1, 19 Nov 2026). They keep Ogg Vorbis inside a WAV: timed by the decoder, and the list says Bitwig may not play that kind (not tried in Bitwig yet) | Better |
| Plugin sounds a save lost | Taken from the newest older save that has them, matched by id | Same, but matched by name when one save has no id (FL 9 may not save it), and a save before Fix's copy of it | Better |
| Samples found somewhere else | (not looked for) | Listed under their own heading beside the project | New |
