---
name: presets-page
description: Presets page backend (crate mo-presets): what's built, open questions, next PRs
metadata:
  type: project
  modified: 2026-10-31T17:05:28.224Z
---

2026-10-31: Presets page backend lives in its own crate `crates/mo-presets` (files, role, list, filter, library,
fixes, octave, export, names, bitwig; NOTES.md lists what's left). Merged: #36 (headers, part guess, song lists,
filters), #44 (library-wide list + presets.db), #49 (Serum 1 octave + export), #53 (Send to Bitwig, .bwpreset
from a template Bitwig saved, via mo_formats::bitwig::encode). Force-push is blocked: after a squash merge use
cherry-pick + ours-merge.
presets.db (agreed with readers/Songs thread 2026-10-31): Rust-only DB beside library.db, table
your_preset_roles(hash PK, role), like C# midi.db your_roles. Fix key = SHA-256 of plugin name + settings bytes +
notes it plays (effects: notes feeding the insert), so clones/default effects keep their own fix; fixed role kept
beside the guess. Solomon confirmed this key 2026-10-31 ("Same sound, same notes").
Open: .bwpreset meta keys and Send-to-Bitwig templates unchecked on Solomon's files; Thresholds need Solomon's MIDI
counts (rule 5); Bitwig reader doesn't give kinds or Note Transpose. fxp_to_bank and name_key now live in mo-formats (PR #54).
Synth octave: Solomon 2026-11-01 said leave it ("too complex for a small gain"): no per-plugin readers beyond Serum 1, no plugin host. A Serum 2 draft sits on local branch presets-serum2 (not pushed). Export clears its own .mo-part-* leftovers (#82). Backend work done; next is the page UI (another thread).
See [[rust-rebuild-status]], [[rust-app-design-decisions]].
