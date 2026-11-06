---
name: solomon-pc-disks
description: Solomon's PC (DESKTOP-E4DMRLT): disks, free space, memory, CPU, and how fast sounds.db reads there; why cloud timings mislead
metadata:
  type: reference
  modified: 2026-11-03T19:36:29.169Z
---

Read-only check on his PC, 2026-11-03 19:32 (Speed up loading Sounds thread):

- CPU Ryzen 9 5950X, 16 cores / 32 threads (the low-priority pool has 32 threads). RAM 32 GB, but 88% in use
  (4 GB free) with the Rust app running; the C# app's logs show it holding 7-14 GB and waiting on paging (PageIn).
- **C: = "P3-1TB", a SATA SSD, 12 GB free of 1 TB (98.8% full).** `%LOCALAPPDATA%\MusicOrganiser` (sounds.db
  4.56 GB, library.db 0.92 GB, midi.db 0.89 GB, all with WALs) is there, and so is the page file. C: was the only busy
  disk.
- Fast drives with room: I: = WD_BLACK SN770 2 TB NVMe (155 GB free); G: = Samsung 990 PRO 2 TB NVMe (27 GB free).
  J: SanDisk SSD PLUS 2 TB SATA (314 GB free), K: Crucial BX500 2 TB SATA (354 GB free, Bitwig), L: Samsung 870 QVO
  (5 GB free, MIDI packs), N: Seagate 4 TB USB backup. Letters: C=0 E=1 L=2 F=3 K=4 D=5 J=6 I=7 G=8 N=9 O=10.
- sounds.db read times, one connection (old code): 802, 503, 833, 135, 253, 414 s cold; 10.3 and 10.9 s when cached.
  16 connections (#183's first build ddec0b0): 199.9 s, no gain, and Songs' library.db read (same disk) took 148.9 s
  (it varies 0.8-125 s anyway when it races the Sounds read). Parallel reads do NOT help on his C:; my 4-core cloud
  VM's fast disk (6-8 s cold) hid that. Never trust cloud timings for his disk: check his logs.
- The sample locator (`mo-app/src/sound_library.rs`, used by the arrangement preview) re-reads every sound's path and
  size from sounds.db when its copy is over 60 s old: about 1.9 GB of pages cold on a test copy like his, so the
  arrangement preview is slow on his C:.
- Freeing space on C: or moving the data folder to I: would speed up every database read: his call (not asked yet).

See [[small-database-files]], [[sounds-backend-status]].
