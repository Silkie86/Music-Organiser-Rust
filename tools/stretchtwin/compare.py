"""Makes the test sound, and says how far two stretched sounds are apart (run.sh). Plain Python 3."""

import math
import random
import struct
import sys

RATE = 44100


def make(path):
    """Two seconds of stereo: a chord, a noisy hit every half second and a kick under it."""
    n = RATE * 2
    random.seed(1)
    left, right = [], []
    for i in range(n):
        t = i / RATE
        tone = (0.2 * math.sin(2 * math.pi * 220 * t) + 0.15 * math.sin(2 * math.pi * 277.18 * t + 0.3)
                + 0.1 * math.sin(2 * math.pi * 329.63 * t))
        beat = i % (RATE // 2)
        hit = 0.6 * math.exp(-beat / 400.0) * random.uniform(-1, 1)
        kick = 0.5 * math.exp(-beat / 3000.0) * math.sin(2 * math.pi * (50 + 80 * math.exp(-beat / 600.0)) * t)
        left.append(tone + hit + kick)
        right.append(0.8 * tone - 0.5 * hit + kick)
    with open(path, "wb") as f:
        f.write(struct.pack("<%df" % n, *left) + struct.pack("<%df" % n, *right))


def nudge(src, dst):
    """The same sound with noise far too quiet to hear (1e-6, 120 dB under it): how far the original moves for
    nothing."""
    with open(src, "rb") as f:
        data = f.read()
    x = struct.unpack("<%df" % (len(data) // 4), data)
    random.seed(5)
    with open(dst, "wb") as f:
        f.write(struct.pack("<%df" % len(x), *[v + random.uniform(-1e-6, 1e-6) for v in x]))


def compare(a, b):
    with open(a, "rb") as f:
        x = f.read()
    with open(b, "rb") as f:
        y = f.read()
    x = struct.unpack("<%df" % (len(x) // 4), x)
    y = struct.unpack("<%df" % (len(y) // 4), y)
    diff = math.sqrt(sum((p - q) ** 2 for p, q in zip(x, y)) / len(x))
    signal = math.sqrt(sum(p * p for p in x) / len(x))
    return 20 * math.log10(diff / signal + 1e-30)


if __name__ == "__main__":
    if sys.argv[1] == "make":
        make(sys.argv[2])
    elif sys.argv[1] == "nudge":
        nudge(sys.argv[2], sys.argv[3])
    else:
        print("%.1f dB" % compare(sys.argv[2], sys.argv[3]))
