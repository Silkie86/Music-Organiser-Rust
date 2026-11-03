# Music Organiser (Rust): report to continue from

Written 2026-11-03, when the cloud session that took over the whole rebuild stopped at its weekly usage limit.
Read this with `CLAUDE.md` (the rules) and `docs/handover/README.md` (each part of the app and its notes).

## 1. Where the project stands

- **Phases 0-2** (shell, file readers, Songs page) are done.
- **Phase 3** (the Sounds page with the galaxy) is nearly done.
- **Backends on main, pages mostly not built:** Space (`mo-space`), Backup (`mo-backup`, which does have its page),
  Presets and Curves (`mo-presets`), MIDI index (`mo-midi`), the FL to Bitwig converter (`mo-convert`).
- **Harmony engine** (`mo-harmony`, phase 6) is in progress.
- **The old C# app** is frozen and still Solomon's daily app. The Rust format registry
  (`tools/formatlab/registry/*.json`) is now the source of truth ("rust is king now", 2026-11-02).

## 2. Merged on 2026-11-02

| PR | What it did | Review effort |
|---|---|---|
| #164 | Year strip on Sounds and Songs; "Light by" on the galaxy (graphics card and CPU fallback) | high |
| #166 | Test that every id the FL and Bitwig readers use is in the registry, and back; the Rust registry leads | medium |
| #167 | Random round-trip tests for Bitwig's format (proptest): byte for byte, edits, damaged files | high |
| #168 | Standard MIDI File writer (`mo_formats::midi::write`): MIDI 2.0 detail until the file, no tempo by default, FL steps lengthened | xhigh, twice |
| #169 | Fuzzing the readers (`fuzz/`, cargo-fuzz): about 7 million broken files, no crash or hang | medium |
| #170 | Launcher: smaller debug builds, a 25 GB build-folder cap, the incremental cache capped at 5 GB, `-BuildDir` for another drive, `.pdb` copied so crash logs name functions (launcher version 6) | xhigh, twice |

## 3. Open work: finish these first

None of these are merged. The `claude/affectionate-noether-wip-*` branches hold unfinished, **untested** fixes: take
them as a starting point and never merge them as they are.

### #171: galaxy timing (measure the lag)

- **PR branch:** `claude/affectionate-noether-galaxy-timing`
- **Unfinished fixes:** `claude/affectionate-noether-wip-galaxy-timing`
- **What it adds:** `[Sweep]` lines (a port of the C# `SweepTiming`: each step from the mouse reaching a star to
  hearing it) and `[Frames]` lines (slow frames and what they were doing). It also marks the galaxy's steps for the
  freeze watchdog. Nothing in how the app behaves changes.
- **Review points to finish (high effort):**
  1. Reads superseded after the 90 ms sweep gap are dropped, so slow or cold file opens never reach `[Sweep]`: log
     them as "dropped while being read".
  2. The star under the previous mouse point is reached again on every move, resetting its wait: skip it, as C#'s
     HitPath starts at i = 1.
  3. Pending entries are keyed by path, so an old read wipes a newer request for the same file: key them by ask id.
  4. Steps that run while no frame is drawn pile onto the next frame: report them as "between frames".
  5. Smooth `[Frames]` lines every 10 s while a job bar animates: only write them when galaxy steps ran.
  6. `[Sweep]` windows are clock time: start at the first sound of a stretch, and write what's left on shutdown.
  7. Late answers are blamed on the step seen at the first 25 ms look: blame the step seen most.
  8. Pruning past 64 entries drops the star waiting its turn: keep ones reached in the last 2 s.
  9. Pass the page into `late()`.
- **Then:** Solomon runs it, sweeps the galaxy for 30 s and pastes the `[Sweep]`, `[Frames]`, `[Freeze]` and `[Slow]`
  lines. Fix the lag from those numbers, not from guesses.

### #173: star look (stacked on #171; merge #171 first)

- **PR branch:** `claude/affectionate-noether-star-look`
- **What it adds:** each star gets a bright core, a white-hot middle and a soft glow, with additive blending. The top
  1.5% get four-point spikes. A slight twinkle is off while a button is held or the view glides.
- **Light by, from Solomon's F12 note** ("at this zoom level you should be able to notice the ones highlighted"):
  unlit stars are dimmer and smaller, and the best-lit ~2,500 stay distinct points at least 1.6 px wide when zoomed
  out. The CPU fallback matches.
- **Its review didn't finish.** Check:
  - dense clusters washing out to white (visible in the screenshots, left middle);
  - the twinkle causing redraws when the window is idle or on another page;
  - whether sorting stars by light breaks picking or sweep-to-play indexes;
  - the CPU fallback drifting from the shader.
- **Not done:** a warm or cool tint per sound kind (it needs a change to the packed star format).

### #172: the app cleans up after itself (rule 28)

- **PR branch:** `claude/affectionate-noether-cleanup`
- **Unfinished fixes:** `claude/affectionate-noether-wip-cleanup`
- **What it adds:**
  - a start-up sweep, `mo_app::tidy`, low priority, about 20 s after start;
  - caps on crash logs and tool reports;
  - removal of half-written files the app left after a crash;
  - the twin test's leftover library copy;
  - rule 28 in `CLAUDE.md`.
