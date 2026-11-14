//! The asset editors as document tabs (stage 56), driven without a window
//! on the miniature game of `doctor_toolkit::testing`: every editor opens
//! as a tab in the Asset Editor layout, Details shows its selection, the
//! asset commands of the tab menus, undo through the workspace history
//! (and the script editor's own), F for the editor in front.

use std::path::PathBuf;
use std::sync::Arc;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::testing as mini;

use super::asset_tab::AssetCmd;
use super::details::tests::frame;
use super::details::{DocRef, Focus};
use super::dock::{self, Pane};
use super::layouts::Screen;
use super::{BrowserApp, Command};
use crate::nav::Location;
use crate::settings::{Settings, Store};
use crate::views::{TabBody, ViewAction};

struct Fixture {
    dir: PathBuf,
    ctx: egui::Context,
    app: BrowserApp,
    stream: usize,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fixture(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("dt-assets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    mini::write_game(&dir).unwrap();
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
    let game = Arc::new(GameInstance::open(&dir).unwrap());
    let (index, errors) = game.build_index(|_| {});
    assert!(errors.is_empty(), "{errors:?}");
    let stream = game.find_stream(mini::LEVEL).unwrap();
    app.game = Some(game);
    app.index = Some(Arc::new(index));
    frame(&ctx, &mut app);
    Fixture {
        dir,
        ctx,
        app,
        stream,
    }
}

impl Fixture {
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            frame(&self.ctx, &mut self.app);
        }
    }

    fn loc(&self, resource: usize) -> Location {
        Location {
            stream: self.stream,
            resource,
        }
    }

    fn tree(&self) -> &egui_tiles::Tree<Pane> {
        self.app.dock.as_ref().unwrap()
    }

    fn has(&self, pane: &Pane) -> bool {
        dock::find(self.tree(), |p| p == pane).is_some()
    }

    fn shown(&self, pane: &Pane) -> bool {
        dock::find(self.tree(), |p| p == pane).is_some_and(|t| dock::is_shown(self.tree(), t))
    }

    /// Opens a resource in its tab and draws it.
    fn open(&mut self, resource: usize) -> u64 {
        let id = self.app.open_resource_tab(self.loc(resource));
        self.frames(3);
        id
    }
}

#[test]
fn every_editor_opens_as_a_tab_of_the_asset_editor_layout() {
    let mut f = fixture("tabs");
    for (resource, fills) in [
        (mini::MESH, false), // no GPU in the tests: the software preview scrolls
        (mini::TEX_HEADER, true),
        (mini::STRINGS, true),
        (mini::PARTSDB, true),
        (mini::SIMGROUP, true),
    ] {
        let id = f.open(resource);
        let pane = dock::find(f.tree(), |p| p.id() == Some(id))
            .and_then(|t| dock::pane(f.tree(), t).cloned())
            .unwrap();
        assert!(matches!(pane, Pane::Resource { .. }), "{resource}");
        assert!(f.shown(&pane), "the tab of {resource} is in front");
        let doc = &f.app.docs[&id];
        let any_fill = doc.views.iter().any(|(_, v)| v.tab_body() == TabBody::Fill);
        if fills {
            assert!(any_fill, "an editor fills the tab of {resource}");
        }
        assert_eq!(
            f.app.inspect,
            Focus::Document(DocRef::Tab(id)),
            "Details follow the tab of {resource}"
        );
    }
    // The tabs share one group, on the Asset Editor screen with Details.
    assert_eq!(f.app.layouts.screen, Screen::Assets);
    assert!(f.shown(&Pane::Details));
    assert!(f.app.details_follow(), "Details is open and follows");
    // The script editor and Movies: one tab each, a second command shows it.
    f.app.pending.push(Command::ScriptEditor);
    f.frames(2);
    f.app.pending.push(Command::ScriptEditor);
    f.frames(2);
    let scripts = f
        .tree()
        .tiles
        .tiles()
        .filter(|t| matches!(t, egui_tiles::Tile::Pane(Pane::Script)))
        .count();
    assert_eq!(scripts, 1);
    assert!(f.app.script_editor.open);
    assert_eq!(f.app.inspect, Focus::Document(DocRef::Script));
    f.app.pending.push(Command::Movies);
    f.frames(2);
    assert!(f.shown(&Pane::Movies));
    assert!(f.app.movies.open);
    assert_eq!(f.app.inspect, Focus::Document(DocRef::Movies));
    // The logic graph is a tab of the same screen.
    f.app.pending.push(Command::LogicEditor);
    f.frames(2);
    assert!(
        dock::find(f.tree(), |p| matches!(p, Pane::Logic { .. })).is_some(),
        "logic tab"
    );
    assert_eq!(f.app.layouts.screen, Screen::Assets);
    // Closing the tabs closes the editors.
    for pane in [Pane::Script, Pane::Movies] {
        let tile = dock::find(f.tree(), |p| *p == pane).unwrap();
        f.app.dock_ops.push(dock::DockOp::Close(tile));
        f.frames(1);
        assert!(!f.has(&pane), "{pane:?}");
    }
    assert!(!f.app.script_editor.open);
    assert!(!f.app.movies.open);
}

