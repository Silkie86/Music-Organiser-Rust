---
name: presets-page-ui
description: Presets page UI thread "Build the Presets page": PR #194 merged (Sounds + Curves sections), defaults picked, follow-ups
metadata:
  type: project
  modified: 2026-11-03T22:59:34.846Z
---

2026-11-03, thread "Build the Presets page" built the page on mo-presets: PR #194 (branch claude/project-thread-6oc5bc).
- Files: mo-app/src/presets.rs (logic, words), ui/pages/presets.slint, mo-ui/src/presets.rs (glue; lists are lazy
  slint Models). Keys: shortcuts PRESETS_ACTIONS (Ctrl+E export, Ctrl+B to Bitwig, Ctrl+Shift+S export shown,
  Alt+1..8 part fixes, Ctrl+Backspace use guess) and CURVES_ACTIONS (F2 rename, Delete hide...).
- Send to Bitwig: export .bwpreset then mo_dawlink Link new_track + insert_file (uses Link directly, skips the crate's
  SENDS queue: follow-up `Bitwig::send_preset` in mo-dawlink).
- Bitwig folder found from Bitwig save paths (ancestor "Bitwig Studio"), so <Bitwig folder> works without a setting.
- Defaults chosen (not yet confirmed by Solomon): newest save of each song only; unused synths hidden (Options shows);
  curves scan every save (curves.db caches).
- #194 MERGED 2026-11-04 (squash 90d3bb1). Presets keys also listed in Settings › Shortcuts (PAGES: Presets, Curves).
  Solomon's 2026-11-04 ask (credits low): no new code until he says so.
- Follow-ups merged 2026-11-04: #235 mo_dawlink::bitwig::send_preset (shares the SENDS queue; fake Bitwig loads
  .bwpreset as a "Polymer" device), #238 Songs Overview link "Its sounds on Presets" -> presets::show_song.
- Left: drag out of the list. No drag-out to Explorer/DAW exists anywhere in the app yet (Slint has none; needs
  Windows OLE DoDragDrop), so it's a shared part for all pages (Sounds, MIDI, Presets): screens thread's call.
  Curve bend drawing is approximate.
- 2026-11-05 F12 notes: #329 MERGED: Format filter (VST2/VST3/CLAP/AU/Native/Other, FilterPicks "format"; no hide-native,
  Solomon's choice), "From" chip renamed "Found in", From::Songs labelled "My songs".
- 2026-11-05 "presets don't send to Bitwig / dragged file loads default": cause = no Bitwig-saved .bwpreset of that
  plugin to use as template (C# had the same limit). #331: bitwig::stand_in_for + build_for_other use any template of
  the same format (VST2/VST3) and swap plugin id (fxp bytes 16..20 / vstpreset class id 8..40), name, kind; blank
  vendor/version/automation/outputs. #331 MERGED 2026-11-05 (bd87ec7). Not yet tried in real Bitwig: Solomon to try Hive, drag the .bwpreset not the .fxb.
See [[presets-page]], [[app-screens-ui]], [[dawlink-bitwig-send]].
