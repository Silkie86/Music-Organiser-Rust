---
name: app-look
description: "Give the app its own look and feel" thread (4 Nov 2026): the app-wide look direction (docs/look.md, "the rack"), place colours, Motion, Meter; PR #269 step 1
metadata:
  type: project
  modified: 2026-11-05T03:50:00.000Z
---

Solomon 2026-11-04 08:51: app-wide UI/UX pass, "don't want it to look generic, its own look and feel, intuitive and
fast, animations and design choices help the user navigate", then "It should all be coherent". Thread
cmsg_018WW9Fe6y3WLLS9ZVAUVttK2xETqPKQWLWdbyFpZZ36cJ, branch claude/project-thread-z0jrju.

Direction (repo `docs/look.md`, every page follows it): studio gear, light means "on" (accent only for picked/playing/
main action/working); each place has its colour (`mo_app::themes::place_colours`, `Theme.place(i)`, `Theme.here`,
`here-colour`) on the rail icon, the one sliding rail light and the PageHeader mark only; `Motion.tap/move/settle`
(90/180/260 ms), only small shapes move, never a whole page; levels as `Meter` (LED segments); titles in Space
Grotesk (`Theme.title-font`, mono on Terminal).

Step 1 = PR #269 (shared style only). Lessons: no easing on ListLine/MenuLine (virtualised lists reuse lines);
sliding lit parts (Segmented, TabBar, rail light) report the picked child's x/width and stay unanimated until a 1 ms
Timer after the first frame; Rust `go_to` sets `Theme.here` before `set_place` (changed handlers lag a frame).
Step 2 = #271 merged (13f73f7): Space drive bars on Meter, EmptyState shows Icons.place(Theme.here), Ctrl+K icons
in place colour (FinderLine.place). Plugins room bar is a stacked share, left alone. TabBar now belongs to the screens
thread (wrapping tabs). Step 3 = #272 merged (951edff): title-font on side-card names, dialogs, EmptyState; SearchBox has Icons.search. Left out on purpose (docs/look.md): button focus ring (steals Space/Enter from shortcuts), place-coloured activity cards (jobs have no place), pulsing lights (full redraw each frame). Questions for Solomon: /mnt/project-files/look/questions.md. Coordinate page-file edits with [[screens-queue]].
Polish pass 1 = #280 merged (ad2ab7f, 5 Nov; Mac job had no runner, Solomon's "Merge all"). Joined filter strips =
#295 merged b870ffb (this thread took FilterPicks/ToggleChip/MenuChipFace/PicksChip from the screens thread): FilterName cap,
joined-left/right corners, `self.x > 0` rounds a wrapped line's first chip, 2 px dark chip edge (Solomon 17:41 "more
black border"). C# app had no joined filters (small-caps headings in a side card).
