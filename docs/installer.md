# The Windows installer

The plan, in plain words, is the "Windows installer plan" doc (15 Nov 2026). This page is the short version of how it's
built.

## What's where

| File | What it does |
| --- | --- |
| `product.toml` | The name (DAWmate), description, publisher, website, logo files and the installer's fixed id. |
| `assets/logo/` | The logo: its SVG drawings and what `tools/logo` makes from them (the .ico, the Mac's .icns, and Setup's pictures `setup-side-*.bmp` and `setup-small-*.bmp`, one per screen scale). |
| `installer/setup.iss` | The Inno Setup script: files, shortcuts, the "Where should your data go?" page, uninstall. |
| `installer/build.ps1` | Reads `product.toml` and `Cargo.toml`, runs Inno Setup, writes `dist/<Name>-Setup-<version>.exe`. |
| `installer/check.ps1` | Tries a setup on a CI machine: new PC, picked data folder, PC with data, update, uninstall. |
| `.github/workflows/release.yml` | A version tag makes a draft release with the setup (below). |
| `crates/mo-app/src/install_choice.rs` | The app's side of the data folder page. |
| `crates/mo-app/src/update.rs`, `crates/mo-ui/src/update.rs` | Updates from inside the app (below). |

## Decisions

- The app goes in the user's own Programs folder (`%LOCALAPPDATA%\Programs\DAWmate`), with no Windows Yes for the install
  or any update (Solomon, 2026-11-15).
- The setup asks where the data folder goes (Solomon, 2026-11-15). On a new PC it starts the folder there
  (`started-here.txt` in it, and `MusicOrganiser-folder.txt` names it). On a PC with data, it leaves the data where it
  is. If another place is picked, it writes `MusicOrganiser-move-asked.txt`, and the app does its usual checked move
  once it has started. The setup never copies, moves or deletes data.
