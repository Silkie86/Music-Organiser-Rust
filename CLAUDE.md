# Music Organiser (Rust): working rules

This repo is the Rust rebuild of the C# app. The plan, the inventory of today's app and the phase order are in
[docs/spec.md](docs/spec.md). Read it before starting a phase.

## Where things are

- **Spec:** `docs/spec.md` (section 3A says what is kept, folded and rethought; section 13 is the phase order).
- **The old C# app:** `Silkie86/Music-Organiser` is the reference. It is public: read it from a read-only clone in a
  scratch folder. Never push to it, never open PRs on it from here. Its C# readers and the format registry
  (`Core/Formats/Registry/*.json`) are the source of truth for file formats.
- **Layout (spec section 12):** one Cargo workspace, `crates/mo-*` (formats, library, space, work, audio, sounds, harmony,
  midi, playground, dawlink, app, ui) and `tools/` (formatlab, theorybench). UI is Slint; screen logic lives in
  `mo-app` so it can be tested without a window.

## Rules carried over (spec section 14)

1. Offline, native, no web UI. Windows first, Mac and Linux ready.
2. Never move or delete samples. Duplicates only hide in the app.
3. Never change an original project: Fix writes "(fixed)" beside it.
4. Never commit Solomon's projects, samples, MIDI packs, Bitwig template, Bitwig binaries or model files.
5. Only counts and averages from Solomon's MIDI go in code.
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
18. Claude merges a PR itself once its code review's points are fixed and CI is green (Solomon, 2026-10-29); don't
    push to the build Solomon is testing without asking.
19. A quick code review on each pull request before it is tested; after a batch merges (at the latest each phase), an
    ultracode code review of the whole app on `main`, then fixes.
20. Claude sets the right effort level itself for each task and review: higher for file formats, data safety and
    whole-app reviews, lower for small fixes. No need to ask first; say which level was used.
21. Highly multithreaded where it helps (rayon), at low priority, never on the UI thread.
22. MIDI 2.0 ready: notes and controllers keep MIDI 2.0 resolution (16-bit velocity, 32-bit controllers, per-note
    controllers, MIDI Clip Files, UMP ports); only a MIDI 1.0 file, port or DAW gets 7-bit values (spec section 4.4).

## How work is done

- No deadlines. A phase is finished when Solomon is happy with it.
- Each page starts with a short question round and mock-ups (keep, fold, rethink), then gets built.
- Small PRs from fresh branches off `main`. Solomon tests at home with the launcher; merges are batched so the app
  restarts once.
- The Rust app shares `%LOCALAPPDATA%\MusicOrganiser` (`library.db`, `sounds.db`, `midi.db`, settings, playground
  ideas) with the C# app. Until a Rust page is trusted and its twin tests pass, it opens the databases read-only.
- Anything that changes files on disk is tried on copies first: copy, check, then rename.
- Twin tests are a guide, not a cage: where Rust reads a file differently from C#, list the difference and judge it
  (better, or a bug).
- Tests that scan sounds use the fixtures folder, never a random temp name.
- Before pushing (what CI runs): `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`; and the same with `--manifest-path tools/galaxycheck/Cargo.toml` when the galaxy check
  or `mo-app` changes (the galaxy check is a workspace of its own).
- CI on a pull request only tests the crates it changes and clippy-checks the crates that use them
  (`tools/dev/ci_scope.py`); pushes to `main` check everything. Before an ultracode review, start a full run by hand
  (Actions, CI, Run workflow on `main`) and make sure it's green.
- Write for Solomon in plain words: Solomon is new to Rust.
