---
name: fl-pattern-clones
description: How FL Studio pattern clones show in .flp files and the "line up clones" Fix for old FL projects (thread started 2026-11-02)
metadata:
  type: project
  modified: 2026-11-02T01:25:00.105Z
---
Solomon (2026-11-02) wants old FL projects fixed so cloned patterns sit on their source's playlist track, written as
a "(fixed)" copy. Census of 21,936 of his saves (RC, read-only, 2026-11-02):
- No clone link/event in the .flp. FL names clones "X #2", "X #3"... (series start at #2, no gaps, clone inserted
  right after its source, renumbering later patterns). Unnamed source: "Pattern N #M", where N goes stale; use the
  nearest earlier non-# pattern, and only with matching channels. No "#" name: leave alone (same channels alone is
  not a clone, e.g. guile_29 "chords main"/"chords").
- In FL 20+ "#" may also come from split-by-channel (base = channel name in 44%): be careful there.
- The problem is in ROW saves: FL 6.0-8.0 (28-byte records), many FL 9.0-9.5 and some 11.1 (32-byte, 0/0x6000 at
  4/6, row 999-n): one row per pattern, so new FL gives each pattern its own track. FL 8.5, 9.6+, 10 saves use track
  records (20480 base) and clones mostly already share a track (FL 10: 84%).
- FL 9.0 often MIXES row records and 20480 track records in one event 233; mo-formats' playlist_items rejects these
  (reads no playlist) -- a reader gap to fix (shared FL reader; converter thread owns readers).
- Census script bug: 20- vs 24-byte notes guessed from length; real readers use version (notes::note_size).
See [[rust-rebuild-rules]].
Layouts (RC census 2026-11-02): record size by version: 28 B to FL 8.0, 32 B FL 8.5-20, then 60/80/88. Classify
each record: row = u16@4 0 and u16@6 0x5000 (to FL 6.0) or 0x6000, slot 999-pattern; clip = 0x5000 then 0x5000+pattern
(or channel). 28 B slot = b12 | b14<<8 (b13 always 0x78). TOP slot: 98 to FL 10, 198 FL 11-12, 499 from 12.9.1.
FL's own conversion (midnight_33 FL 8 -> 11.0.4): rows -> clips, same pos/len, tracks packed in pattern order below
the channel-clip tracks, patterns never placed get none. Notes: 20 B in FL 3.5-8.0, 24 B from 8.5.
Built 2026-11-02 on branch claude/project-thread-xbkq3w: mo-convert/src/fl_clones.rs (line_up, fix_fl_project) and
example fl_lineup (--survey, --write). Test copies go in <FL projects folder>\MO Test\clones\.
2026-11-02: PR #155 merged (squash 24494bc). Fixed test copies remade with final code in MO Test\clones (epic_9 FL6,
midnight_33 FL8, ill vibe_24 FL9 rows, 80's baby_45 FL9 mixed, jonesy_7 FL11; vengance_10 FL10 needs nothing).
WAITING: Solomon to open them in FL 2026. Next idea offered: a Fix button on the Songs page (UI thread).
