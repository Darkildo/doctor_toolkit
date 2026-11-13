"""Type stubs of the extension module `doctor_toolkit._native`.

Resource keys are strings `stream:type:ids`, e.g.
`gf2/frontend:tg4h:00000000.4802e0a7.f3d257db.9cf57f36`; every argument named
`key` also accepts a `Resource`. A resource type is a tag (`"tg4h"`), a
hexadecimal id (`"0xcdb9d42b"`) or an int.
"""

import os
from pathlib import Path
from typing import Any, Iterator, Mapping, Optional, Sequence, Tuple, Union

__version__: str

StrPath = Union[str, "os.PathLike[str]"]
Key = Union[str, "Resource"]
TypeSpec = Union[str, int]
Json = Any

# ------------------------------------------------------------------ errors

class DoctorToolkitError(Exception):
    """Base class of every error raised by the library."""

class FormatError(DoctorToolkitError):
    """Malformed or unsupported data: bad magic, unexpected end, unknown version, RefPack."""

class InvalidArgumentError(DoctorToolkitError):
    """An argument the operation cannot honour, e.g. a refused write."""

class ModError(DoctorToolkitError):
    """A malformed mod package, or a mod operation that cannot be carried out safely."""

class IoError(DoctorToolkitError):
    """An operating system error on a file; the message names the path."""

class LuaSyntaxError(InvalidArgumentError):
    """A syntax error in Lua source; `args` is `(message, line)`."""

# ----------------------------------------------------------------- helpers

def sdbm(data: Union[str, bytes]) -> int:
    """SDBM hash: type tags, memory policies, lower-case SimGroup class names."""

def crc32(data: Union[str, bytes]) -> int:
    """CRC-32 (IEEE): lower-case material and shader names."""

def fnv1a(data: Union[str, bytes]) -> int:
    """32-bit FNV-1a."""

def hash_all(text: str) -> dict[str, int]:
    """`text` under every known hash function, by function name."""

def type_id(tag: TypeSpec) -> int:
    """The type id of a tag, a hexadecimal id or an int."""

def type_name(type_id: int) -> str:
    """The tag of a type id, or the id in hexadecimal."""

def lookup(hash: int) -> list[Tuple[str, str]]:
    """Names of a hash from the builtin name lists, as `(name, origin)`."""

def parse_key(key: str) -> dict[str, Any]:
    """`stream`, `type`, `type_id`, `ids` of a resource key."""

def decode(type: TypeSpec, data: bytes) -> Any:
    """Parses resource data without a game (see `Resource.parse`)."""

def sound_from_snu(data: bytes) -> "Sound":
    """The sound of a `.snu` file."""

def sound_bank(data: bytes) -> dict[str, Any]:
    """The structure of an `sbk` bank: `emitters` (with `name`, `track`,
    `instrument_index`), `instruments` with `tones`, the lookup tables,
    `samples` (format and size) and `problems`."""

def navigation_graph(data: bytes) -> dict[str, Any]:
    """A navigation graph (`graph`) as a dict, the JSON form of `graph-export`:
    `nodes` with `links` and `neighbour_links`, `neighbours`, properties,
    `visibility`, `precomputed_paths`, `rtree`; a summary under `info`."""

def navigation_graph_bytes(graph: dict[str, Any]) -> bytes:
    """Resource data of a navigation graph from a dict as `navigation_graph`
    gives it; link lengths, R-tree and paths follow edited positions and links."""

def lua_decompile(data: bytes) -> dict[str, Any]:
    """Decompiles a `lua` chunk: `source`, `round_trip` (`"identical"`, `"same_code"`,
    `"different"`), `detail` and `errors` (functions that could not be decompiled)."""

def lua_compile(source: str, chunk_name: str = "=script", number_size: int = 4) -> bytes:
    """Compiles Lua 5.0 source to a chunk (4-byte float numbers as in the game).
    Raises `LuaSyntaxError` with the line of the error."""

def lua_listing(data: bytes) -> str:
    """The instructions of a chunk as text."""

def logic_graph(
    game: "Game", scope: str, hubs: bool = True, all: bool = False, original: bool = False
) -> dict[str, Any]:
    """The logic graph of a scope (`mission:<name>`, `checkpoint:<n>`,
    `stream:<stream>`, `group:<stream>#<resource>`, `level:<stream>`, `game`):
    `nodes`, `wires`, `messages` (by hash) and `overview`. See docs/logic-graph.md."""

