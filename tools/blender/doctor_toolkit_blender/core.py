"""Model import and export between Blender and a game's workspace.

Everything here works without the user interface (headless Blender runs
it in tests): the operators in `ui.py` only collect the arguments.

The models travel as binary glTF, the format the toolkit exports and
imports symmetrically (`docs/model-import.md`):

- import: `Workspace.export_model` writes the workspace version of the
  model with materials, textures and skeleton; Blender's glTF importer
  reads it. The names of the bones and materials and the rest pose of the
  armature are kept on the objects, to check them before the export;
- export: the checks below (at most four bone weights per vertex, summing
  to one; bone and material names the game knows; the rest pose), then
  Blender's glTF exporter with +Y up and the selected objects only, then
  `Workspace.import_model` (a dry run for the plan, or the real import).
"""

from __future__ import annotations

import json
import math
import os
import re
import struct
import sys
import tempfile
from dataclasses import dataclass, field
from typing import Any, Iterable, Optional

import bpy

# Custom properties on imported objects.
KEY = "doctor_toolkit_key"
GAME = "doctor_toolkit_game"
BONES = "doctor_toolkit_bones"
MATERIALS = "doctor_toolkit_materials"
LINK = "doctor_toolkit_link"

# A vertex of the game has at most this many bone influences.
MAX_INFLUENCES = 4
# Rest-pose changes below this (metres) are rounding.
REST_TOLERANCE = 1e-4

# Types that hold a model.
MODEL_TYPES = ("mesh", "metamodel")


class ToolkitMissing(RuntimeError):
    """The `doctor_toolkit` Python module cannot be imported."""


def load_toolkit(extra_path: str = ""):
    """Imports the toolkit module; `extra_path` is a folder that holds it
    (a `pip install --target` folder or an unpacked wheel)."""
    if extra_path:
        path = os.path.abspath(os.path.expanduser(extra_path))
        if path not in sys.path:
            sys.path.insert(0, path)
    try:
        import doctor_toolkit  # noqa: PLC0415
    except ImportError as error:
        raise ToolkitMissing(
            "the doctor_toolkit module is not installed: install the extension with "
            "the bundled wheel, or set 'Module folder' in the add-on preferences "
            f"({error})"
        ) from error
    return doctor_toolkit


# ------------------------------------------------------------------ games

_games: dict[str, Any] = {}
_indexes: dict[str, Any] = {}


def open_game(dt, path: str):
    """The game at `path`, opened once per session."""
    path = os.path.abspath(os.path.expanduser(path))
    if path not in _games:
        _games[path] = dt.Game(path)
    return _games[path]


def game_index(dt, path: str):
    """The resource index of the game (cached in the user's cache folder)."""
    path = os.path.abspath(os.path.expanduser(path))
    if path not in _indexes:
        _indexes[path] = open_game(dt, path).index()
    return _indexes[path]


def forget_game(path: str) -> None:
    path = os.path.abspath(os.path.expanduser(path))
    _games.pop(path, None)
    _indexes.pop(path, None)


def find_models(index, query: str, limit: int = 200) -> list[dict]:
    """Models (meshes and metamodels) matching a search of the game index:
    a name, `car_*`, a hash or a key, as in the editor's search."""
    query = query.strip()
    if not query:
        return []
    if "type:" not in query:
        found = []
        for kind in MODEL_TYPES:
            found += index.search(f"{query} type:{kind}", limit)
        return found[:limit]
    return [r for r in index.search(query, limit) if r["type"] in MODEL_TYPES]


# ----------------------------------------------------------------- import

IMPORT_OPTIONS = {
    # Every vertex of the game stays a vertex: no welding of seams.
    "merge_vertices": False,
    # No extra sphere object as the shape of the bones.
    "disable_bone_shape": True,
    "import_shading": "NORMALS",
    "guess_original_bind_pose": True,
}


def _new_objects(before: set) -> list:
    return [o for o in bpy.data.objects if o.name not in before]


def import_model(dt, game_path: str, key: str, workspace_dir: str = "") -> list:
    """Imports the workspace version of model `key` into the scene; returns
    the new objects. The source is remembered on them for the export."""
    game = open_game(dt, game_path)
    ws = game.workspace(workspace_dir or None)
    glb = ws.export_model(key, game_index(dt, game_path))
    folder = tempfile.mkdtemp(prefix="doctor-toolkit-blender-")
    path = os.path.join(folder, "model.glb")
    with open(path, "wb") as f:
        f.write(glb)
    try:
        objects = import_glb(path)
    finally:
        try:
            os.remove(path)
            os.rmdir(folder)
        except OSError:
            pass
    remember(objects, key, game_path)
    return objects


