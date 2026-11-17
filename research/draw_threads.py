"""Processor time of the game's threads for each half of a `draw_ab` run (stage 101).

`doctor_render.asi` with `draw_ab=N` writes a line into `doctor_render.log`
at the end of each half (merging on or off). The render thread's own time is
in that line; the threads of DXVK that execute the calls are not. This script
follows the log while the game runs on the bench (Linux, Wine) and at every
new line reads the processor ticks of every thread of the game from /proc,
summed by thread name (`dxvk-cs`, `dxvk-submit`, `Presentation` — the
render thread of the game, `godfather2.exe` — the main thread, …).

    python3 research/draw_threads.py follow SHADOW/scripts/doctor_render.log OUT.tsv
    python3 research/draw_threads.py [--skip N] OUT.tsv [...]

`follow` runs until it is killed; the game is the `godfather2.exe` process
whose folder is the one the log lies in. The second form prints, per thread
name, milliseconds of processor a frame with merging off and on (median,
least, most) and the change of every half against the mean of its two
neighbours of the other state (median and spread), as research/draw_ab.py
does for the numbers of the log; several files are also pooled. The first N
halves (default 4) are left out: the level is still loading.
"""
import os
import re
import statistics
import sys
import time

LINE = re.compile(r'draws: (merge on|filter on|off); (\d+) frames in ([\d.]+) s')
TICK_MS = 1000.0 / os.sysconf('SC_CLK_TCK')


def game_pid(folder):
    folder = os.path.realpath(folder)
    for pid in os.listdir('/proc'):
        if not pid.isdigit():
            continue
        try:
            with open('/proc/%s/cmdline' % pid, 'rb') as f:
                if b'godfather2.exe' not in f.read():
                    continue
            if os.path.realpath('/proc/%s/cwd' % pid) == folder:
                return int(pid)
        except OSError:
            continue
    return None


def ticks(pid):
    out = {}
    try:
        tasks = os.listdir('/proc/%d/task' % pid)
    except OSError:
        return out
    for task in tasks:
        try:
            with open('/proc/%d/task/%s/stat' % (pid, task)) as f:
                text = f.read()
        except OSError:
            continue
        # The name is in brackets and may hold spaces.
        name = text[text.index('(') + 1:text.rindex(')')]
        rest = text[text.rindex(')') + 2:].split()
        out[name] = out.get(name, 0) + int(rest[11]) + int(rest[12])
    return out


def follow(log, out_path):
    folder = os.path.dirname(os.path.dirname(os.path.abspath(log)))
    pid = None
    seen = 0
    before = None
    with open(out_path, 'w') as out:
        while True:
            if pid is None:
                pid = game_pid(folder)
            try:
                with open(log, errors='replace') as f:
                    lines = [line for line in f if LINE.search(line)]
            except OSError:
                lines = []
            if len(lines) > seen and pid is not None:
                now = ticks(pid)
                if not now:
                    return
                if before is not None and len(lines) == seen + 1:
                    m = LINE.search(lines[-1])
                    spent = {k: v - before.get(k, 0) for k, v in now.items() if v - before.get(k, 0) > 0}
                    out.write('%s\t%s\t%s\t%s\n' % (
                        'on' if m.group(1) != 'off' else 'off', m.group(2), m.group(3),
                        ' '.join('%s=%d' % (k.replace(' ', '_'), v) for k, v in sorted(spent.items()))))
                    out.flush()
                before = now
                seen = len(lines)
            time.sleep(0.02)


def read(path, skip):
    rows = []
    for line in open(path):
        state, frames, seconds, threads = line.rstrip('\n').split('\t')
        frames = int(frames)
        row = {'state': state, 'frames': frames}
        total = 0
        for item in threads.split():
            name, value = item.rsplit('=', 1)
            row[name] = int(value) * TICK_MS / max(frames, 1)
            total += int(value)
        row['all threads'] = total * TICK_MS / max(frames, 1)
        rows.append(row)
    return [r for r in rows[skip:] if r['frames'] >= 20]


def changes(rows, key):
    """Every half against the mean of its neighbours of the other state, %."""
    out = []
    for i in range(1, len(rows) - 1):
        a, b, c = rows[i - 1], rows[i], rows[i + 1]
        if a['state'] == c['state'] != b['state']:
            base = (a.get(key, 0.0) + c.get(key, 0.0)) / 2
            if base <= 0 or b.get(key, 0.0) <= 0:
                continue
            ratio = b[key] / base if b['state'] == 'on' else base / b[key]
            out.append(100 * (ratio - 1))
    return out


def report(title, runs):
    print(title)
    names = {}
    for rows in runs:
        for r in rows:
            for k, v in r.items():
                if k not in ('state', 'frames'):
                    names[k] = names.get(k, 0.0) + v
    # The busiest first; names that take under 0.05 ms a frame are left out.
    count = sum(len(rows) for rows in runs) or 1
    for name in sorted(names, key=lambda k: -names[k]):
        if names[name] / count < 0.05:
            continue
        line = '  %-18s' % name
        for s in ('off', 'on'):
            v = [r.get(name, 0.0) for rows in runs for r in rows if r['state'] == s]
            if v:
                line += ' %s: %7.3f (%.3f–%.3f, n=%d)' % (s, statistics.median(v), min(v), max(v), len(v))
        ch = [c for rows in runs for c in changes(rows, name)]
        if ch:
            q = statistics.quantiles(ch, n=4) if len(ch) >= 4 else [min(ch), statistics.median(ch), max(ch)]
            line += '  on against off: median %+.1f %% (quartiles %+.1f…%+.1f, %+.1f…%+.1f, n=%d)' % (
                statistics.median(ch), q[0], q[2], min(ch), max(ch), len(ch))
        print(line)


def main():
    args = sys.argv[1:]
    if args[:1] == ['follow']:
        follow(args[1], args[2])
        return
    skip = 4
    if args[:1] == ['--skip']:
        skip = int(args[1])
        args = args[2:]
    runs = []
    for path in args:
        rows = read(path, skip)
        runs.append(rows)
        report(path, [rows])
    if len(runs) > 1:
        report('all %d files' % len(runs), runs)


if __name__ == '__main__':
    main()
