---
name: app-screens-ui
description: UI/UX thread "Build the app screens": owns every page in mo-ui/mo-app and studio.slint; plan doc, page order, open questions
metadata:
  type: project
  modified: 2026-11-01T00:39:52.650Z
---

2026-11-01: Thread "Build the app screens" (cmsg_018WW9Fe6y3WLLS9ZVAUVttKKvxJTNQf5Sjt5AVuno8m9X) owns UI/UX of all
pages (mo-ui Slint + mo-app screen logic) and the shared style. Backend threads own their crates; message them for
backend changes.
- Plan doc: https://claude.ai/artifact/PdiLCg2s9fJDVA9GssfTxz (keep, improvements, page order, PR split, questions).
- Mock-ups go on the design canvas https://claude.ai/artifact/QvnfEeSqAfNMV1SSHmEXzv (style tokens installed).
- Proposed order: shared parts PR -> Sounds (5 PRs) -> Space + Backup -> Plugins -> Presets -> MIDI -> Settings/Ctrl+K.
- Main improvement: one place per action (side panel + right-click menu with keys; rows only Play on hover); page
  numbers in an Options popover; Settings folded to General/Folders/DAWs/Shortcuts/Developer; Duplicates on Space.
- Backup screen: the backup thread (cmsg_018WW9Fe6y3WLLS9ZVAUVttK7h8yNPQ6Hp4Wrv77pkq2xp) draws + builds it on the
  canvas with studio.slint parts; it needs a shared "folder list row" (path, size, whole/slimmed switch, remove, Add
  folder row) from this thread. Proposed placement: a section of the Space page (waiting on Solomon).
- Open questions to Solomon (2026-11-01): same list+panel frame everywhere; rail 208px names vs fold to icons; row
  buttons vs panel/menu only; Backup placement; Sounds first.

- #84 merged 2026-11-01 (a8b3623): shared parts in studio.slint (PageHeader+ProgressLine, Segmented (in current;
  page sets it on picked), MoreButton/RightClickMenu with MenuItem{what,key,id,enabled}, OptionsButton, SidePanel,
  PanelDivider, EmptyState, Waveform, FolderRow, Dot, Icons, MenuCard); gallery pages/parts.slint shot by mo-screens.
- Sounds part 1 next: filter/sort screen logic in mo-app; asked Sounds thread for mo_sounds::liking (Liking.cs) and
  used-in-songs counts. Backup mock-up is on the canvas as Backup.dc.html (Space page tab); re-read canvas.json
  before publishing (other threads add boards).
- Sounds part 1 = PR #92 (2026-11-01): Tiles/List/History, filters, panel, playing. Solomon asked for waveforms
  "with more resolution, like the old app": now every stored peak (160) as a thin line via Waveform{shape} +
  mo_app::sounds::wave_path; keep that look. Next: part 1b wires #87's usage + liking (off the window thread).
