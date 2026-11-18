//! The project as the document of the editor (stage 106), without a window:
//! a new project takes the edits into its own folder, closing it gives the
//! game's workspace back, opening it again (also from an editor with no
//! game open) shows its edits; the game's workspace becomes a project; a
//! project is not written into the game files.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::testing as mini;
use doctor_toolkit::workspace::Workspace;
use doctor_toolkit::zone_ops::NewStreamParams;

use super::*;
use crate::app::zone::ZoneAction;
use crate::settings::{Settings, Store};

const ZONE: &str = "gf2/town/doctor";

fn editor() -> (egui::Context, BrowserApp) {
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
    (ctx, app)
}

/// An editor without a window on the game in `dir`.
fn open(dir: &Path) -> (egui::Context, BrowserApp) {
    let (ctx, mut app) = editor();
    let game = Arc::new(GameInstance::open(dir).unwrap());
    let (index, errors) = game.build_index(|_| {});
    assert!(errors.is_empty(), "{errors:?}");
    app.game = Some(game);
    app.index = Some(Arc::new(index));
    app.open_workspace();
    (ctx, app)
}

struct Folders {
    base: PathBuf,
    game: PathBuf,
    project: PathBuf,
}

impl Drop for Folders {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn folders(name: &str) -> Folders {
    let base = std::env::temp_dir().join(format!("dt-document-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let folders = Folders {
        game: base.join("game"),
        project: base.join("Yard Mod"),
        base,
    };
    std::fs::create_dir_all(&folders.game).unwrap();
    mini::write_game(&folders.game).unwrap();
    mini::write_town(&folders.game).unwrap();
    folders
}

/// Every file under `root` with its contents.
fn snapshot(root: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, rel: &str, out: &mut std::collections::BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            let rel = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if entry.path().is_dir() {
                walk(&entry.path(), &rel, out);
            } else {
                out.insert(rel, std::fs::read(entry.path()).unwrap());
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(root, "", &mut out);
    out
}

fn new_zone() -> ZoneAction {
    ZoneAction::AddStream(NewStreamParams {
        name: ZONE.into(),
        parent: mini::TOWN.into(),
        ..Default::default()
    })
}

fn shows_zone(app: &BrowserApp) -> bool {
    app.game.as_ref().unwrap().find_stream(ZONE).is_some()
}

/// Stage 112: Write sources of the Project tab gives the document — a
/// project with its own workspace — source files for what the workspace
/// holds: here a stream made in the editor with its SimGroup. A build of a
/// checkout of them would make the zone again; removing the folder and
/// building takes the stream out of the workspace.
#[test]
fn the_sources_of_a_document_are_written_from_the_project_tab() {
    use doctor_toolkit::zone_ops::NewGroupParams;
    let at = folders("sources");
    let (ctx, mut app) = open(&at.game);
    // No project: nothing to write the sources of.
    assert_eq!(app.project_sources(), None);
    app.new_document(&at.project, false, &ctx).unwrap();
    assert_eq!(app.project_sources(), Some(0), "an empty workspace");

    app.zone_action(new_zone()).unwrap();
    app.zone_action(ZoneAction::AddGroup(NewGroupParams {
        stream: ZONE.into(),
        ..Default::default()
    }))
    .unwrap();
    assert_eq!(app.project_sources(), Some(1), "the SimGroup of the zone");
    let folder = at.project.join("src/gf2/town/doctor.str");
    let manifest = std::fs::read_to_string(folder.join("stream.toml")).unwrap();
    assert!(
        manifest.contains("[new]") && manifest.contains("[[resource.added]]"),
        "{manifest}"
    );
    assert_eq!(app.project_sources(), Some(0), "nothing more to write");
    assert_eq!(
        app.project_tab_root_for_test(),
        Some(at.project.clone()),
        "the Project tab shows the document"
    );

    // The folder is removed by hand: Build reads the project again and
    // removes the stream from the workspace; the editor no longer shows it.
    assert!(shows_zone(&app));
    std::fs::remove_dir_all(&folder).unwrap();
    app.project_build(false);
    assert!(app.workspace.as_ref().unwrap().new_stream(ZONE).is_none());
    assert!(!folder.exists());
    assert!(!shows_zone(&app));
    // Undo brings the zone back; its sources are written again.
    app.undo();
    assert!(shows_zone(&app));
    assert_eq!(app.project_sources(), Some(1));
    assert!(folder.join("stream.toml").is_file());
}

#[test]
fn a_project_is_the_document() {
    let at = folders("document");
    let own = Workspace::default_dir(&at.game);
    let (ctx, mut app) = open(&at.game);
    assert!(app.document.is_none());
    assert!(!app.enabled(Command::ProjectClose));
    assert!(app.enabled(Command::ProjectNew));

    app.new_document(&at.project, false, &ctx).unwrap();
    assert_eq!(app.document_name(), Some("Yard Mod"));
    assert!(app.has_project_workspace());
    assert_eq!(app.workspace_dir(), Some(at.project.join("workspace")));
    assert_eq!(
        app.settings.recent_projects,
        std::slice::from_ref(&at.project)
    );
    assert_eq!(
        app.settings.last_project.as_deref(),
        Some(at.project.as_path())
    );
    assert!(app.enabled(Command::ProjectClose));
    // The Project tab shows it.
    assert_eq!(app.project_tab_root_for_test(), Some(at.project.clone()));

    // An edit is the project's.
    app.zone_action(new_zone()).unwrap();
    assert!(Workspace::exists(&at.project.join("workspace")));
    assert!(!own.exists());
    assert!(shows_zone(&app));

    // The editor does not write a project into the game: not by Apply, and
    // the direct mode is off.
    assert!(!app.direct);
    app.apply(false);
    assert!(
        !at.game.join("gf2/town/doctor.str").exists(),
        "the project was written into the game"
    );
    assert!(!at.game.join("_doctor_toolkit/backups").exists());
    assert!(app.project_refuses_write());

    // Closed: the game's own workspace, without the project's edit.
    app.close_document(&ctx);
    assert!(app.document.is_none() && !app.has_project_workspace());
    assert_eq!(app.workspace_dir(), Some(own.clone()));
    assert!(app.workspace.is_none());
    assert!(!shows_zone(&app));
    assert_eq!(app.settings.last_project, None);
    assert!(!app.project_refuses_write());

    // Opened again in the same editor: the edit is back, undone there.
    app.open_document(at.project.clone(), &ctx);
    assert_eq!(app.document_name(), Some("Yard Mod"));
    assert!(shows_zone(&app));
    app.undo();
    assert!(!shows_zone(&app));
    assert!(!own.exists());
}

#[test]
fn a_project_opens_its_game() {
    let at = folders("opens");
    {
        let (ctx, mut app) = open(&at.game);
        app.new_document(&at.project, false, &ctx).unwrap();
        app.zone_action(new_zone()).unwrap();
    }
    // An editor with no game open: the project names its game.
    let (ctx, mut app) = editor();
    app.open_document(at.project.clone(), &ctx);
    assert!(app.job.is_some(), "the game of the project is being opened");
    let started = Instant::now();
    while app.game.is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the game did not open"
        );
        app.handle_results(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app.document_name(), Some("Yard Mod"));
    assert_eq!(app.workspace_dir(), Some(at.project.join("workspace")));
    assert!(shows_zone(&app));

    // A game opened by itself afterwards has no project.
    app.open_game(at.game.clone(), &ctx);
    let started = Instant::now();
    while app.job.is_some() {
        assert!(started.elapsed() < Duration::from_secs(60));
        app.handle_results(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.document.is_none());
    assert_eq!(app.workspace_dir(), Some(Workspace::default_dir(&at.game)));
    assert!(!shows_zone(&app));
}

#[test]
fn the_workspace_of_the_game_becomes_a_project() {
    let at = folders("adopt");
    let own = Workspace::default_dir(&at.game);
    let (ctx, mut app) = open(&at.game);
    // Nothing to move yet.
    assert!(!app.enabled(Command::ProjectFromWorkspace));
    assert!(app.new_document(&at.project, true, &ctx).is_err());
    assert!(!at.project.exists());

    app.zone_action(new_zone()).unwrap();
    assert!(Workspace::exists(&own));
    assert!(app.enabled(Command::ProjectFromWorkspace));

    // Not while the edit is in the game files: nothing moves, no project
    // folder stays, the editor keeps its workspace.
    app.apply(false);
    assert!(at.game.join("gf2/town/doctor.str").exists());
    let error = app.new_document(&at.project, true, &ctx).unwrap_err();
    assert!(error.contains("put the originals back"), "{error}");
    assert!(!at.project.exists());
    assert!(app.document.is_none() && app.workspace.is_some());
    app.unapply();
    assert!(!at.game.join("gf2/town/doctor.str").exists());

    app.new_document(&at.project, true, &ctx).unwrap();
    assert_eq!(app.document_name(), Some("Yard Mod"));
    assert!(!own.exists(), "the game's workspace moved");
    let ws = app.workspace.as_ref().expect("the project's workspace");
    assert_eq!(ws.dir(), at.project.join("workspace"));
    assert!(ws.new_stream(ZONE).is_some());
    assert!(shows_zone(&app));
    // The history came along.
    app.undo();
    assert!(!shows_zone(&app));
    // A project is open: its workspace is not the game's to move.
    assert!(!app.enabled(Command::ProjectFromWorkspace));
    assert!(
        app.new_document(&at.base.join("other"), true, &ctx)
            .is_err()
    );
}

/// Stage 107: a project reaches the game as a mod. With a mod written into
/// the game files, Play stages "the mods and the project on top" outside
/// the game (the workspace alone was refused there); File → Install
/// project into Mods puts the project into the mod manager under its id,
/// last and enabled, and the game files change only when the mods are
/// written.
#[test]
fn a_project_is_played_and_installed_as_a_mod() {
    use doctor_toolkit::mods::{ModInfo, ModManager, build_from_workspace};
    use doctor_toolkit::play::overlay::{Overlay, Source};
    use doctor_toolkit::resource::lh2::StringTable;

    let at = folders("as-a-mod");
    // A base mod written into the game by the mod manager.
    {
        let game = GameInstance::open(&at.game).unwrap();
        let mut ws = Workspace::for_game(&game, Some(&at.base.join("base-ws"))).unwrap();
        let text = game.find_stream(mini::TEXT).unwrap();
        let mut table = StringTable::parse(&ws.current_data(&game, text, 0).unwrap()).unwrap();
        table.strings[0][0] = "Base".into();
        ws.set_string_table(&game, text, 0, &table).unwrap();
        let package = at.base.join("base.zip");
        let info = ModInfo {
            id: "base".into(),
            name: "Base".into(),
            version: "1".into(),
            ..Default::default()
        };
        build_from_workspace(&ws, &info, None, None, &package).unwrap();
        let mut mods = ModManager::open(&game).unwrap();
        mods.install_file(&game, &package, true).unwrap();
        mods.apply(&game, Default::default()).unwrap();
    }
    let (ctx, mut app) = open(&at.game);
    // No project, then a project without edits: nothing to install.
    assert!(!app.enabled(Command::ProjectInstall));
    assert_eq!(app.project_root(), None);
    app.new_document(&at.project, false, &ctx).unwrap();
    assert!(!app.enabled(Command::ProjectInstall));
    assert_eq!(app.project_root(), Some(at.project.clone()));
    app.zone_action(new_zone()).unwrap();
    assert!(app.enabled(Command::ProjectInstall));

    // Play: what the editor stages for the launch holds the project's zone
    // on top of the mod, and the game folder stays as it is.
    let game = app.disk_game().unwrap();
    let before = snapshot(&at.game);
    assert!(before.contains_key("_doctor_toolkit/mods/registry.json"));
    let source = Source::Project(app.project_root().unwrap());
    let overlay = Overlay::build(&game, Some(&source), None, &at.base.join("overlay")).unwrap();
    let staged: Vec<&str> = overlay
        .redirects()
        .iter()
        .map(|r| r.from.as_str())
        .collect();
    assert!(staged.contains(&"gf2/town/doctor.str"), "{staged:?}");
    // The mod's stream is the game's own file: it is written there already.
    assert!(!staged.contains(&"gf2/text/abcd.str"), "{staged:?}");
    overlay.remove().unwrap();
    assert!(
        snapshot(&at.game) == before,
        "staging the project changed the game folder"
    );
    assert!(!at.game.join("gf2/town/doctor.str").exists());

    // Install into Mods: the project's mod, last and enabled; the Mods
    // window opens on it; the game files wait for the mods to be written.
    app.install_document(&ctx);
    assert!(app.mods_open);
    assert_eq!(app.mods.selected(), Some("yard-mod"));
    let mut mods = ModManager::open(&game).unwrap();
    let ids: Vec<&str> = mods.mods().iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["base", "yard-mod"]);
    let installed = mods.get("yard-mod").unwrap();
    assert!(installed.enabled);
    assert_eq!(installed.manifest.name, "Yard Mod");
    assert_eq!(installed.manifest.streams.len(), 1);
    assert!(!at.game.join("gf2/town/doctor.str").exists());
    mods.apply(&game, Default::default()).unwrap();
    assert!(at.game.join("gf2/town/doctor.str").exists());

    // The Mods window of the editor builds the same mod: its form has the
    // project's id.
    assert_eq!(app.mods.build_info().id, "yard-mod");

    // Without a project there is nothing to install.
    app.close_document(&ctx);
    assert!(!app.enabled(Command::ProjectInstall));
}

/// One frame of the editor.
fn frame(ctx: &egui::Context, app: &mut BrowserApp) {
    use eframe::App as _;
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 800.0),
        )),
        ..Default::default()
    };
    let mut eframe_frame = eframe::Frame::_new_kittest();
    let mut output = ctx.run_ui(input, |ui| app.ui(ui, &mut eframe_frame));
    output.textures_delta.clear();
}

