---
name: plugins-page
description: Plugins page thread: PR #192 part 1 (read-only list, Install these cards, side card), design choices vs C#, what part 2 holds
metadata:
  type: project
  modified: 2026-11-03T23:23:10.236Z
---

Plugins page (spec 3.6), thread "Build the Plugins page", started 2026-11-03.

Part 1 = PR #192 (branch claude/project-thread-ri01qj): read-only. mo_library::plugins (reads plugins, installed_plugins,
scanned_plugins, plugin_scans, plugin_space; tolerates older tables), mo_app::plugins (Read/Plugins/PluginRow, chips,
Install these cards, card, songs_using, formats_used() for Space's spare-copies finder), mo-ui plugins page,
Songs "Using X" filter (songs::show_only). Screenshots in /mnt/project-files/screenshots/plugins/.

Choices that differ from C# (tell Solomon if asked):
- Counts are by SONG, not save; Most used sorts by songs then saves.
- Install these counts songs whose NEWEST save misses the plugin; "opens" = songs missing only it.
- A missing plugin never gets a size; Native devices are one row per DAW.
- Options (card count, unused years) last only for the session until the DB may be written.
- Room sizes come from the C# app's last measure (plugin_space), reused as mo_space PluginSpace.

Part 2 (xhigh, data safety). Solomon 2026-11-04 "Yes to all 3": Uninstall runs the plugin's own uninstaller then
ticked leftovers go to the bin; Move leaves a link behind; Clean up only touches ticked folders.
- 2a Clean up MERGED #276 (ee411f6): mo_app::plugins::clean + mo-ui plugins/clean.rs; re-reads library before
  sending; MIDI folders protected; strict_list_setting for folder settings; Put back all of a plugin's clean-ups;
  Watched bin records sends for a panic part way. Nothing written to library.db.
- 2b Uninstall MERGED #285 (95e7b96): mo_space::plugin_files (bin plugin files, re-checked), uninstallers::run_until
  + typed Ran; mo_app::plugins::uninstall; mo-ui plugins/uninstall.rs shares clean's card (Stage::Pick/Files/Waiting).
- 2c Move MERGED #286 (0596502, 2026-11-05): mo_library::moves::add_moved_folder (C# JSON, strict, one tx, Busy
  retried), mo_app::plugins::moving, mo-ui plugins/moving.rs (Stage::MovePick/MoveChecking/MoveReady/Moving; picker
  tied to card id; folder re-measured). Solomon OK'd moves + 2d writes ("Yes, both", see [[library-db-writers]]).
  Later maybe: Move back (needs a remove-record writer). Screens thread added show_plugin/show_unused (#288).
- 2d Not its / Take off / Add a folder MERGED #290 (56ebc59, 2026-11-05): mo_library::plugin_folders (C# settings
  plugin_space_not/added:<key>) via shared mo_library::setting_list::change (moves.rs uses it too);
  mo_app::plugins::folder_fixes (FolderFix with measured stamp, dropped once re-measured; add_problem refuses drive
  roots, your/MIDI/plugin folders, overlaps). No undo for Not its (C# parity). Part 2 DONE.
See [[space-backend]], [[rust-rebuild-rules]], [[screens-queue]].

2026-11-04 new rules for every page (page-briefs.md "Two newer rules"): filters the same everywhere via shared
FilterPicks (screens thread PR from claude/project-thread-mkvyha; Plugins chips move onto it, grouped, with counts);
"show, don't hide": useful options and actions on the page, menus only for long lists. Plugins follow-up PR: Sort,
unused years, card count and the panel's DAW pick shown on the page instead of the Options popover and menus.
