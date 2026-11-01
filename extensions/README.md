# DAW scripts

Small programs that run inside the DAWs. They are ported from the C# app's `extensions` folder and grow here.

| Folder | Runs in | What it does |
|---|---|---|
| `fl-studio` | FL Studio, as the MIDI controller script "Music Organiser" | The FL link (SysEx on the Music Organiser MIDI port), plus a one-off measurement: with `MEASURE_IN_LIVE_TEST = True` and the project "MO Live Test automation" open, it sets insert 1's fader to 0.00 ... 1.00 and reads the dB FL shows, sets the tempo to automation values 0.00 ... 1.00 and reads the BPM, puts both back, writes the readings in the first inserts' names, several to a name ("v0.80 0.0dB; v0.81 ...", "t0.50 266.0bpm; ...") and saves. FL Studio 2025 doesn't tell scripts the project's file name, so the project is known by its Project info title (F11), which must be exactly "MO Live Test automation", so Format Lab reads them from the .flp (FL scripts can't write files). |

The Bitwig test driver for Format Lab is in `tools/formatlab/daw/bitwig`.

Tests (`python -m unittest discover -s extensions/fl-studio/test`) run the script against a stand-in FL Studio; they
never reach a real DAW or MIDI port (rule 11).
