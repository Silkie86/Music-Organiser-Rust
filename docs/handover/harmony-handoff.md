# Harmony engine: hand-over (2026-11-02)

From the project thread "Build the harmony engine" (repo `Silkie86/Music-Organiser-Rust`). Phase 6 goal (Solomon):
work out chord extensions (jazz and gospel), the root note and inversions. Standing asks: highest effort and ultracode
for harmony work; don't wait on merges; keep going until Solomon says stop; leave real decisions to him.

Docs: the chord reader's design and its "as built" notes per PR are in `harmony/design/chord-reader-design.md`
(project files); the bench and every measured run are in the repo's `tools/theorybench/README.md`.

## Done (on main)

- Part 1 (keys, chord names, spelling; `tools/theorybench`): #45.
- Part 2, the key finder: #60 to #85; its weights #96 (KEY_FINDER_VERSION 4; `keyfit --final` was run once for those
  choices, never again for them).
- Part 3, the chord reader: bench #107/#108, cores #109, hearing #110/#111, candidates #112, evidence #114 (hand-set
  weights `HAND`), counted tables #120, pair table #124, decode #129, speed #132, namer #134, reader API #139
  (CHORD_READER_VERSION 1), facts #143, explain #145, words #152, `chords --engine rust` on the bench #158.
- #162 `theorybench chordfit` (fits the reader's 39 numbers on labelled music) with mo-harmony's `ChordFitter`, and
  the first fit's report (README, "The chord reader's first fit"). Code-reviewed at high effort, review fixes in;
  squash-merged as deacf60, and the thread's branch restarted from main.
  The first fit is NOT shipped: it gains on roots, chord kinds and inversions on every set, but breaks 27 hand-case
  gates and loses extensions (the loss only sees a label's root and core). The README now says its numbers come
  from the run before the review's fixes (a rerun starts at loss 2.406 a bar, not 2.418).

## Next PR: the chord fit keeps every gate

1. **Make the chord hand cases public**, as `keyfind::cases` already is: a new `chordread::cases` module. Today they
   live only in tests: `score_tests::hand_cases` (one stretch read on its own, `Want` First/TopTwo/Kept, judged by
   `reads`/`kept`), `decode_tests::whole_cases` (read whole, `Check` Reads/One/Starts/NonePushed/Top/Kept, judged by
   `holds`; plus every stretch case read whole as "w" + name), `name_tests::named_cases` and `named_targets` (read
   whole and named, `Want` Writes/Cuts/Flags/Among/Never/Intervals/Roots/Together/HeardFrom/NeverOver).
   - No unwrap or expect outside tests (the workspace denies them). To keep the case data's text unchanged: a
     `Writer` that keeps slips in a `RefCell<Vec<String>>` and has `notes`, `line` and `bars`; inside each case
     list bind `let notes = |n: &str, s: i64, l: i64| w.notes(n, s, l);` (and `line`, `bars`), build the list,
     then `w.done()?`. `dense(4)` becomes `dense_on(&w, 4)`.
   - Move `Music` (pub fields `parts`, `grid`, `span`; builders `pub(crate)`); `hear_tests` keeps an
     `impl Music { hear_into, hear, map }` block. Test files keep thin wrappers (`fn hand_cases() { cases::...()
     .unwrap() }`) so their other tests don't change. `reader_tests::hand_cases`, `explain_tests` and `words_tests`
     use them too.
   - API: `Step { Stretch, Whole, Named }`; `Verdict { step, name, gate, wrong, read }`; `Book::read(&Model) ->
     Result<Book, String>` (stretch verdicts, whole verdicts, named gates then named targets); `Book::broken()`;
     `fit_cases() -> Result<Vec<FitCase>, String>`, `FitCase { name, music (looped), labels: Vec<FitLabel> }` with a
     label for each `Check::Reads(units, names)` (the whole cases' and each stretch case's First want), in ticks
     `hearing.start(hearing.reported().start + unit)`, readings found by name (12 roots by `Core::ALL`, matching
     `Label::to_string()`).
   - The three gate tests become `Book` checks with `SHIPPED`. A move must change nothing: the decode fingerprint
     must stay `300 pieces, all: 57bda66c6f357eee` (`cargo +1.99 test -q -p mo-harmony --release
     show_the_fingerprints -- --ignored --nocapture`), so keep the order (whole cases, then stretch cases read whole).
2. **chordfit checks the gates**: after a fit, `Book::read` with the fitted numbers; print each broken gate by step,
   and the targets met; say so in the `--out` file. A later `--write` (into `chordread/weights.rs`) refuses weights
   that break a gate (design: "chordfit refuses to write weights that break a fit gate").
3. **chordfit learns from the hand cases**: each `FitCase` read with `ChordFitter` like a piece, as its own term in
   the aim: loss = the data's loss per bar + μ × (the cases' mean of −ln P(labels)), with its slope and Fisher added
   the same way (`Totals::reading`); `--hand μ` (try 1, which weighs about as much as all the data; 0 leaves it out).
   Then the same fit as the first, and check: every gate, the names and the extensions (measure as in the README's
   first-fit section). If a soft term can't hold every gate: on music read whole, "the labelled way beats the best
   way that breaks the gate by a margin" is a straight-line rule in the numbers; add such rules as half-spaces to
   `ruled.rs`'s projection (Dykstra), found as cutting planes.
   - A quick trial I had ready but didn't run: write the hand cases as bench items (source `hand`, licence `CC0`,
     `"whole":"loop"`, `"render":[[0,1,{}]]` so they're heard as given, chords as reading names such as `C|Cmaj7`),
     always in training, `--weigh hand=1000`, with two small local-only hacks in chordfit (labels by name; folds).
     It answers "can a soft term hold the gates" before the move is done.
