---
name: small-database-files
description: Solomon wants the database files small (sounds.db 4.56 GB, library.db 0.9 GB on his PC); where sounds.db's space goes and the compact plan
metadata:
  type: feedback
  modified: 2026-11-03T18:14:40.297Z
---

Solomon (2026-11-03 18:11-18:14, "Speed up loading Sounds" thread): "also make sure the database files are not huge",
"is there a way to decrease that sounds db", "does it have to be that large". Extends rule 23 (store each fact once).

Facts: his sounds.db is 4.56 GB for 453,885 sounds (+8 MB WAL), library.db 0.9 GB (from his session logs). Both are
written by the C# app; the Rust app opens them read-only until twin tests pass, so it can't shrink them yet.

Where sounds.db's space goes (test copy built like the C# app writes it, 5.1 GB): `sounds` table 72%, `sound_prints`
26%, indexes + galaxy maps ~3%. Each sounds row is ~4.7 KB but costs ~8 KB on disk (spread over two 4 KB pages):
analysis JSON ~2.3 KB (160 waveform points and the 73 features as text; the features are stored again as a blob;
KeyName/RootNoteName stored though quick to work out), CLAP 512 floats = 2 KB.

**How to apply:** no big caches; the only agreed copy is the compact library below. When Rust writes its library: numbers as numbers, waveform as 160 bytes, features once, big fingerprints (CLAP, prints) in tables of their
own so list rows pack tightly (that also makes the list read fast); try it on a copy first (copy, check, rename).
First told him "sounds part under 1 GB", corrected: ~1.3 GB with CLAP at full precision. See [[rust-rebuild-rules]], [[sounds-backend-status]].

DECIDED (Solomon, 2026-11-03 18:32, card): "Yes, beside the old one": the Rust app builds its own compact sound
library from the old sounds.db (one pass, nothing re-analysed, every tag/star/play/hidden copy/galaxy place/CLAP kept),
leaves the old file untouched for the C# app, refreshes the copy when the C# app changes the old one, and the old file
can go once Rust can scan, print and run CLAP itself. Told him: about 1.3 GB at first (prints not copied until Rust has
its own duplicate finder), about 2.3 GB with prints, vs 4.6 GB. PR 1 (parallel one-pass read of the old file) first.
