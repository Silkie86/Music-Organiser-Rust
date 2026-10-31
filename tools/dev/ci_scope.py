"""Works out what a pull request's CI run needs to check (Solomon, 2026-10-31: only build the parts that change).

Reads the files the PR changes (one per line, on stdin) and writes GitHub Actions outputs:

- full:      "true" when everything must be checked: a change to the workspace itself (Cargo.toml, Cargo.lock,
             deny.toml, the CI files, rust-toolchain) or to a file outside any one crate.
- test:      "-p a -p b": the crates that changed. Their tests run.
- check:     "-p a -p b": those crates and every crate that uses them. Clippy checks them, so a change that breaks
             a crate further up still fails the PR, without building and running that crate's tests.
- ui:        "true" when the screens could look different (mo-ui or mo-app changed): the screenshots job runs.
- formats:   "true" when what the readers find could change (mo-formats, mo-library or the twin tool): the twin
             test runs.
- galaxy:    "true" when the galaxy check (a workspace of its own) or mo-app changed.
- launcher:  "true" when the PowerShell launcher scripts changed.
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


def scope(changed, crates):
    """What to check for these changed files (paths from the repo's root, with forward slashes)."""
    result = {"full": False, "test": set(), "ui": False, "formats": False, "galaxy": False, "launcher": False}
    for path in changed:
        if path.startswith("tools/galaxycheck/"):
            result["galaxy"] = True
            continue
        if path.startswith("tools/dev/") and path.endswith(".ps1"):
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

    result["ui"] = bool(result["test"] & {"mo-ui", "mo-app"})
    result["formats"] = bool(result["test"] & {"mo-formats", "mo-library", "mo-twin"})
    result["galaxy"] = result["galaxy"] or "mo-app" in result["test"]
    result["check"] = check
    if result["full"]:
        result.update(test=set(crates), check=set(crates), ui=True, formats=True, galaxy=True, launcher=True)
    return result


def flags(names):
    return " ".join(f"-p {n}" for n in sorted(names))


def main():
    # --full: a push to main or a run started by hand checks everything, whatever changed.
    changed = [] if "--full" in sys.argv else [line.strip().replace("\\", "/") for line in sys.stdin if line.strip()]
    s = scope([".github/"] if "--full" in sys.argv else changed, packages())
    lines = [
        f"full={str(s['full']).lower()}",
        f"test={flags(s['test'])}",
        f"check={flags(s['check'])}",
        f"ui={str(s['ui']).lower()}",
        f"formats={str(s['formats']).lower()}",
        f"galaxy={str(s['galaxy']).lower()}",
        f"launcher={str(s['launcher']).lower()}",
        f"rust={str(bool(s['check'])).lower()}",
    ]
    print("\n".join(lines))
    if "GITHUB_OUTPUT" in os.environ:
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
