//! The tools of live debugging and what goes with it (stage 104):
//! `game_log`, `crash_reports` and `detect_games` read; `pause`, `resume`,
//! `breakpoints`, `camera` and `push_textures` act in the running game
//! (`--allow-game`).

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use super::Server;
use crate::tools;

#[tool_router(router = debug_read_router, vis = "pub(super)")]
impl Server {
    #[tool(
        description = "The journal of the running game over the live link: what it prints (Lua \
                       print, traced nodes of visual scripts), Lua errors with their call stack \
                       (`errors` for those alone), with `messages` also the messages of the bus \
                       with their receivers. `after` continues from `next` of the previous \
                       call; `filter` keeps events that contain a text.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn game_log(
        &self,
        params: Parameters<tools::debug::GameLogParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("game_log", move |k| tools::debug::game_log(k, params.0))
            .await
    }

    #[tool(
        description = "The crash reports of the game (written by the crash module when the \
                       game crashes): the list, newest first, with the exception, the address \
                       and what the knowledge base names it; with `report` (1 is the newest) \
                       the text of one. After play ends unexpectedly, look here.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn crash_reports(
        &self,
        params: Parameters<tools::debug::CrashReportsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("crash_reports", move |k| {
            tools::debug::crash_reports(k, params.0)
        })
        .await
    }

    #[tool(
        description = "The copies of the game found on this computer (Steam libraries, Wine \
                       and Proton prefixes, Lutris, Heroic, Bottles) with their edition and how \
                       each can be started, and the edition of the game this server serves.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn detect_games(
        &self,
        params: Parameters<tools::debug::DetectGamesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("detect_games", move |k| {
            tools::debug::detect_games(k, params.0)
        })
        .await
    }
}

#[tool_router(router = debug_game_router, vis = "pub(super)")]
impl Server {
    #[tool(
        description = "Stop the running game in its next level frame. It stays stopped until \
                       resume (or by itself after the hold limit, 120 s unless breakpoints \
                       set another); reading tools and screenshot answer meanwhile.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn pause(
        &self,
        params: Parameters<tools::debug::PauseParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("pause", move |k| tools::debug::pause(k, params.0))
            .await
    }

    #[tool(
        description = "Let a stopped game go on (after pause or a breakpoint), or with `step` \
                       for one frame, after which it stops again.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn resume(
        &self,
        params: Parameters<tools::debug::ResumeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("resume", move |k| tools::debug::resume(k, params.0))
            .await
    }

    #[tool(
        description = "Breakpoints of the running game: stop it when a message goes through \
                       the bus (`messages`, `to` one object), when it prints a text (`prints`; \
                       a traced node of a visual script) or when Lua fails (`lua_error`). \
                       Without rules: the breakpoints and whether the game stands, where and \
                       why. `add` keeps the earlier ones, `clear` removes all, `once` drops a \
                       rule after its hit, `hold_s` is the longest stop, `wait_s` waits for \
                       the stop. A stopped game goes on with resume.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn breakpoints(
        &self,
        params: Parameters<tools::debug::BreakpointsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("breakpoints", move |k| {
            tools::debug::breakpoints(k, params.0)
        })
        .await
    }

    #[tool(
        description = "The free camera: the running game draws from `position`, turned by \
                       `yaw_degrees` and `pitch_degrees` or towards `look_at`; what is not \
                       given stays as the game's camera has it, the field of view is the \
                       game's. \
                       `release` gives the game its camera back. With screenshot: a picture \
                       of the level from any point. The player does not move (player reads \
                       the game's camera).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn camera(
        &self,
        params: Parameters<tools::debug::CameraParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("camera", move |k| tools::debug::camera(k, params.0))
            .await
    }

    #[tool(
        description = "Send the textures the workspace edits into the running game without a \
                       restart (all, or one by `name`): a texture the game has loaded is \
                       replaced when the new one has its size, format and levels. The game \
                       files are not changed.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn push_textures(
        &self,
        params: Parameters<tools::debug::PushTexturesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run("push_textures", move |k| {
            tools::debug::push_textures(k, params.0)
        })
        .await
    }
}
