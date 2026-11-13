#!/usr/bin/env python3
"""Reads the memory of the game run by the bench, to find where the player is.

The kernel lets a process read the memory of its descendants only
(Yama ptrace_scope 1), so the bench starts the game through this script:

    gamemem.py serve SOCKET -- wine godfather2.exe

`serve` forks, the child becomes the game, and the parent answers requests
on the Unix socket SOCKET until the game exits. Requests (`gamemem.py ask
SOCKET OP [ARGS...]`, one per connection) are:

    scan X,Y,Z RADIUS   world matrices whose translation lies within RADIUS
                        of X,Y,Z, as "address x y z uy" lines
    read ADDRESS...     the matrices at these addresses, "address x y z
                        fx fy fz uy" (fx fy fz: the third row, the forward
                        axis; uy: y of the second row, 1 for an upright one)

`gamemem.py pick BEFORE AFTER` compares a scan with a read of the same
addresses after the bench walked forward and prints the matrices that moved:
"address kind distance sign x y z". Matrices that moved are grouped by their new
position (one object has several copies: render, physics, animation); kind
`player` is the largest group of upright matrices that moved along their
forward axis (sign: +1 when the player faces +row 3, -1 when -row 3),
`camera` the largest group of tilted ones, `other` the rest.

A world matrix of the game is 16 floats, 16-byte aligned, rows: right, up,
forward, translation; the fourth column is 0, 0, 0, 1 (`godfather2.exe`
0x00550d30 reads the player's position at +0x30, +0x34, +0x38 of such a
matrix). The player is the matrix that moves forward when the bench walks
(`hold w`): `track` in tools/game-bench.sh compares two scans.

`gamemem.py find SOCKET X,Y,Z TOLERANCE FILE` writes the upright matrices at
a known position of the player (where a save puts him: loading the same save
puts him at the same place to the millimetre) to FILE; `gamemem.py where
SOCKET FILE` prints the player's position and facing from FILE.

Only memory is read; nothing is written to the game.
"""

import json
import math
import os
import signal
import socket
import struct
import sys

try:
    import numpy as np
except ImportError:  # pragma: no cover - the bench checks for numpy
    np = None


def regions(pid):
    """Readable, writable private regions of PID (heaps and data)."""
    with open(f"/proc/{pid}/maps") as maps:
        for line in maps:
            parts = line.split()
            start, end = (int(v, 16) for v in parts[0].split("-"))
            perms = parts[1]
            path = parts[5] if len(parts) > 5 else ""
            if not perms.startswith("rw") or path.startswith("/dev/") or path == "[vvar]":
                continue
            # The game is a 32-bit process: Wine's own 64-bit areas are not
            # where its objects live.
            if start >= 1 << 32 and not os.environ.get("GAMEMEM_ALL"):
                continue
            yield start, end


def read(mem, addr, size):
    try:
        mem.seek(addr)
        return mem.read(size)
    except OSError:
        return b""


def is_matrix(m):
    """A world matrix: unit, orthogonal rows, last column 0, 0, 0, 1."""
    if abs(m[3]) > 1e-6 or abs(m[7]) > 1e-6 or abs(m[11]) > 1e-6 or abs(m[15] - 1) > 1e-6:
        return False
    rows = [m[0:3], m[4:7], m[8:11]]
    for r in rows:
        if abs(sum(v * v for v in r) - 1) > 1e-3:
            return False
    for a, b in ((0, 1), (0, 2), (1, 2)):
        if abs(sum(x * y for x, y in zip(rows[a], rows[b]))) > 1e-3:
            return False
    return all(abs(v) < 1e5 for v in m[12:15])


def scan(pid, centre, radius):
    """Addresses of world matrices whose translation is near CENTRE."""
    cx, cy, cz = centre
    found = []
    chunk = 16 << 20
    with open(f"/proc/{pid}/mem", "rb", buffering=0) as mem:
        for start, end in regions(pid):
            addr = start
            while addr < end:
                size = min(chunk, end - addr)
                data = read(mem, addr, size)
                base = addr
                addr += size
                if len(data) < 64:
                    continue
                n = len(data) // 16 * 16
                f = np.frombuffer(data[:n], dtype="<f4").reshape(-1, 4)
                # One 16-byte row per line: row i is a translation row when
                # the fourth floats of rows i-3..i are 0, 0, 0, 1.
                w = f[:, 3]
                last = np.nonzero(w == 1.0)[0]
                last = last[last >= 3]
                if last.size == 0:
                    continue
                ok = (w[last - 1] == 0) & (w[last - 2] == 0) & (w[last - 3] == 0)
                last = last[ok]
                t = f[last, :3]
                d2 = (t[:, 0] - cx) ** 2 + (t[:, 1] - cy) ** 2 + (t[:, 2] - cz) ** 2
                last = last[d2 <= radius * radius]
                for row in last:
                    m = f[row - 3 : row + 1].reshape(-1).tolist()
                    if is_matrix(m):
                        found.append((base + (row - 3) * 16, m[12], m[13], m[14], m[5]))
    return found


