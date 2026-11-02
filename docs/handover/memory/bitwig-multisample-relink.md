---
name: bitwig-multisample-relink
description: mo_convert::relink fix_bitwig_project for Bitwig multisample copies, what's verified, who owns the bin step
metadata:
  type: project
---
Merged PR #69 (2026-10-31), still_named in PR #75. Part of [[fl-to-bitwig-converter-plan]].
mo_convert::relink::fix_bitwig_project(project, &[Relink{copy, library}]) writes "<name> (fixed).bwproject" (never over a file). Fixed.changed = copies safe for the bin (byte-identical, unnamed in fixed project incl. zip, unnamed by any other Bitwig file in the folder tree incl. auto-backups). The Clean and Storage thread calls it and owns the bin step (only after Solomon checks the fixed project plays). Pass ALL of a project's copies in one call. Verified on real GARAGE FOR BW 20: refs live in class 0x4a6 (fields 0x512 bare name, 0xd3a "multi-samples/…") and class 0x2a field 0x3b (old Library path, absolute I:\ project path, and "copied from" paths in other projects/templates), plus meta used_files. Not yet confirmed: that Bitwig opens the fixed project and plays (Solomon's check).
