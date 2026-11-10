# The app's look: "the rack"

Solomon, 4 Nov 2026: the app should have its own look and feel, not a generic one, feel intuitive and fast, and any
animation or design choice should help you find your way and understand the app. It should all be coherent.

This page is the direction every page follows. The tokens and parts live in `crates/mo-ui/ui/studio.slint`; a page
never makes its own.

## The idea

The app is a piece of studio gear you keep your music in: dark panels, sunken screens (`Well`), top-lit buttons, and
light that means something. It should feel like a rack unit on a desk, not a web page.

## Five rules

1. **Light means "on".** The accent colour is only for what's on: what's picked, what's playing, the one main action
   on a page, a job that's working. Never for decoration. Anything lit can be turned off by clicking it.
2. **Every place has its colour**, like the coloured strips on a mixing desk (`Theme.place(i)`, from
   `mo_app::themes::place_colours`): Songs coral, Sounds cyan, Presets violet, MIDI yellow, Plugins periwinkle, Space
   lime, Backup pink, Settings grey. The place's icon on the rail, the rail's light and the mark beside the page's name
   wear it, so you know where you are at a glance. Nothing else does: the colour names a place, the accent says "on".
   Light themes get darker inks; Terminal stays one green.
3. **Motion shows where things went**, at one pace (`Motion`): `tap` 90 ms for hover and press, `move` 180 ms for
   something lit sliding to where you picked (the rail's light, a view's lit button, a tab's underline), `settle`
   260 ms for something arriving (the page's mark growing in, a glow fading). Only small shapes move: never fade,
   slide or blur a whole page (that costs a frame on a big list), and nothing moves on its own unless a job is
   working.
4. **Levels are meters.** How far a job has got and how full something is show as a `Meter`: lit segments like a
   channel's LEDs, amber past a warning point. The same meter everywhere a share is shown.
5. **One voice for titles.** Page names, the app's name, side-card names, dialog and empty-page titles are in Space Grotesk (`Theme.title-font`, mono
   on Terminal); everything else in the theme's font; numbers, keys and times in its mono face so columns line up.

## Shipped

- Step 1 (#269): `Motion`, place colours, the rail's sliding light and coloured icons, the page mark and title
  face in `PageHeader`, `Meter` (job lines and activity cards), the sliding lit block in `Segmented` and underline in
  `TabBar`, hover and press easing on chips and rail lines (never on list or menu lines: lists reuse them as they scroll).
- Step 2: Space's drive bars on `Meter`; `EmptyState` shows the place's icon in its colour (`Icons.place(i)`); Ctrl+K
  results' icons in the colour of the place they live on (`FinderLine.place`).

- Step 3: the title face on every side card's name and on dialog and empty-page titles; every search box has the
  same magnifier as Ctrl+K (`Icons.search`), lit while you type.

- Polish pass 1 (Solomon, 4 Nov 16:48, "looks a bit amateur"): one control height (32 px: buttons, view switches,
  search boxes); a picked view is lifted with a lit strip along its bottom, not painted in the accent (one accent per
  row, not a block of it); the rail's header has the app's mark and lines up with the icon and name columns; Ctrl+K
  at the top of the rail looks like the search field it opens, its key in a keycap; Graphite's faint words a step
  brighter so second lines stay readable.

- Joined filters (Solomon, 4 Nov 17:19, "the chips should be visually connected"): each filter's chips are one
  strip behind its name (`FilterPicks`): the name in a sunken cap, the chips touching, its More menu at the end. A
  strip that breaks onto a new line rounds again where the line starts. A chip or menu on its own stays a pill.

- Easier small words (Solomon, 10 Nov, UX review D4: the small grey text wasn't comfortable on his 3440 px screen
  at 100%): second lines, chips, counts and dates 13 to 14 px, capitals, small and tiny words a step up too, and every
  theme's faint words brighter (darker on Paper) so they reach about 4.5:1 on the page.

- Wide windows (the UX review of 10 Nov, A1 and B1; Solomon's window is 3440 across): a page's list of lines stops
  at 1280 px and its side card at 760 px (`Fit` in `ui/list.slint`), side by side from the left with the page's rows
  of controls stopping at the same width, so a line's name and its numbers stay within one look of each other. The
  card's place is always there, empty ("Pick a part") until something is picked, so nothing moves when it is. On
  MIDI, Presets, Plugins and Songs so far; `mo-screens` draws every place at 3440 x 1369 too (`wide-<place>.png`).
  Songs turns it round, since a song's card holds its arrangement: the list takes a third of the room up to 760 px
  and the card the rest, up to twice 1280 px.
  Space's lines and its card stop at the same widths (the card still shows only when a line is picked: the drives
  need the room at 1440), Settings stops at a list's width, and Backup at a list and a card's.
  On Sounds the card stops at 760 px and the views made of lines (List, History, Rediscover) at 1280 px; Tiles and
  the Galaxy take all the room, since more of them is the point.

- One top row and one side card on every page (the UX review of 10 Nov, A5, A6, B2 to B4). The top row reads: views
  (where a page has them), search, how the list is ordered (only what no column heading can do, such as Shuffle), and
  the page's own action at the far right; filters start on the next row. A side card reads from the top: its name
  plate, its name, its picture (waveform, piano roll, curve), the drag strip straight under the picture, then its
  buttons in even columns (`CardActions` given the card's `room`: two on a narrow card, four on a wide one) with the
  lit play button first, then into the DAW, then show and copy, then its facts side by side. On MIDI and Presets
  so far.

- Column headings order a list (the UX review of 10 Nov, B5): a click on a heading (`ColumnHead` in `studio.slint`)
  puts the list in that column's order, a second click turns it round; the heading the list goes by is lit in the
  accent with a small arrow. A page whose headings do this has no "Order" row of chips. On Plugins, MIDI,
  Presets, Songs and Sounds (Presets: a third click puts the list back as it was found; Songs: by name or status
  the time headings step aside; Sounds: its List view's Sound, Type and Key, and the Order chips stay, since tiles
  have no headings and Shuffled, Most liked and Newest have no column).

## Next steps

- Left out on purpose: a focus ring on every button (the app is driven by its shortcut list, and focusable buttons
  would take Space and Enter from it); activity cards in a place colour (a job doesn't say which place started it,
  and guessing from the page on show would mislead); a pulsing light on running cards (a moving thing redraws the
  window every frame).
