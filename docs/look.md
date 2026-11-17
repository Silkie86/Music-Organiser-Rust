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
  at 1280 px (`Fit` in `ui/list.slint`), so a line's name and its numbers stay within one look of each other, and
  the side card takes all the room that's left: the page fills the window (Solomon, 11 Nov: the card stopped at
  760 px too, and the right of a wide screen stood empty). The card's place is always there, empty ("Pick a part")
  until something is picked, so nothing moves when it is; on Space it shows once the window is wider than the
  lines get. Backup's right column grows the same way. `mo-screens` draws every place at 3440 x 1369 too
  (`wide-<place>.png`). Songs turns it round, since a song's card holds its arrangement: the list takes a third of
  the room up to 760 px and the card the rest, up to twice 1280 px. Settings stops at a list's width.
  Space's lines and its card stop at the same widths (the card still shows only when a line is picked: the drives
  need the room at 1440), Settings stops at a list's width, and Backup at a list and a card's.
  On Sounds the card stops at 760 px and the views made of lines (List, History, Rediscover) at 1280 px; Tiles and
  the Galaxy take all the room, since more of them is the point.

- One top row and one side card on every page (the UX review of 10 Nov, A5, A6, B2 to B4). The top row reads: views
  (where a page has them), search, how the list is ordered (only what no column heading can do, such as Shuffle), and
  the page's own action at the far right; filters start on the next row. A side card reads from the top: its name
  plate, its name, its picture (waveform, piano roll, curve), the drag strip straight under the picture, then its
  buttons in even columns (`CardActions` given the card's `room`: two on a narrow card, four on a wide one) with the
  lit play button first, then into the DAW, then show and copy, then its facts side by side. On MIDI, Presets,
  Sounds, Plugins and Space so far. A button whose words need more than a column takes the whole line, and puts
  them on more lines when they're wider than that (`StudioButton.fold-width`); a part's links go one under the other when
  they don't fit beside each other. Nothing in a card runs past its edge, however narrow it is (Solomon's F12 note,
  17 Nov 2026). Buttons that change files stand apart under a thin line (Plugins' Clean up, Uninstall
  and Move). A choice between numbers is a chip strip (`FilterPicks`); the lit-underline switch (`Segmented`,
  `TabBar`) is for views and tabs, and for one of three in the filter column.
  A chip for a choice that would show nothing ("Arranging 0") has faint words (`ToggleChip.dim`). A dot that shows
  a state is green, amber, red or grey, never a DAW's colour. Shuffle is a plain button everywhere.
- A list starts at the same height in every view of a page (the UX review of 10 Nov, E2). A view's own choices go
  in the side column where the filters are (Sounds' Rediscover and Kits; the folded tab then says CHOICES), and
  only its one main action sits in the top row (Shuffle, Roll a kit).
- A long explanation folds behind a "How this works" line (`HowItWorks`, review C7): on a Space card under its
  buttons, and at the top of Settings' Folders, DAWs and Shortcuts. What must be known before a button is pressed
  (what Remove sends to the Recycle Bin) stays in sight beside the button.
- What you give a thing is set on its card, where it shows (review A2): a status in a `MenuChip`, stars in a
  `StarsPick` (a click on the last lit star takes them away), a heart in a `HeartToggle`, tags as `TagChip`s with a
  `WordBox` to add one. A click shows at once; the library is written off the window's thread.
- Advice above a list folds into one strip (`NoticeStrip`, review A4): one line that says what there is, with
  Show and Hide, so the list starts high. Its words shorten with "..." before the Show button is pushed off. On
  Plugins it holds the install cards, the room-to-free line and the measure line; a warning the user must act on
  (a half-made copy) stays outside it.
  A line inside a card has no buttons of its own: a click picks it and the card's buttons act on it (a song's Saves
  tab), with the same in its right-click menu.

- Jobs live in the rail (the UX review of 10 Nov, A3): what the app is working on shows as slim lines under "In your
  DAWS" (`RailJobLine`: name, how far in words, its `Meter`), at most four with the rest counted, and a finished one
  stays a few seconds. Only a job that failed is a card at the bottom right, as it needs reading. Nothing covers the
  side card any more.

