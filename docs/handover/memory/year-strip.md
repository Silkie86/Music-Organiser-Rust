---
name: year-strip
description: Year strip on Sounds/Songs: #180 ported 7d51116's review fixes; that old branch must not be merged; open History-chips question
metadata:
  type: project
  modified: 2026-11-03T19:57:38.738Z
---

Year strip (YearRange/YearBar in studio.slint, first merged in #164). Thread "Port the year strip fixes to main"
(2026-11-03) brought the #164 review fixes from commit 7d51116 (branch `claude/year-strip-review-fixes`, old base
2c33903) over by hand as PR #180, merged 2026-11-03 as 12ae375. **Never merge `claude/year-strip-review-fixes`**: its useful parts are in #180.

Left out of 7d51116 on purpose:
- its galaxy brightness change (same average brightness whatever Light by): replaced by the #176 star look, which
  Solomon approved ([[galaxy-star-look]]);
- its galaxy grid cache: no gain, main already caches the light ranks.

How it works after #180:
- Shown on Sounds views 0-2 (Tiles, List, Galaxy) only, not History (3), which ignores the filters.
- Used on/off clears the picked years (`mo_app::sounds::Filters::toggle`); the worker keeps the year counts with a
  `Weak` hold on the SoundList, so it never keeps an old list alive.
- Page passes the span as places: `picked-from`/`picked-to` (SoundsData/SongsData `years-from`/`years-to`) from
  `mo_app::year_strip::picked_places`; no `picked` flag on YearBar.
- Short window: lines shrink from 20px (`full-line`), bars and round corners go below 14px, names thin to every 2nd/3rd
  year plus the hovered year and the span's ends.

Open (for the screens work, not asked of Solomon yet): History still shows the filter chips, which do nothing there
(same as the C# app). Told the screens-queue thread ([[screens-queue]]).
