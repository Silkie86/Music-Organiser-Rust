---
name: new-rust-databases
description: "Build the new Rust databases" thread (2026-11-05): steps 1-6 (sounds-rust.db the only sound library, songs.db replaces library.db, MIDI notes split, archive old files), decisions and who owns what
metadata:
  type: project
  modified: 2026-11-05T12:10:00.000Z
---

Thread started 2026-11-05 04:04 from the plan doc https://claude.ai/artifact/3gLinXYvu2g2SPNRfefbXT ("New Rust
databases", steps 1-6). Solomon: C# app retired, OK to archive old DBs, versioned rebuilds (rule 6 relaxed). xhigh.

Step 1 (branch claude/project-thread-f21ja7): sounds-rust.db becomes the app's own sound library. compact.rs:
copy_info key `own_since` (unix s) = adopted; set on the page's own passes (sync_reading/remake_reading), by
read_for_page when the copy is in step (`adopt`), or by `last_sync` (page's behind path, mo-ui bring_up_to_date).
After that: in_step true, sync() refused, read_for_page never behind and works with sounds.db gone; a damaged own
copy is remade from sounds.db while it exists, keeping own_since. Rust writes nothing into the sound library yet, so
a remake loses nothing: MUST change when Rust writes plays/stars/scans, and a FORMAT bump (step 5) must convert the own
copy itself (sounds.db may be gone). Sample finder: mo_app::sound_library::
database_in/database_beside (sounds-rust.db, falls back to sounds.db only before the copy exists).

Step 1 = PR #303 (review fixes: copy adopted as is when sounds.db gone; data_move::LEFT_BEHIND now pairs (old,
replacement): sounds.db + -wal/-shm left behind only while sounds-rust.db is there). library.db NOT left behind yet.
Step 2 = PR #309 MERGED 2026-11-05 (6cf6f52): mo_library::songs_db = C# tables/columns unchanged
(so Library + settings writers work as is) but projects.info -> project_info(project_id, deflated BLOB); sample_index
+ plugin_ids dropped; songs_info(made_from = list_copy::stamp_of(library.db), made_at, own_since). make(): one
read snapshot, rowids kept, SHA-256 of every row read back + integrity_check, .new-<pid> then rename; refuses unknown
tables/columns and never replaces an own/newer/unreadable songs.db. at_start(dir) in main.rs: own -> songs.db;
made + library.db unchanged or gone -> adopt; else library.db + mo_app::song_library::make_later (45 s, card).
stamp_of now counts an empty -wal as none. PR #309 review fixes: at_start(dir, app_version); adopt failure -> library.db this start (never write a non-own songs.db); make removes an out-of-step songs.db first; failed make (not room) writes songs-db-failed.txt (stamp, build, tries, last, why): 2 tries same stamp+build -> wait 1 week. tidy removes library-list-rust.db once songs.db is own. Space lists the
libraries in use. A Rust settings write to library.db before the switch just means songs.db is remade next run.

Coordination: data move #299 merged (paths::moved_from() = old folder after a switch: archive step looks there). Instant thread skips plan C; Plugins/Space skip re-reads while stamp_of(library) is unchanged. See [[app-feel-instant]], [[compact-sound-library]].

Step 3a = PR #316 MERGED (cd3c27d): song writes to own songs.db. Step 4 = PR #327 MERGED (1120fd8): midi-index.db layout 2. Details: [[new-rust-databases-steps-3-4]].

Step 5a = PR #337 MERGED 2026-11-05 (d552ae2): sounds-rust.db FORMAT 2 (see
[[compact-sound-library]]). Old own copy (format 1) converted by compact::lay_out AFTER the page has its sounds
(card "Making your sound library smaller"); never before. NotNow -> read as is,
retried next Sounds open. Step 5b part 1 = PR #338 MERGED (c22ef24): first 200 of the page's shuffle (shown_ids via path index
+ first_shuffled + some(ids)) sent before all (Step::Rest, prefix of the whole shuffled view, Page.partial); only
Tiles/List + Shuffled. Part 2 = PR #339 MERGED (6ded4b3): copy's all() w/o waves, SoundsDb::waves(ids), mo-ui sounds/peaks.rs. Folder ids
deferred (mo-library sample finder reads sounds.path). Step 6: archive old DBs to I: after a week, never delete.
