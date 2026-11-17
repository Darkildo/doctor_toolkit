//! A new zone made in the window on the miniature game with its town
//! (`doctor_toolkit::testing::write_town`), without a window and without
//! writing into the game: the New zone wizard, the forms of the zone
//! window, the level view on the staged zone, objects added to it from the
//! neighbouring zone, and the same after the workspace was applied.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use doctor_toolkit::container::ustream::StreamLayout;
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::level::edit::ModelChoice;
use doctor_toolkit::level::{Location, TYPE_METAMODEL, TYPE_SIMGROUP};
use doctor_toolkit::resource::hkx::TYPE_HKX;
use doctor_toolkit::resource::hkx::collision::CollisionKind;
use doctor_toolkit::testing as mini;
use doctor_toolkit::zone_ops::{AddResourceParams, CloneParams, ImportModelParams, NewGroupParams};

use super::*;
use crate::app::zone::{Form, ZoneAction, parse_action};
use crate::level_view::LevelAction;
use crate::settings::{Settings, Store};

const ZONE: &str = "gf2/town/doctor";

struct Fixture {
    dir: PathBuf,
    ctx: egui::Context,
    app: BrowserApp,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// An editor without a window on the game in `dir`.
fn open(dir: &Path) -> (egui::Context, BrowserApp) {
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
    app.wizards.preset_used = true;
    app.level_preset_used = true;
    let game = Arc::new(GameInstance::open(dir).unwrap());
    let (index, errors) = game.build_index(|_| {});
    assert!(errors.is_empty(), "{errors:?}");
    app.game = Some(game);
    app.index = Some(Arc::new(index));
    // As when a game is opened: the workspace, then its new streams staged.
    app.open_workspace();
    (ctx, app)
}

fn fixture(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("dt-zone-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    mini::write_game(&dir).unwrap();
    mini::write_town(&dir).unwrap();
    let (ctx, app) = open(&dir);
    Fixture { dir, ctx, app }
}

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

/// Draws frames until level tab `id` has its zones loaded.
#[track_caller]
fn wait(ctx: &egui::Context, app: &mut BrowserApp, id: u64) {
    let started = Instant::now();
    loop {
        frame(ctx, app);
        let level = &app.levels[&id];
        if level.loaded() && !level.busy() {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the zones load: {} (loaded {}, busy {}; {})",
            level.status_text(),
            level.loaded(),
            level.busy(),
            app.status
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    frame(ctx, app);
}

/// Opens a level tab with `zones` loaded; returns its id.
fn level_tab(ctx: &egui::Context, app: &mut BrowserApp, zones: &[&str]) -> u64 {
    let before: Vec<u64> = app.levels.keys().copied().collect();
    app.open_zones_in_level(zones.iter().map(|z| (*z).to_owned()).collect());
    frame(ctx, app);
    frame(ctx, app);
    let id = *app
        .levels
        .keys()
        .find(|id| !before.contains(id))
        .expect("a level tab");
    wait(ctx, app, id);
    id
}

fn stream_of(app: &BrowserApp, name: &str) -> usize {
    app.game.as_ref().unwrap().find_stream(name).unwrap()
}

/// The SimGroup of the level of tab `id` that is in stream `name`, with
/// the positions of its objects.
fn group_of(app: &BrowserApp, id: u64, name: &str) -> (Location, Vec<[f32; 3]>) {
    let stream = stream_of(app, name);
    let level = app.levels[&id].level().expect("loaded");
    let group = level
        .groups
        .iter()
        .find(|g| g.location.stream == stream)
        .unwrap_or_else(|| panic!("no SimGroup of {name} in the level"));
    let positions = group
        .objects
        .iter()
        .map(|o| {
            o.transform
                .map_or([f32::NAN; 3], |m| [m[3][0], m[3][1], m[3][2]])
        })
        .collect();
    (group.location, positions)
}

/// The resources the workspace added to the new zone, by type.
fn added(app: &BrowserApp, type_id: u32) -> Vec<u32> {
    app.workspace
        .as_ref()
        .unwrap()
        .new_stream_resources(ZONE)
        .iter()
        .filter(|c| c.key.type_id == type_id)
        .map(|c| c.key.ids[3])
        .collect()
}

/// Drives the wizard: the zone with a SimGroup and a cell, one operation.
fn make_zone(app: &mut BrowserApp) {
    let mut w = app.new_wizard(Kind::NewZone, &Start::default());
    assert!(w.state.task_ref().check(0, app).is_err(), "no name yet");
    let State::Zone(z) = &mut w.state else {
        panic!("the zone wizard");
    };
    z.preset("name", ZONE, app);
    z.preset("parent", mini::TOWN, app);
    assert_eq!(z.cell.level, mini::TOWN, "the level follows the parent");
    z.preset("min", "30/0/30", app);
    z.preset("max", "50/10/50", app);
    app.wizard_next(&mut w);
    app.wizard_next(&mut w);
    assert_eq!(w.error, None);
    assert_eq!(w.step, 2, "the summary");
    assert_eq!(w.state.task_ref().summary(app).len(), 3);
    let ops = app.workspace.as_ref().map_or(0, |ws| ws.head());
    app.wizard_record(&mut w);
    assert_eq!(w.error, None);
    assert!(w.done.is_some());
    let State::Zone(z) = &w.state else {
        unreachable!()
    };
    assert_eq!(z.made(), Some(ZONE));
    let ws = app.workspace.as_ref().unwrap();
    assert_eq!(ws.head(), ops + 1, "the wizard is one operation");
    assert!(ws.new_stream(ZONE).is_some());
}

#[test]
fn a_zone_is_made_filled_and_shown_before_anything_is_written() {
    let mut f = fixture("built");
    let (ctx, dir) = (f.ctx.clone(), f.dir.clone());
    let app = &mut f.app;
    let disk_streams = app.game.as_ref().unwrap().streams().len();
    make_zone(app);

    // The zone is a stream of the game the editor shows, after the game's own.
    let game = app.game.clone().unwrap();
    let zone = game.find_stream(ZONE).expect("staged");
    assert_eq!(zone, disk_streams);
    assert!(game.is_staged(zone));
    assert_eq!(app.disk_game().unwrap().streams().len(), disk_streams);
    assert_eq!(added(app, TYPE_SIMGROUP).len(), 1);
    app.zone.cell.level = mini::TOWN.into();
    app.list_zone_cells();
    let cells = app.listed_cells().expect("listed");
    assert_eq!(cells.len(), 2);
    assert!(cells.iter().any(|c| c.stream == ZONE && c.new));

    // A model file imported into the new zone as a new set with collision
    // (what the Import model window does with "Into stream" and "Collision").
    let file = dir.join("box.glb");
    std::fs::write(&file, mini::box_glb([2.0, 2.0, 2.0], mini::MATERIAL_NAME)).unwrap();
    app.zone_action(ZoneAction::ImportModel(ImportModelParams {
        file,
        template: format!("{}#{}", mini::YARD, mini::YARD_MODEL),
        name: Some("doctor_box".into()),
        stream: Some(ZONE.into()),
        collision: Some("box".into()),
        ..Default::default()
    }))
    .unwrap();
    let imported = added(app, TYPE_METAMODEL);
    assert_eq!(imported.len(), 1, "the new set is in the zone");
    assert_eq!(added(app, TYPE_HKX).len(), 1, "with collision of its own");

    // The level view loads the new zone with its neighbour.
    let id = level_tab(&ctx, app, &[mini::YARD, ZONE]);
    let (yard_group, crates) = group_of(app, id, mini::YARD);
    assert_eq!(crates, [mini::CRATE_POSITION]);
    let (zone_group, none) = group_of(app, id, ZONE);
    assert!(none.is_empty());
    // The list of zones has it.
    assert!(app.levels[&id].zone_names().iter().any(|z| z == ZONE));

    // Details → Add object → Add to: the crate of the yard as the template,
    // its model cloned with collision of its own into the new zone.
    let template = app.levels[&id].find(yard_group, 0).unwrap();
    app.levels
        .get_mut(&id)
        .unwrap()
        .set_selection(Some(template), [template].into());
    let yard = stream_of(app, mini::YARD);
    let crate_model = Location {
        stream: yard,
        resource: mini::YARD_MODEL,
    };
    app.level_action(
        id,
        LevelAction::Add {
            group: zone_group,
            template: (yard_group, 0),
            model: ModelChoice::CloneWithCollision(crate_model, CollisionKind::Mesh, None),
            position: [40.0, 0.0, 40.0],
        },
    );
    // The clone changed the layout of the staged stream: the level loads
    // again and the new object is selected.
    wait(&ctx, app, id);
    let (zone_group, placed) = group_of(app, id, ZONE);
    assert_eq!(placed, [[40.0, 0.0, 40.0]]);
    assert_eq!(
        app.levels[&id].primary(),
        app.levels[&id].find(zone_group, 0)
    );
    assert_eq!(added(app, TYPE_METAMODEL).len(), 2);
    assert_eq!(added(app, TYPE_HKX).len(), 2);
    assert_eq!(
        app.levels[&id].level().unwrap().coverage().meshes,
        2,
        "the crate of the yard and its clone are drawn"
    );

    // An object showing the imported model: nothing is cloned, the level
    // stays loaded.
    let (yard_group, _) = group_of(app, id, mini::YARD);
    app.level_action(
        id,
        LevelAction::Add {
            group: zone_group,
            template: (yard_group, 0),
            model: ModelChoice::Existing(imported[0]),
            position: [44.0, 0.0, 40.0],
        },
    );
    assert!(app.levels[&id].loaded());
    frame(&ctx, app);
    let (_, placed) = group_of(app, id, ZONE);
    assert_eq!(placed, [[40.0, 0.0, 40.0], [44.0, 0.0, 40.0]]);
    // Moved like any object.
    let mut m = doctor_toolkit::level::IDENTITY;
    m[3][..3].copy_from_slice(&[45.0, 0.0, 41.0]);
    app.level_action(
        id,
        LevelAction::SetTransform {
            group: zone_group,
            object: 1,
            matrix: m,
        },
    );
    frame(&ctx, app);
    assert_eq!(group_of(app, id, ZONE).1[1], [45.0, 0.0, 41.0]);

    // An empty object of a class, and one of a class the schema lacks.
    app.level_action(
        id,
        LevelAction::AddEmpty {
            group: zone_group,
            class: "EnableDisableStreams".into(),
        },
    );
    frame(&ctx, app);
    assert_eq!(group_of(app, id, ZONE).1.len(), 3, "{}", app.status);
    let ops = app.workspace.as_ref().unwrap().head();
    app.level_action(
        id,
        LevelAction::AddEmpty {
            group: zone_group,
            class: "NoSuchClassOfTheGame".into(),
        },
    );
    assert_eq!(app.workspace.as_ref().unwrap().head(), ops);

    // Resources cloned into the zone from the yard, cloned inside the zone
    // (no place is asked for in a new stream), and a raw resource.
    let before = app
        .workspace
        .as_ref()
        .unwrap()
        .new_stream_resources(ZONE)
        .len();
    app.zone_action(ZoneAction::Clone(CloneParams {
        stream: mini::YARD.into(),
        indices: vec![mini::YARD_MESH],
        to: Some(ZONE.into()),
        ..Default::default()
    }))
    .unwrap();
    app.zone_action(ZoneAction::Clone(CloneParams {
        stream: ZONE.into(),
        indices: vec![0],
        ..Default::default()
    }))
    .unwrap();
    let raw = dir.join("raw.bin");
    std::fs::write(&raw, b"raw data").unwrap();
    app.zone_action(ZoneAction::AddResource(AddResourceParams {
        stream: ZONE.into(),
        type_tag: "lh2".into(),
        file: raw,
        ..Default::default()
    }))
    .unwrap();
    let after = app
        .workspace
        .as_ref()
        .unwrap()
        .new_stream_resources(ZONE)
        .len();
    assert!(after >= before + 3, "{before} -> {after}");
    wait(&ctx, app, id);
    assert_eq!(group_of(app, id, ZONE).1.len(), 3);
    // A second SimGroup with copies of the objects of the first.
    let (zone_group, _) = group_of(app, id, ZONE);
    app.zone_action(ZoneAction::AddGroup(NewGroupParams {
        stream: ZONE.into(),
        copy_of: Some(format!("{ZONE}#{}", zone_group.resource)),
        objects: Some(vec![0]),
    }))
    .unwrap();
    assert_eq!(added(app, TYPE_SIMGROUP).len(), 2);

    // Every form of the window and the wizard draw.
    for form in Form::ALL {
        app.open_zone_form(form);
        frame(&ctx, app);
    }

    // Undo takes the second SimGroup back, redo returns it; the views follow.
    app.undo();
    assert_eq!(added(app, TYPE_SIMGROUP).len(), 1);
    app.redo();
    assert_eq!(added(app, TYPE_SIMGROUP).len(), 2);
    wait(&ctx, app, id);

    // Nothing of all this is in the game folder.
    assert!(!dir.join(format!("{ZONE}.str")).exists());
    let on_disk = GameInstance::open(&dir).unwrap();
    assert_eq!(on_disk.streams().len(), disk_streams);

    // Applied: the file holds what the views showed, SimGroups last.
    app.apply(false);
    let written = std::fs::read(dir.join(format!("{ZONE}.str"))).expect(&app.status);
    let layout = StreamLayout::parse(&written).unwrap();
    assert_eq!(layout.resources.len(), after + 1);
    let types: Vec<u32> = layout.resources.iter().map(|r| r.header.type_id).collect();
    assert_eq!(types[types.len() - 2..], [TYPE_SIMGROUP, TYPE_SIMGROUP]);
    wait(&ctx, app, id);
    assert_eq!(group_of(app, id, ZONE).1.len(), 3);
    // The tab of the wizard draws at every step.
    app.start_wizard(Kind::NewZone, Start::default());
    frame(&ctx, app);
}

/// 103-clone-fix in the editor: "Add object" with "Clone it" into a stream
/// the workspace created, after the workspace was applied and the editor
/// started again (the stream is in the game folder and new in the workspace).
#[test]
fn a_model_is_cloned_into_an_applied_new_zone() {
    let mut f = fixture("applied");
    let dir = f.dir.clone();
    make_zone(&mut f.app);
    f.app.apply(false);
    assert!(dir.join(format!("{ZONE}.str")).exists(), "{}", f.app.status);
    drop(std::mem::replace(&mut f.app, open(&dir).1));

    let (ctx, mut app) = open(&dir);
    let game = app.game.clone().unwrap();
    let zone = game.find_stream(ZONE).unwrap();
    assert!(game.is_staged(zone), "the workspace's version is shown");
    assert_eq!(
        game.streams().len(),
        GameInstance::open(&dir).unwrap().streams().len(),
        "in the place of the file of the game folder"
    );
    let id = level_tab(&ctx, &mut app, &[mini::YARD, ZONE]);
    let (yard_group, _) = group_of(&app, id, mini::YARD);
    let (zone_group, _) = group_of(&app, id, ZONE);
    let crate_model = Location {
        stream: stream_of(&app, mini::YARD),
        resource: mini::YARD_MODEL,
    };
    app.level_action(
        id,
        LevelAction::Add {
            group: zone_group,
            template: (yard_group, 0),
            model: ModelChoice::Clone(crate_model),
            position: [40.0, 0.0, 40.0],
        },
    );
    wait(&ctx, &mut app, id);
    assert_eq!(
        group_of(&app, id, ZONE).1,
        [[40.0, 0.0, 40.0]],
        "{}",
        app.status
    );
    assert_eq!(added(&app, TYPE_METAMODEL).len(), 1);
    // Written again with the clone.
    app.apply(false);
    let written = std::fs::read(dir.join(format!("{ZONE}.str"))).unwrap();
    let layout = StreamLayout::parse(&written).unwrap();
    assert!(
        layout
            .resources
            .iter()
            .any(|r| r.header.type_id == TYPE_METAMODEL),
        "{}",
        app.status
    );
    drop(app);
}

#[test]
fn a_removed_stream_leaves_the_views() {
    let mut f = fixture("removed");
    let ctx = f.ctx.clone();
    let app = &mut f.app;
    make_zone(app);
    let id = level_tab(&ctx, app, &[mini::YARD, ZONE]);
    assert_eq!(app.levels[&id].level().unwrap().groups.len(), 2);
    // The cell first (its form lists and removes), then the stream.
    app.zone_action(ZoneAction::RemoveCell {
        level: mini::TOWN.into(),
        stream: ZONE.into(),
    })
    .unwrap();
    app.zone_action(ZoneAction::RemoveStream(ZONE.into()))
        .unwrap();
    assert!(app.game.as_ref().unwrap().find_stream(ZONE).is_none());
    assert!(!app.game.as_ref().unwrap().has_staged());
    wait(&ctx, app, id);
    assert_eq!(app.levels[&id].level().unwrap().groups.len(), 1);
    // Not a stream of the workspace any more.
    assert!(
        app.zone_action(ZoneAction::RemoveStream(ZONE.into()))
            .is_err()
    );
    // Undo brings the zone back into the list of streams.
    app.undo();
    assert!(app.game.as_ref().unwrap().find_stream(ZONE).is_some());
}

#[test]
fn zone_presets_run_actions_and_stop_at_the_first_error() {
    let mut f = fixture("preset");
    let dir = f.dir.clone();
    let app = &mut f.app;
    let file = dir.join("box.glb");
    std::fs::write(&file, mini::box_glb([2.0, 2.0, 2.0], mini::MATERIAL_NAME)).unwrap();
    let script = format!(
        "stream:name={ZONE},parent={town}; simgroup:stream={ZONE}; \
         cell:level={town},stream={ZONE},min=30/0/30,max=50/10/50; \
         clone:stream={yard},index={model}+{mesh},to={ZONE}; \
         import:file={file},template={yard}#{model},name=doctor_box,stream={ZONE},collision=box; \
         form=cell",
        town = mini::TOWN,
        yard = mini::YARD,
        model = mini::YARD_MODEL,
        mesh = mini::YARD_MESH,
        file = file.display(),
    );
    app.run_zone_script(&script).unwrap();
    let ws = app.workspace.as_ref().unwrap();
    assert_eq!(ws.head(), 5, "one operation per action");
    assert_eq!(added(app, TYPE_METAMODEL).len(), 2);
    assert_eq!(added(app, TYPE_HKX).len(), 1);
    assert!(app.game.as_ref().unwrap().find_stream(ZONE).is_some());
    assert!(app.zone.open, "form= opens the window");
    assert_eq!(app.zone.cell.min, "30, 0, 30", "the form shows the action");
    // An action that fails stops the script and says which one.
    let error = app
        .run_zone_script(&format!(
            "simgroup:stream=gf2/nowhere; remove-stream:name={ZONE}"
        ))
        .unwrap_err();
    assert!(error.contains("gf2/nowhere"), "{error}");
    assert!(app.workspace.as_ref().unwrap().new_stream(ZONE).is_some());
    for bad in [
        "teleport:x=1",
        "stream:name=a",
        "cell:level=a,stream=b,min=1/2,max=3/4/5",
        "clone:stream=a,index=x",
        "stream:name=a,parent=b,colour=red",
    ] {
        assert!(parse_action(bad).is_err(), "{bad}");
    }
    assert_eq!(
        parse_action("remove-cell:level=gf2/town,stream=gf2/town/doctor").unwrap(),
        ZoneAction::RemoveCell {
            level: "gf2/town".into(),
            stream: ZONE.into()
        }
    );
}
