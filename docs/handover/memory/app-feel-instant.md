---
name: app-feel-instant
description: "Make the app feel instant" thread (2026-11-05): whole-app audit (UI thread, speed, DB load), Rust-only databases since C# is dead; plan then PRs
metadata:
  type: project
  modified: 2026-11-05T03:15:08.350Z
---

Solomon 2026-11-05 03:12: after outstanding PRs merge, audit where the main thread can be held up, performance gains,
database load time; app should feel instant (smart/lazy loading). C# app is dead: Rust-only, OK to scrap old DBs
(library.db, sounds.db...) and build new ones.

2026-11-05: open PRs at start #280 (polish), #291 (Space Remove card), #292 (startup log, other thread). Audit started
read-only on main 56ebc59 (4 areas: UI thread, startup/page loading, databases, background jobs). Told the startup
thread (owner of #292) that the DB query/folder-walk audit is now this thread's.

Plan doc (Claude Doc): https://claude.ai/artifact/3gLinXYvu2g2SPNRfefbXT (2026-11-05). Key findings: window thread
clean after open; Sounds read_all at start whatever page (sounds.rs:497); no disk queue; app's own library.db
settings writes stale the saves copy; Plugins re-reads each visit + filters on window thread; Presets
Arc::make_mut copies 100k items (presets.rs:983); Presets/Plugins as start page bug (go_to before connect);
quick_check every preset-sounds.db open; Bitwig copies hashes in memory only. sample_usage does NOT walk info
(0/1 values in record header; measured 12-15 MB) - don't "fix" it blindly.
New DBs proposed: songs.db (replaces library.db), sounds-rust.db becomes the only sounds DB, midi notes own table.
DECIDED (Solomon, 2026-11-05 ~03:30, ticked all three in the plan doc): YES move the app's data folder to I:
(copy, check, switch; old C: folder stays until he removes it); YES archive old library.db/sounds.db/midi.db to I:
once the new Rust files have worked a week (never delete); YES rule 6 relaxed: each Rust DB has a version, a new
version may rebuild its file by copy, check, rename.
PR order: quick fixes 1,4,6,7,8; Plugins/Presets; Sounds lazy + disk queue; Songs stale-first + shared list;
remember work; new DBs (xhigh).
See [[startup-speed]], [[off-the-ui-thread]], [[small-database-files]], [[compact-sound-library]].

Split 2026-11-05 04:05 (Solomon asked if all in one thread): new threads "Build the new Rust databases"
(cmsg_...4bHi22eFESuy79H14cdkAA), "Move the data folder to I:" (cmsg_...2HXxyBszkTk8GPMc6fuseh; pointer file,
mo_app::data_move, paths::moved_from), "Remember work between app starts" (cmsg_...GMz6PWdzzX6pnCMmJHHQ9y, plan E +
background::bulk_pool()). This thread keeps: quick fixes, Plugins/Presets window-thread work, Sounds lazy load,
disk queue (B), Songs stale-first (C), shared saves list (D). PR #296 = quick fixes 4+6.
Merged 2026-11-05: #301 (settings writes keep saves copy, Plugins skips unchanged re-reads), #308 (Presets
overlay fixes `fixed_now`, `pending` lines, never Arc::make_mut), #311 (Plugins lazy list model + lowercase search
words once; filtering kept on window thread, sub-ms). Next: Sounds lazy load + disk queue, shared saves list.
#313 merged 2026-11-05: Sounds read deferred (sounds::wanted: Sounds/Space shown, Ctrl+K, or 3 s after open).
Left: one disk queue (B), shared saves list (D, waits on songs.db thread), small items in plan doc.
2026-11-05 08:03 HOLD (Solomon): don't merge anything to main until he says go; keep branches + PRs open.
2026-11-05 08:08: hold lifted (Solomon: 'you can merge if you want now').
