//! The tools of in-engine cut-scenes (stage 104): `cutscenes` reads, the
//! others record edits in the workspace (`--allow-edits`).

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use super::Server;
use crate::tools;

#[tool_router(router = cutscene_read_router, vis = "pub(super)")]
impl Server {
    #[tool(
        description = "In-engine cut-scenes: every folder stream with shots and the sequences \
                       that play them (no arguments, `filter`); with `stream` the cut-scene — \
                       shots in playing order, sound tracks with their subtitles and times; \
                       with `stream` and `shot` (resource index or name such as igc16_s02) the \
                       shot — actors, clips, commands with index and time, camera key times; \
                       with `time` also where the camera (position, angles, lens) and the \
                       actors are then. The workspace's version unless `original`. \
                       resource_json of a shot gives every key.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn cutscenes(
        &self,
        params: Parameters<tools::cutscene::CutscenesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("cutscenes", move |k| {
            tools::cutscene::cutscenes(k, params.0)
        })
        .await
    }
}

#[tool_router(router = cutscene_edit_router, vis = "pub(super)")]
impl Server {
    #[tool(
        description = "Edit the camera of a cut-scene shot. With `time` (seconds from the start \
                       of the shot) the key there takes `position`, `rotation` (degrees) and \
                       the lens (`focal_length` in mm or `vertical_fov`); a key that is not \
                       there is added on the path, what is not given stays as the path has it; \
                       `delete_key` removes the keys at `time`. Without `time` the whole shot \
                       changes: `offset` and `turn` move every key, the lens is set on every \
                       key. One undoable operation with its diff; `cutscenes` shows the result.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn cutscene_camera(
        &self,
        params: Parameters<tools::cutscene::CutsceneCameraParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("cutscene_camera", move |k| {
            tools::cutscene::cutscene_camera(k, params.0)
        })
        .await
    }

    #[tool(
        description = "Edit an actor of a cut-scene shot: with `time` its key there takes \
                       `position` and `rotation` (degrees, or `quaternion`), `delete_key` \
                       removes its keys at `time`; without `time`, `offset` moves the actor for \
                       the whole shot (where it is created and every key); `clip` lets it play \
                       another clip of the shot's banks. One undoable operation with its diff.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn cutscene_actor(
        &self,
        params: Parameters<tools::cutscene::CutsceneActorParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("cutscene_actor", move |k| {
            tools::cutscene::cutscene_actor(k, params.0)
        })
        .await
    }

    #[tool(
        description = "Edit a command on the time line of a cut-scene shot: move it to `time` \
                       (the other commands keep theirs) and (or) change `fields` of it — the \
                       text of a text command (time of day, a message of the game, shadow \
                       ranges, time scale), the seconds and colour of a fade, the speed of an \
                       animation, the state of an effect, keys of a curve. One undoable \
                       operation with its diff. A whole shot is replaced with import_file \
                       (the JSON of export).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn cutscene_command(
        &self,
        params: Parameters<tools::cutscene::CutsceneCommandParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("cutscene_command", move |k| {
            tools::cutscene::cutscene_command(k, params.0)
        })
        .await
    }
}
