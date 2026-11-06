---
name: sounds-card-used-in
description: Sounds card "Used in" list (Solomon's F12 note 5 Nov 2026 08:28): how the projects a sound is used in are read and shown, design choices, follow-ups
metadata:
  type: project
  modified: 2026-11-05T10:51:20.445Z
---

Solomon's note (5 Nov 2026 08:28): "should be a list of projects used in maybe on the card somewhere you can switch to
see". Built by the screens thread ([[screens-queue]]), #336, MERGED 5 Nov 2026 (b0fc319).

- A Segmented switch on the Sounds card: "Sounds like this" | "Used in N" (SoundsData.card-list, stays from sound to
  sound). Lines: DAW dot, project name, day saved; a click opens the save on the Songs page (`songs::show_save`, as the
  Plugins card's song lines do). At most 50, newest first ("The newest 50:").
- Data: `mo_library::Library::sample_usage_with_projects` reads each counted project's path in the SAME join as the
  use counts (no second pass over `projects`, whose C# rows carry `info` mid-row; see [[startup-speed]]).
  `SampleUse.project_ids` / `Usage.project_ids` (sorted, deduped); `ProjectFile { id, path, mtime }`;
  `SoundList::set_project_files` + `projects_of(i)` (None until both use and files are known).
- A Rediscover pick that no project uses lists the projects of the sound its line says it sounds like
  (`rediscover::Suggestion.like`, mo-ui page `re_likes` by id, so moves need no remap).
- Lists saves (project files), matching the Used column's "N projects". Grouping by song (as the Plugins card does,
  with Every save) is a possible follow-up to offer Solomon.
- Review (medium): `Usage::add` counts each project ONCE by id when a use has ids (C# summed: a project naming a
  sound by two paths counted twice); id-less uses (twin tool) still sum. `recent` is capped at `projects` (a use says
  how many are recent, not which). An undated save (mtime 0) shows no day, not 1 Jan 1970.