4. Then: the training mix (design C12); λ, σ and the thresholds; speed step 2 (before mo-app); mo-app; Chord check.
   After the batch merges, an ultracode review of the whole app on `main` (rule 19), with a full CI run first.

## The key finder refit on Solomon's PC (rule 5 exception)

Solomon allowed (2026-11-01) fitting about 20 key-finder numbers on his own packs (`keyfit --rescale`); nothing else
is learned from them, and only numbers leave his PC. A Remote Control session started on 2026-11-02 at 04:01 could
not find his MIDI pack folders and asked him where they are; there was no answer, and I ended it at 05:00 for this
hand-over. Nothing ran on his packs. It may have left a clone of main in `%LOCALAPPDATA%\MusicOrganiser\keyrefit\repo`.

To resume: start a Remote Control session (folder `%LOCALAPPDATA%\MusicOrganiser`) with the brief below, word for
word. When its numbers come back, open the cloud PR: `crates/mo-harmony/src/keyfind/weights.rs` from its diff,
KEY_FINDER_VERSION 5 (rule 7), `keyfind/hand_case_tests.rs` updated, its `test-runs.md` line, and the public sets
scored with `keyfit --shipped` before and after.

> Refit the key finder's allowed numbers on Solomon's own MIDI packs, on this PC, then report back NUMBERS ONLY.
> Solomon allowed this on 2026-11-01 (the rule 5 exception: each stream's scale, the minor weight and the
> calibration, about 20 numbers, fitted with `theorybench keyfit --rescale`; nothing else is learned from his
> music). The steps are in the repo's tools/theorybench/README.md, section "Solomon's packs (on his computer only)":
> read it first.
>
> HARD RULES (data safety):
> - Work only inside a NEW folder: %LOCALAPPDATA%\MusicOrganiser\keyrefit (the clone in keyrefit\repo, the data
>   files in keyrefit\data). Write nothing anywhere else.
> - In %LOCALAPPDATA%\MusicOrganiser, only READ the settings to find the MIDI folders. Never open, copy or change
>   library.db, sounds.db, midi.db or any other file there, and never touch run-latest-rust (the launcher's own clone
>   and build, which Solomon tests with).
> - His MIDI files, projects and samples are read only: never move, rename, change or delete anything.
> - Never commit, push or open a pull request from this PC. Never upload, paste or report his MIDI, packs.jsonl, the
>   .tsv dumps, or any file, folder or pack name. Only numbers come back (counts, shares right, log-losses, ranges,
>   hand-case gates), plus weights.rs and the test-runs.md line, which hold numbers only.
> - Run `--final` at most once, and only if the keep rule below passes.
>
> STEPS:
> 1. Clone main fresh: git clone https://github.com/Silkie86/Music-Organiser-Rust.git into
>    %LOCALAPPDATA%\MusicOrganiser\keyrefit\repo (git from PATH, or the copy Visual Studio brings, as
>    tools\dev\run-latest.ps1 finds it; with no git at all, download main as a zip from GitHub). Note main's commit
>    hash. Rust is already installed (the launcher builds with cargo).
> 2. Find his MIDI pack folders in the C# app's settings in %LOCALAPPDATA%\MusicOrganiser (read only). If you can't
>    find them, ask Solomon where his MIDI packs are.
> 3. From keyrefit\repo: `cargo run -p mo-theorybench --release -- packs "<each MIDI folder>" --out
>    %LOCALAPPDATA%\MusicOrganiser\keyrefit\data\packs.jsonl`. Note the summary's counts and any warning (fewer than
>    10 packs, or one pack with more than a third of the pieces), without names.
> 4. `cargo run -p mo-theorybench --release -- keyfit %LOCALAPPDATA%\MusicOrganiser\keyrefit\data\packs.jsonl
>    --inputs full --shipped --dump %LOCALAPPDATA%\MusicOrganiser\keyrefit\data\shipped.tsv`
> 5. `cargo run -p mo-theorybench --release -- keyfit %LOCALAPPDATA%\MusicOrganiser\keyrefit\data\packs.jsonl
>    --inputs full --rescale --against %LOCALAPPDATA%\MusicOrganiser\keyrefit\data\shipped.tsv`
> 6. Keep rule (README): on folds 1 to 4 against --shipped, the whole loops' accuracy rises with its range above 0,
>    OR their log-loss falls by 0.01 or more; AND no set gets worse beyond its range; AND no hand case's gate breaks.
>    If it fails (or the rescale stops, for example his pieces fall in fewer than two folds), stop here and report.
> 7. Only if it passes: run the same keyfit line with --rescale --final once (it adds a line to test-runs.md in the
>    clone), then with --rescale --write crates/mo-harmony/src/keyfind/weights.rs. Don't change KEY_FINDER_VERSION
>    or any test: that's done in the cloud afterwards.
> 8. Report back: main's commit hash; the packs summary counts; the --shipped and --rescale reports (tables of numbers
>    by set and kind of input, no file names); the keep rule's verdict with the numbers it rests on; the --final
>    report and the exact line it added to test-runs.md; and the full git diff of weights.rs. Leave keyrefit\data on
>    this PC (the dumps may be needed for a later comparison; Solomon can delete the keyrefit folder whenever he
>    likes).
>
> About the MIDI folders: if Solomon says to read the setting, do it on a copy, never on the original: copy
> library.db (plus library.db-wal and library.db-shm if they exist) into %LOCALAPPDATA%\MusicOrganiser\keyrefit\data,
> read only the MIDI folders list from the copy, then delete the copy. The original library.db stays unopened. If he
> types the folders, or puts them in keyrefit\data\midi-folders.txt, use those. Either way, don't repeat the folder
> names back in your reports.

