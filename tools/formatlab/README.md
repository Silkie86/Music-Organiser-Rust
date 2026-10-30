# Format Lab

Format Lab looks inside Bitwig and FL Studio files, to learn how each DAW saves things. It only reads files.

```
cargo run -p mo-formatlab -- tree "Loop Idea.bwproject" --depth 3
cargo run -p mo-formatlab -- diff "before.flp" "after.flp"
cargo run -p mo-formatlab -- find "Song.flp" 128.5
cargo run -p mo-formatlab -- census "D:\Music\Projects"
cargo run -p mo-formatlab -- fields fl 224
```

- **tree** shows everything in a file, named where the field registry knows the name.
- **diff** names every value that differs between two saves. Save a project, change one thing in the DAW, save it
  again under another name, and diff the two: what changed is what holds that setting.
- **find** shows everywhere a file holds a value: as text, as a number, or inside a plugin's bytes (as a 32-bit integer
  or a 32- or 64-bit float, in either byte order).
- **census** counts, across every project in some folders, the FL events and Bitwig classes and fields the registry
  doesn't know yet, most used first. It keeps only ids and counts, nothing of the music, so its report can be pasted
  into a conversation.
- **fields** lists what the registry knows, or everything about one id.

The registry (`registry/bitwig.json`, `registry/fl-studio.json`) is a copy of the C# app's `Core/Formats/Registry`,
which stays the source of truth until the Rust app takes it over.

MO Probe (`probe`) and the live tests (`live`) come with the DAW links in phase 5.

## Where it differs from the C# Format Lab

Checked by running both on every twin-test file: `tree`, `diff`, `find` (several values each), `census` and `fields`
give the same text, apart from these.

| What | C# | Rust | Judged |
|---|---|---|---|
| An FL save cut short, or with a garbled header | Says nothing. | Says what the reader found, under the file's name ("Warning: …"). | Better |
| Text in an FL save whose version has no minor part ("9") | Read as UTF-16, so it comes out garbled. | One byte a letter, as the Rust reader reads it (see `crates/mo-formats/DIFFERENCES.md`). | Better |
| A Bitwig list of 64-bit floats (type 0x18) | Shown as "System.Double[]". | Shown as its numbers, and found by `find`. | Better |
| A metadata key or map key written twice in a Bitwig file | The last value shown, in the first one's place. | Each shown where it is. | Better |
| A reference to a Bitwig file's top object | "object #1". | "body". | Better |
| An FL arrangement (FL 12.9 onwards) | Starts at its playlist, so its number and name (events 99 and 241) sit under the last channel. | Starts at event 99, as "arrangement 0", "arrangement 1" and so on. | Better |
| `diff` of a long list or text that changes past what's shown (32 values, 300 letters) | "No differences." | Listed as changed. | Better |
| `find` with a number | Looks for it in data only as numbers. | As text too ("808" in a preset called "808 Kick"). | Better |
| `census` | Leaves out the same folders and files as the library scan. | The same rules, and at low priority on all cores but one. | Same |
| Text cut at 300 letters | Counts UTF-16 units, so an emoji counts as two. | Counts letters. | Same for nearly all text |
