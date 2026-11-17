//! Every wizard driven without a window on the miniature game of
//! `doctor_toolkit::testing`: its steps, what it records (one operation)
//! and its tab drawn at every step.

use std::path::PathBuf;
use std::sync::Arc;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit::resource::partsdb::PartsDb;
use doctor_toolkit::testing as mini;
use doctor_toolkit::workspace::ResourceKey;

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

/// An editor without a window on a fresh copy of the miniature game.
fn fixture(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("dt-wizard-{name}-{}", std::process::id()));
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
    // No preset of the environment.
    app.wizards.preset_used = true;
    let game = Arc::new(GameInstance::open(&dir).unwrap());
    let (index, errors) = game.build_index(|_| {});
    assert!(errors.is_empty(), "{errors:?}");
    app.game = Some(game);
    app.index = Some(Arc::new(index));
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

/// Resource `resource` of the level stream of the miniature game.
fn level(app: &BrowserApp, resource: usize) -> Location {
    Location {
        stream: app.game.as_ref().unwrap().find_stream(mini::LEVEL).unwrap(),
        resource,
    }
}

fn start(app: &mut BrowserApp, kind: Kind, loc: Option<Location>) -> Wizard {
    let start = Start { loc, model: None };
    app.new_wizard(kind, &start)
}

/// Next through every step, then record; the wizard must end done.
fn finish(app: &mut BrowserApp, w: &mut Wizard) -> String {
    while w.step < w.summary_step() {
        let step = w.step;
        app.wizard_next(w);
        assert!(w.error.is_none(), "step {step}: {:?}", w.error);
        assert_eq!(w.step, step + 1);
    }
    assert!(!w.state.task_ref().summary(app).is_empty());
    app.wizard_record(w);
    assert!(w.error.is_none(), "{:?}", w.error);
    assert!(w.tx.mark.is_none());
    w.done.clone().expect("done")
}

