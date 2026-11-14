//! The Content Browser without a window, on the miniature game of
//! `doctor_toolkit::testing`: folders and filters, search, workspace marks,
//! previews and their cache, a model dropped into a level view.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::hash::sdbm;
use doctor_toolkit::navigation::GameIndex;
use doctor_toolkit::testing as mini;

use super::catalog::{Category, Mark, Place};
use super::preview::{self, DiskCache, Kind, PreviewId, PreviewJob, PreviewSource, Shown};
use super::*;
use crate::settings::{Settings, Store};

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

/// An editor without a window on a fresh copy of the miniature game, with
/// its game index built and the Content Drawer open.
fn fixture(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("dt-content-{name}-{}", std::process::id()));
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
    let game = Arc::new(GameInstance::open(&dir).unwrap());
    let (index, errors) = game.build_index(|_| {});
    assert!(errors.is_empty(), "{errors:?}");
    app.nav.index = Some(Arc::new(GameIndex::build(&game, &|_, _| {})));
    app.game = Some(game);
    app.index = Some(Arc::new(index));
    app.drawers.add(Pane::ContentBrowser);
    app.drawers.open = Some(Pane::ContentBrowser);
    Fixture { dir, ctx, app }
}

fn frame(f: &mut Fixture) {
    use eframe::App as _;
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 800.0),
        )),
        ..Default::default()
    };
    let mut eframe_frame = eframe::Frame::_new_kittest();
    let app = &mut f.app;
    let mut output = f.ctx.run_ui(input, |ui| app.ui(ui, &mut eframe_frame));
    output.textures_delta.clear();
}

/// Frames until `done` holds (at most ten seconds).
fn frames_until(f: &mut Fixture, done: impl Fn(&BrowserApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(&f.app) {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(10));
        frame(f);
    }
}

fn browser(app: &mut BrowserApp) -> &mut ContentState {
    app.content
        .browsers
        .get_mut(&0)
        .expect("the drawer was drawn")
}

fn types(app: &mut BrowserApp) -> Vec<String> {
    let mut v: Vec<String> = browser(app)
        .listing
        .assets
        .iter()
        .map(|a| a.type_name())
        .collect();
    v.sort();
    v
}

#[test]
fn folders_streams_filters_and_search() {
    let mut f = fixture("browse");
    frame(&mut f);
    let app = &mut f.app;
    // Content → gf2: the folder text and the stream level.
    let tree = app.content.tree.clone();
    let gf2 = tree.folder("gf2").unwrap();
    assert_eq!(gf2.total, 2);
    assert_eq!(gf2.streams[0].path, "gf2/level");
    assert_eq!(tree.folders[gf2.children[0]].path, "gf2/text");
    let state = browser(app);
    assert_eq!(state.listing.folders.len(), 1, "gf2 at the root");

    state.go(Place::Folder("gf2".into()));
    frame(&mut f);
    let state = browser(&mut f.app);
    assert_eq!(state.listing.folders.len(), 1);
    assert_eq!(state.listing.streams.len(), 1);
    assert!(state.listing.assets.is_empty(), "a folder without a filter");

    // The stream: its assets, companions left out, names from the index.
    browser(&mut f.app).go(Place::Stream(mini::LEVEL.into()));
    frame(&mut f);
    assert_eq!(
        types(&mut f.app),
        [
            "lh2", "material", "mesh", "partsdb", "rcb", "simgroup", "tg4h"
        ]
    );
    let state = browser(&mut f.app);
    assert!(state.listing.assets.iter().any(|a| a.name == "wall"));

    // A filter; with it a folder lists the assets of its streams.
    state.filter.categories = vec![Category::Texture, Category::Model];
    frame(&mut f);
    assert_eq!(types(&mut f.app), ["mesh", "tg4h"]);
    let state = browser(&mut f.app);
    state.filter.categories = vec![Category::Text];
    state.go(Place::Folder("gf2".into()));
    frame(&mut f);
    assert_eq!(types(&mut f.app), ["lh2", "lh2"], "level and text");
    let state = browser(&mut f.app);
    state.go(Place::Folder("gf2/text".into()));
    frame(&mut f);
    assert_eq!(types(&mut f.app), ["lh2"]);

    // The search of the game index, under the folder shown.
    let state = browser(&mut f.app);
    state.filter.categories.clear();
    state.go(Place::Folder(String::new()));
    state.search = "wall".into();
    state.edited = Some(Instant::now() - SEARCH_DELAY);
    frame(&mut f);
    frames_until(&mut f, |app| {
        app.content.browsers[&0].job.is_none() && app.content.browsers[&0].results.is_some()
    });
    frame(&mut f);
    let names: Vec<(String, String)> = browser(&mut f.app)
        .listing
        .assets
        .iter()
        .map(|a| (a.name.clone(), a.type_name()))
        .collect();
    assert!(names.contains(&("wall".into(), "tg4h".into())), "{names:?}");
    assert!(names.iter().any(|(_, t)| t == "material"), "{names:?}");
    assert!(browser(&mut f.app).listing.folders.is_empty());
    // ... with a type filter, and in another folder.
    let state = browser(&mut f.app);
    state.filter.categories = vec![Category::Texture];
    state.edited = Some(Instant::now() - SEARCH_DELAY);
    frame(&mut f);
    frames_until(&mut f, |app| app.content.browsers[&0].job.is_none());
    frame(&mut f);
    assert_eq!(types(&mut f.app), ["tg4h"]);
    let state = browser(&mut f.app);
    // Another place: the search runs again by itself.
    state.go(Place::Folder("gf2/text".into()));
    frame(&mut f);
    frames_until(&mut f, |app| app.content.browsers[&0].job.is_none());
    frame(&mut f);
    assert!(browser(&mut f.app).listing.assets.is_empty());
    // A query that does not parse says why.
    let state = browser(&mut f.app);
    state.search = "type:nosuchtype".into();
    state.edited = Some(Instant::now() - SEARCH_DELAY);
    frame(&mut f);
    assert!(matches!(
        &browser(&mut f.app).results,
        Some((_, Err(e))) if e.contains("nosuchtype")
    ));
}

