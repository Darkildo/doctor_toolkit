"""Panel, operators and settings of the add-on. The work is done in `core`."""

from __future__ import annotations

import bpy
from bpy.props import (
    BoolProperty,
    CollectionProperty,
    EnumProperty,
    IntProperty,
    PointerProperty,
    StringProperty,
)

from . import core

ADDON = __package__


def prefs(context):
    addon = context.preferences.addons.get(ADDON)
    return addon.preferences if addon else None


def toolkit(context):
    p = prefs(context)
    return core.load_toolkit(p.module_path if p else "")


def game_dir(context) -> str:
    state = context.scene.doctor_toolkit
    if state.game_dir:
        return bpy.path.abspath(state.game_dir)
    p = prefs(context)
    return bpy.path.abspath(p.game_dir) if p and p.game_dir else ""


def workspace_dir(context) -> str:
    p = prefs(context)
    return bpy.path.abspath(p.workspace_dir) if p and p.workspace_dir else ""


class DOCTORTK_Preferences(bpy.types.AddonPreferences):
    bl_idname = ADDON

    game_dir: StringProperty(
        name="Game folder",
        description="The Godfather II folder (the one with the gf2 folder) opened by default",
        subtype="DIR_PATH",
    )
    workspace_dir: StringProperty(
        name="Workspace folder",
        description="Where edits are recorded; empty: <game>/_doctor_toolkit/workspace, "
        "the folder the editor and the command line use",
        subtype="DIR_PATH",
    )
    module_path: StringProperty(
        name="Module folder",
        description="Folder with the doctor_toolkit Python module (pip install --target, or "
        "an unpacked wheel), when the extension does not bundle it",
        subtype="DIR_PATH",
    )

    def draw(self, context):
        layout = self.layout
        layout.prop(self, "game_dir")
        layout.prop(self, "workspace_dir")
        layout.prop(self, "module_path")
        try:
            dt = core.load_toolkit(self.module_path)
            layout.label(text=f"doctor_toolkit module: {dt.__file__}", icon="CHECKMARK")
        except core.ToolkitMissing as error:
            layout.label(text=str(error), icon="ERROR")


class DOCTORTK_Result(bpy.types.PropertyGroup):
    key: StringProperty()
    label: StringProperty()
    kind: StringProperty()
    stream: StringProperty()


class DOCTORTK_Line(bpy.types.PropertyGroup):
    text: StringProperty()
    warning: BoolProperty()


class DOCTORTK_State(bpy.types.PropertyGroup):
    game_dir: StringProperty(
        name="Game", description="The game folder; empty: the one of the preferences",
        subtype="DIR_PATH",
    )
    query: StringProperty(
        name="Search",
        description="Model name, pattern (car_*), hash or key; 'type:metamodel' for metamodels",
    )
    results: CollectionProperty(type=DOCTORTK_Result)
    result_index: IntProperty()
    target_key: StringProperty(
        name="Model",
        description="Key of the model to replace; filled in for imported models",
    )
    mode: EnumProperty(
        name="Mode",
        items=[
            ("REPLACE", "Replace", "Replace the model in place; ids stay the same"),
            ("NEW", "New model set", "Clone the model under new ids and import into the clone"),
        ],
    )
    new_name: StringProperty(name="Name", description="Name of the new model set")
    retarget: StringProperty(
        name="Retarget",
        description="Resource numbers of the stream (SimGroup, metamodel) to point at the "
        "new set, separated by commas",
    )
    plan: CollectionProperty(type=DOCTORTK_Line)
    message: StringProperty()
    link_path: StringProperty(
        name="Linked file",
        description="Working file of the toolkit's \"Open in external editor\"",
        subtype="FILE_PATH",
    )


def selected_or_all(context):
    objects = list(context.selected_objects)
    return objects or [o for o in context.scene.objects if core.KEY in o]


def set_plan(state, lines, warnings=()):
    state.plan.clear()
    for text in lines:
        item = state.plan.add()
        item.text = text
        item.warning = text.startswith("warning")
    for text in warnings:
        item = state.plan.add()
        item.text = f"check: {text}"
        item.warning = True


class _Toolkit:
    """Shared error handling: toolkit errors become reports."""

    def run(self, context):
        raise NotImplementedError

    def execute(self, context):
        try:
            return self.run(context)
        except core.ToolkitMissing as error:
            self.report({"ERROR"}, str(error))
        except Exception as error:  # noqa: BLE001 - shown to the user
            self.report({"ERROR"}, f"{type(error).__name__}: {error}")
        return {"CANCELLED"}


