---
name: screens-queue
description: "Work through the screens queue" thread (2026-11-03): owns Space, Sounds, Songs, the shared style; merged PRs, 5 Nov F12 notes ALL merged (#325 #332 #333 #336), export folder row #342 merged, what waits on Solomon, lessons
metadata:
  type: project
  modified: 2026-11-05T22:05:00.000Z
---

Thread "Work through the screens queue" (cmsg_018WW9Fe6y3WLLS9ZVAUVttK7AGnQrihYA63AXBV2evuEv, branch
claude/project-thread-mkvyha). Space work: [[space-page-ui]]. Older: [[app-screens-ui]]; rules: [[ui-show-dont-hide]];
traps: [[slint-layout-traps]]. Page threads follow /mnt/project-files/screens/page-briefs.md.

**Merged:** #181-#274; Space #275-#306 (see [[space-page-ui]], [[space-songs-remove]], [[space-plugins-unused]],
[[space-sound-copies]]); #281 job card titles; whole-app review Sounds fixes #312 #314 #317 #319.

**5 Nov F12 notes (all merged by 11:55):** #325 exports: a picked export folder that's missing STOPS the export
with a card (Solomon's card choice 07:38; no Documents fallback, his C: is nearly full); one export root in mo-app.
Export folder row: Solomon said yes 20:48 ("Yeh there should be"): #342 MERGED 5 Nov (94ea48a). Exports card at
the bottom of Settings › Folders; writes `export_folder` in the song library via `folders::set_export_folder`
(setting_list::change_text, one shared safe transaction); exports and card share `bitwig_convert::export_setting_in`.
#332 Space draws nothing while hidden, Program Files "(old copy)" goes to the admin step. #333 `mo_app::year_strip::strip` (no
years after this one or before 1981, all five pages) + Sounds card DragHandle. Used in switch: [[sounds-card-used-in]]
(#336, merged b0fc319). Galaxy keys + Rediscover year strip: other thread (claude/project-thread-8d9evl, #334).

Still waiting on Solomon: saves Remove card (cmsg_018WW9Fe6y3WLLS9ZVAUVttKWwsikhpuV8rQpHX4BhpN2C), Space 2c. The Rust
duplicate finder offer is answered (20:47, his "Hide duplicate sounds on Sounds" thread builds it; [[space-sound-copies]]).
Fold into the next Settings PR: the tab bar's kept-note ("the C# app's settings are only read") is stale since the
Folders tab writes the library; folders.rs `change` still says "open the C# app once to make it".

Solomon may pause merges while he tests at home (held 08:03, lifted 08:08 on 5 Nov): follow the newest word.

**Shared filters:** `mo_app::filters` (CHIPS_UP_TO=10, choices go by WORDS), studio.slint FilterPicks, mo-ui
`filters::FilterModels`; FilterPicks, ToggleChip, MenuChipFace, PicksChip and filter row spacing belong to the look
thread (claude/project-thread-z0jrju): use, don't change.

**Lessons:** check `git branch --show-current` before pushing. After a squash merge: `git checkout -B <branch>
origin/main && git merge -s ours origin/<branch>`, cherry-pick. Run mo-screens (`cargo run --bin mo-screens -- <dir>`)
before pushing card layout changes: its guards click fixed points. Old libmo_ui/mo_screens in target/debug/deps are
400 MB each: delete older ones. [[screenshot-compare-traps]]. Locked-folder tests (mo-space tests/admin.rs) stop early
as root: run the test binary as nobody (`setpriv --reuid=65534 --regid=65534 --clear-groups`). Windows-only code:
clippy it with `--target x86_64-pc-windows-msvc` on a copy (scratchpad make-wincheck.py). On Linux
`mo_formats::paths::stem` splits only on '/': use `stem(file_name(path))` for Windows paths in tests.
