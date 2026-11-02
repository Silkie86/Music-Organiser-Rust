---
name: port-csharp-workarounds
description: Solomon: much C# code (e.g. galaxy) only worked around C#'s limits; don't copy those parts into Rust
metadata:
  type: feedback
  modified: 2026-11-01T16:19:24.939Z
---

Solomon (2026-11-01, on the galaxy port): "a lot of code for the galaxy was to mitigate the fact it was a C# app".

Then (16:21): "use the tricks that made it as fast as possible, but only if they are applicable to Rust too". So keep speed tricks that work in any language (sampling for PCA, NN-descent, all cores); drop the .NET-only ones.

**Why:** the C# app sampled, capped and split work (PCA on 20,000 sounds, new sounds placed against 10,000, fingerprints made on demand, a non-repeatable parallel layout) to stay fast and small in .NET. Rust doesn't need most of that.

**How to apply:** when porting, check each limit or shortcut and say whether it's there because of C#. Keep twin-checkable behaviour where it matters (file formats, data), but for maps and analysis, prefer the plain Rust version and list the change in DIFFERENCES.md (twin tests are a guide, not a cage). This extends rule 24 in [[rust-rebuild-rules]] (no C# workarounds), which the galaxy port missed. See [[sounds-backend-status]].
