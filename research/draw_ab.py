"""Draw calls on and off in one run of the game (stage 101).

`doctor_render.asi` with `draw_ab=N` turns its merging of draw calls off and
on every N seconds and writes a line into `doctor_render.log` for each
half. This script reads such logs and prints, per state, the frame time,
the processor time of the render thread and of the whole process for a
frame and the draws that reached Direct3D: median, least, most, and the
change of every half against the mean of its two neighbours of the other
state (median, quartiles, least and most). Several logs are also pooled.
research/draw_threads.py does the same for the threads of DXVK.

    python3 research/draw_ab.py [--skip N] doctor_render.log [...]

The first N halves (default 2) are left out: the level is still loading.
"""
import re
import statistics
import sys

LINE = re.compile(
    r'draws: (merge on|filter on|off); (\d+) frames in ([\d.]+) s \(([\d.]+) ms a frame\); a frame: (\d+) draws '
    r'\((\d+) to the device, (\d+) joined into (\d+)\), (\d+) primitives, (\d+) calls \((\d+) dropped\); '
    r'render thread ([\d.]+) ms, process ([\d.]+) ms')


def read(path, skip):
    rows = []
    for line in open(path, errors='replace'):
        m = LINE.search(line)
        if m:
            rows.append({'state': 'on' if m.group(1) != 'off' else 'off', 'frames': int(m.group(2)),
                         'frame_ms': float(m.group(4)), 'draws': int(m.group(5)), 'device': int(m.group(6)),
                         'thread_ms': float(m.group(12)), 'process_ms': float(m.group(13))})
    return [r for r in rows[skip:] if r['frames'] >= 20]


def changes(rows, key):
    """Every half against the mean of its neighbours of the other state, %."""
    out = []
    for i in range(1, len(rows) - 1):
        a, b, c = rows[i - 1], rows[i], rows[i + 1]
        if a['state'] == c['state'] != b['state']:
            base = (a[key] + c[key]) / 2
            if base <= 0 or b[key] <= 0:
                continue
            ratio = b[key] / base if b['state'] == 'on' else base / b[key]
            out.append(100 * (ratio - 1))
    return out


def report(title, runs):
    print(title)
    for key, name in (('draws', 'draws of the game'), ('device', 'draws to Direct3D'), ('frame_ms', 'frame, ms'),
                      ('thread_ms', 'render thread, ms'), ('process_ms', 'process, ms')):
        line = '  %-18s' % name
        for s in ('off', 'on'):
            v = [r[key] for rows in runs for r in rows if r['state'] == s]
            line += ' %s: median %8.2f (%.2f–%.2f, n=%d)' % (s, statistics.median(v), min(v), max(v), len(v))
        ch = [c for rows in runs for c in changes(rows, key)]
        if ch:
            q = statistics.quantiles(ch, n=4) if len(ch) >= 4 else [min(ch), statistics.median(ch), max(ch)]
            line += '  on against off: median %+.1f %% (quartiles %+.1f…%+.1f, %+.1f…%+.1f, n=%d)' % (
                statistics.median(ch), q[0], q[2], min(ch), max(ch), len(ch))
        print(line)


def main():
    args = sys.argv[1:]
    skip = 2
    if args[:1] == ['--skip']:
        skip = int(args[1])
        args = args[2:]
    runs = []
    for path in args:
        rows = read(path, skip)
        if len(rows) < 4 or len({r['state'] for r in rows}) < 2:
            print(path)
            print('  too few halves: %d' % len(rows))
            continue
        runs.append(rows)
        report(path, [rows])
    if len(runs) > 1:
        report('all %d logs' % len(runs), runs)


if __name__ == '__main__':
    main()
