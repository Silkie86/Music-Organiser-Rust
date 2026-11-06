---
name: ui-show-dont-hide
description: Solomon's standing UI rules (4 Nov 2026): filters the same on every page, and useful things on the page, not hidden in menus
metadata:
  type: feedback
  modified: 2026-11-04T05:31:59.896Z
---

Two standing UI rules from Solomon, for every page. Page briefs: /mnt/project-files/screens/page-briefs.md.

1. **Filters, the same on every page** (note 04:41, 4 Nov 2026, verbatim): "difficult to see what youve selected and
   difficult to see the options on the filters. on all filters you should be able hover scroll when its a big list and
   be able to see whats selected and with ones that only would have a few options you can display them and you can
   just select them but where it makes sense you should be able to select more that one. make sure this is consistant
   throughout the app"
2. **Show, don't hide** (05:30, 4 Nov 2026, verbatim): "theres alot of screen real estate so things dont have to be in
   as many menus and as hidden as they are makes sure things that are useful arent hidden away"

**Why:** he couldn't see what a filter had picked or what its choices were, and useful options sat in menus while the
window had room to spare.

**How to apply:**
- Filters use the shared `FilterPicks` (studio.slint) with `mo_app::filters`: a short list is chips of its own behind
  the filter's name; a long one is a chip whose menu scrolls (16 lines, ListView), every picked line ticked, the chip
  saying what's picked ("Part: Bass + 2"); more than one pick where it makes sense; each choice shows how many lines
  it would show with the other filters as they are.
- Options that were in an options (⋯) menu go on the page as chips or visible buttons; menus only for long lists or
  rare extras that are also on the page (a right-click menu repeats the card's buttons).
- Built by the screens thread ([[screens-queue]]); page threads follow the briefs.