- The installer writes only its own files: the app, the Bitwig extension beside it (`extensions\`) and the CLAP plugin
  (`plugins\DAWmate.clap`). The app puts them into the DAWs itself (Solomon, 2026-11-25), a little after it starts
  (`mo_app::daw_links`): at its first start, the FL Studio script and the Bitwig extension go into the DAWs whose
  folders are there, and the plugin into `Common Files\CLAP`, where every DAW finds it (Solomon, 2026-11-25). Only
  that copy needs an administrator, so Windows asks Yes for it, once for each version (No: not asked again until the
  next version). After an update, the copies already in place are updated, unless a newer version put them in. One
  the DAW has open (Bitwig keeps its extension and plugins open) waits for a start when the DAW is closed. Uninstall
  leaves the plugin in Common Files\CLAP (taking it out would need an administrator too).
  Each link keeps which version put it in, only once every folder took it, so one that couldn't be written (a copy
  Bitwig had open) is tried again later, and then only the copies that aren't the app's own yet; a link or plugin you
  took out stays out. Settings › DAWs can still install or take out the links by hand (and then the look leaves them
  alone that run; Take out there counts as the first look, so the link isn't put back). The plugin has no row there
  yet. A launcher build never does this by itself, nor does a run whose settings can't be kept, and a test plugin put
  in by hand (`tools/dev/install-test-plugin.ps1`, which takes the app's own plugin out) keeps the app from putting
  the plugin in too; the app's own goes back in once it's gone (the script's `-Remove` puts it back itself). The
  plugin now shows in DAWs as "DAWmate" (its ID is unchanged, so songs that have it still load it).
- The app also sets the helper's startup line. Uninstall removes that line only when it points at the copy being
  removed.

## Updates

Only a copy the setup put in (its `unins000.exe` is beside it) updates itself; the launcher's builds never do, and while
the installed copy is open the launcher's window keeps a new build waiting until it is closed, rather than opening it in
its place (starting the launcher by hand still does). Three minutes after every start, then once a day while it stays
open, it asks GitHub's Releases page (through Windows' own `curl.exe`) whether a newer version is out. That is all it
sends online, and Settings › General › Updates turns it off (a fetch going stops, and the fetched setups go). curl.exe
doesn't use a proxy set only in Windows' settings (only the `HTTPS_PROXY` or `ALL_PROXY` environment variables), so
behind such a proxy the look fails, and the log has curl's error. A fetch whose connection dropped is tried again
within the hour, twice at most; any other failure waits for the next day's look. A newer setup is fetched into the
data folder's `updates` folder, checked against the `.sha256` beside it on the release, and the rail then offers
"Restart to update": the click checks the setup again and starts it, and the app closes as it always does. The setup
waits for the app to end (`/WAITPID`), puts the new version in without pages and opens it (`/UPDATED=1`); if a file
can't be replaced it says so with Retry, and if it stops before the files go in it opens the old version again. Its
log goes beside it (`<setup>.log`), and when the next start is still the old version the app shows a card saying the
update didn't go in, with the log's last lines in the app's log. The first update, from 0.9.0-beta.1, has none of this
yet: beta.1's own code starts the setup, with no `/WAITPID` (the setup waits five seconds instead) and no log beside
it, so a file in use stops it with no Retry box, beta.1 opens again with no card, and the setup's log is the newest
`Setup Log *.txt` in `%TEMP%`. So the path above is first tried on the update after that one. Uninstalling asks the
app and the helper to close first (`music-organiser.exe --close-for-setup`). A test version (with a dash) is offered only to a copy that is a test version too. Old setups are
removed at the next look, and so is a setup GitHub no longer lists (a release taken back is never offered).

## Making a release

1. Raise `version` in `Cargo.toml`'s `[workspace.package]` (a PR like any other). A version with a dash, like
   `0.9.0-beta.1`, is a test release.
2. Wait until main's own CI run for that commit is green (Actions, CI, the run for the merge). The release stops at
   its first step if it isn't, as a PR's run only tests what the PR changed. Then on GitHub: Actions, Release, Run
   workflow. Leave the branch on `main`, tick **Make the draft release**, and run it.
   (Pushing a tag such as `v0.9.0-beta.1` on `main` does the same.)
3. The Release workflow builds the Bitwig extension, the app and the CLAP plugin (`dist` profile, about 25 minutes). It then makes the
   setup, tries it with `installer/check.ps1`, and puts a **draft** release on the Releases page, tagged `v` plus the
   version. The draft has the setup and its SHA-256. A ticked run that isn't on `main`, a tag that doesn't match the
   version, or a version that's already published stops before anything is built: raise the version first.
4. Read the draft over and publish it. Nobody sees it until then. Publishing makes the tag, which starts the workflow
   once more; that run sees the release is out and stops at once.

To try the workflow without releasing anything: Run workflow without the tick, from any branch. It does everything
except the release, and keeps the setup as a download.

## Building one by hand (on Windows)

```
cargo build --profile dist --bin music-organiser
cargo build --profile dist -p mo-plugin
installer\build.ps1 -Exe target\dist\music-organiser.exe -BitwigExtension extensions\bitwig-java\target\DAWmate.bwextension -ClapPlugin target\dist\mo_plugin.dll
```

Never run `installer\check.ps1` on your own PC. It puts your data folder aside while it works, and it's meant only for
CI.

## The Mac download (a test build, not signed yet)

The Release workflow's two **Mac build** jobs (on every run, try-outs too) build the app and the CLAP plugin for Apple
silicon and for Intel side by side (about an hour and a half each from cold); the **Mac app** job joins them and makes `DAWmate-<version>-mac.zip` (`installer/mac/package.sh`): `DAWmate.app` and a Read me.
It's kept as the run's `mac` download, not put on the release.

- **Inside the app** (as a Mac app keeps what it brings): the CLAP plugin's bundle in `Contents/PlugIns/DAWmate.clap`
  and the Bitwig extension in `Contents/Resources/extensions` (`mo_dawlink::scripts::mac_app_contents`). Run from its
  bundle, the app puts its DAW links in by itself half a minute after it starts, as the installed app does on Windows:
  the plugin goes whole into `~/Library/Audio/Plug-Ins/CLAP` (the user's own, no administrator; copied beside itself,
  checked, then swapped in), the extension and FL script where they go on Windows. Self-update stays Windows only.
- **Signed ad hoc only.** Apple silicon runs nothing unsigned, so the bundles carry a signature with no name. Without an
  Apple Developer ID (99 USD a year) and notarisation, macOS refuses a double-click on the first open: on macOS 15 or
  later, System Settings, Privacy & Security, Open Anyway; on 14 or earlier, right-click, Open, Open (the Read me says
  both). Solomon decided on 27 Nov 2026 not to buy a Developer ID for now: Mac testers take the run's `mac` download.
- **Tried on GitHub's Mac** by `installer/mac/check.sh`: the bundles are whole (Info.plist, both chips, signatures,
  `clap_entry`, the extension), and the app starts with a scratch home folder, runs a minute, writes no crash log, puts
  its plugin in, and closes when asked. Then `clap-validator` (the CLAP project's checker) loads the plugin as a DAW
  would.
- `Info.plist` says why the app may use the sound card's input (recording a take; without it macOS closes an app the
  moment it opens one) and why it asks Finder for things (sending to the Trash, so Finder's Put Back works).
