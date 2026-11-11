# Handover: carry on the whole Rust rebuild (2026-11-02)

> **Newer:** start with [CONTINUE.md](CONTINUE.md) (2026-11-06). This file is the 2026-11-02 handover; its "How to
> work" part still holds, its "Where things stand" part is out of date.

Until now the work was done by several Claude sessions inside one claude.ai project, each owning one part (screens,
harmony, Sounds, MIDI, Space, Backup, Presets, converter). Solomon is moving ALL of it to one Claude Code cloud session.
This folder is everything that session needs. Those other sessions have stopped, so nothing here is owned by anyone
else any more.

## Read in this order

1. `CLAUDE.md` (rules, checks before pushing) and `docs/spec.md` (3A keep/fold/rethink, 11 tests, 12 layout,
   13 phase order, 14 rules).
2. This file.
3. `memory/` = the project's shared notes, copied as they were on 2026-11-02 (Solomon's drive paths replaced by
   placeholders such as `<FL projects folder>`, because this repo is public). `[[name]]` in them means `memory/name.md`.
   Start with `rust-rebuild-rules.md` (Solomon's rules 1-28), `rust-rebuild-status.md`, `rust-app-design-decisions.md`,
   `app-screens-handover.md`, `app-screens-ui.md`, then `harmony-handoff.md`. The others are per-crate notes; read one
   when you touch that crate.
4. `docs/harmony/` = the harmony engine's design and experiment notes (they used to live outside the repo).

Paths like `/mnt/project-files/...` in the notes pointed to a shared folder you can't see. Everything you need from
it is copied into `docs/`; the rest was screenshots and bench output that can be made again.

## How to work (short form, full rules in `memory/rust-rebuild-rules.md`)

- Solomon is new to Rust: write PRs and replies in plain words. Keep going until he says stop; ask only for real
  decisions. No deadlines.
- Small PRs from fresh branches off `main`. Before pushing: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (CI uses Rust 1.99, so use
  `cargo +1.99`; `CARGO_INCREMENTAL=0` saves disk; Linux needs `libasound2-dev`). When `mo-app` or the galaxy
  changes, also run the same three with `--manifest-path tools/galaxycheck/Cargo.toml`.
- Every PR gets a code review before merging. Pick the effort yourself and say which you used: xhigh for anything that
  writes files or databases, high for readers and harmony, lower for small fixes.
- Rule 18: you merge (squash) once the review's points are fixed and CI is green. Don't push to a build Solomon is
  testing without asking.
- Rule 19: after a batch merges (at the latest each phase), start a full CI run on `main` by hand (Actions, CI, Run
  workflow), then an ultracode whole-app review, then fix what it finds. One is due now that phase 3 is nearly done
  and many backends landed.
- Force-push is refused. After a squash merge: `git fetch origin main <branch> && git checkout -B <branch>
  origin/main && git merge -s ours origin/<branch>`, then push. Merging needs the PR head's full SHA.
- On a `Cargo.lock` conflict, take main's, run `cargo metadata`, commit.
- Databases (library.db, sounds.db, midi.db in `%LOCALAPPDATA%\MusicOrganiser`, shared with the C# app) stay
  READ-ONLY until Solomon trusts a page. Rust-only side files (presets.db, curves.db, midi-index.db, backup.txt) are
  fine. Anything that writes files: copy, check, rename. Never move or delete samples; fixes write "(fixed)" copies.
- Screenshots: `mo-screens` renders pages headlessly; show Solomon every new or changed screenshot with the PR.
- Solomon tests at home with the launcher (`tools/dev/Run latest.cmd`), which builds `main` or a pushed branch in
  Debug. He sends notes with F12 in the app; they land in `%LOCALAPPDATA%\MusicOrganiser\notes` on his PC (ask him to
  paste them, unless your session can reach his PC).
- The old C# app (`Silkie86/Music-Organiser`, public) is the read-only reference: clone it into a scratch folder,
  never push to it. The container can install .NET for twin tests: `apt-get install -y dotnet-sdk-10.0`
  (`memory/cloud-container-tools.md`).

## Where things stand

Phases 0-2 (shell, file readers, Songs page) are done. Phase 3 (Sounds page) is nearly done. Phase 4/5 backends
(Space, Backup, Presets, Curves, converter) are on main; most of their pages are not built yet. Phase 6 harmony is
in progress. Main was at `deacf60` (#162) when this was written.

### Open work in git

- **PR #164** "Year strip on Sounds and Songs; Light by on the galaxy" (branch `claude/project-thread-uptbg1`,
  2c33903). Local fmt/clippy/tests passed. To do: a quick code review, the galaxy check (the three checks with
  `--manifest-path tools/galaxycheck/Cargo.toml`), green CI, merge, then reset that branch as above. Do this first.
- Branch `claude/affectionate-noether-qw3i3h` (2 commits, your own earlier work): Bitwig proptest round-trip tests
  plus review fixes, no PR yet. It is based on an older `main`, so rebase it onto `main` and open the PR.
- All other `claude/project-thread-*` branches hold only merged work; leave them.

### Each workstream and what's next

**Screens and UI** (`mo-ui` Slint pages, `studio.slint` shared style, `mo-app` screen logic). Notes:
`memory/app-screens-handover.md`, `memory/app-screens-ui.md`, `memory/gpu-for-graphics.md`.
Queue from Solomon, in order:
1. Finish #164.
2. Lag on the galaxy: Solomon still feels it ("check the old project, it did a report to find the bottleneck").
   The log shows a 332 ms freeze on Sounds "in no marked step" (drawing or layout). Find the C# app's performance
   report or frame-timing code, port it, and measure before changing anything. Also: Sounds read takes ~833 s for
   453,885 sounds and the Songs library read 15-87 s in his logs (the launcher builds Debug; mo-ui isn't optimised).
   Earlier freezes: 4480 ms on Songs, 459 ms on a key press.
3. Bug: while sounds load, the galaxy's empty state draws two subtitles over each other ("Listening to your sounds
   first..." and "Try fewer filters...").
4. Arrangement and waveforms drawn on the GPU (wgpu). Solomon wants ALL custom graphics on the GPU, with the CPU only
   as a fallback and for headless screenshots.
5. Drag sounds (tiles, list rows) into a DAW with a nice animation (the item lifts and dims, a card follows the
   cursor, a pulse when it lands); also drag MIDI clips (written as .mid) and audio clips (their sample) out of the
   Songs arrangement and tracks. That needs a Standard MIDI File writer in `mo-formats` (not built yet).
6. "Send to Bitwig" on the Sounds side card, like the old app; show the ⋯ actions on every Sounds card; the side card
   is cut off on the right at 4K or ultrawide.
7. Year strip on every other list page (MIDI, Presets, Plugins...).
8. Then the pages not built yet, each starting with a short question round and mock-ups: Space (with Duplicates from
   `mo_sounds::duplicates`), Plugins, Presets (with a Curves section on `mo_presets::curves`), MIDI, Settings (General,
   Folders, DAWs, Shortcuts, Developer; must show the "Made with Slint" credit), Ctrl+K. Agreed layout: the same
   list + side panel frame everywhere; actions in the side panel and right-click menu (each menu line shows its key),
   not row buttons; rail with names and icons. Plan doc: https://claude.ai/artifact/PdiLCg2s9fJDVA9GssfTxz; design
   canvas: https://claude.ai/artifact/QvnfEeSqAfNMV1SSHmEXzv; style: https://claude.ai/artifact/YVTaf9wbierXjBX8ocHyUj
   (Solomon's claude.ai artifacts; read them with the Artifact tool if you have it, otherwise ask him).

**Harmony engine** (`mo-harmony`, `tools/theorybench`). Full handover: `harmony-handoff.md` (read it all; it has
the next PR step by step). Solomon wants the highest effort here.
- Next PR: make the chord hand cases public (`chordread::cases`), have chordfit check every gate, then learn from
  the hand cases. The decode fingerprint must stay `300 pieces, all: 57bda66c6f357eee`.
- Then the training mix, speed step 2, the mo-app hookup (key and chords on the MIDI page and Songs), Chord check.
  Songs read keys with VERSION 4's numbers (`theorybench/src/keyfind_v4.rs` moved into mo-harmony as a second set),
  not version 5's (Solomon, 2026-11-11): measured on his songs (`theorybench songkeys`, "Songs" in
  `docs/harmony/key-finder-experiments.md`) version 5 pulls major songs to minor, and on public songs it is no better
  whole and much worse on stretches. None of his songs has a typed key and he changes keys and samples as he works,
  so the public sets stay the measure; the MIDI page keeps version 5.
- The key-finder refit on Solomon's own MIDI packs (rule 5 exception) can only run on his PC. It needs a Remote
  Control session that Solomon starts, plus the folders where his MIDI packs are. The brief to give it is in
  `harmony-handoff.md`, word for word. Done once (2026-11-03): version 5. A second rescale starts from version 4's
  weights (`tools/theorybench/README.md`, "Rescaling again").
- Bench data is gone with the old container; rebuild it with `tools/theorybench/README.md`.
- Tunes and chords together (`mo_harmony::together`, 2026-11-11): how well a tune sits over an accompaniment, in
  all 12 keys, for "find chords for this melody" and "find a melody for these chords" (Solomon's ask: built round the
  song he's working on, read from its newest save). Measured with `theorybench pairs` beside the C# matcher ported
  as it was; the numbers are fitted on pop and held to two rules for jazz, gospel and blues. What was tried:
  `docs/harmony/together-experiments.md`. No screen uses it yet.
- The song being worked on (`mo_midi::playing`, `mo_midi::fit`, 2026-11-11): a saved project read as it plays, each
  instrument's notes in their place in the song (not from `extract`, which moves every pattern to bar 1: FL Studio
  from the converter's song, a part a channel, channels switched off left out; Bitwig and Studio One from the
  arrangement the Songs preview draws), the song saved last picked from the library's songs, and a part from the
  library fitted to the song's picked parts and bars: a tune over its chords, or chords under its tune, with the
  move that makes it fit. The picked bars go round as picked, a bar of rest too. Known gaps: an FL sampler channel's
  notes aren't pitches the project vouches for (`Playing::sample`: left out unless picked); a library part's length
  is only known from its notes. (The FL arrangement layouts the Songs preview draws didn't mark a channel that's
  switched off: PR #476 marks it muted.)
- A first look for searching the whole library (`mo_harmony::together::first_look`, 2026-11-11): the close look
  needs each part's notes from disk, so a search sorts every part first by a little kept in memory. Measured
  (`theorybench pairs --quick`): what the catalogue holds today (which notes a part plays, with no timing) is no
  better than chance; how long each pitch class sounds in every two quarter notes (`slots_of`, 12 bytes a slot) is
  enough. A tune for chords: keep the best 2 in 100 (`Backing::outline`, about a microsecond a part). Chords for a
  tune: keep the best 10 in 100 (`Outline::guessed`, 3 to 6 microseconds).
- The MIDI index keeps each set of notes' slots for that first look (`slots` in `midi-index.db`,
  `mo_midi::store::slots`, 2026-11-11): filled when notes are saved, and after a read for notes saved before the
  table was there or by another `fit::SLOTS_VERSION` (`indexer::fill_slots`, from the notes the index keeps, no
  file opened; `SLOTS_VERSION` is `mo_harmony::SLOTS_VERSION` and how `mo-midi` asks, like `keys::VERSION`). Kept
  small: two bytes a slot for which pitch classes sound and a byte for each that does. A search loads them all once
  (`MidiStore::slots`, a `SlotBook` by note hash, about 200 bytes a part in memory). Whoever keeps a book has to
  load it again after a read that saved parts or made slots (`Summary::slots`): the MIDI page's "did anything
  change" check after a read doesn't count `slots` yet, and must once it keeps a book. The fill tells no progress
  (like the shapes one: seconds, once).
- The search (`mo_midi::fit::Target::search`, 2026-11-11): every part of the right kind, each set of notes once,
  sorted by its slots (the first look, on the pool). A loop a pack has in every key (one shape, read the same way:
  its kind, cut or not) is one loop from there on. Then the best 2 in 100 (tunes) or 10 in 100 (chords), never
  fewer than 300, have their notes read (on the caller's thread: the index's connection is one thread's) and get
  the close look on the pool, 256 at a time, the likeliest first, a batch judged while the next is read (two wait
  at the most). Of a loop's copies the one given is the one that needs moving least (it's read and looked at
  closely too); where the copy picked can't be read, the next is. A stop flag ends it for a newer search (rule 12). On
  100,000 made-up parts, 4 threads, the notes already in memory: 0.05 s for tunes, 0.46 s for chords; reading
  the kept parts' notes from a real index on disk isn't in those times. For whoever calls it:
  - leave the song's own parts out of the candidates before the search (they're in the library and fit their own
    song best; taken out afterwards the list comes up short, and a pack loop the song uses goes with them);
  - load the book again after a read that saved parts or made slots;
  - the `notes` call is the caller's: keep what it read when searches follow close on each other;
  - `Search::not_ready` and `Search::unread` count what was left out, and `stopped` says the list isn't whole;
  - `Found::sits` is the part found moved, never the song: `best().0 == -2` is "drop it in two semitones down".
  In a song that isn't in 4/4 a library part goes round by its own bars of 4/4 in both looks (a clip dropped into
  a song keeps its length). The bars a loop takes up are one rule for both looks (`mo_harmony::own_bars`), and
  parts are sorted by `Sits::judged`, the number the best move is picked by. Whether a part is a piece cut from a
  longer stretch is `StoredPhrase::is_cut` (the rule by name moved from `mo-app` to `mo_midi::index::is_cut`; the
  MIDI page's card still spells the fallback out and should call it).
- The MIDI page's "Fits my song" list (2026-11-11): the parts that go with a song you're working on.
  - `mo_midi::fit::for_song`: `Pick::usual` (what of a song is fitted to when nothing is picked by hand: its
    chords, arps and basslines for a tune, its melodies for chords, over up to 8 bars from where the most of them
    play together, and only the parts that play in those bars), `Own::of` (the song's own parts in the catalogue:
    whatever is in one of its saves, by the saves' paths as the song library groups them, since that knows the
    saves joined or split by hand; and a pack's loop the song uses in the pack's other keys, which is the same
    shape in the same folder of `.mid` files, never drums, as the catalogue's "in 12 keys". The same shape anywhere
    else is not the song's: a held note or a one-pitch bassline is in many places), `tried` (the parts the page's
    filters let through, without the song's own) and `Tried::search` (the search, its answer as places in the
    catalogue).
  - `mo_app::midi_fits`: the screen's logic and words. `song_choices` (the 8 songs saved last, each with every
    save it has; the first is the one being worked on), `SongChoice::own`, `fitted` (what the song gives to fit
    to, or why nothing can be, in plain words; nothing asked, chords that can't be read fall back to the melody),
    `tried` (the page's own filters stay in force, through `mo_app::midi::passing`), `look` (the search, the best
    200, with how long it took and how much of that was reading notes: `log_line` is for the log, so the time on a
    real index gets measured), and the words (`fit_text`: "Fits 88%, 2 semitones down"; `result_text`,
    `left_out_text`).
  - One call does a whole search, `midi_fits::search`: it reads through `midi_fits::Reads` (the song's save, the
    index's slots, a part's notes) and keeps in a `midi_fits::Kept` what's slow to make: the song until its save
    is written again (its time on disk is looked at for every search), the slots and the song's own parts for the
    very catalogue they were made for (another `Arc<Catalogue>` has them made again; the slots also when the
    page's `fit_slots_made` goes up: a reading that only made slots, which the page's "changed" check doesn't
    count). `page_line` and `empty_words` are what the page says about its answer.
  - On the page (`mo-ui/src/midi.rs`, `midi.slint`): a "Fits my song" chip whose menu picks one of the songs
    offered (or Off), two joined chips for what's looked for (the song decides until one is clicked), and the
    list, each line with its fit before its details. The search has a thread of its own (`FITS`); with it on, the
    list's own worker only counts what each filter's choice would show (`mo_app::midi::choice_counts`). Each new
    list tells the search before it to stop. Off lets go of what the searches kept. A search always answers: one
    that breaks says so in the list's place. "Parts like this one" and Ctrl+K's "show this part" turn it off:
    the list is ranked one way at a time; headings and Shuffle do nothing while it's on.
  - The songs offered come from the Songs page's thread (`songs.rs`'s `fit_songs`, given to
    `midi::songs_offered` with every list it makes), so they're grouped as the Songs page groups them and nothing
    is read for them. That's also how a list follows its song: the Songs page's folder watch reads a save a few
    seconds after the DAW writes it, the song is found again by its saves (`SongChoice::among`, never by its
    label), and its list is worked out again from the save written last. If the folders can't be watched, picking
    the song again in the menu reads it again.
  - A part in the list is the part as it fits (2026-11-11): `midi_fits::moved` moves its notes by exactly what its
    line says (`NoteEvent::moved` in `mo-formats`; a note with no key to go to is left out), and on the page every
    way a part's notes leave the index goes through one value, `PartOut` (`Page::part_out`: where to read them, how
    far they're moved, the name that says so, "Arp clip (3 semitones up)"): the card's roll and chords
    (`read_notes`), Play loop, Play in Bitwig and FL, Send to Bitwig and the drag. The card's key stays the part's
    own (its heading says so, and the chords' that they're moved); a line above the roll says how it's moved
    (`card_text`).
  - How the picked part is moved is its own state (`Page::picked_fit`, worked out by `fit_picked` when a part is
    picked and when a list comes on show): its line's in a "Fits my song" list; kept when a newer list of the same
    save of the same song no longer has it (a filter ticked or a word typed while it plays changes nothing that's
    heard) and through the index being read again; gone with the plain list or another song. `fit_follows` then
    brings what's seen and heard into line, each compared with the move it was made with (`card_move`,
    `loop_move`, `daw_move`): the roll and chords are read again, the loop or the part in a DAW started again,
    only when the move is another.
  - The octave a part goes at (2026-11-11): `mo_midi::fit::octave`. A search settles the key and says nothing of
    the octave (octaves make no difference to how a part sits), so `Found::middle` (the middle of where the part
    plays: half-way between its lowest and highest key, strays aside) and `Registers` (where each kind of part plays
    in the song, made in `midi_fits::fitted`) pick it: the octave nearest to half-way between the kind's usual
    place (`octave::usual`) and where it plays in the song; the usual place when the song has none of the kind.
    `Fit::octaves` holds it, and `Fit::moved` is the whole move (semitones for the key plus 12 an octave), which is
    what the line, the card and the name say in one go (`move_words`: "9 semitones down", "an octave up") and what
    `Page::fit_move`, `PickedFit` and `PartOut` carry. Measured by `theorybench octave`
    (`docs/harmony/together-experiments.md`): about 3 in 4 tunes land at the octave they were written at, 14
    points more than the C# app's rule, and no rule does better, so changing it by hand is next. The index already
    keeps an octave you chose for a set of notes (`your_octaves`, `MidiStore::set_your_octave`, taken in from the
    C# app); nothing reads it yet.
  Not built yet: changing the octave by hand, picking a song's parts and bars by hand, and putting the part found
  at the right bar of the project.

**Sounds backend** (`mo-sounds`, `mo-audio`): finished. Left: streaming long files while they play; sounds.db
writes once trusted. A decoder panic on a damaged file is caught with `mo_catch::catch`: one warning line in the log,
not a crash. Never `ORDER BY` on the sounds table for whole-table reads; sort in Rust. Notes:
`memory/sounds-backend-status.md`.

**MIDI backend** (`mo-midi`): index, analyser and key hookup are merged. Left: folder walk via
`mo_library::scan::find_files`, PhraseJoiner (the arrangement readers it needs now exist), chords once the chord
reader is hooked into mo-app; ask Solomon to run `tools/dev/MIDI twin test.cmd`. Notes: `memory/midi-page-backend.md`.

**FL to Bitwig converter** (`mo-convert`): automation (tempo, faders, pan, on/off, knob-move clips) merged. Left:
every re-convert makes "Name (2)" because the `written` stamp isn't stored while library.db is read-only (keep the
stamps in a Rust-only file in the data folder); the first convert of a song took 36 s (cold sample search?); FL 7/8
plugins have no vendor or path; knob-move lanes are named "setting N" rather than the plugin's parameter names;
not yet checked live in Bitwig: the master lanes and the tempo lane; DAWproject export (phase 5). Decided later:
FL native plugins in Bitwig, 3x Osc, Grid patches. Notes: `memory/fl-to-bitwig-converter-plan.md`,
`memory/bitwig-automation-lanes.md`, `memory/bitwig-automation-clips.md`.

