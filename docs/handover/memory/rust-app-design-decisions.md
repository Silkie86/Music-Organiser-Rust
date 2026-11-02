---
name: rust-app-design-decisions
description: Solomon's agreed app map, keys, page ideas and FL reader notes for the Music Organiser Rust rebuild
metadata:
  type: project
---

- Default keys: Presets Ctrl+3 (app.goPresets, new), MIDI Ctrl+4 (app.goMidi, C# id), Plugins Ctrl+5, Space Ctrl+6.
- Mock-ups: https://claude.ai/artifact/Fyra62DanmTLkG6cYDPzNT
- Style (design system, 2026-10-31): https://claude.ai/artifact/YVTaf9wbierXjBX8ocHyUj. Screens in Solomon's
  "design tool" (the Design canvas, which is what he means by it): https://claude.ai/artifact/QvnfEeSqAfNMV1SSHmEXzv.
  Do page mock-ups there, using the style's tokens.

App map decisions (Solomon, 2026-10-29):
- Rail (changed 2026-10-29 evening): Songs, Sounds, Presets, MIDI, Plugins, Space (Ctrl+1..6), Settings; the
  Playground becomes a separate app later (same workspace, shared crates). Plus Ctrl+K "jump to anything"
  (grouped results; Enter open, Space play, Ctrl+Enter open in DAW, Tab one kind, Esc close; per-page Ctrl+F stays).
- Song Health strip: "Fix all" plus a button per item; the list folds away to a one-line summary (folded by default).
- Song Overview keeps at least the C# Overview's information: header facts, status/favourite/key, counts line with
  missing counts as links, FL playlist / as-in-Bitwig switch, summary line, ruler with markers, clips showing
  notes/waveforms/automation, hover label, "not carried over" note.
- Galaxy: a view of Sounds beside Tiles/List, sharing its filters (non-matches fade), search flies the map, same
  selection across views, kit builder works on the map, Rediscover glows, F11 full screen.
- Shortcut keys are NOT printed on pages (buttons, rail, search boxes). Keys show only in tooltips, menus (rule 15)
  and the F1 key sheet.

Faster test loop (agreed; the C# build-and-test rounds were too slow): phase 0 adds headless Slint screenshots of
changed pages (fake library, Solomon's theme) on every PR and the launcher's Rust mode with `cargo watch` incremental
rebuilds; later, Slint live preview in VS Code on Solomon's PC for design sessions.

FL reader notes (2026-10-29): PyFLP (demberto/PyFLP) is GPLv3, last release 2.2.1 (2023), reference only: facts
checked against Solomon's files, never copied code unless the app goes GPLv3. It breaks on FL 25+.
- 2026-10-29: Solomon dropped Serum 1 -> Serum 2 conversion. Don't
  carry it into the Rust plan unless asked. DECIDED: Playground as a separate app (MIDI page back on the rail) and a
  Presets page after Sounds (phase 5); spec updated.
- Presets page idea (Solomon): guess each preset's role from its notes (bass/lead/chords/drums), correctable.
- Preset role guess must use the pitch the plugin actually plays: FL channels can transpose on the wrapper (root note
  shift; C# FlStudioReader reads EvRootNote; the channel pitch knob isn't read yet), so bass vs lead uses
  written note + (C5 - root) + pitch. Same for Bitwig: add Note Transpose devices before the instrument. Also read the
  plugin's own octave/coarse-tune settings from its saved state (e.g. a synth's oscillator octave) where the format is
  known, so the 'real' pitch is what the listener hears.

Songs page (phase 2) answers, Solomon 2026-10-31: Open in FL Studio is the main button until phase 5; song fields
read-only until library.db is trusted; list newest save first, grouped This week / This month / by year with a year
scrubber; "Might be the same song" line in Health with Join / Keep apart; folder watch reads new saves live with a
card. NO Elektron/hardware filter chip (scrapped). Save grouping must match the C# SongVersions exactly. Grouping improvements approved 2026-10-31: renamed-save doubts (similar plugins/channels/tempo), same file in copied folders = one save, split cache for speed. REJECTED: cross-DAW same-song doubts.

See [[rust-rebuild-status]].

DB layout (Solomon, 2026-10-31): when Rust takes over writing, it builds a NEW compact database from a copy of the
C# library.db (old file kept): song fields stored once per song, plugins and sample paths stored once and
referenced, derivable fields (name, folder, name_key, file_name, stem, installed) worked out not stored. C# then
stops seeing new changes. Until then Rust reads library.db read-only. See [[rust-rebuild-rules]] rule 23.

Outside readers checked for FL/Bitwig facts: [[external-format-references]].
