---
name: ctrl-k-finder
description: Ctrl+K finder (thread "Build the Ctrl+K finder", 2026-11-03): where it lives, how a page adds its own source, answers picked
metadata:
  type: project
  modified: 2026-11-05T03:30:00.000Z
---

Merged 2026-11-03 as PR #195 (8c587d9; no C# reference, new). keys_changed in mo-ui lib.rs refreshes its pages/actions. Rules: [[rust-rebuild-rules]].

**Code:** logic and words in `crates/mo-app/src/finder.rs` (Kind, Source trait, find() on rayon keeping top-k per
kind, Lines/Found list source, pages(), actions(), songs(), Sounds(Arc<SoundList>), lines(), hints(), Recent);
glue `crates/mo-ui/src/finder.rs` (add_source, on_open, toggle/close, Newest worker on the pool); look
`crates/mo-ui/ui/finder.slint` (FinderData global, own TextInput because SearchBox can't pass Tab/Up/Ctrl+Enter).

**How a page joins (Plugins, Presets, MIDI, Settings threads):** from its own connect: call
`crate::finder::on_open(Kind::Plugin, |app, hit, how| ...)` once (go to the place with `crate::go_to`, pick hit.id),
and `crate::finder::add_source(app, "plugins", Arc::new(src))` each time its list is read (same name replaces).
`src` is a `mo_app::finder::Lines { kind: Some(Kind::Plugin), lines: Vec<Found>, .. }` or its own Source impl.
Kind already has Preset, MidiPart, Plugin, Setting. No edit to finder.rs needed. Build the source off the window thread.

**Answers I picked (posted 2026-11-03 ~20:05, waiting on Solomon):** grouped, best group first, 4 per group with
"N more" lines; Up/Down, Enter, Ctrl+Enter (song in its DAW), Tab/Shift+Tab one kind, Esc; Space plays a sound
only after the arrows moved; empty box = recent jumps (this session only) then every place; opening clears that
page's filters only if they hide it; faint keys line at the foot; actions show their key at the right.

**Part 3, PR #293 merged 5 Nov 2026 (634b5cb):** Plugins (id = key\u{1f}own, `plugin_of`; opens via plugins::show_plugin), Presets
(`finder::Presets(Arc<Vec<Item>>)`, id = place in items, added once the read ends; presets::show_preset clears
hiding filters, picks after the filtering via to_pick/pick_now), MIDI parts (`midi_parts`, id = phrase hash, built in
midi::Loaded; midi::show_part, `wanted` before the index is read), Settings tabs (`finder::settings()`, words per tab;
settings::show_tab). mo-ui `finder::when_opening(f)`: runs at each open; plugins/presets/MIDI index are read once in
the background on first open (MIDI: index only, no read_midi). lib.rs `page_actions(place)` shared by key sheet and
finder; page actions run only on the page on show. Fuzzy letters must sit within 3x the word's length.

