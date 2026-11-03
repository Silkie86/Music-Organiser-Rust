# femtovg 0.27.0, patched for Music Organiser

This is the drawing library Slint uses to draw the window on the graphics card, copied from crates.io (femtovg 0.27.0,
MIT or Apache-2.0, see `LICENSE-MIT` and `LICENSE-APACHE`) with one change. The workspace's `Cargo.toml` points
`femtovg` here with `[patch.crates-io]`.

Only what the app builds is kept: `src`, the licences and the readme. The manifest lost the upstream examples and tests
(their files aren't in the crate) and its release profile (Cargo ignores profiles outside the workspace's root).

## The change

`src/renderer/wgpu.rs`: the cache of pipelines (the graphics card's ready-made drawing steps: one for flat colour, one
for text, one for pictures, and so on) is cleaned every 64 renders. Upstream cleaned it after every render, dropping
every pipeline that render didn't use.

Slint renders twice each frame when the app has a rendering notifier (the app has one: it shares the graphics card
with the galaxy and the arrangement, and times each frame for the `[Frames]` log lines). The first render only clears
the window, so it used no pipeline and the clean dropped them all. Every frame then built every pipeline again: on a
test machine with a software graphics card that was 70% of the window's time, and a frame of the Songs page took
115 ms instead of 6.

`src/lib.rs`: `#![allow(warnings)]`. Built here as a local crate, it gets the app's `-D warnings`, and upstream has two
warnings with the features Slint picks (an unused import and an unused method), which would stop the build.

Each change is marked "Music Organiser patch".

## When to remove it

femtovg 0.28 keeps its pipelines in its own way. Once a Slint release uses femtovg 0.28 or later, delete this folder and
the `[patch.crates-io]` lines and the `exclude` entry in the workspace's `Cargo.toml`.
