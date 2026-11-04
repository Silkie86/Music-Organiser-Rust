# femtovg 0.27.0, patched for Music Organiser

This is the drawing library Slint uses to draw the window on the graphics card, copied from crates.io (femtovg 0.27.0,
MIT or Apache-2.0, see `LICENSE-MIT` and `LICENSE-APACHE`) with one fix. The workspace's `Cargo.toml` and the galaxy
check's (`tools/galaxycheck/Cargo.toml`) point `femtovg` here with `[patch.crates-io]`, and a test in `mo-ui`
(`tests/drawing_library.rs`) fails if either stops using this copy.

Only what the app builds is kept: `src`, the licences and the readme. The manifest lost the upstream examples and tests
(their files aren't in the crate) and its release profile (Cargo ignores profiles outside the workspace's root).

## The fix

`src/renderer/wgpu.rs`: the cache of pipelines (the graphics card's ready-made drawing steps: one for flat colour, one
for text, one for pictures, and so on) is only cleaned once it holds more than 256, and then drops the ones not used
since the last clean. Upstream cleaned it after every render, dropping every pipeline that render didn't use. Each
pipeline is one combination of a few settings (shader, blending, stencil, format, topology), so a page uses 10 to 20
of them and the cache stays far below 256.

Slint renders twice each frame when the app has a rendering notifier (the app has one: it shares the graphics card
with the galaxy and the arrangement, and times each frame for the `[Frames]` log lines). The first render only clears
the window, so the clean kept only the clearing pipeline and dropped the rest. Every frame then built every other
pipeline again: on a test machine with a software graphics card that was 70% of the window's time, and a frame of the
Songs page took 115 ms instead of 6.

`src/lib.rs`: `#![allow(unused_imports, dead_code)]`. Built here as a local crate, it gets the app's `-D warnings`, and
upstream has two warnings with the features Slint picks (an unused import and an unused method), which would stop the
build.

Each change is marked "Music Organiser patch".

## When to remove it

femtovg 0.28 keeps its pipelines in its own way. Once a Slint release uses femtovg 0.28 or later, the `mo-ui` test says
the copy isn't used any more. Then delete this folder, the `[patch.crates-io]` lines in both `Cargo.toml` files, the
`exclude` entry in the workspace's `Cargo.toml`, and the test.