- Filters sit in a column at the page's side (Solomon, 10 Nov 2026: the chips "are taking up alot of space and look
  messy"; he picked the column from three drawings). `FilterColumn` in `studio.slint` holds a `FilterList` for each
  filter (its name in capitals, each choice a `TickLine` with how many lines it would show, a long list's rest in a
  menu under them) and a `TickLine` for each on/off choice. Every choice stays in sight; a choice with nothing to
  show is greyed. A view's own settings are not filters and go on the view: the Galaxy's are small drop-downs
  (`OneOfChip`) on the map. On every list page. In a window under 860 px across the column folds to a slim
  "FILTERS" tab, lit while a filter is picked; a click opens it over the list. Not every filter is a tick list: one
  of three is a small switch (Sounds' Length: All, Hits, Loops), and a span of numbers is a slider with two handles
  (`RangeSlider`, Sounds' Tempo), with bars over it for how many sounds there are at each tempo, the ones between the
  handles lit.

- Column headings order a list (the UX review of 10 Nov, B5): a click on a heading (`ColumnHead` in `studio.slint`)
  puts the list in that column's order, a second click turns it round; the heading the list goes by is lit in the
  accent with a small arrow. A page whose headings do this has no "Order" row of chips. On Plugins, MIDI,
  Presets, Songs and Sounds (Presets: a third click puts the list back as it was found; Songs: by name or status
  the time headings step aside; Sounds: its List view's Sound, Type and Key, and an "Order" drop-down stays in
  the top row beside the search, since tiles have no headings and Shuffled, Most liked and Newest have no column).

- Light spills (the frontend-design pass of 14 Nov 2026, step 1): what's on doesn't only change colour, its light
  falls on what's beside it, so "on" reads from across the room and the app looks like gear, not a dark web page.
  `Theme.spill` is the accent, faint, and always fades to nothing: along a picked list line from its accent bar, up
  the face of a picked view (`Segmented`) and behind a picked tab's name (`TabBar`) from their lit strips, and as a
  thin glow round the page's one lit button. On the rail the picked place's line is washed in the place's own colour
  (`Theme.spill-of`). Panels catch the light too: a side card has a top-lit edge like a button's, and a sunken
  screen (`Well`) a lit lip along its bottom. None of it moves, so it costs no frames, and nothing unlit gets any.
- Quiet buttons (the frontend-design pass, step 2): a side card had up to eight buttons that all looked the same.
  Now there are three steps of weight: the one lit button, the raised ones that play, send or make something, and
  flat ones (`StudioButton.quiet`: no raised face, a thin outline) for those that only show or copy where a thing is.
  `mo_app::themes::quiet_button` says which from the button's words ("Show ...", "Copy ..."), so every page agrees.
  Same sizes and places as before.
- No empty row under a page's title (the frontend-design pass, step 3): the page's own job line ("Listening to new
  sounds: 458 of 1,204") used to keep a row of its own, empty nearly all the time, so every page but Songs started
  23 px lower with a gap under its name. It now sits on the title's row after the summary (`PageHeader`), which
  shortens to make room; nothing jumps when a job starts or ends, and every page starts at the same height as Songs.
- One raised panel (the frontend-design pass, step 4): every card on a page is a `Panel` (`studio.slint`): the
  surface colour, the dark edge and a top-lit edge, as a button has. Settings' cards, Backup's, Space's drives,
  Songs' song card, the Sounds cards and every `SidePanel` are one; before, each page drew its own, some with a grey
  outline and some with a dark one. A card that says something with its outline (a drive nearly full, the Swept
  card with the keys) sets `border-color`. A new card on a page starts from `Panel`, never from a plain rectangle.
- The year strip is a little meter (the frontend-design pass, step 5): `YearRange` stands in its own sunken channel
  (`framed`, off on Songs where it already stands in the sunken list), each year a slot filled by how much of the
  list is in it, with the year's name over it. The years picked are lit as a picked line of a list is (the accent
  down the left edge and over the line), and All is a line of the strip, lit while no year is picked. No underlines,
  no link.
- Space's drives fill their rows (step 5): every drive card on a row is as wide as the others and the rows are as
  even as they can be (four drives: 2 and 2, never 3 and 1). How many go on a row is counted for the room the page
  has while a card is open, so opening one never moves the lines.
- Nothing cut short stays a mystery (Solomon's note of 15 Nov 2026: "when message exceeds limits of chip hovering
  should read the whole message"): words cut short with "…" show whole in a tooltip while the mouse rests on them,
  on every page. A new line of words that can be cut is a `CutText` (`studio.slint`), not a `Text` with `overflow:
  elide`; on a list line it takes the mouse from the line (`over: line.resting; mouse: line.mouse;`), anywhere else
  it senses it itself (`own-mouse`). A filter's `TickLine` shows its words the same way, the mouse anywhere on the
  line, and follows words that change ("Hide duplicates · checking 40%"). A line's own tooltip gives way to one put
  up from inside the line. A tooltip too long for one line goes on over more lines (560 px at most).

## Next steps

- Left out on purpose: a focus ring on every button (the app is driven by its shortcut list, and focusable buttons
  would take Space and Enter from it); activity cards in a place colour (a job doesn't say which place started it,
  and guessing from the page on show would mislead); a pulsing light on running cards (a moving thing redraws the
  window every frame).
