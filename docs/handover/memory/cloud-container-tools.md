---
name: cloud-container-tools
description: how to get .NET (to run the old C# app, twin tests, C# theory bench) and Python tools in the cloud container
metadata:
  type: reference
---

The cloud container can run the old C# app (found 2026-10-31): dotnet's own download host is blocked, but Ubuntu's
archive has it: `apt-get update && apt-get install -y dotnet-sdk-10.0` (10.0.114, Ubuntu 24.04). NuGet restore works
through the proxy. The C# app targets net10.0; build with `-p:MoBuildNative=false -p:MoBuildBitwig=false`.
So twin tests (tools/twin/cs), the C# TheoryBench (old repo tools/MusicOrganiser.TheoryBench) and the C# helper
tools/theorybench/cs can run locally, not only in CI.
Also: pip works (music21, mido installed fine); git clone of public GitHub repos works, but the GitHub API is limited
to the scoped repo. Run Python that reads downloaded data with `python3 -I`.
See [[harmony-engine-interfaces]].

Data hosts (2026-11-01): raw.githubusercontent, jsDelivr, zenodo, dropbox, huggingface are blocked; public GitHub
repos clone fine (add_repo first, then an anonymous `git clone --filter=blob:none --no-checkout` and
`git checkout HEAD -- <paths>`). The McGill Billboard charts (CC0) come from the boomerr1 mirror this way.
Gotcha: the code-review skill checks out a local `prNN` branch; switch back to the thread's branch before editing or
committing, or the fix lands on `prNN` and the push misses it.