class DOCTORTK_OT_open_game(_Toolkit, bpy.types.Operator):
    """Open the game and its resource index (the first time reads every stream)"""

    bl_idname = "doctor_toolkit.open_game"
    bl_label = "Open game"

    def run(self, context):
        dt = toolkit(context)
        path = game_dir(context)
        if not path:
            self.report({"ERROR"}, "Choose the game folder")
            return {"CANCELLED"}
        core.forget_game(path)
        game = core.open_game(dt, path)
        index = core.game_index(dt, path)
        context.scene.doctor_toolkit.message = (
            f"{len(game.streams)} streams, {len(index)} resources"
        )
        self.report({"INFO"}, context.scene.doctor_toolkit.message)
        return {"FINISHED"}


class DOCTORTK_OT_search(_Toolkit, bpy.types.Operator):
    """Search the models of the game"""

    bl_idname = "doctor_toolkit.search"
    bl_label = "Search"

    def run(self, context):
        dt = toolkit(context)
        state = context.scene.doctor_toolkit
        found = core.find_models(core.game_index(dt, game_dir(context)), state.query)
        state.results.clear()
        for r in found:
            item = state.results.add()
            item.key = r["key"]
            item.label = r.get("label") or r["key"]
            item.kind = r["type"]
            item.stream = r["stream"]
        state.result_index = 0
        state.message = f"{len(found)} model(s)"
        return {"FINISHED"}


class DOCTORTK_OT_import(_Toolkit, bpy.types.Operator):
    """Import the chosen model with materials, textures and skeleton (the workspace version)"""

    bl_idname = "doctor_toolkit.import_model"
    bl_label = "Import model"
    bl_options = {"REGISTER", "UNDO"}

    def run(self, context):
        dt = toolkit(context)
        state = context.scene.doctor_toolkit
        if not 0 <= state.result_index < len(state.results):
            self.report({"ERROR"}, "Choose a model in the list")
            return {"CANCELLED"}
        key = state.results[state.result_index].key
        objects = core.import_model(dt, game_dir(context), key, workspace_dir(context))
        state.target_key = key
        state.plan.clear()
        state.message = f"imported {key}: {len(objects)} object(s)"
        self.report({"INFO"}, state.message)
        return {"FINISHED"}


class DOCTORTK_OT_fix_weights(_Toolkit, bpy.types.Operator):
    """Keep the four largest bone weights of every vertex and make them sum to one"""

    bl_idname = "doctor_toolkit.fix_weights"
    bl_label = "Limit weights"
    bl_options = {"REGISTER", "UNDO"}

    def run(self, context):
        check = core.prepare_for_export(selected_or_all(context), fix=True)
        set_plan(context.scene.doctor_toolkit, check.fixed, check.warnings + check.errors)
        self.report({"INFO"}, f"{len(check.fixed)} object(s) fixed")
        return {"FINISHED"}


class _Export(_Toolkit):
    dry_run = True

    def run(self, context):
        dt = toolkit(context)
        state = context.scene.doctor_toolkit
        objects = selected_or_all(context)
        key, source_game = core.source_of(objects)
        key = state.target_key or key
        if not key:
            self.report({"ERROR"}, "Import a model first, or type the key of the model to replace")
            return {"CANCELLED"}
        path = game_dir(context) or source_game
        check = core.prepare_for_export(objects, fix=True)
        if check.errors:
            set_plan(state, [], check.errors)
            self.report({"ERROR"}, check.errors[0])
            return {"CANCELLED"}
        new_name = state.new_name.strip() if state.mode == "NEW" else None
        if state.mode == "NEW" and not new_name:
            self.report({"ERROR"}, "Give the new model set a name")
            return {"CANCELLED"}
        retarget = [int(x) for x in state.retarget.replace(",", " ").split()] if new_name else []
        summary = core.export_to_workspace(
            dt, path, key, objects, new_name, retarget, self.dry_run, workspace_dir(context)
        )
        set_plan(state, core.plan_lines(summary), check.fixed + check.warnings)
        if self.dry_run:
            state.message = f"plan for {key}: nothing recorded yet"
        else:
            state.message = f"recorded in the workspace: {key}"
        self.report({"INFO"}, state.message)
        return {"FINISHED"}


class DOCTORTK_OT_check(_Export, bpy.types.Operator):
    """Export the selection and show what the import would do, without recording it"""

    bl_idname = "doctor_toolkit.check_export"
    bl_label = "Check"
    dry_run = True


class DOCTORTK_OT_export(_Export, bpy.types.Operator):
    """Export the selection into the game's workspace (one undoable operation there)"""

    bl_idname = "doctor_toolkit.export_model"
    bl_label = "Export to workspace"
    dry_run = False


