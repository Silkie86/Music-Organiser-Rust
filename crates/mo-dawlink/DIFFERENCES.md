# DAW links: where Rust differs from the C# app

The C# app's `Core/DawLink/BitwigLink.cs` and `BitwigSend.cs` are the reference, with the Sounds page's `ToBitwig`
and `SendToBitwig` (in `SoundsViewModel.cs`). The extension in Bitwig stays the C# repo's (`extensions/bitwig-java`,
3.18; the script in `extensions/bitwig` is its fallback). Each difference is listed with a verdict: **better** (on
purpose), **same idea, new shape**, or **not yet**.

## The link

| What | C# | Rust | Verdict |
|---|---|---|---|
| Finding it | port 19791 on 127.0.0.1, then ::1 | the same | same |
| Messages | a 4-byte big-endian length in front; one that comes without it reads to its newline | the same; a length past 64 MB, or something that isn't the link's messages, is a broken stream | same |
| Requests on the wire | JSON with everything outside ASCII as `\u` escapes (System.Text.Json's default) | the same, and DEL escaped too, as the extension writes its own | same |
| Requests at once | several on one connection, each answer matched to its request by its number | one at a time on a `Link`, each waited for; something that wants two things at once opens two links (the link in Bitwig takes several connections, and makes the changes they ask for one at a time, in the order they came) | same idea, new shape (no async in the app's crates) |
| An answer that doesn't come in time | "Bitwig didn't answer in time."; the connection stayed open, and the answer was dropped if it came later | the same words, and the connection is closed: on Windows a read that ran out of time leaves a connection that mustn't be used again (it can lose what comes next), so the next send connects again | same idea, new shape |
| Messages that aren't the answer waited for | passed over | passed over, by their number | same |
| No greeting within 3 seconds | "Something answered on port 19791, but not the Music Organiser script in Bitwig." | the same, then "If Bitwig is busy (opening a project, say), try again in a moment.": the link greets from Bitwig's controller thread, which may be slow to come to it while Bitwig is busy | better |
| A greeting from an extension part of which didn't start | read as it came: with no `changes` in it, a send said "Bitwig isn't letting the app make changes" | `Error::NotStarted`, with the extension's own words for what didn't start | better |
| A connection that's closed | "Bitwig closed the connection." when Bitwig closed it while a request waited; "The connection to Bitwig is closed." for a request after; "Lost the connection to Bitwig." when it broke | the same, with the system's words for why it broke: "Lost the connection to Bitwig (the system's words)." | same |
| Words that point at Settings > DAW links | the C# app's own Settings | "the old app's Settings > DAW links": the Rust app has no page for installing the extension yet | same idea, new shape |
| Keeping changes to one project (`onlyIn`) | only the live tests set it; Send to Bitwig went on in whichever project was in front | every send keeps its changes to the project open as it began (`Link::stay_in_open_project`), so when another comes to the front part way, the link stops there instead of going on in that one | better |
| Telling that apart (`IsElsewhere`) | from the link's words | `Error::is_elsewhere`, from the same words | same |
| Everything else the link can do | project, device, set, clip, transpose, action, recorder, notes, transport, the live tests... | `Link::command` sends any command and gives back Bitwig's answer as it came; typed calls come with the pages that need them | not yet |
| The FL Studio link, watching both DAWs, the checks, the live tests | there | later in phase 5 | not yet |

## Send to Bitwig

| What | C# | Rust | Verdict |
|---|---|---|---|
| A sound | a new track named after it, the sound on a Sampler; "Bitwig didn't load Kick.wav", "Bitwig put it on X rather than a Sampler" | the same, with the same words | same |
| A sound that isn't there any more | "Kick.wav isn't at C:\\... any more." before anything is sent | the same | same |
| The Drum Machine's ID | read from Bitwig files the app knew (projects with one, the template, Bitwig's presets) the first time, kept in `library.db` (`bitwig_drum_machine`); with none of them, kits went on the Drum Machine selected in Bitwig | the ID the C# app kept, when there is one, else Bitwig's own `8ea97e45-0255-40fd-bc7e-94419741e9d1` (the one DrivenByMoss finds Drum Machines by; the format registry has it under field 0x99): no slow search, and a new Drum Machine from the first send | better |
| A Drum Machine that doesn't come | the setting is cleared, and kits go on the selected Drum Machine until the app restarts | `SentKit::drum_machine` is false; the screen then sends to `KitTarget::Selected` until the app restarts (`library.db` stays read-only, so the setting isn't cleared). A device Bitwig hasn't named yet goes ahead, and counts as no Drum Machine when the link then says it isn't one. One Bitwig named Drum Machine stays one when Bitwig is slow to show its pads: "Bitwig added the Drum Machine but didn't show its pads in time, so the sounds didn't go on it." | same idea, new shape |
| General MIDI pads | `KitPads`: kick on C1, snare on D1, clap on D#1, closed hat on F#1, tom on A1, open hat on A#1, crash on C#2, the rest on the free notes from C1 up | the same, twin-tested on the kit builder's ten pads | same |
| Something going wrong part way | an error, as if nothing had happened, while the track (and perhaps the sound) stayed in Bitwig | an error only when nothing changed; once a send may have changed something, the result comes back, with words for what may not have happened: "Bitwig didn't answer in time. There may be an empty track for it in Bitwig.", "The track is in Bitwig, but the sound may not be on it: ...", "The Drum Machine is in Bitwig, but the sounds may not be on it: ...". The link can stop a change part way (another project came to the front, changes turned off), after Bitwig made the track | better |
| A kit for the selected Drum Machine | Bitwig's words as an error: none selected, it isn't a Drum Machine, its pads aren't shown from the lowest note | the same words as an error, as nothing went on it; something that stops it after the sounds start going on gives back the result, with "The sounds may not be on the Drum Machine selected in Bitwig: ..." | better |
| Two sends at once | the kit button was off while a kit was sent; sounds could go at once, each on its own connection | the app's sends go in one at a time, in the order they came (each takes a number, as at a counter), so one send's new track never lands in the middle of another's, and a kit for the selected Drum Machine goes on what's selected once the send before it is done | better |
| A kit sound that isn't there any more | sent: the link waited 5 seconds for Bitwig, then "Bitwig didn't load Kick (C1)" | left out before sending, and said ("Kick (C1) isn't where the app last saw it, so it was left out."); with none of them there, nothing is sent ("None of the kit's 10 sounds are where the app last saw them (Kick.wav was at C:\\...), so nothing was sent.") | better |
| Pads the link can't take (two sounds for one note, a note past 127) | `KitPads` never makes them | refused before anything is sent, in words (`Error::Invalid`): only a mistake in the app could make them | better |
| The kit's card | "Kit #12 is in Bitwig", "10 pads on a Drum Machine", with which sound is on which pad in the page's status line | `SentKit::title` and `detail` (which sound is on which pad, the pads left alone, left out, or not loaded) | same idea, new shape |
| Counting a send in Most liked | recorded in `sounds.db` (`SoundEvent.Send`) | not yet: `sounds.db` stays read-only until the Sounds page is trusted | not yet |
| The rest of BitwigSend (an FL channel with its plugin, knobs and preset) | there | with the converter's pages (phase 5) | not yet |

## Tests

| What | C# | Rust | Verdict |
|---|---|---|---|
| What they talk to | the real script, run in the C# repo's JavaScript stand-in for Bitwig (needs Node.js) | `bitwig::fake`, a stand-in of the link's own in Rust that answers as extension 3.18 does, in its words; other crates' tests get it with the `fake-bitwig` feature | same idea, new shape |
| Twins | `PutsAKitOnADrumMachine`, `PlacesAKitTheGeneralMidiWay`, `WontSendThroughAnOldScriptOrWithChangesOff` | the same three, in `tests/send.rs` | same |
| Sends cut short | not for sends (the link's tests check a change kept to its project, and Bitwig going away) | the stand-in can bring another project to the front part way through a change, hang up, stay silent or answer late on any request, or turn changes off after its greeting, and each send is checked to say what may have happened | better |
