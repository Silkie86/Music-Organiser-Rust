---
name: midi-page-screens
description: Thread "Build the MIDI page" (2026-11-03): MIDI page UI plan, PR split, C# settings keys, picks posted to Solomon
metadata:
  type: project
  modified: 2026-11-05T22:30:00.000Z
---

Thread "Build the MIDI page" (cmsg_018WW9Fe6y3WLLS9ZVAUVttKJHcQqhL5x1kkMdpL7EkTtZ), started 2026-11-03, branch
claude/project-thread-m7cvm8. Page files: mo-app/src/midi.rs, mo-ui/ui/pages/midi.slint, mo-ui/src/midi.rs.

PR split: (1) browse: list + multi kind chips + From + Key chip + order + year strip + side card (new key finder's
reading of the picked part, read live from its notes; piano roll as Slint rectangles), data from midi-index.db if it
has parts else C# midi.db read-only. (2) "Read my MIDI": mo_midi::indexer::run into midi-index.db (Rust's own file),
progress/Stop/failures. Then fit to a song, What goes with this, bars X-Y, GM sound, Send to Bitwig, drag.

C# settings (library.db `settings` table): midi_folders (JSON list), midi_older_saves ("1" = every save),
midi_list_limit (500). C# shortcut ids: midi.fit Ctrl+G, midi.moreLike Ctrl+L, midi.toPlayground Ctrl+P, midi.toBitwig
Ctrl+B, midi.save Ctrl+S, midi.octaveUp Shift+Alt+=, midi.octaveDown Shift+Alt+-, midi.source Ctrl+Shift+E,
midi.playTogether Shift+Space.

Picks posted 2026-11-03 20:20 (going ahead, waiting on changes): new-finder key on card, old keys in list until index
read; multi-select kinds; order + separate Shuffle; no Play until GM sound; middle C = C4.
Solomon wants to be told when the page shows keys (to check the key finder). See [[midi-page-backend]],
[[harmony-engine-status]], [[key-finder-refit]].

State 2026-11-04 08:40: merged #191, #205 (Read my MIDI; Solomon told keys show), #223 (roll keyboard/names,
then replaced by GPU roll #261), #259 (From chips + Send to Bitwig as a clip, Ctrl+B), #265 (Kind/From/Key on shared
FilterPicks with counts, multi-pick; From: neither picked = both; mo_app::midi::view returns (hits, ChoiceCounts)).
Force-push is blocked: after a squash merge, merge main into the branch. Next: remember roll switches across
restarts, fit to a song, What goes with this, GM sound, drag out, failures list + retry, kind correction.
2026-11-05: GM loop playback (Solomon's F12 note; he chose GeneralUser GS via rustysynth): #328 engine (mo_midi::gm
sound matching: drums > file program > name words > instrument family > kind; mo_audio::gm render_loop two passes,
Player::play_looped(_if)) and #335 UI (Play loop/Stop on card, Space = app.play, SoundFont = setting midi_soundfont
in rust-settings.json else %LOCALAPPDATA%\MusicOrganiser\sounds\GeneralUser-GS.sf2, Choose a SoundFont) merged.
No download button (offline rule); offer one if his file is missing. Next ideas: pick another GM sound on the card,
tempo control, loop points.
Drag out (Solomon said yes 5 Nov): PR #346 (drag a list row or the card's DragHandle into a DAW as one MIDI clip,
`mo_app::drag_out::from_part`, Windows only like Sounds drag). Merged 9e9b161 (5 Nov 22:36).

