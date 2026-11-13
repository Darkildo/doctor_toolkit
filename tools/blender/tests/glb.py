"""A small reader of binary glTF for the round-trip test: the triangles of
every primitive with their positions and first UVs. Standard library only."""

from __future__ import annotations

import json
import struct

COMPONENT = {5120: "b", 5121: "B", 5122: "h", 5123: "H", 5125: "I", 5126: "f"}
COUNT = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}


def read(data: bytes) -> tuple[dict, bytes]:
    magic, version, _ = struct.unpack_from("<4sII", data, 0)
    if magic != b"glTF" or version != 2:
        raise ValueError("not a glTF 2.0 binary")
    offset = 12
    doc, binary = None, b""
    while offset < len(data):
        length, kind = struct.unpack_from("<I4s", data, offset)
        chunk = data[offset + 8 : offset + 8 + length]
        if kind == b"JSON":
            doc = json.loads(chunk)
        elif kind == b"BIN\0":
            binary = chunk
        offset += 8 + length
    return doc, binary


def accessor(doc: dict, binary: bytes, index: int) -> list[tuple]:
    acc = doc["accessors"][index]
    view = doc["bufferViews"][acc["bufferView"]]
    fmt = COMPONENT[acc["componentType"]]
    n = COUNT[acc["type"]]
    size = struct.calcsize(fmt)
    stride = view.get("byteStride") or size * n
    base = view.get("byteOffset", 0) + acc.get("byteOffset", 0)
    out = []
    for i in range(acc["count"]):
        out.append(struct.unpack_from(f"<{n}{fmt}", binary, base + i * stride))
    if acc.get("normalized") and fmt != "f":
        scale = float((1 << (8 * size - (0 if fmt.isupper() else 1))) - 1)
        out = [tuple(max(c / scale, -1.0) for c in v) for v in out]
    return out


def triangles(data: bytes) -> list[tuple[tuple, tuple, tuple, tuple]]:
    """Every triangle as ((p0, p1, p2), (uv0, uv1, uv2), (w0, w1, w2),
    (c0, c1, c2)), mesh by mesh; `w` is a dict of bone name to weight (empty
    without a skin), `c` the vertex colour RGBA (white without colours)."""
    doc, binary = read(data)
    nodes = doc.get("nodes", [])
    skins = doc.get("skins", [])
    skin_of = {n["mesh"]: n.get("skin") for n in nodes if "mesh" in n}
    out = []
    for m, mesh in enumerate(doc.get("meshes", [])):
        skin = skin_of.get(m)
        joint_names = (
            [nodes[j].get("name", str(j)) for j in skins[skin]["joints"]]
            if skin is not None
            else []
        )
        for prim in mesh["primitives"]:
            if prim.get("mode", 4) != 4:
                continue
            attrs = prim["attributes"]
            pos = accessor(doc, binary, attrs["POSITION"])
            uv = (
                accessor(doc, binary, attrs["TEXCOORD_0"])
                if "TEXCOORD_0" in attrs
                else [(0.0, 0.0)] * len(pos)
            )
            if "indices" in prim:
                idx = [i[0] for i in accessor(doc, binary, prim["indices"])]
            else:
                idx = list(range(len(pos)))
            colors = [(1.0, 1.0, 1.0, 1.0)] * len(pos)
            if "COLOR_0" in attrs:
                colors = [(*c, 1.0)[:4] for c in accessor(doc, binary, attrs["COLOR_0"])]
            weights = [{} for _ in pos]
            if joint_names and "JOINTS_0" in attrs and "WEIGHTS_0" in attrs:
                joints = accessor(doc, binary, attrs["JOINTS_0"])
                values = accessor(doc, binary, attrs["WEIGHTS_0"])
                for v, (js, ws) in enumerate(zip(joints, values)):
                    total = sum(ws) or 1.0
                    for j, w in zip(js, ws):
                        if w > 0:
                            name = joint_names[j]
                            weights[v][name] = weights[v].get(name, 0.0) + w / total
            for t in range(0, len(idx) - 2, 3):
                corners = idx[t : t + 3]
                out.append(
                    (
                        tuple(pos[i] for i in corners),
                        tuple(uv[i] for i in corners),
                        tuple(weights[i] for i in corners),
                        tuple(colors[i] for i in corners),
                    )
                )
    return out


def names(data: bytes) -> tuple[list[str], list[str]]:
    """Joint names of the skins and material names."""
    doc, _ = read(data)
    nodes = doc.get("nodes", [])
    joints = [nodes[j].get("name", "") for s in doc.get("skins", []) for j in s["joints"]]
    materials = [m.get("name", "") for m in doc.get("materials", [])]
    return joints, materials


def _corners_match(a: tuple, b: tuple, tolerance: float) -> int:
    """The rotation of `b` whose corners are within `tolerance` of `a`'s, or -1."""
    for r in range(3):
        if all(
            max(abs(x - y) for x, y in zip(a[i], b[(i + r) % 3])) <= tolerance for i in range(3)
        ):
            return r
    return -1


def match(before: list, after: list, tolerance: float = 1e-4):
    """Pairs triangles of `after` with equal ones of `before` (same corners
    within `tolerance`, same winding). Returns (pairs, unmatched before,
    unmatched after); a pair is (old, new, rotation)."""
    cell = 1e-2

    def cell_of(points):
        c = [sum(p[k] for p in points) / 3 for k in range(3)]
        return tuple(int(v // cell) for v in c)

    pool: dict = {}
    for t in before:
        pool.setdefault(cell_of(t[0]), []).append(t)
    pairs, added = [], []
    for t in after:
        cx, cy, cz = cell_of(t[0])
        found = None
        for dx in (0, -1, 1):
            for dy in (0, -1, 1):
                for dz in (0, -1, 1):
                    bucket = pool.get((cx + dx, cy + dy, cz + dz), [])
                    for i, old in enumerate(bucket):
                        r = _corners_match(t[0], old[0], tolerance)
                        if r >= 0:
                            found = (bucket, i, old, r)
                            break
                    if found:
                        break
                if found:
                    break
        if found:
            bucket, i, old, r = found
            bucket.pop(i)
            pairs.append((old, t, r))
        else:
            added.append(t)
    removed = [t for bucket in pool.values() for t in bucket]
    return pairs, removed, added


def deviation(pairs) -> tuple[float, float, float, float]:
    """Largest difference of positions, UVs, bone weights (by bone name) and
    vertex colours over matched triangles."""
    worst_pos = worst_uv = worst_weight = worst_color = 0.0
    for old, new, r in pairs:
        for i in range(3):
            j = (i + r) % 3
            worst_pos = max(worst_pos, *(abs(a - b) for a, b in zip(new[0][i], old[0][j])))
            worst_uv = max(worst_uv, *(abs(a - b) for a, b in zip(new[1][i], old[1][j])))
            a, b = new[2][i], old[2][j]
            for bone in set(a) | set(b):
                worst_weight = max(worst_weight, abs(a.get(bone, 0.0) - b.get(bone, 0.0)))
            worst_color = max(worst_color, *(abs(a - b) for a, b in zip(new[3][i], old[3][j])))
    return worst_pos, worst_uv, worst_weight, worst_color
