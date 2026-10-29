# Getting started on your PC

What to install once, and how to run the Rust app while it grows.

## Install

1. **Rust:** download `rustup-init.exe` from <https://rustup.rs> and run it with the default choices. It uses the
   Visual Studio C++ build tools you already have.
2. **VS Code** with two free extensions: **rust-analyzer** (Rust) and **Slint** (live screen preview).
3. **cargo-watch**, for rebuilding on every change: `cargo install cargo-watch`.

## Run it

From the repo folder:

| Command | What it does |
|---|---|
| `cargo run` | Builds and opens the app. The first build takes a few minutes; after that, a few seconds. |
| `cargo watch -x run` | Rebuilds and reopens the app every time you edit the code yourself. |
| `cargo run --bin mo-screens` | Draws every page into `screenshots\` without opening the app. |
| `cargo test` | Runs every test. |

## The launcher: test a pull request without typing anything

`tools\dev\Run latest.cmd` does what the C# app's launcher does, for the Rust app. Double-click it once (from a
download of this repo, or GitHub's "Download raw file" on both files in `tools\dev`), and it puts **Music Organiser
Rust (latest)** on your Desktop. From then on:

1. Double-click the shortcut and pick a pull request, or main (Enter picks the last one).
2. It downloads that version into its own folder (`%LOCALAPPDATA%\MusicOrganiser\run-latest-rust`), builds it and
   opens the app. The first build takes a few minutes; after that, under a minute.
3. Leave its window open. Every 20 seconds it checks GitHub; when Claude pushes, it builds the new code while the app
   stays open, then swaps the app for the new build. A build that fails is shown in the window and the app keeps
   running as it was. An open F12 note is never closed: the swap waits until it's saved or cancelled.

It never touches the C# launcher or its folder, so both can be used side by side.

## See a screen change as it's written

Open any file in `crates/mo-ui/ui/` in VS Code and click **Show Preview** above the component (the Slint
extension adds it). The preview redraws as soon as the file changes, without building the app, so a page can be
adjusted while you watch.

## Screenshots on every pull request

Each PR's CI run has a **screenshots** download with a picture of every page, and of Songs in every theme. You can
look before you pull anything.
