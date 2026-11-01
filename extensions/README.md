# DAW scripts

Small programs that run inside the DAWs. They are ported from the C# app's `extensions` folder and grow here.

| Folder | Runs in | What it does |
|---|---|---|
| `fl-studio` | FL Studio, as the MIDI controller script "Music Organiser" | The FL link (SysEx on the Music Organiser MIDI port), plus a one-off measurement: with `MEASURE_IN_LIVE_TEST = True` and the project "MO Live Test automation" open, it sets each insert's fader to 0.00 ... 1.00 and reads the dB FL shows, sets the tempo to automation values 0.00 ... 1.00 and reads the BPM, puts both back, writes the readings as insert names ("v0.80 0.0dB", "t0.50 266.0bpm") and saves, so Format Lab reads them from the .flp (FL scripts can't write files). |

The Bitwig test driver for Format Lab is in `tools/formatlab/daw/bitwig`.

Tests (`python -m unittest discover -s extensions/fl-studio/test`) run the script against a stand-in FL Studio; they
never reach a real DAW or MIDI port (rule 11).
