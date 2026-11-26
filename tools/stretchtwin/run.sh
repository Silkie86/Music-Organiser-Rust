#!/bin/sh
# Checks mo-stretch against the original Signalsmith Stretch (C++), on Linux or a Mac with git, a C++ compiler and
# Python 3. Not run by CI. Usage: tools/stretchtwin/run.sh [work folder]
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
work=${1:-$(mktemp -d)}
mkdir -p "$work"
cd "$work"
# The versions mo-stretch was turned from: Stretch 1.3.2 (a commit: it has no tag of its own) and Linear 0.6.6.
if [ ! -d signalsmith-stretch ]; then
	git init -q signalsmith-stretch
	git -C signalsmith-stretch fetch -q --depth 1 https://github.com/Signalsmith-Audio/signalsmith-stretch.git 88061fd4eda56a2353c5d65c74aa3f4ccf76f9da
	git -C signalsmith-stretch checkout -q FETCH_HEAD
fi
[ -d signalsmith-linear ] || git clone -q --depth 1 --branch 0.6.6 https://github.com/Signalsmith-Audio/linear.git signalsmith-linear
c++ -O2 -std=c++17 -I"$work/signalsmith-stretch" -I"$work" "$here/twin.cpp" -o twin
cargo build -q --release --manifest-path "$root/Cargo.toml" -p mo-stretch --example raw
ours="$root/target/release/examples/raw"
python3 "$here/compare.py" make in.raw
python3 "$here/compare.py" nudge in.raw nudged.raw
# The same length, longer, shorter, much longer; pitch up, pitch down keeping the formants, pitch and formants.
for case in "88200 0 0 0" "110250 0 0 0" "70560 0 0 0" "170000 0 0 0" "88200 3 0 0" "88200 -5 0 1" "132300 2 4 0"; do
	set -- $case
	./twin in.raw 2 44100 88200 "$1" "$2" "$3" "$4" theirs.raw
	./twin nudged.raw 2 44100 88200 "$1" "$2" "$3" "$4" nudged-theirs.raw
	"$ours" in.raw 2 44100 88200 "$1" "$2" "$3" "$4" ours.raw
	echo "out $1, semitones $2, formants $3, keep formants $4: ours vs theirs $(python3 "$here/compare.py" diff theirs.raw ours.raw), theirs vs theirs nudged $(python3 "$here/compare.py" diff theirs.raw nudged-theirs.raw)"
done
