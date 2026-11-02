---
name: fl-to-bitwig-converter-plan
description: Gaps in the C# FL-to-Bitwig converter (automation, FL native plugins) that the Rust phase 5 converter should close
metadata:
  type: project
  modified: 2026-10-31T12:10:00.000Z
---

Solomon (2026-10-30) doubts the C# converter handled FL automation properly. Read of C# Export/BitwigProject.cs:
- Carried: channel volume/pan, VST plugin params, mixer-insert effect params, on/off (by muting clips).
- Left out (noted in "not carried over"): tempo automation, master volume, insert fader/pan (FL fader->gain mapping
  never worked out), insert slot on/off, pitch, FL native plugin params, params of swapped (upgraded) plugins, and
  tension curves (points joined straight; the Rust reader also drops tension for now).
- Plan for phase 5: close tempo, insert fader/pan, slot on/off, curve shape; check each against a real song of
  Solomon's opened in both DAWs. Ask Solomon for a song where automation came out wrong.

FL native plugins in Bitwig (Solomon asked about a wrapper): own wrapper not realistic (Image-Line natives run only
in FL). Proposed: swap to Image-Line's VST twins where they exist (Sytrus, Harmor, Harmless, FLEX, Gross Beat...),
testing per plugin whether the native state loads into the VST; otherwise the "FL Studio" VST plugin hosting the
native one (state hard to fill). Not decided yet; revisit in phase 5. See [[rust-rebuild-status]].

3x Osc (Solomon asked 2026-10-30): its full settings are in the FLP's native plugin state, undocumented and not
decoded by C# or Rust. Proposed: decode with formatlab from tiny one-knob-changed test saves Solomon makes (kept on
his PC, only offsets in code), then the phase 5 converter fills a prepared 3-oscillator Grid patch from the Bitwig
template (shapes, coarse/fine, detune, levels, pan; channel envelope/filter too). Asked whether to add it to the plan.
Grid patches (Solomon asked): layout inside a Poly Grid not mapped yet (module classes, wires unknown). Plan: never
build from scratch; copy a Grid patch Solomon saves in the template, change values, swap modules from spares kept
in it; map with formatlab on one-change Bitwig saves; prove in the "MO Live Test" project only.
Format Lab (proposed to Solomon 2026-10-30): keep it lean in phase 1 (tree, diff, find, census); in phase 5 add
DAW-assisted mapping: an FL script reads/sets native plugin params by name (plugins module), saves after each
change, and Format Lab pairs the values with the changed bytes; Bitwig controller script likewise for device params
(not Grid wiring). Live test project only.
DECIDED (Solomon 2026-10-30 06:53): 3x Osc and Grid patches wait until later. Format Lab must be as automatic as
possible, driven through the DAWs' script APIs (no hand-made saves). Checked 2026-10-30: FL scripts CAN save:
transport.globalTransport(midi.FPT_Save=92) and FPT_SaveNew=93 (new numbered version); plugins.getParamCount/Name/
Value/setParamValue (API v8+). Still to try: whether 3x Osc exposes all its knobs that way. Bitwig: Save action.

STATE 2026-10-31: Rust crate `mo-convert` (plan.rs DAW-neutral plan -> bitwig.rs writer -> save.rs, sample.rs).
PR #37 (plan) merged; PR #43 (writer + sample + save) open. C# differences judged in crates/mo-convert/DIFFERENCES.md.
Still to do, in order: close C# gaps above (tempo, insert fader/pan, slot on/off, curves); LostSounds from older
saves; missing-sample search via mo_library::samples (rule 13); MP3/OGG/M4A timing once mo-audio decodes.
The convert's `written` stamp uses .NET ticks to match C# in the shared library.db key `bitwig_convert:{file}`.
FL edit events (per-pattern recorded knob moves, Solomon asked 2026-10-31): read by neither C# nor the Rust FL
reader; converter drops them silently. Proposed: readers thread reads them, converter places them per pattern clip;
meanwhile a "not carried over" note. Asked Solomon for a song using them.

Multisample copies relink: see [[bitwig-multisample-relink]].
Automation gaps work (tempo, faders, on/off, curves) and Bitwig's lane layout: see [[bitwig-automation-lanes]].
Follow-ups from Solomon's log 2026-11-02 (build 8e06858): first convert of sly_7 took 36 s (2nd 0.4 s), sly_17 10.8 s
(cold sample search?); every re-convert makes "Name (2)" because the `written` stamp isn't stored while library.db is
read-only (by design in mo-app bitwig_convert) -> could keep stamps in a Rust-only file in the data folder.
