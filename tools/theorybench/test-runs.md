# Runs on the kept-back fold

Every `theorybench keyfit --final` or `--write` run adds a line here: fold 0 is only looked at these ways (`--final` scores it alone; `--write` scores every fold, fold 0 included, before it writes the weights that ship), so this is how often it was. A commit marked -dirty had changes not yet committed when it ran.

| Date | Run | Finder version | Commit | Corrections penalty | Full mix: mo-harmony (C# app) |
|---|---|---|---|---|---|
| 2026-11-01 | --final (pop909cl; billboard at 0.25; rs200 measured only) | 3 | 0e91907 | 0.0001 | Whole songs with one key 89.6% (88.9%); Whole songs that change key 71.4% (74.3%); 1-bar windows 42.8% (31.0%); 2-bar windows 58.7% (46.5%); 4-bar windows 73.4% (63.6%); 8-bar windows 81.5% (75.4%); 16-bar windows 85.0% (81.5%); 1-bar openings 80.6% (66.3%); 2-bar openings 78.5% (64.0%); 4-bar openings 78.7% (66.3%); 8-bar openings 86.6% (80.4%); 16-bar openings 88.8% (87.2%) |
| 2026-11-01 | --write (pop909cl; billboard at 0.25; rs200 measured only) | 3 | 0e91907 | 0.0001 | Whole songs with one key 90.7% (87.8%); Whole songs that change key 73.4% (69.5%); 1-bar windows 41.5% (29.7%); 2-bar windows 56.8% (44.7%); 4-bar windows 71.6% (62.2%); 8-bar windows 81.4% (75.2%); 16-bar windows 84.9% (81.3%); 1-bar openings 78.6% (57.1%); 2-bar openings 77.9% (63.0%); 4-bar openings 80.0% (66.3%); 8-bar openings 87.2% (79.3%); 16-bar openings 90.3% (85.5%) |
| 2026-11-01 | --write (pop909cl; billboard at 0.25; rs200 measured only) | 3 | 64b49c3-dirty | 0.0001 | Whole songs with one key 90.7% (87.8%); Whole songs that change key 73.4% (69.5%); 1-bar windows 41.5% (29.7%); 2-bar windows 56.8% (44.7%); 4-bar windows 71.6% (62.2%); 8-bar windows 81.4% (75.2%); 16-bar windows 84.9% (81.3%); 1-bar openings 78.6% (57.1%); 2-bar openings 77.9% (63.0%); 4-bar openings 80.0% (66.3%); 8-bar openings 87.2% (79.3%); 16-bar openings 90.3% (85.5%) |
| 2026-11-01 | --write (pop909cl; billboard at 0.25; rs200 measured only) | 4 | 40b0eb7-dirty | 0.0001 | Whole songs with one key 90.7% (87.8%); Whole songs that change key 73.4% (69.5%); 1-bar windows 41.5% (29.7%); 2-bar windows 56.8% (44.7%); 4-bar windows 71.6% (62.2%); 8-bar windows 81.4% (75.2%); 16-bar windows 84.9% (81.3%); 1-bar openings 78.6% (57.1%); 2-bar openings 77.9% (63.0%); 4-bar openings 80.0% (66.3%); 8-bar openings 87.2% (79.3%); 16-bar openings 90.3% (85.5%) |
| 2026-11-01 | --write (pop909cl; billboard at 0.25; rs200 measured only) | 4 | 101e49a | 0.0001 | Whole songs with one key 90.7% (87.8%); Whole songs that change key 73.4% (69.5%); 1-bar windows 41.5% (29.7%); 2-bar windows 56.8% (44.7%); 4-bar windows 71.6% (62.2%); 8-bar windows 81.4% (75.2%); 16-bar windows 84.9% (81.3%); 1-bar openings 78.6% (57.1%); 2-bar openings 77.9% (63.0%); 4-bar openings 80.0% (66.3%); 8-bar openings 87.2% (79.3%); 16-bar openings 90.3% (85.5%) |
| 2026-11-03 | --rescale --final, pulled back to 0.50 (packs) | 4 | 3be3d26 | none (rescale) | Whole loops 46.9% (38.4%); Pieces cut from longer parts 41.2% (27.9%); 1-bar windows 29.0% (20.0%); 2-bar windows 33.1% (25.9%); 4-bar windows 38.4% (31.9%); 8-bar windows 39.9% (35.3%); 16-bar windows 40.7% (38.4%); 1-bar openings 43.8% (30.9%); 2-bar openings 42.5% (32.6%); 4-bar openings 47.4% (37.2%); 8-bar openings 44.7% (38.2%); 16-bar openings 46.2% (38.9%) |
| 2026-11-03 | --rescale --write, its folds pulled back to 0.50 to 0.55 (packs) | 4 | 3be3d26-dirty | none (rescale) | Whole loops 55.0% (36.5%); Pieces cut from longer parts 41.3% (28.5%); 1-bar windows 38.9% (19.5%); 2-bar windows 43.8% (27.3%); 4-bar windows 46.7% (33.7%); 8-bar windows 47.3% (36.3%); 16-bar windows 47.4% (37.4%); 1-bar openings 58.1% (27.4%); 2-bar openings 54.5% (32.7%); 4-bar openings 53.4% (37.6%); 8-bar openings 51.2% (37.5%); 16-bar openings 50.1% (37.7%) |

## Tunes and chords together (`pairs`)

Every `theorybench pairs --folds 0` run adds a line here by hand. Each is 8-bar stretches, each against 50 from other
songs: its own over another song's / first of all 51 / over another stretch of its song, then its own pair best
unmoved.

| Date | Run | `TOGETHER_VERSION` | Commit | Chords for a tune: new (old app) | A tune for chords: new (old app) | Best unmoved: new (old app) |
|---|---|---|---|---|---|---|
| 2026-11-11 | pairs pop909cl --folds 0, the fit at 3,000 steps | 1 | f0564ad-dirty | 93.1% / 53.8% / 85.0% (89.5% / 35.2% / 83.1%) | 92.3% / 45.5% / 84.0% (89.6% / 36.5% / 82.3%) | 94.7% (93.8%) |
| 2026-11-11 | pairs pop909cl --folds 0, the fit run on to 8,000 steps (as shipped; no number moved more than 0.02) | 1 | f0564ad-dirty | 93.1% / 54.0% / 85.0% (89.5% / 35.2% / 83.1%) | 92.3% / 45.5% / 84.0% (89.6% / 36.5% / 82.3%) | 94.7% (93.8%) |
