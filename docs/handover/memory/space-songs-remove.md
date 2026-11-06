---
name: space-songs-remove
description: Space songs card: part 5 read-only (#284 merged), 5b part 1 Remove + Put back for old autosaves and caches (#287 merged), how each is checked, review facts, what waits
metadata:
  type: project
  modified: 2026-11-05T01:06:00.000Z
---

Screens thread ([[screens-queue]]); page parts in [[space-page-ui]].

**Part 5 read-only, #284 merged 4 Nov (c2a84d0).** mo-app `space_songs` (lines COMPACT + THIN; `Shown::of` works
cards out on the look thread). Shared `song_room::Others` (backup too) drops the save a song OPENS (first present
save) from what's offered; mo-space keeps newest BY TIME. Copy rule: `mo_space::song::COPY_SECONDS=60`, saves <60 s
apart (and the one before) never thinned. Test trap: on Linux `paths::stem` splits only on '/'.

**5b part 1: #287 merged 5 Nov 01:04 (6af04f6).** Remove + Put back for
CompactKind::Backup and ::Cache only; no library.db write (library never lists autosaves/caches).
- `space_songs::remove(picked, from, settings, bin, stop)`: `still_offered` measures again the songs picked from (by
  name, or a save that is an opened save), keeps only same path key + same kind. Cache goes with bytes measured NOW
  (guard doesn't compare a folder's size); Backup goes AS PICKED (guard compares file size/time -> Changed). Mutation
  checked: using fresh items for autosaves lets a rewritten one go.
- `kept_saves` (opened + newest-by-time present saves) -> Guard.keep (defence only, unreachable).
- `went_inside`: lines preview skipped inside a removed folder -> `done.went_with`; `Sent::new` gives them the outer's
  why. `Counted` noun: in-bin counted by kind, lost by one kind.
- `space_removing::log_lines` (LOGGED=50, problems grouped by why); mo-ui `send_to_bin` uses it for every clean-up.
- mo-space `clean::BinIndex` (by place = folder key + name or stem key; by id): 98 s -> 60 ms, same result.
- mo-ui: songs Remove holds only the songs look; settings come from `Shown::settings()`.
- High review: 9 fixed, 1 kept (lines show room freed after Put back, like spare/old cards).

**Waits:** identical + older saves Remove need a library.db choice: decision card
cmsg_018WW9Fe6y3WLLS9ZVAUVttKWwsikhpuV8rQpHX4BhpN2C (unanswered at 5 Nov 00:45). Unused audio: hand-picked only,
check rule 2 / spec before building. Follow-up: per-file trash on Windows is slow for thousands, and the activity
card shows no progress. #286 (Plugins thread): `mo_library::moves::add_moved_folder` writer; that thread said (5 Nov 01:40) Solomon OK'd
library.db writes for moves and #286 merges on green; it touches mo-ui plugins.rs, plugins/clean.rs, mo_app::space_old
(MOVED_FOLDERS re-exported from mo_library::moves): merge main into #288 keeping both sides.
