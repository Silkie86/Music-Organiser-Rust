# Music Organiser extension for Bitwig Studio

The Bitwig link as a Java controller extension (`MusicOrganiser.bwextension`), for Bitwig Studio 6.1 and later
(controller API 23). It replaces the JavaScript controller script in `../bitwig`, which stays in the repository
for now as the fallback. Both are the same controller to Bitwig (same vendor, model and ID), so a Music Organiser
controller already added in Bitwig stays added, with its *Let the app make changes* setting. Install one or the
other, not both.

What it does, its commands and their answers are in [the extensions README](../README.md#bitwig-link). It talks
to the app exactly as the script does, and adds what only an extension can do: Bitwig's newer API, reports
written to files, and (from 3.7) a MIDI loopback to run Bitwig's commands as a hardware controller would.

## Building

Needs a JDK (17 or later; Bitwig runs the extension on its own Java).

```
cd extensions/bitwig-java
./mvnw package          # mvnw.cmd package on Windows; or mvn package with Maven installed
```

That runs the tests and makes `target/MusicOrganiser.bwextension`. Maven gets Bitwig's API from Bitwig's own
repository (`com.bitwig:extension-api:23` from maven.bitwig.com); it's only needed to build, since Bitwig has it
when the extension runs, so nothing of Bitwig's goes in the extension or the repository. CI builds it too
(the `bitwig-extension` job) and keeps the file as a download called `music-organiser-bitwig-extension`.

## Installing

The app does it: building the app also builds the extension when a JDK is installed (on Windows,
`winget install Microsoft.OpenJDK.21`; see `MusicOrganiser.BitwigExtension.targets`), and then **Install** in
Settings → DAW links → Bitwig Studio puts the extension in place of the script. By hand: copy
`MusicOrganiser.bwextension` into `Extensions` in Bitwig's user folder (normally
`Documents\Bitwig Studio\Extensions`; on Linux `~/Bitwig Studio/Extensions`), and take
`Controller Scripts\Music Organiser` out of that folder if the JavaScript script is there. Bitwig picks the
extension up by itself (restart Bitwig if it doesn't). If the controller isn't added yet: Settings → Controllers → Add controller →
Music Organiser → DAW link.

### The MIDI loopback (3.7)

The extension has one MIDI in and one MIDI out, only for its loopback: it sends itself MIDI through the out, and
Bitwig hands it back on the in as if a device had sent it. Bitwig 6.1.3 ignored Save run straight away by the
link without these ports, and once they were set it saved from its flush (which 3.8 makes the default route) both
times, and straight away once in two. To set it up: in loopMIDI, add a
port called **Music Organiser Bitwig**, then in Bitwig, Settings → Controllers → Music Organiser, set both MIDI
in and MIDI out to it. Bitwig can fill them in by itself when it finds the port (the extension offers it for
auto-detection). Never use **Music Organiser**, the FL Studio link's port: FL Studio must never get what the
Bitwig link sends itself. The self-check and `hello`'s `loopback` say whether the loopback works. Bitwig 6.1.3
starts the extension, and the link answers, with its MIDI in and out unassigned, but it has only saved for the link
once they were set (the flush was never tried without them), so set them after installing 3.7 or later, whichever
route the app uses. See
[Routes and the MIDI loopback](../README.md#routes-and-the-midi-loopback).

## Reports

The extension writes to the app's data folder, next to MO Probe's reports:

| System | Folder |
|---|---|
| Windows | `%LOCALAPPDATA%\MusicOrganiser\reports\bitwig` |
| macOS | `~/Library/Application Support/MusicOrganiser/reports/bitwig` |
| Linux | `$XDG_DATA_HOME/MusicOrganiser/reports/bitwig` (or `~/.local/share/...`) |

- **`<date>-link.jsonl`** (UTC date): one line for each command the app sent (but a `hello` that went well: the
  app says hello every couple of seconds), with `time`, `cmd`, `ms` (how long
  the answer took), `ok` and `error`, and a line when the extension starts (`event: "start"`, with Bitwig's
  version), stops, and when the app connects or goes. Nothing from a project goes in beyond names, and the
  paths the app sent when an error mentions one.
- **`<time>-selfcheck.json`**: a self-check, written a second after Bitwig starts the extension (once Bitwig
  has sent the values it watches) and whenever the app sends `selftest`. It has Bitwig's version and API
  version, the Java version, the extension's version, whether changes are allowed, and a check for each part
  of Bitwig's API the link uses or offers (`name`, `ok`, `detail`, `ms`): the application, project, transport,
  the selected track and device and its parameters, drum pads, the 128 tracks with their devices, clip slots
  and IDs, the Sampler and Note Transpose pitch, the popup browser, the settings, the remote controls, the last
  clicked parameter, parameter steps, origins and automation, the master recorder, the arranger (its timeline
  and clip launcher, the panel layout, the play start, the clip selected there and launcher clips' colours, for
  `arrange`), the clips selected in the arranger and the clip launcher with their notes (for `notes`), where a file can go after each track (for `filetrack`) and Bitwig's
  actions, the command routes (the loopback's MIDI ports, the buttons the link made, and whether its own MIDI
  has come back, with how to set the loopback up when it hasn't), then whether the app can connect (port 19791)
  and whether the reports folder takes files. Each part
  is set up on its own, so one that fails in a future Bitwig leaves the rest working, and the self-check says
  which and why.

Reports older than 14 days (by the date their names start with) are deleted when the extension starts; other
files in the folder are left alone. Writing is best effort: a report that can't be written never stops the link,
and the self-check says what went wrong.

## What's new in link version 3

`hello` says `link: 3`, `runtime: "java"`, `extension` (this extension's version) and `loadedFrom` (the file Bitwig loaded it from, when it can tell, so the app can put the next version there). (`engine` still says
whether Bitwig's audio engine is on.) The rest is additions, which the app can ignore:

- `project`: each track's `id`, the ID Bitwig keeps it under, once Bitwig has said.
- `device`: the selected device's remote controls on the page Bitwig shows, as `page` and `controls` (`index`,
  `name`, `value`, `display`, `steps`, `stepNames`, `origin`, `automated`, and `param`: the parameter it is,
  when the link can tell). A parameter that is one of those controls carries the same `steps` (-1 for a
  continuous value), `stepNames` (when Bitwig names them), `origin` (0 to 1: where the value rests, 0.5 for a
  pan) and `automated`. Bitwig's API gives these only for remote controls and the last clicked parameter, not
  for every parameter by its ID, and doesn't say which parameter a control is; the link matches a control to
  the one parameter with its name and value.
- `clicked`: the parameter last clicked in Bitwig: `exists` (false until one is clicked), `name`, `value`,
  `display`, `steps`, `stepNames`, `origin`, `automated`, and `locked` (whether it's pinned in Bitwig).
- `recorder` ✎ with `do`: `"start"`, `"stop"` or `"status"` (the default): Bitwig's master recorder, answering
  once it has started or stopped with `active` and `duration` (in milliseconds).
- `selftest`: runs the self-check, writes it to the reports folder and answers with it, plus `file`.
- `notes` (extension 3.15): the notes of the clips selected in Bitwig, for the Playground's Get chords from
  Bitwig. It only reads, so it doesn't need changes allowed. The answer's `clips` has one entry for each clip
  there is, the arranger's selected clip first, then the clip launcher's: `from` (`arranger` or `launcher`),
  `track` (the name of its track), `onSelectedTrack` (whether that's Bitwig's selected track), `start` and
  `length` (its loop, in beats), `cut` (true when it's longer than the 256 beats the link reads) and `notes`, by
  start, then key, on every MIDI channel: `start` (beats from the clip's start, to the 16th note the link reads
  it at), `length` (in beats), `key` and `velocity` (0 to 127). With no clip selected in either it fails: "No
  clip is selected in Bitwig. Click your chord clip in the arranger or the clip launcher, then try again."
- `arrange` ✎ with `track`, `slot` and `at` (optional, in beats): **an experiment** (extension 3.6), which only
  the live tests send. Bitwig's API can't put a clip on the arranger timeline, so the link copies a launcher
  clip and pastes it there, as a person would. Paste puts down whatever is on Bitwig's clipboard, which may be
  something copied before when the copy didn't take, so before it copies the link gives the launcher clip a
  fresh colour (the test project's clip keeps it) and checks the pasted clip has it. When Bitwig rounds that
  colour to one of its own, other clips can have it too, so the link can't be sure then. The answer says what
  Bitwig showed before and after: `arranged`, whether it worked (`appeared` and `sameAsSource` both true, and
  the mark `exact`); `appeared`, when a clip showed up selected on the track after the paste; `sameAsSource`,
  when that clip has the launcher clip's marking colour; `copiedFromTimeline`, when the timeline had the focus
  as it copied, so it didn't paste; `layout`, `launcher`, `mark` (the clip's colour `before`, the one `asked`
  for, the one it `got`, whether it `took` and whether it's `exact`, the very colour asked for),
  `launcherFocus`, `timeline`, `focus`, the clip selected in the arranger `before`, `beforePaste` and `after`,
  its `pastedColor`, and the track's `launcherClipsBefore` and `launcherClipsAfter`. From 3.7 it takes `via`
  too (below), and says `via` and `ran` (whether each of `launcherFocus`, `copy`, `focus` and `paste` ran by
  it). From 3.8 it takes `checks: true`: when its paste ran but put nothing in the arranger, and Bitwig has the
  launcher's focus action, it then checks a paste into an empty launcher slot and Bitwig's Duplicate on the
  launcher clip by the same route, deletes a clip either makes in the launcher (a track Duplicate makes is left),
  and says what each found in `checks`. See
  [the extensions README](../README.md#clips-in-the-arranger-an-experiment).
- `filetrack` ✎ with `track`, `path` and `at` (optional, in beats): **an experiment** (extension 3.8), which only
  the live tests send. It inserts the file after the track as a new track (the track's
  `afterTrackInsertionPoint`, as a drop between tracks), with the play start moved to `at` first, and says where
  Bitwig put it: the new track's `index`, `name`, `type` and launcher clips, and the clip selected in the
  arranger before and after. From 3.9 it takes `trackId` in place of `track`, and says the new track's `id` and
  every track that's new (`newTracks`), found by ID.
- `record` ✎ with `trackId`, `slot`, `at` and `length` (in beats, whole bars): **an experiment** (extension
  3.9), which only the live tests send. It records the launcher clip into the arranger, as Bitwig's user guide
  says a person does, which needs no focus or selection (copy and paste act on Bitwig's own selection, which
  is why `arrange` failed). It refuses unless Bitwig is stopped with arranger record off and the slot has a
  clip. It keeps every setting it changes, then sets them so only the clip records and nothing is heard:
  metronome, arranger loop, punch in and out, both overdubs and both automation writes off, automation write
  mode touch, pre-roll none, launch quantize one bar, the master muted, every track disarmed (the clip's track
  armed only with `arm: true`), the tempo (`tempo`, optional, so a faster one takes less time), and the play
  start at `at`. It reads each back, and refuses, putting them back, when one doesn't take. Then it turns
  arranger record on, launches the clip (Bitwig starts from the play start), stops the clip's track in its last
  bar (it stops on the bar line), turns record off, stops Bitwig only while it plays or may be about to (Stop
  while stopped moves the play start to the beginning), pressing Stop again while it still shows playing and
  waiting until it has shown stopped for a moment, and returns the track to the arrangement. With `deleteLauncherClip: true`
  it deletes the launcher clip next, so what's heard after comes from the arranger alone. Then it plays the
  arranger back from `replayFrom` (`at` unless given) to a bar past the clip, listening to the track's notes, and
  puts every setting back, reading each back again. `record: false` plays the clip without recording it, to tell
  whether the link hears the track's notes. Tracks are found by their IDs, so one going in above changes
  nothing. Each step after a wait checks "Let the app make changes" and the track again. It stops (record
  off, Bitwig stopped, everything put back; the master stays muted while Bitwig still plays) when
  `recordabort` asks, when changes are turned off, when Bitwig stops the link, or on an error; when another project comes to the front, it
  leaves Bitwig alone. The answer: `decided` (`recorded`, `nothing`, `cantTell`, `heard`, `cantHear`,
  `refused`, `aborted`, `abandoned` and so on), what it `played` and heard (each note's `key` and `at`), the
  `replay` (notes heard `inPass` and `before`, and whether the launcher clip played), what it `seen`, the
  `tempo`, what was `refused`, what Bitwig showed `after` (and any setting that didn't come back,
  `restoreFailed`), and what changed in the tracks and launcher slots, by ID (`diff`).
- `recordabort`: asks a running `record` to stop (it answers `running`). It isn't a change, so it reaches the
  job whatever project is in front.
- `transport` (extension 3.16): for the Playground's Follow Bitwig, whether Bitwig is `playing`, where
  (`position`, in beats from the start), where Play starts (`playStart`), its `tempo` and `timeSignature`. The
  app asks many times a second while it follows, so these aren't logged.
- `stoptracks` ✎ with `trackIds` (extension 3.16): stops the launcher clips on those tracks and puts them back
  on the arranger, for the Playground's Play in Bitwig, answering with the IDs it found as `stopped`.
- `probe` ✎ with `via` and `gap` (optional, in ms): runs Bitwig's panel commands (`toggle_arranger`, then
  the focus actions of the clip launcher and the arranger) twice each, `gap` apart, by the route, and says for
  each whether its panel showed before and after each go (`visible`), whether the route `ran` it, and whether
  the link put the panel back as it was (`restored`). A panel that shows and hides with them means Bitwig
  carried them out for the link.
- From 3.18, a change still waiting behind others when its app closes the connection (it gave up waiting) isn't
  made, so asking again doesn't make it twice (apart from `stoptracks`, which the app sends as it closes), and a
  `replace` into an empty slot waits as long as any new clip.
- From 3.17, `clip` with `launch` launches the clip in step with the song (synced), so a swapped part carries on
  with the others instead of starting again from its own start, and a `replace` waits for the clip's name to change
  (the app names each file it swaps in differently) rather than a fixed moment, before it launches.
- From 3.16, `transport` says where Bitwig is playing, `stoptracks` stops tracks' launcher clips, and `clip` takes
  `replace` (swap a slot's clip for the file) and `launch`, for the Playground playing along with Bitwig.
- From 3.15, `notes` answers with the notes of the clip selected in the arranger and in the clip launcher, for the
  Playground's Get chords from Bitwig.
- From 3.14, an effect after a pad's empty Sampler is one Bitwig calls `audio-effect`, as well as `audio_to_audio`.
  The 2026-10-19 live run showed Bitwig 6.1.5 calling devices `instrument` and `note-effect`, not the
  `note_to_audio` the link was written for, so a pad with an effect after its Sampler would have counted as taken.
- From 3.13, `device` gives a parameter on the remote controls page Bitwig shows its control's value, and gives
  `"value": null, "stale": true` for one whose text has changed since Bitwig last told the link its value. In
  the 2026-10-18 live run, Bitwig 6.1.5 showed each new value of MO Probe's knobs (CLAP and VST3), turned by
  the probe or by the link, but never told the link the numbers, so it went on giving the old ones.
- From 3.12, `record` goes by the clock until Bitwig's play position first moves. As it starts, Bitwig can still
  say where the go before stopped (the third live run: 24.2 for a clip at bar 5), which had the link stop the
  track as the clip launched, so the take got only a bar of it, or one note.
- From 3.11, `record`'s answer has a `trace`: a line for each look (every 25 ms) and each command it sent, with
  Bitwig's play position and whether it played, the slot played or was queued, and how many notes the track
  played ("1234 look @16.042 playing slot notes 1"). It's there to work out runs like Solomon's second, where
  the unarmed go's clip played only its first bar and the link's stop came at beat 24.3, not 20.
- From 3.10, when Bitwig goes on playing the clip's track past the clip's end (Solomon's first run: it ignored the
  track's stop with the track unarmed, while armed goes stopped on time), `record` ends just past the end with
  record off first (`played.ended: "cut"`), so the take can't run long, and it returns the track to the
  arrangement again just before the replay. Recording armed is the way the app records.
- From 3.9, `newtrack` says the new track's `id`, and `clip` takes `trackId` in place of `track`. The
  self-check says how Bitwig is set for recording (arranger record, pre-roll, launch quantize, automation write
  mode and the master's mute).

Not new to version 3, since the script does it too: any request can carry `onlyIn`, a project's name, and the
link refuses it, or stops a change part way, unless Bitwig has that project open; `hello` says `onlyIn: true`.
The live tests use it to keep their changes in their own project (see
[the extensions README](../README.md#only-in-one-project)).

Also not new to version 3: from extension 3.7, `action` answers with `project`, the project Bitwig had in front
just before the action ran (the script does the same), so the live tests can tell which project a Save went to.
Earlier versions answer `{}`. If the link can't read the project's name, it still runs the action and leaves
`project` out.

New in 3.7, the command routes: `action` and `arrange` take `via`, the route to run Bitwig's commands by (Save,
Copy, Paste, the two focus actions and, from 3.8, Duplicate, which arrange's checks run): `direct` (straight away, the default before 3.8 and the only route the
script has), `flush` (from Bitwig's next flush, the default from 3.8, as Bitwig 6.1.3 saved from there), `midi` (when a SysEx message the link sent itself comes back through
the loopback), `buttonCode` (when a press of one of its own hardware buttons comes back, from a note on channel
16, and the link runs the command) and `button` (another button, which Bitwig runs the command for itself). The
answer says `via` and `ran`. A route that hasn't come back within a second is dropped, so a late message or
press never runs the command, except a late `button` press, whose command Bitwig runs itself before the link
hears of it. `hello` says `loopback`:
whether the link's own MIDI has come back, from the message it sends itself as Bitwig starts it and each time
the app connects.

## Code

`src/main/java/org/musicorganiser/bitwig`:

- `MusicOrganiserExtensionDefinition`: who the extension is to Bitwig (listed in
  `META-INF/services/com.bitwig.extension.ExtensionDefinition`).
- `Link`: the link itself, a port of `MusicOrganiser.control.js` function by function, plus the newer API,
  the self-check and the command routes. Everything runs on Bitwig's controller thread.
- `Json`, `Framing`: a small JSON reader and writer (so the extension needs nothing but the JDK) and how
  messages travel.
- `Reports`: the reports folder and its files. The folder and the clock are handed in, so tests use their own.

## Tests

`mvn test` (or `./mvnw test`) runs the extension without Bitwig, in `src/test/java/org/musicorganiser/bitwig`:

- `FakeBitwig`: a stand-in Bitwig, a port of the script's (`../bitwig/test/fake-bitwig.js`) made from Bitwig's
  own API interfaces with `java.lang.reflect.Proxy`. As in Bitwig, reading a value the extension didn't mark as
  interesting in init fails, changes happen a moment after they're asked for and reach the extension bit by
  bit, and time only passes when a test says so. Calling anything it doesn't stand in for fails and names it.
  The tests talk to the extension as the app does: requests in through the connection's receive callback,
  answers checked as plain ASCII JSON lines. It also stands in for the MIDI ports, the hardware surface and its
  buttons, and a loopback (on by default, with a delay, or off), and knows which Bitwig callback is running
  (init, the connection, a task, the flush, SysEx, a hardware press, or the test itself), so a test can have it
  honour Save, copy, paste and the focus actions only in some of them, as Bitwig 6.1.3 seemed to. Its MIDI never
  leaves the test.
- `LinkTest`: every scenario of the script's tests (`../bitwig/test/link.test.js`), under the same names, plus
  two apps at once and a connection that breaks.
- `NewerApiTest`: link version 3's additions: track IDs, remote controls' `steps`, `stepNames`, `origin` and
  `automated`, `clicked`, `recorder` and `notes` (with the stand-in's cursor clips and their note grids).
- `ArrangerTest`: the `arrange` experiment, with the stand-in's arranger, clipboard and focused panel (copy and
  paste work in the panel in front, and carry the clip's colour): a clip copied into the arranger at bar 5 in
  its marking colour, the Arrange layout switched to and a hidden clip launcher and timeline shown, the
  launcher given the focus first (twice when the first go hid it), no paste when the timeline had the focus as
  the link copied, each step Bitwig doesn't go along with (with nothing pasted), and the answers when Bitwig
  pastes into the launcher instead or ignores paste, without its focus action, or without an arranger for
  extensions at all. Also the colour mark: a clip already on the clipboard pasted when the copy didn't take
  (appeared, but not the same clip), a colour Bitwig ignores or rounds to its own (never `arranged` then, as a
  clip that was on the clipboard in the colour Bitwig rounded the mark to looks just the same), and a fresh
  colour for each arrange. From 3.8, the checks: a paste in the launcher that made the marked clip and a
  Duplicate there (both clips deleted again), a copy that didn't take, a Duplicate that went to the track, a check
  with nothing to work on, a check stopped part way, and no checks unless asked, when arrange didn't paste, when its
  paste put something in the arranger (rounded mark or not, on the track or another), when a clip on the track was
  selected in the arranger already, or without the launcher's focus action; and
  `filetrack`, with the file going to the arranger, the launcher, or nowhere.
- `RecordTest`: the `record` job, with the stand-in's transport (it plays on the stand-in's clock, records a
  launched clip into the arranger while record is on, and plays that back): a clip recorded at bar 5 and heard
  in the replay, with everything put back and Stop never pressed while stopped; each setting set (and read
  back) before record goes on; a play without recording; the answers when the link hears no notes, or Bitwig
  records nothing, or only an armed track; a faster tempo with the launcher clip deleted; a track that doesn't
  stop; `recordabort`, Bitwig stopping the link and a setting that doesn't take; another project in front; a
  track going in above; automation write coming on as it records, and a play position that doesn't move; odd
  requests; `newtrack`'s ID and `clip` by ID; `probe`; and the self-check's line.
- `RoutesTest`: the command routes: Save by every route, the MIDI each sends, a route that never comes back
  (`ran: false`, and a late message or press doesn't run it, nor does a stale message or the link's own ping that
  comes back while a newer command waits), a Bitwig that honours commands only for hardware or
  only in SysEx, the callback each route runs in, an unknown route, `onlyIn` and changes checked again as a
  routed command runs, `hello`'s `loopback`, the self-check's detail, a Bitwig without MIDI ports, and `arrange`
  by the buttons, refused routes and no paste when the copy didn't run. From 3.8, a command with no route runs
  from the flush, and arrange's checks stop before their paste and Duplicate when the launcher's focus action
  doesn't come back (its first run, or the one that would show the launcher again after it hid it).
- `SelfCheckTest`, `LinkLogTest`, `ReportsTest`: the self-check and the reports, in a temporary folder,
  including a Bitwig missing part of the API (each part fails on its own), another port, an older API and a
  folder that can't be written.
- `ParityTest`: sends the same few hundred requests, plenty of them odd, to the script (in its own stand-in, run
  with Node.js by `src/test/js/run-script.js`) and to the extension, and checks every answer is the same apart
  from version 3's additions. It's skipped without Node.js, except on CI.
- `JsonTest`, `FramingTest`: the JSON reader and writer, and how messages travel.
