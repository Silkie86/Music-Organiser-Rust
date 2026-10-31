Made here with ffmpeg: half a second of a 440 Hz tone, stereo, 22,050 Hz, in each format the app decodes with
symphonia. Nothing from anyone's sample library.

    ffmpeg -f lavfi -i sine=frequency=440:sample_rate=22050:duration=0.5 -ac 2 -map_metadata -1 tone.flac
    (and tone.mp3 at 64k, tone.m4a and tone.aac as AAC 64k, tone.ogg as Vorbis q0)
