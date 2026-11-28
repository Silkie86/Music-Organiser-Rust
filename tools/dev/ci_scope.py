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
With --check it runs its own checks and asks nothing of the repo: CI does that first.
Plain Python 3 with nothing to install, so it runs the same on any runner.
"""

import json
import os
import posixpath
import re
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
# What a crate's screens take in from another crate's folder is read by its build too. That isn't listed here:
# slint_reads works it out from the screens themselves.
#
# How a .slint file takes a file in: `import { A } from "a.slint";`, `export * from "a.slint";`, a font with
# `import "a.ttf";` and a picture with `@image-url("a.png")`. Each is looked for from the folder of the file that
# names it.
TAKES_IN = re.compile(r'\b(?:from|import)\s+"([^"\n]+)"\s*;|@image-url\(\s*"([^"\n]+)"')
# The app's own code the setup runs (the uninstall asks the app and the helper to close with --close-for-setup): a
# change to it tries the setup too.
SETUP_RUNS = ("crates/mo-ui/src/main.rs", "crates/mo-app/src/install_choice.rs")
# The crates whose findings the twin test compares with the C# app's.
READERS = {"mo-formats", "mo-library", "mo-audio", "mo-sounds", "mo-twin"}


def packages():
    """The workspace's folder, and each crate in it: its folder and the crates it uses from the workspace."""
    out = subprocess.run(["cargo", "metadata", "--no-deps", "--format-version", "1"], check=True,
                         capture_output=True, text=True).stdout
    meta = json.loads(out)
    root = meta["workspace_root"].replace("\\", "/").rstrip("/") + "/"
    crates = {}
    for p in meta["packages"]:
        folder = os.path.dirname(p["manifest_path"].replace("\\", "/")).removeprefix(root) + "/"
        crates[p["name"]] = (folder, {d["name"] for d in p["dependencies"] if d.get("path")})
    return root, crates


def slint_files(root):
    """Every .slint file git keeps, from the repo's root."""
    out = subprocess.run(["git", "-C", root, "ls-files", "-z", "--", "*.slint"], check=True, capture_output=True,
                         text=True).stdout
    return [f for f in out.split("\0") if f]


def text_in(root):
    """A reader of the repo's files: it gives a file's text, or None when the file isn't there."""
    def text_of(file):
        try:
            with open(os.path.join(root, file), encoding="utf-8", errors="replace") as f:
                return f.read()
        except OSError:
            return None
    return text_of


def slint_reads(crates, files, text_of):
    """What each crate's screens take in from outside the crate's own folder: (file, crate) pairs, like READ_BY's.

    Slint builds a crate's screens together with every file they import, however far along, and Cargo knows nothing
    of it. The plugin's panel imports some of mo-ui's screens and fonts, and the plugin doesn't use mo-ui: a pull
    request that changed one of those files didn't check the plugin, so a break in its panel would first have shown
    on main (noticed 28 Nov, with #813). Worked out from the files, so the list keeps itself: `files` is every
    .slint file in the repo, `text_of` gives one's text. A file that is gone is still named by what imports it, so
    taking a file away checks the crates that read it as well.
    """
    reads = set()
    for name, (folder, _) in crates.items():
        seen = set()
        todo = [f for f in files if f.startswith(folder)]
        while todo:
            file = todo.pop()
            if file in seen:
                continue
            seen.add(file)
            if not file.startswith(folder):
                reads.add((file, name))
            text = text_of(file) if file.endswith(".slint") else None
            for imported, picture in TAKES_IN.findall(text or ""):
                target = imported or picture
                # What Slint brings itself and a library (mo-ui's "@product") are no files of the repo's.
                if target != "std-widgets.slint" and not target.startswith("@"):
                    todo.append(posixpath.normpath(posixpath.join(posixpath.dirname(file), target)))
    return reads


def scope(changed, crates, full=False, reads=READ_BY):
    """What to check for these changed files (paths from the repo's root, with forward slashes); `full` checks all.

    `reads` is who reads which files outside their own folder: READ_BY, and in a real run what slint_reads finds.
    """
    result = {"full": full, "test": set(), "ui": False, "formats": False, "galaxy": False, "launcher": False,
              "installer": False, "logo": False}
    read_by = set()
    for path in changed:
        read_by.update(reader for prefix, reader in reads if path.startswith(prefix))
        if path in SETUP_RUNS:
            result["installer"] = True
        if path.startswith("tools/galaxycheck/"):
            result["galaxy"] = True
            continue
        # The tool that makes the logo's pictures: a workspace of its own, run by hand, checked on its own.
        if path.startswith("tools/logo/"):
            result["logo"] = True
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
                      installer=True, logo=True)
    return result


