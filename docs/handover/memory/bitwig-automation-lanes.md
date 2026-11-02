---
name: bitwig-automation-lanes
description: How Bitwig 6.1.6 saves tempo/volume/pan/on-off automation lanes, and the converter's automation-gap progress
metadata:
  type: project
---
Learned from Solomon's live test (2026-11-01, Bitwig 6.1.6, "MO Live Test automation", recorded by the MO Test
Driver controller script, tools/formatlab/daw/bitwig). Part of [[fl-to-bitwig-converter-plan]].

- Tempo lane: class 0x23d on the project root, at 0x168 per-user state {user} > 0x4e6, also in root 0x2dc9 > 0x4ba.
  Target 0x2a3 class 0xf1 with 0x41 -> the TEMPO setting (class 0x4c1), 0x133b "Transport", 0x133a "Tempo".
  Points class 0x387 in list class 0x389 (0x2af beat, 0x28f real BPM, 0x359e=1, 0xba6 curve 0, 0x2ebe 0).
- Volume/pan: target class 0xf1, 0x41 -> mixer setting, "Mixer"/"Volume"|"Pan". Volume saved = API value x 2^(1/3)
  (cube-root gain); pan -1..1. Recorded moves give two points per move (old then new) on one beat.
- Device On/Off: target class 0x65 with 0xec device, 0xed "On", 0x133b device name, 0x133a "On/Off", 0xe92 -> class
  0xc6 {0x1293 "", 0x1b2d Bool 1}. Points class 0x7d {0x2af beat, 0x12e Bool} one per change, in list class 0xa4.
  Written by the converter since PR #91 (merged 2026-11-01). FL slot on/off: 0.5 and above is on.
- Master lane: tracks[0] > 0x897 > 0x89b (not confirmed by name).

Second run (04:35, driver 2 s steps): volume/pan scaling confirmed on 5 values; moves in the first ~4 s of playing
were lost and "write" mode also wrote other tracks' controls at stop -> PR #97 uses touch mode + 6 s lead-in,
version 1.2 (re-run not needed). Bitwig's audio engine access-violates on a normal quit (harmless).
Still to do: FL measurement of fader -> dB and tempo
0..1 -> BPM (FL link script MEASURE_IN_LIVE_TEST, needs Solomon to install the new FL script and make an empty FL
"MO Live Test automation" project in <FL projects folder>\MO Test); then tempo, master and insert
fader/pan lanes; curve shapes; FL edit events.
FL gotcha (2026-11-01, FL 2025, scripting v40): ui.getProgTitle() gives scripts only "FL Studio 2025", never the
project name. The FL script now also matches general.getProjectTitle() (File > Project info title) - PR #100. The link's
"title" field to the app has the same blind spot: the Rust DAW link must not rely on it.
FL MEASURED (2026-11-01 05:19, FL 2025 v40, insert 1 via mixer.setTrackVolume): tempo automation value v -> BPM =
10 + 512 v exactly. Insert fader v -> dB is no simple formula (0.01 -50.335, 0.1 -29.131, 0.5 -9.179, 0.8 0.0,
1.0 +5.59); full 101-point table kept in the converter code (plan.rs). Insert fader/pan lanes merged in PR #103
(2026-11-01): on each track the insert's channels feed, master insert on the master, a point every 0.05 fader travel.
FL event 235 = insert routes (byte per insert, master 0; FL25 trims zeros), read + bus paths in PR #128
(2026-11-01, checked on 744 of Solomon's projects); sends/sidechains not told apart, so lanes stop at a send. Tempo lane merged in PR #105 (transport 0x190 gets 0xfa3 list before 0x211; holds project tempo before
FL's first point; 20 BPM floor in the writer). Not yet checked live: master's new lanes list and the tempo lane. A new FL 2025 project has only 16 inserts.
FL edit events (event 223): 12-byte records u32 tick in pattern, u32 target (as link 227), f32 0..1; confirmed on
Solomon's sly_15 (LegacyCell Encoder1, FL 25.2.5, PR #130). Plugin-setting ones carried as placed automation; in 120/744 projects.
Bitwig automation clips (for edit events): see [[bitwig-automation-clips]].
