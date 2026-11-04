# DAW scripts

Small programs that run inside the DAWs. They are ported from the C# app's `extensions` folder and grow here.

| Folder | Runs in | What it does |
|---|---|---|
| `bitwig-java` | Bitwig Studio 6.1 and later, as the Java controller extension "Music Organiser" (`MusicOrganiser.bwextension`) | The Bitwig link: what Bitwig shows for the open project, and Send to Bitwig. Moved here from the C# repo (Solomon, 2026-11-03); the C# repo's copy gets no more changes. Built by CI (the `Bitwig extension` job); see its README. |
| `bitwig` | Bitwig Studio, as the JavaScript controller script | The older form of the Bitwig link, kept as the extension's fallback; the extension's ParityTest checks both answer alike. |
| `fl-studio` | FL Studio, as the MIDI controller script "Music Organiser" | The FL link (SysEx on the Music Organiser MIDI port), plus a one-off measurement: with `MEASURE_IN_LIVE_TEST = True` and the project "MO Live Test automation" open, it sets insert 1's fader to 0.00 ... 1.00 and reads the dB FL shows, sets the tempo to automation values 0.00 ... 1.00 and reads the BPM, puts both back, writes the readings in the first inserts' names, several to a name ("v0.80 0.0dB; v0.81 ...", "t0.50 266.0bpm; ...") and saves. FL Studio 2025 doesn't tell scripts the project's file name, so the project is known by its Project info title (F11), which must be exactly "MO Live Test automation", so Format Lab reads them from the .flp (FL scripts can't write files). |

The Bitwig test driver for Format Lab is in `tools/formatlab/daw/bitwig`.

Tests (`python -m unittest discover -s extensions/fl-studio/test`) run the script against a stand-in FL Studio; they
never reach a real DAW or MIDI port (rule 11).

## Bitwig link

Two builds of the same link, which talk to the app the same way:

- **`bitwig-java`**: a Java controller extension against Bitwig's controller API 23 (Bitwig 6.1 and later), built
  with Maven into `MusicOrganiser.bwextension`, which goes in `Extensions` in Bitwig's user folder. It's a port
  of the script, function by function, and adds what only an extension can do: Bitwig's newer API (link version
  3 below), reports it writes to files, with a self-check of every part of the API it uses, and (from 3.7) a MIDI
  loopback to run Bitwig's commands as a hardware controller would (see [its README](bitwig-java/README.md) and
  [Routes and the MIDI loopback](#routes-and-the-midi-loopback)).
- **`bitwig`**: `MusicOrganiser.control.js`, plain ES5 against Bitwig's controller API 18: the fallback, kept as
  it was (link version 2, without the version 3 additions).

They're the same controller to Bitwig (the same ID), so only one should be installed at a time.

The link listens on port 19791 (`host.createRemoteConnection`). Each message is UTF-8 JSON with a
4-byte big-endian length in front, which is how Bitwig's remote connections frame messages; the link
also ends each message with a newline, so the app reads them either way. Text outside ASCII is sent as
`\u` escapes. When the app connects, the link greets it with `{"event": "hello", "result": {...}}`.

Requests are `{"id": 1, "cmd": "...", ...}`. Answers are `{"id": 1, "ok": true, "result": ...}`, or
`{"id": 1, "ok": false, "error": "..."}` with the reason in words the app shows as they are.

| Command | Asks for | Result |
|---|---|---|
| `hello` | | `link` (this protocol's version: 3 from the extension, 2 from the script), `product`, `version`, `api`, `platform`, `port`, `project`, `modified`, `engine` (whether Bitwig's audio engine is on), `changes`, `onlyIn` (`true`: the link honours a request's `onlyIn`, [below](#only-in-one-project)). Version 3 adds `runtime` (`"java"`) and `extension` (its version), and from extension 3.4 `loadedFrom` (the file Bitwig loaded it from, when it can tell; 3.5 can tell in a folder with a space in its name, as Bitwig Studio's are). From extension 3.7 `loopback`: whether the extension's own MIDI has come back to it through the loopback, so the routes through it work ([below](#routes-and-the-midi-loopback)). |
| `project` | | `name`, `modified`, `tempo`, `timeSignature`, `playing`, `position`, `trackCount`, `tracks`: the first 128 tracks in Bitwig's flat list, each with `index`, `name`, `type`, `group`, `color`, `active`, `mute`, `solo`, `arm`, `volume`, `pan` (as shown, with `volumeValue` and `panValue` from 0 to 1), `deviceCount` and the first 16 `devices` (`name`, `preset`, `kind`, `plugin`, `enabled`). Version 3 adds each track's `id`, the ID Bitwig keeps it under, once Bitwig has said. |
| `device` | | The selected device: `track`, `device` (or null), `drumPads` (whether it has a Drum Machine's pads) and every parameter as `id` (what Bitwig saves it under), `name`, `value` (0 to 1) and `display` ("-6.0 dB"). Waits up to 2 seconds for the display texts. Version 3 adds the device's remote controls on the page Bitwig shows (`page`, and `controls` with `index`, `name`, `value`, `display`, `steps`, `stepNames`, `origin`, `automated` and `param`), and gives a parameter that is one of those controls the same `steps` (-1 for continuous), `stepNames`, `origin` and `automated`: see [below](#link-version-3). |
| `actions` | | Every Bitwig action: `id`, `name`, `category`, `menu` |
| `select` | `track` (its index in `project`), `device` (optional) | `{}` |
| `set` ✎ | `param` (an `id` from `device`), `value` (0 to 1) | `{}` |
| `insert` ✎ | `kind`: `vst3` or `clap` with `plugin` (the ID the plugin scanner reads), `vst2` with `plugin` as a number, `bitwig` with `device` (a UUID), or `file` with `path` (a preset, or a sample, which goes in a Sampler). Then `track` (optional) and `at`: `"end"` (the default) or `"start"` of its chain. | Without `track`, at the end of the selected track's chain: `{}`, since Bitwig quietly ignores what it can't use. With `track`, once Bitwig has added it: `added`, `index` and `device` (as in `project`), or `added: false`. |
| `newtrack` ✎ | `name`, `color` (optional, `#rrggbb`) | `index` and `name`. An instrument track after the last main track (before the effect tracks), selected. If adding it opened Bitwig's browser, the script closes it. |
| `clip` ✎ | `track`, `path` (a MIDI or audio file), `slot` (optional). From extension 3.16 also `replace` (true: swap the clip already in `slot`, which must be given, for the file) and `launch` (true: launch the clip once it's in, which starts Bitwig playing if it's stopped; from 3.17 in step with the song, and a replaced clip once its name has changed) | `slot`: where Bitwig made the clip, by default the first empty one in the first 8 scenes; `launched: true` when it launched it |
| `pads` ✎ | `pads`, each with `note` (0 to 127), `path` and `name` (optional). Then `track` and `device` for a Drum Machine's place, or nothing for the selected device. | `filled`, `replaced` (those of the filled that went in place of an empty Sampler), `skipped` (pads that already had something on them, left alone) and `failed`, as notes, and `taken`: what's on each skipped pad (`note`, `name`, `devices`, `first` device's name, its `sample`). Each sample goes on a free pad as if dropped there, in a Sampler. A pad whose first device is a Sampler with no sample, still named as it came (a Drum Machine preset can put one on every pad), with nothing after it but audio effects (up to 3), is free too: the sample goes in that Sampler's place, keeping the effects after it. |
| `transpose` ✎ | `track`, `device` (0 or 1), `semitones` | `device`, `value`, `display`. Sets a Sampler's Transpose (fractions allowed) or a Note Transpose's Coarse (whole semitones). |
| `action` ✎ | `action` (an `id` from `actions`), `via` (optional: the route to run it by; without one, `flush` from extension 3.8, as Bitwig 6.1.3 saved from there, and `direct` before; see [below](#routes-and-the-midi-loopback)) | `project`: the project Bitwig had in front just before the action ran, so the app can tell which one a Save went to with more than one open; `via`: the route; and `ran`: whether the link ran the action by it (a route through Bitwig's flush or the loopback that doesn't come back within a second doesn't, and the link then never runs it). From extension 3.7 and the script; older extensions answer `{}`, and the extension leaves `project` out if it can't read it. The script runs actions straight away only: it answers `via: "direct"` and `ran: true`, and refuses any other route with `This needs the Java extension 3.7.` |
| `clicked` (3) | | The parameter last clicked in Bitwig: `exists` (false until one is), `name`, `value`, `display`, `steps`, `stepNames`, `origin`, `automated`, `locked` |
| `notes` (3.15) | | The clips selected in the arranger and the clip launcher (arranger first), for the Playground's Get chords from Bitwig: each one's `from`, `track`, `onSelectedTrack`, loop `start` and `length` in beats, `cut` (longer than the 256 beats read) and `notes` (`start` and `length` in beats, `key`, `velocity`). Fails when no clip is selected |
| `transport` (3.16) | | For the Playground's Follow Bitwig, which asks many times a second (so it isn't logged): `playing`, `position` (where Bitwig is playing, in beats from the start), `playStart` (where Play starts), `tempo` (BPM) and `timeSignature` |
| `stoptracks` ✎ (3.16) | `trackIds` | Stops the launcher clips on those tracks and puts them back on the arranger, for the Playground's Play in Bitwig: `stopped` (the IDs of the tracks it found) |
| `recorder` ✎ (3) | `do`: `"start"`, `"stop"` or `"status"` (the default) | Bitwig's master recorder, once it has started or stopped: `active`, `duration` (milliseconds recorded) |
| `selftest` (3) | | The self-check (see [the extension's README](bitwig-java/README.md#reports)), written to the reports folder, with `file` |
| `arrange` ✎ (3) | `track`, `slot` (a launcher slot with a clip), `at` (optional: where in the arranger, in beats from the start; the play start by default), `via` (optional, from extension 3.7: the route for the copy, the paste and the focus actions, `flush` by default from 3.8 and `direct` before; [below](#routes-and-the-midi-loopback)), `checks` (optional, from 3.8: `true` to check, when its paste ran but put nothing in the arranger, whether a paste into an empty launcher slot and Bitwig's Duplicate work by the same route) | **An experiment**, from extension 3.6, which only the live tests send ([below](#clips-in-the-arranger-an-experiment)). Once its steps are through, whether it worked or not: `arranged` (both of the next two, with an `exact` mark), `appeared` (a clip showed up selected on the track after the paste, with the track's launcher clips as they were), `sameAsSource` (that clip has the launcher clip's marking colour, so it's a copy of that clip and not whatever was on Bitwig's clipboard before, unless Bitwig rounded the mark: see `exact`), `copiedFromTimeline` (the arranger timeline had the focus when the link copied, so it didn't paste), `at`, `layout` (before), `launcher` and `timeline` (`before` and `after`: whether the Arrange layout's clip launcher, and the arranger timeline, showed), `mark` (the fresh colour the link gave the launcher clip before it copied: the clip's colour `before`, the one `asked` for, the one it `got`, each `#RRGGBB`, whether it `took`, and whether it's `exact`: the very colour asked for, not one Bitwig rounded it to), `launcherFocus` and `focus` (how many times the link ran Bitwig's `focus_or_toggle_clip_launcher` and `focus_or_toggle_arranger`), `before`, `beforePaste` and `after` (the clip selected in the arranger: `exists`, and its `track`'s name), `pastedColor` (the colour of the clip selected in the arranger at the end, `#RRGGBB`, or null when none is), `launcherClipsBefore` and `launcherClipsAfter` (the track's clips in its first 8 launcher slots). From extension 3.7 also `via` (the route) and `ran`: for `launcherFocus`, `copy`, `focus` and `paste`, whether the link ran that step by the route (null for a step it didn't get to, and for `launcherFocus` when Bitwig has no such action). From 3.8, with `checks`, when its paste ran but put nothing in the arranger and Bitwig has `focus_or_toggle_clip_launcher`, also `checks`: `launcherPaste` and `duplicate`, each null when it couldn't run, and `error` when one stopped part way ([below](#clips-in-the-arranger-an-experiment)). |
| `filetrack` ✎ (3) | `track`, `path` (a file), `at` (optional: where to move the play start first, in beats from the start) | **An experiment**, from extension 3.8, which only the live tests send ([below](#clips-in-the-arranger-an-experiment)): inserts the file after the track, as a new track, through the track's `afterTrackInsertionPoint`, and answers once Bitwig settles: `added` (how many tracks), the new track's `index`, `name`, `type`, `launcherClips` (its clips in the first 8 launcher slots) and whether it's `selected` (null and false when there's none), `tracksBefore`, `tracksAfter`, `arranger` and `arrangerBefore` (the clip selected in the arranger, as `arrange` gives it), `at` (where the play start was), and `onTrack` (the given track's launcher clips `before` and `after`). |

✎ needs changes turned on. (3) only in link version 3, the extension. `insert` with a track, `newtrack`,
`clip`, `pads`, `transpose`, `recorder`, `arrange` and `filetrack` wait for Bitwig to make the change (up to 5 seconds for
each step), and run one at a time in the order sent; from extension 3.7 so does `action`, whose route may take up
to a second to come back. In the app this is `Core/DawLink/BitwigLink.cs`.

### Only in one project

Any request can carry `onlyIn`, a project's name. The link then refuses it unless Bitwig has exactly that
project open, with `Bitwig has "<open>" open, not "<onlyIn>", so the link stopped before changing anything
there.` A change that waits for Bitwig checks again when it starts (it may have waited behind others) and
before each of its steps after a wait, and stops with that error, since Bitwig's track bank, selected track
and selected device follow whichever project is in front: without this, a new track asked for in one project
could be named, coloured or selected in another that came to the front meanwhile. At the same points it checks
again that *Let the app make changes* is still on. An `onlyIn` that isn't text is refused (`onlyIn has to be a
project's name.`); null, or none, means no check. The live tests send it with every request that changes or
selects something, so a run only ever changes its own test project (see
[Only in a project made for them](#only-in-a-project-made-for-them)), and the Playground's Play in Bitwig
sends the project that was in front when it was turned on, so it waits while another is in front rather than
making tracks there. Both the script and the extension do it, and say so with `onlyIn: true` in their hello.

### Link version 3

The extension's additions are fields and commands the app can ignore, so the app talks to the script and the
extension alike. Bitwig's API tells a parameter's steps (and their names), its origin (where its value rests:
0.5 for a pan, 0 for a level) and whether it's automated only for a device's remote controls and the last
clicked parameter, not for every parameter by its ID, and it doesn't say which parameter a remote control is.
So `device` lists the remote controls of the page Bitwig shows for the selected device, and the link matches
each to the one parameter with its name and value (`param`), whose entry then carries the same details.

The extension also keeps reports in `%LOCALAPPDATA%\MusicOrganiser\reports\bitwig` (the app's data folder;
see [its README](bitwig-java/README.md#reports) for macOS and Linux): a line for each command (but the app's
regular hellos) in
`<date>-link.jsonl`, and a self-check of each part of Bitwig's API it uses in `<time>-selfcheck.json`, written
when Bitwig starts it and on `selftest`. A Bitwig problem in a real session can be read there afterwards.

### Routes and the MIDI loopback

Bitwig 6.1.3 ignored every command of its user interface the link ran (Save, copy, paste, bringing a panel to the
front), whether it ran them as a request came in or in a task it scheduled, while it carried out every other
change. The same Save works for controllers such as DrivenByMoss, which run it while Bitwig handles a press of a
hardware button. So from 3.7 the extension can run Bitwig's Save, Copy and Paste actions and the two focus actions
(`focus_or_toggle_arranger`, `focus_or_toggle_clip_launcher`), and from 3.8 Duplicate for `arrange`'s checks, by a route, which `action` and `arrange` take as
`via`. The live tests then found that Bitwig 6.1.3 saves from its flush once the extension's MIDI in and out are
set ([below](#routes-and-the-midi-loopback)), so from 3.8 that's the route the link takes when the request names
none (and so the app's own commands take it):

| Route | How the link runs the command |
|---|---|
| `direct` | Straight away, as the request comes in (as before 3.7, and the default before 3.8). |
| `flush` | From Bitwig's next flush, which the link asks for (the default from 3.8). |
| `midi` | When a SysEx message the link sent itself comes back through the loopback: `F0 7D 4D 4F 42`, three 7-bit bytes of the message's number, `F7` (`7D` is the ID for non-commercial use, so no maker's device takes it for its own). |
| `buttonCode` | When a press of one of the link's own hardware buttons comes back through the loopback: the link sends a note on MIDI channel 16 (from note 110, one for each command), Bitwig presses the button, and the link runs the command. |
| `button` | The same with another of the link's buttons (from note 100), which Bitwig runs the command for itself, as for a controller's button; the link only hears of the press. |

The last three need the loopback. In loopMIDI, add a port called **Music Organiser Bitwig**, then in Bitwig,
Settings > Controllers > Music Organiser, set both MIDI in and MIDI out to it. Never set them to **Music
Organiser**: that's the FL Studio link's port, and FL Studio must never get the notes and messages the Bitwig link
sends itself. The extension asks Bitwig for one MIDI in and one MIDI out, only for the loopback, and offers the
Music Organiser Bitwig port for both, so Bitwig can fill them in when it finds the port (auto-detection).
Bitwig 6.1.3 starts the extension with them unassigned, and the link answers. In the live runs without them
(extensions 3.3 and 3.6, which have no ports, so only `direct`) Save did nothing; with them set (3.7 and 3.8) Save
worked from the flush both times and straight away once in two. Nobody has tried the flush without them, so set
them after installing 3.7 or later, whichever route the app uses: the steps Settings shows after *Install* ask for them, and *Check
connection* says when the extension's own MIDI hasn't come back through them (the hello's `loopback` is false),
as while they aren't set; the extension can't tell that from a port that doesn't hand its MIDI back. A Mac or
Linux has no loopMIDI; a virtual MIDI port there (an IAC bus on a Mac) may do instead, which nobody has tried.

A route other than `direct` has a second to come back (through Bitwig's flush, or the loopback). When it doesn't,
the link drops it, so a message or press that turns up late never runs the command, and answers `ran: false`.
Before it runs a command that came back, it checks `onlyIn` and *Let the app make changes* again
([Only in one project](#only-in-one-project)). A press carries no number, so the link can't tell a late press from
the next one of the same button; and for `button`, Bitwig has run the command before the link hears of the press,
so the link can't hold that one back.

The extension sends itself a message as Bitwig starts it, and again each time the app connects, and says in its
hello (`loopback`) and its self-check (*command routes*) whether any of its own MIDI has come back, which is how
the app and the live tests know the loopback works. Without the loopback's ports, `action` and `arrange` refuse
the routes through it, saying so.

### Clips in the arranger (an experiment)

Bitwig's controller API (23) can put a clip in a launcher slot, but not on the arranger timeline: only launcher
slots have insertion points. The app would like what it sends (*To Bitwig* from FL Studio, the Kit builder) to
land in the arranger, so extension 3.6 tries the only other way, Bitwig's own editing, as a person would do it.
`arrange` switches to the Arrange layout if another one shows, shows its clip launcher if it's hidden, selects
the track and its launcher clip, gives the clip a fresh colour, gives the clip launcher the focus, copies the clip
(`Application.copy`), moves the play start to `at`, brings the arranger timeline to the front and pastes
(`Application.paste`), which puts the copy on the selected track. Each step waits for Bitwig to show it, and the
request fails, saying which step, when Bitwig doesn't (all but the colour: see below).

Paste puts down whatever is on Bitwig's clipboard, which every open project shares. When the copy didn't take,
that's whatever was copied before: a clip you copied in your own song, or the last run's copy. To tell its own
clip from that, the link marks the launcher clip with a fresh colour before it copies (`mark`): a new hue each
time, a golden-ratio step round the colour wheel from the last one, and clearly unlike the clip's colour then.
It waits for Bitwig to show the new colour (Bitwig may round it to one of its own, so any clear change counts)
and keeps the colour the clip `got`, which the pasted clip's colour is compared with. The mark is `exact` when
that's the colour asked for (within 2 of 255 in each of red, green and blue). A colour Bitwig rounded is one of
its own, which other clips can have too (one you copied, or an earlier run's copy whose mark Bitwig rounded the
same way), so the link can't be sure a pasted clip in it is the launcher clip's copy. When Bitwig doesn't change
the clip's colour, the link carries on, with `took` false, but can't tell its clip from another. The launcher
clip keeps its new colour: only the test project gets this.

Bitwig copies and pastes in whichever of its panels has the focus, and the API can't say which one has it. The
link brings a panel to the front with Bitwig's actions `focus_or_toggle_clip_launcher` (when Bitwig has one by
that ID: nobody has seen Bitwig's list yet, so without it the link copies from whichever panel has the focus) and
`focus_or_toggle_arranger`. Each hides its panel when that's in front already, so the link runs it a second time
when it does. When `focus_or_toggle_arranger` hid the timeline, the timeline had the focus as the link copied
(nothing in between moves it), so Bitwig copied from the timeline, not the launcher clip: the link doesn't paste,
and says `copiedFromTimeline`.

Nobody knows yet whether Bitwig goes along with copy and paste from an extension, so once the steps are through
the answer always says what Bitwig showed. `appeared` is true when the clip selected in the arranger (Bitwig's
arranger cursor clip, which follows a pasted clip) has moved to the track, where it wasn't just before the paste,
and the track's launcher clips are as they were; more launcher clips after than before means Bitwig pasted into
the launcher instead. `sameAsSource` is true when the mark took and the clip selected in the arranger has the
colour the launcher clip got (`pastedColor`, within 2 of 255 in each of red, green and blue), and `arranged` when
both are and the mark is `exact`. A clip that appeared in another colour is whatever Bitwig's clipboard held
before. The API can't say where an arranger clip starts, so `at` is where it was meant to go, and only a look at
Bitwig shows whether it's there. Copy replaces whatever was on Bitwig's clipboard (one for every project open),
and nothing is undone when it doesn't work (the layout, the play start and the launcher clip's colour stay as
the link left them), so only the live tests send it, in their own test project (*Changes › Clip to arranger*);
the app's sends don't use it yet. In the app it's `BitwigLink.ArrangeAsync`.

From 3.7, `arrange` can run its copy, its paste and the two focus actions by a route
([above](#routes-and-the-midi-loopback)): `direct` works as before, with the API's copy and paste; any other
route (`flush`, the default from 3.8) runs Bitwig's Copy and Paste actions, and the focus actions, by that route.
`ran` says which of those steps ran. A step that didn't run doesn't stop the request: the link answers with what
Bitwig shows, and doesn't paste when the copy didn't run, so it never puts down whatever was on Bitwig's
clipboard before (nor do the checks below, which only follow a paste that ran).

The 3.7 live run (on 6.1.3) put nothing in the arranger by any route, while Save worked from the flush. To tell
which part fails, `arrange` from 3.8 takes `checks: true`: when its own paste ran but put nothing in the arranger
(no clip appeared on the track, and the clip selected there is as it was and wasn't on the track already, where
a new one would look just like it), and Bitwig has
`focus_or_toggle_clip_launcher`, it then runs two checks by the same route, each starting as `arrange` does (it
selects the track and a launcher slot, and gives the clip launcher the focus). They don't run otherwise, since
their paste could then put down what Bitwig's clipboard held before, or a clip in the arranger that would hide
what a later go put there. A check stops before its paste or Duplicate when Bitwig doesn't select its slot or the
route doesn't run the focus action.

- `launcherPaste`: a paste into the track's first empty launcher slot, which needs no arranger. When the mark
  took and is `exact`, a clip there in the launcher clip's marking colour (`sameAsMark`) means the copy took, so
  it's the arranger's part that fails, and one in another colour means the copy didn't take; nothing means the
  paste didn't land in the launcher either (if it went anywhere else, the link doesn't see it, and it's left).
  It says which `slot`, whether it got `selected`, the launcher's `focus` count, which steps `ran` (`focus`,
  `paste`), whether a clip was `made`, its `colour`, and whether the link `removed` it again (it deletes it, so
  the next go starts as this one did).
- `duplicate`: Bitwig's Duplicate action on the launcher clip, which needs no clipboard. A new launcher clip
  (`launcher`, how many more) means Bitwig's editing actions work in the launcher by this route, and the link
  deletes it again (`removed`); a new track (`tracks`) means Duplicate went to the track, so the launcher didn't
  have the focus, and that track is left in the test project. With neither, Duplicate changed nothing the link
  can see.

A check that stops part way (another project came to the front, say) puts why in `checks.error`, and `arrange`
still answers with what it found.

`filetrack` (3.8) tries a way round the clipboard altogether. Bitwig's API can't put a clip on the arranger, but
a track can insert a file after itself (`afterTrackInsertionPoint().insertFile`), as when a file is dropped
between tracks, and Bitwig may make a new track with the file's clip in the arranger, at the play start, as it
does for a drop. The answer says whether a track came, and whether the clip went to its launcher instead or the
clip selected in the arranger moved to it; only a look at Bitwig shows where it starts. The live tests try it
with their MIDI file (*Changes › MIDI file as a track*).

What the live runs found (6.1.3, 2026-10-15). Everything `arrange` does through the API works: the clip takes
each mark colour, exactly as asked, and the play start moves to `at`. Bitwig's Copy, Paste and Duplicate don't
act on what the API selected, though: they act on Bitwig's own selection, whatever was last clicked. In the two
runs that started with nothing selected (3.7 at 18:52, 3.8), by every route, Copy, Paste and Duplicate changed
nothing at all, in the arranger, the launcher or the track list (3.8 had just restarted Bitwig, so its clipboard
was empty too). The 3.7 run at 22:19 started with Bitwig's selection on a track, and there Paste ran (from the
flush, then on the link's MIDI) and put whole tracks in the track list, copies of “Send 185248” and “Devices
221955”, just above the clip's track. The link, which reads tracks by their place in its track bank, then looked
at a pasted track and took the clip for gone. `filetrack` made no track: Bitwig ignores a MIDI
file inserted after a track, while the same file into a launcher slot works.

So 3.9 stops copying and records instead. Bitwig's user guide describes recording launcher clips into the
arranger: with arranger record on, launching a clip while Bitwig is stopped starts it from the play start, and
what the clip plays goes into the arranger on its track. That needs no focus or selection, only API calls the
extension already makes work. The `record` job ([the extension's README](bitwig-java/README.md#whats-new-in-link-version-3))
sets Bitwig up first so only the clip records and nothing is heard (the master muted, every track disarmed, no
metronome, pre-roll, loop, punch or overdub, touch automation, launch quantize one bar), reading each setting
back and refusing when one doesn't take. Then it records, stops the track on the clip's last bar, turns record
off, plays the arranger back to hear what went in, and puts every setting back as it was. It runs in real time,
so it takes the clip's length and the replay's; a faster `tempo` shortens both. It finds tracks by their IDs,
so a track going in above can't send a step to the wrong one (as the pasted tracks did), and `clip`,
`filetrack` and `arrange`'s clean-up do the same from 3.9. It's still an experiment: the live tests try it
(*Changes › Record into the arranger*, below), and the app's sends won't use it until a run shows it works.
`probe` sees whether Bitwig carries out its panel commands for the link at all, which says whether a way that
needs the focus in a panel could ever work. In the app they're `BitwigLink.RecordAsync` and `ProbeAsync`.

### Sending into Bitwig

`Core/DawLink/BitwigSend.cs` builds on these commands:

- **An FL Studio channel** (*To Bitwig* in the MIDI window) gets a new track named after it. A sample goes
  in a Sampler, whose Transpose makes up for the channel's root key and pitch knob (FL Studio plays a sample
  at its own pitch on the root key; a new Sampler does on C3), so it sounds as in FL Studio. A plugin loads
  with the settings the project saved: as a Bitwig preset when Bitwig has a saved preset of that plugin to
  build one from, else as a `.vstpreset`, or for VST2 a one-program `.fxb`. If Bitwig doesn't take those, the
  plugin loads with its default sound by the ID the plugin scanner read, and the channel says so. A moved
  root key puts a Note Transpose before the plugin. The notes go in as a clip in the first empty launcher
  slot. FL Studio's own plugins can't load in Bitwig; their channels still get their notes.
- **FL Studio's knobs**: when FL Studio has the project open, the app reads each plugin's knobs there
  first (a channel only when FL Studio has it with the same plugin), selects the plugin in Bitwig, sets
  the knobs whose value differs, matched by name (ignoring case, spaces and punctuation, since the plugin's
  format can differ), and reads them back to see which took. That brings tweaks made since the last save
  across, and most of the sound when Bitwig doesn't take the saved settings: anything that isn't a knob,
  like a wavetable or a sample, stays at its default, and the channel says so.
- **Read back**: after each channel, the app reads the project back from Bitwig and says if the track, its
  instrument, or the Note Transpose before it isn't there as sent.
- **A Sound Lab kit** (*Send to Bitwig* on the Kit builder) goes on a new track with a Drum Machine, kick on
  C1, snare on D1, clap on D#1, closed hat on F#1, tom on A1, open hat on A#1 and cymbal on C#2 (where
  General MIDI drum parts go), and the other pads on the free notes from C1 up. Adding a Drum Machine needs
  its ID, which the app reads from a Bitwig project or preset that has one (projects in the library first).
  Until it finds one, the kit goes on the Drum Machine selected in Bitwig, leaving pads that have sounds
  alone.