def logic_messages(
    game: "Game", name: str, search: bool = False, original: bool = False
) -> Any:
    """Who sends and who hears message `name` across the game (`None` when
    nobody does); with `search`, every message whose name contains `name`."""

def logic_missions(game: "Game") -> list[dict[str, Any]]:
    """The missions: `name` and `streams`."""

def logic_schema() -> dict[str, Any]:
    """What the schema of SimGroup pins covers."""

def logic_run(
    game: "Game",
    scope: str,
    events: list[str] | None = None,
    until: float | None = None,
    steps: int = 20000,
    seed: int = 1,
    lua: bool = True,
    reactions: list[dict[str, Any]] | None = None,
    breaks: list[str] | None = None,
    compare: str | None = None,
    original: bool = False,
) -> dict[str, Any]:
    """Runs the logic of a scope without the game: `journal` (who sent what
    to whom and when), `stop`, `time`, `steps`, `actors`, `variables` and
    `comparison` with a journal of the game (`compare`: its JSON text).
    `events` as in `logic-run --events` (default `["load"]`). See
    docs/logic-graph.md §9."""

def script_compile(graph_json: str) -> dict[str, Any]:
    """Compiles a visual script graph (JSON text): `source` (Lua), `chunk`
    (bytes, or `None` with errors) and `diagnostics` (dicts with `severity`,
    `node`, `pin`, `message`)."""

def character_glb(
    game: "Game",
    stream: str,
    database: str,
    set: str,
    choices: Optional[list[str]] = None,
    lod: int = 0,
    format: str = "glb",
) -> bytes:
    """A character of a parts database (`partsdb`) as binary glTF, or as
    `format`: `fbx`, `usdz`, `usd`/`usda`/`usdc` (without textures).

    `choices`: `"slot=variant"`, `"preset=item"` or `"tag=name"`; unset slots
    take the first variant of the set. Material swaps and morphs are applied.
    """

def animation_glb(
    game: "Game",
    stream: str,
    bank: int,
    clips: Optional[list[str]] = None,
    database: Optional[str] = None,
    set: Optional[str] = None,
    database_stream: str = "gf2/globalfolder/characters/allcharacters",
    format: str = "glb",
) -> bytes:
    """Clips of the animation bank (`bnk` resource index) as glTF animations
    (or `format` as for `character_glb`), on the rig skeleton or on the
    character `database`/`set`."""

def read_model(path: Union[str, os.PathLike[str]]) -> dict[str, Any]:
    """What a model file (glTF, FBX, OBJ, USD) holds as the importer reads it:
    `parts`, `materials`, `joints`, `clips`, `warnings` (Y up, metres)."""

def animation_bank(data: bytes) -> dict[str, Any]:
    """Clips of an animation bank (`bnk` data): names, frames, bone sets."""

def animation_graph(data: bytes) -> dict[str, Any]:
    """Node graph of an animation bank: `entries` (node trees), `issues`, `slots`."""

def mod_info(path: StrPath) -> dict[str, Any]:
    """Contents of a mod package (verified) or an old toolkit patch (`.vpat`)."""

def build_sample_game(root: StrPath) -> dict[str, Any]:
    """Writes a miniature synthetic game; see `doctor_toolkit.testing`."""

# -------------------------------------------------------------------- game

