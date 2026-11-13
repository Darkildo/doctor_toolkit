"""Looks for pure tones in a capture of the bench (AUDIO_CAPTURE=1) (raw s32 stereo 48 kHz WAV
written by ALSA's file plugin; header sizes may be unfinished).

Usage: audio_tones.py capture.wav freq_hz [freq_hz ...]
Prints, per second of capture, the share of energy within +-5 Hz of each
frequency, and a summary.
"""
import sys

import numpy as np


def load(path):
    raw = open(path, "rb").read()
    # Find the fmt chunk for the sample format.
    fmt_at = raw.find(b"fmt ")
    channels = int.from_bytes(raw[fmt_at + 10:fmt_at + 12], "little")
    rate = int.from_bytes(raw[fmt_at + 12:fmt_at + 16], "little")
    bits = int.from_bytes(raw[fmt_at + 22:fmt_at + 24], "little")
    tag = int.from_bytes(raw[fmt_at + 8:fmt_at + 10], "little")
    data_at = raw.find(b"data") + 8
    body = raw[data_at:]
    if bits == 32 and tag == 3:
        x = np.frombuffer(body[: len(body) // 4 * 4], dtype="<f4").astype(np.float64)
    elif bits == 32:
        x = np.frombuffer(body[: len(body) // 4 * 4], dtype="<i4").astype(np.float64) / 2**31
    else:
        x = np.frombuffer(body[: len(body) // 2 * 2], dtype="<i2").astype(np.float64) / 2**15
    x = x[: len(x) // channels * channels].reshape(-1, channels).mean(axis=1)
    return x, rate


def main():
    x, rate = load(sys.argv[1])
    freqs = [float(f) for f in sys.argv[2:]]
    win = rate
    hits = {f: 0 for f in freqs}
    loud = 0
    rows = []
    for i in range(len(x) // win):
        seg = x[i * win:(i + 1) * win]
        energy = float((seg**2).sum())
        if energy / win < 1e-6:
            continue
        loud += 1
        spec = np.abs(np.fft.rfft(seg * np.hanning(win))) ** 2
        f_axis = np.fft.rfftfreq(win, 1 / rate)
        total = spec.sum()
        shares = []
        for f in freqs:
            band = (f_axis > f - 5) & (f_axis < f + 5)
            share = spec[band].sum() / total
            shares.append(share)
            if share > 0.5:
                hits[f] += 1
        peak = f_axis[spec.argmax()]
        rows.append((i, energy / win, peak, shares))
    for i, e, peak, shares in rows:
        print(f"{i:4d}s rms {e ** 0.5:.3f} peak {peak:7.1f} Hz " +
              " ".join(f"{f:.0f}Hz:{s:.2f}" for f, s in zip(freqs, shares)))
    print(f"seconds with sound: {loud}")
    for f in freqs:
        print(f"seconds where {f:.0f} Hz holds > 50% of the energy: {hits[f]}")


if __name__ == "__main__":
    main()