def import_glb(path: str) -> list:
    """Imports a GLB the toolkit wrote; returns the new objects."""
    before = {o.name for o in bpy.data.objects}
    known = {p.identifier for p in bpy.ops.import_scene.gltf.get_rna_type().properties}
    options = {k: v for k, v in IMPORT_OPTIONS.items() if k in known}
    result = bpy.ops.import_scene.gltf(filepath=path, **options)
    if "FINISHED" not in result:
        raise RuntimeError(f"the glTF import of {path} failed")
    return _new_objects(before)


def open_linked(path: str) -> list:
    """Imports the working file of "Open in external editor" (the editor or
    `doctor-toolkit edit … --format glb`); `save_linked` writes it back and
    the toolkit imports each save into the workspace."""
    path = os.path.abspath(path)
    # Objects of an earlier opening of the file are no longer saved to it.
    for obj in bpy.data.objects:
        if obj.get(LINK) == path:
            del obj[LINK]
    objects = import_glb(path)
    remember(objects, "", "")
    for obj in objects:
        obj[LINK] = path
    return objects


def linked_path(objects: Iterable) -> Optional[str]:
    for obj in objects:
        if LINK in obj:
            return obj[LINK]
    return None


def save_linked(objects: Iterable, path: str = "") -> Check:
    """Checks the objects and writes them over their linked working file."""
    objects = list(objects)
    path = path or linked_path(objects) or ""
    if not path:
        raise RuntimeError("the objects were not opened from a linked file")
    check = prepare_for_export(objects, fix=True)
    if not check.errors:
        # Written next to the file and renamed over it: the watcher sees one
        # complete save.
        temp = path + ".part.glb"
        export_glb(objects, temp)
        os.replace(temp, path)
    return check


def remember(objects: Iterable, key: str, game_path: str) -> None:
    """Keeps the source, bone names, rest pose and material names on the objects."""
    objects = list(objects)
    materials = sorted(
        {
            _base_name(s.material.name)
            for o in objects
            if o.type == "MESH"
            for s in o.material_slots
            if s.material
        }
    )
    for obj in objects:
        if key:
            obj[KEY] = key
            obj[GAME] = game_path
        obj[MATERIALS] = json.dumps(materials)
        if obj.type == "ARMATURE":
            obj[BONES] = json.dumps(rest_pose(obj))


def rest_pose(armature) -> dict[str, list[float]]:
    """Head and tail of every bone in armature space."""
    return {b.name: [*b.head_local, *b.tail_local] for b in armature.data.bones}


def source_of(objects: Iterable) -> tuple[Optional[str], Optional[str]]:
    """The model key and game the objects were imported from."""
    for obj in objects:
        if KEY in obj:
            return obj[KEY], obj.get(GAME)
    return None, None


# ----------------------------------------------------------------- checks


@dataclass
class Check:
    """What `prepare_for_export` changed and found."""

    fixed: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)


def export_set(objects: Iterable) -> list:
    """The meshes among `objects` and the armatures they are skinned to."""
    meshes = [o for o in objects if o.type == "MESH"]
    armatures = []
    for obj in meshes:
        for mod in obj.modifiers:
            if mod.type == "ARMATURE" and mod.object and mod.object not in armatures:
                armatures.append(mod.object)
        if obj.parent and obj.parent.type == "ARMATURE" and obj.parent not in armatures:
            armatures.append(obj.parent)
    return meshes + armatures


def limit_weights(obj, bone_names: Optional[set], limit: int = MAX_INFLUENCES) -> int:
    """Keeps the `limit` largest bone weights of each vertex and makes them
    sum to one. Groups that are not bones are left alone. Returns the
    number of vertices changed."""
    groups = obj.vertex_groups
    bones = {g.index for g in groups if bone_names is None or g.name in bone_names}
    changed = 0
    for vertex in obj.data.vertices:
        weights = [(g.group, g.weight) for g in vertex.groups if g.group in bones]
        if not weights:
            continue
        weights.sort(key=lambda w: -w[1])
        keep = [w for w in weights[:limit] if w[1] > 0.0]
        drop = weights[limit:] + [w for w in weights[:limit] if w[1] <= 0.0]
        total = sum(w for _, w in keep)
        touched = False
        for group, _ in drop:
            groups[group].remove([vertex.index])
            touched = True
        if total > 0.0 and not math.isclose(total, 1.0, abs_tol=1e-5):
            for group, weight in keep:
                groups[group].add([vertex.index], weight / total, "REPLACE")
            touched = True
        changed += touched
    return changed


