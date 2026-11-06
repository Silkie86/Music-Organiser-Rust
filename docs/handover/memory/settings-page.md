---
name: settings-page
description: Settings page thread (2026-11-03): rust-settings.json store, tabs built, Solomon's answers, what's next (Shortcuts, DAWs)
metadata:
  type: project
  modified: 2026-11-03T20:35:39.098Z
---

Thread "Build the Settings page" (cmsg_018WW9Fe6y3WLLS9ZVAUVttKGgUfpbm4fYdE9rFrBXM4QH, branch claude/project-thread-jf7vpx).

**DECIDED (Solomon, 2026-11-03 20:21):** Rust keeps its settings changes in its OWN file `rust-settings.json` in the
app's data folder (mo_app::app_settings), reading the C# app's choices from library.db's `settings` table (read-only)
for any key not changed in Rust (keys in FROM_CSHARP, same names as C#). C# never sees Rust's changes. Written via
.mo-part + sync + rename; an unreadable file is kept as `.unreadable`.

**Built (PR 1):** General (theme cards, accent, font, size of everything, start page, pace, sound output, About with
Slint's AboutSlint "Made with Slint" + version), Folders (show only, from library.db: roots, sample_folders,
midi_folders, plugin_folders or mo_space defaults, space_moved_folders), Developer (version, build, data/logs folders,
this run's log, Try an activity card). main.rs now opens in the saved look/keys/start page.
- Size of everything applies AT ONCE: Window::dispatch_event(ScaleFactorChanged + Resized) with system scale x size.
- Pace: mo_app::background::Pace (Gentle half, Balanced cores-2, Fast all); Rust default Fast (unset); pool size fixed
  at start, so a change takes over next start.
- Sound output: mo_ui::sounds::player()/choose_output(); Settings lists outputs off-thread.
- std-widgets Palette colour scheme follows Theme.light (apply_look), for the Slint logo and scroll bars.
- Inter font left out (not bundled); a saved "Inter" follows the theme.

**Merged:** PR 1 = #193 (2026-11-03, squash 24fd2af). **Shortcuts tab = PR #199** (one live KEYS list in mo_ui,
mo_ui::shortcuts() + keys_changed(); key set rules ported from C# in mo_app::shortcuts::set; screens thread told to
hook Space's tooltips, which still copy at start). Force-push is blocked here: after a squash merge, restart the
branch from main and join the old branch with `git merge -s ours` so the push fast-forwards.
**Merged #199** (Shortcuts). **DAWs tab = PR #202**: read-only Bitwig check (Bitwig::check) + FL "comes later"; installing scripts and live tests still in the C# app. **Solomon said YES (2026-11-03 23:32)**: the Rust app may install the DAW scripts into his Bitwig and FL folders, and may edit his music folders in library.db. **Next:** folder editing, then script install. Folder editing waits for Solomon's OK to write library.db.
See [[rust-rebuild-rules]], [[app-screens-ui]], [[dawlink-bitwig-send]].
**DECIDED (Solomon, 2026-11-03 23:58):** the Bitwig Java extension's code MOVES into the Rust repo; CI builds it and the Rust app ships and installs its own copy (the C# copy stops getting updates). FL script install: mo_dawlink::scripts (from extensions/fl-studio).
2026-11-04 00:30: #202 merged. Folders = PR #204 (mo_library::folders, the only library.db writer). Local branches waiting their turn (one PR at a time on claude/project-thread-jf7vpx): daw-scripts-wip (FL install), bitwig-install-wip (= FL + extensions/bitwig-java + extensions/bitwig moved from C#, CI job "Bitwig extension", Bitwig install, launcher Add-BitwigExtension). maven.bitwig.com is blocked in the cloud session: only CI can build the extension.
2026-11-04 02:30: #204 (folders) and #221 (FL install + extensions moved) merged. #232 (Bitwig extension install + launcher builds it with mvnw, needs a JDK: winget install Microsoft.OpenJDK.21) = last Settings PR, MERGED 2026-11-04 02:34 (f24d42c). Settings page complete. Solomon asked (credits low): no new code, only merge. **Next after #232:** Solomon tests Settings at home; then live tests (phase 5) and FL MIDI port setup (still in the C# app).
2026-11-04 06:10: "useful things not hidden in menus" applied to Settings: Start on = Segmented row on its own card, PR #254 merged (55b7b8e). Play through stays a MenuChip (long list).
