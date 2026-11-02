---
name: external-format-references
description: Other GitHub FL/Bitwig/DAWproject readers checked for format facts (licences, notes files, what to verify)
metadata:
  type: reference
---

Rule (Solomon 2026-10-31): never reuse their code; note FLP/Bitwig facts we can glean, check each on Solomon's saves.

LYNX SHARE (github Ka1-S/Lynx-Share, checked 2026-10-31): C++/JUCE FL-clone DAW, AGPL-3.0, reference only like PyFLP
(never copy code; facts partly from decompiling FL). src/flp/Flp.cpp has Layer children (0x5E, flags 0x90),
arrangements (0x63, name 0xF1), playlist record size from flag 0xA9 (60/80/88). Use when closing the FL "Not yet" items.
Full fact list: /mnt/project-files/notes/fl-facts-from-lynx-share.md (to check: stretch Time i32 vs f32 by 0xA9 bit 0).
ProjectConverter (git-moss, LGPL-3.0, checked 2026-10-31): Reaper <-> DAWproject only, no FL/Bitwig/S1 reading.
For the phase 5 DAWproject export, follow bitwig/dawproject (MIT, schema). Notes: /mnt/project-files/notes/projectconverter-notes.md
GitHub reader survey 2026-10-31: /mnt/project-files/notes/github-daw-readers-survey.md. Key: Meowrium/PyFLP says event
172 is 4 bytes in FL24/25 and varint in FL26 (clashes with rule 9: formatlab census needed); jdormit/bwpreset (MIT)
maps Bitwig Grid patches (classes, cables as text paths) for the phase 5 Grid plan.

See [[rust-app-design-decisions]], [[fl-to-bitwig-converter-plan]].