#[test]
fn clones_are_new_in_the_workspace_and_undone() {
    let mut f = fixture("clone");
    frame(&mut f);
    browser(&mut f.app).go(Place::Stream(mini::LEVEL.into()));
    browser(&mut f.app).filter.categories = vec![Category::Texture];
    frame(&mut f);
    let wall = browser(&mut f.app).listing.assets[0].clone();
    assert_eq!(wall.mark, Mark::None);
    f.app.content_clone(&wall);
    frame(&mut f);
    let mut marks: Vec<Mark> = browser(&mut f.app)
        .listing
        .assets
        .iter()
        .map(|a| a.mark)
        .collect();
    marks.sort_by_key(|m| *m == Mark::New);
    assert_eq!(marks, [Mark::None, Mark::New], "the clone, with its data");
    let ws = f.app.workspace.as_ref().unwrap();
    assert_eq!(ws.operations().len(), 1);
    assert!(
        ws.changes()
            .any(|c| c.key.type_id == sdbm(b"tg4d") && c.is_added())
    );
    f.app.undo();
    frame(&mut f);
    assert_eq!(browser(&mut f.app).listing.assets.len(), 1);
}

fn texture_job(app: &BrowserApp, version: Option<String>) -> PreviewJob {
    let game = app.game.as_ref().unwrap();
    let stream = game.find_stream(mini::LEVEL).unwrap();
    let file = game.open_stream(stream).unwrap();
    let header = file.layout().resources[mini::TEX_HEADER].header;
    PreviewJob {
        id: PreviewId {
            key: doctor_toolkit::workspace::ResourceKey::new(mini::LEVEL, &header),
            version,
        },
        at: Some(Location {
            stream,
            resource: mini::TEX_HEADER,
        }),
        kind: Kind::Texture,
    }
}