- Backup thread builds BackupSection on the Space place (PR #89 logic, part 2 UI); Space frame later takes it in.
- #116 (part 1b, used counts + Most liked) merged 2026-11-01. Part 2b Sounds like this = PR #119 (SidePanel
  now scrolls). Next: Rediscover UI on mo_sounds::rediscover (#117: suggest/why_nothing/years/folders, run on_pool);
  kits arrive as #118 from the Sounds thread.
- #119 merged. Part 3 Rediscover = PR #122 (view 4; Adventurous as a 4-choice MenuChip instead of C#'s slider).
  Backends ready on main: kits #118 (mo_sounds::kits roll/export, export on pool, never overwrites), duplicates
  #121 for Space (mo_sounds::duplicates::read; pass hidden sounds too; hiding stays with C# until writes trusted).
- #122 Rediscover merged 2026-11-01. Galaxy backend #123: galaxy::space(Placing, &index), db.map(space),
  out_of_date -> update(..., fresh) on pool; keep Updated.save in memory only (sounds.db read-only).
  #126 changed it: SEED is u64, a fingerprint is Cow<[f32]> (galaxy::Vector), fingerprints().ids a slice.

- 2026-11-01 merged: #122 Rediscover (view 4), #125 Kits (view 5, sounds/kit.rs; no vibe box, no Send to Bitwig yet),
  #127 Galaxy (view 2, sounds/galaxy.rs; view maths in mo_app::galaxy_view; stars drawn to one image off-thread).
  Next: galaxy Colour by/regions/legend/trail, then Space (Backup + Duplicates #121).
See [[rust-rebuild-rules]], [[rust-app-design-decisions]], [[music-backup-plan]].
- #133 merged 2026-11-01: Solomon asked for the old sliders back (Rediscover Adventurous, kit Randomness) via shared
  StudioSlider (two-way bound to SoundsData in-out properties). Re-posted the 7 open questions 20:48; waiting.
- ANSWERS (Solomon 2026-11-01 20:53): type chips always showing, MULTI-select + Clear; tiles 112px OK for now; side
  panel always open; left rail = names AND icons; actions in panel + right-click menu only (not row buttons);
  BACKUP = its OWN PAGE in the rail (room to grow), not a Space section (told backup thread). Q4 (same list+panel frame
  everywhere) unanswered -> going with yes.
- 2026-11-01 merged: #135 multi-type chips (Filters.sound_types Vec, toggle_type, Clear types chip), #137 Place::Backup
  (rail under Space, Ctrl+7 app.goBackup, backup-place in app.slint; Space shows ComingPage again), #138 rail icons
  (Icons.songs..settings in studio.slint, RailItem.icon, PlaceInfo.icon).
- 2026-11-01/02 merged (Solomon's F12 notes on b19cd34): #141 galaxy picture drawn on every core + moved at once
  (mo_app::galaxy_picture; root Cargo.toml `[profile.dev.package.mo-app] opt-level = 2` because the launcher builds
  Debug; optimising mo-ui cost ~2 min/rebuild), #142 left drag sweeps/plays (90 ms gap), right/middle drag pans,
  #144 Open in Bitwig for FL songs (Ctrl+O/Ctrl+B, Open in FL Studio beside it; songs.rs convert_save) + watchdog::doing
  markers ([Slow] lines, freeze line says the step), #146 wheel/Whole map glide (View::fly_at_mouse/glide, 16 ms timer,
  1 px/pt pictures while moving), screen-blend glow + nebula (Cloud, map-aligned), hover ring, NEAR 10.
  Solomon wants the galaxy "buttery smooth, like flying in and out of a galaxy". Notes are read from
  %LOCALAPPDATA%\MusicOrganiser\notes via device_request_folder_access on that exact folder (the
  parent folder request gets refused). The launcher can build branch pushes, not only main.
- 2026-11-02: Solomon's launcher build failed: C: was FULL ("rustc-LLVM ERROR ... no space on device", rustc exit 101).
  #147 retries after clearing target\debug\incremental on a rustc crash; #148 (launcher v4) checks free space first
  (clears incremental + release under 10 GB, won't build under 4 GB) and says plainly when the drive is full.
- #150 (2026-11-02): right-click menu on Saves tab rows; FL "(autosaved at" hidden (scan + Live::load); Sounds search min-width; watchdog marks only the window thread (WINDOW OnceLock).
- 2026-11-02: Solomon wants FL knob-move Bitwig curves (.bwcurve) as a Curves section of the Presets page; added to plan doc; asked converter thread (Carry FL knob moves into Bitwig) what data it hands over.
- 2026-11-02 #157: galaxy most zoom 0.8*sqrt(n); galaxy_names (C# FindRegions port) + MapName; sweep trail + Swept card; all mo-* crates but mo-ui opt-level 2 in dev (slow start was unoptimised library code). WMA decoder owned by Sounds backend (mo_audio::can_decode coming).
- 2026-11-02: curves backend on main (#156): mo_presets::curves CurvesDb (curves.db beside library.db, Rust-only): open/out_of_date/scan(rayon, off UI)/list/get/rename/set_hidden/has; file(&Curve, folder 'Curves' under app data); save_to_bitwig; is_export. CurveSource.song = display name, newest first.
- 2026-11-02 #161: galaxy on wgpu (mo-ui gpu.rs picks Slint femtovg-wgpu after probing a real adapter; MO_DRAW_CPU=1 forces CPU; sounds/sky.rs = C# GalaxyGpu.cs port: stamps Rgba16F, quarter blur glow, squeeze, crisp close up; numbers in mo_app::galaxy_look; hits via mo_app::galaxy_grid 192x192). Look test: GALAXY_GPU_LOOK=dir cargo test -p mo-ui --lib sky -- --include-ignored (apt install mesa-vulkan-drivers for llvmpipe). Queued: read-ahead, mini-map, arrangement on GPU, notes 03:25-03:28 (drag to DAW, Send to Bitwig on side card, ⋯ actions on cards; side card clipped right at 4K).
- Solomon 2026-11-02 04:00-04:06: drag-to-DAW needs a nice animation; galaxy "Light by" (Liked/Used/Length/Newest/Tempo-key) approved over 3D/rotation; Year filter = shared year-range strip (from Songs' year scrubber) beside the side card on EVERY list page, drag to pick a range (Sounds: years used when In my projects is on, else year added).