#[test]
fn details_show_the_selection_of_the_editor_and_its_header() {
    let mut f = fixture("details");
    let id = f.open(mini::STRINGS);
    assert_eq!(f.app.inspect, Focus::Document(DocRef::Tab(id)));
    // The Details panel draws the document (header, references) without
    // losing it.
    f.frames(2);
    assert!(f.app.docs.contains_key(&id));
    // A list selection takes Details back; a click on the tab follows it again.
    f.app.inspect = Focus::Resource;
    let tile = dock::find(f.tree(), |p| p.id() == Some(id)).unwrap();
    let pane = dock::pane(f.tree(), tile).cloned().unwrap();
    f.app.focus_document(&pane);
    assert_eq!(f.app.inspect, Focus::Document(DocRef::Tab(id)));
    // Pinned, Details stops following: the editors show their own column.
    f.app.inspectors.main.pinned = Some(super::details::Pinned::Document(DocRef::Tab(id)));
    f.frames(1);
    assert!(!f.app.details_follow());
    f.app.inspectors.main.pinned = None;
    f.frames(1);
    assert!(f.app.details_follow());
    // Maximized, Details is hidden: the same.
    f.app.dock_ops.push(dock::DockOp::Maximize(Some(id)));
    f.frames(2);
    assert!(!f.app.details_follow());
    f.app.dock_ops.push(dock::DockOp::ToggleMaximize(tile));
    f.frames(2);
    assert!(f.app.details_follow());
}

#[test]
fn asset_commands_of_the_tab_menus() {
    let mut f = fixture("commands");
    let id = f.open(mini::TEX_HEADER);
    let loc = f.loc(mini::TEX_HEADER);
    let key = f.app.key_of(loc).unwrap().to_string();
    f.app.asset_cmds.push((loc, AssetCmd::CopyKey));
    f.frames(1);
    assert!(f.app.status.contains(&key), "{}", f.app.status);
    // Show in Content Browser: the drawer of the Asset Editor opens.
    f.app.asset_cmds.push((loc, AssetCmd::ShowInContent));
    f.frames(2);
    assert_eq!(f.app.drawers.open, Some(Pane::ContentBrowser));
    // Show in stream: the stream tab of the resource.
    f.app.asset_cmds.push((loc, AssetCmd::ShowInStream));
    f.frames(2);
    assert!(dock::find(f.tree(), |p| matches!(p, Pane::Stream { .. })).is_some());
    // Open in tab: the tab it has.
    let before = f.app.docs.len();
    f.app.asset_cmds.push((loc, AssetCmd::OpenTab));
    f.frames(2);
    assert_eq!(f.app.docs.len(), before);
    assert!(f.app.docs.contains_key(&id));
}

#[test]
fn edits_of_an_editor_go_through_the_workspace_history() {
    let mut f = fixture("undo");
    let loc = f.loc(mini::STRINGS);
    let _ = f.open(mini::STRINGS);
    let table = mini::sample_table(["edited", "words"]);
    f.app.view_action(loc, ViewAction::SaveStrings(table));
    f.frames(1);
    let ws = f.app.workspace.as_ref().unwrap();
    assert!(ws.undo_label().is_some(), "one operation");
    // The History panel lists it; Ctrl+Z takes it back.
    f.app.pending.push(Command::Undo);
    f.frames(1);
    let ws = f.app.workspace.as_ref().unwrap();
    assert!(ws.undo_label().is_none());
    assert!(ws.redo_label().is_some());
    f.app.pending.push(Command::Redo);
    f.frames(1);
    assert!(f.app.workspace.as_ref().unwrap().undo_label().is_some());
}

#[test]
fn the_script_editor_in_front_undoes_its_own_edits() {
    let mut f = fixture("script-undo");
    f.app.pending.push(Command::ScriptEditor);
    f.frames(2);
    assert!(f.app.front_is(&Pane::Script));
    let name = f.app.script_editor.graph().name.clone();
    f.app.script_editor.rename_for_test("renamed");
    assert!(f.app.enabled(Command::Undo));
    f.app.pending.push(Command::Undo);
    f.frames(1);
    assert_eq!(f.app.script_editor.graph().name, name);
    f.app.pending.push(Command::Redo);
    f.frames(1);
    assert_eq!(f.app.script_editor.graph().name, "renamed");
    // Nothing went into the workspace.
    assert!(
        f.app
            .workspace
            .as_ref()
            .is_none_or(|ws| ws.undo_label().is_none())
    );
}

#[test]
fn f_frames_the_editor_in_front() {
    let mut f = fixture("frame");
    f.app.pending.push(Command::ScriptEditor);
    f.frames(2);
    let event = egui::Event::Key {
        key: egui::Key::F,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    // The key is taken before the widgets: the editor in front framed.
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 800.0),
        )),
        ..Default::default()
    };
    input.events.push(event);
    let mut seen = true;
    let mut frame_out = eframe::Frame::_new_kittest();
    let ctx = f.ctx.clone();
    let mut output = ctx.run_ui(input, |ui| {
        use eframe::App as _;
        f.app.ui(ui, &mut frame_out);
        seen = ui.input(|i| i.key_pressed(egui::Key::F));
    });
    output.textures_delta.clear();
    assert!(!seen, "F was consumed for the script editor");
}
