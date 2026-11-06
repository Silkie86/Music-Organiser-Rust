---
name: slint-layout-traps
description: Slint 1.18 layout and event traps found while building the screens (FlexboxLayout measuring, if/for items measured at one-line width and the explicit-width fix, opaque GPU pictures, popups)
metadata:
  type: reference
  modified: 2026-11-05T07:35:00.000Z
---

Traps in Slint 1.18.1 found by the screens thread ([[screens-queue]]); each cost a debugging round.

- ListLine's TouchArea after `@children` ate the children's clicks: put it first.
- The galaxy's GPU picture is opaque: anything drawn over it needs `z: 1`. z may also be a runtime value, per
  instance in a `for` (compiler passes/z_order.rs). Per-corner radii work in both renderers.
- A SidePanel's content needs a last `Rectangle { vertical-stretch: 1; }` or it spreads out.
- A PopupWindow holding a ListView needs `close-policy: close-on-click-outside`.
- A repeater keeps its rows (and an open menu its place) only if the ModelRc is the same one: change rows in place
  (mo-ui `filters::FilterModels`).
- Callbacks wired only in `connect` aren't wired in mo-screens: split a `wire_*` out.
- **FlexboxLayout:** `alignment: stretch` + `horizontal-stretch` acts as flex-grow, `preferred-width` as
  flex-basis; `cross-axis-alignment: center` centres chips beside buttons; also `flex-wrap`,
  `spacing-horizontal/-vertical`. `flex-wrap` set where a FlexboxLayout component is USED does nothing: expose a
  property and bind it inside. A wrapping one reports a "square" preferred width to a HorizontalLayout parent.
- A FilterPicks inside an `if` within a FlexboxLayout is measured as if it wrapped: keep filters unconditional.
- A HorizontalLayout row with a big min-width makes the page wider than a 900 px window: use a FlexboxLayout.
- **if/for items are measured at their one-line width** for the parent's preferred height (their layout-given
  width isn't known yet), then laid out at the real width: wrapped lines overflow. Fixes: give the repeated item an
  explicit `width` binding and each wrapping Text an explicit width worked out from it (ActivityCard `inside`,
  4 Nov: the column's preferred height then counts every line); or report the height (songs.slint `TabRoom`); or a
  zero-height marker Rectangle last whose laid-out `y` is the real bottom (SidePanel `end`; give it
  `vertical-stretch: 0`). A Rectangle round a layout takes only its MIN height.
- In a Flickable, a fixed-height child with no `y` is centred: set `y: 0`. A scrolling panel's content
  VerticalLayout: `height: max(flick.height, self.preferred-height)`; pin each ListView's `preferred-height`.

- **init before change trackers:** a component's `init =>` runs before its `changed x =>` trackers are set up, so a
  property bumped from (or during) init is the tracker's starting value and never fires. To act once a ListView made
  afresh is laid out, call a callback from its init and do the work a turn later (`Timer::single_shot(ZERO)`; mo-screens
  capture() runs timers). Sounds `list-made` (PR C, 5 Nov).

**mo-screens:** guard clicks and hovers at fixed points (compare captures before and after), move them when a row
is added above; after a wheel turn move the mouse away (`hover(window, -1.0, -1.0)`). Quick checks: a scratch bin in
crates/mo-ui/src/bin (never committed), built with `cargo +1.99 build --bin <name>` to reuse the 1.99 builds.
