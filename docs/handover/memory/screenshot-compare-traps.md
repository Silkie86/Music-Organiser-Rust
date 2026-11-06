---
name: screenshot-compare-traps
description: how to compare mo-screens screenshots between commits so "the same to the pixel" is true (Pillow alpha-only bbox, Sounds shuffle seed, faketime ticks, folder path in Settings)
metadata:
  type: reference
  modified: 2026-11-04T01:14:03.372Z
---

Found 2026-11-04 (Draw arrangement and waveforms on GPU thread), checking "same screenshots before and after" claims:

- Pillow 12: `ImageChops.difference(a, b).getbbox()` on RGBA images looks at ALPHA ONLY by default, so colour
  changes read as "identical". Compare with numpy over all channels (`np.abs(a.astype(int) - b).max(axis=2)`), or
  `getbbox(alpha_only=False)`. Earlier "pixel-identical" claims made with getbbox were unverified.
- mo-screens shows the Sounds pages shuffled by `new_seed()` (mo-ui `sounds.rs`, SystemTime nanos), and the galaxy
  card shows a random pick, so two runs of the SAME build differ on every Sounds page. `faketime '2026-11-04
  12:00:00'` does NOT freeze the clock here: it starts there and ticks (nanos differ call to call). Hold the seed
  with a local, never committed edit: `sed -i 's/std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
  .map_or(1, |d| d.as_nanos() as u64)/0x5eed_u64/' crates/mo-ui/src/sounds.rs` (one line in the file), build, then
  put that one line back the same way (replace `0x5eed_u64` with the old text). Never `git checkout --` the file
  while it has other uncommitted edits: that throws them away (auto mode refuses it, 4 Nov).
- Rediscover and Kits draw with mo-sounds `fresh_seed()` (rediscover.rs, SystemTime subsec nanos) when their seed
  is 0, so sounds-rediscover and sounds-kits differ too unless that line is held the same way
  (`(nanos % i32::MAX as u32) as i32 | 1` -> `0x5eed`), which changes mo-sounds and rebuilds more.
- `GITHUB_SHA=0000000` holds the commit label; shoot into the SAME folder path every run (the Settings Developer
  tab prints the data folder), then rename it.
- Left over after that: the activity card's progress strip (3 px tall, bottom right of cards/finder/note/key-sheet
  shots) follows real time. Mask it.

See [[gpu-for-graphics]].
