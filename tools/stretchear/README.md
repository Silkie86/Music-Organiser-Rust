# Stretcher ear test

Picks up to 20 loops from a folder (each with its tempo in its name, its folder's name or its ACID notes; 30 s at
most; mono or stereo) and stretches each to 80% of its tempo (slower) and 125% (faster) with four stretchers:
Signalsmith Stretch alone, the `timestretch` library alone, what the app plays today (`timestretch` with the hits
put back), and our drum stretch (`mo_stretch::drums`).

```
cargo run --release -p mo-stretchear -- "J:\samples\Some pack\Loops" "I:\Stretch ear test" 20
```

The second folder must be new or empty. Each loop gets a folder with `original.wav` and `slower A.wav` to
`faster D.wav`: the letters are in a different order for every loop, so listen without knowing which is which.
Then open `key.csv`: it says which letter was which stretcher, with each one's scores (ghost before the hits, hit
sharpness, timing, distance from the original; see `mo_stretch::score`). The loops themselves are only read.
