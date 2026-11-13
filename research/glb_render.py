#!/usr/bin/env python3
"""Checks a .glb written by `doctor-toolkit models` and renders it with its base textures.

Usage: glb_render.py <model.glb> <out.png> [yaw_degrees]
"""
import io
import json
import struct
import sys

import numpy as np
from PIL import Image

COMPONENT = {5126: ("<f4", 4), 5125: ("<u4", 4), 5121: ("u1", 1), 5123: ("<u2", 2)}
WIDTH = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}


def load(path):
    data = open(path, "rb").read()
    magic, version, total = struct.unpack_from("<4sII", data, 0)
    assert magic == b"glTF" and version == 2 and total == len(data), "bad GLB header"
    jlen, jtype = struct.unpack_from("<I4s", data, 12)
    assert jtype == b"JSON"
    doc = json.loads(data[20:20 + jlen])
    blen, btype = struct.unpack_from("<I4s", data, 20 + jlen)
    assert btype == b"BIN\0" and blen == doc["buffers"][0]["byteLength"]
    return doc, data[28 + jlen:28 + jlen + blen]


def accessor(doc, binary, index):
    a = doc["accessors"][index]
    v = doc["bufferViews"][a["bufferView"]]
    dtype, size = COMPONENT[a["componentType"]]
    n = WIDTH[a["type"]]
    assert v["byteOffset"] % 4 == 0 and v["byteOffset"] + v["byteLength"] <= len(binary)
    arr = np.frombuffer(binary, dtype=dtype, count=a["count"] * n, offset=v["byteOffset"]).reshape(a["count"], n)
    if "min" in a:
        assert np.allclose(arr.min(0), a["min"], atol=1e-5) and np.allclose(arr.max(0), a["max"], atol=1e-5)
    return arr


def main(path, out, yaw=35.0):
    doc, binary = load(path)
    images = []
    for img in doc.get("images", []):
        v = doc["bufferViews"][img["bufferView"]]
        images.append(np.asarray(Image.open(io.BytesIO(binary[v["byteOffset"]:v["byteOffset"] + v["byteLength"]])).convert("RGB")))
    tris, cols = [], []
    for prim in doc["meshes"][0]["primitives"]:
        pos = accessor(doc, binary, prim["attributes"]["POSITION"])
        idx = accessor(doc, binary, prim["indices"]).reshape(-1, 3)
        assert idx.max() < len(pos)
        if "NORMAL" in prim["attributes"]:
            nrm = accessor(doc, binary, prim["attributes"]["NORMAL"])
            assert np.allclose(np.linalg.norm(nrm, axis=1), 1, atol=1e-3)
        tex = None
        mat = doc["materials"][prim["material"]] if "material" in prim else None
        if mat and "baseColorTexture" in mat.get("pbrMetallicRoughness", {}):
            tex = images[doc["textures"][mat["pbrMetallicRoughness"]["baseColorTexture"]["index"]]["source"]]
        uv = accessor(doc, binary, prim["attributes"]["TEXCOORD_0"]) if "TEXCOORD_0" in prim["attributes"] else None
        for t in idx:
            tris.append(pos[t])
            if tex is not None and uv is not None:
                u, v = uv[t].mean(0)
                h, w, _ = tex.shape
                cols.append(tex[int((v % 1) * (h - 1)), int((u % 1) * (w - 1))])
            else:
                cols.append(np.array([180, 180, 180]))
    tris = np.array(tris)
    cols = np.array(cols, dtype=float)
    a = np.radians(yaw)
    rot = np.array([[np.cos(a), 0, np.sin(a)], [0, 1, 0], [-np.sin(a), 0, np.cos(a)]])
    tilt = np.radians(20)
    rot = np.array([[1, 0, 0], [0, np.cos(tilt), -np.sin(tilt)], [0, np.sin(tilt), np.cos(tilt)]]) @ rot
    p = tris @ rot.T
    n = np.cross(p[:, 1] - p[:, 0], p[:, 2] - p[:, 0])
    n /= np.maximum(np.linalg.norm(n, axis=1, keepdims=True), 1e-12)
    light = np.clip(np.abs(n @ np.array([0.3, 0.8, 0.5])), 0.25, 1)
    size = 700
    lo, hi = p[..., :2].reshape(-1, 2).min(0), p[..., :2].reshape(-1, 2).max(0)
    scale = (size - 20) / max(hi - lo)
    img = np.full((size, size, 3), 40, np.uint8)
    depth = np.full((size, size), -np.inf)
    for tri, col, li in zip(p, cols, light):
        xy = (tri[:, :2] - lo) * scale + 10
        xy[:, 1] = size - xy[:, 1]
        x0, y0 = np.floor(xy.min(0)).astype(int)
        x1, y1 = np.ceil(xy.max(0)).astype(int)
        if x1 - x0 > size or y1 - y0 > size:
            continue
        xs, ys = np.meshgrid(np.arange(max(x0, 0), min(x1 + 1, size)), np.arange(max(y0, 0), min(y1 + 1, size)))
        if xs.size == 0:
            continue
        (ax, ay), (bx, by), (cx, cy) = xy
        den = (by - cy) * (ax - cx) + (cx - bx) * (ay - cy)
        if abs(den) < 1e-9:
            continue
        w0 = ((by - cy) * (xs - cx) + (cx - bx) * (ys - cy)) / den
        w1 = ((cy - ay) * (xs - cx) + (ax - cx) * (ys - cy)) / den
        w2 = 1 - w0 - w1
        inside = (w0 >= 0) & (w1 >= 0) & (w2 >= 0)
        z = w0 * tri[0, 2] + w1 * tri[1, 2] + w2 * tri[2, 2]
        sel = inside & (z > depth[ys, xs])
        depth[ys[sel], xs[sel]] = z[sel]
        img[ys[sel], xs[sel]] = (col * li).astype(np.uint8)
    Image.fromarray(img).save(out)
    print(f"{path}: {len(doc['meshes'][0]['primitives'])} primitive(s), {len(tris)} triangles, "
          f"{len(doc.get('materials', []))} material(s), {len(images)} image(s)")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], float(sys.argv[3]) if len(sys.argv) > 3 else 35.0)