/// Draws frames until the Mods window has done what it was asked.
#[track_caller]
fn wait_for_mods(ctx: &egui::Context, app: &mut BrowserApp) {
    let started = Instant::now();
    loop {
        frame(ctx, app);
        if !app.base.bring_queued && !app.mods.is_busy() && app.mods.manager().is_some() {
            // The frame that tells the editor what the job did.
            frame(ctx, app);
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the Mods window is still busy: {:?}",
            app.mods.busy_label()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Installs a mod `id` of `version` that sets the first word of a string
/// table of the game in `at`.
fn install_mod(at: &Folders, id: &str, version: &str, place: (&str, usize), word: &str) {
    use doctor_toolkit::mods::{ModInfo, ModManager, build_from_workspace};
    use doctor_toolkit::resource::lh2::StringTable;

    let game = GameInstance::open(&at.game).unwrap();
    let ws_dir = at.base.join(format!("ws-{id}-{version}"));
    let mut ws = Workspace::for_game(&game, Some(&ws_dir)).unwrap();
    let stream = game.find_stream(place.0).unwrap();
    let mut table = StringTable::parse(&ws.current_data(&game, stream, place.1).unwrap()).unwrap();
    table.strings[0][0] = word.into();
    ws.set_string_table(&game, stream, place.1, &table).unwrap();
    let package = at.base.join(format!("{id}-{version}.zip"));
    let info = ModInfo {
        id: id.into(),
        name: format!("Mod {id}"),
        version: version.into(),
        ..Default::default()
    };
    build_from_workspace(&ws, &info, None, None, &package).unwrap();
    ModManager::open(&game)
        .unwrap()
        .install_file(&game, &package, true)
        .unwrap();
}

/// Stage 108: the base of a project. Made on a game with mods, the project
/// is offered to take them as its base and takes none by itself; with the
/// base recorded, a game whose mods are others shows the bar at the top of
/// the window, and "Bring the mods to the project's base" saves how the
/// mods were as a set, puts the base into the game and takes the bar away.
#[test]
fn the_mods_of_the_game_are_compared_with_the_base_of_the_project() {
    use doctor_toolkit::mods::ModManager;
    use doctor_toolkit::project::{BaseDifference, ProjectConfig};

    let at = folders("base");
    let mods = || ModManager::open(&GameInstance::open(&at.game).unwrap()).unwrap();
    let apply = || {
        let game = GameInstance::open(&at.game).unwrap();
        ModManager::open(&game)
            .unwrap()
            .apply(&game, Default::default())
            .unwrap();
    };
    let arrangement = || -> Vec<(String, bool)> {
        mods()
            .arrangement()
            .into_iter()
            .map(|e| (e.id, e.enabled))
            .collect()
    };
    let pairs = |list: &[(&str, bool)]| -> Vec<(String, bool)> {
        list.iter()
            .map(|(id, on)| ((*id).to_owned(), *on))
            .collect()
    };
    install_mod(&at, "city", "1.1.0", (mini::TEXT, 0), "City");
    install_mod(&at, "lights", "1.0", (mini::LEVEL, mini::STRINGS), "Lights");
    apply();

    let (ctx, mut app) = open(&at.game);
    assert!(!app.base_bar_shown());
    assert!(!app.enabled(Command::ProjectBaseRecord));

    // A new project on a game with mods: the offer, and no base by itself.
    app.new_document(&at.project, false, &ctx).unwrap();
    assert!(app.base_bar_shown());
    assert_eq!(app.base.offer, ["Mod city", "Mod lights"]);
    assert!(app.base.differences.is_empty());
    assert_eq!(ProjectConfig::read(&at.project).unwrap().base, None);
    assert!(app.enabled(Command::ProjectBaseRecord));
    assert!(!app.enabled(Command::ProjectBaseApply));
    frame(&ctx, &mut app);
    assert_eq!(ProjectConfig::read(&at.project).unwrap().base, None);

    // Taken: the base is in project.toml, the bar goes, and the mod the
    // Mods window builds of the project depends on the base.
    app.run(Command::ProjectBaseRecord, &ctx);
    let config = ProjectConfig::read(&at.project).unwrap();
    let base = config.base.as_ref().expect("the base is recorded");
    assert_eq!(config.format, 2);
    assert!(base.exclusive);
    let ids: Vec<(&str, &str)> = base
        .mods
        .iter()
        .map(|m| (m.id.as_str(), m.version.as_str()))
        .collect();
    assert_eq!(ids, [("city", "^1.1.0"), ("lights", "^1.0")]);
    assert!(!app.base_bar_shown(), "{:?}", app.base.differences);
    let depends: Vec<(String, String)> = app
        .mods
        .build_info()
        .relations
        .depends
        .into_iter()
        .collect();
    assert_eq!(
        depends,
        [
            ("city".to_owned(), "^1.1.0".to_owned()),
            ("lights".to_owned(), "^1.0".to_owned())
        ]
    );

    // Another program arranges the mods otherwise and writes them.
    install_mod(&at, "extra", "1.0", (mini::TEXT, 0), "Extra");
    mods().set_enabled("lights", false).unwrap();
    apply();
    let mine = pairs(&[("city", true), ("lights", false), ("extra", true)]);
    assert_eq!(arrangement(), mine);
    app.check_base();
    assert!(app.base_bar_shown());
    assert_eq!(
        app.base.differences,
        [
            BaseDifference::Disabled {
                id: "lights".into()
            },
            BaseDifference::Extra {
                id: "extra".into(),
                written: true
            }
        ]
    );
    assert!(app.enabled(Command::ProjectBaseApply));
    // The bar draws.
    frame(&ctx, &mut app);

    // Brought: the Mods window opens and does it.
    app.run(Command::ProjectBaseApply, &ctx);
    assert!(app.mods_open && app.base.bring_queued);
    wait_for_mods(&ctx, &mut app);
    assert_eq!(
        arrangement(),
        pairs(&[("city", true), ("lights", true), ("extra", false)])
    );
    assert!(
        app.base.differences.is_empty(),
        "{:?}",
        app.base.differences
    );
    assert!(!app.base_bar_shown());
    assert!(!app.enabled(Command::ProjectBaseApply));
    assert!(
        mods()
            .in_game(&app.disk_game().unwrap())
            .unwrap()
            .written()
            .count()
            == 2
    );
    // The way back is a set of the Mods window.
    assert_eq!(app.mods.sets(), (&["Before Yard Mod".to_owned()][..], None));
    app.mods.use_set(&ctx, "Before Yard Mod");
    wait_for_mods(&ctx, &mut app);
    app.mods.apply(&ctx);
    wait_for_mods(&ctx, &mut app);
    assert_eq!(arrangement(), mine);
    assert!(app.base_bar_shown(), "the mods are not the base again");
    assert_eq!(app.base.differences.len(), 2);

    // A mod of the base that is gone cannot be brought, and the project
    // cannot be played on a game without it: both say which.
    mods().uninstall("lights").unwrap();
    app.check_base();
    assert!(!app.enabled(Command::ProjectBaseApply));
    app.bring_mods_to_base();
    assert!(app.status.contains("lights"), "{}", app.status);
    assert!(!app.base.bring_queued);
    assert!(app.base_stops_play());
    assert!(app.status.contains("lights"), "{}", app.status);
    // Another version in range is said and does not stop the launch.
    mods().set_enabled("extra", false).unwrap();
    install_mod(
        &at,
        "lights",
        "1.2.0",
        (mini::LEVEL, mini::STRINGS),
        "Lights 1.2",
    );
    apply();
    assert!(!app.base_stops_play());
    assert_eq!(
        app.base.differences,
        [BaseDifference::Changed {
            id: "lights".into(),
            built_on: "1.0".into(),
            found: "1.2.0".into()
        }]
    );
    assert!(app.status.contains("lights"), "{}", app.status);
    // The base taken from the game again is the mods as they are.
    app.run(Command::ProjectBaseRecord, &ctx);
    assert!(!app.base_bar_shown());
    let base = ProjectConfig::read(&at.project).unwrap().base.unwrap();
    assert_eq!(base.mods[1].built_on, "1.2.0");

    // Without the project there is nothing to compare.
    mods().set_enabled("city", false).unwrap();
    app.check_base();
    assert!(app.base_bar_shown());
    app.close_document(&ctx);
    assert!(!app.base_bar_shown());
    frame(&ctx, &mut app);
    assert!(!app.enabled(Command::ProjectBaseRecord));
    assert!(app.mods.build_info().relations.depends.is_empty());
}

/// Stage 111: the base mod of a project is updated under its edit. The bar
/// at the top of the window tells that an edit was made on other data,
/// Play says so and starts all the same, Rebase moves the edit onto what
/// the game gives now and takes the notice away, and one undo brings both
/// back; the Changes panel draws the stale edit with its three ways.
#[test]
fn stale_edits_are_told_in_the_bar_and_rebased() {
    use doctor_toolkit::mods::ModManager;
    use doctor_toolkit::project::BaseDifference;
    use doctor_toolkit::resource::lh2::StringTable;
    use doctor_toolkit::workspace::ResourceKey;

    use crate::app::document::Rebase;

    let at = folders("rebase");
    let apply = || {
        let game = GameInstance::open(&at.game).unwrap();
        ModManager::open(&game)
            .unwrap()
            .apply(&game, Default::default())
            .unwrap();
    };
    let place = (mini::LEVEL, mini::STRINGS);
    install_mod(&at, "city", "1.0.0", place, "City");
    apply();

    let (ctx, mut app) = open(&at.game);
    app.new_document(&at.project, false, &ctx).unwrap();
    app.run(Command::ProjectBaseRecord, &ctx);
    assert!(!app.base_bar_shown(), "{:?}", app.base.differences);

    // The edit of the project: the second word of the table whose first
    // one the base mod sets.
    let game = app.disk_game().unwrap();
    let level = game.find_stream(mini::LEVEL).unwrap();
    let ws = app.workspace_mut().unwrap();
    let data = ws.current_data(&game, level, mini::STRINGS).unwrap();
    let mut table = StringTable::parse(&data).unwrap();
    assert_eq!(table.strings[0][0], "City", "made on the base");
    table.strings[0][1] = "Mine".into();
    ws.set_string_table(&game, level, mini::STRINGS, &table)
        .unwrap();
    app.after_edit(None);
    let key: ResourceKey = app
        .workspace
        .as_ref()
        .unwrap()
        .changes()
        .next()
        .unwrap()
        .key
        .clone();
    let words = |app: &BrowserApp| -> Option<Vec<String>> {
        let data = app.workspace.as_ref()?.data(&key).unwrap()?;
        Some(StringTable::parse(&data).unwrap().strings[0].clone())
    };
    assert!(app.base.stale.is_empty());
    assert!(!app.base_bar_shown());
    assert!(!app.enabled(Command::ProjectRebase));

    // Another program updates the base mod and writes the mods.
    install_mod(&at, "city", "1.1.0", place, "Town");
    apply();
    app.check_base();
    assert_eq!(app.base.stale.len(), 1);
    assert_eq!(app.base.stale[0].key, key);
    assert!(app.base_bar_shown());
    assert!(
        matches!(app.base.differences[..], [BaseDifference::Changed { .. }]),
        "{:?}",
        app.base.differences
    );
    assert!(app.enabled(Command::ProjectRebase));
    // The bar draws, and so does the Changes panel with the stale edit.
    frame(&ctx, &mut app);
    app.changes.select(key.clone());
    let mut output = ctx.run_ui(Default::default(), |ui| app.changes_pane(ui));
    output.textures_delta.clear();
    // Play is not stopped by a stale edit, and says what it means.
    assert!(!app.base_stops_play());
    assert!(
        app.status.contains("1 edit was made on other data"),
        "{}",
        app.status
    );

    // Rebased: one operation, the edit on the new word of the base.
    let head = app.workspace.as_ref().unwrap().head();
    app.run(Command::ProjectRebase, &ctx);
    assert!(app.base.stale.is_empty(), "{}", app.status);
    assert!(app.status.contains("1 edit rebased"), "{}", app.status);
    assert_eq!(app.workspace.as_ref().unwrap().head(), head + 1);
    assert_eq!(words(&app).unwrap(), ["Town", "Mine"]);
    assert!(!app.enabled(Command::ProjectRebase));
    // What is left in the bar is the base itself, until it is taken anew.
    assert!(app.base_bar_shown());
    app.run(Command::ProjectBaseRecord, &ctx);
    assert!(!app.base_bar_shown(), "{:?}", app.base.differences);
    frame(&ctx, &mut app);

    // One undo: the edit as it was, stale again, and the bar is back.
    app.undo();
    assert_eq!(words(&app).unwrap(), ["City", "Mine"]);
    assert_eq!(app.base.stale.len(), 1);
    assert!(app.base_bar_shown());
    assert!(app.enabled(Command::ProjectRebase));

    // The other two ways, as the Changes panel asks for them.
    app.rebase(Rebase::KeepMine(vec![key.clone()]));
    assert!(app.base.stale.is_empty(), "{}", app.status);
    assert_eq!(words(&app).unwrap(), ["City", "Mine"], "mine, as it was");
    app.undo();
    assert_eq!(app.base.stale.len(), 1);
    app.rebase(Rebase::TakeBase(vec![key.clone()]));
    assert!(app.base.stale.is_empty(), "{}", app.status);
    assert_eq!(words(&app), None, "the edit is given up");
    assert!(!app.base_bar_shown());
    app.undo();
    assert_eq!(words(&app).unwrap(), ["City", "Mine"]);

    // Without the project the bar has nothing to tell.
    app.close_document(&ctx);
    assert!(!app.base_bar_shown());
}