#[test]
fn texture_previews_are_cached_and_follow_the_workspace() {
    let mut f = fixture("preview");
    let root = f.dir.join("cache");
    let app = &mut f.app;
    let game = app.game.clone().unwrap();
    let index = app.index.clone().unwrap();
    let cache = DiskCache::new(&root, preview::RENDER_VERSION);
    let gpu = std::sync::Mutex::default();
    let source = PreviewSource::new(game.clone(), index.clone(), None, Some(cache.clone()));
    let job = texture_job(app, None);
    // A miss makes the image, a hit reads it back.
    let (first, from_disk) = preview::make(&source, &job, &gpu).unwrap();
    assert!(!from_disk);
    assert_eq!((first.width, first.height), (4, 4));
    assert!(first.rgba.chunks(4).all(|p| p == [0, 0, 0, 255]), "black");
    let (again, from_disk) = preview::make(&source, &job, &gpu).unwrap();
    assert!(from_disk);
    assert_eq!(again, first);

    // The workspace changes the texture's data: another version, a miss.
    let stream = game.find_stream(mini::LEVEL).unwrap();
    let ws = app.workspace_mut().unwrap();
    ws.set_resource(
        &game,
        stream,
        mini::TEX_DATA,
        vec![0xFF, 0xFF, 0, 0, 0, 0, 0, 0],
        "white",
    )
    .unwrap();
    let marks = catalog::Marks::of(app.workspace.as_ref());
    let source = PreviewSource::new(game, index, app.workspace.as_ref(), Some(cache));
    let header = job.id.key.clone();
    let (mark, version) = marks
        .by_key
        .iter()
        .find(|(k, _)| k.type_id == sdbm(b"tg4d"))
        .map(|(_, (m, sha))| (*m, sha.clone()))
        .unwrap();
    assert_eq!(mark, Mark::Changed);
    let job = PreviewJob {
        id: PreviewId {
            key: header,
            version: Some(version),
        },
        ..job
    };
    let (edited, from_disk) = preview::make(&source, &job, &gpu).unwrap();
    assert!(!from_disk);
    assert!(
        edited.rgba.chunks(4).all(|p| p == [255, 255, 255, 255]),
        "white"
    );
    assert!(preview::make(&source, &job, &gpu).unwrap().1);

    // A model without a GPU: the preview says why, nothing hangs.
    let mesh = game_mesh_job(app);
    let error = preview::make(&source, &mesh, &gpu).unwrap_err();
    assert!(error.contains("GPU"), "{error}");
}

fn game_mesh_job(app: &BrowserApp) -> PreviewJob {
    let game = app.game.as_ref().unwrap();
    let stream = game.find_stream(mini::LEVEL).unwrap();
    let file = game.open_stream(stream).unwrap();
    let header = file.layout().resources[mini::MESH].header;
    PreviewJob {
        id: PreviewId {
            key: doctor_toolkit::workspace::ResourceKey::new(mini::LEVEL, &header),
            version: None,
        },
        at: Some(Location {
            stream,
            resource: mini::MESH,
        }),
        kind: Kind::Model,
    }
}

#[test]
fn the_browser_shows_previews_made_on_worker_threads() {
    let mut f = fixture("tiles");
    f.app
        .content
        .previews
        .set_cache_root(Some(f.dir.join("cache")));
    frame(&mut f);
    let state = browser(&mut f.app);
    state.go(Place::Stream(mini::LEVEL.into()));
    state.filter.categories = vec![Category::Texture, Category::Model];
    frame(&mut f);
    // The tiles ask; the workers answer; the texture becomes a preview,
    // the mesh fails without a GPU.
    let texture = texture_job(&f.app, None);
    let mesh = game_mesh_job(&f.app);
    frames_until(&mut f, |app| !app.content.previews.busy());
    frame(&mut f);
    let previews = &mut f.app.content.previews;
    assert!(matches!(previews.get(&texture), Shown::Ready(_)));
    assert!(matches!(previews.get(&mesh), Shown::Failed(_)));
    assert_eq!(previews.stats.made, 1);
    assert!(previews.memory() > 0);
    // After the game files change, they are made again (here: from the disk).
    f.app.content.files_changed();
    frame(&mut f);
    assert_eq!(browser(&mut f.app).listing.len(), 2, "the place stays");
    frames_until(&mut f, |app| !app.content.previews.busy());
    frame(&mut f);
    assert_eq!(f.app.content.previews.stats.from_disk, 1);
    assert!(matches!(
        f.app.content.previews.get(&texture),
        Shown::Ready(_)
    ));
}

