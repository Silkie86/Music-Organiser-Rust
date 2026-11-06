---
name: space-library-review-fixes
description: Thread fixing whole-app review (2026-11-03) findings in space-move, space-rest, library and kits.rs: PRs #207/#212/#214 merged; #237 (bin) and #239 (folder moves) after #174
metadata:
  type: project
  modified: 2026-11-04T05:00:00.000Z
---

Thread "Fix the Space and library review findings" (cmsg_018WW9Fe6y3WLLS9ZVAUVttKL2EbSP6SGmfUxEY9NaHsyQ), started
2026-11-04. Report: /mnt/project-files/reviews/whole-app-review-2026-11-03.md (search "(space-move," etc.).
Rules: [[rust-rebuild-rules]].

- MERGED 2026-11-04: #207 588c15f, #214 22271eb, #212 af22055.
- PR #207 (claude/kits-review-fixes): kits read-only samples, failed export removes its folder, rating overflow
  (also rediscover.rs).
- PR #212 (claude/library-review-fixes): all 14 library points. Notable choices: mo-formats ProjectInfo.details is an
  IndexMap (reader order, same as C#/library.db); watch Changes gained `went` (paths that may have been folders:
  Live::update re-checks saves inside, no root walk); is_reworked skips plugins of an unread save (differs from C#).
- PR #214 (claude/space-review-fixes): spare close() (installed name longer needs a MAKERS prefix), maker folders not
  ticked, only DAW-made autosaves count (is_daws_autosave), twins keep the one thin() keeps; DIFFERENCES.md updated.
- #174 merged 2026-11-04 ~03:30; the remaining 12 points: ALL MERGED 2026-11-04 (#237 c9cac78, #239 03b8263). Thread done.
  - PR #237 (claude/space-bin-review-fixes): clean.rs 759 HRESULTs, 251 (private Prepared guard, Sending.sent map),
    285 Backup check uses song::is_daws_autosave, 852 Linux placeholder removed only while item still in Trash.
    Review fixed. Accepted: Backup check uses folders below project root, not song folder (scan never lists those).
  - PR #239 (claude/folder-move-review-fixes): all 8 folder_move/after_move points. API change: problem() and
    move_folder() take `protected: &[String]` after `moved` (CantMove::Protected). Tree.links_lead keeps inner links on
    `from`. same_file/newer use steps() (tolerance by sub-second part of times). clean::volume_of_path (Windows GUID).
    put_back_checked (pub; problem() never renames, the app/move calls it). old copy removal byte-compares (same_inside:
    Stop-aware, skips OneDrive online-only). lost() = link doesn't reach `to`. Accepted (PR comment): lost() also lists
    links pointed elsewhere on purpose; a drive root as sample folder refuses all; destination not checked vs samples.
  - Old-copies thread (Remind about old copies) will add link-target compare in differences() on top of #239.
- admin.rs:266 (temp-file name test) is already done in PR #172 (`is_temp_file`): skip.

Tips: alsa missing here so clippy per-crate, not --workspace; no Windows cross-compile (rusqlite needs mingw).
