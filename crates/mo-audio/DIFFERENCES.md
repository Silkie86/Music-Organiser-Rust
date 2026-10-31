# Where the Rust sound file reader differs from the C# one

The C# `Sounds/AudioFile.cs` is the guide, not the answer (CLAUDE.md). Each difference is judged: **better**, **not
yet** or **bug**.

| What | C# | Rust | Judged |
|---|---|---|---|
| WAV with a bit depth that isn't whole bytes (12-bit, 20-bit) | Bytes per sample = bits ÷ 8 rounded down, so a 12-bit file was read one byte a sample and came out as noise. | Rounded up, as the file stores them. | Better |
| WAV `fmt ` chunk shorter than 16 bytes | Read on into the next chunk's bytes. | What's missing reads as 0, so the file says it has no audio data. | Better |
| WAV `acid` chunk the file cuts short | Stopped with an end-of-file error, so the whole file failed. | The cut-short chunk is left out and the audio is read. | Better |
| AIFF `COMM` chunk shorter than 18 bytes | Stopped with an out-of-range error. | Says the chunk is cut short. | Same result, clearer |
| AIFF with a bit depth of 0 | Divided by zero. | Says the bit depth isn't supported. | Same result, clearer |
| Writing a WAV (kit export) | Wrote straight to the final name. | Writes beside it as `.wav.part`, then renames; a failed write removes the `.part` file, so a stopped export leaves no half file. | Better |
| MP3, FLAC, M4A, OGG, AAC | Windows' decoders (Media Foundation), Windows only. | symphonia, on every system. Lengths can differ by an encoder's padding (a few ms), as each decoder trims it its own way. | Better (Mac and Linux read them) |
| WMA | Windows' decoders. | Not read (no Rust decoder). | Not yet |
| A written WAV whose samples take an odd number of bytes (mono 24-bit, odd length) | No pad byte after the data, which RIFF requires. | Pads it, as RIFF says. | Better |
| An M4A or AAC file's whole length | Windows' decoder trims the encoder's padding. | symphonia's AAC decoder doesn't, so a whole read counts ~10 ms more than a partial one. | Bug, small (fix with a padding-aware AAC path later) |
| A FLAC or OGG file cut short | Windows read the part that's there. | Says the file is cut short or damaged (MP3 gives the part that's there). | Not yet |

## Playback

The C# `OpenAlPlayer` (OpenAL Soft through Silk.NET) is the guide; `play::Player` is built on cpal.

| What | C# | Rust | Judged |
|---|---|---|---|
| Sound output library | OpenAL Soft, a separate library the app ships. | cpal, built into the app (WASAPI on Windows, Core Audio on a Mac, ALSA on Linux). | Better (nothing extra to ship) |
| Output delay | About 22 ms (OpenAL's own mixing plus Windows'). | Asks for 10 ms buffers where the output allows, else the output's own size; the delay the system reports is in `info().delay_ms` for the log. | Better, to be measured on Solomon's PC |
| Moving from one sound to the next | Cut at once (a click when sweeping). | The old sound fades out over 3 ms while the new one starts; a move part way fades in. | Better |
| Changing a sound's rate to the output's | OpenAL's resampler. | A cubic curve through the four nearest samples. | Same |
| Sounds with more than two channels | Channels past the first two go into both sides at -3 dB. | The same. A one-speaker output gets left and right mixed. | Same |
| Noticing outputs come and go | Windows tells it; on a Mac or Linux it looked every few seconds. | Windows tells cpal when the default output changes or one goes, and the player reopens; everywhere, a new sound looks at most every 3 s whether the chosen output came back or the default moved, and `check()` looks at once. | Same |
| An output that keeps going | Reopened up to 3 times a sound; after one wouldn't open, a sound tried again 2 s later. | The same. A sound waiting for an output tries again by itself after 2 s, and sounds swept meanwhile don't each try. | Same |
| Long files | Streamed from the disk as they play. | Plays what's been read into memory; streaming comes with the Sounds page (it's how the page will hand sounds to the player). | Not yet |