#[test]
fn a_model_dropped_into_the_level_stands_on_the_ground_and_is_undone() {
    use crate::level_view::drop::ground_point;
    use crate::level_view::walk::physics::{Owner, Placed, World};
    use rapier3d::prelude::{Pose, SharedShape, Vector};

    let mut f = fixture("drop");
    // A zone whose one object stands at the origin with a model.
    let zone = {
        use doctor_toolkit::container::ustream::{
            FileHeader, ResourceHeader, Storage, StreamWriter,
        };
        use doctor_toolkit::resource::simgroup::{
            CLASS_ENTITY, CLASS_SYSTEM_COMMANDS, Guid, SimGroup,
        };
        let mut group = SimGroup::empty();
        let object = group
            .push_object(
                CLASS_ENTITY,
                Guid(*b"CRATE-0000000001"),
                &[(CLASS_SYSTEM_COMMANDS, 2), (CLASS_ENTITY, 2)],
            )
            .unwrap();
        group
            .set_transform(object, doctor_toolkit::level::IDENTITY)
            .unwrap();
        group.set_model(object, Some(0x7777)).unwrap();
        let mut writer = StreamWriter::new(&FileHeader::GF2_PC, doctor_toolkit::io::Endian::Little);
        let header = ResourceHeader {
            mem_policy: sdbm(b"default"),
            alignment: 0x10,
            type_id: sdbm(b"simgroup"),
            ids: [0x20; 4],
            data_size: 0,
        };
        writer
            .push_resource(&header, &group.encode(), Storage::RefPack)
            .unwrap();
        writer.finish()
    };
    std::fs::write(f.dir.join("gf2/zone.str"), zone).unwrap();
    let app = &mut f.app;
    let game = Arc::new(GameInstance::open(&f.dir).unwrap());
    let index = Arc::new(game.build_index(|_| {}).0);
    app.game = Some(game.clone());
    app.index = Some(index.clone());
    let stream = game.find_stream("gf2/zone").unwrap();
    // A metamodel the level finds (the workspace's, say), and a floor of
    // collision with its top at y = 2.
    let model = 0x4d4d_4d4d;
    let mut overrides = doctor_toolkit::level::Overrides::default();
    overrides.by_id.insert(
        (doctor_toolkit::level::TYPE_METAMODEL, model),
        (stream, Arc::new(Vec::new())),
    );
    let mut view = crate::level_view::LevelView::with_level_for_test(
        game.clone(),
        index,
        &[stream],
        overrides,
        &f.ctx,
    );
    let world = World::new(&[Placed {
        owner: Owner {
            layer: 0,
            object: (0, 0),
            hkx: 1,
        },
        parts: Arc::new(vec![(Pose::IDENTITY, SharedShape::cuboid(50.0, 1.0, 50.0))]),
        pose: Pose::from_translation(Vector::new(0.0, 1.0, 0.0)),
    }]);
    let (origin, dir) = ([3.0, 20.0, 4.0], [0.0, -1.0, 0.0]);
    let at = ground_point(&world, origin, dir).unwrap();
    assert_eq!(at, [3.0, 2.0, 4.0]);

    let asset = DraggedAsset {
        type_id: sdbm(b"metamodel"),
        id: model,
        stream,
        resource: usize::MAX,
    };
    let (action, status) = view.drop_for_test(&asset, Some(at), origin, dir);
    let Some(action) = action else {
        panic!("no action: {status}");
    };
    let crate::level_view::LevelAction::Add {
        group,
        template,
        model: choice,
        position,
    } = &action
    else {
        panic!("not an Add");
    };
    assert_eq!(group.stream, stream);
    assert_eq!(*template, (*group, 0), "the object of the zone");
    assert_eq!(
        *choice,
        doctor_toolkit::level::edit::ModelChoice::Existing(model)
    );
    assert_eq!(*position, at);
    let group = *group;

    // Recorded as "Add object" records it: one operation, undone by Undo.
    let tab = app.new_pane_id();
    app.levels.insert(tab, view);
    app.level_action(tab, action);
    let ws = app.workspace.as_ref().expect("made by the edit");
    assert_eq!(ws.operations().len(), 1, "{}", app.status);
    let g = doctor_toolkit::level::edit::current_group(&game, ws, group).unwrap();
    let added = g.objects.last().unwrap();
    assert_eq!(g.objects.len(), 2);
    assert_eq!(g.model(added), Some(model));
    let m = g.transform(added).unwrap();
    assert_eq!([m[3][0], m[3][1], m[3][2]], at, "on the ground");
    // The dropped object is selected (stage 54: the gizmo moves it next).
    let level = &app.levels[&tab];
    let dropped = level.find(group, 1).expect("in the level");
    assert_eq!(level.primary(), Some(dropped));
    assert_eq!(level.selection().len(), 1);
    app.undo();
    let ws = app.workspace.as_ref().unwrap();
    let g = doctor_toolkit::level::edit::current_group(&game, ws, group).unwrap();
    assert_eq!(g.objects.len(), 1, "undone");

    // A model the game does not have is not placed.
    let missing = DraggedAsset {
        id: 0x1234_5678,
        ..asset
    };
    let view = app.levels.get_mut(&tab).unwrap();
    let (action, status) = view.drop_for_test(&missing, Some(at), origin, dir);
    assert!(action.is_none());
    assert!(status.contains("12345678"), "{status}");
}

