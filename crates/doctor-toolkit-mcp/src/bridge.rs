//! The bridge with the open editor (`doctor_toolkit::bridge`): what the
//! user has selected, showing a resource or an object there, and writes
//! the user allows in the editor while it holds the workspace.

use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;

pub use doctor_toolkit::bridge::EDITOR;
use doctor_toolkit::bridge::{Reply, Request};

use crate::state::{ToolError, ToolResult, Toolkit, fail};
use crate::tools::Output;

/// How long the user may take to allow a write in the editor.
const ASK_TIMEOUT: Duration = Duration::from_secs(180);
/// How long the editor may take to answer anything else (one frame or a few).
const TIMEOUT: Duration = Duration::from_secs(10);

/// Who asks, as the editor shows it.
const CLIENT: &str = "an AI client (doctor-toolkit mcp)";

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct EditorSelectionParams {}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct EditorOpenParams {
    /// A resource key `stream:type:ids` (from `search`), or a search query
    /// with exactly one result.
    pub resource: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct EditorShowObjectParams {
    /// The object: `<stream>#<resource>:<object>`.
    pub object: String,
    /// `level` (the 3D level tab, default) or `logic` (the logic graph tab).
    #[serde(default)]
    pub view: Option<String>,
}

#[cfg(unix)]
fn ask(kit: &Toolkit, request: &Request, timeout: Duration) -> ToolResult<Reply> {
    use doctor_toolkit::bridge::Client;
    let path = kit.bridge.as_ref().ok_or_else(|| {
        ToolError("no settings folder: the editor's bridge cannot be found".into())
    })?;
    let mut client = Client::connect(path).map_err(|e| {
        ToolError(format!(
            "the editor does not answer on its bridge ({e}): it is not running, or is an older one"
        ))
    })?;
    client
        .request(request, timeout)
        .map_err(|e| ToolError(format!("the editor's bridge: {e}")))
}

#[cfg(not(unix))]
fn ask(_: &Toolkit, _: &Request, _: Duration) -> ToolResult<Reply> {
    fail("the bridge with the editor is not there on this system yet")
}

fn answer(reply: Reply) -> ToolResult<Output> {
    if reply.ok {
        Ok(Output::Json(reply.result))
    } else {
        fail(reply.error.unwrap_or_else(|| "the editor refused".into()))
    }
}

pub fn editor_selection(kit: &Toolkit, _: EditorSelectionParams) -> ToolResult<Output> {
    answer(ask(kit, &Request::Selection, TIMEOUT)?)
}

pub fn editor_open(kit: &Toolkit, p: EditorOpenParams) -> ToolResult<Output> {
    let id = crate::tools::resolve(kit, &p.resource)?;
    let key = kit.index().key(id).to_string();
    answer(ask(kit, &Request::Open { key }, TIMEOUT)?)
}

pub fn editor_show_object(kit: &Toolkit, p: EditorShowObjectParams) -> ToolResult<Output> {
    if crate::tools::parse_object_id(kit, &p.object)?.is_none() {
        return fail(format!(
            "`{}`: give an object as `<stream>#<resource>:<object>`",
            p.object
        ));
    }
    let request = match p.view.as_deref().unwrap_or("level") {
        "level" => Request::ShowObject { object: p.object },
        "logic" => Request::ShowLogic { object: p.object },
        other => return fail(format!("`view` is `level` or `logic`, not `{other}`")),
    };
    answer(ask(kit, &request, TIMEOUT)?)
}

/// The editor's workspace, lent for one write; given back when dropped
/// through [`Lent::give_back`].
pub struct Lent {
    #[cfg(unix)]
    client: doctor_toolkit::bridge::Client,
}

impl Lent {
    /// The write is done: the editor takes its workspace back and reads it again.
    pub fn give_back(self) {
        #[cfg(unix)]
        {
            let mut client = self.client;
            let _ = client.request(&Request::Return, TIMEOUT);
        }
    }
}

/// Asks the user in the editor to let this server write the workspace for
/// `what`; waits for the answer.
pub fn lend(kit: &Toolkit, what: &str) -> ToolResult<Lent> {
    #[cfg(unix)]
    {
        use doctor_toolkit::bridge::Client;
        let path = kit
            .bridge
            .as_ref()
            .ok_or_else(|| ToolError("there is no bridge to ask the editor".into()))?;
        let mut client = Client::connect(path).map_err(|e| {
            ToolError(format!(
                "its bridge does not answer ({e}); edit there, or close the game in the editor"
            ))
        })?;
        let reply = client
            .request(
                &Request::Lend {
                    client: CLIENT.into(),
                    summary: what.to_owned(),
                },
                ASK_TIMEOUT,
            )
            .map_err(|e| {
                ToolError(format!(
                    "the editor did not answer whether the write is allowed ({e}); nothing was \
                     written"
                ))
            })?;
        if !reply.ok {
            return fail(format!(
                "the write was not allowed there ({}); nothing was written",
                reply.error.unwrap_or_default()
            ));
        }
        Ok(Lent { client })
    }
    #[cfg(not(unix))]
    {
        let _ = (kit, what);
        fail(
            "the workspace is open for writing in the editor; edit there, or close the game in the editor",
        )
    }
}