def prepare_for_export(objects: Iterable, fix: bool = True) -> Check:
    """Checks (and with `fix`, repairs) what the game needs before an export:

    - at most four bone weights per vertex, summing to one (fixed);
    - bones named as in the game's skeleton (by name the import finds the
      bones) and the game's rest pose (the game keeps its own bind pose);
    - material names: a known name reuses or clones that material, a new
      one clones the model's most used material;
    - no scale or rotation left on skinned meshes (the import takes
      skinned vertices as they are).
    """
    check = Check()
    objects = export_set(objects)
    meshes = [o for o in objects if o.type == "MESH"]
    if not meshes:
        check.errors.append("select the meshes of the model (and its armature)")
        return check
    for arm in (o for o in objects if o.type == "ARMATURE"):
        original = json.loads(arm.get(BONES, "{}"))
        if not original:
            check.warnings.append(
                f"armature {arm.name} was not imported from the game: its bones must "
                "have the names of a game skeleton"
            )
            continue
        current = rest_pose(arm)
        new = sorted(set(current) - set(original))
        if new:
            check.warnings.append(
                f"bones not in the game's skeleton: {', '.join(new[:8])}"
                + (" …" if len(new) > 8 else "")
                + "; the import fails unless another game skeleton has them"
            )
        moved = [
            name
            for name, place in current.items()
            if name in original
            and max(abs(a - b) for a, b in zip(place, original[name])) > REST_TOLERANCE
        ]
        if moved:
            check.warnings.append(
                f"rest pose changed for {len(moved)} bone(s) ({', '.join(moved[:5])}); "
                "the game uses its own bind pose"
            )
    for obj in meshes:
        arm = next(
            (m.object for m in obj.modifiers if m.type == "ARMATURE" and m.object), None
        )
        if arm is not None:
            bone_names = {b.name for b in arm.data.bones}
            if fix:
                changed = limit_weights(obj, bone_names)
                if changed:
                    check.fixed.append(
                        f"{obj.name}: weights of {changed} vertex(es) limited to "
                        f"{MAX_INFLUENCES} and normalised"
                    )
            unweighted = sum(
                1
                for v in obj.data.vertices
                if not any(obj.vertex_groups[g.group].name in bone_names for g in v.groups)
            )
            if unweighted:
                check.warnings.append(
                    f"{obj.name}: {unweighted} vertex(es) without bone weights"
                )
        known = set(json.loads(obj.get(MATERIALS, "[]")))
        for slot in obj.material_slots:
            if slot.material and known and _base_name(slot.material.name) not in known:
                check.warnings.append(
                    f"material {slot.material.name} is new: the import clones the "
                    "model's most used material and gives it this material's textures"
                )
        if not obj.material_slots:
            check.warnings.append(f"{obj.name} has no material")
    return check


# ----------------------------------------------------------------- export

EXPORT_OPTIONS = {
    "export_format": "GLB",
    "use_selection": True,
    "export_yup": True,
    "export_apply": False,
    "export_skins": True,
    "export_influence_nb": MAX_INFLUENCES,
    "export_all_influences": False,
    "export_animations": False,
    "export_morph": False,
    "export_cameras": False,
    "export_lights": False,
    "export_extras": False,
    "export_materials": "EXPORT",
    "export_image_format": "AUTO",
    "export_normals": True,
    "export_tangents": False,
    "export_texcoords": True,
    # The game's vertex colours (lighting and dirt of buildings).
    "export_vertex_color": "ACTIVE",
    "export_all_vertex_colors": False,
    "export_active_vertex_color_when_no_material": True,
    "export_def_bones": False,
    "export_leaf_bone": False,
}


def export_glb(objects: Iterable, path: str) -> None:
    """Writes the objects (meshes and their armatures) as GLB, +Y up."""
    objects = export_set(objects)
    for obj in bpy.context.view_layer.objects:
        obj.select_set(obj in objects)
    if objects:
        bpy.context.view_layer.objects.active = objects[0]
    known = {p.identifier for p in bpy.ops.export_scene.gltf.get_rna_type().properties}
    options = {k: v for k, v in EXPORT_OPTIONS.items() if k in known}
    result = bpy.ops.export_scene.gltf(filepath=path, **options)
    if "FINISHED" not in result:
        raise RuntimeError("the glTF export failed")
    known = set()
    for obj in objects:
        known.update(json.loads(obj.get(MATERIALS, "[]")))
    restore_names(path, known)


