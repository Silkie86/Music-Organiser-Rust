# Stretch twin check

`mo-stretch` is Signalsmith Stretch (C++, MIT) turned into Rust. `run.sh` checks the two still agree: it fetches the
original at the versions it was turned from, builds it with `twin.cpp`, stretches the same test sound with both
(longer, shorter, pitch moved, formants kept or moved) and says how far apart they are.

They never agree to the last bit: the Fourier transforms round differently, and the stretcher's sums carry small
differences on. So each line also says how far the original moves from itself when the sound is nudged by noise far
too quiet to hear (1e-6, 120 dB under it). Ours should be about as close to the original as that, or closer.

On 26 Nov 2026 (Linux, g++): ours was 82 to 129 dB under the sound for stretching only, and 58 to 71 dB under it with
the pitch moved; the nudged original was 59 to 111 dB and 59 to 70 dB under. With the pitch moved, the worst places were
the same short moments in both, where a peak in the spectrum sits right on the line between counting and not.

Not run by CI (it needs a C++ compiler and the network). Run it after changing anything in `mo-stretch` that should
keep it the same as the original: `tools/stretchtwin/run.sh`.
