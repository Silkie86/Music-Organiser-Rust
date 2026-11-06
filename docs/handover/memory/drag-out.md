---
name: drag-out
description: Drag sounds, MIDI clips and audio clips out of the app into a DAW: design, files, Windows/Slint/winit facts, review fixes, what's left
metadata:
  type: project
  modified: 2026-11-04T21:45:00.000Z
---

Thread "Drag sounds and clips into a DAW" (2026-11-03, screens queue item 5), branch `claude/project-thread-ni66xq`,
PR #206 MERGED 2026-11-04 as eae7fd9 (squash, on main 2da0b0b). Solomon then said: no new code, credits low.
PR #277 MERGED 2026-11-04 as 54b030d: the Galaxy's Swept list drags out too (SweptLine.place, via drag-alike).
**Next:** Solomon tries the 5 steps in the PR body on Windows (FL and Bitwig). Follow-ups, when asked: fold
`drag_out::sweep` into `mo_app::tidy` (now on main); tiles on the GPU (#243): pass the lifted row to
`TileGrid.picture` so a dragged tile's frame and waveform dim too (only its words do now); Mac/Linux drag; count
drags in sounds.db once that page is trusted. A drag with audio waits for #249's file search (`STILL_LOOKING`);
drag_card.rs's swash catches are `mo_catch::catch` (#242).

**What goes out:** Sounds tiles, list lines, kit pads and "Sounds like this" lines as the sample's own file (rule 2;
`is_file` off the UI thread). Arrangement: a clip, or a track by its name, as a .mid (FL pattern clip: a MIDI track per
channel via `PreviewNote.part`/`PreviewClip.parts`; same-named FL channels made unique "Perc 2" in the FL reader;
step notes lengthened) plus its samples (each checked `is_file` when dragged; "%" in names is fine). One-part clip's
card line = the clip's name. Left-drag on clip/track name drags; empty space or an empty track's name pans.

**Code:** mo-app `drag_out` (payloads, Temp folders, sweep, `loudest`/`share` helpers), `drag_card` (tiny-skia+swash),
`arrangement/pick.rs` (`pick_at`, `left_press`/`moved_far`, DRAG_START 4); mo-ui `drag.rs` (`start`/`busy`/`let_go`),
`drag/ole.rs`, `sounds/drag_out.rs` (`card_at_row` for mo-screens), `gpu::after_next_frame`, `watchdog::pause`.
mo-screens composites the card into sounds-/arrangement-drag-lifted.png.

**Temp files (rule 28):** `temp/mo-drag-<pid>-<n>` + `mo-drag.txt` first; sweep removes only listed plain .mid of exact
size a day old; `finish_up` at close checks only this run's folder names. Sweeps run on Windows only (Linux /tmp is
shared). Fold into `tidy::sweep` once the clean-up PR merges.

**Verified facts:** Slint 1.18 winit backend has no native drag (`start_drag` unimplemented) so OLE is ours. std
ListView/ScrollView set `mouse-drag-pan-enabled: false`; our SidePanel Flickable now does too (else it steals a
line's press at 8 px and sends Cancel). ListLine treats Cancel as drag-let-go. OLE eats WM_LBUTTONUP, so winit's
adapter `pressed` stays true and CursorLeft is ignored: ole.rs posts WM_LBUTTONUP after DoDragDrop (winit does the
same after WM_EXITSIZEMOVE). Before DoDragDrop, check the main button (GetAsyncKeyState, SM_SWAPBUTTON) or Windows
drops at once. WM_PAINT spins during the modal loop: QuietPaint validates it.

**Not yet:** Mac/Linux drag; drags not recorded in sounds.db. Related: [[app-cleanup-rule-28]], [[app-screens-ui]],
[[rust-rebuild-rules]], [[cloud-build-disk]].