- **The xhigh review said NOT safe yet. Finish these:**
  1. **HIGH:** remove `clear_own_folder`. It empties `%TEMP%\MusicOrganiser-Rust` by age, but nothing makes that
     folder; with TEMP unset that's the user's folder, and a test showed it deleting a code folder's `.git`. Future
     temp files get exact names (e.g. `mo-clip-<pid>-<n>.mid`) and are cleared at the top level only.
  2. Always write the crash file for a fatal main-thread crash (caught panics used up the 10 slots first). Ideally
     write one file per panic location.
  3. The session-log prune must match the whole name, so a user's "- Copy.log" is kept.
  4. Files dated in the future count as oldest, and the prune never removes the log it has just made.
  5. `twin-test.ps1` only removes `library-copy.db` when it's a plain file.
  6. Reword rule 28: "removes only plain files whose whole name the app gives them, in the folders it writes them
     to; never by folder alone, never through a link, never what the user made". Point spec section 14 at the rules
     file.
- **After the fixes,** get them re-checked at xhigh before merging, because this deletes files.

### Harmony: chord hand cases public, and chordfit checks every gate

- **Branch:** `claude/affectionate-noether-harmony-gates` (one commit). **Unfinished:**
  `claude/affectionate-noether-wip-harmony`. No PR yet.
- Follow the next-PR steps in `docs/handover/harmony-handoff.md`. The decode fingerprint must stay
  `300 pieces, all: 57bda66c6f357eee`. Effort: the highest (Solomon's wish).

## 4. Decisions waiting on Solomon

Don't block on these; carry on with other work.

From the clean-up PR (#172); nothing here is deleted until Solomon says:
1. F12 notes and their pictures: keep them all, or remove ones older than about 3 months?
2. Items the app puts in the Recycle Bin: should the app empty them itself after about 30 days? That would end the
   undo for them.
3. "(old copy)" folders: should the Space page remind him, with their size, after about 2 weeks?
4. A folder move cut off by a crash leaves a half copy that blocks the next try: should the move leave a marker so the
   app can clear it?
5. The backup drive's "Older versions": keep them for ever, or only the last few runs or months?
6. A file deleted from his music stays on the backup: keep it, or clear it after some time?

Older ones, from the handover README:
- Where his MIDI packs are, and a Remote Control session on his PC, for the key-finder refit. Cloud sessions can't
  reach Windows folders, so this needs Claude Code running on his PC.
- Three chord-name choices: C13(#11) and C9(#11) instead of (b5); Eb/E instead of Eb/Fb; C7(#9,#5) and C7(b9,#11)
  instead of C7alt.
- Opening the FL clone-fix test copies in FL 2026, and checking a relinked "(fixed)" Bitwig project plays.
- Dropping his own Bitwig curve at bar 10 (to prove the curve's x times the clip length gives the beat).

## 5. What comes next, in order

1. Finish and merge #171, then #173, then #172, then the harmony PR. Each needs its review points fixed and green CI.
2. **Ultracode whole-app review on `main`** (rule 19): it's overdue. Start a full CI run by hand first (Actions, CI,
   Run workflow on `main`).
3. **The screens queue** (`docs/handover/README.md`):
   - the galaxy's empty state drawing two subtitles over each other while sounds load;
   - the Sounds read time: 135 s for 453,885 sounds in his last log, down from 833 s but still slow;
   - the arrangement and waveforms drawn on the GPU;
   - dragging sounds and MIDI clips into a DAW, with the lift, follow and land animation (the MIDI writer from #168
     is ready for it);
   - "Send to Bitwig" on the Sounds card, ⋯ actions on every card, and the side card cut off at 4K;
   - the year strip on the other list pages;
   - then the pages not built yet (Space, Plugins, Presets, MIDI, Settings, Ctrl+K), each starting with a question
     round and mock-ups.
4. **Smaller items:**
   - converter stamps in a Rust-only file, so a re-convert stops making "Name (2)";
   - a longer FL fuzz run;
   - the 8 Bitwig curve fields missing from the registry;
   - extending the registry test to `mo-convert`'s Bitwig writer;
   - the twin tests' release builds still landing on C:.

## 6. How to carry on

- **On Solomon's PC with Claude Code** (it can use API credits from a Claude Console account):
  1. Install it with `irm https://claude.ai/install.ps1 | iex`.
  2. Clone the repo into its own folder.
  3. Run `claude`, sign in with the Console account, and say: "Read docs/handover/CONTINUE-REPORT.md and carry on."
  - Working locally, it can read the F12 notes (`%LOCALAPPDATA%\MusicOrganiser\notes`) and logs directly.
- **Testing:** the launcher (`Music Organiser Rust (latest)` on the Desktop). Pick `main`, or a PR to test. To build
  on another drive, run this once: `"%LOCALAPPDATA%\MusicOrganiser\run-latest-rust\Run latest.cmd" -BuildDir D:\Builds`.
- **Before pushing** (what CI runs): `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. Use `cargo +1.99` to match CI.
  Add the galaxy check (`--manifest-path tools/galaxycheck/Cargo.toml`) when `mo-app` changes.
- **PRs:** small, from fresh branches off `main`. Every PR gets a code review: xhigh for anything that writes or
  deletes files, high for readers, the galaxy and harmony, lower for small fixes. Merge (squash) once the points are
  fixed and CI is green. Force-push is refused: after a squash merge, rebuild the branch from `main` with
  `git merge -s ours`.

## 7. Lessons from this session

- **Never share one `CARGO_TARGET_DIR` between git worktrees.** Cargo reused another worktree's build, and tests ran
  against the wrong code.
- **Cloud disk is about 40 GB:** delete `target/` after each job, and run only a few builds at once.
- **Independent reviews pay off.** They found a MIDI file bug that lost every note on a track (names of 128-255
  bytes), a launcher state where an unplugged drive meant the app never opened, and a sweep that could empty a
  user's folder. Review the fixes again for anything that deletes files.
- **Measure before changing:** the galaxy lag fix waits for #171's numbers from Solomon's PC.
