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
5. **One voice for titles.** Page names, the app's name and big titles are in Space Grotesk (`Theme.title-font`, mono
   on Terminal); everything else in the theme's font; numbers, keys and times in its mono face so columns line up.

## Shipped

- Step 1 (this PR): `Motion`, place colours, the rail's sliding light and coloured icons, the page mark and title
  face in `PageHeader`, `Meter` (job lines and activity cards), the sliding lit block in `Segmented` and underline in
  `TabBar`, hover and press easing on chips, menu lines and list lines.

- Step 2: Space's drive bars on `Meter`; `EmptyState` shows the place's icon in its colour (`Icons.place(i)`); Ctrl+K
  results' icons in the colour of the place they live on (`FinderLine.place`).

## Next steps

- A visible focus ring (accent) for keyboard use, on every shared control.
- Activity cards carry the colour of the place their job belongs to.
