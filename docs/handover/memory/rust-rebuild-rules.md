---
name: rust-rebuild-rules
description: Solomon's standing rules for the Music Organiser Rust rebuild (spec section 14), plus how PRs, reviews and effort levels work
metadata:
  type: feedback
  modified: 2026-10-29T08:23:12.774Z
---

Full spec lives in the repo at `docs/spec.md`; rules also in `CLAUDE.md`. Old C# repo `Silkie86/Music-Organiser` is
read-only reference (public; scratch clone, never push).

1. Offline, native, no web UI. Windows first, Mac/Linux ready.
2. Never move or delete samples. Duplicates only hide in the app.
3. Never change an original project: Fix writes "(fixed)" beside it.
4. Never commit Solomon's projects, samples, MIDI packs, Bitwig template/binaries or model files.
5. Only counts/averages from Solomon's MIDI go in code (2026-11-01 exception: ~20 key-finder weights, see
   [[harmony-engine-status]]).
6. Never add a column to `plugins`; schema changes only add. RELAXED (Solomon, 2026-11-05, C# dead): each Rust DB carries a version; a new version may rebuild its file by copy, check, rename. See [[app-feel-instant]].
7. Bump a reader's version when what it finds changes.
8. Bitwig: objects defined at first use; no class-1 ref in single-object field; decode must end at the zip offset.
9. FL: 3-byte event 172 from FL 25; note size by FL version; read damaged saves on with a warning.
10. Live tests only touch a project named "MO Live Test…" with changes allowed.
11. DAW scripts plain ASCII; tests never reach a real DAW, folder or MIDI port.
12. No lag: shared slots, low priority, no UI waits, newest wins, virtualised lists, read big files once.
13. Missing samples: search project and DAW folders first.
14. Song fields belong to the song.
15. One app-wide shortcut list; every menu line shows its key.
16. One shared style; reuse the main control for smaller lists.
17. Crashes always log; F12 notes keep drafts through restarts.
18. Merging: changed by Solomon 2026-10-29 -- Claude merges automatically after the code review's points are fixed
    and CI is green (no Merge card). Still don't push to the build Solomon is testing without asking.
19. Normal-effort code review on each PR before Solomon tests it; after a batch merges (at latest each phase), an
    ultracode whole-app review on `main`, then fixes.
20. Claude picks effort per task itself (higher for formats, data safety, whole-app reviews; lower for small fixes)
    and says which level it used.

Effort (2026-10-30, widened 2026-10-31 as rule 26): xhigh for anything that writes files or touches databases,
    high for readers, ultracode per phase review. 26: use whatever effort and tools the work needs; if more effort
    the first time (higher-effort reviews, workflows, agents) saves corrections later, do it without asking.

21. (Solomon, 2026-10-29) Highly multithreaded wherever it helps: scans, twin test, sound analysis, MIDI index,
    galaxy layout spread over all cores (rayon, one file per task), still at low priority and never on the UI thread.
22. (Solomon, 2026-10-30) MIDI 2.0 ready everywhere (spec 4.4; CLAUDE.md rule 22).

**Why:** set by Solomon 2026-10-29 in the hand-off spec; most are lessons from the C# app.
**How to apply:** every thread in this project; each page starts with a question round + mock-ups before building.
See [[rust-rebuild-status]].

Working mode (Solomon, 2026-10-30 20:29): "Keep going until I say stop; let me know when I need to make a decision."
Work through the phase plan without pausing for check-ins; message only for real decisions or results.

23. (Solomon, 2026-10-31) Data storage: keep databases as small as possible; store each fact once, never in several
    places; if something is quick to work out (from a path, a join, a count), work it out instead of storing it.
    The C# library.db didn't do this. Applies to mo-library and every later database (sounds.db, midi.db).
24. (Solomon, 2026-10-31) Don't port C# garbage-collector workarounds (pooling, object reuse to dodge GC pauses,
    connection bags); Rust has no GC. Port the behaviour, then make it fast the Rust way (borrow, don't copy).
25. (Solomon, 2026-10-31) Show every screenshot taken (mo-screens): copy the new/changed ones to
    `/mnt/project-files/screenshots/<page>/` and attach them (max 6 per message) when posting the result.

27. (Solomon, 2026-10-31) Don't wait for merges: start the next job while a PR's CI runs, as long as it won't get in
    the way (different files, branched so it can be rebased).
