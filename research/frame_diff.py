"""Compares screenshots of the game pixel by pixel (stage 101).

    python3 research/frame_diff.py A.png B.png [C.png ...]
    python3 research/frame_diff.py --states [--settled] [--grow N] [--over D] s0_off.png s1_off.png s2_on.png ...

Prints for every pair of neighbours: pixels that differ, their share, the
largest difference of a channel, the mean difference over the differing
pixels, and the box that holds them.

With --states the frames are a sequence of a stopped game stepped a few
frames at a time, the name of each ending in its state (`_on`, `_off`).
What moves by itself (people, trees) differs between neighbours of one
state: those pixels, grown by N (default 6), make a mask. The neighbours of
different states are then compared outside the mask: a setting that keeps
the picture leaves nothing there beyond what pairs of one state leave
(judged by the mask of the other pairs). With --settled only every second
frame is taken (the second of each pair of one state, when the game draws
so slowly that the first may still show the state before); with --over D
differences of a channel up to D count as none (the software renderer of
the bench changes the last bits of most pixels from frame to frame). Used to check that the picture stays
the same with the graphics module's merging of draw calls on and off
(docs/analysis/game-bench.md §38).
"""
import sys

import numpy as np
from PIL import Image


def load(path):
    return np.asarray(Image.open(path).convert('RGB'), dtype=np.int16)


def diff(a, b):
    d = np.abs(a - b).max(axis=2)
    ys, xs = np.nonzero(d)
    n = len(ys)
    if n == 0:
        return {'pixels': 0, 'share': 0.0, 'max': 0, 'mean': 0.0, 'box': None, 'over8': 0}
    return {'pixels': n, 'share': n / d.size, 'max': int(d.max()), 'mean': float(d[ys, xs].mean()),
            'box': (int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max())), 'over8': int((d > 8).sum())}


def grow(mask, n):
    out = mask.copy()
    for _ in range(n):
        m = out.copy()
        m[1:, :] |= out[:-1, :]
        m[:-1, :] |= out[1:, :]
        m[:, 1:] |= out[:, :-1]
        m[:, :-1] |= out[:, 1:]
        out = m
    return out


def states(paths, n, over=0):
    images = [load(p) for p in paths]
    state = [p.rsplit('.', 1)[0].rsplit('_', 1)[-1] for p in paths]
    pairs = list(zip(range(len(paths)), range(1, len(paths))))
    same = [(i, j) for i, j in pairs if state[i] == state[j]]
    cross = [(i, j) for i, j in pairs if state[i] != state[j]]
    if not same:
        # The states alternate (--settled): frames of one state are two apart.
        same = [(i, i + 2) for i in range(len(paths) - 2) if state[i] == state[i + 2]]
    moved = {pair: np.abs(images[pair[0]] - images[pair[1]]).max(axis=2) > over for pair in same}

    def mask_without(left_out):
        mask = np.zeros(images[0].shape[:2], dtype=bool)
        for pair in same:
            if pair != left_out:
                mask |= moved[pair]
        return grow(mask, n)

    whole = mask_without(None)
    print('%d pairs of one state, %d of different states; mask of what moves by itself: %d pixels (%.2f %%)' % (
        len(same), len(cross), int(whole.sum()), 100 * whole.sum() / whole.size))
    # A pair of one state is judged by the mask of the other pairs of one
    # state: what is left is the noise of the method. A pair of different
    # states is judged by the mask of all but one of them, the same size.
    for kind, group in (('one state', same), ('different states', cross)):
        left = []
        for k, (i, j) in enumerate(group):
            mask = mask_without((i, j) if kind == 'one state' else same[k % len(same)])
            d = np.abs(images[i] - images[j]).max(axis=2)
            d[d <= over] = 0
            frame = int((d > 0).sum())
            d[mask] = 0
            ys, xs = np.nonzero(d)
            left.append(len(ys))
            box = (int(xs.min()), int(ys.min()), int(xs.max()), int(ys.max())) if len(ys) else None
            print('  %-16s %-28s whole frame %7d; outside the mask %6d pixels, max %3d, box %s' % (
                kind, '%s : %s' % (paths[i].rsplit('/', 1)[-1], paths[j].rsplit('/', 1)[-1]), frame, len(ys),
                int(d.max()), box))
        if left:
            print('  %-16s outside the mask: median %d, %d…%d pixels' % (
                kind, sorted(left)[len(left) // 2], min(left), max(left)))


def main():
    paths = sys.argv[1:]
    if paths and paths[0] == '--states':
        paths = paths[1:]
        n, over, settled = 6, 0, False
        while paths and paths[0] in ('--grow', '--over', '--settled'):
            if paths[0] == '--settled':
                settled = True
                paths = paths[1:]
                continue
            if paths[0] == '--grow':
                n = int(paths[1])
            else:
                over = int(paths[1])
            paths = paths[2:]
        if settled:
            # Only the second frame of every pair of one state: the state
            # has surely been in force for a few frames.
            paths = [p for i, p in enumerate(paths) if i % 2 == 1]
        states(paths, n, over)
        return
    if len(paths) < 2:
        raise SystemExit(__doc__)
    images = [load(p) for p in paths]
    for (pa, a), (pb, b) in zip(zip(paths, images), zip(paths[1:], images[1:])):
        r = diff(a, b)
        name = '%s : %s' % (pa.rsplit('/', 1)[-1], pb.rsplit('/', 1)[-1])
        if r['pixels'] == 0:
            print('%-40s identical' % name)
        else:
            print('%-40s %7d pixels (%.3f %%), over 8: %d, max %d, mean %.2f, box %s' % (
                name, r['pixels'], 100 * r['share'], r['over8'], r['max'], r['mean'], r['box']))


if __name__ == '__main__':
    main()
