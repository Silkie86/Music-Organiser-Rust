---
name: galaxy-star-look
description: Galaxy "star look" (stars drawn as stars, Light by at every zoom): PR #176 replaced #173; review rounds, open look questions for Solomon
metadata:
  type: project
  modified: 2026-11-03T16:48:15.035Z
---

2026-11-03: the outside session's star look PR #173 (branch claude/affectionate-noether-star-look, untested wip) was
reviewed twice at max effort and every point fixed on branch claude/project-thread-6dolih as **PR #176**; #173 was
closed (its branch left alone). #176 MERGED 2026-11-03 17:06 as 1bc3185 (CI green). Files: crates/mo-app/src/galaxy_look.rs (look rules, Light by ranks, glints by rank,
twinkle rules `twinkle_amount`/`twinkles`/`beat`), galaxy_picture.rs (processor's picture, 64-row bands, glow lookup),
crates/mo-ui/src/sounds/sky.rs (wgpu card, 5 passes, pass 5 = crisp stars; screen blend on dark pages, over blend on
light pages via themes::is_light), sounds/galaxy.rs (page glue, twinkle timer). Merge order after it: #172 (app clean-up),
then harmony.

Look facts worth keeping:
- Light by Nothing far out (zoom 1-12) is pixel-identical to #173 on the card.
- Close up on dark pages, card cores are ~10% dimmer than #173 (glow and core are screened together so a star keeps
  its hue; #173 added them and clipped per channel, blue went cyan, Paper got rings).
- Processor's picture with Length close up at 200% is ~170 ms on 4 cores (main 48 ms): every lit star has a big
  glow; off the UI thread, newest wins. Card: a twinkle frame only redraws pass 5.

Solomon's answer (2026-11-03 17:45, after seeing the before/after pictures): "all good". Keep both looks as
they are: white-hot star middles stay on light themes (Paper), close-up cores on dark pages stay as merged. No
[Frames] lines sent yet; ask again only if he reports lag on the galaxy.

Renders: /mnt/project-files/screenshots/star-look/ (final/ = all 30 views; before-after-*.png = #173 vs #176).
Render test: `GALAXY_GPU_LOOK=<dir> [GALAXY_GPU_TIME=1] cargo +1.99 test -p mo-ui --lib sounds::sky::tests::render
-- --ignored --nocapture` (software card is fine).

Follow-ups not done: g.scale not updated on a DPI change without a resize; least lit sound (rank 0) looks unlit;
the processor's picture has no spikes. See [[gpu-for-graphics]], [[app-screens-ui]], [[rust-rebuild-rules]].
