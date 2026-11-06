# CONTINUE: carry on the Rust rebuild in a fresh Claude Code session (2026-11-06)

Solomon's claude.ai project (many parallel "threads", each a Claude session with shared project memory) is pausing
because his monthly usage ran out. He carries on in Claude Code with API credit. That session has NO project memory
and can't see the old threads, so everything it needs is in this repo. Main was at `e651756` (#348) when this was
written. This file replaces the "Where things stand" part of [README.md](README.md) (that one is from 2026-11-02).

## Read first

1. `CLAUDE.md` (rules 1-28, checks before pushing) and `docs/spec.md` (3A, 12, 13, 14).
2. This file.
3. `docs/handover/memory/` = every project memory file, copied 2026-11-06 (Solomon's paths replaced by placeholders
   such as `<FL projects folder>`, because the repo is public). `INDEX.md` is the memory index, one line per file;
   `[[name]]` means `memory/name.md`. Paths like `/mnt/project-files/...` and `claude.ai/artifact/...` links point to
   the old project's shared folder and docs, which you can't open; ask Solomon if you need one.

## Solomon's standing rules (all of them, in one place)

The numbered rules 1-28 are in `CLAUDE.md`; the background for 1-27 is in `memory/rust-rebuild-rules.md`. On top:

- **Plain words.** Solomon is new to Rust. Lead with the result, short sentences, no jargon.
- **Keep going until he says stop**; ask only for real decisions (give a recommendation). Don't wait for merges:
  start the next job while CI runs, on a separate branch (rule 27).
- **Merging (rule 18):** you merge (squash) a PR yourself once its code review's points are fixed and CI is green.
  The Mac job on a PR is advice only; on `main` a Mac failure is fixed straight after. Don't push to a build he is
  testing without asking. If he says "hold merges", hold until he says go.
- **Reviews (rules 19, 20, 26):** a code review on every PR before merging; pick the effort yourself and say which:
  xhigh for anything that writes files or databases, high for file readers and harmony, lower for small fixes. After
  a batch (at the latest each phase): full CI run on `main` by hand, then a whole-app review, then fixes. With API
  credit these cost real money now: keep reviews proportionate and tell Solomon before a very large one.
- **C# app is dead (2026-11-05).** Rust-only now; no need to stay compatible with C# formats or databases. Rule 6 is
  relaxed: each Rust database has a version, and a new version may rebuild its file by copy, check, rename. The C#
  repo stays a read-only reference for file formats.
- **Nothing may hold the UI thread (2026-11-04).** Anything that can lock or slow the screens goes off the window
  thread, at low priority (rule 21 rayon pools). Measure before guessing (`[Start]`, `[Freeze]` log lines).
- **Most efficient way (2026-11-04).** Never rescan unchanged files; cache by size and date (like `midi-index.db`).
- **App should feel instant (2026-11-05):** lazy loading, first screenful first, newest wins.
- **Small databases (rule 23):** each fact stored once; work out what's quick to work out.
- **All custom drawing on the GPU (wgpu)**, CPU only as fallback (`memory/gpu-for-graphics.md`).
- **UI:** filters the same on every page (`mo_app::filters`, `FilterPicks`); useful actions on the page, not hidden
  in menus (`memory/ui-show-dont-hide.md`); one shared style (rule 16); every menu line shows its key (rule 15).
- **Show every screenshot** you take with `mo-screens` (rule 25).
- **Chord names:** no brackets on altered chords (C13#11, C7#9#5); non-altered extras keep brackets (Cm(maj7),
  Cm(add9), C7(no3)); never slash chords.
- **Clean-up (rule 28):** F12 notes older than ~3 months go to the Recycle Bin; the app never empties the bin (warns);
  always remind him about "(old copy)" folders; crash-cut moves leave a marker; backup keeps older versions for ever
  (with an option to remove) and files deleted from his music stay on the backup with a warning.