fn operations(app: &BrowserApp) -> Vec<String> {
    app.workspace
        .as_ref()
        .map(|ws| {
            ws.operations()[..ws.head()]
                .iter()
                .map(|op| op.label.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Opens `w` in a tab and draws it at every step up to `last`.
fn draw_steps(f: &mut Fixture, mut w: Wizard, last: usize) {
    let id = f.app.new_pane_id();
    let kind = w.kind;
    w.step = 0;
    f.app.wizards.tabs.insert(id, w);
    f.app.dock_ops.push(DockOp::Open(Pane::Wizard { id, kind }));
    for step in 0..=last {
        if let Some(w) = f.app.wizards.tabs.get_mut(&id) {
            w.step = step;
        }
        frame(&f.ctx, &mut f.app);
        frame(&f.ctx, &mut f.app);
        assert!(f.app.wizards.tabs.contains_key(&id), "step {step}");
    }
}

#[test]
fn texts_wizard_replaces_a_word_in_one_operation() {
    let mut f = fixture("texts");
    let app = &mut f.app;
    let mut w = start(app, Kind::Texts, Some(level(app, mini::STRINGS)));
    let State::Texts(t) = &mut w.state else {
        panic!()
    };
    assert!(t.tables.len() >= 2, "both tables are offered");
    assert!(t.edited.is_some(), "started from the table");
    t.find = "world".into();
    t.replace = "Mundo".into();
    assert_eq!(t.matches(), 1);
    assert_eq!(t.replace_all(), 1);
    // A CSV edit on top: the second string changes too.
    let csv = "hash,text\r\n00000001,Hola\r\n00000002,Mundo\r\n";
    t.load_csv(csv.as_bytes());
    assert_eq!(t.changes().len(), 2);
    let label = finish(app, &mut w);
    assert!(label.starts_with("Wizard: Edit 2 strings"), "{label}");
    assert_eq!(operations(app), [label]);
    let data = app.current_data(level(app, mini::STRINGS)).unwrap();
    let table = StringTable::parse(&data).unwrap();
    assert_eq!(table.strings[0], ["Hola", "Mundo"]);
    // Nothing changed: Next refuses.
    let mut again = start(app, Kind::Texts, Some(level(app, mini::STRINGS)));
    again.step = 1;
    app.wizard_next(&mut again);
    assert!(again.error.is_some());
    assert_eq!(again.step, 1);
    draw_steps(&mut f, again, 3);
}

#[test]
fn params_wizard_finds_an_attribute_and_sets_it() {
    let mut f = fixture("params");
    let app = &mut f.app;
    let mut w = start(app, Kind::Parameters, Some(level(app, mini::SIMGROUP)));
    let State::Params(p) = &mut w.state else {
        panic!()
    };
    // The curated list is about the real game: none of it is in this one.
    assert!(p.readings.values().all(|r| r.is_err()));
    assert!(!p.add_curated("weapons.pistol.damage"));
    assert_eq!(p.category, None, "a SimGroup opens the finder");
    assert_eq!(p.finder.stream, mini::LEVEL);
    p.finder.class = "11111111".into();
    p.finder.attribute = "2".into();
    p.find(app);
    assert_eq!(p.finder.results.len(), 1, "{:?}", p.finder.error);
    let found = p.finder.results[0].clone();
    let (attr, value) = found.attrs[0].clone();
    p.add_found(app, &found, attr, &value);
    assert_eq!(p.chosen.len(), 1);
    assert_eq!(p.chosen[0].reading.objects[0].value, -7.0);
    // A bad value keeps the step.
    p.chosen[0].mode = params::Mode::All;
    p.chosen[0].value = "1.5".into();
    w.step = 1;
    app.wizard_next(&mut w);
    assert!(w.error.is_some());
    let State::Params(p) = &mut w.state else {
        panic!()
    };
    p.chosen[0].value = "42".into();
    w.step = 0;
    let label = finish(app, &mut w);
    assert!(label.contains("Set 1 game parameter:"), "{label}");
    assert_eq!(operations(app).len(), 1);
    let State::Params(p) = &w.state else { panic!() };
    let reading = p.chosen[0]
        .tunable
        .read(app.game.as_ref().unwrap(), app.workspace.as_ref())
        .unwrap();
    assert_eq!(reading.objects[0].value, 42.0);
    let w = start(app, Kind::Parameters, None);
    draw_steps(&mut f, w, 3);
}

/// A PNG of `n`×`n` red pixels.
fn png(n: usize) -> Vec<u8> {
    let rgba: Vec<u8> = (0..n * n).flat_map(|_| [255, 0, 0, 255]).collect();
    doctor_toolkit::resource::pixels::encode_png(n, n, &rgba).unwrap()
}

#[test]
fn retexture_wizard_lists_textures_and_replaces_them() {
    let mut f = fixture("retexture");
    let image = f.dir.join("new_wall.png");
    std::fs::write(&image, png(8)).unwrap();
    let app = &mut f.app;
    let mut w = start(app, Kind::Retexture, Some(level(app, mini::MESH)));
    let State::Retexture(r) = &mut w.state else {
        panic!()
    };
    assert_eq!(r.slots.len(), 1, "{:?}", r.message);
    assert_eq!(r.slots[0].texture.name, "wall");
    assert!(r.slots[0].chosen, "colour textures are ticked");
    // Without a new image the images step refuses.
    w.step = 2;
    app.wizard_next(&mut w);
    assert!(
        w.error.as_deref().unwrap().contains("wall"),
        "{:?}",
        w.error
    );
    let State::Retexture(r) = &mut w.state else {
        panic!()
    };
    let exported = f.dir.join("exported");
    assert_eq!(r.export(app, &exported).unwrap(), 1);
    assert!(exported.join("wall.png").is_file());
    r.set_image(app, 0, &image);
    assert!(matches!(&r.slots[0].new, Some(Ok(n)) if n.size == (8, 8)));
    w.step = 0;
    let label = finish(app, &mut w);
    assert!(label.contains("Retexture 1 texture: wall"), "{label}");
    assert_eq!(operations(app), [label]);
    let ws = app.workspace.as_ref().unwrap();
    assert_eq!(ws.len(), 2, "header and data of the texture");

    // A character set lists the textures of its parts.
    let w = start(app, Kind::Retexture, Some(level(app, mini::PARTSDB)));
    let State::Retexture(r) = &w.state else {
        panic!()
    };
    assert_eq!(r.sets, ["db"]);
    assert_eq!(r.slots.len(), 1, "{:?}", r.message);
    draw_steps(&mut f, w, 4);
}

#[test]
fn model_wizard_replaces_a_mesh_with_a_new_file() {
    let mut f = fixture("model");
    let file = f.dir.join("new.glb");
    std::fs::write(&file, mini::triangle_glb(3.0, mini::MATERIAL_NAME, false)).unwrap();
    let app = &mut f.app;
    let mut w = start(app, Kind::ReplaceModel, Some(level(app, mini::MESH)));
    let State::Model(m) = &mut w.state else {
        panic!()
    };
    assert_eq!(m.target, Some(level(app, mini::MESH)));
    let reference = m
        .export_reference(app, f.dir.join("reference.glb"))
        .unwrap();
    assert!(std::fs::read(&reference).unwrap().starts_with(b"glTF"));
    // The new file is required.
    w.step = 2;
    app.wizard_next(&mut w);
    assert!(w.error.is_some());
    let State::Model(m) = &mut w.state else {
        panic!()
    };
    m.set_file(app, file);
    assert!(
        matches!(m.plan, Some(Ok(_))),
        "{:?}",
        m.plan.as_ref().map(|p| p.as_ref().err())
    );
    w.step = 0;
    let label = finish(app, &mut w);
    assert!(label.starts_with("Wizard: "), "{label}");
    assert_eq!(operations(app), [label]);
    let w = start(app, Kind::ReplaceModel, Some(level(app, mini::MESH)));
    draw_steps(&mut f, w, 5);
}

#[test]
fn outfit_wizard_adds_a_skinned_part_to_a_set() {
    let mut f = fixture("outfit");
    let skinned = f.dir.join("jacket.glb");
    std::fs::write(&skinned, mini::triangle_glb(0.0, "jacket_cloth", true)).unwrap();
    let flat = f.dir.join("flat.glb");
    std::fs::write(&flat, mini::triangle_glb(0.0, "jacket_cloth", false)).unwrap();
    let app = &mut f.app;
    let mut w = start(app, Kind::Outfit, Some(level(app, mini::PARTSDB)));
    let State::Outfit(o) = &mut w.state else {
        panic!()
    };
    assert!(o.db.is_some());
    assert_eq!(o.sets, [true], "the only set is ticked");
    assert_eq!(o.template, Some(0), "the head part has the mesh");
    // Not skinned: refused.
    o.set_file(app, flat);
    w.step = 1;
    app.wizard_next(&mut w);
    assert!(
        w.error.as_deref().unwrap().contains("skinned"),
        "{:?}",
        w.error
    );
    let State::Outfit(o) = &mut w.state else {
        panic!()
    };
    o.part_name = "jacket".into();
    o.variant_name = "jacket".into();
    o.set_file(app, skinned);
    o.tags = "hatCulled".into();
    assert!(matches!(&o.plan, Some(Ok(p)) if p.summary.skeleton.is_some()));
    // Tint colours of the new material are edited before recording.
    assert_eq!(o.tints.len(), 1);
    w.step = 0;
    let label = finish(app, &mut w);
    assert_eq!(label, "Wizard: Add outfit jacket");
    assert_eq!(operations(app), [label]);
    let data = app.current_data(level(app, mini::PARTSDB)).unwrap();
    let db = PartsDb::parse(&data).unwrap();
    let part = db.part_index("jacket").expect("the part was added");
    assert_eq!(db.parts[part].tags, ["hatCulled"]);
    let variant = db.variant_index("jacket").unwrap();
    assert!(
        db.sets[0]
            .choices
            .iter()
            .any(|c| c.variants.contains(&variant))
    );
    let mesh = db.parts[part].models[0].0;
    let ws = app.workspace.as_ref().unwrap();
    assert!(
        ws.changes().any(|c| c.key.ids[3] == mesh && c.is_added()),
        "the new mesh is in the workspace"
    );
    let w = start(app, Kind::Outfit, Some(level(app, mini::PARTSDB)));
    draw_steps(&mut f, w, 6);
}

#[test]
fn place_wizard_joins_level_edits_and_cancel_takes_them_back() {
    let mut f = fixture("place");
    let app = &mut f.app;
    let group = level(app, mini::SIMGROUP);
    let group = doctor_toolkit::level::Location {
        stream: group.stream,
        resource: group.resource,
    };
    for cancel in [false, true] {
        let mut w = start(app, Kind::PlaceObject, None);
        app.wizard_next(&mut w);
        assert!(w.error.is_some(), "a model is needed");
        let State::Place(p) = &mut w.state else {
            panic!()
        };
        p.model = Some(level(app, mini::MESH));
        let tab = p.level;
        app.wizard_next(&mut w);
        assert!(w.error.is_none(), "{:?}", w.error);
        assert!(w.tx.mark.is_some(), "the task records from here");
        app.wizard_next(&mut w);
        assert!(w.error.is_some(), "nothing placed yet");
        let before = operations(app).len();
        // Two edits in the level view: a copy, and a copy of the copy.
        for object in [0, 1] {
            app.level_action(
                tab,
                crate::level_view::LevelAction::Duplicate { group, object },
            );
        }
        assert_eq!(operations(app).len(), before + 2);
        if cancel {
            let id = app.new_pane_id();
            app.wizard_cancel(id, &mut w);
            assert_eq!(operations(app).len(), before);
            assert_eq!(app.workspace.as_ref().unwrap().redo_label(), None);
        } else {
            let label = finish(app, &mut w);
            assert!(label.contains("2 edits"), "{label}");
            assert_eq!(operations(app), [label]);
        }
    }
    let w = start(app, Kind::PlaceObject, None);
    draw_steps(&mut f, w, 3);
}

#[test]
fn package_wizard_checks_builds_and_installs_a_mod() {
    let mut f = fixture("package");
    let app = &mut f.app;
    // Something to package: an edited string table.
    let mut texts = start(app, Kind::Texts, Some(level(app, mini::STRINGS)));
    let State::Texts(t) = &mut texts.state else {
        panic!()
    };
    t.find = "Hello".into();
    t.replace = "Howdy".into();
    t.replace_all();
    finish(app, &mut texts);
    assert!(app.enabled(Command::WizardPackage));

    let mut w = start(app, Kind::PackageMod, None);
    let State::Package(p) = &mut w.state else {
        panic!()
    };
    assert_eq!(p.items.len(), 1);
    // A name is needed.
    w.step = 1;
    app.wizard_next(&mut w);
    assert!(w.error.is_some());
    let State::Package(p) = &mut w.state else {
        panic!()
    };
    p.info.name = "Howdy World".into();
    p.info.id = doctor_toolkit::mods::id_from_name(&p.info.name);
    p.info.author = "Tester".into();
    let out = f.dir.join("howdy.zip");
    p.out = Some(out.clone());
    w.step = 0;
    let label = finish(app, &mut w);
    assert!(label.contains("howdy.zip"), "{label}");
    assert_eq!(operations(app).len(), 1, "building records nothing");
    let archive = doctor_toolkit::mods::package::ModArchive::open(&out).unwrap();
    assert_eq!(archive.manifest().id, "howdy-world");
    assert_eq!(archive.manifest().resources.len(), 1);
    let State::Package(p) = &mut w.state else {
        panic!()
    };
    assert!(p.built.is_some());
    let id = p.install(app, &out, &f.ctx).unwrap();
    assert_eq!(id, "howdy-world");
    // A second package of the same change now overlaps the installed mod.
    let mut again = start(app, Kind::PackageMod, None);
    let State::Package(p) = &mut again.state else {
        panic!()
    };
    p.info.name = "Other".into();
    p.info.id = "other".into();
    p.run_checks(app);
    let checked = p.checked.clone().unwrap();
    assert!(checked.errors.is_empty(), "{checked:?}");
    assert!(
        checked.warnings.iter().any(|w| w.contains("howdy-world")),
        "{checked:?}"
    );
    draw_steps(&mut f, again, 4);
}

#[test]
fn wizards_need_a_game_and_start_from_the_menu_and_views() {
    let mut f = fixture("menu");
    let game = f.app.game.take();
    for kind in Kind::ALL {
        assert!(!f.app.enabled(kind.command()), "{kind:?}");
        assert_eq!(Kind::from_slug(kind.slug()), Some(kind));
    }
    f.app.game = game;
    f.app.pending.push(Command::WizardTexts);
    f.app.pending.push(Command::Tasks);
    frame(&f.ctx, &mut f.app);
    frame(&f.ctx, &mut f.app);
    assert_eq!(f.app.wizards.tabs.len(), 1);
    let tree = f.app.dock.as_ref().unwrap();
    assert!(crate::app::dock::find(tree, |p| *p == Pane::Tasks).is_some());
    // The views of a model offer its wizards.
    let loc = level(&f.app, mini::MESH);
    f.app
        .view_action(loc, crate::views::ViewAction::StartWizard("retexture"));
    frame(&f.ctx, &mut f.app);
    let retexture = f
        .app
        .wizards
        .tabs
        .values()
        .find(|w| w.kind == Kind::Retexture)
        .expect("started from the view");
    let State::Retexture(r) = &retexture.state else {
        panic!()
    };
    assert_eq!(r.target, Some(loc));
    assert_eq!(
        crate::wizard_view::tasks_for(doctor_toolkit::hash::sdbm(b"lh2")),
        [("texts", "wizard.texts.title")]
    );
    // Keys built at run time have their English text.
    for c in doctor_toolkit::tunables::CATEGORIES {
        let key = format!("wizard.params.category.{c}");
        assert!(doctor_toolkit_mods::i18n::has_key(&key), "{key}");
    }
    let _ = ResourceKey::new(mini::LEVEL, &Default::default());
}

#[test]
fn curated_parameters_have_russian_texts() {
    let russian = params::russian();
    let curated = doctor_toolkit::tunables::gf2();
    for t in curated {
        let (label, description) = russian
            .get(&t.id)
            .unwrap_or_else(|| panic!("{}: no Russian text", t.id));
        assert!(!label.trim().is_empty(), "{}: empty label", t.id);
        assert!(
            !description.trim().is_empty(),
            "{}: empty description",
            t.id
        );
    }
    for id in russian.keys() {
        assert!(
            curated.iter().any(|t| &t.id == id),
            "{id}: unknown parameter"
        );
    }
    assert_eq!(russian.len(), curated.len());
}

/// Stage 103: a preset stops at the first step that fails, and a value may
/// hold a comma (`\,` or a JSON string).
#[test]
fn wizard_preset_stops_on_an_error_and_takes_commas() {
    assert_eq!(
        split_preset(r#"texts, find=a\,b ,replace="x, \"y\"",file=C:\dir\a.glb,step=2"#),
        [
            "texts",
            r"find=a,b",
            r#"replace="x, \"y\"""#,
            r"file=C:\dir\a.glb",
            "step=2"
        ]
    );
    assert_eq!(
        preset_item(r#"replace="x, \"y\"""#),
        ("replace".to_owned(), r#"x, "y""#.to_owned())
    );
    assert_eq!(preset_item("only"), ("only".to_owned(), String::new()));
    assert_eq!(
        preset_item(r"file=C:\dir\a.glb").1,
        r"C:\dir\a.glb",
        "kept as written"
    );
    // Nothing to replace: the step refuses, nothing is recorded.
    let mut f = fixture("preset-fail");
    f.app.wizards.preset_used = false;
    f.app
        .run_wizard_preset("texts,table=level,find=absent,replace=x,replace_all,step=9");
    assert!(operations(&f.app).is_empty());
    let error = f.app.preset_error().expect("the run failed").to_owned();
    assert!(
        error.contains("DOCTOR_TOOLKIT_GUI_WIZARD: texts"),
        "{error}"
    );
    // An unknown task fails too.
    let mut f = fixture("preset-unknown");
    f.app.run_wizard_preset("nonsense");
    assert!(f.app.preset_error().unwrap().contains("unknown task"));
    // A replacement with a comma, both ways of writing it.
    for (n, value) in [r"Mundo\, amigo", r#""Mundo, amigo""#].iter().enumerate() {
        let mut f = fixture(&format!("preset-comma-{n}"));
        let spec = format!("texts,table=level,find=world,replace={value},replace_all,step=9");
        f.app.run_wizard_preset(&spec);
        assert_eq!(f.app.preset_error(), None);
        assert_eq!(operations(&f.app).len(), 1);
        let loc = level(&f.app, mini::STRINGS);
        let table = StringTable::parse(&f.app.current_data(loc).unwrap()).unwrap();
        assert!(
            table.strings[0].iter().any(|s| s.contains("Mundo, amigo")),
            "{:?}",
            table.strings[0]
        );
    }
}

/// Stage 103: the sample of a new part is found outside the first ticked
/// set — `_all` of the player has nothing with a mesh in the hat slot —
/// and the wizard says where it comes from; a part may have a second level
/// of detail.
#[test]
fn outfit_wizard_takes_the_sample_from_another_set() {
    use doctor_toolkit::resource::partsdb::{MODEL_SLOTS, Part};
    use doctor_toolkit::resource::simgroup::Hash32;
    let mut f = fixture("outfit-all");
    let skinned = f.dir.join("hat.glb");
    std::fs::write(&skinned, mini::triangle_glb(0.0, "hat_cloth", true)).unwrap();
    let far = f.dir.join("hat_far.glb");
    std::fs::write(&far, mini::triangle_glb(0.5, "hat_cloth", true)).unwrap();
    let app = &mut f.app;
    let at = level(app, mini::PARTSDB);
    // `_all`: a copy of the set whose only choice of the slot is an empty part.
    let mut db = PartsDb::parse(&app.current_data(at).unwrap()).unwrap();
    let all = db.clone_set(0, "_all").unwrap();
    let empty = db
        .add_part(Part {
            name: "no_hat".into(),
            slot: Some(0),
            models: [Hash32(0); MODEL_SLOTS],
            label: String::new(),
            tags: Vec::new(),
        })
        .unwrap();
    let variant = db.add_variant("no_hat", vec![empty as u16]).unwrap();
    for choice in &mut db.sets[all].choices {
        if choice.slot == 0 {
            choice.variants = vec![variant];
        }
    }
    let game = app.game.clone().unwrap();
    app.workspace_mut()
        .unwrap()
        .set_resource(&game, at.stream, at.resource, db.encode().unwrap(), "_all")
        .unwrap();
    let mut w = start(app, Kind::Outfit, Some(at));
    let State::Outfit(o) = &mut w.state else {
        panic!()
    };
    assert_eq!(o.sets, [false, true], "`_all` is ticked");
    let templates = o.templates();
    assert!(templates.iter().all(|t| !t.ticked), "{templates:?}");
    assert_eq!(
        templates[0],
        outfit::Template {
            part: 0,
            set: Some(0),
            ticked: false
        }
    );
    assert_eq!(o.template, Some(0), "the sample of the other set");
    assert!(
        w.state.task_ref().check(0, app).is_ok(),
        "the first step passes"
    );
    // A sample named in the preset; an unknown one is an error of the step.
    let State::Outfit(o) = &mut w.state else {
        panic!()
    };
    o.preset("template", "nothing", app);
    assert!(w.state.task_ref().check(0, app).is_err());
    let State::Outfit(o) = &mut w.state else {
        panic!()
    };
    o.preset("template", &db.parts[0].name, app);
    assert_eq!(o.template, Some(0));
    o.part_name = "cap".into();
    o.variant_name = "cap".into();
    o.set_file(app, skinned);
    o.set_lod_file(app, Some(far));
    assert!(
        matches!(&o.lod_plan, Some(Ok(_))),
        "the far mesh is planned: {:?}",
        o.lod_plan.as_ref().and_then(|p| p.as_ref().err())
    );
    let summary = o.summary(app).join("\n");
    assert!(summary.contains("which is not ticked"), "{summary}");
    assert!(summary.contains("cap_lod2"), "{summary}");
    let before = operations(app).len();
    let label = finish(app, &mut w);
    assert_eq!(operations(app).len(), before + 1, "one operation");
    assert_eq!(label, "Wizard: Add outfit cap");
    let db = PartsDb::parse(&app.current_data(at).unwrap()).unwrap();
    let part = &db.parts[db.part_index("cap").unwrap()];
    let (near, far) = (part.models[0].0, part.models[1].0);
    assert!(near != 0 && far != 0 && near != far, "{near:x} {far:x}");
    let ws = app.workspace.as_ref().unwrap();
    for mesh in [near, far] {
        assert!(
            ws.changes().any(|c| c.key.ids[3] == mesh && c.is_added()),
            "mesh {mesh:x} is in the workspace"
        );
    }
    let variant = db.variant_index("cap").unwrap();
    assert!(
        db.sets[all]
            .choices
            .iter()
            .any(|c| c.variants.contains(&variant))
    );
    assert!(
        !db.sets[0]
            .choices
            .iter()
            .any(|c| c.variants.contains(&variant))
    );
    // The hints of the materials step and the size of the preview.
    assert!(outfit::is_stub_material("jacket_classic_op01_mat"));
    assert!(!outfit::is_stub_material("jacket_classic_op01_greyBlack"));
    let black = outfit::Tint {
        hash: 1,
        name: "m".into(),
        colours: vec![("g_TintColor0", vec![0.0; 4])],
    };
    assert!(black.is_black());
    let room = egui::vec2(1500.0, 700.0);
    let width = outfit::preview_width(room);
    assert!(
        width * 0.9 + 200.0 <= room.y + 0.5,
        "{width}: the view fits"
    );
    assert!(outfit::preview_width(egui::vec2(400.0, 2000.0)) <= 400.0);
}

/// Stage 103: with "In all tables and languages" the Edit texts wizard
/// gives the changed entries their text in every table that has them.
#[test]
fn texts_wizard_edits_every_table() {
    let mut f = fixture("texts-everywhere");
    let app = &mut f.app;
    let here = level(app, mini::STRINGS);
    let mut w = start(app, Kind::Texts, Some(here));
    let State::Texts(t) = &mut w.state else {
        panic!()
    };
    t.preset("find", "World", app);
    t.preset("replace", "Mundo", app);
    t.preset("replace_all", "", app);
    t.preset("everywhere", "", app);
    assert!(t.everywhere);
    let label = finish(app, &mut w);
    assert!(label.contains("2 table(s)"), "{label}");
    assert_eq!(operations(app).len(), 1, "one operation");
    let other = Location {
        stream: app.game.as_ref().unwrap().find_stream(mini::TEXT).unwrap(),
        resource: 0,
    };
    for loc in [here, other] {
        let table = StringTable::parse(&app.current_data(loc).unwrap()).unwrap();
        assert_eq!(table.strings[0][1], "Mundo", "{loc:?}");
    }
}
