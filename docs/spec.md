# Music Organiser in Rust: rebuild spec

Written 2026-10-29 from `main` at 4515ce8 (everything up to PR #50) plus open PRs #51 to #56 and the project notes.
It describes the whole app as it is today, what the Rust version must keep, which Rust tools to use and why, and the
order to move things over in.

**This spec is the hand-off to a new Claude project.** You're starting the Rust app in its own project and repo, and a
new project can't see this project's memory or shared files. So everything the new project needs is written here, and
the old repo `Silkie86/Music-Organiser` is the reference. It is public, so new threads read it online (a read-only
clone into a scratch folder) rather than having it added to the new project. Nothing is ever pushed to it from there.

Where this spec says **"Rule"**, it is something you decided or a lesson the app learned the hard way. The Rust app
must keep it.

> **Rust in plain words.** Rust is a language that compiles to a normal Windows program (one `.exe`), like C++, but
> it checks at build time that two threads can't trample the same data and that memory is always freed. There is no
> garbage collector, so there are no GC pauses and memory goes back the moment something is done with it. The price
> is a stricter compiler and slower builds. A *crate* is Rust's word for a package (like a NuGet package), and
> *Cargo* is the tool that builds, tests and fetches crates (like `dotnet` plus NuGet in one).

---

## 1. Why rebuild, and what "done" means

**What Rust buys this app**

- **Memory and pauses.** The app has hit 10 to 14 GB and 0.3 to 11 s freezes, much of it from .NET's garbage
  collector moving big buffers around (57 MB .flp files, 357k sounds, FL 6 notes with 2-billion-tick lengths). Rust
  frees memory straight away and never stops the world, so this whole class of problem goes.
- **One program to ship.** A single `.exe` (plus the ONNX Runtime DLL) instead of a .NET install, with fast startup.
- **Mac and Linux for free.** Most of the crates below already work on all three, including audio decoding, which
  today goes through Windows Media Foundation.
- **Threads you can trust.** Rust refuses to compile code where the UI and a background job share data without a lock,
  which is exactly the kind of bug behind several past freezes.

**What it costs**

- The C# app is about 100,000 lines (59k core and view models, 16k Avalonia views, 28k tests) built over three weeks
  of intensive work. A careful port is months of threads, not days.
- Much of the speed could also be won inside C#. Rust is worth it for the memory, pauses, single exe and long-term
  footing, not because C# can't work.

**Not a copy (you, 2026-10-29).** The C# app was where ideas got tried as they came. The Rust app keeps what you
like, folds the verbose parts into other parts of the app, and uses the faster language to make things feel instant.
The parsers and engines are a strong start, not a ceiling: they get improved, not just copied. There's no rush.

**The C# app is frozen**: it gets bug fixes and open PRs #51 to #56 only, and stays your daily app until the Rust one
is better. New features are built only in Rust.

**Done means**

1. Everything marked Keep or Fold in section 3A works in Rust, on Windows first, and builds and passes tests on macOS
   and Linux. Section 3 is the inventory of today's app to choose from, not a list to copy line by line.
2. The Rust app opens your existing library (`library.db`, `sounds.db`, `midi.db`, settings, playground ideas)
   without re-scanning, and the C# app can still open them afterwards (section 6).
3. Every C# test of a kept behaviour has a Rust twin; tests of dropped or reworked parts are retired on purpose (section 11).
4. The speed targets in section 8 are met or beaten on your PC with your full library.
5. You've used it daily for a while and said "switch".

---

## 2. What stays exactly as it is

Not everything gets rewritten. These parts are already separate programs talking over simple messages, so the Rust app
just talks to them the same way.

| Part | Language | Why it stays |
|---|---|---|
| Bitwig extension (`extensions/bitwig-java`, ext 3.17) | Java | Bitwig only loads Java extensions. TCP port 19791, 4-byte big-endian length + JSON. |
| Bitwig JS controller script (fallback) | JavaScript | Same reason. Same protocol and controller ID. |
| FL Studio MIDI script (`device_MusicOrganiser.py`) | Python | FL only runs Python. SysEx link `F0 7D 4D 4F …` over loopMIDI. |
| Plugin scanner `mo-scan` | C++ | Uses the VST3/CLAP/VST2 C++ headers, runs one plugin per process, prints JSON lines. Can be ported later (section 9, phase 8). |
| MO Probe test plugin | C++ | Same. |
| CLaMP 3 export (`tools/clamp3`, Python) | Python | Runs once on GitHub to make the ONNX model release. |
| Models: Basic Pitch ONNX, CLAP (download), CLaMP 3 (release `clamp3-c2-onnx-v1`) | ONNX | Rust runs the same files. |
| Data files: `ChordKey.tsv`, `ChordMoves.tsv`, format registry JSON | data | Embedded in the Rust exe the same way. |
| Fonts: IBM Plex Sans/Mono, Space Grotesk/Mono (OFL) | data | Same files. |

---

## 3. What the app does today (the inventory)

This is everything the C# app does today, so nothing good gets forgotten. Section 3A says what happens to each
part: kept, folded into something else, or rethought.

### 3.1 The window

- Left rail: **Projects, Sound Lab, MIDI, Playground, Plugins**, Settings at the bottom (Ctrl+1 to 5, Ctrl+,).
  Agreed but not built yet: a **Space** page (Ctrl+6), and folding the MIDI page into the Playground.
- **"In your DAWs"** rows on the sidebar: the project open in Bitwig or FL right now; click to reveal it.
- **Activity cards** bottom right, one per job: started, progress, finished or failed. A running card can be put away;
  "N running" brings them back. Every job reports through this.
- **Busy buttons**: a button dims under a spinner while its job runs.
- **Key sheet** (F1 or ?), **F12 note for Claude** (text + screenshot into `notes\`), F11 full screen.
- **Page size**: Settings "Size of everything" 70 to 150%, plus a per-page magnifier; Ctrl+= / Ctrl+- / Ctrl+0
  size only the page on show.
- **Back/forward** (Alt+Left/Right), page up/down (Ctrl+PgUp/PgDn).
- **Disk nearly full** sign under Settings (under 10 GB, or under the app's own size), checked at start and every 5 min.
- Opens maximised; start page is a setting.

### 3.2 Projects

- Finds projects in your folders; skips DAW autosave/backup folders and the DAW's own demo, tutorial and template
  folders, FL "(autosave)" and "(overwritten at …)" files, and `._` files.
- **One row per song**: numbered saves in one folder (`Song.flp`, `Song_2.flp` … `Song_77.flp`) are one song.
  "Doubts" pairs (close names) can be joined or kept apart (table `song_choices`). The Saves tab moves saves between
  songs; Reset undoes.
- **When made**: the earliest of the oldest save's date and FL's own created date (event 237). Grouped by time
  ("This week", "2019"…), with a year scrubber.
- **Runs** of saves split by a 3-month gap (setting `version_gap_months`), labelled "YYYY original / rework /
  re-save". Reworked = plugin families changed, track count changed, notes changed by over a fifth, or length moved
  over 8 bars, compared with the original run's last save.
- Search (focused on open, Ctrl+F; several words; a year), Down/Esc/Enter, filter chips, group by DAW, status, folder,
  month, year, Elektron box, rating. Filters start empty each time.
- Song fields: status (Idea → Released), star rating, favourite, key (typed wins over the key heard from MIDI), tags,
  notes. **Rule:** they belong to the song, are written to every save and inherited by new saves.
- Song panel (2/3 of the width, divider saved): Open (main button; for FL songs it's **Open in Bitwig**, which
  converts), Open in FL Studio, Older saves menu (runs, newest saves, See all), Show in folder, Copy path, Export MIDI /
  Presets / DAWproject, Fix, Check with the DAW.
- Tabs: **Overview** (counts line; arrangement preview: FL playlist view and "as it opens in Bitwig" view, Bitwig
  arranger, Studio One tracks, audio waveforms from 10 ms peaks, automation; Fix list under a divider), **Plugins**,
  **Samples**, **Saves**, **MIDI that fits**, **Details**, song **Space** (size, Compact, Thin older saves).
- **MIDI window**: channels with what plays them, mini piano roll, export per channel (FL patterns placed as in the
  playlist), To Bitwig / Send all to Bitwig, knob copy.
- **Presets window**: export all, third-party only, FL-native only, synths or effects, one plugin; unused FL synths
  tucked away.
- **Fix** (writes `Name (fixed).ext` next to the original, never changes the original, reads the copy back and deletes
  it if unreadable, then asks Keep or Delete after you open it):
  - relink samples found by file name (ticked) or similar names (suggested, with folder-aware ranking and a pick list);
  - shows the path the DAW looked for;
  - repair damaged FL saves from the newest older whole save (any save of the song, not only the newest; PR #55);
  - plugins are only listed ("swap by hand"), never written.
- **Missing samples rule**: found in this order: where the project says; same path on the project's drive; folders
  that moved with the project; beside the project or its samples/recordings/Media/Audio folders; anywhere in the
  project's folder tree; the DAW's content folders (FL Packs, Bitwig library and installed-packages, Studio One
  folders). **Rule:** samples are never moved or deleted.
- **Missing plugins** judged by the plugin's saved ID first (Studio One today), then by name; other installed versions
  shown ("You have Serum 2"). FL-bundled plugins and any Waves plugin (WaveShell) count as installed.
- Elektron boxes (Analog Rytm MKII, Digitakt II, Syntakt) spotted through their Overbridge plugins.
- Live DAW state: a project open in a DAW shows so; it's re-read when the DAW saves it.
- Folder watching keeps the library current while the app is open (changes reported once folders go quiet).

### 3.3 Sound Lab

- Sample folders scanned and every sound analysed offline: type, pitch (YIN), key with Camelot, chord timeline,
  tempo (ACID tag, name, or onset autocorrelation snapped to bars), loudness, features vector.
- **Type sorting that learns**: user label > name/folder rule > your own softmax classifier (p ≥ 0.45) > CLAP
  zero-shot. Your corrections count three times. The Teach tab lists disagreements.
- **Your own tags, learned** after five examples.
- **Views**: Tiles (default), List, History; Shuffled on open, or Most liked, Name, Type, Key, Newest.
  **Autoplay** on click/arrows (previews aren't counted as plays). Star ratings with keys 0 to 5 (PR #56).
- **Most liked** weights (plays, drags, sends, favourites, stars) as sliders in Settings.
- **Detect notes** (Basic Pitch ONNX) saves MIDI.
- **Describe** search and sharper types with CLAP (optional ~205 MB download, then Deep listen).
- **Sounds like this** (nearest neighbours).
- **Rediscover**: unused sounds close to your taste, kept varied; forgotten folders; per-year.
- **Kit builder**: random kits, locks, per-pad re-roll, favour unused, one pack, key for bass pads, vibe text; export
  a numbered folder, Elektron-ready 48 kHz 16-bit option; Send to Bitwig onto a Drum Machine (GM pads, kick on C1).
- **Galaxy**: every sound as a star on a map (cut-down UMAP over CLAP and/or own features; saved in `sound_maps`;
  only re-laid out when over a fifth is new), drawn on the GPU with a CPU fallback; hold-and-sweep plays, click picks,
  middle/right drag pans, wheel zooms, pack names, mini-map, read-ahead so sweeping sounds instant.
- **Duplicates**: exact hash, converted copies (spectral distance), trimmed loops (fingerprints); Same sound vs Very
  similar; keep suggestion; Hide extra copies (Same only), Show again, Undo batch. **Rule:** only hides in the app,
  never touches files. Can run after each folder scan.
- **In your projects** tab: samples your projects use.
- Drag a sound out into a DAW. Space plays.
- Playback: choose the sound output (or follow Windows' default), survive the output disappearing (DAW grabbing the
  interface) by reopening, about 20 ms output delay.

### 3.4 MIDI page (to be folded into the Playground)

- Index of MIDI folders plus MIDI inside every project (FL, Bitwig; Studio One not yet), newest save of each song
  only (setting), failed files listed with retry. Files are only read.
- Parts as one entry per set of notes ("in 20 files"), kinds (chords, basslines, melodies, drums…), instrument names
  from track/plugin/file names or GM program.
- **Harmony fit** against a song or a part, all 12 transpositions, refined over time against what sounds together;
  "Find parts for bars X to Y".
- **Description search** with CLaMP 3 (Names ⟷ Sound slider), More like this.
- Editable roll, GM playback through a SoundFont (GeneralUser GS or any .sf2), octave, tempo, mute, Send to Bitwig,
  drag out.

### 3.5 Playground (a small MIDI DAW)

- One timeline of clips on tracks; drag, slide, right edge repeats, left edge trims; new track; resizable panes.
- **Chord lane / chord track**: chords are heard from the chord and bass clips, not typed (double-click adds one);
  loops past its end; own mute and clear; Hear button plays it alone.
- **Suggest** parts that fit the section (never from the same song), **Swap in**, **Write**, **Reharm** (3 steps,
  tried for real, Keep = one undo step), **Harmonise**, **From a song**, **From Bitwig**, **what comes next** chips.
- Fit per go round (shift ±6 with a cost per semitone), pins when changed by hand, Fit again.
- Energy curve, Groove (one slider: feel → rhythm → whole playing), Inversions/Revoicer (keeps note count, minimal
  voice movement).
- Snapping like Bitwig (Grid, Relative, Objects; Shift = free), ruler play-from marker, loop, zoom/pan.
- Piano roll: note names (off/names/over the chord), Ctrl+click, box select, group drag, snap to the chords.
- Note colours by chord role (green strongest, blue strong, violet 9/11/13, grey weak, red clash), shaded by velocity.
- Follow Bitwig, Play in Bitwig, To Bitwig (Ctrl+B), drag to the DAW as `.mid`.
- Ideas saved in `playground\current.json` and `playground\ideas\`.
- Agreed, not built: Notes menu (Quantise, Twice, Half, Reverse, Legato, Humanise), velocity lane, Ideas list,
  "Fits the open song" chip, chord track sent only when no chord part already plays it.

### 3.6 Plugins

- Every plugin used in your projects, with chips: All, Missing, Installed, Instruments, Effects, VST2, VST3, CLAP,
  Native; Never used, Big and unused.
- **"Install these to open more of your old projects"** cards ranked by how many projects each one alone unlocks.
- Side panel: Used in N projects (click opens on Projects), Installed copies (from `mo-scan`: exact IDs, vendor,
  version, kind, Read parameters / Read all parameters, 4 at a time, crash-proof, only new files re-read).
- **Space**: plugin size (guessed folders, Not its / Add a folder), Shared, Last used; Clean up (sounds only when no
  project uses the plugin; presets when unused N years; never program files), Uninstall (registry uninstaller), Spare
  copies (unused formats, AAX without Pro Tools; admin once for Program Files), Recycle Bin (Ctrl+Shift+Delete).
- **Move plugin data** to another drive: copy with size check, rename the original to "(old copy)", put a junction in
  its place; admin relaunch for protected folders; repoint, link again, tidy up doubled copies, move back with
  freshen; warns on broken or replaced links.

### 3.7 Converting and exporting

- **FL → Bitwig** native `.bwproject` writer, built from prototype tracks in your Bitwig template: samplers with root
  key, plugins with saved state (VST2 `.fxb`, VST3), Note Transpose, insert effects on the channel's track, master
  effects on master, clips per pattern placement, muted clips/tracks, steps, volume/pan/plugin automation, lost plugin sounds taken from older saves, and a "not carried over" note.
- **DAWproject** export (for Bitwig and Studio One).
- **MIDI** (one file per channel, no tempo event) and **presets** (.fxp, .vstpreset, raw FL states, Bitwig stored
  presets).

### 3.8 DAW links

- Install/update scripts from Settings ("Set up both"), choosing Bitwig user and extensions folders and FL user data.
- Watch both DAWs (poll every 2 s, retry rules), Check with Bitwig / FL (tempo, tracks, devices).
- Send into Bitwig: new track, clip, Drum Machine pads, transpose, insert by ID, knob copy and read-back, record into
  the arranger (armed).
- **Live tests** for Bitwig (37 checks). **Rule:** they change nothing unless changes are allowed AND the open
  project's name starts with "MO Live Test", checked before every change.
- MO Probe live link and knob checks.

### 3.9 Settings tabs

General (theme, accent, font, size, start page, performance pace, sound output, your library files), Projects, Sounds,
Plugins (plugin data homes), MIDI, Playground, Shortcuts, DAW links, Developer (live tests, self-check, MO Probe,
logs). **Rule:** a page's numbers are options on its own tab.

### 3.10 Diagnostics (for testing at home)

- Session log `logs\session-*.log` with the build/commit Version line, pages, warnings, errors, `[Freeze]` lines
  (UI stall over 250 ms with the page and running jobs), `[Job]` lines, `[Facts]`, `[Sweep]`.
- `audio.log`, F12 notes, self-check reports, live-test reports, duplicate-match CSVs.
- **Rule:** crashes always log; a log that stops after its header means the app was killed from outside.

### 3.11 Tools

- `formatlab`: tree, diff, find, census, probe, live, fields.
- `theorybench`: chord/key/fit benchmarks, `--corpus`, `--save-chord-key`, `--save-chord-moves`.
- Screenshot runner and the `app-screenshots` and `playground-check` skills in `.claude/skills`.
- "Music Organiser (latest)" launcher: picks an open PR or main, keeps it updated, restarts on new code, waits while
  an F12 note is open.

---

## 3A. Keep, fold, rethink

A first proposal for you to change. Before each phase builds a page, a short question round (like the grill-me
sessions) settles that page's map, with mock-ups first.

### Keep (things you've said you like)

- **Songs grouped from numbered saves**, first-made dates, runs (original / rework / re-save), song fields shared by
  every save.
- **Fix** writing a "(fixed)" copy with Keep or Delete, and **Open in Bitwig** for FL songs.
- **The Playground**: its DAW feel, chord lane, note colours by chord role, suggestions, Reharm, Bitwig link.
- **The look**: shaded studio style, Graphite default, the 9 themes, IBM Plex, lots of feedback and animation.
- **The galaxy**, drawn on the graphics card, with sweep-to-play.
- **Shortcuts on everything**, shown in every menu, one key sheet.
- **Activity cards** for every job; F12 notes and logs for testing.
- **Safety rules** (section 14): never move or delete samples, never change an original.

### Fold (fewer places, same power)

| Today | Becomes |
|---|---|
| Rail of Projects, Sound Lab, MIDI, Playground, Plugins (+ planned Space) | Four places: **Songs**, **Sounds**, **Playground**, **Plugins**, plus Settings. **Ctrl+K** jumps to anything (songs, sounds, MIDI parts, plugins, ideas) from anywhere. |
| MIDI page | Folded into the Playground as its Browse side (as you already chose). |
| MIDI window and Presets window | Tabs on the song panel, not separate windows. |
| Fix, Check with the DAW, Repair, missing lists | One **Health** strip on a song: what's wrong, one Fix button, what was done. |
| Plugin space, song Space tab, app files card, duplicates, planned Space page | One **Space** view (a mode of Plugins, or its own page): biggest wins first across plugins, songs, spare copies, duplicate sounds, app files. |
| Sound Lab tabs (In your projects, Teach, Duplicates, Rediscover, Kit builder, Galaxy) | One sounds list with view switches (Tiles, List, Galaxy) and filter chips ("used in my songs", "unused"); Rediscover is a sort; Kit builder is a side panel; Teach becomes fixing a label right where you see it. |
| Settings: 9 tabs | Fewer: **General** (look, size, pace, sound output), **Folders** (projects, samples, MIDI, plugin data), **DAWs**, **Shortcuts**, **Developer**. Each page's numbers live in a small options popover on that page. |
| DAW link set-up by hand | Set up on first run, with a status dot in the sidebar; Settings only when something's wrong. |
| Format Lab, live tests, MO Probe, self-check in the app | One Developer area, plus the `formatlab` command line. |

### Rethink (because Rust is faster)

- **Nothing waits.** Search across everything as you type; lists show at once on start; scans and analysis fill in
  live instead of "Getting ready…" screens.
- **One background engine** for scans, analysis, MIDI indexing and models, with the no-lag rules built in from day
  one rather than added after freezes.
- **Match plugins by ID everywhere** (FL and Bitwig too, not only Studio One), so "missing" is always right.
- **Known gaps to fix while porting**: Studio One MIDI and export; FL envelopes; Bitwig clip names and the Sampler
  one-shot field; the chord ear naming Dm7 over a walking bass as Fmaj13/A; the key guess flipping after Richer
  chords; round-robin samples counted as duplicates.
- **Twin tests as a guide, not a cage**: where Rust reads a file differently from C#, the difference is listed and
  judged (better, or a bug), never forced to match.

### Parked

- **Serum 1 → Serum 2** (knob translator and swap in Fix and Open in Bitwig): left out for now; it was mainly a test
  (you, 2026-10-29). The C# code and `Serum2FromSerum1.json` stay in the old repo if it's ever wanted.

### For you to sort

Things that aren't clear yet, to settle in each page's question round: Elektron box detection, kit export for
Elektron, CLAP description search (205 MB download), CLaMP 3 MIDI search (430 MB), DAWproject export, the Bitwig JS
script fallback (the Java extension may be enough), and the WPF app (it goes when the C# app retires).

---

## 4. File formats the app reads and writes

These are the heart of the app and the riskiest part to port. Every rule below came from real files. The C# readers
and the format registry (`Core/Formats/Registry/*.json`) are the reference; the Rust port must find at least as much in
the same files, and every difference is listed and judged (section 11's "twin test").

### 4.1 FL Studio `.flp` (read, repair, relink)

- `FLhd` header then `FLdt` + length, then events: id < 64 byte, < 128 word, < 192 dword, ≥ 192 variable length.
- **Rule:** event 172 is written with 3 bytes when the FL version (event 199) is 25 or later.
- Some FL 4 to 7 saves have junk after `FLhd`; read on with a warning. A file with no `FLhd` is "empty or damaged",
  not an error.
- Notes (224): 24 bytes FL 9+, 20 bytes FL 4 to 8 (the version decides). Length 0 = step. Muted flag byte 19 bit 0x20.
- Playlist (233): item size by version (FL 6-8: 28; 9-20: 32; 21: 60; 25: 80; 26: 88), pattern/audio/automation
  numbering rules per version, track = top − slot (top 499 from FL 12.9.1, else 98 or 198).
- Channels: 21 type, 135 root key, 219 pan/volume/pitch, 218 envelopes, 104 insert routing, 64 index, 201 plugin.
- Mixer: insert name 204 before its 236; effects before their slot's 98; 127 inserts.
- Plugins: Fruity Wrapper 213 fields 50 kind, 51 VST2 id, 52 VST3 class, 53 state (VST3 records), 57; VST2 state has a
  21-byte FL header; selected VST2 program; KORG LegacyCell states; SliceX.
- Automation: 234 points (delta beats), 227 links, targets for channel, mixer and plugin params.
- First-made date: event 237, two Delphi doubles.
- Repair: keep the damaged save's events, finish from the newest older whole save's matching channel; fix the
  channel count word.
- Reader version (setting `reader_version:FL Studio`), currently 6. **Rule:** bump it whenever what a reader finds
  changes, so every song is re-read once.
- **Rule:** read each big file once per job into one reused buffer (C# `ReadOnce`); in Rust, memory-map it.

### 4.2 Bitwig `.bwproject` (decode, encode byte-for-byte, write new projects, relink)

- `BtWg` + 38 hex chars header (body and zip offsets), meta block (`application_version_name`, `used_files`,
  `structure`), then an object tree: u32 class, fields (u32 id, u8 type, value), u32 0. Types 0x01 to 0x1a as listed in
  `BitwigFormat.cs`. Strings with the top length bit are UTF-16BE.
- **Rule:** the decoder must end exactly at the zip offset (that's its correctness check). If it can't, fall back to a
  text scan and say so on the project.
- Encoder re-writes a decoded file byte for byte. **Rules:** objects are defined at first use, wherever that is;
  a single-object field must never hold a "class 1" reference (write a bare reference 0x0b instead).
- Ids and layout: devices 0x40/0x728/0x1a5, tracks 0x90, mixer, clips 0x47 → key lanes → notes, Sampler zones and MD5
  of the sample file, automation lanes and points, Note Transpose fields, plugin-state zip entries.
- Writer copies prototype tracks from your template (never committed to the repo).
- Reader version currently 2.

### 4.3 Studio One `.song` (read)

- A zip: `song.xml`, `Devices/audiomixer.xml`, `audiosynthfolder.xml`, `musictrackdevice.xml`, media pool.
- Plugins from `ghostData/classInfo` (name, classID, category, subCategory), not the shown insert name.
  VST2 IDs from Steinberg's GUID scheme.
- Notes live in `Performances/*.musicx` (UBJSON) and `*.pattern` files, not inline.
- **Rule:** never add a column to the `plugins` table (older builds insert 8 values by position); per-plugin extras go
  in their own table (`plugin_ids`).
- Not yet: Studio One MIDI in the index, fuller Studio One export (agreed).

### 4.4 Other formats

- **MIDI files**: read SMF 0/1; write one file per channel, **no tempo event** (keep the ReaderTests no-tempo test).
- **DAWproject**: zip with `project.xml` and `metadata.xml`, plugin states under `plugins/`.
- **Presets**: `.fxp`/`.fxb` (chunk `FPCh` or parameter `FxCk`), `.vstpreset`, Bitwig `.bwpreset` (Drum Machine UUID
  learned from your files).
- **VST3 `moduleinfo.json`** inside bundles.
- **Audio**: WAV (incl. ACID/smpl chunks), AIFF, MP3, FLAC, M4A/AAC, OGG.

---

## 5. Engines

### 5.1 Harmony engine (`Midi/Jazz`, used everywhere)

- **ChordReader** (one chord reader for the whole app): beat by beat, dynamic programming over spans, pushes count on
  the beat, approach notes ×0.25, bass only when low/alone/a 5th below the next; ChordSpeller names every chord and
  spells by key; slash chords keep their bass.
- ChordScales, Voicer, Reharmoniser, Harmoniser (Viterbi), NoteStrength, ChordFunctions, ChordKey (key from chords),
  ChordLane, chord-next from `ChordMoves.tsv` blending 1- and 2-chord context.
- **Rule:** tensions count as good notes (jazz/gospel); suggestions skip the section's own songs.
- Key from MIDI with a profile averaged from POP909 and your packs (24 numbers).
- Credits: ChoCo counts ship (CC BY 4.0, credit in the README); Jazz Harmony Treebank is only for measuring, never
  shipped. **Rule:** only counts or averages from your MIDI go in code; never your files.

### 5.2 Sound analysis

- Decoding, resampling, DSP (FFT, bands, onsets, YIN), the features vector, CLAP embeddings (ONNX), Basic Pitch
  transcription (ONNX, C# port of basic_pitch 0.4.0 without bends), softmax classifier, Rediscover, kits, Liking.
- Duplicates: exact audio hash, 24-band onset-aligned print, 26 ms waveform sketch, LSH keys, spectral distance
  (90th-percentile band dB after level removal), fingerprints for sounds ≥ 2 s (SoundFingerprinting in C#).
- Galaxy layout: PCA to 32 dims from 20k samples, 15 nearest neighbours (exact under 6000, NN-descent above), parallel
  settling, start spread ±10·√(n/10k), 35% radial evening-out.

### 5.3 MIDI index and search

- Indexer (low priority, incremental, versioned: `MidiIndexer.Version` 8; per-song stored version = Version×100 +
  reader version). Phrase analyser, phrase joiner (back-to-back clips), cut points, instruments, catalogue by note hash.
- HarmonyMatch / HarmonyTarget, refine top 500 in half-bar slots.
- CLaMP 3: MTF text + M3 patches, XLM-R unigram tokenizer, two ONNX encoders, 768-d vectors stored by note hash.

### 5.4 Playground engine

- PlaygroundSession, Timeline (clips, go rounds, offsets), SectionMatcher, ChordFollow, ChordProgressions, Energy,
  Groove, Inversions, DawFollow, BitwigClipChords.

### 5.5 Library

- Scanner (incremental by size + mtime, reader versions), SampleLocator, SongVersions, SongChoices, MatchFinder
  (fuzzy names, folder-aware), ProjectFixer, FolderWatch, plugin catalogue and scans, space (sizes, spare copies,
  compact, thin, folder moves, recycle bin, uninstallers, admin steps).

---

## 6. Data: keep the same files

The Rust app uses the **same folder and the same databases**, so you can go back and forth while it grows.

| File (in `%LOCALAPPDATA%\MusicOrganiser`) | What's in it |
|---|---|
| `library.db` | settings, roots, projects, plugins, samples, tags, song_choices, installed_plugins, sample_index, plugin_scans, scanned_plugins, plugin_ids, plugin_space, folder_space |
| `sounds.db` | settings, sounds, sound_tags, sound_events, sound_maps, sound_prints, sound_matches, sound_hidden |
| `midi.db` | sources, phrases, your_roles, clamp, your_octaves |
| `playground\current.json`, `playground\ideas\` | Playground ideas |
| `models\clap`, `models\clamp3`, SoundFont | downloaded models |
| `logs\`, `notes\`, `reports\` (live, checks, bitwig, duplicates), `probe\` | diagnostics |

**Rules for the shared files**

- SQLite in WAL mode, foreign keys on. Readers use their own read-only connections so the window never waits on a
  write. Reads inside a write use the write connection.
- Schema changes only add tables or indexes, never change or drop what an older build reads. Never add a column to
  `plugins`. New indexes are built in the background the first time ("BuildNewIndexes").
- Settings keys stay the same names (`start_page`, `sound_output`, `bitwig_template`, `shortcuts`,
  `appearance_page_sizes`, `galaxy_basis`, `playground_*`, `reader_version:<DAW>`, `space_moved_folders`, …). Section
  3.9's tabs map to them one for one; the full list is collected in phase 1 from `SettingsViewModel.cs` and the
  `*Key` constants.
- Revision counters (ProjectsRevision, PluginsRevision, SampleIndexRevision) decide which pages reload. A write a page
  must show bumps its revision.
- Until a Rust page is trusted, it opens the databases read-only.

---

## 7. Background work rules (no lag, ever)

**Rule (you, 2026-10-15):** "don't allow any scan to take up so many resources that the program becomes laggy."

- One shared pool of background slots for every job together. Settings › Performance: **Gentle** (default, half the
  threads), **Balanced** (all but two), **Fast** (all, only while the app isn't in front).
- Background threads run at below-normal priority, including `mo-scan` processes.
- ONNX models use the same thread budget with spinning off.
- **Rule:** the UI thread never takes a lock a background thread can hold, never waits on a database write, never
  reads a file (previews open on a worker, newest request wins).
- Heavy page loads run off the UI thread with a version counter so the newest request wins.
- Folder walks take size and modified time from the directory listing, never one call per file.
- Anything that can hold thousands of rows is virtualised.
- No per-item loop calls something that walks the whole library.
- Progress is reported at most every 250 ms.
- A UI watchdog logs any stall over 250 ms with the page and the running jobs.

---

## 8. Speed and size targets

Your library: about **357k sounds**, **16.7k projects (18,959 saves)**, and hundreds of thousands of MIDI parts.

| What | Today (C#) | Rust target |
|---|---|---|
| Window stalls | `[Freeze]` lines of 0.3 to 11 s seen, mostly fixed | none over 100 ms with every job running |
| Memory | 2 GB normal, spikes of 10 to 14 GB | under 1.5 GB with the full library, no spikes |
| Startup to Projects list | a few seconds | under 1 s to a usable list |
| Search as you type (16.7k projects, 357k sounds) | fast | under 30 ms per key |
| Re-read every FL save (reader version bump) | tens of minutes, 3.4 GB allocated for 330 FL 8 files | no more than today, flat memory |
| Galaxy layout, 300k sounds | ~45 s, 0.65 GB | same or better |
| Galaxy frame | ~10 ms zoomed in, ~45 ms whole sky | under 16 ms always (60 fps) |
| Sound from sweep to ear | ~52 ms | under 40 ms |
| Output delay | ~22 ms | ~10 to 20 ms |

These are measured with the same bench ideas the C# app has (SweepLatencyBench, perf generator for 300k sounds), run
in CI on a fake library and on your PC with the real one.

---

## 9. Rust tools, and why

### 9.1 The window (UI framework): **Slint**

| Option | Verdict |
|---|---|
| **Slint** | **Pick.** A markup language (`.slint` files) for screens, with Rust code behind them, very like Avalonia's `.axaml` + view models. Live preview while you edit a screen (the nearest thing to Hot Reload). Themes through global colour tokens, so the 9 themes and accents carry over. Virtualised lists. Windows, macOS and Linux. Free for desktop apps under its royalty-free licence (it asks for a small credit) or GPLv3. |
| egui | Runner-up. Easiest for hand-drawn views (piano roll, timeline, galaxy) and very fast, but it redraws everything every frame and looks like a tool, not a polished product. Harder to make your shaded, joined, pressed-in look. |
| iced | Good, but its Elm-style code is harder to follow for a beginner, and theming is more work. |
| GPUI (Zed's) | Very fast, but thin docs and young on Windows. |
| Tauri / Dioxus / Electron | Ruled out: they draw the app as a web page, which you said no to in October. |

How the hard parts work in Slint:

- **Custom drawn views** (galaxy, piano roll, timeline, chord lane, arrangement, waveforms, energy curve): drawn with
  **wgpu** (Rust's GPU library: Direct3D 12 on Windows, Metal on Mac, Vulkan on Linux) into a texture Slint shows,
  or with **tiny-skia** on the CPU for simple ones. The galaxy keeps a CPU fallback.
- **Dragging files out to a DAW**: Slint doesn't do this itself; the **`drag`** crate (Windows OLE, macOS, Linux)
  starts the drag from the window handle.
- **Menus with keys, tooltips, popups, multiple windows**: built in. The key system stays one app-wide list
  (section 10).
- **Fonts**: the same `.ttf` files embedded.

### 9.2 Crates (Rust packages)

| Job | Crate | Replaces in C# |
|---|---|---|
| SQLite | `rusqlite` (bundled SQLite with JSON1) | Microsoft.Data.Sqlite |
| Background slots and priority | `rayon` (own pool sized to the pace) + `thread-priority` | Parallelism |
| Messages between threads | `crossbeam-channel` | Tasks, UiWork |
| Network and timers for DAW links and downloads | `tokio` (only for the link and downloads) + `ureq` for simple downloads | sockets, HttpClient |
| Big files | `memmap2` | ReadOnce + ArrayPool |
| Binary readers | plain byte cursors (`byteorder`), maybe `binrw` for fixed records | BinaryReader |
| Zip, XML, JSON | `zip`, `quick-xml`, `serde` + `serde_json` | System.IO.Compression, XDocument, System.Text.Json |
| zlib, zstd, CBOR, UBJSON | `flate2`, `zstd`, `ciborium`, small own UBJSON reader | ZstdSharp, own code |
| MIDI files | `midly` | own MidiFileReader/Writer |
| MIDI ports (FL SysEx link) | `midir` | WinMM code |
| Audio decoding | `symphonia` (WAV, AIFF, MP3, FLAC, AAC/M4A, OGG) + own RIFF chunk reader for ACID/smpl | own WAV/AIFF + Media Foundation |
| Resampling | `rubato` | Dsp.Resample |
| Playback | `cpal` (WASAPI on Windows) + own small mixer and reconnect logic | OpenAL Soft (Silk.NET), NAudio |
| FFT | `realfft` / `rustfft` | own FFT |
| SoundFont synth | `rustysynth` (the Rust port of MeltySynth, same author) | MeltySynth |
| ONNX models | `ort` (ONNX Runtime) | Microsoft.ML.OnnxRuntime |
| Tokenizer for CLaMP 3 | `tokenizers` (Hugging Face) | own UnigramTokenizer |
| Audio fingerprints for trimmed loops | `rusty-chromaprint` (needs re-tuning against your matches CSV) | SoundFingerprinting |
| Nearest neighbours | own NN-descent port; `instant-distance` (HNSW) as an option | own code |
| Folder watching | `notify` + `notify-debouncer-full` | FileSystemWatcher |
| Folder walks | `jwalk` (parallel) | FindFileStamps |
| Fuzzy names | `strsim` | MatchFinder's own scoring |
| Windows bits | `windows` (official), `junction`, `trash`, `winreg`, `opener` | P/Invoke, Shell |
| Logging | `tracing` + `tracing-appender` | AppLog, SessionLog |
| Errors | `anyhow` (app), `thiserror` (libraries) | exceptions |
| Tests | built-in `cargo test`, `insta` (snapshot files), `proptest` (round trips), `criterion` (benchmarks) | xUnit |
| Installer | `cargo-packager` (MSI/NSIS on Windows, .dmg on Mac, AppImage/.deb on Linux) | none yet |

### 9.3 Tools on your PC

- **Rust** via `rustup` (it uses the Visual Studio C++ build tools you already have).
- **Editor**: Visual Studio 2026 doesn't really do Rust. Use **VS Code** with the **rust-analyzer** and **Slint**
  extensions (free), or **RustRover** (free for non-commercial use). Both run, debug and show live screen previews.
- `cargo watch` (or `bacon`) rebuilds on change; the launcher gets a Rust mode.

---

## 10. Look and feel

- **9 themes**: Graphite (default: neutral greys, mint accent), Studio, Retro (Space fonts), Midnight, Paper (light),
  Hardware, Terminal, Synthwave, High contrast. Accents: theme's own, Violet, Blue, Mint, Amber, Coral, Pink.
  Light accents get dark text. Less motion switch.
- Fonts: IBM Plex Sans, Plex Mono for numbers and dates; Retro uses Space Grotesk / Space Mono.
- **Your style, one shared styles file**: shaded top-lit buttons, joined button groups, pressed-in fields, small
  corners, two-tone grooved separators, flat shaded clips, little text with tooltips. Bitwig is an example, never a
  copy. In Slint this becomes one `studio.slint` library of components (Group, Panel, Lcd, Groove, Caps, PlayIcon…)
  that every page uses, plus a test that fails if a page sets a raw colour or its own button look.
- DAW colours: FL #5EA8FF, Bitwig #F2994A, Studio One #4ACB8B. Sound tiles coloured by type.
- Lots of feedback: hover and press transitions, spinners, cards sliding in, page cross-fades.
- One `PlayIcon` for every play/stop button (never typed ▶ or ■).
- **Shortcuts**: one app-wide list with ids (`app.*`, `playground.*`…), defaults, only changed keys saved under
  `shortcuts`, Settings › Shortcuts to change them, the key sheet. Ctrl+1..5 pages, Ctrl+, Settings, Ctrl+F search,
  Ctrl+Up/Down step the list, Ctrl+Left/Right tabs, Space plays, F1/? keys, F5 rescan, F11, F12 note, Ctrl+B Bitwig,
  Ctrl+Shift+E Explorer, Ctrl+Shift+C copy path. A page's own key beats an app-wide one; in text boxes only Ctrl and
  F-keys count. **Rule:** every menu line shows its key, and every action can take one.
- **Rule:** smaller lists and panels are built from the main control (reuse, not copies).

---

## 11. Tests to port

Today: about **930 test methods** (about 1,150 test cases counting each row), in 50 core test files and 20 UI test
files, plus native tests and the stand-in DAWs.

### 11.1 How tests move

| C# test files | Rust home |
|---|---|
| ReaderTests, FlRepairTests, BitwigWriterTests, BitwigProjectTests, BitwigPresetTests, DawProjectTests, ArrangementTests, FormatLabTests, ProjectKeyTests | `mo-formats` |
| LibraryTests, FixTests, FolderWatchTests, SpaceTests, PluginScannerTests, ShellTests | `mo-library` |
| SoundTests, DuplicateTests, GmSoundsTests, MidiRenderTests, SoundOutputSettingTests | `mo-sounds`, `mo-audio` |
| ChordReaderTests, JazzHarmonyTests, HarmonyTheoryTests, HarmoniserTests, ChordTrackTests | `mo-harmony` |
| MidiBrowserTests, MidiToolsTests, InstrumentTests, Clamp3Tests, Playground*Tests | `mo-midi`, `mo-playground` |
| BitwigLinkTests, BitwigSendTests, BitwigRecordTests, FlLinkTests, DawWatchTests, DawScriptTests, MoProbeLiveTests, LiveTests*, SelfCheckTests | `mo-dawlink` |
| ViewModelTests, ResponsivenessTests, LiveTestsViewModelTests | `mo-app` (screen logic without a window) |
| Avalonia tests (shortcuts, page sizes, playground page, projects page, sounds page, grid, timeline, piano roll, galaxy, style check, file drag, audio, diagnostics) | `mo-ui` with Slint's testing backend (no window needed) |

### 11.2 Special tests that must exist from day one

- **Twin test (most important).** The C# `formatlab` dumps what each reader sees for a file as JSON; the Rust reader's
  JSON is compared with it, and each difference is marked as an improvement or a bug. Run on the test files in CI, and on your whole library at home (reading only, no files
  leave your PC, only a pass/fail summary). Same for MIDI index phrases, chord readings and sound analysis numbers
  (with small tolerances).
- **Byte-for-byte Bitwig round trip** (decode → encode = same bytes) on every test file, plus random round trips
  with `proptest`.
- **Format registry test**: every id the code uses is in the registry JSON with its name, and back again.
- **Older build can still use the library** (the `plugins` column rule).
- **No-tempo MIDI writer test.**
- **Stand-in DAWs**: `fake-bitwig.js`, `serve.js`, `fake_fl.py`, `serve.py` keep working against the Rust link code.
  **Rule:** tests never touch a real DAW, its folders or a real MIDI port.
- **Responsiveness tests**: the UI thread is never blocked by a job (port ResponsivenessTests).
- **Style check**: no raw colours or own button looks on a page; no typed ▶ or ■.
- **Test data rules**: tests that scan sounds use the fixtures folder, never a random temp name (it can spell
  "snare"). Never commit your project files, samples, MIDI packs, Bitwig template or Bitwig binaries.

### 11.3 CI

- GitHub Actions on Windows, macOS and Linux (the repo is public, so minutes are free): `cargo fmt --check`,
  `cargo clippy`, `cargo test`, the twin tests against the C# build, benchmarks on main.
- Run the Bitwig Java build and the C++ scanner build as today.
- Pushes batched; one run per PR push; replaced runs cancelled.

---

## 12. How the code is laid out

One Cargo *workspace* (a folder holding several crates that build together) in the **new repo** (for example
`Silkie86/music-organiser-rs`; the name is yours to pick). The old C# repo stays as it is. The new repo's CI checks out
the old repo too, to build the C# `formatlab` for the twin tests.

```
music-organiser-rs/
  crates/
    mo-formats     FL, Bitwig, Studio One, MIDI, DAWproject, presets, registry   (no UI, no database)
    mo-library     databases, scanner, sample locator, song versions, fixer, watch, space
    mo-work        background slots, pace, newest-wins jobs, progress, activity model
    mo-audio       decode, resample, playback, output choice, previews
    mo-sounds      analysis, ONNX models, classifier, duplicates, galaxy layout, kits, rediscover
    mo-harmony     chord reader and the jazz engine
    mo-midi        index, phrases, catalogue, CLaMP 3, harmony match
    mo-playground  session, timeline, groove, energy, section matcher
    mo-dawlink     Bitwig TCP, FL SysEx, scripts, watch, send, live tests, probe
    mo-app         every screen's logic and state (like today's ViewModels), no UI framework in it
    mo-ui          the Slint screens and custom-drawn views, and main()
  tools/
    formatlab, theorybench
```

The same split as today (Core → ViewModels → Avalonia), so each crate can be tested without a window.

---

## 13. The move, phase by phase

Each phase ends with something you can open and try. The C# app stays your daily app until phase 7.

| Phase | What gets built | You can try |
|---|---|---|
| **0. Footing** | Agree the new app map from section 3A, with mock-ups. New project and repo (add the new repo to the project; the old one is read online; copy this spec in as `docs/spec.md`; write the rules in section 14 into the new project's memory and a `CLAUDE.md`), Rust + VS Code on your PC, the workspace, CI on three systems, the launcher's Rust mode, the window shell: rail, themes, fonts, shared styles, activity cards, key sheet, F12 notes, session log, watchdog. | An empty Rust app in your theme that logs like the C# one. |
| **1. File readers** | `mo-formats`: FL, Bitwig (decode/encode), Studio One, MIDI; the twin test; `formatlab` in Rust. | The twin test over your whole library, reading only. |
| **2. Projects** | `mo-work`, `mo-library` (read-only first, then writing), scanner, sample locator, song versions, Projects page with song panel, Overview arrangement, Saves, Plugins/Samples tabs, search and filters, folder watch. | Finding and opening old projects every day. |
| **3. Sound Lab** | `mo-audio`, `mo-sounds`: playback with output choice and reconnect, tiles/list/history, analysis, classifier, tags, Rediscover, kits, Detect notes, CLAP, duplicates, then the galaxy on the GPU. | Browsing and sweeping all 357k sounds. |
| **4. Plugins and space** | Plugins page, `mo-scan` hookup, sizes, clean up, spare copies, uninstall, folder moves with junctions and admin steps, the agreed Space page. | Plugin lists and freeing space (tried on a test folder first). |
| **5. DAW links and converting** | `mo-dawlink`: Bitwig and FL links, watch, checks, sends, live tests; Fix; FL → Bitwig writer; DAWproject, MIDI and preset export; FL repair. | Fix and Open in Bitwig on real songs, live tests. |
| **6. MIDI and Playground** | `mo-harmony`, `mo-midi`, `mo-playground`; the Playground with the MIDI page folded in (as agreed), CLaMP 3 search, GM sound, and the agreed Notes menu, velocity lane, Ideas list. | Writing ideas and sending them to Bitwig. |
| **7. Switch** | Settings complete, every remaining agreed feature (Ctrl+K finder, first-run steps, "Fits the open song"), installer, a final whole-app review, then the default launcher entry becomes Rust. Retiring the C# code is your call. | The Rust app as your daily app. |
| **8. Optional** | Port `mo-scan` and MO Probe to Rust (`clap-sys`, `vst3`), FL live tests, Studio One MIDI and export. | |

**Rules for every phase**

- No deadlines. A phase is finished when you're happy with it, not when a date passes.
- **Code review after the pull requests (your rule, 2026-10-29).** Once a batch of pull requests is merged (at the
  latest at the end of each phase), run an **ultracode** whole-app code review on `main`, then fix what it finds before
  the next phase starts.
- **A quick review on every pull request too.** Before you test a PR at home, it gets a normal-effort code review and
  its fixes, so bugs are caught before they cost you a test round. The ultracode review above then catches what only
  shows when the PRs meet.
- Each page starts with a short question round and mock-ups (keep, fold, rethink), then gets built.
- Each phase lands as small PRs from fresh branches off `main`; you test at home with the launcher; nothing merges
  without your yes, batched so the app restarts once.
- A Rust page writes to the shared databases only after its twin tests pass and you've tried it.
- Anything that changes files on disk (Fix, folder moves, clean-ups) is tried on copies first and keeps the C# safety
  rules (copy, check, then rename; never delete samples; "(fixed)" copies; Keep or Delete).

---

## 14. Rules carried over (one list)

1. Offline, native, no web UI. Windows first, Mac and Linux ready.
2. Never move or delete samples. Duplicates only hide in the app.
3. Never change an original project: Fix writes "(fixed)" beside it.
4. Never commit your projects, samples, MIDI packs, Bitwig template, Bitwig binaries or model files.
5. Only counts and averages from your MIDI go in code.
6. Never add a column to `plugins`; schema changes only add.
7. Bump a reader's version when what it finds changes.
8. Bitwig: define objects at first use; no class-1 reference in a single-object field; decode must end at the zip.
9. FL: 3-byte event 172 from FL 25; note size by FL version; read damaged saves on with a warning.
10. Live tests only touch a project called "MO Live Test…" with changes allowed.
11. DAW scripts stay plain ASCII; tests never reach a real DAW, folder or MIDI port.
12. No lag: shared slots, low priority, no UI waits, newest wins, virtualised lists, read big files once.
13. Missing samples: search the project and DAW folders first.
14. Song fields belong to the song.
15. One app-wide shortcut list; every menu line shows its key.
16. One shared style; reuse the main control for smaller lists.
17. Crashes always log; F12 notes keep their draft through restarts.
18. Merging needs your yes every time; don't push to the build you're testing without asking.
19. A quick code review on each pull request before you test it; after a batch merges (at the latest each phase), an
    ultracode code review of the whole app on `main`, then fixes.
20. Claude sets the right effort level itself for each task and review (your rule, 2026-10-29): higher for file
    formats, data safety and whole-app reviews, lower for small fixes. No need to ask you first; say which level was used.

---

## 15. Open questions

- **New project set-up:** the new project needs its new repo (the old one is read online), your PC folders approved again (Bitwig, FL, Studio One
  project folders, `L:\New Midi`, the app data folder) and the test files from this project's shared folder
  (`bitwig-tests/`, Playground demo MIDI) uploaded again, since shared files don't carry across projects.
- **Decided 2026-10-29:** C# frozen (bug fixes and open PRs #51 to #56 only), and the Rust app is an improved
  rebuild, not a copy. Section 3A is the first proposal for what's kept, folded and rethought.
- `rusty-chromaprint` for trimmed-loop duplicates needs tuning against your matches CSV; if it can't match today's
  results, the SoundFingerprinting parts we use get ported by hand.
- Slint's wgpu hookup for the galaxy is checked in phase 0 with a 300k-star test before anything else is built on it.
  If it falls short, the galaxy view uses egui inside a Slint window, or its own window.
