"""Read, convert and edit the game files of the EARS engine (The Godfather II PC).

The package wraps the Rust library ``doctor-toolkit``::

    import doctor_toolkit as dt

    game = dt.Game("/path/to/The Godfather II")
    stream = game.stream("gf2/frontend")
    for texture in stream.textures():
        print(texture.name, texture.width, texture.height)

See ``docs/python.md`` in the repository for a guide.
"""

from ._native import (
    DoctorToolkitError,
    Font,
    FormatError,
    Game,
    Index,
    InvalidArgumentError,
    IoError,
    ModError,
    ModManager,
    Project,
    Model,
    Resource,
    ResourceIter,
    Sound,
    Stream,
    StringTable,
    Texture,
    Workspace,
    __version__,
    animation_bank,
    animation_glb,
    character_glb,
    crc32,
    decode,
    fnv1a,
    hash_all,
    logic_graph,
    logic_messages,
    logic_missions,
    logic_schema,
    lookup,
    lua_compile,
    script_compile,
    lua_decompile,
    lua_listing,
    LuaSyntaxError,
    mod_info,
    parse_key,
    project_object_guid,
    read_model,
    sdbm,
    sound_bank,
    sound_from_snu,
    type_id,
    type_name,
)
from . import testing

open_game = Game

__all__ = [
    "DoctorToolkitError",
    "Font",
    "FormatError",
    "Game",
    "Index",
    "InvalidArgumentError",
    "IoError",
    "ModError",
    "ModManager",
    "Model",
    "Resource",
    "ResourceIter",
    "Sound",
    "Stream",
    "StringTable",
    "Texture",
    "Workspace",
    "__version__",
    "animation_bank",
    "animation_glb",
    "character_glb",
    "crc32",
    "decode",
    "fnv1a",
    "hash_all",
    "logic_graph",
    "logic_messages",
    "logic_missions",
    "logic_schema",
    "lookup",
    "lua_compile",
    "script_compile",
    "lua_decompile",
    "lua_listing",
    "LuaSyntaxError",
    "mod_info",
    "open_game",
    "parse_key",
    "read_model",
    "sdbm",
    "sound_bank",
    "sound_from_snu",
    "testing",
    "type_id",
    "type_name",
]
