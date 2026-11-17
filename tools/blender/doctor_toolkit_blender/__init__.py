"""doctor_toolkit for Blender: models of The Godfather II in and out of a
game's workspace.

The panel is in the 3D viewport's sidebar (N), tab "Doctor Toolkit":
open a game, search its models, import one with materials, textures and
skeleton, edit it, check the export plan and record it in the workspace
(replacing the model, or as a new model set). The toolkit (`doctor-toolkit`
or the editor) applies the workspace to the game.

The add-on uses the `doctor_toolkit` Python module: bundled as a wheel in
the extension package, or found through the "Module folder" preference.
"""

bl_info = {
    "name": "doctor_toolkit",
    "author": "doctor_toolkit contributors",
    "version": (0, 0, 8),
    "blender": (4, 2, 0),
    "location": "3D Viewport > Sidebar > Doctor Toolkit",
    "description": "Import and export models of The Godfather II through a doctor_toolkit workspace",
    "doc_url": "https://github.com/Darkildo/doctor_toolkit/blob/main/docs/blender.md",
    "category": "Import-Export",
}

if "ui" in locals():
    import importlib

    importlib.reload(core)  # noqa: F821
    importlib.reload(ui)  # noqa: F821
else:
    from . import core, ui  # noqa: F401


def register():
    ui.register()


def unregister():
    ui.unregister()
