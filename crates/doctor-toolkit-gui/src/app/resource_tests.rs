//! Stage 103, package B: the operations on a resource from its menus and
//! views, driven without a window on the miniature game — a file into the
//! resource, material parameters, a string in every table, a new string
//! with translations, the `$name` references of a movie, and the preset
//! `DOCTOR_TOOLKIT_GUI_RESOURCE` that does them without input.

use std::path::PathBuf;
use std::sync::Arc;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit::resource::material::{Material, ParamEdit, ParamValue};
use doctor_toolkit::testing as mini;
use doctor_toolkit::text::StringEdit;

use super::BrowserApp;
use super::asset_tab::AssetCmd;
use crate::nav::Location;
use crate::settings::{Settings, Store};
use crate::views::ViewAction;

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
    let dir = std::env::temp_dir().join(format!("dt-resource-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    mini::write_game(&dir).unwrap();
    mini::write_languages(&dir).unwrap();
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

/// Resource `resource` of stream `stream` of the miniature game.
fn at(app: &BrowserApp, stream: &str, resource: usize) -> Location {
    Location {
        stream: app.game.as_ref().unwrap().find_stream(stream).unwrap(),
        resource,
    }
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

fn table(app: &BrowserApp, loc: Location) -> StringTable {
    StringTable::parse(&app.current_data(loc).unwrap()).unwrap()
}

#[test]
fn a_file_replaces_a_resource_by_its_type_or_as_stored() {
    let mut f = fixture("import");
    let loc = at(&f.app, mini::LEVEL, mini::STRINGS);
    // The JSON of the export with one text changed: read by type.
    let mut doc = doctor_toolkit::text::export_json(&table(&f.app, loc), mini::LEVEL, 5, 0);
    let text = serde_json::to_string(&doc).unwrap();
    assert!(text.contains("World"), "{text}");
    doc = serde_json::from_str(&text.replace("World", "moon")).unwrap();
    let json = f.dir.join("strings.json");
    std::fs::write(&json, serde_json::to_vec(&doc).unwrap()).unwrap();
    assert!(f.app.import_resource_file(loc, &json, false));
    assert_eq!(operations(&f.app).len(), 1);
    assert!(table(&f.app, loc).strings[0].iter().any(|s| s == "moon"));
    // The same file again: nothing to record, and no error.
    assert!(f.app.import_resource_file(loc, &json, false));
    assert_eq!(operations(&f.app).len(), 1);
    assert_eq!(f.app.preset_error(), None);
    // As stored.
    let raw = f.dir.join("table.bin");
    let stored = mini::sample_table(["Raw", "Data"]).to_bytes().unwrap();
    std::fs::write(&raw, &stored).unwrap();
    assert!(f.app.import_resource_file(loc, &raw, true));
    assert_eq!(f.app.current_data(loc).unwrap(), stored);
    assert_eq!(operations(&f.app).len(), 2);
    // A file that is not what the type takes is an error, and so is a type
    // without an import by type.
    assert!(!f.app.import_resource_file(loc, &raw, false));
    assert!(f.app.preset_error().is_some());
    let mesh = at(&f.app, mini::LEVEL, mini::MESH);
    assert!(!f.app.import_resource_file(mesh, &json, false));
    assert_eq!(operations(&f.app).len(), 2, "nothing more was recorded");
    // The menu of the asset offers both for a string table, raw only for a mesh.
    let lh2 = doctor_toolkit::text::TYPE_LH2;
    assert!(doctor_toolkit::import::file::is_supported(lh2));
    assert!(!doctor_toolkit::import::file::is_supported(
        doctor_toolkit::resource::mesh::TYPE_MESH
    ));
    // Undo takes the import back.
    f.app.undo();
    assert!(table(&f.app, loc).strings[0].iter().any(|s| s == "moon"));
    frame(&f.ctx, &mut f.app);
}

#[test]
fn material_parameters_are_edited_in_the_view_and_recorded() {
    let mut f = fixture("material");
    let loc = at(&f.app, mini::LEVEL, mini::MATERIAL);
    let before = Material::parse(&f.app.current_data(loc).unwrap()).unwrap();
    let count = before.floats("g_Color").unwrap().len();
    let new: Vec<f32> = (0..count).map(|i| 0.1 * (i as f32 + 1.0)).collect();
    // The view of the material: fields, then Apply.
    let mut view =
        crate::material_view::MaterialView::new(Ok(before.clone()), (loc.stream, loc.resource));
    assert!(view.set_floats("g_Color", &new));
    let action = view.apply().expect("an edit to record");
    assert!(matches!(action, ViewAction::Apply { .. }));
    f.app.view_action(loc, action);
    assert_eq!(f.app.preset_error(), None);
    assert_eq!(operations(&f.app).len(), 1);
    let after = Material::parse(&f.app.current_data(loc).unwrap()).unwrap();
    assert_eq!(after.floats("g_Color").unwrap(), new);
    assert_eq!(
        after.params[0].value, before.params[0].value,
        "the rest is kept"
    );
    // The same through the operation the preset uses; the same values again
    // record nothing.
    let edit = ParamEdit::parse("g_BaseSampler=tex:000000aa").unwrap();
    assert!(f.app.set_material_params(loc, std::slice::from_ref(&edit)));
    assert!(f.app.set_material_params(loc, &[edit]));
    assert_eq!(operations(&f.app).len(), 2);
    let after = Material::parse(&f.app.current_data(loc).unwrap()).unwrap();
    assert_eq!(after.params[0].value, ParamValue::Texture(0xaa));
    // A parameter the material does not have, and a resource that is not a
    // material.
    let bad = ParamEdit::parse("g_Nothing=1").unwrap();
    assert!(!f.app.set_material_params(loc, std::slice::from_ref(&bad)));
    assert!(f.app.preset_error().unwrap().contains("g_Nothing"));
    let strings = at(&f.app, mini::LEVEL, mini::STRINGS);
    assert!(!f.app.set_material_params(strings, &[bad]));
    assert_eq!(operations(&f.app).len(), 2);
    // "Edit material…" of the material panel opens the editor of the material.
    crate::material_view::ask_edit(&f.ctx, before.name_hash);
    frame(&f.ctx, &mut f.app);
    frame(&f.ctx, &mut f.app);
    assert!(f.app.docs.values().any(|d| d.loc == loc), "the tab is open");
    assert!(
        f.app
            .docs
            .values()
            .any(|d| d.views.iter().any(|(id, _)| *id == "material")),
        "with the material view"
    );
    assert!(!f.app.open_material(0x1234_5678), "no such material");
}

#[test]
fn a_string_changes_in_every_table_and_a_new_one_takes_translations() {
    let mut f = fixture("strings");
    let level = at(&f.app, mini::LEVEL, mini::STRINGS);
    // Every table of the game that has the entry.
    let index = f.app.index.clone().unwrap();
    let tables = doctor_toolkit::text::all_tables(&index);
    assert!(tables.len() >= 2, "{tables:?}");
    let hash = table(&f.app, level).hashes[1];
    let with_entry: Vec<Location> = tables
        .iter()
        .map(|t| Location {
            stream: t.stream,
            resource: t.resource,
        })
        .filter(|&l| table(&f.app, l).find(hash).is_some())
        .collect();
    assert!(with_entry.len() >= 2, "the entry is in both tables");
    // The view with "In all tables and languages" ticked: Save asks for
    // the edit everywhere.
    let mut view = crate::text_view::TextView::from_table(Ok(table(&f.app, level)), None);
    assert!(view.set_text(hash, "Everywhere"));
    assert_eq!(view.changed_entries(), [(hash, "Everywhere".to_owned())]);
    view.save_everywhere = true;
    f.app.view_action(
        level,
        ViewAction::EditStrings(vec![StringEdit {
            hash,
            text: "Everywhere".into(),
            language: None,
        }]),
    );
    assert_eq!(f.app.preset_error(), None);
    assert_eq!(operations(&f.app).len(), 1, "one operation for all tables");
    for &l in &with_entry {
        let t = table(&f.app, l);
        let row = t.find(hash).unwrap();
        assert!(t.strings.iter().all(|column| column[row] == "Everywhere"));
    }
    // An entry no table has is an error.
    assert!(!f.app.edit_strings_everywhere(&[StringEdit {
        hash: 0xdead_beef,
        text: "x".into(),
        language: None,
    }]));
    assert!(f.app.preset_error().is_some());
    assert_eq!(operations(&f.app).len(), 1);
    // A new string: a language the game does not have is refused, nothing
    // is recorded.
    let mut translations = std::collections::BTreeMap::new();
    translations.insert("zz".to_owned(), "?".to_owned());
    assert!(
        !f.app
            .add_string_translated("doctor_new", "Hello", &translations)
    );
    assert_eq!(operations(&f.app).len(), 1);
    // The form of the view gives its translations to the app: `en` gets
    // its own text.
    f.app.view_action(
        level,
        ViewAction::AddString {
            name: "doctor_new".into(),
            text: "Hello".into(),
            everywhere: true,
            translations: vec![("en".into(), "Hello in English".into())],
        },
    );
    assert_eq!(operations(&f.app).len(), 2);
    let hash = doctor_toolkit::hash::string_id("doctor_new");
    let language =
        doctor_toolkit::text::language_tables(f.app.game.as_ref().unwrap(), &index).unwrap();
    assert_eq!(language.len(), 1);
    for t in &language {
        let l = Location {
            stream: t.stream,
            resource: t.resource,
        };
        let table = table(&f.app, l);
        let row = table.find(hash).expect("the string was added");
        assert_eq!(table.strings[0][row], "Hello in English");
    }
    frame(&f.ctx, &mut f.app);
}

#[test]
fn collision_is_built_for_a_metamodel_only() {
    let mut f = fixture("collision");
    use doctor_toolkit::resource::hkx::collision::CollisionKind;
    // The miniature game has no metamodel: the action says so and records
    // nothing (the build itself is checked on the game, see the stage notes).
    let mesh = at(&f.app, mini::LEVEL, mini::MESH);
    assert!(!f.app.build_collision(mesh, CollisionKind::Box, None));
    assert!(f.app.preset_error().is_some());
    assert!(operations(&f.app).is_empty());
    // The command of the asset menu goes the same way.
    f.app
        .asset_cmds
        .push((mesh, AssetCmd::BuildCollision(CollisionKind::Mesh)));
    frame(&f.ctx, &mut f.app);
    assert!(operations(&f.app).is_empty());
}

#[test]
fn the_resource_preset_runs_its_items_and_stops_at_the_first_error() {
    let mut f = fixture("preset");
    let material = at(&f.app, mini::LEVEL, mini::MATERIAL);
    let count = Material::parse(&f.app.current_data(material).unwrap())
        .unwrap()
        .floats("g_Color")
        .unwrap()
        .len();
    let values = vec!["0.5"; count].join("/");
    // Two parameters are one operation.
    let spec = format!("param=g_Color={values},param=g_BaseSampler=tex:000000bb");
    assert!(f.app.run_resource_preset(material, &spec));
    assert_eq!(operations(&f.app).len(), 1);
    let m = Material::parse(&f.app.current_data(material).unwrap()).unwrap();
    assert_eq!(m.floats("g_Color").unwrap(), vec![0.5; count]);
    assert_eq!(m.params[0].value, ParamValue::Texture(0xbb));
    // A string everywhere (the text holds a comma), then a new string.
    let strings = at(&f.app, mini::LEVEL, mini::STRINGS);
    let hash = table(&f.app, strings).hashes[0];
    let spec = format!(r#"string="0x{hash:08x}:Hello, there",add_string=doctor_preset:New\, too"#);
    assert!(f.app.run_resource_preset(strings, &spec));
    assert_eq!(f.app.preset_error(), None);
    assert_eq!(operations(&f.app).len(), 3);
    assert!(
        table(&f.app, strings)
            .strings
            .iter()
            .all(|c| c[0] == "Hello, there")
    );
    // The first item that fails stops the run: the edit after it is not made.
    let spec = format!("param=g_Nothing=1,param=g_Color={values}");
    let before = operations(&f.app).len();
    assert!(!f.app.run_resource_preset(material, &spec));
    assert!(
        !f.app
            .run_resource_preset(material, "collision=box,raw=/nonexistent")
    );
    assert!(!f.app.run_resource_preset(material, "explode"));
    assert!(!f.app.run_resource_preset(strings, "string=no-colon"));
    assert!(
        !f.app.run_resource_preset(strings, "apx=a:b"),
        "not a movie"
    );
    assert_eq!(operations(&f.app).len(), before);
    assert!(f.app.preset_error().unwrap().contains("g_Nothing"));
    frame(&f.ctx, &mut f.app);
}

#[test]
fn the_text_references_of_a_movie_are_listed() {
    // The view counts the `$name` references of the movie; pointing them at
    // another string is `text::retarget_apx` (tested in the library).
    use doctor_toolkit::resource::apx::{AptFile, Block, TAG_CONSTANTS, TAG_TITLE};
    let file = AptFile {
        blocks: vec![
            Block {
                tag: TAG_TITLE,
                data: b"menu\0".to_vec(),
            },
            Block {
                tag: TAG_CONSTANTS,
                data: b"Apt constant file\x1a\0$fe_press_start\0\0$ui_x\0$ui_x\0".to_vec(),
            },
        ],
    };
    let data = file.to_bytes().unwrap();
    let view = crate::apx_view::ApxView::from_data(Ok(data), None);
    assert_eq!(view.text_refs(), [("fe_press_start", 1), ("ui_x", 2)]);
}