- **Allowed writes (Solomon OK'd):** installing DAW scripts into his Bitwig/FL folders; the Settings/Folders,
  moves, Plugins folder fixes and export folder writers of the song library (`memory/library-db-writers.md`).
- **Never** commit his projects, samples, MIDI packs (`<MIDI packs folder>`), Bitwig template/binaries or models.

## How to work here (things that changed since 2026-11-02)

- Before pushing: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` (Rust 1.99), plus the same with `--manifest-path tools/galaxycheck/Cargo.toml` when
  `mo-app` or the galaxy changes. PR CI only tests the crates changed (`tools/dev/ci_scope.py`).
- After a squash merge: `git fetch origin main <branch> && git checkout -B <branch> origin/main && git merge -s ours
  origin/<branch>` (force-push is refused). On a `Cargo.lock` conflict take main's and run `cargo metadata`.
- Solomon tests with the launcher (`tools/dev/Run latest.cmd`, version 8): Debug by default, `-Release` and a
  "Music Organiser Rust (release)" shortcut since #318. `[profile.dist]` (LTO) exists for a shipped build (#320).
- **The app's data folder may now be on I:** (`I:\MusicOrganiser`; the pointer is
  `%LOCALAPPDATA%\MusicOrganiser-folder.txt`). F12 notes and logs are in `<data folder>\notes` and `\logs`. If this
  session runs on his PC, read them there.
- Windows-only code: clippy it with `--target x86_64-pc-windows-msvc` (or `-gnu` for the `midi-port` feature).
- Disk: `target/debug` grows past 20 GB; `CARGO_INCREMENTAL=0` and deleting old `mo_ui`/`mo_screens` deps helps.

## Current state (2026-11-06)

Phases 0-3 done (shell, readers, Songs, Sounds). Pages built: Songs, Sounds (galaxy, tiles, list, Rediscover,
history, Hide duplicates, Used in, drag-out), MIDI (piano roll on GPU, chord chart, "in N keys", drag-out, Play in
Bitwig/FL), Presets (Send/Audition in Bitwig), Plugins, Space (clean-up cards), Settings (folders, DAWs, data move,
exports), Ctrl+K finder, Backup backend. Harmony: key finder v5 (refit on his packs), chord reader v5 (calibrated,
#330) but no page reads chords except the MIDI card chart. New Rust databases: `songs.db` (replaces library.db),
`sounds-rust.db` format 2 (own sound library), `midi-index.db` layout 2. Last merged: #348.

## Open work: each line is next step / waiting on

1. **Whole-app review (xhigh, with consistency and UX)**: started 2026-11-06 03:27 in the old project, on main since
   #240 (108 PRs, ~60k lines); it may not have finished. If no report reached Solomon, run it again (full CI on main
   first), then fix the findings in small PRs. Areas: songs.db, sound library, MIDI/harmony, DAW link and audition,
   Space, Plugins, app core and data move, Sounds and drag-out, Presets and Ctrl+K; across features: shared
   databases, UI-thread work, rescans; UX: filters, keys, buttons, wording, look, clear flows.
2. **MIDI engine ideas** (`memory/midi-engine-ideas.md`): done #341 (same loop in other keys), #345 (chord chart).
   Next: **chord search** ("Am F C G" in any key), which needs chords kept in `midi-index.db`. Then idea 3 ("fits my
   song", measure v5 on his songs first), 4, 5. **Split FL channels** (old FL projects spread one phrase over several
   channels): waiting on Solomon to name one or two old projects to test on.
3. **Database archive (from 12 Nov)**: step 6 of `memory/new-rust-databases.md`: once the new Rust files have worked
   a week, archive the old library.db, sounds.db, midi.db to I: (never delete; look in `paths::moved_from()` too).
   Then tell Solomon when he may delete the old C: data folder (keep the launcher's `run-latest-rust` and the pointer).
4. **Waiting on Solomon's PC tests** (nothing to build until he reports):
   - Data folder move to I: (#299, #315): did he press Move, and do the next start's `[Data folder]` log lines show
     the switch?
   - Release build via launcher 8 (`-Release` shortcut): does it build and start? Send `[Start]`, `[Graphics]`,
     `[Songs]` lines (startup thread wants them for its next fix; 2nd start was 4.66 s to first window in Debug).
   - Bitwig extension: reinstall from Settings › DAWs (needs 3.20) before trying Audition / Play in Bitwig.
   - Play in FL needs loopMIDI running. Audition and Play were only tried against stand-ins, never real Bitwig/FL.
   - Presets Send to Bitwig failed for every preset he tried (WAVESTATION, Hive: "No Bitwig preset could be made")
     and a dragged fxb/fxp loads the plugin's default sound: check whether still true on latest main, then fix.
5. **Smaller open items:**
   - Chord reader: card "full-mix one-chord bar 0.80 (kept) vs fitted 0.58" is his to change; next step moves the
     weights further toward the fit (`memory/harmony-engine-status.md`).
   - Make-it-instant leftovers: one disk queue, shared saves list (`memory/app-feel-instant.md`).
   - Screens: saves Remove card and Space 2c wait on Solomon; fold the stale "C# settings are only read" text out of
     Settings (`memory/screens-queue.md`).
   - F12 notes routed to other threads (check on main whether done): Presets filter chips by plugin format (VST2/VST3/CLAP), clearer "Made by / From"
     chip names; MIDI loop with a General MIDI sound (needs a sound-source decision); Plugins "Move AmpliTube 4" moved
     the AmpliTube 5 folder (`memory/f12-notes-2026-11-05.md`).
   - "Check Windows MIDI 2.0 support" result is waiting for Solomon to read.
