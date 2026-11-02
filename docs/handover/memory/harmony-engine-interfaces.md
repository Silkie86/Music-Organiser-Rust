---
name: harmony-engine-interfaces
description: what other threads take from mo-harmony (MIDI index cache by note hash + harmony_version) and how it's measured (theorybench, C# baseline)
metadata:
  type: project
---

Agreed 2026-10-31 (harmony thread to "Build the MIDI page backend", asked by the coordinator):
- Key and chords come ONLY from mo-harmony. No port of the C# EstimateMidiKey/first-chord rule or ChordReader anywhere.
- The MIDI index (midi-index.db) STORES mo-harmony's answers as a cache keyed by note hash + harmony_version
  (mo-harmony exports a version constant, bumped when what it finds changes, rule 7); older rows are redone in the
  background at low priority. Compact facts only: key tonic 0-11/null, mode major/minor/null ("mode unclear", "no
  clear key"), calibrated confidence 0-1, chords as a small blob (start, length, root, interval bits, bass). Names,
  Roman numerals, Camelot are made by mo-harmony when shown. Chroma not stored unless needed for similarity (then
  from mo-harmony too). Until the key finder lands the columns stay empty, never C# results.
- Speed measured in the cloud container: old key finder 1-4 microseconds a part; old chord reader about 3 ms per
  whole POP909 song. New engine target: under 1 ms for an 8-16 bar part. Live for the part on screen, stored for search.
- Still owed to the MIDI thread: function names + version constant when the key finder PR is up.

Measuring: tools/theorybench (Rust) reads JSON-lines labelled music made by tools/theorybench/convert/*.py; the
real C# engines run through tools/theorybench/cs (built against the old repo clone). Data stays outside the repo
(*.jsonl ignored). See [[harmony-engine-plan]], [[cloud-container-tools]].