**Old FL project fixes** (`mo-convert/src/fl_clones.rs`, #155): the fixed test copies are in his MO Test folder.
Waiting for Solomon to open them in FL 2026. Next idea offered: a Fix button on the Songs page. Notes:
`memory/fl-pattern-clones.md`.

**Bitwig curves** (`mo-presets::curves`, #151/#156): done. Left: prove that a curve point's x times the clip length
gives the beat (Solomon was asked to drop his own curve at bar 10); a doc nit (`Curve.points` and `length` docs say
"first found", but the code uses the newest source). Notes: `memory/bitwig-curve-files.md`.

**Space / clean-up** (`mo-space`): backend finished, with all 45 review points fixed. Bitwig multisample copies: the
"(fixed)" project is written by `mo_convert::relink`; copies go to the bin only after Solomon checks that the fixed
project plays. Notes: `memory/space-backend.md`, `memory/bitwig-multisample-relink.md`.

**Backup** (`mo-backup`, Backup page): finished; it has its own page in the rail.

**Presets** (`mo-presets`): backend finished (`crates/mo-presets/NOTES.md`); page not built. Notes:
`memory/presets-page.md`.

**Readers** (`mo-formats`): "Not yet" items in `crates/mo-formats/DIFFERENCES.md` need Solomon's saves. Still to
do: fuzzing (a reader may error but never panic or hang), the format registry test (spec 11.2), Bitwig note
expressions. FL event 172 size in FL 26 is open (see `memory/external-format-references.md`).

## Waiting on Solomon (don't block on these; carry on with something else)

- Where his MIDI packs are (for the key refit), and a Remote Control session started on his PC.
- Three chord-name choices (the C# names stay until he says yes): C13(#11) and C9(#11) instead of (b5) when a 9 or 13
  sounds; Eb/E instead of Eb/Fb; C7(#9,#5) and C7(b9,#11) instead of C7alt. If he says yes: a `chord.rs` PR with a
  version bump.
- Opening the FL clone-fix test copies in FL 2026, and checking that a relinked "(fixed)" Bitwig project plays.
- Dropping his own Bitwig curve at bar 10 (curves x-to-beat proof).
- Windows MIDI 2.0 developer API: Microsoft plans it from late November; nothing to do until then.
