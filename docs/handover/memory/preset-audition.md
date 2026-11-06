---
name: preset-audition
description: Audition presets with MIDI: option A chosen (Bitwig plays it), plus "play MIDI on the selected channel in the DAW" for Bitwig AND FL; #340 Bitwig, #343 FL, #347 MIDI buttons merged; #348 Presets Audition/Keep merged (all 4 parts done)
metadata:
  type: project
  modified: 2026-11-05T21:02:00.984Z
---

2026-11-05, thread "Audition presets with MIDI in the app". Options doc (Claude Doc):
https://claude.ai/artifact/LS7gAjdwLh5zsyDqistoJV

Solomon's answers 2026-11-05 20:49: "Have option to play midi on current midi channel" + "In daw" + "Also fl studio".
So: Play on selected = the MIDI part plays through whatever is on the selected track (Bitwig) / selected channel
(FL). Option A (Bitwig plays the preset) taken as chosen (card closed on A by Claude, not his tap).

- PR #340 MERGED 2026-11-05 (squash b48dbf6), review fixes in: noted once named, Stop returns to arranger and skips the SENDS queue. Was: extension 3.19 `selectedtrack`, `clip` `audition:true`,
  `deleteclip` (only a noted audition clip still with its name; stops track/transport first). Rust
  `Bitwig::play_on_selected(path) -> Playing`, `stop_playing(&Playing)`; Error::OldExtension/NoTrackSelected/
  NotAnInstrumentTrack; stand-in fake answers the new commands. Java tests only run on CI (maven.bitwig.com blocked
  in cloud). Solomon must reinstall the extension (Settings > DAWs).
- PR #343 MERGED 2026-11-05 (squash 16b2db4): `mo_dawlink::fl::play(open_port()?, &Part{notes: Vec<NoteEvent>, ppq,
  length, tempo}) -> Playing` (drop/stop = notes let go, doesn't wait); midir behind feature `midi-port` (mo-ui
  turns it on); FL script untouched (only OnSysEx, so notes reach the selected channel). Cloud has no ALSA: check
  midi-port with `--target x86_64-pc-windows-gnu`.
- PR #347 MERGED 2026-11-05 (squash 4426c93): MIDI card Play in Bitwig (Ctrl+Shift+B) / Play in FL (Ctrl+Shift+F);
  mo-ui midi.rs IN_DAW + DAW_ASKED (stale start cancelled), FL_WATCH timer, finish_up on exit; temp "MO Play <pid>-<n>.mid"
  swept by tidy. Local branch name was pr347: push with HEAD:claude/project-thread-hnslri.
- PR #348 MERGED 2026-11-06 (squash e651756): Presets Audition in Bitwig (Ctrl+Shift+B) / Stop / Keep in Bitwig (Ctrl+Shift+K); extension 3.20
  (newtrack audition, deletetrack, stoptracks transport); parts per preset role in mo_app::preset_audition (A minor);
  a save's .bwpreset is exported to the export folder first. Solomon must reinstall the extension (3.20).
  Force-push to the branch is refused by auto mode: merge the old remote branch in instead. Preset audition track = new
  track with preset + launched audition clip; Keep leaves it. No delete-track command yet (Track.deleteObject not in
  the Java fake).
- Rejected for now: in-app VST3/CLAP host (B/C), Bitwig-recorded previews (D).

Related: [[presets-page-ui]], [[dawlink-bitwig-send]], [[midi-page-backend]].

Deferred from #348 review (accepted): preset inserted by track index (as Send); a still-starting audition isn't
stopped on app close; song sounds re-exported each audition; newest-wins code duplicated MIDI/Presets.
Also fixed there: mo-sounds copies test on Windows (fs::copy keeps mtime).
