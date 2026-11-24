# i-slint-backend-winit 1.18.1, patched for Music Organiser (for now only)

This is Slint's window backend (the part that talks to winit, which talks to Windows), copied from crates.io
(i-slint-backend-winit 1.18.1, licences in `LICENSES/`) with notes added and nothing else changed. The workspace's
`Cargo.toml` points `i-slint-backend-winit` here with `[patch.crates-io]`; `mo-ui` names it as a dependency only to
read the notes. The galaxy check doesn't use it.

## Why

On 24 Nov 2026 Solomon's maximised window dropped to 1440 x 900 by itself, 10 s after he brought it back from the
taskbar, on Songs with no page change. 1440 x 900 is the size the window has when it isn't maximised, so something
took it out of maximised. In winit 0.30 on Windows that happens when Slint asks for a size that differs from the
window's own (`request_inner_size`), moves it (`set_outer_position`), sets maximised to false, or when the screen's
scale changes; or when the user does it. His "Size of everything" is 125%, so Slint draws at 1.25 while winit's scale
stays 1.0, and a size Slint asks for in logical pixels reaches winit at the wrong scale: a strong suspect.

## The notes

`lib.rs` has `mo_probe`: the last 64 things this backend asked winit to do to the window's size and state, each with
the line that asked. `winitwindowadapter.rs` notes `resize_window` (with both scales, the window item's size and the
adapter's), `set_maximized`, `set_fullscreen`, `set_minimized`, `set_min_inner_size`, `set_max_inner_size`,
`set_outer_position`, winit's scale changes and winit's word that maximised changed. The app notes its own direct
winit calls too (`note_from_app`: the start-up mend, full screen put back after the taskbar, another copy's Show). When the window leaves maximised, `mo_ui::settings::resized` writes the
notes from the 30 s before into the log's `[Window]` line.

Each change is marked "Music Organiser patch". Only what the app builds is kept (the upstream tests are gone).

## When to delete it

Once a log names the cause and it is fixed: delete `vendor/i-slint-backend-winit`, its two lines in the workspace's
`Cargo.toml`, the dependency in `crates/mo-ui/Cargo.toml` and the notes part of `mo_ui::settings::resized`.
