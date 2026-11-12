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
    the octave (octaves make no difference to how a part sits), so the search works it out for each part it finds
    (`Found::octaves`, from `Registers::octaves_for`): the octave that brings the middle of the part (half-way
    between its lowest and highest key, strays aside) nearest to where its kind belongs. That's the kind's usual
    place (`octave::usual`); for a bassline or chords, half-way from there to where the song's own play in the bars
    fitted to (`Registers::by_bars`, kept in the `Target` that `Target::of_song` makes). Never an octave that takes
    the part off the keyboard. A loop the library has octaves apart is given as the copy that needs the least
    moving in all (`Found::moved`, `nearer_copies`). `Fit::octaves` holds it on the page's side, and `Fit::moved` is
    the whole move (semitones for the key plus 12 an octave), which is what the line, the card and the name say in
    one go (`move_words`: "9 semitones down", "an octave up") and what `Page::fit_move`, `PickedFit` and `PartOut`
    carry. Measured by `theorybench octave` (`docs/harmony/together-experiments.md`): about 3 in 4 tunes land at
    the octave they were written at (the C# app's rule: 52 to 68 in 100), and no rule does better, so it can be
    changed by hand.
  - The octave by hand (2026-11-11): a fitted part's card has Octave down (Alt+Down), Octave up (Alt+Up) and,
    once the octave is one you chose, The app's octave; the line's right-click menu has them too. A step is from
    where the part is now (`Fit::octave_step`: 3 octaves at the most, never off the keyboard, by the part's
    `Found::edges`), and what you chose goes in place of the app's pick for those notes wherever they're fitted
    (`Fit::yours`, `Fit::octaves_now`), octaves chosen in the C# app too. The page has a choice at once
    (`midi_fits::YourOctaves` over the catalogue's, `with_yours` after each search and each change;
    `Page::change_octave`), and the index is told off the window's thread (`remember_octave`: the newest for each
    set of notes, written in turn, `mo_app::background::Latest`; tried 3 times when the index is busy, and waited
    for a moment as the app closes). `MidiStore::keep_your_octaves` writes it: only into an index that's there,
    never making or converting one. The search itself doesn't know of your octaves. A part in a plain list is as
    it is in the library: no octave buttons; drums never have them, and an octave kept for the same notes doesn't
    move a drum part. A held key steps once (7 octaves to be at; each step starts what's playing again), and at
    the last octave the key is still the page's.
  - Heard with the song (2026-11-11): in a "Fits my song" list, Play loop plays the part with what it was fitted
    to: the song's picked parts over the picked bars, read from its save, at the song's tempo, through the
    SoundFont. `midi_fits::SongBars` is that stretch of the song (`SongBars::of`: each part once, in the song's
    order, never drums or a part resting there, each with a General MIDI sound guessed from its track and
    plugin names, in the order the page's line names them); an answer carries it (`Answer::Fits::with`) and the
    picked part keeps the one it was found with (`PickedFit::with`), so a tune found over the chords still plays
    over them after the list turns to chords. How long they play: `SongBars::round`, until both loops start
    together again or once through the longer past 32 bars (`mo_harmony::round_of`, the matcher's rule), held to
    the two minutes of sound there can be by whole turns or whole bars, so a slow song never jumps the beat.
    The sound is the part's loop as it's dropped (from its first bar line, at the song's tempo) and the song's
    loop under it, each going round from its own bar lines (`mo_audio::gm::layered`: a turn a frame longer than
    its sound goes on with the loop's start, a loop cut where the whole goes round fades, and nothing is ever
    louder than full). The song's parts are made in one pass by one synth, a channel for each sound
    (`mo_audio::gm::render_together`), and go in at `SONG_GAIN` of their own loudness, or turned down to that
    much of the part's when they're louder (`song_gain`, by `mo_audio::gm::level`): six parts of a song don't
    drown the one being tried. The song's loop is kept (`SONG_LOOP`, one loop) so stepping down the list makes
    only each part, and let go with the chip or the list (`forget_song_loop`). If it can't be made, the part
    plays alone and the log says why. A "Play with the song" chip (on by default, the setting
    `midi_fits_with_song`) turns it off; the button says which it is, and the card's sound line says what's
    heard (`midi_fits::with_song_line`, or the part's own tempo: `show_sound` picks between the two with nothing
    read again). Never for drums or a plain list. Play in FL, Send and the drag are the part alone, as before.
  - The same in Bitwig (2026-11-11): with the chip on, Play in Bitwig starts the part on the selected track
    with the song's fitted bars going round in Bitwig's arranger (`Bitwig::play_with_song` with a `SongLoop`;
    the page's `bitwig_song`, the bars from `midi_fits::bitwig_loop`). Extension 3.22: a `clip` request may
    carry `loop: {start, length}` in beats. The extension stops Bitwig, remembers its loop and play start, sets
    them to the bars, and launches the part from its start; `deleteclip` (the app's Stop) stops Bitwig and puts
    back each setting still as the link set it (what Solomon moved meanwhile stays his), and says whether
    Bitwig showed them back (`TakenOut::loop_back`; the card and log say when it didn't). The bars go round
    for the newest part only, so a part the app let go of (a stop that failed twice) doesn't keep the loop from
    going back after the next one; a stop that fails part way leaves the loop for the next try. Only when
    Bitwig has the project the song was read from open (the pinned project's name is the save's; the name is
    all Bitwig tells, so two saves with one name can't be told apart), the song was read by its arranger (not
    the clip launcher), the extension is 3.22 or newer and its greeting says Bitwig let it reach the loop
    (`songLoop`); otherwise the part plays alone and the card says why (`midi_fits::bitwig_words`). The
    extension must be installed again from Settings > DAWs. Not yet tried in a real Bitwig: whether a launcher
    clip plays on through the loop's jump back, what "none"/"from_start" do when Bitwig is stopped, and how
    soon after a launch Bitwig says it's playing (`extensions/bitwig-java/README.md` lists them). The
    extension can't be built on the cloud machine (its Maven host is blocked there): CI's "Bitwig extension"
    job compiles and tests it.
  - What's judged is what's dropped (2026-11-11): one rule for a part's clip, `mo_midi::analyser::clip_of`
    (whole bars from its first bar line, as many as the index counts, or to the end of the bar its last heard
    note starts in; a note held a hair over the end stops there). The search hears a found part go round over it,
    both looks, both ways round (`fit::slots_of`, `Target::fit`, `chart::backing`), and it's the loop that's
    played (`mo_app::midi::loop_notes`) and the MIDI file that's dragged and sent (`part_clip`, which now ends on
    a bar line). A last note a hair over a bar line no longer makes the part a bar longer to the search than it
    is in the DAW. The index makes its slots again (`fit::SLOTS_ASKING`).
  - Into the song at the right bar, in Bitwig (2026-11-11, Solomon chose both ways). Bitwig's controller API
    can put a clip in the clip launcher but not on the arranger's timeline, so:
    **Put in the song** (a button on a found part's card, `midi.putInSong`, no key by default as it writes into
    the project) has Bitwig record the part in: the part's MIDI file is written to the export folder as Send
    writes it (and removed again once Bitwig has it), goes in a launcher slot of the SELECTED track, the
    extension's `record` job (3.9, tried live on Bitwig 6.1.5 in October) plays it once into the arranger from
    the first of the bars it was found for with everything else silenced and disarmed, and the launcher clip
    comes out again
    (`mo_dawlink::bitwig::Bitwig::put_in_song`, `bitwig/put.rs`; the page's `put_in_song` and `put_part`). It
    fills the fitted bars (a shorter part goes round) or its own length when longer (`midi_fits::put_place`),
    replaces what was in those bars on that track (Bitwig's undo takes it out; how many presses is untried),
    and takes as long as the bars play: the card says which bars and about how long, moves by the clock, and
    ends with what the link saw (`midi_fits::put_words`: all of it, cut short, or not seen). Only in the song's
    own project, on an instrument track, with Bitwig stopped (what the app plays in Bitwig is stopped first),
    and where the song's bars are whole bars of Bitwig's time signature now. One at a time: while one is being
    put in, the same button reads "Stop the recording" (`PutButton`), which calls it off before Bitwig records
    or has the link stop the job (`recordabort`, `Bitwig::stop_putting`, sent by `put_part`'s clock thread);
    what played so far stays in the arranger. Closing the app doesn't stop a recording under way: the job in
    Bitwig finishes and puts its settings back by itself.
    **Send to Bitwig** from such a list also moves Bitwig's play start to the first of those bars, so the clip
    it makes is dragged down to the mark (`Bitwig::send_part_for_song`, `Mark`, `midi_fits::mark_words`). That
    needs the extension 3.23 (a new `playstart` command; older ones get a line saying to install it again).
    Both go by where the part was fitted (`Page::bitwig_place`), whether "Play with the song" is on or not, and
    only for a Bitwig song read by its arranger. Not yet tried in a real song: see
    `extensions/bitwig-java/README.md` (many tracks, whether the take replaces what's there, the clip's name,
    tempo changes inside the bars, how many presses of undo take it out).
  **Parts and bars picked by hand** (the logic; the page's controls come next): `midi_fits::fitted` takes a
  `YourPick` (`mo-app/src/midi_fits/yours.rs`): the parts by name (`PartName`: its name and which of that name it
  is, as a save written again moves a part's place), the bars by number, either left to the app
  (`mo_midi::fit::for_song::Pick::by_hand` and `usual_in`). Parts by hand decide what's looked for (chords for
  them when all are melodies, else parts over them), whatever the switch says; a sampler can be picked by hand
  only; bars are held to the song and to 32. Every search's answer carries a `Picker` (each part that can be
  fitted to and whether it's on, the bars, the song's length), also when nothing can be fitted
  (`Answer::CantFit`), and the page changes its copy of it (`toggle`, `set_bars`, `apps_parts`, `apps_pick`)
  and asks for `Picker::pick`. `YourPicks` remembers the pick for the last 32 songs under the song library's
  key (`SongChoice::key`), as one setting (`midi_fits_picks`). Until the page has its controls it always asks
  with nothing picked.
  Not built yet: those controls, and putting a found part at its bar in FL Studio (there it's the drag).

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
