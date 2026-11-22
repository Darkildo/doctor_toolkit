//! Routers of the tools that bring the MCP server level with the editor
//! (stage 104, packages B, D, E and G): what the wizards do, sound and
//! animation data, resources and projects, runs without the game, and the
//! meaning of what the mouse does in the level view. [`Server::new`] adds
//! them by the flags of the server, as its other routers.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use crate::server::Server;
use crate::tools;

#[tool_router(router = authoring_read_router, vis = "pub(crate)")]
impl Server {
    #[tool(
        description = "Game parameters of the curated list (damage, health, prices, police \
                       heat, ...): each is an attribute of some SimGroup objects, with its \
                       current values, kind and bounds. Filter by `category` or text; `id` \
                       shows one with the value of every object. set_tunables changes them.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn tunables(
        &self,
        params: Parameters<tools::wizards::TunablesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("tunables", move |k| tools::wizards::tunables(k, params.0))
            .await
    }

    #[tool(
        description = "Run the logic of a scope without the game: the objects of a mission, a stream \
                       or a SimGroup on the model of the game's classes — messages, variables, \
                       timers, Lua scripts. `events` are the inputs (load, send:MSG, enter:OBJ, \
                       var:NAME=N, wait:SECONDS); the answer is the journal in pages with why the run \
                       stopped. `actors` lists the objects and which are black boxes; `reactions` say \
                       what a black box sends. Repeatable: the same call gives the same journal.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn logic_run(
        &self,
        params: Parameters<tools::runs::LogicRunParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("logic_run", move |k| tools::runs::logic_run(k, params.0))
            .await
    }

    #[tool(
        description = "Try a script on a stand-in for the game: a script graph (compiled first), Lua \
                       source or a file. It is loaded and started, then `steps` happen in order \
                       (send:MESSAGE, wait:SECONDS, var:NAME=NUMBER, player:X,Y,Z, relay:FROM=TO, \
                       stop); the answer is what the script did — messages sent, variables set, game \
                       functions called, timers, prints, errors.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn script_run(
        &self,
        params: Parameters<tools::runs::ScriptRunParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("script_run", move |k| tools::runs::script_run(k, params.0))
            .await
    }

    #[tool(
        description = "Look into a save file of the game (read only): the summary (name, city, play \
                       time, checksums, chunks) and the decoded content of chunks — story variables, \
                       money, families, buildings, crew, inventory. A folder lists its saves.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn save_info(
        &self,
        params: Parameters<tools::runs::SaveInfoParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("save_info", move |k| tools::runs::save_info(k, params.0))
            .await
    }

    #[tool(
        description = "Write the resources of streams as files in common formats (PNG, glTF, JSON, \
                       Lua, WAV) with a manifest, into a folder of the output folder (--output-dir): \
                       <folder>/<stream>/<type>/<name>.<ext>. Edit the files and `pack` records the \
                       changed ones in the workspace. Nothing in the game or the workspace changes.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn unpack(
        &self,
        params: Parameters<tools::files::UnpackParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("unpack", move |k| tools::files::unpack(k, params.0))
            .await
    }

    #[tool(
        description = "Make a mod project in a folder of the output folder (--output-dir) from \
                       streams of the game, or from what the workspace changes (the resources it \
                       replaces and adds, the streams it creates): every resource becomes a source \
                       file under src/, equal resources of several streams go to shared/ once. \
                       Without streams and without from_workspace: an empty project with a \
                       workspace of its own. Nothing in the game or the workspace changes.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn project_new(
        &self,
        params: Parameters<tools::files::ProjectNewParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("project_new", move |k| {
            tools::files::project_new(k, params.0)
        })
        .await
    }

    #[tool(
        description = "The world grids of a level: which streams the game loads by the position of \
                       the player. The summary of each grid, and with `cells` every cell — stream, \
                       kind, level of detail, centre, radius, box, the streams it needs. \
                       add_zone_cell adds a cell, remove_zone_cell takes one out.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn zone_cells(
        &self,
        params: Parameters<tools::files::ZoneCellsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("zone_cells", move |k| tools::files::zone_cells(k, params.0))
            .await
    }

    #[tool(
        description = "The speech tables of the game: the records of the speech event table (cool- \
                       down, rank, flags, priority) with the names of the events, the entries of the \
                       speech banks (file, mix categories, settings, path in the archives), and the \
                       chatter sets — who can say voice lines. set_speech_events and voice_add change \
                       them.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn speech(
        &self,
        params: Parameters<tools::assets::SpeechParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("speech", move |k| tools::assets::speech(k, params.0))
            .await
    }

    #[tool(
        description = "The mixer of the game: the mix categories, bank pools and ducks of the audio \
                       configuration and the list of mixer snapshots; with `name` the channels of one \
                       snapshot (volume, time, low-pass frequency of each category). set_mix changes \
                       them.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn mix(
        &self,
        params: Parameters<tools::assets::MixParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("mix", move |k| tools::assets::mix(k, params.0))
            .await
    }

    #[tool(
        description = "The node graph of an animation bank: its entries by name with the result of \
                       the check; `entry` gives the node tree of one entry as JSON, `save_as` writes \
                       the whole graph into the output folder (the form set_animation_graph reads \
                       back).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn animation_graph(
        &self,
        params: Parameters<tools::assets::AnimationGraphParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("animation_graph", move |k| {
            tools::assets::animation_graph(k, params.0)
        })
        .await
    }

    #[tool(
        description = "Where the ground is, by the collision of zones (the same world the level view \
                       of the editor builds): the ground straight below a `point` with the object it \
                       belongs to, or the point on the ground a ray hits (`ray_origin`, \
                       `ray_direction`) — where a model dropped along that ray stands. For placing \
                       objects: add_object and move_object take the position, or put on the ground \
                       themselves with `on_ground`.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn ground(
        &self,
        params: Parameters<tools::scene::GroundParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("ground", move |k| tools::scene::ground(k, params.0))
            .await
    }

    #[tool(
        description = "Check that a player walks from A to B in a straight line over the collision of \
                       zones, without the game: a capsule with gravity, steps and slopes, as the Walk \
                       panel of the editor. The answer says whether B was reached, or where the \
                       player got stuck (and on which object) or fell through, with the path walked.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn walk_route(
        &self,
        params: Parameters<tools::scene::WalkRouteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("walk_route", move |k| tools::scene::walk_route(k, params.0))
            .await
    }
}