class DOCTORTK_OT_link_open(_Toolkit, bpy.types.Operator):
    """Open the working file of the toolkit's "Open in external editor" (GLB)"""

    bl_idname = "doctor_toolkit.link_open"
    bl_label = "Open linked file"
    bl_options = {"REGISTER", "UNDO"}

    filepath: StringProperty(subtype="FILE_PATH")
    filter_glob: StringProperty(default="*.glb", options={"HIDDEN"})

    def invoke(self, context, event):
        context.window_manager.fileselect_add(self)
        return {"RUNNING_MODAL"}

    def run(self, context):
        objects = core.open_linked(self.filepath)
        state = context.scene.doctor_toolkit
        state.link_path = self.filepath
        state.message = f"opened {self.filepath}: {len(objects)} object(s)"
        return {"FINISHED"}


class DOCTORTK_OT_link_save(_Toolkit, bpy.types.Operator):
    """Check the model and save it over the linked file; the toolkit imports the save"""

    bl_idname = "doctor_toolkit.link_save"
    bl_label = "Save linked file"

    def run(self, context):
        state = context.scene.doctor_toolkit
        objects = selected_or_linked(context)
        check = core.save_linked(objects, state.link_path)
        set_plan(state, check.fixed, check.warnings + check.errors)
        if check.errors:
            self.report({"ERROR"}, check.errors[0])
            return {"CANCELLED"}
        state.message = f"saved {state.link_path or core.linked_path(objects)}"
        self.report({"INFO"}, state.message)
        return {"FINISHED"}


def selected_or_linked(context):
    """The objects opened from the linked file; the selection when there are none."""
    linked = [o for o in context.scene.objects if core.LINK in o]
    return linked or list(context.selected_objects)


class DOCTORTK_UL_results(bpy.types.UIList):
    def draw_item(self, context, layout, data, item, icon, active_data, active_property, index):
        layout.label(text=item.label, icon="MESH_DATA" if item.kind == "mesh" else "OUTLINER_OB_GROUP_INSTANCE")
        layout.label(text=item.stream)


class DOCTORTK_PT_panel(bpy.types.Panel):
    bl_label = "Doctor Toolkit"
    bl_space_type = "VIEW_3D"
    bl_region_type = "UI"
    bl_category = "Doctor Toolkit"

    def draw(self, context):
        layout = self.layout
        state = context.scene.doctor_toolkit
        col = layout.column(align=True)
        col.prop(state, "game_dir")
        col.operator(DOCTORTK_OT_open_game.bl_idname, icon="FILE_FOLDER")

        box = layout.box()
        row = box.row(align=True)
        row.prop(state, "query", text="")
        row.operator(DOCTORTK_OT_search.bl_idname, text="", icon="VIEWZOOM")
        box.template_list("DOCTORTK_UL_results", "", state, "results", state, "result_index", rows=5)
        box.operator(DOCTORTK_OT_import.bl_idname, icon="IMPORT")

        box = layout.box()
        box.prop(state, "target_key")
        box.prop(state, "mode", expand=True)
        if state.mode == "NEW":
            box.prop(state, "new_name")
            box.prop(state, "retarget")
        row = box.row(align=True)
        row.operator(DOCTORTK_OT_fix_weights.bl_idname, icon="GROUP_VERTEX")
        row.operator(DOCTORTK_OT_check.bl_idname, icon="VIEWZOOM")
        box.operator(DOCTORTK_OT_export.bl_idname, icon="EXPORT")

        box = layout.box()
        box.label(text="Linked file (Open in external editor)")
        row = box.row(align=True)
        row.operator(DOCTORTK_OT_link_open.bl_idname, icon="FILE_FOLDER")
        row.operator(DOCTORTK_OT_link_save.bl_idname, icon="FILE_TICK")
        if state.link_path:
            box.label(text=state.link_path)

        if state.message:
            layout.label(text=state.message, icon="INFO")
        if state.plan:
            box = layout.box()
            for line in state.plan:
                box.label(text=line.text, icon="ERROR" if line.warning else "DOT")


CLASSES = (
    DOCTORTK_Preferences,
    DOCTORTK_Result,
    DOCTORTK_Line,
    DOCTORTK_State,
    DOCTORTK_OT_open_game,
    DOCTORTK_OT_search,
    DOCTORTK_OT_import,
    DOCTORTK_OT_fix_weights,
    DOCTORTK_OT_check,
    DOCTORTK_OT_export,
    DOCTORTK_OT_link_open,
    DOCTORTK_OT_link_save,
    DOCTORTK_UL_results,
    DOCTORTK_PT_panel,
)


def register():
    for cls in CLASSES:
        bpy.utils.register_class(cls)
    bpy.types.Scene.doctor_toolkit = PointerProperty(type=DOCTORTK_State)


def unregister():
    del bpy.types.Scene.doctor_toolkit
    for cls in reversed(CLASSES):
        bpy.utils.unregister_class(cls)