class Game:
    """An installed game: streams (`.str`) and tables of contents (`.toc`)."""

    def __init__(self, root: StrPath) -> None: ...
    @property
    def root(self) -> Path: ...
    @property
    def profile(self) -> str: ...
    @property
    def streams(self) -> list[str]:
        """Names of all streams, e.g. `gf2/frontend`."""
    @property
    def tocs(self) -> list[str]:
        """Paths of the tables of contents relative to the game directory."""
    def stream_files(self) -> list[dict[str, Any]]:
        """`name`, `path`, `size` of every stream."""
    def has_stream(self, name: str) -> bool: ...
    def stream(self, name: str) -> "Stream":
        """Opens a stream; `KeyError` when there is none."""
    def resources(
        self, type: Optional[TypeSpec] = None, streams: Optional[Sequence[str]] = None
    ) -> "ResourceIter":
        """Iterates over resources of the game or of some streams."""
    def resource(self, key: Key) -> "Resource":
        """The resource with this key; `KeyError` when there is none."""
    def read(self, key: Key) -> bytes:
        """Decompressed data of a resource."""
    def toc(self, path: str) -> dict[str, Any]:
        """A table of contents: `version`, `files` (`name`, `hash`, `data_size`, `deps`)."""
    def index(self, cache: Union[None, bool, StrPath] = None) -> "Index":
        """The game index; cached in the user cache directory by default."""
    def workspace(self, dir: Optional[StrPath] = None, create: bool = True) -> "Workspace":
        """Opens (or creates) the workspace, by default `<game>/_doctor_toolkit/workspace`."""
    def init_workspace(self, dir: Optional[StrPath] = None) -> "Workspace":
        """Creates an empty workspace; refused when one exists."""
    def mods(self) -> "ModManager":
        """The mod manager of the game."""
    def unpack(
        self,
        out: StrPath,
        streams: Optional[Sequence[str]] = None,
        types: Optional[Sequence[TypeSpec]] = None,
        *,
        keys: Optional[Sequence[Key]] = None,
        index: Optional["Index"] = None,
        dds: bool = False,
        wav: bool = False,
        raw: bool = True,
        views: bool = True,
        model_assets: bool = True,
        model_format: str = "glb",
    ) -> dict[str, Any]:
        """Writes resources in common formats with a manifest (`doctor-toolkit unpack`);
        models as `model_format` (glb, fbx, obj, usd, usda, usdc, usdz), which `pack`
        imports back when edited."""
    def pack(
        self, dir: StrPath, workspace_dir: Optional[StrPath] = None, dry_run: bool = False
    ) -> dict[str, Any]:
        """Records the changed files of an unpacked directory in the workspace."""
    def integrity(self, full: bool = False) -> list[dict[str, Any]]:
        """`path`, `condition`, `changed` of every stream and table of contents."""
    def archive_entries(self) -> list[dict[str, Any]]:
        """`archive`, `name`, `size` of every entry of the `.viv` sound archives."""
    def archive_sound(self, name: str) -> "Sound":
        """A streamed sound of the `.viv` archives by entry name."""

class Stream:
    """A memory-mapped stream file; iterating yields its resources."""

    @property
    def name(self) -> str: ...
    @property
    def rel_path(self) -> str: ...
    @property
    def path(self) -> Path: ...
    @property
    def size(self) -> int:
        """File size in bytes."""
    @property
    def data_size(self) -> int:
        """Sum of the decompressed resource sizes."""
    def __len__(self) -> int: ...
    def __getitem__(self, index: int) -> "Resource": ...
    def __iter__(self) -> "ResourceIter": ...
    def resources(self, type: Optional[TypeSpec] = None) -> list["Resource"]: ...
    def find(self, type: Optional[TypeSpec] = None, id: Optional[int] = None) -> Optional["Resource"]:
        """The first resource of a type whose last id is `id`."""
    def get(self, key: Key) -> Optional["Resource"]: ...
    def read(self, index: int) -> bytes: ...
    def textures(self) -> list["Texture"]: ...

class Resource:
    """One resource of a stream."""

    @property
    def stream(self) -> str: ...
    @property
    def index(self) -> int: ...
    @property
    def key(self) -> str: ...
    @property
    def type(self) -> str: ...
    @property
    def type_id(self) -> int: ...
    @property
    def ids(self) -> list[int]: ...
    @property
    def id(self) -> int:
        """The last id, the name hash for most types."""
    @property
    def data_size(self) -> int: ...
    @property
    def stored_size(self) -> int: ...
    @property
    def compressed(self) -> bool: ...
    @property
    def mem_policy(self) -> str: ...
    @property
    def alignment(self) -> int: ...
    def read(self) -> bytes:
        """The decompressed data."""
    def parse(self) -> Any:
        """`Texture`, `StringTable`, `Model`, `Font`, `list[Sound]` or a dict, by type."""
    def texture(self) -> "Texture": ...
    def string_table(self) -> "StringTable": ...
    def model(self) -> "Model": ...
    def to_dict(self) -> dict[str, Any]: ...

class ResourceIter(Iterator[Resource]):
    def __iter__(self) -> "ResourceIter": ...
    def __next__(self) -> Resource: ...

# ----------------------------------------------------------------- formats

