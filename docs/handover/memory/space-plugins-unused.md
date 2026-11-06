---
name: space-plugins-unused
description: Space part 6 "Plugins you haven't used" (PR #288): how it matches the Plugins page (years, gone, moves), look/stamp rules, folder-fixes follow-up after #290
metadata:
  type: project
  modified: 2026-11-05T02:20:00.000Z
---

Space page part 6 by the screens thread ([[screens-queue]], [[space-page-ui]]); the Plugins page is [[plugins-page]].

**Part 6, plugins you haven't used (read-only), PR #288 (5 Nov ~01:40).** mo-app `space_plugins`: Found {plugins
(unused), used, years, now, sizes, measured, gone}; `as_of(years, now)` re-splits with no read; `without(gone)`
(consuming) counts plugins gone to nothing; Measured.room = mo_space PluginSpace (own room = PluginSpace::bytes).
mo-ui: Space follows the Plugins page's years (`plugins::unused_years()`) and its Clean ups/uninstalls
(`gone_this_session`); both pages leave out Space's binned spare copies (`space::plugin_files_in_bin`). Look again
when stale (5 min) but read only if `mo_library::list_copy::stamp_of` changed. Plugins page hook: `show_plugin(key,
own, name)` (gone: searches name), `show_unused`; mo-app `Plugins::show_one/show_only_unused`, `plugins::unused_since`.
`space::wins_note(nones, lines)` + NOTHING_TO_FREE; the empty Well is hidden. Medium review: 10 of 10 fixed.
5 Nov ~02:00: main's #286 (Plugins Move, leaves a link) merged into #288 with no clashes; #288 also keeps the
Plugins page's moved folders (`Page.moved`, pushed in moving.rs `moved_done`, forgotten in clean.rs `forget_binned`,
given by `gone_this_session`), so a moved folder leaves both pages' figures (its room is in Space's old copies line).
Plugins thread (cmsg_018WW9Fe6y3WLLS9ZVAUVttKRxqDYm5PDXACbbzdUynsdd) OK'd it (session-only is fine: the C# app's
next measure sees the link). FOLLOW-UP when their "Not its / Add a folder" PR merges: it adds
`Page.folder_fixes` + `plugins::folder_fixes_this_session()` (mo-ui) and `Plugins::apply_folder_fixes` (applied in
arrived() after forget_binned): Space's plugins look must apply them too, so a folder ruled out there stops counting
on Space. Small arrived() conflict expected for whoever merges second. #290 is that PR (up 5 Nov 02:16): fixes are
written to library.db (mo_library::plugin_folders), so the stamp changes and Space re-reads at its next look; to be
live, pass `folder_fixes_this_session()` into the look and recompute (no read) when they change. apply/share_added
are private there: ask for a pub way to apply fixes to Space's own rooms if Space keeps Measured, not Plugins.

5 Nov 03:00: #290 merged. Space side done on local branch space-fixes-wip (d4f8bb1, not pushed): `space_plugins::look(read,
now, fixes)` applies `folder_fixes_this_session()`; Page.plugins_fixes; shown() looks again when they differ; the stamp
shortcut only stands while the fixes match. Test in plugins/tests.rs (fails without). PR after #291 merges.
#291 (Remove progress, mo_space::clean::Heard + mo_app Tally) merged 6ce7989; told the Plugins thread (they rebuild Watched on Heard).

