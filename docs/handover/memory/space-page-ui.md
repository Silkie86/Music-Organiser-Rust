---
name: space-page-ui
description: Space page UI parts by the screens thread: spare copies (#275, #279), admin Put back (#282), old copies Remove + Put back (#283), songs read-only (#284), songs Remove (#287), all merged; part 6 unused plugins PR #288 ([[space-plugins-unused]])
metadata:
  type: project
  modified: 2026-11-05T05:10:00.000Z
---

Built by the screens thread ([[screens-queue]]); backend in [[space-backend]]. Space questions:
/mnt/project-files/space-page/questions.md.

**Part 4 (read-only, merged #275, e277a6f):** mo-app `space_spare`; mo_space::spare::find takes `spare::Used`.

**Part 4b, Remove + Put back: #279 merged 4 Nov 17:52 (43473b1).** mo-app `binned` (space-binned.json via
`app_settings::write_checked`; unreadable file never written over; unplugged-drive items kept; KEEP 50 only for
clean-ups the bin can't vouch for); `space_bin` (Recycle Bin line/card, `newest_put_back`).

**Admin Put back: #282 merged (c33938b).** Removed.stamp = what the Bin step checked; stamped + `admin().can_ask()`
-> one `--put-back` step.

**Part 3b, old copies Remove + Put back: #283 merged 4 Nov 20:42 (5ec7c84).** High-effort review: 7 of 10 fixed, 3 kept
(big figure counts unticked kept files; look walks old copy twice, 2nd from OS cache; remove compares serially).
- mo-space: `Leftover.only` (size/time at look); `old_copies::remove(picked, then, now, bin, stop)`: found again in
  CURRENT records, full check (bytes=true), sure only if `l.only.is_none() && copy empty`; one `Sending`.
  `send_one`/`Going` shared with remove_old_copy/remove_old_home. `old_home_stays`: copy missing -> stays.
  `NOTHING_TO_CHECK` pub. remind runs per leftover in parallel (rayon). NotKept stays (library.db write needs OK).
- mo-app `space_removing`: generic Removing<T>; `start()` drops explicit `true` ticks (consent doesn't carry over;
  unticks stay); shared `stayed`, `crashed_words`. Shortcut `space.stop` added.
- mo-ui: `Clean<T>` statics SPARE/OLD + `send_to_bin`; `hold_looks` stops running old/spare looks at any removal or
  put back start, `look_after` re-looks after (also Measure pressed while busy); old copies Stop (`old_stop`).

**Part 5 songs (read-only, #284 merged) and 5b (Remove + Put back for old autosaves and caches, #287 merged 6af04f6):** see
[[space-songs-remove]]. Shot compare: convert("RGB") first (alpha-only getbbox lied).

**Part 6, plugins you haven't used (read-only), PR #288:** see [[space-plugins-unused]].

**Copies of the same sound (information only), #302 merged (07a539d):** see [[space-sound-copies]].

**Next Space:** 2c Recycle Bin + Put back for Bitwig copies only after Solomon hears a fixed project play (ask then
about auto-backups keeping copies). Then songs Remove part 2 (after the card), then unused plugins.