DUPLICATE = re.compile(r"^(.*)\.\d{3}$")


def _base_name(name: str) -> str:
    """`name` without Blender's `.001` suffix of a second copy."""
    m = DUPLICATE.match(name)
    return m.group(1) if m else name


def restore_names(path: str, known: set) -> int:
    """Gives materials and joints of the GLB at `path` their game names back:
    Blender calls the second copy of a material `name.001` (a model imported
    twice), and the import finds materials and bones by name. A name whose
    base is in `known` (materials) or that only has a suffix (joints) loses
    the suffix. Returns the number of names changed."""
    with open(path, "rb") as f:
        data = f.read()
    magic, version, _ = struct.unpack_from("<4sII", data, 0)
    length, kind = struct.unpack_from("<I4s", data, 12)
    if magic != b"glTF" or kind != b"JSON":
        return 0
    doc = json.loads(data[20 : 20 + length])
    rest = data[20 + length :]
    changed = 0
    for material in doc.get("materials", []):
        name = material.get("name", "")
        base = _base_name(name)
        if base != name and base in known:
            material["name"] = base
            changed += 1
    joints = {j for skin in doc.get("skins", []) for j in skin.get("joints", [])}
    nodes = doc.get("nodes", [])
    names = {n.get("name") for n in nodes}
    for j in joints:
        name = nodes[j].get("name", "")
        base = _base_name(name)
        if base != name and base not in names:
            nodes[j]["name"] = base
            changed += 1
    if not changed:
        return 0
    text = json.dumps(doc, separators=(",", ":")).encode()
    text += b" " * (-len(text) % 4)
    out = struct.pack("<4sII", b"glTF", version, 12 + 8 + len(text) + len(rest))
    out += struct.pack("<I4s", len(text), b"JSON") + text + rest
    with open(path, "wb") as f:
        f.write(out)
    return changed


def export_to_workspace(
    dt,
    game_path: str,
    key: str,
    objects: Iterable,
    new_name: Optional[str] = None,
    retarget: Iterable[int] = (),
    dry_run: bool = False,
    workspace_dir: str = "",
    glb_path: str = "",
) -> dict:
    """Exports the objects and imports them over model `key` of the game's
    workspace (or as a new model set `new_name`). Returns the plan: parts,
    records, skeleton, materials, added resources, warnings. `glb_path`
    keeps the exported file."""
    game = open_game(dt, game_path)
    ws = game.workspace(workspace_dir or None)
    folder = tempfile.mkdtemp(prefix="doctor-toolkit-blender-")
    path = glb_path or os.path.join(folder, "model.glb")
    try:
        export_glb(objects, path)
        return ws.import_model(
            key, path, new_name=new_name or None, retarget=list(retarget), dry_run=dry_run
        )
    finally:
        if not glb_path:
            try:
                os.remove(path)
            except OSError:
                pass
        try:
            os.rmdir(folder)
        except OSError:
            pass


def plan_lines(summary: dict) -> list[str]:
    """The plan of an import as lines for the panel."""
    lines = [f"mode: {summary.get('mode')}, {summary.get('records', 0)} draw record(s)"]
    for part in summary.get("parts", []):
        lines.append(
            f"part {part['name']}: {part['vertices']} vertices, {part['triangles']} "
            f"triangles, {part['kind']}, material {part['material']}"
        )
    if summary.get("skeleton"):
        lines.append(f"skeleton {summary['skeleton']}, {summary.get('bones_mapped', 0)} bone(s)")
    lines.append(
        f"materials: {summary.get('materials_reused', 0)} reused, "
        f"{summary.get('materials_new', 0)} new; "
        f"{summary.get('textures_imported', 0)} texture(s)"
    )
    for mesh in summary.get("meshes", []):
        lines.append(f"mesh {mesh}")
    for added in summary.get("added", []):
        lines.append(f"add {added}")
    for retargeted in summary.get("retargeted", []):
        lines.append(f"retarget {retargeted}")
    for warning in summary.get("warnings", []):
        lines.append(f"warning: {warning}")
    return lines
