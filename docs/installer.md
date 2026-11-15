# The Windows installer

The plan, in plain words, is the "Windows installer plan" doc (15 Nov 2026). This page is the short version of how it's
built.

## What's where

| File | What it does |
| --- | --- |
| `product.toml` | The name (DAWmate), description, publisher, website, logo files and the installer's fixed id. |
| `installer/setup.iss` | The Inno Setup script: files, shortcuts, the "Where should your data go?" page, uninstall. |
| `installer/build.ps1` | Reads `product.toml` and `Cargo.toml`, runs Inno Setup, writes `dist/<Name>-Setup-<version>.exe`. |
| `installer/check.ps1` | Tries a setup on a CI machine: new PC, picked data folder, PC with data, update, uninstall. |
| `crates/mo-app/src/install_choice.rs` | The app's side of the data folder page. |

## Decisions

- The app goes in the user's own Programs folder (`%LOCALAPPDATA%\Programs\DAWmate`), with no Windows Yes for the install
  or any update (Solomon, 2026-11-15).
- The setup asks where the data folder goes (Solomon, 2026-11-15). On a new PC it starts the folder there
  (`started-here.txt` in it, and `MusicOrganiser-folder.txt` names it). On a PC with data, it leaves the data where it
  is. If another place is picked, it writes `MusicOrganiser-move-asked.txt`, and the app does its usual checked move
  once it has started. The setup never copies, moves or deletes data.
- The installer writes only its own files. The app installs the DAW links itself (Settings › DAWs) from the copy of the
  Bitwig extension beside it (`extensions\`), and it sets the helper's startup line too. Uninstall removes that line
  only when it points at the copy being removed.
- The CLAP plugin joins in 1.1.

## Building one by hand (on Windows)

```
cargo build --profile dist --bin music-organiser
installer\build.ps1 -Exe target\dist\music-organiser.exe -BitwigExtension extensions\bitwig-java\target\MusicOrganiser.bwextension
```

Never run `installer\check.ps1` on your own PC. It puts your data folder aside while it works, and it's meant only for
CI.
