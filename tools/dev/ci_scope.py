"""Works out what a pull request's CI run needs to check (Solomon, 2026-10-31: only build the parts that change).

Reads the files the PR changes (one per line, on stdin) and writes GitHub Actions outputs:

- full:      "true" when everything must be checked: a change to the workspace itself (Cargo.toml, Cargo.lock,
             deny.toml, the CI files, rust-toolchain) or to a file outside any one crate.
- test:      "-p a -p b": the crates that changed. Their tests run on Windows and Mac.
- check:     "-p a -p b": those crates and every crate that uses them, however far up. Clippy checks them all, and
             Linux runs all their tests, so a change that breaks a crate further up still fails the PR.
- ui:        "true" when the screens could look different (mo-ui, mo-app or what they use): the screenshots run.
- formats:   "true" when what the readers find could change (mo-formats, mo-library, the sound analyser in mo-audio
             and mo-sounds, the twin tool, or a crate they use, such as mo-catch): the twin test runs.
- galaxy:    "true" when the galaxy check (a workspace of its own), mo-app or what mo-app uses changed.
- launcher:  "true" when the launcher's scripts (tools/dev) changed: every .ps1 there is parsed as Windows
             PowerShell 5.1 reads it.
- installer: "true" when the Windows installer (installer/), the name and logo it takes (product.toml, assets/) or the
             app's own code the setup runs (SETUP_RUNS) changed: the setup .exe is built and tried.
- rust:      "true" when any crate is to be checked at all (a PR of only notes checks none).

Pushes to main and runs started by hand (before an ultracode review) skip this and check everything.
Plain Python 3 with nothing to install, so it runs the same on any runner.
"""

import json
import os
import subprocess
import sys

# A change to any of these can change every crate.
WHOLE = ("Cargo.toml", "Cargo.lock", "deny.toml", "rust-toolchain", "rust-toolchain.toml", ".cargo/", ".github/",
         "clippy.toml", "rustfmt.toml", "tools/dev/ci_scope.py")
# Changes that never change what's built: notes, docs and the launcher's own scripts (checked on their own).
NOTHING = ("docs/", "README", "CLAUDE.md", "screenshots/", "tools/dev/", ".gitignore", ".gitattributes")
# Files outside a crate's folder that the crate's tests read, and that crate: a change to one of them tests it too
# (the 26 Nov review's IN-1). mo-ui checks that the launcher's scripts are plain ASCII and that the galaxy check's lock
# still takes the patched drawing library; mo-dawlink checks its Bitwig ids against the format registry; mo-app's drag
# card tests write with mo-ui's fonts.
READ_BY = (
    ("tools/dev/", "mo-ui"),
    ("tools/galaxycheck/Cargo.lock", "mo-ui"),
    ("tools/formatlab/registry/", "mo-dawlink"),
    ("crates/mo-ui/fonts/", "mo-app"),
)
# The app's own code the setup runs (the uninstall asks the app and the helper to close with --close-for-setup): a
# change to it tries the setup too.
SETUP_RUNS = ("crates/mo-ui/src/main.rs", "crates/mo-app/src/install_choice.rs")
# The crates whose findings the twin test compares with the C# app's.
READERS = {"mo-formats", "mo-library", "mo-audio", "mo-sounds", "mo-twin"}


def packages():
    """Each crate in the workspace: its folder and the crates it uses from the workspace."""
    out = subprocess.run(["cargo", "metadata", "--no-deps", "--format-version", "1"], check=True,
                         capture_output=True, text=True).stdout
    meta = json.loads(out)
    root = meta["workspace_root"].replace("\\", "/").rstrip("/") + "/"
    crates = {}
    for p in meta["packages"]:
        folder = os.path.dirname(p["manifest_path"].replace("\\", "/")).removeprefix(root) + "/"
        crates[p["name"]] = (folder, {d["name"] for d in p["dependencies"] if d.get("path")})
    return crates


def scope(changed, crates, full=False):
    """What to check for these changed files (paths from the repo's root, with forward slashes); `full` checks all."""
    result = {"full": full, "test": set(), "ui": False, "formats": False, "galaxy": False, "launcher": False,
              "installer": False}
    read_by = set()
    for path in changed:
        read_by.update(reader for prefix, reader in READ_BY if path.startswith(prefix))
        if path in SETUP_RUNS:
            result["installer"] = True
        if path.startswith("tools/galaxycheck/"):
            result["galaxy"] = True
            continue
        if path.startswith(("installer/", "assets/")) or path == "product.toml":
            result["installer"] = True
            if path.startswith("installer/"):
                continue
        if path.startswith("tools/dev/"):
            result["launcher"] = True
        if path.startswith(WHOLE) or path in WHOLE:
            result["full"] = True
            continue
        owner = next((name for name, (folder, _) in crates.items() if path.startswith(folder)), None)
        if owner:
            result["test"].add(owner)
        elif not path.startswith(NOTHING):
            # Somewhere no rule covers: play safe.
            result["full"] = True

    # Every crate that uses a changed one, however far up.
    check = set(result["test"])
    grew = True
    while grew:
        grew = False
        for name, (_, uses) in crates.items():
            if name not in check and uses & check:
                check.add(name)
                grew = True

    result["ui"] = bool(check & {"mo-ui", "mo-app"})
    # From every crate checked, not only the ones changed: a change to a crate the readers use (mo-catch) can change
    # what they find too.
    result["formats"] = bool(check & READERS)
    result["galaxy"] = result["galaxy"] or "mo-app" in check
    # A crate that only reads a changed file runs its tests; its code didn't change, so nothing that uses it needs to.
    result["test"] |= read_by
    result["check"] = check | read_by
    if result["full"]:
        result.update(test=set(crates), check=set(crates), ui=True, formats=True, galaxy=True, launcher=True,
                      installer=True)
    return result


def flags(names):
    return " ".join(f"-p {n}" for n in sorted(names))


def main():
    # --full: a push to main or a run started by hand checks everything, whatever changed.
    full = "--full" in sys.argv
    changed = [] if full else [line.strip().replace("\\", "/") for line in sys.stdin if line.strip()]
    s = scope(changed, packages(), full)
    lines = [
        f"full={str(s['full']).lower()}",
        f"test={flags(s['test'])}",
        f"check={flags(s['check'])}",
        f"ui={str(s['ui']).lower()}",
        f"formats={str(s['formats']).lower()}",
        f"galaxy={str(s['galaxy']).lower()}",
        f"launcher={str(s['launcher']).lower()}",
        f"installer={str(s['installer']).lower()}",
        f"rust={str(bool(s['check'])).lower()}",
    ]
    print("\n".join(lines))
    if "GITHUB_OUTPUT" in os.environ:
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
