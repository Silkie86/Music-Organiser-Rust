---
name: galaxy-arrow-keys
description: Galaxy arrow keys (nearest star that way, Swept card Up/Down) and Rediscover's year strip, PR #334 (2026-11-05)
metadata:
  type: project
  modified: 2026-11-05T10:32:23.050Z
---

Thread "Fix Galaxy arrow keys and Rediscover years" (branch claude/project-thread-8d9evl), from Solomon's F12 notes
09:07 and 08:26 on 2026-11-05. PR #334 MERGED 4df1fd5 (2026-11-05). Rediscover's years now go through
`mo_app::year_strip::strip` (#333); the twin tool's year wording is a judged difference in mo-sounds DIFFERENCES.md.

- Galaxy arrows: `mo_app::galaxy_grid::Grid::toward` (within 45 degrees, at least NEAR points on, score = distance + off-line);
  mo-ui `galaxy::arrow` picks, puts it on the trail, autoplays and keeps it in sight. `SoundsData.galaxy-swept-keys`
  (set by clicks on the Swept card, cleared by a map press, Left/Right, Clear or `keys` losing focus) sends Up/Down to
  the card. Hints are shown on the map and the card.
- Rediscover: `Options.years: Option<(i32,i32)>`, `rediscover::years(sounds)` -> (year, count), and SoundsData
  `re-years*` feeding the same YearRange as Tiles/List/Galaxy.
- Accepted: an arrow press during a glide replaces it.

See [[screens-queue]], [[year-strip]], [[ui-show-dont-hide]].
