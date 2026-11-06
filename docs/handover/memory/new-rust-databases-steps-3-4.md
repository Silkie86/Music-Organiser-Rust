---
name: new-rust-databases-steps-3-4
description: Details of "Build the new Rust databases" steps 3a (song writes to songs.db, #316) and 4 (midi-index.db layout 2, #327), both merged 2026-11-05
metadata:
  type: project
  modified: 2026-11-05T12:10:00.000Z
---

Split out of [[new-rust-databases]].

Step 3a = PR #316 MERGED 2026-11-05 (cd3c27d): mo_library::song_writes::write(songs_db, Batch{put, missing: Missing{id,path}}) only on own songs.db (app_id+version+own_since in tx), IMMEDIATE tx + read-back. NEVER deletes a save row (scan-ignored saves stay, Live::load hides them via scan::is_ignored). Ids: row at path wins, else own id; taken new id -> next free (Written.ids, Live::renumber all at once). Live: with_writes when own, to_put/to_mark, keep() (busy -> retry 60 s; other error -> one save at a time, not_kept logged), kept details leave LiveDetails; catch_up() 20 s after start, checked after every batch, runs even if the watch fails. Not yet: edits UI, last_opened on open, change counter.

Step 4 = PR #327 MERGED 2026-11-05 (1120fd8; review fixes in; old-layout index not kept open by the roll, open_read_only waits on BRINGING, missing midi.db not marked): midi-index.db LAYOUT 2 = notes facts in `notes`, blobs in `note_data(notes PK FK cascade, data)`, `about(name,value)` with took_in_old_corrections. Older layout (0/1) -> copy_to_layout: midi-index.db.new-<pid> via ATTACH old (rw so its close removes -wal/-shm), INSERT..SELECT ids kept, every column compared + fk + integrity_check, sync, then rename ONLY if no -wal/-shm remain (= nobody else open; read-only closes LEAVE them, rw last close removes). NotNow -> page reads layout 1 (notes() has both SQLs), writer open fails "tried again next time". bring_up_to_date(path) in mo-ui midi::load: copy + C# corrections INSERT OR IGNORE once (Rust rows win), then midi.db never opened. C# list fallback, midi-index.read mark (tidy RETIRED), old-key card line, summary from_cs removed. Read conn: cache 32 MB + temp_store MEMORY (default 2 MB cache thrashes: 2 GB read; 32 MB: 678 MB layout1 vs 25 MB layout2 on 60k notes/200k parts test).
