"""Which known sound plays in each second of a bench capture.

Usage: audio_match.py capture.wav ref1.wav [ref2.wav ...]
For every loud second of the capture, the normalised cross-correlation peak
against each reference (resampled to the capture rate, mono).
"""
import sys

import numpy as np

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from audio_tones import load  # noqa: E402


def resample(x, src, dst):
    if src == dst:
        return x
    n = int(len(x) * dst / src)
    return np.interp(np.arange(n) * src / dst, np.arange(len(x)), x)


def main():
    cap, rate = load(sys.argv[1])
    refs = []
    for path in sys.argv[2:]:
        x, r = load(path)
        refs.append((path.rsplit("/", 1)[-1], resample(x, r, rate)))
    # 8 kHz, 2-second windows every 4 seconds: fast enough.
    step = rate // 8000
    cap = cap[::step]
    refs = [(n, r[::step]) for n, r in refs]
    rate = rate // step
    win = rate * 2
    summary = {name: 0 for name, _ in refs}
    for i in range(0, len(cap) // win, 2):
        seg = cap[i * win:(i + 1) * win]
        if (seg**2).mean() < 1e-5:
            continue
        seg = seg - seg.mean()
        best = []
        for name, ref in refs:
            n = len(ref) + win
            size = 1 << (n - 1).bit_length()
            corr = np.fft.irfft(np.fft.rfft(ref, size) * np.conj(np.fft.rfft(seg, size)), size)
            corr = corr[: len(ref) - win]
            # Normalise by the energy of the reference window at each lag.
            c = np.concatenate(([0.0], np.cumsum(ref**2)))
            energy = (c[win:] - c[:-win])[: len(corr)]
            score = corr / np.sqrt(energy * (seg**2).sum() + 1e-12)
            k = int(np.abs(score).argmax())
            best.append((abs(score[k]), name, k / rate))
        score, name, at = max(best)
        if score > 0.6:
            summary[name] += 1
        print(f"{i * 2:6.1f}s  best {name} at {at:7.2f}s  r={score:.2f}  " +
              " ".join(f"{n}:{s:.2f}" for s, n, _ in best))
    print("windows matched (r > 0.6):", summary)


main()
