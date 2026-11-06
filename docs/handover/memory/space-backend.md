---
name: space-backend
description: Clean/Storage (Space) backend thread: mo-space crate, safety guard design, what's done and next
metadata:
  type: project
  modified: 2026-11-05T02:20:00.000Z
---

Thread "Build the Clean and Storage backend" owns `crates/mo-space` (split out of mo-library).

Merged 2026-10-31: part 1 #38 (size, song Compact/Thin, clean preview->Recycle Bin->find->undo, app_files; Guard
checks paths as written AND real); parts 2+3 #52 (plugins, plugin_space, spare find, plugin_clean); part 4 #56
(uninstallers); part 5 #59 (folder_move: copy, "(old copy)", junction, move_back); part 6 #61 (after_move: lost,
link_again, move_home, repoint, old homes); part 7 #64 (admin.rs administrator step, squash 3becfec). The file-readers
thread put `mo_space::admin::run_step(&args().skip(1))` first in mo-ui main() (#68); AsAdmin only asks after it ran.
All clean-ups send through `clean::Sending` (send/flush/finish); `cant_keep` there refuses network drives and
OneDrive online-only (folders walked on Windows) for every flow.

Merged later: part 8 #72 (spare remove), Bitwig copies #73/#76. Rule 19 whole-crate review (2026-11-01, 45 points,
list in thread): part 1 #86, part 2 #88, part 3 #90 (tests; link test helpers now in tests/common/mod.rs) all merged.
#25 (Solomon chose "Ask before and after"): #95 merged: `--move-folder check` admin step; check_rename_or_ask asks
before copying (move_folder, link_again; link_again counts room first); move_back not checked first (by design).
All 45 review points done 2026-11-01.

Bitwig multisample copies (Solomon 2026-10-31): Bitwig on <Bitwig folder> (Library, Projects; project files still
name I:\ and %USERPROFILE%\Documents paths). Each project's `multi-samples\` holds byte-identical Library copies
named "X-3-2-2…" (GARAGE FOR BW 1: 242 MB). The Library also has a NON-identical "Kicks 1 Select-3-2-…-2" copy:
match by bytes, never by name. Solomon chose a "(fixed)" project pointing at the Library; copies go to the bin only
after he opened the fixed one and checked it plays (name rule 2 on that card). Writer: converter thread
(cmsg_018WW9Fe6y3WLLS9ZVAUVttKLARPGzSeyTZMVRoLTKcdsP), PR #69:
`mo_convert::relink::fix_bitwig_project(project, &[Relink{copy, library}]) -> Result<Fixed{file, changed, left}>`;
bin only `Fixed::changed`; pass all of a project's copies in ONE call (each call writes a new "(fixed N)"). Bin step: `mo_space::bitwig::remove_copies(&FixedProject{file, changed, library_folders, still_named}, bin, stop)`; the app passes `|copy, lib| mo_convert::relink::still_named(original, &Relink{copy, library: lib})` (converter PR #75).
Folder access to <Bitwig folder> granted (read).

PR #174 MERGED 2026-11-04 (squash e76afed): half_copy markers + clear, old_copies::remind, in_bin::remind,
write_through, batched files into your folders; Drives::holds_links + CantMove::NoLinksThere/NoLinksHere; mid-copy
changes (as_now, went, Tree::last/real_from/as_files, saved-over links replaced). PR #251 MERGED 2026-11-04 (squash
f3c22ca, Solomon "merge all"): max reviews 15-25 fixed, all mutation-checked (inner moved folders' links
re-pointed after both passes, as admin where guarded; links the 1st pass made in your folder are Tree::ours). The
26th review's points + a quick review's are #289 MERGED 2026-11-05 (squash 469498e): copy_fresh/stays_while_gone
keep a copy while its source can't be reached, also a whole copy just before the swap. Next: crash leftovers "(checking)" rename and ".mo-copying" files still need the user
(DIFFERENCES); then the Space page.

Open: where measured sizes live between runs is for the library DB design thread (rule 23).

Tips: run guarded (locked-folder) tests as an ordinary user (`su tester`; root ignores folder permissions);
Windows-only code checked with clippy `--target x86_64-pc-windows-gnu`.
Cargo.lock conflicts: take main's, run `cargo metadata`, merge right after. Force-push of a merged branch is
blocked: put follow-ups on a new branch name.

See [[rust-rebuild-status]], [[rust-rebuild-rules]].