def flags(names):
    return " ".join(f"-p {n}" for n in sorted(names))


def own_checks():
    """The script's own checks, on made-up crates: CI runs them before it asks what changed (`--check`), so a slip
    in here stops the run instead of quietly checking less."""
    crates = {"app": ("crates/app/", set()), "panel": ("crates/panel/", {"low"}), "low": ("crates/low/", set()),
              "tool": ("tools/tool/", {"panel"})}
    files = {
        "crates/app/ui/app.slint": 'import { Style } from "style.slint";\nimport { Other } from "pages/other.slint";',
        "crates/app/ui/style.slint": """import { Button } from "std-widgets.slint";
            import { Product } from "@product";
            import "../fonts/Sans.ttf";
            export * from "pages/page.slint";""",
        "crates/app/ui/pages/page.slint": """export component Page {
                in property <string> t: "from" + ", " + "far";
                Image { source: @image-url("../img/logo.png"); }
            }""",
        "crates/app/ui/pages/other.slint": "export component Other {}",
        "crates/panel/ui/panel.slint": """import {
                Style,
                Page } from "../../app/ui/style.slint";
            import { Gone } from "../../app/ui/gone.slint";
            import { Own } from "own.slint";""",
        "crates/panel/ui/own.slint": "export component Own {}",
    }

    def same(what, got, want):
        # Not `assert`: Python leaves those out when it is asked to run fast (-O).
        if got != want:
            sys.exit(f"ci_scope.py, its own checks: {what}: got {got}, wanted {want}")

    reads = slint_reads(crates, sorted(files), files.get)
    # Followed through style.slint to the page, its picture and the font, but not to what Slint brings or to a
    # library, and not into a string that says "from"; own.slint is the panel's own; nothing for the app, whose
    # screens stay in its folder; and gone.slint is named although it isn't there.
    same("what the screens take in", sorted(reads), sorted((f, "panel") for f in (
        "crates/app/ui/style.slint", "crates/app/ui/pages/page.slint", "crates/app/ui/img/logo.png",
        "crates/app/fonts/Sans.ttf", "crates/app/ui/gone.slint")))

    def asks(path, test, check):
        s = scope([path], crates, reads=tuple(reads))
        same(f"a change to {path}", (sorted(s["test"]), sorted(s["check"])), (test, check))

    # A file the panel takes in tests the panel too, but not what uses the panel: the panel's own code is as it was.
    asks("crates/app/ui/pages/page.slint", ["app", "panel"], ["app", "panel"])
    asks("crates/app/fonts/Sans.ttf", ["app", "panel"], ["app", "panel"])
    asks("crates/app/ui/img/logo.png", ["app", "panel"], ["app", "panel"])
    asks("crates/app/ui/gone.slint", ["app", "panel"], ["app", "panel"])
    # A screen the panel doesn't take in checks the app alone, as before.
    asks("crates/app/ui/pages/other.slint", ["app"], ["app"])
    asks("crates/app/ui/app.slint", ["app"], ["app"])
    # A change to a crate's code still checks all that use it, however far up.
    asks("crates/low/src/lib.rs", ["low"], ["low", "panel", "tool"])
    asks("crates/panel/ui/panel.slint", ["panel"], ["panel", "tool"])
    print("ci_scope.py: its own checks pass")


def main():
    if "--check" in sys.argv:
        own_checks()
        return
    # --full: a push to main or a run started by hand checks everything, whatever changed.
    full = "--full" in sys.argv
    changed = [] if full else [line.strip().replace("\\", "/") for line in sys.stdin if line.strip()]
    root, crates = packages()
    # A full run checks every crate whatever it reads, so it needs no list.
    reads = READ_BY if full else READ_BY + tuple(sorted(slint_reads(crates, slint_files(root), text_in(root))))
    for path in changed:
        for reader in sorted({reader for prefix, reader in reads if path.startswith(prefix)}):
            print(f"{reader} reads {path}")
    s = scope(changed, crates, full, reads)
    lines = [
        f"full={str(s['full']).lower()}",
        f"test={flags(s['test'])}",
        f"check={flags(s['check'])}",
        f"ui={str(s['ui']).lower()}",
        f"formats={str(s['formats']).lower()}",
        f"galaxy={str(s['galaxy']).lower()}",
        f"launcher={str(s['launcher']).lower()}",
        f"installer={str(s['installer']).lower()}",
        f"logo={str(s['logo']).lower()}",
        f"rust={str(bool(s['check'])).lower()}",
    ]
    print("\n".join(lines))
    if "GITHUB_OUTPUT" in os.environ:
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