class Texture:
    """A texture (`tg4h` + `tg4d`)."""

    @property
    def name(self) -> str: ...
    @property
    def package(self) -> str: ...
    @property
    def name_hash(self) -> int: ...
    @property
    def width(self) -> int: ...
    @property
    def height(self) -> int: ...
    @property
    def format(self) -> str: ...
    @property
    def kind(self) -> str:
        """`flat`, `cube` or `volume`."""
    @property
    def mip_count(self) -> int: ...
    @property
    def layer_count(self) -> int: ...
    @property
    def srgb(self) -> bool: ...
    @property
    def data_size(self) -> int: ...
    @property
    def params(self) -> dict[int, Union[str, int]]: ...
    @property
    def keys(self) -> Optional[Tuple[str, str]]:
        """Keys of the `tg4h` and `tg4d`, or `None`."""
    @property
    def data(self) -> bytes:
        """Pixel data as stored."""
    def header_bytes(self) -> bytes: ...
    def to_dds(self) -> bytes: ...
    def to_png(self, layer: int = 0) -> bytes: ...
    def rgba(self, layer: int = 0) -> Tuple[int, int, bytes]:
        """`(width, height, rgba)` of the largest mip level."""
    def replaced_by(self, image: bytes) -> "Texture":
        """A replacement made from a PNG or DDS file (nothing is recorded)."""
    def to_dict(self) -> dict[str, Any]: ...

class StringTable:
    """A string table (`lh2`); changes stay in the object until recorded."""

    @classmethod
    def parse(cls, data: bytes) -> "StringTable": ...
    @property
    def key(self) -> Optional[str]: ...
    @property
    def version(self) -> int: ...
    @property
    def flags(self) -> int: ...
    @property
    def languages(self) -> list[str]: ...
    @property
    def language_count(self) -> int: ...
    @property
    def hashes(self) -> list[int]: ...
    def __len__(self) -> int: ...
    def __contains__(self, hash: int) -> bool: ...
    def __getitem__(self, hash: int) -> str: ...
    def __setitem__(self, hash: int, text: str) -> None:
        """Sets the string in every language column."""
    def get(self, hash: int, language: int = 0) -> Optional[str]: ...
    def set(self, hash: int, text: str, language: int = 0) -> None: ...
    def items(self, language: int = 0) -> list[Tuple[int, str]]: ...
    def to_dict(self) -> dict[str, Any]: ...
    def to_bytes(self) -> bytes: ...
    def copy(self) -> "StringTable": ...

class Model:
    """A model (`mesh` + `meshvolatile`)."""

    @property
    def name(self) -> str: ...
    @property
    def key(self) -> str: ...
    @property
    def bounding_sphere(self) -> list[float]: ...
    @property
    def aabb_min(self) -> list[float]: ...
    @property
    def aabb_max(self) -> list[float]: ...
    @property
    def bones(self) -> list[Tuple[int, int]]: ...
    @property
    def records(self) -> list[dict[str, Any]]: ...
    def vertices(self, record: int) -> dict[str, Any]:
        """Packed little-endian buffers: `positions` (float32x3), `normals`, ..."""
    def triangles(self, record: int) -> bytes:
        """Packed little-endian uint32x3."""
    def to_glb(self, index: Optional["Index"] = None) -> bytes:
        """Binary glTF; with the index, materials, textures and skeleton are embedded."""
    def export(
        self,
        path: Union[str, os.PathLike[str]],
        index: Optional["Index"] = None,
        format: Optional[str] = None,
    ) -> list[str]:
        """Writes the model as glb, fbx, obj, usd, usda, usdc or usdz (default
        from the extension); returns the paths written."""

class Font:
    """A font (`ffn`) with its atlas texture."""

    @property
    def name(self) -> str: ...
    @property
    def version(self) -> int: ...
    @property
    def metrics(self) -> list[int]: ...
    @property
    def pages(self) -> list[int]: ...
    @property
    def glyphs(self) -> list[dict[str, Any]]: ...
    @property
    def atlas(self) -> Optional[Texture]: ...
    def covers(self, char: str) -> bool: ...
    def to_dict(self) -> dict[str, Any]: ...

class Sound:
    """A sound of an `sbk` bank or of the `.viv` archives (EA-XAS)."""

    @property
    def index(self) -> Optional[int]: ...
    @property
    def name(self) -> Optional[str]: ...
    @property
    def codec(self) -> int: ...
    @property
    def channels(self) -> int: ...
    @property
    def sample_rate(self) -> int: ...
    @property
    def sample_count(self) -> int: ...
    @property
    def loop_start(self) -> Optional[int]: ...
    @property
    def duration(self) -> float: ...
    def pcm(self) -> bytes:
        """Interleaved int16 little-endian samples."""
    def to_wav(self) -> bytes: ...
    def to_snu(self) -> bytes: ...

# -------------------------------------------------------------- navigation

