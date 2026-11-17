//! Every tool of the server is in the table of capabilities
//! (`data/toolkit/capabilities.toml`, see `docs/parity.md`), and the table
//! names no tool the server does not have.

use doctor_toolkit::parity::{self, Kind};
use doctor_toolkit_mcp::server::{Gate, Server};

#[test]
fn every_tool_is_in_the_table_of_capabilities() {
    let catalog = Server::catalog();
    assert!(catalog.len() > 60, "{}", catalog.len());
    let names: Vec<&str> = catalog.iter().map(|(name, _)| name.as_str()).collect();
    let table = parity::table();
    let problems = table.check_names(Kind::Mcp, names.iter().copied(), names.iter().copied());
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn the_table_has_the_flag_of_every_tool() {
    let table = parity::table();
    for (name, gate) in Server::catalog() {
        let listed = match table.gate(&name) {
            parity::Gate::Read => Gate::Read,
            parity::Gate::Edits => Gate::Edits,
            parity::Gate::Game => Gate::Game,
        };
        assert_eq!(listed, gate, "{name}: [mcp_gates] of the table");
    }
}