/// The previews of every model and texture of a large stream of the game,
/// made one after another and then read back from the cache: the time of
/// each and the size of the cache. `EARS_GF2_DIR` is the game;
/// `PREVIEW_BENCH_STREAM` the stream (the characters by default).
#[test]
#[ignore = "needs the game (EARS_GF2_DIR) and a GPU"]
fn previews_of_a_large_stream() {
    let Some(dir) = std::env::var_os("EARS_GF2_DIR") else {
        eprintln!("EARS_GF2_DIR is not set");
        return;
    };
    let name = std::env::var("PREVIEW_BENCH_STREAM")
        .unwrap_or_else(|_| "gf2/globalfolder/characters/allcharacters".into());
    let game = Arc::new(GameInstance::open(&dir).unwrap());
    let index = Arc::new(game.build_index(|_| {}).0);
    let stream = game.find_stream(&name).expect("the stream");
    let file = game.open_stream(stream).unwrap();
    let cache_dir = std::env::temp_dir().join(format!("dt-preview-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache_dir);
    let cache = DiskCache::new(&cache_dir, preview::RENDER_VERSION);
    let source = PreviewSource::new(game.clone(), index, None, Some(cache));
    let gpu = std::sync::Mutex::new(thumb::GpuSlot::new(None, true));
    let jobs: Vec<PreviewJob> = file
        .layout()
        .resources
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let kind = match Category::of(r.header.type_id) {
                Category::Model if !catalog::is_companion(r.header.type_id) => Kind::Model,
                Category::Texture => Kind::Texture,
                _ => return None,
            };
            Some(PreviewJob {
                id: PreviewId {
                    key: doctor_toolkit::workspace::ResourceKey::new(&name, &r.header),
                    version: None,
                },
                at: Some(Location {
                    stream,
                    resource: i,
                }),
                kind,
            })
        })
        .collect();
    drop(file);
    for pass in ["made", "from the cache"] {
        let mut by_kind: std::collections::BTreeMap<String, (usize, Duration, usize)> =
            Default::default();
        let started = Instant::now();
        for job in &jobs {
            let t = Instant::now();
            let result = preview::make(&source, job, &gpu);
            let e = by_kind.entry(format!("{:?}", job.kind)).or_default();
            match result {
                Ok(_) => {
                    e.0 += 1;
                    e.1 += t.elapsed();
                }
                Err(_) => e.2 += 1,
            }
        }
        for (kind, (n, time, failed)) in &by_kind {
            eprintln!(
                "{pass}: {kind}: {n} in {:.1} s, {:.1} ms each, {failed} without a preview",
                time.as_secs_f64(),
                time.as_secs_f64() * 1000.0 / (*n).max(1) as f64
            );
        }
        eprintln!("{pass}: all in {:.1} s", started.elapsed().as_secs_f64());
    }
    let version = cache_dir.join(format!("v{}", preview::RENDER_VERSION));
    let sizes: Vec<u64> = std::fs::read_dir(version)
        .unwrap()
        .flatten()
        .flat_map(|d| std::fs::read_dir(d.path()).unwrap().flatten())
        .map(|f| f.metadata().unwrap().len())
        .collect();
    eprintln!(
        "cache: {} files, {:.1} MiB",
        sizes.len(),
        sizes.iter().sum::<u64>() as f64 / (1 << 20) as f64
    );
    std::fs::remove_dir_all(&cache_dir).unwrap();
}
