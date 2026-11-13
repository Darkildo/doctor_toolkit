//! A Model Context Protocol server for doctor-toolkit: an AI client (Claude
//! Code, Claude Desktop, any MCP client) reads the game through the
//! toolkit — search, resources as JSON, the logic graph, level objects,
//! strings, the workspace — over stdio, without network. Stage 50 only
//! reads; see `docs/mcp.md`.
//!
//! The command line starts it as `doctor-toolkit mcp GAME`.

pub mod output;
pub mod resources;
pub mod server;
pub mod state;
pub mod tools;

use std::sync::Arc;

use rmcp::ServiceExt;

pub use server::Server;
pub use state::{Config, Toolkit};

/// An error of starting or running the server.
pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// Opens the game of `config` and builds the game index in the background,
/// so that the first tool call waits less.
pub fn open(config: &Config) -> Result<Arc<Toolkit>, Error> {
    let kit = Arc::new(Toolkit::open(config)?);
    let warm = kit.clone();
    std::thread::Builder::new()
        .name("index".into())
        .spawn(move || {
            warm.index();
        })?;
    Ok(kit)
}

/// Serves `kit` over a transport (stdio, or a pipe in tests) until the
/// client disconnects.
pub async fn serve<T, E, A>(kit: Arc<Toolkit>, transport: T) -> Result<(), Error>
where
    T: rmcp::transport::IntoTransport<rmcp::RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let running = Server::new(kit).serve(transport).await?;
    running.waiting().await?;
    Ok(())
}

/// Serves the game of `config` on standard input and output until the
/// client closes them. Nothing but protocol messages goes to stdout.
pub fn serve_stdio(config: &Config) -> Result<(), Error> {
    let kit = open(config)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(serve(kit, rmcp::transport::stdio()))
}
