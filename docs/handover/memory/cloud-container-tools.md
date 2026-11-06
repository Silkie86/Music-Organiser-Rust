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
Disk (2026-11-03): the session's disk allowance is tight; the main target/ reaches ~21 GB and galaxycheck's ~7 GB.
"No space left on device" mid-build: delete target/debug/incremental (and tools/galaxycheck/target/debug/incremental),
build with CARGO_INCREMENTAL=0, and delete old test executables (`find target/debug/deps -maxdepth 1 -type f
-perm -u+x ! -name '*.so' -mmin +40 -delete` freed ~9 GB; without `! -name '*.so'` it also takes the proc-macro
libraries, and the next build starts nearly from scratch). NEVER give a second worktree the main repo's CARGO_TARGET_DIR (found
2026-11-03, wrong advice before): cargo names the workspace crates' artifacts the same in both trees and keeps their
source paths relative, so whichever tree built last counts as fresh for the other (mo-app compiled against the
worktree's mo-space; tests silently ran the other tree's code). Each worktree gets its own target dir, removed after.
