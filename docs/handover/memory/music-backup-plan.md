---
name: music-backup-plan
description: Music backup feature (thread "Plan music backup options"): options doc, recommendation, Space thread's answers, waiting on Solomon
metadata:
  type: project
  modified: 2026-10-31T22:22:14.201Z
---

2026-10-31: Solomon asked for a way for the app to back up his music. Options doc:
https://claude.ai/artifact/PquPK4SFipt3WfPukAZ8XD. A = mirror on backup drive, only changes copied, temp name ->
check -> rename, replaced/deleted files kept as dated older versions, identical files stored once (NTFS hard links),
covers outside sample folders from readers' sample lists, per-song "backed up" + Health warning (RECOMMENDED);
B = packed dated snapshots; C = per-song pack (later extra on top of A); D = hand list to FreeFileSync/File History.
Decision card posted; still need: pick, backup target (drive/USB/NAS/OneDrive), trigger (button/plug-in/weekly).
Solomon 22:25: "A, but as streamlined as possible: big unused files not saved, multiple large saves not brought over".
Agreed rules (PR #67, part 1, crate mo-backup, plan.rs): left out always = SameSave, old autosaves, caches; left out
when big = mid-sitting saves >=5 MB (or NewestOnly), plain audio >=10 MB no project uses; always go = newest save,
small saves, used samples, audio beside a save / in song's own folder (bounces), "always" folders, sample libraries
(unless leave_out_unused_libraries). #67 merged 050d984. Part 2 = copy.rs #71 merged b93950a (temp, hash check, rename, older versions <5MB, 2s FAT window, hash-before-write, drive-gone check; helpers ported from mo-space folder_move, could be shared). Part 3 = hard links (count once by file id in size.rs).

Space thread answers (2026-10-31): new crate mo-backup depending on mo-space is fine. Moved folders: MovedFolder
(`from` = junction, `to` = real copy; library.db setting space_moved_folders, parse_moved) -> back up `to` once,
record `from` as link; "(old copy)" folders are ordinary folders. No file-level dupe finder in mo-space: do own
size-then-hash pass (check mo-sounds first). If hard linking, measuring must count a file once by file id (size.rs
doesn't yet). Suggests backup drive fill + last run shown on Space page, backup screen owned by backup thread.

See [[space-backend]], [[rust-rebuild-rules]].

Solomon 23:09: wants options on the backup screen: format, and Full vs slimmed "essentials" that keep only song
milestones (not every big version). Proposed levels Full / Slim / Essentials + format plain folders (rec.) or zip per
song. #74 merged bc22dfa: plan::Level + plan::milestones (first, newest, last save of each day's sitting, + marked via
SongInfo.milestones). Decision card open: what counts as a milestone (recommended: marked + last of day + big
changes). Still need: backup drive + format (NTFS needed for hard links), zip-per-song wanted?
Solomon 23:51 chose milestones = last save of each day's sitting + big changes (marked saves always go too).
#78 merged ecffb97: plan::big_changes(SaveShape: tracks/plugins/samples/length_beats): +-3 tracks/plugins, +-5 samples,
+-25% & 16 beats length; on a cut the save before is the milestone. Still need: backup drive + its format.
2026-11-01: Solomon "what you waiting for" -> keep building without the drive answer. PR #79 = record.rs
(<backup>/.mo-backup/record.tsv: hash,size,src time,copy time,path; dirty flag; size index) + hard-link sharing
(compare bytes, link, verify same file; cant_share flag for exFAT). Next: backup screen (mo-app logic + mo-ui page),
zip-per-song format, song panel "backed up" line.
2026-11-01: #79 merged a5c89da. PR #83 = pack.rs one zip per song (Packer, pack_names by song id, pack.txt owner line,
crc fallback for time shifts; safe_file_name moved to mo_formats::textscan). Solomon 00:21 "should be able to bulk
back up by folder" -> folders.rs (folder list, Mode Whole/Slimmed, gather -> files + always), next PR after #83.
#83 merged 4292def (pack + folders). "Build the app screens" thread owns UI/style: draw backup mock-up on its canvas
https://claude.ai/artifact/QvnfEeSqAfNMV1SSHmEXzv, build only from studio.slint parts; it proposes Backup as a
section of the Space page (asked it for a shared folder-list row part). Waiting on its placement answer.
2026-11-01 03:20: backup screen mock-up drawn as artboard Backup.dc.html on the screens canvas (Space page, Backup
tab): drive card + Back up now, How much (Full/Slim/Essentials), Format (Plain folders/Zip per song; Whole folders
stay plain in zip mode), folder list (FolderRow from PR #84 in studio.slint), "What goes / left out and why" panel.
Waiting on Solomon's verdict before building mo-app logic + Slint page.
2026-11-01 04:xx: Solomon liked the mock-up. PR #89 = mo-app/backup.rs (Settings in data_dir/backup.txt, prepare,
run, page lines) + Library::sample_paths. Zip mode: songs zipped, everything else (whole folders, files no song owns)
plain. Song zip id = min save id. Next: part 2 Slint page (Space page doesn't exist yet; coordinate placement in
app.slint with the screens thread), then song panel "backed up" line.
#89 merged d3735fe. #94 merged 3624638 (2026-11-01) = part 2 Slint page (pages/backup.slint BackupSection + BackupData global, mo-ui/src/backup.rs,
shown on Space place via app.slint space-place; rfd 0.17 xdg-portal folder picker; walks only once Space shown).
#99 merged c47e02a (2026-11-01) = song panel "backed up" line (BackedUp in data_dir/backup-songs.txt, run() returns Ran{last,went}, song_id = min save id). #113 merged 27ba418 (2026-11-01, Solomon "do that now"; backup feature complete) = big changes as Essentials milestones from library counts (track_count/plugin/sample/length_bars*4; SaveShape.tracks now Option; errored saves skipped).
Solomon 2026-11-01 20:53: Backup gets its OWN page in the rail (not a Space section). #136 merged b5cd8f7 = BackupPage (no Space tabs), still at space-place; screens thread adds the Backup place, rail name+icon, and moves backup::shown() + the app-opened check.