## Waiting on Solomon

- **Where his MIDI packs are**, for the refit above.
- **Three chord-name choices** (explained to him on 2026-11-02; the C# app's names stay until he says yes), my picks:
  1. C13(#11) and C9(#11) instead of C13(b5) when a 9 or 13 sounds; C7(b5) kept for the bare shell.
  2. Eb/E, not Eb/Fb.
  3. Write C7(#9,#5) and C7(b9,#11) instead of C7alt.
  Once he answers: a `chord.rs` PR (bump the reader's version, rule 7).

## Working notes

- Before pushing (what CI runs, Rust 1.99): `export CARGO_INCREMENTAL=0`, `cargo +1.99 fmt --all --check`,
  `cargo +1.99 clippy -p mo-harmony -p mo-theorybench --all-targets -- -D warnings`,
  `cargo +1.99 test -p mo-harmony -p mo-theorybench`.
- After a squash merge, the thread's branch restarts from main without a force push (refused):
  `git fetch origin main <branch> && git checkout -B <branch> origin/main && git merge -s ours origin/<branch>`,
  then push. Merging needs the PR head's full SHA.
- The bench data, the voice.py renders, the C# app's answers and the fits lived in the old session's scratch and are
  gone; rebuild them with the README ("The labelled music", "Charts played as music", the C# baselines; C# helper in
  `tools/theorybench/cs`). A fit: `theorybench chordfit <pop909cl.jsonl> <billboard-A.jsonl> --steps 40 --out
  fit.json` (about 45 minutes on 4 cores, about 62 s a reading); score it with `chords --weights fit.json` on the
  sets in the README's first-fit table.
- The hand cases with fitted numbers: `SET="..." cargo test -p mo-harmony --release show_the_cases_read_whole --
  --ignored --nocapture` (chordfit prints the SET line).
