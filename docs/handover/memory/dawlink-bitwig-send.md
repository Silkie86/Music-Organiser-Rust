---
name: dawlink-bitwig-send
description: mo-dawlink crate (Send to Bitwig, phase 5 start): API for the screens, Drum Machine ID, stand-in Bitwig, what's not done
metadata:
  type: project
  modified: 2026-11-03T19:56:22.736Z
---
2026-11-03, thread "Build the Bitwig link for Send to Bitwig" built `crates/mo-dawlink`: PR #182 merged 2026-11-03 (squash 57925cf on main) after two max-effort reviews; API handed to the screens thread the same day.
Solomon's F12 note 2026-11-02: the Sounds card should have Send to Bitwig like the old app. The UI wiring is the
screens thread's ("Work through the screens queue").

- API (all wait for Bitwig, so call on `mo_app::background::spawn`): `mo_dawlink::bitwig::send_sound(path, name)
  -> Result<SentSound, Error>`; `kit_pads(&[KitSound])` then `send_kit(name, &pads, KitTarget)`;
  `drum_machine(lib.setting("bitwig_drum_machine"))` picks the ID; after `SentKit::drum_machine == false` the screen
  uses `KitTarget::Selected` until restart; `Bitwig::check()` = what's open / can send; `Error::is_elsewhere()`.
  Card words: `sending_sound`, `SOUND_PLACE`, `SOUND_FAILED`, `sending_kit`, `KIT_FAILED`, `title()`, `detail()`,
  `kit_name(seed)`.
- Contract: `Err` = nothing changed in Bitwig; `Ok` with `problem` = something may have changed (e.g. an empty track).
- Sends take numbers in a process-wide queue (FIFO, like a deli counter) and go one at a time in the order they
  reach the crate; std Mutex isn't fair (Windows SRWLock), which is why it isn't a plain lock.
- Drum Machine UUID 8ea97e45-0255-40fd-bc7e-94419741e9d1 (DrivenByMoss ModelImpl.java:67), Sampler
  468bc14b-b2e7-45a1-9666-e83117fe404e; all three IDs are in tools/formatlab/registry/bitwig.json field 0x99, and
  a unit test in device.rs keeps them equal. C# kept an ID it read from Bitwig files in library.db
  `bitwig_drum_machine` ("" = it didn't give a Drum Machine); Rust prefers that, else the built-in one.
- Extension 3.18's pads step waits 5 s for a new device's pads; "Device d on track t isn't a Drum Machine." can mean a
  slow Bitwig. Rust keeps drum_machine true when insert named the device "Drum Machine".
- Every send keeps changes to the project open as it began (`onlyIn`); C# Send didn't.
- Tests: `bitwig::fake` (feature `fake-bitwig`; the crate turns it on for its own tests by a dev-dependency on itself),
  answering in extension 3.18's words; tests in `crates/mo-dawlink/tests` use `CARGO_TARGET_TMPDIR` fixtures.
- Running the C# repo's JS stand-in (`node extensions/bitwig/test/serve.js`) was refused by the cloud session's
  safety check (code from outside); the JS script's protocol was checked by reading it instead.
- Not yet: sends aren't counted in Most liked (sounds.db read-only); FL link, watch, checks, live tests, the FL
  channel send (rest of phase 5). mo-convert keeps its own Sampler/Note Transpose IDs. Differences judged in
  `crates/mo-dawlink/DIFFERENCES.md`.

Related: [[rust-rebuild-rules]], [[app-screens-ui]], [[sounds-backend-status]].
