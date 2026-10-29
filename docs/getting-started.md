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
| `cargo watch -x run` | Rebuilds and reopens the app every time the code changes (a pulled branch, or an edit). |
| `cargo run --bin mo-screens` | Draws every page into `screenshots\` without opening the app. |
| `cargo test` | Runs every test. |

## See a screen change as it's written

Open any file in `crates/mo-ui/ui/` in VS Code and click **Show Preview** above the component (the Slint
extension adds it). The preview redraws as soon as the file changes, without building the app, so a page can be
adjusted while you watch.

## Screenshots on every pull request

Each PR's CI run has a **screenshots** download with a picture of every page, and of Songs in every theme. You can
look before you pull anything.
