---
name: space-sound-copies
description: Space "Copies of the same sound" line + card (question 4, information only), #302 merged 07a539d: how it reads, caches and follows the Sounds page; finder gap answered 5 Nov 20:47 (Hide duplicates thread)
metadata:
  type: project
  modified: 2026-11-05T22:10:00.000Z
---

Built by the screens thread ([[screens-queue]], [[space-page-ui]]). #302 squash-merged 5 Nov 05:23 (07a539d), branch
claude/project-thread-mkvyha. Space question 4: copies show as information only; never removed (rule 2).

**Shape:** a line under Also taking room (between App files and the Recycle Bin) + card. Card: 20 sounds (LISTED),
biggest room first, each "Show on Sounds page" (Part.sound id, `space::ON_SOUNDS`); 5 files each (FILES), keep first,
why + folder link; "Hidden in the app". Room = same-sound copies incl. hidden (hiding leaves the file); very similar
don't count. Words in mo-app `space_sounds` (look/line/card).

**Data:** mo-sounds `SoundsDb::hidden_sounds()` (Sounds list drops hidden ones) and `SoundsDb::finds()` -> `Finds
{matches, exact, sizes (only ids with copies), hidden}` in ONE read transaction. `duplicates::groups` takes `impl
IntoIterator<&Sound>` and only maps ids with copies. Test trap: never compare `Finds` by Debug (HashMap order random).

**Following the Sounds page:** mo-ui sounds.rs `Known {Reading, Unread, Read(Arc<SoundList>, db)}` via `known()`;
page flag `unread` (set on a failed read, cleared by reading() and show()). show(), use_known, brought_up_to_date and
a failed read call `space::sounds_changed`, which only acts while Space is on show and has looked. Space holds the list
by `Weak` (`copies_of`, ptr_eq), caches `copies_read: (Arc<Finds>, stamp)`; re-reads only if `SoundsDb::stamp`
changed or Measure again; a new list regroups from memory; failed look resets copies_of. `show_sound_by_id` is shared
with Ctrl+K's finder.

**Gap to raise (C# dead since 2026-11-05):** sound_matches came from the C# duplicate finder; nothing in Rust finds
copies among new samples or hides copies. Needs a Rust finder + Hide (Sounds backend / Sounds page), a sounds db
write. Offered to Solomon with #302 (5 Nov 05:05): "I can build a Rust duplicate finder if you want one".
ANSWERED 5 Nov 20:47 in his own new thread "Hide duplicate sounds on Sounds" (cmsg_018WW9Fe6y3WLLS9ZVAUVttKByChh4hjTAiw5hVqzEcMbB):
"Don't know if we need a page the shows duplicates I just think you should be able toggle hide duplicates on the sound
page in general and just make it really strict so real duplicates get hidden not similar sounding". That thread builds
a strict cached finder (same decoded audio bit for bit, or same bytes) + the Sounds toggle. Once it merges: ask
Solomon whether Space's copies card reads that finder or goes (he doubts a duplicates page is needed).