class Index:
    """Every resource of a game with its names and references."""

    def __len__(self) -> int: ...
    @property
    def stream_count(self) -> int: ...
    @property
    def cache_file(self) -> Optional[Path]: ...
    @property
    def report(self) -> dict[str, Any]: ...
    @property
    def record_errors(self) -> int: ...
    def refresh(self) -> None:
        """Re-reads the streams changed since the index was built."""
    def search(self, query: str, limit: int = 1000) -> list[dict[str, Any]]:
        """Search results: `key`, `label`, `type`, `stream`, `index`, `hits`; limit 0 = all."""
    def count(self, query: str) -> int: ...
    def record(self, key: Key) -> dict[str, Any]: ...
    def refs(self, key: Key) -> dict[str, Any]:
        """`uses`, `used_by` and `copies` of a resource."""
    def keys(self, type: Optional[TypeSpec] = None, stream: Optional[str] = None) -> list[str]: ...
    def lookup(self, hash: int) -> list[Tuple[str, str]]:
        """Names of a hash known from the builtin lists and the game."""

# --------------------------------------------------------------- workspace

class Workspace:
    """Edits kept on disk with undo/redo, written into the game by `apply`."""

    @property
    def dir(self) -> Path: ...
    def __len__(self) -> int: ...
    def __contains__(self, key: Key) -> bool: ...
    @property
    def changes(self) -> list[dict[str, Any]]: ...
    @property
    def keys(self) -> list[str]: ...
    @property
    def history(self) -> list[dict[str, Any]]: ...
    @property
    def undo_label(self) -> Optional[str]: ...
    @property
    def redo_label(self) -> Optional[str]: ...
    def data(self, key: Key) -> Optional[bytes]: ...
    def original_data(self, key: Key) -> Optional[bytes]: ...
    def read(self, key: Key) -> bytes:
        """Edited data, or the game's when the resource is not changed."""
    def set_data(
        self, key: Key, data: bytes, label: Optional[str] = None, storage: str = "keep"
    ) -> bool: ...
    def set_many(
        self, edits: Mapping[str, bytes], label: Optional[str] = None, storage: str = "keep"
    ) -> int:
        """One operation per stream; returns the number of operations recorded."""
    def import_texture(self, key: Key, image: bytes) -> Texture:
        """Records a texture from a PNG or DDS file."""
    def import_model(
        self,
        key: Key,
        path: Union[str, os.PathLike[str]],
        new_name: Optional[str] = None,
        retarget: Sequence[int] = (),
        dry_run: bool = False,
    ) -> dict[str, Any]:
        """Imports a model file (glTF, FBX, OBJ, USD) over a mesh or metamodel, or as a new model set; returns the summary."""
    def export_model(self, key: Key, index: Optional[Index] = None) -> bytes:
        """The workspace version of a mesh or metamodel as GLB, the file `import_model` takes back."""
    def import_animation(
        self,
        key: Key,
        path: Union[str, os.PathLike[str]],
        clip: str,
        add: bool = False,
        animation: Optional[str] = None,
        looping: Optional[bool] = None,
        scheme: int = 4,
        max_angle: float = 0.002,
        max_shift: float = 0.0005,
        dry_run: bool = False,
    ) -> dict[str, Any]:
        """Imports an animation (glTF, FBX, USD) over a clip of a `bnk` bank, or as a new clip with a named entry; returns the summary."""
    def set_clip_events(self, key: Key, clip: str, events: Sequence[Mapping[str, Any]]) -> bool:
        """Replaces the events of a clip (time in ticks, 2 per frame); one operation."""
    def set_animation_graph(self, key: Key, graph: Mapping[str, Any]) -> bool:
        """Replaces the node graph of a bank (`entries` as `animation_graph` gives); one operation."""
    def string_table(self, key: Key) -> StringTable: ...
    def set_string_table(self, table: StringTable, key: Optional[Key] = None) -> bool: ...
    def edit_strings(
        self, key: Key, strings: Mapping[int, str], language: Optional[int] = None
    ) -> bool: ...
    def file_keys(self) -> list[str]:
        """Changed files outside the streams: `viv:<archive entry>`, `file:<path>`."""
    def file_data(self, key: str) -> bytes:
        """Current contents of a file outside the streams (change or original)."""
    def set_file(self, key: str, data: bytes, label: Optional[str] = None) -> None:
        """Records new contents of a file outside the streams (one operation)."""
    def import_sound(self, target: str, path: str | os.PathLike[str]) -> float:
        """Replaces `<sbk key>#<sound>` or a `.viv` entry with an audio file; returns seconds."""
    def revert_files(self, keys: Sequence[str]) -> bool: ...
    def revert(self, keys: Sequence[str]) -> bool: ...
    def revert_all(self) -> bool: ...
    def undo(self) -> Optional[str]: ...
    def redo(self) -> Optional[str]: ...
    def reload(self) -> None: ...
    def status(self) -> dict[str, Any]: ...
    def diff(self, filter: Optional[str] = None) -> list[dict[str, Any]]: ...
    def apply(self, streams: Optional[Sequence[str]] = None, force: bool = False) -> dict[str, Any]:
        """Writes the changes into the game (backups, atomic writes, TOC sizes)."""
    def unapply(self, force: bool = False) -> dict[str, Any]:
        """Puts the original files back; the changes stay pending."""
    def build_mod(
        self,
        out: StrPath,
        name: str,
        *,
        id: Optional[str] = None,
        version: str = "1.0",
        author: str = "",
        description: str = "",
        url: Optional[str] = None,
        keys: Optional[Sequence[str]] = None,
        preview: Optional[bytes] = None,
    ) -> dict[str, Any]: ...

