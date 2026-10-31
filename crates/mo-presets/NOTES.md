# mo-presets: what's done and what's next

The Presets page's engine (spec section 3.4a). No UI, no database, no writing yet.

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

## To check on Solomon's files

- `.bwpreset` metadata keys (`device_name`, `device_type`, `creator`, `preset_category`, `tags`) are from Bitwig's
  usual layout, not yet seen in one of Solomon's presets. When a key is missing the reader falls back to the device
  object (fields 0x9a, 0x12de, 0xbe5, as the C# `BitwigPreset.Inspect` reads them), so a wrong key costs only the extras.
- The thresholds in `role::Thresholds` are starting points. Rule 5: set them from counts and averages of Solomon's MIDI
  (`role::NoteStats` is that summary), for example with a census in the twin tool.

## Next

- More plugins' own octave (`octave`: Serum 1 is read, from the C# `Serum1State` offsets, with its sub oscillator
  when only that plays; Serum 2's CBOR state next), and Bitwig Note Transpose devices (needs the Bitwig reader to say which devices come
  before an instrument).
- Bitwig: the reader doesn't say which presets are instruments, so no Bitwig synth is marked unused yet.
- Export and Send to Bitwig (`.bwpreset` from a template Bitwig saved, as the C# `BitwigPreset`), copy-check-rename.