def read_matrices(pid, addrs):
    out = []
    with open(f"/proc/{pid}/mem", "rb", buffering=0) as mem:
        for a in addrs:
            data = read(mem, a, 64)
            if len(data) != 64:
                out.append((a, None))
                continue
            m = struct.unpack("<16f", data)
            out.append((a, m if is_matrix(m) else None))
    return out


def handle(pid, request):
    op = request[0]
    if op == "scan":
        centre = [float(v) for v in request[1].split(",")]
        radius = float(request[2]) if len(request) > 2 else 30.0
        return "".join(f"{a:#x} {x:.3f} {y:.3f} {z:.3f} {uy:.4f}\n" for a, x, y, z, uy in scan(pid, centre, radius))
    if op == "read":
        lines = []
        for a, m in read_matrices(pid, [int(v, 0) for v in request[1:]]):
            if m is None:
                lines.append(f"{a:#x} none\n")
            else:
                lines.append(
                    f"{a:#x} {m[12]:.3f} {m[13]:.3f} {m[14]:.3f} {m[8]:.4f} {m[9]:.4f} {m[10]:.4f} {m[5]:.4f}\n"
                )
        return "".join(lines)
    return f"error: unknown request {op}\n"


def serve(path, command):
    child = os.fork()
    if child == 0:
        os.execvp(command[0], command)
    # Socket paths are limited to 108 bytes: bind the name in its directory.
    os.chdir(os.path.dirname(os.path.abspath(path)))
    path = os.path.basename(path)
    if os.path.exists(path):
        os.unlink(path)
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(path)
    server.listen(4)
    server.settimeout(1.0)

    def forward(sig, _frame):
        os.kill(child, sig)

    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, forward)
    status = 0
    try:
        while True:
            done, st = os.waitpid(child, os.WNOHANG)
            if done:
                status = os.waitstatus_to_exitcode(st)
                break
            try:
                conn, _ = server.accept()
            except socket.timeout:
                continue
            with conn:
                request = json.loads(conn.makefile().readline())
                try:
                    reply = handle(child, request)
                except Exception as e:  # report, keep serving
                    reply = f"error: {e}\n"
                conn.sendall(reply.encode())
    finally:
        server.close()
        if os.path.exists(path):
            os.unlink(path)
    sys.exit(max(status, 0))


def pick(before_path, after_path):
    before = {}
    for line in open(before_path):
        p = line.split()
        if len(p) >= 4:
            uy = float(p[4]) if len(p) > 4 else 1.0
            before[p[0]] = (float(p[1]), float(p[2]), float(p[3]), uy)
    groups = {}
    for line in open(after_path):
        p = line.split()
        if len(p) != 8 or p[0] not in before:
            continue
        x, y, z, fx, fy, fz, uy = (float(v) for v in p[1:])
        bx, by, bz, buy = before[p[0]]
        dx, dz = x - bx, z - bz
        dist = (dx * dx + dz * dz) ** 0.5
        if not 0.3 < dist < 30:
            continue
        fl = (fx * fx + fz * fz) ** 0.5
        align = (dx * fx + dz * fz) / (dist * fl) if fl > 1e-6 else 0
        # The player's root matrix is upright; the camera (tilted) and the
        # bones are not. Copies of one matrix (render, physics, animation)
        # share the position: the largest group of copies is the player.
        upright = uy > 0.9995 and buy > 0.9995
        kind = "upright" if upright and abs(align) > 0.9 else "tilted"
        # Copies are written in different frames: within 0.75 m on the
        # ground and 0.3 m in height they are one object.
        key = next(
            (
                k
                for k in groups
                if k[0] == kind and (k[1] - x) ** 2 + (k[3] - z) ** 2 < 0.75**2 and abs(k[2] - y) < 0.3
            ),
            (kind, x, y, z),
        )
        groups.setdefault(key, []).append((p[0], dist, 1 if align > 0 else -1, x, y, z))
    order = sorted(groups.items(), key=lambda kv: (kv[0][0] != "upright", -len(kv[1])))
    names = {"upright": "player", "tilted": "camera"}
    for key, members in order:
        kind = names.pop(key[0], "other")
        for a, dist, sign, x, y, z in members:
            print(f"{a} {kind} {dist:.2f} {sign} {x:.3f} {y:.3f} {z:.3f}")