# ----------------------------------------------------------------- project

class Project:
    """A mod as a folder of source files (`docs/project.md`), built into the
    workspace when they change."""

    def __init__(
        self, path: StrPath, game: Game | None = None, workspace_dir: StrPath | None = None
    ) -> None: ...
    @staticmethod
    def create(
        game: Game,
        path: StrPath,
        *,
        name: str | None = None,
        id: str | None = None,
        author: str = "",
        streams: list[str] | None = None,
        types: list[TypeSpec] | None = None,
        from_workspace: bool = False,
        workspace_dir: StrPath | None = None,
        index: Index | None = None,
        dds: bool = False,
        model_format: str = "glb",
        shared: bool = True,
    ) -> Project:
        """Makes a project in a new or empty folder from streams of the game or
        from the resources the workspace changes."""
    @property
    def root(self) -> Path: ...
    @property
    def name(self) -> str: ...
    @property
    def id(self) -> str: ...
    def entries(self) -> list[dict[str, Any]]:
        """`id`, `kind`, `type`, `name`, `ids`, `files`, `keys` of every entry."""
    def status(self) -> dict[str, Any]:
        """`entries` (`id`, `state`, `kind`, `type`, `name`, `patch`),
        `unknown_files`, `warnings`, `to_build`."""
    def build(
        self, *, only: list[str] | None = None, force: bool = False, dry_run: bool = False
    ) -> dict[str, Any]:
        """Builds changed entries into the workspace as one operation:
        `built`, `up_to_date`, `errors`, `warnings`, `seq`."""
    def pull(self, only: list[str] | None = None) -> list[str]:
        """Writes the workspace's version of out-of-sync entries into their files."""
    def build_mod(self, out: StrPath, preview: StrPath | None = None) -> dict[str, Any]:
        """A mod of the project's changes in the workspace: `resources`, `added`, `file_size`."""

def project_object_guid(scope: str, name: str) -> str:
    """GUID (32 hexadecimal digits) a project gives the new object `name`."""

# -------------------------------------------------------------------- mods

class ModManager:
    """The installed mods of a game, in load order."""

    @property
    def dir(self) -> Path: ...
    @property
    def mods(self) -> list[dict[str, Any]]: ...
    def manifest(self, id: str) -> dict[str, Any]: ...
    def install(self, path: StrPath, enable: bool = True) -> dict[str, Any]: ...
    def install_workspace(
        self,
        workspace: Workspace,
        name: str,
        *,
        id: Optional[str] = None,
        version: str = "1.0",
        author: str = "",
        description: str = "",
        url: Optional[str] = None,
        keys: Optional[Sequence[str]] = None,
    ) -> dict[str, Any]: ...
    def uninstall(self, id: str) -> None: ...
    def enable(self, id: str, enabled: bool = True) -> None: ...
    def move(self, id: str, position: int) -> None: ...
    def conflicts(self, enabled_only: bool = True) -> list[dict[str, Any]]: ...
    def clashes(self, enabled_only: bool = True) -> list[dict[str, Any]]: ...
    def check(self) -> list[str]: ...
    def status(self) -> dict[str, Any]: ...
    def apply(self, force: bool = False, ignore_compatibility: bool = False) -> dict[str, Any]: ...
    def unapply(self, force: bool = False) -> dict[str, Any]: ...
    def export(self, id: str, out: StrPath) -> int: ...
