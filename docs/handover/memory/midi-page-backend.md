---
name: midi-page-backend
description: MIDI page backend thread (mo-midi crate): parts done, plan, improvements over the C# MIDI page
metadata:
  type: project
  modified: 2026-10-31T10:23:17.978Z
---

Thread "Build the MIDI page backend" (started 2026-10-31). Crate `crates/mo-midi`, backend only (page UI stays in the
file readers thread). Solomon 2026-10-31: "don't just copy the old midi page, look for improvements".

- Part 1 = PR #35 (merged 2026-10-31): read-only C# midi.db (`MidiIndex`), `Catalogue` (one entry per note hash, copies, within,
  search/like/shuffle, role counts), `instruments` port, `PhraseChord`/`PhraseMember` formats. Improvements: notes by
  id only when hash matches (SQLite reuses ids), `Stopper` (SQLite interrupt) for newest-wins, rayon search.
  Left for mo-library's owner: shared read-only open helper, one `song_of(path)`.
- Part 2 (PR #41, merged 2026-10-31 with part 3): PhraseAnalyser port (normalise, split, cut points, shape, role, chroma, features, SHA-256 note hash
  first 8 bytes hex upper) + twin check against Solomon's midi.db (role, bars, lo/hi, chroma, hash). Key/chords wait
  for mo-harmony (phase 6); features slot 19-30 and 47 depend on key.
- Part 3 (PR #41, merged): indexer (rayon, low priority, newest first, away drives kept), PhraseJoiner, and NEW file midi-index.db
  (agreed with readers thread 2026-10-31): files(path,kind,mtime,size,version,error; join projects to library.db by
  path nocase, never save id; no daw/tempo/name), notes once per hash (16-bit velocity), parts per file.
  your_roles/your_octaves: read C# midi.db live read-only until the switch, keep only Rust-made fixes in own tables
  (Rust wins); copy once at the switch. Harmony thread asked whether key/chords/chroma come from mo-harmony.
- Next: folder walk via mo_library::scan::find_files (Sounds thread), PhraseJoiner (needs arrangement
  readers), chords when mo-harmony's chord reader lands (key hookup = PR #101: keys table per notes row, KeyFact + keys::VERSION = finder*100+ASKING, re-read from stored notes); ask Solomon to run tools/dev/"MIDI twin test.cmd".
- Improvement ideas: role guess with the heard pitch (FL root note, as the Presets plan), transposed copies grouped,
  keep 16-bit velocity in the new DB.

See [[rust-rebuild-rules]], [[harmony-engine-plan]], [[rust-app-design-decisions]].