#[tool_router(router = authoring_edit_router, vis = "pub(crate)")]
impl Server {
    #[tool(
        description = "Change game parameters of the curated list (`tunables` lists them): a \
                       `value` for every object of the parameter, or `multiply` to scale each \
                       object's own value. Every copy of the attribute changes in one undoable \
                       operation with its diff; `dry_run` only says.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn set_tunables(
        &self,
        params: Parameters<tools::wizards::SetTunablesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("set_tunables", move |k| {
            tools::wizards::set_tunables(k, params.0)
        })
        .await
    }

    #[tool(
        description = "Add a voice line from a recording: the sound goes into the overlay \
                       archive, the speech bank gets its entry, the speaker's chatter set the \
                       line (and the event when it is new), a new event its record in the event \
                       table; subtitles by language. One undoable operation with its diff.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn voice_add(
        &self,
        params: Parameters<tools::wizards::VoiceAddParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("voice_add", move |k| tools::wizards::voice_add(k, params.0))
            .await
    }

    #[tool(
        description = "Add a part of clothes to a parts database: the part with its meshes \
                       (import_model with `name` makes a mesh), a variant made of it, and the \
                       sets that may wear the variant (`only` makes it the one choice of the \
                       slot). One undoable operation with its diff.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn partsdb_add_part(
        &self,
        params: Parameters<tools::wizards::PartsdbAddPartParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("partsdb_add_part", move |k| {
            tools::wizards::partsdb_add_part(k, params.0)
        })
        .await
    }

    #[tool(
        description = "Let sets of a parts database wear an existing variant in a slot; `only` \
                       makes it the only choice of the slot in those sets. One undoable \
                       operation with its diff.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn partsdb_allow(
        &self,
        params: Parameters<tools::wizards::PartsdbAllowParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("partsdb_allow", move |k| {
            tools::wizards::partsdb_allow(k, params.0)
        })
        .await
    }

    #[tool(
        description = "Set float parameters of a material by name (tint colours \
                       `g_TintColor0`..2 and any other number parameter resource_json shows): a \
                       material of the game, or one the workspace added with an imported model. \
                       Every copy of the material (same name hash, any stream) gets the edit, \
                       since the game uses the copy it loads first; `this_stream_only` limits it \
                       to the given resource. One undoable operation with its diff.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn set_material(
        &self,
        params: Parameters<tools::wizards::SetMaterialParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("set_material", move |k| {
            tools::wizards::set_material(k, params.0)
        })
        .await
    }

    #[tool(
        description = "Record the files of an unpacked folder (`unpack`) that changed since in the \
                       workspace: textures, models, strings, logic JSON, Lua, raw data. Unchanged \
                       files are skipped; a changed read-only view, a missing file or a file not in \
                       the manifest is an error and then nothing is recorded. One undoable operation \
                       with its diff; `dry_run` only lists.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn pack(
        &self,
        params: Parameters<tools::files::PackParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("pack", move |k| tools::files::pack(k, params.0))
            .await
    }

    #[tool(
        description = "Write the workspace's version of out-of-sync entries into the source files of \
                       the project (--project), so that the project follows edits made by other \
                       tools; with sources, also write source files for everything the project's \
                       own workspace holds and no entry has yet (added resources, new streams). \
                       The workspace does not change.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn project_pull(
        &self,
        params: Parameters<tools::files::ProjectPullParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("project_pull", move |k| {
            tools::files::project_pull(k, params.0)
        })
        .await
    }

    #[tool(
        description = "Take changes out of the workspace: those whose key is given or contains the \
                       text, or all of them. The resources are as in the game files again. One \
                       undoable operation with its diff (undo brings the changes back).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn revert(
        &self,
        params: Parameters<tools::files::RevertParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("revert", move |k| tools::files::revert(k, params.0))
            .await
    }

    #[tool(
        description = "Rebase the changes made on other data than the game gives now: a mod \
                       under them was updated, or the mods of the game are others since the \
                       edits, and a mod built of them would take back what changed. SimGroups, \
                       string tables, the parts database and the world grid are merged by their \
                       operations (those left out are listed by object or string); any other \
                       resource needs `keep` (yours stays) or `take_base` (the game's data). \
                       `check` only lists them. One undoable operation with its diff.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn rebase(
        &self,
        params: Parameters<tools::files::RebaseParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("rebase", move |k| tools::files::rebase(k, params.0))
            .await
    }

    #[tool(
        description = "Take the cell of a stream out of the world grids of a level: the game no \
                       longer loads the stream by the position of the player. One undoable operation \
                       with its diff.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn remove_zone_cell(
        &self,
        params: Parameters<tools::files::RemoveZoneCellParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("remove_zone_cell", move |k| {
            tools::files::remove_zone_cell(k, params.0)
        })
        .await
    }

    #[tool(
        description = "Change the parameters of one sound (emitter) of a sound bank: volume, pitch \
                       and its random spread, velocity, falloff distance, priority, polyphony, flags, \
                       mixer category; remove a variation. resource_json of the bank shows its \
                       emitters. One undoable operation with its diff.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn sbk_edit(
        &self,
        params: Parameters<tools::assets::SbkEditParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("sbk_edit", move |k| tools::assets::sbk_edit(k, params.0))
            .await
    }

    #[tool(
        description = "Add sound files to a sound bank: as new variations of an emitter, or as a new \
                       emitter the game plays by name (`PlayEmx` of scripts, `audio: play` of \
                       animations) with the parameters of an existing one. The files are encoded in \
                       the format of the emitter's sounds. One undoable operation with its diff.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn sound_add(
        &self,
        params: Parameters<tools::assets::SoundAddParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("sound_add", move |k| tools::assets::sound_add(k, params.0))
            .await
    }

    #[tool(
        description = "Add records to the speech event table for events that have none (the common \
                       values of the game), or change cool-down, priority, flags and rank of the \
                       named records. One undoable operation with its diff.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn set_speech_events(
        &self,
        params: Parameters<tools::assets::SetSpeechEvents>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("set_speech_events", move |k| {
            tools::assets::set_speech_events(k, params.0)
        })
        .await
    }

    #[tool(
        description = "Change a channel of a mixer snapshot in every copy of it — volume, low-pass \
                       frequency, times of a mix category (a category the snapshot lacks gets a \
                       channel) — or add a mix category to the audio configuration. One undoable \
                       operation with its diff.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn set_mix(
        &self,
        params: Parameters<tools::assets::SetMixParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("set_mix", move |k| tools::assets::set_mix(k, params.0))
            .await
    }

    #[tool(
        description = "Change a Flash interface file (apx): replace an embedded texture with a PNG or \
                       DDS, the Apt movie with a .apt file, the constant table with a .const file \
                       (export writes these parts), and point `$name` text references of the movie at \
                       other strings. One undoable operation with its diff.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn apx_edit(
        &self,
        params: Parameters<tools::assets::ApxEditParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("apx_edit", move |k| tools::assets::apx_edit(k, params.0))
            .await
    }

    #[tool(
        description = "Import an animation file (glTF, FBX, USD) into an animation bank: replace the \
                       animation of a clip, or `add` a new clip with a named entry that plays it. The \
                       answer says the frames, the bones taken from the file, the rest pose, the \
                       encoding error and sizes. One undoable operation with its diff; `dry_run` only \
                       says.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn import_animation(
        &self,
        params: Parameters<tools::assets::ImportAnimationParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("import_animation", move |k| {
            tools::assets::import_animation(k, params.0)
        })
        .await
    }

    #[tool(
        description = "The events of a clip of an animation bank (sounds, effects, messages at a time \
                       of the clip): show them, or replace the whole list with `events`. One undoable \
                       operation with its diff.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn clip_events(
        &self,
        params: Parameters<tools::assets::ClipEventsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("clip_events", move |k| {
            tools::assets::clip_events(k, params.0)
        })
        .await
    }

    #[tool(
        description = "Replace the node graph of an animation bank with a graph in JSON (as \
                       animation_graph writes it). The graph is checked first; one with errors is \
                       refused with the list of problems. One undoable operation with its diff.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn set_animation_graph(
        &self,
        params: Parameters<tools::assets::SetAnimationGraphParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("set_animation_graph", move |k| {
            tools::assets::set_animation_graph(k, params.0)
        })
        .await
    }
}

#[tool_router(router = authoring_game_router, vis = "pub(crate)")]
impl Server {
    #[tool(
        description = "Put back the original of every game file the toolkit changed (from its \
                       backups): the game is as before any apply, also files written outside the \
                       workspace. The edits stay in the workspace, pending. Refused while the game \
                       runs.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn restore_originals(
        &self,
        params: Parameters<tools::files::RestoreOriginalsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("restore_originals", move |k| {
            tools::files::restore_originals(k, params.0)
        })
        .await
    }
}
