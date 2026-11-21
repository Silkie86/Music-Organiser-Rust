Made here with ffmpeg from a 440 Hz tone. Nothing from anyone's sample library.

- `tone-short.ogg`: 0.24 seconds, stereo, 22,050 Hz, Vorbis q0: as long as the start the converter decodes to ask a
  file how long it is.

      ffmpeg -f lavfi -i sine=frequency=440:sample_rate=22050:duration=0.24 -ac 2 -map_metadata -1 -fflags +bitexact -c:a libvorbis -q:a 0 tone-short.ogg

- `tone-no-length.mp3`: 3 seconds (0.3 of tone, then silence), mono, VBR, with no length written in it (no Xing
  header), so a reader that guesses from its first frames and its size gets it wrong: the converter leaves it for Bitwig.

      ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=22050:duration=0.3,apad=whole_dur=3" -ac 1 -map_metadata -1 -fflags +bitexact -c:a libmp3lame -q:a 9 -write_xing 0 -id3v2_version 0 tone-no-length.mp3
