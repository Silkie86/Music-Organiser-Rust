---
name: midi-engine-ideas
description: Thread "Get more out of the MIDI engine" (2026-11-05): how MIDI parts attach today, ranked feature ideas doc, waiting on Solomon's picks
metadata:
  type: project
  modified: 2026-11-05T20:54:42.726Z
---

2026-11-05, Solomon: the MIDI/harmony engine feels under-used and how MIDI clips are attached is unclear.
Doc: https://claude.ai/artifact/YNSF3fcBRw4Kjq8n7o1cw2 ("MIDI: how clips attach, and what to build next").

Facts checked on main 6ded4b3:
- Parts link to their file only (midi-index.db files/notes/parts/keys; song by path match, no id). Same notes = one
  line "in N files". Songs page arrangement reads clips live from the project, not from the index; no cross-links.
- Chord reader v5 is used by NO page (only theorybench).
- extract.rs: one part per channel/track; the PhraseJoiner (back-to-back clips) is NOT ported yet.
- Solomon: OLD FL projects often spread one phrase over separate channels in a pattern (not in later FL or Bitwig);
  today those show as unrelated half-parts.

Ranked ideas: 1 rejoin split FL channels (+ back-to-back clips), 2 chords on card + chord search, 3 fits my song
(measure v5 on his songs first), 4 what goes with this, 5 Songs<->MIDI links, 6 same loop in 12 keys grouped,
7 card play tools, 8 audition preset with this part (Audition presets thread), 9 next chord/reharmonise.
MIDI drag-out belongs to "Build the MIDI page" thread. Solomon 2026-11-05 21:01: "start the low hanging fruit first" (not at his PC).
- Idea 6 MERGED #341 (64e5a4a): midi-index.db `shapes` table (analyser::shape_of), back-filled after a read;
  Catalogue groups .mid files in ONE folder with the same shape (not projects, not drums) -> Entry.key_group/in_keys,
  line_count; list shows one line "in N keys" (Solomon: any copy in another key = one line, no key buttons wanted).
- Idea 2 part 1 MERGED #345 (011ebbe): mo_midi::chart::read (first 16 bars), mo_app::midi::chord_chart lead-sheet
  line on the MIDI card (CHORDS). Not stored; chord search needs chords kept in midi-index.db (next).
- Idea 1 (split FL channels): Solomon's idea = repeats (channels whose stretches come back together); waits on
  his sample projects. Bars assumed 4/4 everywhere on the MIDI page.

Related: [[midi-page-backend]], [[midi-page-screens]], [[harmony-engine-status]], [[key-finder-refit]], [[preset-audition]].