def request(path, words):
    """Sends one request to the server at PATH and returns the reply."""
    os.chdir(os.path.dirname(os.path.abspath(path)))
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
        s.settimeout(120)
        s.connect(os.path.basename(path))
        s.sendall((json.dumps(words) + "\n").encode())
        chunks = []
        while True:
            b = s.recv(65536)
            if not b:
                break
            chunks.append(b)
    return b"".join(chunks).decode()


def find(path, centre, tolerance, out):
    """Upright matrices within TOLERANCE of CENTRE (a known position of the
    player, e.g. where a save puts him) -> OUT as "address sign x y z"."""
    lines = []
    for line in request(path, ["scan", centre, tolerance]).splitlines():
        p = line.split()
        if len(p) == 5 and float(p[4]) > 0.9995:
            lines.append(f"{p[0]} 1 {p[1]} {p[2]} {p[3]}\n")
    with open(out, "w") as f:
        f.writelines(lines)
    print(len(lines))


def where(path, addr_file):
    """Position and facing of the player from the matrices of ADDR_FILE
    ("address sign x y z", by `find` or `track`): "x y z dx dz yaw".

    Some of the matrices are copies that stay where the player was found;
    once some have moved, only those are kept (ADDR_FILE is rewritten).
    The answer is the largest group of matrices at one place.
    """
    rows = [line.split() for line in open(addr_file) if line.strip()]
    if not rows:
        sys.exit("gamemem: no matrices to read")
    reply = request(path, ["read"] + [r[0] for r in rows]).splitlines()
    live = []
    for r, line in zip(rows, reply):
        p = line.split()
        if len(p) != 8 or float(p[7]) < 0.9995:
            continue
        x, y, z = float(p[1]), float(p[2]), float(p[3])
        d2 = (x - float(r[2])) ** 2 + (y - float(r[3])) ** 2 + (z - float(r[4])) ** 2 if len(r) >= 5 else 0
        # Memory reused by something else jumps far away: not the player.
        if d2 > 20.0**2:
            continue
        live.append((r[0], int(r[1]), x, y, z, float(p[4]), float(p[6]), d2 > 0.05**2))
    if not live:
        sys.exit("gamemem: the matrices are gone (`track` or `find` again)")

    def biggest(ms):
        groups = []
        for m in ms:
            for g in groups:
                if (g[0][2] - m[2]) ** 2 + (g[0][4] - m[4]) ** 2 < 0.75**2:
                    g.append(m)
                    break
            else:
                groups.append([m])
        return max(groups, key=len) if groups else []

    # The player has many copies (render, physics, animation) that move
    # together; one or two matrices that moved are memory reused by
    # something else.
    moved = biggest([m for m in live if m[7]])
    still = biggest([m for m in live if not m[7]])
    best = moved if len(moved) >= 3 or not still else still
    with open(addr_file, "w") as f:
        for a, sign, x, y, z, *_ in best:
            f.write(f"{a} {sign} {x:.3f} {y:.3f} {z:.3f}\n")
    a, sign, x, y, z, fx, fz, _ = best[0]
    dx, dz = sign * fx, sign * fz
    n = (dx * dx + dz * dz) ** 0.5 or 1
    dx, dz = dx / n, dz / n
    print(f"{x:.3f} {y:.3f} {z:.3f} {dx:.4f} {dz:.4f} {math.degrees(math.atan2(dx, dz)):.1f}")


def main():
    if len(sys.argv) >= 4 and sys.argv[1] == "serve" and "--" in sys.argv:
        i = sys.argv.index("--")
        if np is None:
            sys.exit("gamemem: numpy is required")
        serve(sys.argv[2], sys.argv[i + 1 :])
    elif len(sys.argv) == 4 and sys.argv[1] == "pick":
        pick(sys.argv[2], sys.argv[3])
    elif len(sys.argv) >= 4 and sys.argv[1] == "ask":
        sys.stdout.write(request(sys.argv[2], sys.argv[3:]))
    elif len(sys.argv) == 6 and sys.argv[1] == "find":
        find(sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5])
    elif len(sys.argv) == 4 and sys.argv[1] == "where":
        where(sys.argv[2], sys.argv[3])
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
