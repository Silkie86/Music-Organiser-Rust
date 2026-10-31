# mo-presets: what's done and what's next

The Presets page's engine (spec section 3.4a). No UI. It writes only `presets.db` and the export folder.

## Done

- **Preset files on disk** (`files`): `.fxp` (program name, plugin's four letters), `.fxb` (program count),
  `.vstpreset` (class id; plugin name, category, kind and tags from its "Info" chunk, read from the file's end when the
  state before it is big), `.bwpreset` (device, creator, category, tags from its metadata; for a plugin, its id from the
  state file in the zip). Only headers are read, never the whole state.
- **Part guess** (`role`): bass, lead, chords, pad, stabs/plucks, arp, drums, FX, from the notes as heard (FL root key
  and pitch knob), with names as clues for a weak call or a part with no notes. Each guess carries its reason and says
  when the octave or transpose wasn't checked. Your fix always wins (`list::apply_fixes`).
- **One song's sounds** (`list`): FL (instruments by channel, effects take the part of the channels feeding their
  insert, master effects get none) and Bitwig (every preset on a track shares the track's part).
- **Choices** (`filter`): maker and type with counts (the C# `PresetFilter`), load hints, details line, search.

- **The whole list** (`library`): many saves and preset folders at once, over every core, as the page's rows.
- **Your fixes** (`fixes`): `presets.db` (`your_preset_roles`), kept by a fingerprint of the plugin, its settings
  and the notes it plays (agreed with the readers thread, like the C# `midi.db`'s `your_roles`).

- **Export** (`export`): a save's sounds as preset files in `<export folder>\<save>\Presets`, copy-check-rename, each
  sound keeping its name whichever are exported; one bad preset is reported, not stopping the rest.
- **Send to Bitwig** (`bitwig`): a `.bwpreset` beside each exported sound whose plugin has a preset Bitwig saved
  (the newest one per plugin and VST2/VST3, found among the presets the page already lists). The template is decoded,
  its settings file and preset name changed field by field, written with `mo_formats::bitwig::encode`, read back and
  checked; a sound whose settings are for another plugin (by id) is refused. VST2 programs go in as one-program banks.

## To check on Solomon's files

- `.bwpreset` metadata keys (`device_name`, `device_type`, `creator`, `preset_category`, `tags`) are from Bitwig's
  usual layout, not yet seen in one of Solomon's presets. When a key is missing the reader falls back to the device
  object (fields 0x9a, 0x12de, 0xbe5, as the C# `BitwigPreset.Inspect` reads them), so a wrong key costs only the extras.
- Send to Bitwig's templates: a preset with another device beside its plugin (a chain) isn't used, and devices inside
  the plugin's device (modulators) are allowed. Check on a few of Solomon's own Bitwig presets that a plain one-plugin
  preset passes, and that one made with Send to Bitwig opens in Bitwig with its sound.
- The thresholds in `role::Thresholds` are starting points. Rule 5: set them from counts and averages of Solomon's MIDI
  (`role::NoteStats` is that summary), for example with a census in the twin tool.

## Next

- More plugins' own octave (`octave`: Serum 1 is read, from the C# `Serum1State` offsets, with its sub oscillator
  when only that plays; Serum 2's CBOR state next), and Bitwig Note Transpose devices (needs the Bitwig reader to say which devices come
  before an instrument).
- Bitwig: the reader doesn't say which presets are instruments, so no Bitwig synth is marked unused yet.
- Send to Bitwig reads each template again when the page lists them (they were read once by the list); the list
  could keep what a template needs instead.
- A sound's Bitwig preset name is kept for it in the export names, so two plugins with one sound name ("Pad.fxp",
  "Pad.vstpreset") now give "Pad 2.vstpreset" for the second, where the C# app gave "Pad.vstpreset".
- Export: half-written `.part-*` files left by a crash aren't cleared yet; names are cut by UTF-16 units as the C#
  app does, not by Linux's 255-byte limit.
