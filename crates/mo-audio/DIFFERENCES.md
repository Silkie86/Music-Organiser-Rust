# Where the Rust sound file reader differs from the C# one

The C# `Sounds/AudioFile.cs` is the guide, not the answer (CLAUDE.md). Each difference is judged: **better**, **not
yet** or **bug**.

| What | C# | Rust | Judged |
|---|---|---|---|
| WAV with a bit depth that isn't whole bytes (12-bit, 20-bit) | Bytes per sample = bits ÷ 8 rounded down, so a 12-bit file was read one byte a sample and came out as noise. | Rounded up, as the file stores them. | Better |
| WAV `fmt ` chunk shorter than 16 bytes | Read on into the next chunk's bytes. | What's missing reads as 0, so the file says it has no audio data. | Better |
| AIFF `COMM` chunk shorter than 18 bytes | Stopped with an out-of-range error. | Says the chunk is cut short. | Same result, clearer |
| AIFF with a bit depth of 0 | Divided by zero. | Says the bit depth isn't supported. | Same result, clearer |
| Writing a WAV (kit export) | Wrote straight to the final name. | Writes beside it as `.wav.part`, then renames, so a stopped export leaves no half file. | Better |
| MP3, FLAC, M4A, OGG, AAC | Windows' decoders. | Not yet: symphonia comes in the next part. | Not yet |
