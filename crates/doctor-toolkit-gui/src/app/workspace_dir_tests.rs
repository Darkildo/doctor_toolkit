//! The editor on a workspace that is not the game's own (stage 105): the
//! folder comes from [`BrowserApp::workspace_dir`] everywhere, so a
//! workspace given to the editor (`DOCTOR_TOOLKIT_GUI_WORKSPACE`, later a
//! project's) is the one its edits go into, its views read and its bridge
//! lends; the game's own folder is not touched.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::testing as mini;
use doctor_toolkit::workspace::Workspace;
use doctor_toolkit::zone_ops::NewStreamParams;

use super::*;
use crate::app::zone::ZoneAction;
use crate::settings::{Settings, Store};

const ZONE: &str = "gf2/town/doctor";

/// An editor without a window on the game in `dir`, with the workspace in
/// `workspace` (the game's own when `None`).
fn open(dir: &Path, workspace: Option<&Path>) -> (egui::Context, BrowserApp) {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = BrowserApp::new(
        &cc,
        None,
        None,
        None,
        Settings::default(),
        Store { dir: None },
    );
    app.notes.log_to_file = false;
    app.level_preset_used = true;
    app.workspace_at = workspace.map(Path::to_owned);
    let game = Arc::new(GameInstance::open(dir).unwrap());
    let (index, errors) = game.build_index(|_| {});
    assert!(errors.is_empty(), "{errors:?}");
    app.game = Some(game);
    app.index = Some(Arc::new(index));
    app.open_workspace();
    (ctx, app)
}

struct Folders {
    game: PathBuf,
    workspace: PathBuf,
}

impl Drop for Folders {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.game);
        let _ = std::fs::remove_dir_all(&self.workspace);
    }
}

fn folders(name: &str) -> Folders {
    let base = std::env::temp_dir().join(format!("dt-wsdir-{name}-{}", std::process::id()));
    let folders = Folders {
        game: base.with_extension("game"),
        workspace: base.with_extension("workspace"),
    };
    for dir in [&folders.game, &folders.workspace] {
        let _ = std::fs::remove_dir_all(dir);
    }
    std::fs::create_dir_all(&folders.game).unwrap();
    mini::write_game(&folders.game).unwrap();
    mini::write_town(&folders.game).unwrap();
    folders
}

fn new_zone() -> ZoneAction {
    ZoneAction::AddStream(NewStreamParams {
        name: ZONE.into(),
        parent: mini::TOWN.into(),
        ..Default::default()
    })
}

#[test]
fn the_editor_works_with_the_workspace_it_was_given() {
    let at = folders("given");
    let own = Workspace::default_dir(&at.game);
    {
        let (_ctx, mut app) = open(&at.game, Some(&at.workspace));
        assert_eq!(app.workspace_dir().as_deref(), Some(at.workspace.as_path()));
        assert!(app.workspace.is_none(), "nothing is recorded yet");

        app.zone_action(new_zone()).unwrap();
        let ws = app.workspace.as_ref().expect("the edit made the workspace");
        assert_eq!(ws.dir(), at.workspace);
        assert!(ws.new_stream(ZONE).is_some());
        assert!(Workspace::exists(&at.workspace));
        assert!(
            !own.exists(),
            "the game's own workspace folder was made: {}",
            own.display()
        );
        // The staged stream is a stream of the game the views show.
        assert!(app.game.as_ref().unwrap().find_stream(ZONE).is_some());

        // The logic editor keeps its layouts and runs in the same folder.
        app.open_logic_tab("", None);
        let logic = app.logics.values().next().expect("a logic tab");
        assert_eq!(logic.workspace_dir(), Some(at.workspace.as_path()));
    }

    // An editor on the game's own workspace does not see the edit…
    {
        let (_ctx, app) = open(&at.game, None);
        assert_eq!(app.workspace_dir(), Some(own.clone()));
        assert!(app.workspace.is_none());
        assert!(app.game.as_ref().unwrap().find_stream(ZONE).is_none());
    }
    // …and one given the folder again does.
    let (_ctx, app) = open(&at.game, Some(&at.workspace));
    let ws = app.workspace.as_ref().expect("the workspace of the folder");
    assert!(ws.new_stream(ZONE).is_some());
    assert!(app.game.as_ref().unwrap().find_stream(ZONE).is_some());
    assert!(!own.exists());
}

#[test]
fn a_relative_folder_does_not_depend_on_where_the_editor_started() {
    let at = folders("relative");
    let (_ctx, mut app) = open(&at.game, None);
    app.workspace_at = Some(PathBuf::from("some/workspace"));
    let dir = app.workspace_dir().unwrap();
    assert!(dir.is_absolute(), "{}", dir.display());
    assert!(dir.ends_with("some/workspace"));
}

/// A program that writes another workspace than the editor's is refused at
/// once: lending the editor's would not let it write its own.
#[cfg(unix)]
#[test]
fn the_bridge_lends_only_the_workspace_of_the_window() {
    use doctor_toolkit::bridge::{Client, Request, Server};
    use std::time::Duration;

    let at = folders("bridge");
    let (ctx, mut app) = open(&at.game, Some(&at.workspace));
    app.zone_action(new_zone()).unwrap();
    let socket = at.workspace.with_extension("sock");
    let _ = std::fs::remove_file(&socket);
    app.serve_bridge_for_test(Server::start(&socket, || {}).unwrap());

    let ask = |workspace: PathBuf| {
        let socket = socket.clone();
        std::thread::spawn(move || {
            Client::connect(&socket)
                .unwrap()
                .request(
                    &Request::Lend {
                        client: "test".into(),
                        summary: "an edit".into(),
                        workspace: Some(workspace),
                    },
                    Duration::from_secs(20),
                )
                .unwrap()
        })
    };
    let other = Workspace::default_dir(&at.game);
    let refused = ask(other.clone());
    while !refused.is_finished() {
        app.bridge_requests(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    let reply = refused.join().unwrap();
    assert!(!reply.ok);
    let error = reply.error.unwrap_or_default();
    assert!(
        error.contains(&at.workspace.display().to_string())
            && error.contains(&other.display().to_string()),
        "{error}"
    );
    assert!(!app.bridge_asks_for_test(), "the user is not asked");

    // Its own folder: the user is asked (nobody answers here).
    let asked = ask(at.workspace.clone());
    let started = std::time::Instant::now();
    while !app.bridge_asks_for_test() {
        assert!(started.elapsed() < Duration::from_secs(20), "no question");
        app.bridge_requests(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(app);
    let _ = asked.join();
    let _ = std::fs::remove_file(&socket);
}
