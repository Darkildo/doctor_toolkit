//! The panel without a window: dropped files, sets of mods, undo, the
//! states of the cards, writing with a result.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::mods::{ModInfo, build_from_workspace};
use doctor_toolkit::testing::{self, LEVEL, MESH, NAV};
use doctor_toolkit::workspace::{ResourceEdit, StorageChoice, StreamAction, Workspace};

use super::*;
use crate::state::ModState;

/// A miniature game in a temporary folder (with the extra streams when
/// `extra`).
fn game(extra: bool) -> (tempfile::TempDir, Arc<GameInstance>) {
    let dir = tempfile::tempdir().unwrap();
    testing::write_game(dir.path()).unwrap();
    if extra {
        testing::write_extra_streams(dir.path()).unwrap();
    }
    let game = GameInstance::open(dir.path()).unwrap();
    (dir, Arc::new(game))
}

/// A mod `id` that sets resource `index` of `stream` to `data`.
fn build(
    game: &GameInstance,
    out: &Path,
    id: &str,
    stream: &str,
    index: usize,
    data: Vec<u8>,
) -> PathBuf {
    let mut ws = Workspace::create(out.join(format!("ws-{id}")), game).unwrap();
    let edit = ResourceEdit {
        index,
        data,
        storage: StorageChoice::Keep,
    };
    ws.set_resources(game, "edit", game.find_stream(stream).unwrap(), &[edit])
        .unwrap();
    let info = ModInfo {
        id: id.to_owned(),
        name: format!("Mod {id}"),
        version: "1.0".into(),
        author: "tester".into(),
        description: "A test mod.\n\nMore.".into(),
        url: None,
        relations: Default::default(),
        whole_resources: false,
    };
    let path = out.join(format!("{id}.zip"));
    build_from_workspace(&ws, &info, None, None, &path).unwrap();
    path
}

fn panel(game: &Arc<GameInstance>, ctx: &egui::Context) -> ModsPanel {
    let mut panel = ModsPanel::standalone();
    panel.set_game(Some(game.clone()), ctx);
    panel.wait();
    panel
}

/// `(id, enabled)` in load order.
fn order(panel: &ModsPanel) -> Vec<(String, bool)> {
    panel
        .manager()
        .unwrap()
        .mods()
        .iter()
        .map(|m| (m.id.clone(), m.enabled))
        .collect()
}

fn pairs(list: &[(&str, bool)]) -> Vec<(String, bool)> {
    list.iter().map(|(i, e)| ((*i).to_owned(), *e)).collect()
}

/// A file dropped on the window.
#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

/// One frame of the panel; `dropped` are files dropped on the window.
fn frame(ctx: &egui::Context, panel: &mut ModsPanel, dropped: &[PathBuf]) -> Option<PanelEvent> {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 800.0),
        )),
        dropped_files: dropped
            .iter()
            .map(|p| Arc::new(Dropped(p.clone())) as egui::DroppedFileHandle)
            .collect(),
        ..Default::default()
    };
    let mut event = None;
    let mut output = ctx.run_ui(input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            event = panel.show(ui, None);
        });
    });
    output.textures_delta.clear();
    event
}

#[test]
fn dropped_mod_files_install_and_others_are_refused() {
    let (_dir, game) = game(false);
    let temp = tempfile::tempdir().unwrap();
    let a = build(&game, temp.path(), "a", LEVEL, MESH, vec![1; 300]);
    let text = temp.path().join("notes.txt");
    std::fs::write(&text, "hello").unwrap();
    let ctx = egui::Context::default();
    let mut panel = panel(&game, &ctx);
    frame(&ctx, &mut panel, &[]);
    frame(&ctx, &mut panel, &[a, text]);
    panel.wait();
    assert_eq!(order(&panel), pairs(&[("a", true)]));
    assert_eq!(panel.selected(), Some("a"), "the new mod is selected");
    let (message, error) = panel.message().unwrap();
    assert!(error && message.contains("notes.txt"), "{message}");
    // The panel draws its cards, the selected mod and the dialogs.
    for _ in 0..3 {
        frame(&ctx, &mut panel, &[]);
    }
    // Opened from the system: asks first.
    panel.offer_install(vec![temp.path().join("a.zip")]);
    assert!(panel.has_dialog());
    frame(&ctx, &mut panel, &[]);
}

/// The picture on the card of mod `id`, as the panel holds it.
fn picture_of(panel: &ModsPanel, id: &str) -> Option<Option<egui::TextureHandle>> {
    let m = panel.manager().unwrap().get(id)?;
    panel.previews.get(&cards::preview_key(m)).cloned()
}

#[test]
fn an_update_shows_the_picture_of_the_new_version() {
    let (_dir, game) = game(false);
    let temp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut panel = panel(&game, &ctx);
    // Version 1.0 without a picture, as the card shows it.
    let first = build(&game, temp.path(), "a", LEVEL, MESH, vec![1; 300]);
    panel.install_files(&ctx, vec![first]);
    panel.wait();
    frame(&ctx, &mut panel, &[]);
    assert!(
        matches!(picture_of(&panel, "a"), Some(None)),
        "no picture yet"
    );
    // Version 1.1 of the same mod with a picture, installed over it while
    // the window stays open (stage 63: the card kept the empty picture).
    let mut ws = Workspace::create(temp.path().join("ws-a2"), &game).unwrap();
    let edit = ResourceEdit {
        index: MESH,
        data: vec![2; 300],
        storage: StorageChoice::Keep,
    };
    ws.set_resources(&game, "edit", game.find_stream(LEVEL).unwrap(), &[edit])
        .unwrap();
    let info = ModInfo {
        id: "a".into(),
        name: "Mod a".into(),
        version: "1.1".into(),
        author: "tester".into(),
        description: "A test mod.".into(),
        url: None,
        relations: Default::default(),
        whole_resources: false,
    };
    let png = doctor_toolkit::resource::pixels::encode_png(2, 1, &[255; 8]).unwrap();
    let preview = doctor_toolkit::mods::Preview::from_image(png).unwrap();
    let second = temp.path().join("a-1.1.zip");
    build_from_workspace(&ws, &info, None, Some(&preview), &second).unwrap();
    panel.install_files(&ctx, vec![second]);
    panel.wait();
    frame(&ctx, &mut panel, &[]);
    let picture = picture_of(&panel, "a").expect("the card of 1.1 was drawn");
    assert_eq!(picture.expect("the picture of 1.1").size(), [2, 1]);
    assert_eq!(panel.previews.len(), 1, "the entry of 1.0 is dropped");
}

#[test]
fn cards_tell_the_state_of_each_mod() {
    let (_dir, game) = game(false);
    let (_other_dir, other) = game_with_nav();
    let temp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut panel = panel(&game, &ctx);
    let a = build(&game, temp.path(), "a", LEVEL, MESH, vec![1; 300]);
    let b = build(&game, temp.path(), "b", LEVEL, MESH, vec![2; 300]);
    let nav = build(&other, temp.path(), "nav", NAV, 0, vec![3; 64]);
    panel.install_files(&ctx, vec![a, b, nav]);
    panel.wait();
    assert_eq!(
        panel.state_of("a"),
        Some(ModState::Conflict(vec![state::Rival {
            name: "Mod b".into(),
            wins: true,
            resources: 1
        }]))
    );
    assert!(matches!(panel.state_of("b"), Some(ModState::Conflict(_))));
    assert_eq!(panel.state_of("nav"), Some(ModState::Misfit(1)));
    for id in ["a", "nav"] {
        panel.set_enabled(&ctx, id, false);
        panel.wait();
    }
    assert_eq!(panel.state_of("a"), Some(ModState::Off));
    assert_eq!(panel.state_of("b"), Some(ModState::NotWritten));
    assert!(!panel.up_to_date());
    panel.apply(&ctx);
    panel.wait();
    assert_eq!(panel.state_of("b"), Some(ModState::Ready));
    assert!(panel.up_to_date());
    // Writing told its result in a window.
    assert!(matches!(
        panel.dialog,
        Some(dialogs::Dialog::Report(Report { error: false, .. }))
    ));
    for id in ["a", "b", "nav"] {
        let state = panel.state_of(id).unwrap();
        assert!(!state.text().is_empty() && !state.hint().is_empty());
    }
}

/// A game whose extra streams the plain game lacks.
fn game_with_nav() -> (tempfile::TempDir, Arc<GameInstance>) {
    game(true)
}

#[test]
fn changes_are_undone() {
    let (_dir, game) = game(false);
    let temp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut panel = panel(&game, &ctx);
    let files: Vec<PathBuf> = ["a", "b", "c"]
        .iter()
        .enumerate()
        .map(|(i, id)| build(&game, temp.path(), id, LEVEL, MESH, vec![i as u8; 300]))
        .collect();
    panel.install_files(&ctx, files);
    panel.wait();
    assert_eq!(panel.undo_label(), None, "installing is not undone");

    panel.set_enabled(&ctx, "b", false);
    panel.wait();
    assert!(panel.undo_label().unwrap().contains("Mod b"));
    panel.undo(&ctx);
    panel.wait();
    assert_eq!(
        order(&panel),
        pairs(&[("a", true), ("b", true), ("c", true)])
    );
    assert_eq!(panel.undo_label(), None);

    panel.move_mod(&ctx, "c", 0);
    panel.wait();
    assert_eq!(order(&panel)[0].0, "c");
    panel.undo(&ctx);
    panel.wait();
    assert_eq!(
        order(&panel),
        pairs(&[("a", true), ("b", true), ("c", true)])
    );

    panel.set_enabled(&ctx, "a", false);
    panel.wait();
    panel.remove(&ctx, "a");
    panel.wait();
    assert_eq!(order(&panel), pairs(&[("b", true), ("c", true)]));
    panel.undo(&ctx);
    panel.wait();
    assert_eq!(
        order(&panel),
        pairs(&[("a", false), ("b", true), ("c", true)]),
        "back in its place, still off"
    );

    // A removed mod that is not undone leaves no files behind.
    panel.remove(&ctx, "c");
    panel.wait();
    let dir = panel.manager().unwrap().dir().to_path_buf();
    drop(panel);
    let left: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.') || n == "c")
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn sets_of_mods_switch_and_undo() {
    let (_dir, game) = game(false);
    let temp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut panel = panel(&game, &ctx);
    let files: Vec<PathBuf> = ["a", "b"]
        .iter()
        .enumerate()
        .map(|(i, id)| build(&game, temp.path(), id, LEVEL, MESH, vec![i as u8; 300]))
        .collect();
    panel.install_files(&ctx, files);
    panel.wait();
    panel.save_set(&ctx, " Everything ");
    panel.wait();
    assert_eq!(
        panel.sets(),
        (&["Everything".to_owned()][..], Some("Everything"))
    );
    panel.set_enabled(&ctx, "a", false);
    panel.wait();
    assert_eq!(panel.sets().1, None);
    panel.save_set(&ctx, "Only b");
    panel.wait();
    panel.use_set(&ctx, "Everything");
    panel.wait();
    assert_eq!(order(&panel), pairs(&[("a", true), ("b", true)]));
    assert_eq!(panel.sets().1, Some("Everything"));
    panel.undo(&ctx);
    panel.wait();
    assert_eq!(panel.sets().1, Some("Only b"));
    panel.delete_set(&ctx, "Only b");
    panel.wait();
    assert_eq!(panel.sets().0, ["Everything".to_owned()]);
    // The menu of sets draws.
    frame(&ctx, &mut panel, &[]);
}

/// Stage 108: the mods are brought to the base of a project — the
/// arrangement before is saved as a set, the base is enabled and written —
/// and the set (or Undo) is the way back.
#[test]
fn the_mods_are_brought_to_the_base_of_a_project() {
    use doctor_toolkit::project::{Base, BaseMod};
    let (_dir, game) = game(false);
    let temp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut panel = panel(&game, &ctx);
    let files: Vec<PathBuf> = ["a", "b"]
        .iter()
        .enumerate()
        .map(|(i, id)| build(&game, temp.path(), id, LEVEL, MESH, vec![i as u8; 300]))
        .collect();
    panel.install_files(&ctx, files);
    panel.wait();
    panel.set_enabled(&ctx, "b", false);
    panel.wait();
    let base = Base {
        exclusive: true,
        mods: vec![BaseMod {
            id: "b".into(),
            version: "^1.0".into(),
            built_on: "1.0".into(),
            sha256: String::new(),
        }],
    };
    assert!(!panel.up_to_date());
    panel.bring_to_base(&ctx, base.clone(), "Yard Mod");
    assert_eq!(frame(&ctx, &mut panel, &[]), Some(PanelEvent::WillWrite));
    panel.wait();
    assert_eq!(frame(&ctx, &mut panel, &[]), Some(PanelEvent::GameChanged));
    assert_eq!(order(&panel), pairs(&[("b", true), ("a", false)]));
    assert!(panel.up_to_date());
    assert_eq!(panel.state_of("b"), Some(ModState::Ready));
    assert_eq!(panel.sets(), (&["Before Yard Mod".to_owned()][..], None));
    // The result is told in a window, with the name of the set.
    assert!(panel.has_dialog());
    let (message, error) = panel.message().unwrap();
    assert!(!error && message.contains("Yard Mod"), "{message}");
    assert!(panel.undo_label().unwrap().contains("Yard Mod"));

    // Back by the set.
    panel.use_set(&ctx, "Before Yard Mod");
    panel.wait();
    assert_eq!(order(&panel), pairs(&[("a", true), ("b", false)]));
    // A mod of the base that is not installed: told, nothing changes.
    let mut missing = base;
    missing.mods[0].id = "gone".into();
    panel.bring_to_base(&ctx, missing, "Yard Mod");
    panel.wait();
    let (message, error) = panel.message().unwrap();
    assert!(
        error && message.contains("`gone` 1.0 is not installed"),
        "{message}"
    );
    assert_eq!(order(&panel), pairs(&[("a", true), ("b", false)]));
    assert_eq!(panel.sets().0.len(), 1);
}

#[test]
fn integrity_and_reports_are_plain_words() {
    let (_dir, game) = game(false);
    let ctx = egui::Context::default();
    let mut panel = panel(&game, &ctx);
    panel.check_integrity(&ctx);
    panel.wait();
    let Some(dialogs::Dialog::Report(report)) = &panel.dialog else {
        panic!("no report");
    };
    assert!(!report.error, "{}", report.text);
    let mut report = ApplyReport::default();
    assert!(describe(&report, "Applied").starts_with("Nothing to do"));
    report
        .streams
        .push(("gf2/a".into(), StreamAction::Rebuilt(2)));
    report
        .streams
        .push(("gf2/b".into(), StreamAction::Restored));
    report.written.push("gf2/a.str".into());
    assert_eq!(
        describe(&report, "Applied"),
        "Applied: 1 stream rebuilt, 1 restored, 1 file written"
    );
}

#[test]
fn a_card_moves_to_the_gap_it_is_dropped_in() {
    use crate::cards::target;
    assert_eq!(target(0, 0), None);
    assert_eq!(target(0, 1), None);
    assert_eq!(target(0, 2), Some(1));
    assert_eq!(target(0, 3), Some(2));
    assert_eq!(target(2, 0), Some(0));
    assert_eq!(target(2, 2), None);
    assert_eq!(target(2, 3), None);
    assert_eq!(
        crate::cards::summary("One\ntwo  three\n\nNext"),
        "One two three"
    );
    assert!(is_mod_file(Path::new("a/b.ZIP")) && !is_mod_file(Path::new("a.txt")));
}

/// The game as a write left it that stopped after the stream and before the
/// table of contents, with NVIDIA's shader cache next to it: the panel says
/// so on opening, and Restore originals brings the stream back byte for byte.
#[test]
fn an_interrupted_write_is_told_and_restored() {
    use doctor_toolkit::mods::manager::applied_path;
    use doctor_toolkit::workspace::{AppliedStream, Deployment, FileStamp};

    let (dir, _) = game(false);
    let root = dir.path();
    let cache = root.join("GLCache/a5006eb268eeda1b/1271864ec7dbc5c8");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(
        cache.join("483ab422c6120620.toc"),
        b"CDVN\0\0\x03\0 a shader",
    )
    .unwrap();
    let game = Arc::new(GameInstance::open(root).unwrap());
    let temp = tempfile::tempdir().unwrap();
    let a = build(&game, temp.path(), "a", LEVEL, MESH, vec![1; 500]);
    let level = root.join("gf2/level.str");
    let toc = root.join("gf2/level.toc");
    let (original_level, original_toc) =
        (std::fs::read(&level).unwrap(), std::fs::read(&toc).unwrap());
    let mut manager = ModManager::open(&game).unwrap();
    manager.install_file(&game, &a, true).unwrap();
    manager
        .apply(
            &game,
            doctor_toolkit::mods::manager::ModApplyOptions::default(),
        )
        .unwrap();
    let written = std::fs::read(&level).unwrap();
    std::fs::write(&toc, &original_toc).unwrap();
    let original = FileStamp::of_data(&original_level);
    let mut record = Deployment::default();
    record.streams.insert(
        LEVEL.to_owned(),
        AppliedStream {
            path: "gf2/level.str".into(),
            original: original.clone(),
            written: original,
            in_flight: Some(FileStamp::of_data(&written)),
            resources: Default::default(),
            data_size: None,
            new_stream: None,
        },
    );
    record.save(&applied_path(root)).unwrap();

    let ctx = egui::Context::default();
    let mut panel = panel(&game, &ctx);
    assert_eq!(panel.overview.interrupted, [LEVEL]);
    assert!(!panel.up_to_date());
    frame(&ctx, &mut panel, &[]);
    panel.unapply(&ctx);
    panel.wait();
    assert_eq!(std::fs::read(&level).unwrap(), original_level);
    assert_eq!(std::fs::read(&toc).unwrap(), original_toc);
    assert!(panel.overview.interrupted.is_empty());
    assert!(!panel.manager().unwrap().has_written());
}

/// A mod `id` of version `version` that needs the mods `depends` (id,
/// condition) and sets resource `index` of `gf2/level` to `data`.
fn build_needing(
    game: &GameInstance,
    out: &Path,
    id: &str,
    version: &str,
    depends: &[(&str, &str)],
    index: usize,
    data: Vec<u8>,
) -> PathBuf {
    let mut ws = Workspace::create(out.join(format!("ws-{id}-{version}")), game).unwrap();
    let edit = ResourceEdit {
        index,
        data,
        storage: StorageChoice::Keep,
    };
    ws.set_resources(game, "edit", game.find_stream(LEVEL).unwrap(), &[edit])
        .unwrap();
    let info = ModInfo {
        id: id.to_owned(),
        name: format!("Mod {id}"),
        version: version.into(),
        relations: doctor_toolkit::mods::Relations {
            depends: depends
                .iter()
                .map(|(d, c)| ((*d).to_owned(), (*c).to_owned()))
                .collect(),
            ..Default::default()
        },
        ..ModInfo::default()
    };
    let path = out.join(format!("{id}-{version}.zip"));
    build_from_workspace(&ws, &info, None, None, &path).unwrap();
    path
}

#[test]
fn cards_tell_what_a_mod_needs_and_the_order_keeps_it() {
    let (_dir, game) = game(false);
    let temp = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut panel = panel(&game, &ctx);
    let base = build_needing(&game, temp.path(), "base", "1.0", &[], MESH, vec![1; 300]);
    let patch = build_needing(
        &game,
        temp.path(),
        "patch",
        "1.0",
        &[("base", ">=1.1")],
        MESH + 1,
        vec![2; 64],
    );
    // The patch first: the base, installed later, goes above it.
    panel.install_files(&ctx, vec![patch, base]);
    panel.wait();
    assert_eq!(order(&panel), pairs(&[("base", true), ("patch", true)]));
    let Some(ModState::NeedsMods(needs)) = panel.state_of("patch") else {
        panic!("{:?}", panel.state_of("patch"));
    };
    assert_eq!(needs[0].kind, state::NeedKind::Version("1.0".into()));
    assert_eq!(needs[0].label(), "Mod base ≥1.1");
    let state = panel.state_of("patch").unwrap();
    assert!(state.text().contains("Mod base") && state.hint().contains("1.0"));

    // Dragging the base below the patch is refused, with the reason.
    panel.move_mod(&ctx, "base", 1);
    panel.wait();
    assert_eq!(order(&panel)[0].0, "base");
    let (message, error) = panel.message().unwrap();
    assert!(error && message.contains("Mod base"), "{message}");

    // Turning the base off asks first; so does turning the patch on alone.
    let base_2 = build_needing(
        &game,
        &temp.path().join("2"),
        "base",
        "1.2",
        &[],
        MESH,
        vec![3; 300],
    );
    panel.install_files(&ctx, vec![base_2]);
    panel.wait();
    assert_eq!(panel.state_of("patch"), Some(ModState::NotWritten));
    panel.set_enabled(&ctx, "base", false);
    assert!(matches!(
        panel.dialog,
        Some(dialogs::Dialog::DisableNeeded { .. })
    ));
    panel.dialog = None;
    panel.switch(&ctx, "patch", false);
    panel.wait();
    panel.switch(&ctx, "base", false);
    panel.wait();
    panel.set_enabled(&ctx, "patch", true);
    assert!(matches!(
        panel.dialog,
        Some(dialogs::Dialog::EnableNeeded { .. })
    ));
    panel.dialog = None;
    panel.enable_with_dependencies(&ctx, "patch");
    panel.wait();
    assert_eq!(order(&panel), pairs(&[("base", true), ("patch", true)]));

    // Sorting puts a mod moved by hand above what it needs back below it.
    let mut manager = panel.manager.take().unwrap();
    let entry = |id: &str| doctor_toolkit::mods::manager::SetEntry {
        id: id.into(),
        enabled: true,
    };
    manager.arrange(&[entry("patch"), entry("base")]).unwrap();
    panel.manager = Some(manager);
    panel.refresh(&ctx);
    panel.wait();
    assert!(matches!(
        panel.state_of("patch"),
        Some(ModState::NeedsMods(n)) if n[0].kind == state::NeedKind::Later
    ));
    panel.sort(&ctx);
    panel.wait();
    assert_eq!(order(&panel), pairs(&[("base", true), ("patch", true)]));
    assert_eq!(panel.state_of("patch"), Some(ModState::NotWritten));
    panel.select("patch");
    frame(&ctx, &mut panel, &[]);
}

/// The smallest file the mod manager takes for a 32-bit DLL.
fn dll(tag: &str) -> Vec<u8> {
    let mut data = vec![0u8; 0x100];
    data[..2].copy_from_slice(b"MZ");
    data[0x3C] = 0x40;
    data[0x40..0x44].copy_from_slice(b"PE\0\0");
    data[0x44..0x46].copy_from_slice(&0x014Cu16.to_le_bytes());
    data[0x56..0x58].copy_from_slice(&0x2102u16.to_le_bytes());
    data.extend_from_slice(tag.as_bytes());
    data
}

/// A mod `id` that changes a mesh and carries the module `<id>.asi`.
fn build_with_code(game: &GameInstance, out: &Path, id: &str, required: bool) -> PathBuf {
    use doctor_toolkit::mods::{NativeBuild, Relations};
    let mut ws = Workspace::create(out.join(format!("ws-{id}")), game).unwrap();
    let edit = ResourceEdit {
        index: MESH,
        data: vec![0x5A; 300],
        storage: StorageChoice::Keep,
    };
    ws.set_resources(game, "edit", game.find_stream(LEVEL).unwrap(), &[edit])
        .unwrap();
    let info = ModInfo {
        id: id.to_owned(),
        name: format!("Mod {id}"),
        version: "1.0".into(),
        author: "tester".into(),
        description: "A mod with code.".into(),
        url: None,
        relations: Relations {
            native: Some(NativeBuild {
                modules: vec![(format!("{id}.asi"), dll(id))],
                platform_abi: Some(doctor_toolkit::mods::native::PLATFORM_ABI),
                required,
            }),
            ..Relations::none()
        },
        whole_resources: false,
    };
    let path = out.join(format!("{id}.zip"));
    build_from_workspace(&ws, &info, None, None, &path).unwrap();
    path
}

#[test]
fn a_mod_with_code_asks_before_anything_executable() {
    use doctor_toolkit::mods::native::{Consent, NativeHost, NativeState};
    let (dir, game) = game(false);
    let root = dir.path();
    let temp = tempfile::tempdir().unwrap();
    let platform = temp.path().join("doctor_platform.asi");
    let loader = temp.path().join("doctor_loader.dll");
    std::fs::write(&platform, dll("platform")).unwrap();
    std::fs::write(&loader, dll("loader")).unwrap();
    let ctx = egui::Context::default();
    let mut panel = panel(&game, &ctx);
    panel.set_native_host(NativeHost {
        edition: Ok(()),
        loader: None,
        platform_binary: Some(platform),
        loader_binary: Some(loader),
    });
    let soft = build_with_code(&game, temp.path(), "soft", false);
    panel.install_files(&ctx, vec![soft]);
    panel.wait();

    // The question opens by itself; the card waits, nothing is in the game.
    assert!(matches!(panel.dialog, Some(dialogs::Dialog::Code(_))));
    assert_eq!(
        panel.state_of("soft"),
        Some(ModState::CodeWaiting { required: false })
    );
    let view = panel.code_of("soft").unwrap().clone();
    assert_eq!(view.consent, Consent::Unasked);
    assert_eq!(view.question.modules[0].path, "scripts/soft.asi");
    assert_eq!(
        view.question
            .toolkit
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        ["scripts/doctor_platform.asi", "dinput8.dll"]
    );
    frame(&ctx, &mut panel, &[]);
    // "Decide later" asks no more by itself; writing puts the data only.
    panel.dialog = None;
    panel.refresh(&ctx);
    panel.wait();
    assert!(panel.dialog.is_none(), "the question opens by itself once");
    panel.apply(&ctx);
    panel.wait();
    panel.dialog = None;
    assert!(!root.join("scripts").exists());
    assert_eq!(
        panel.state_of("soft"),
        Some(ModState::CodeWaiting { required: false })
    );

    // "Data only": the mod is ready without its code.
    panel.answer_code(&ctx, "soft", Some(false));
    panel.wait();
    assert_eq!(panel.state_of("soft"), Some(ModState::Ready));
    assert!(!root.join("scripts").exists());

    // "Allow": not in the game yet, then installed by writing.
    panel.answer_code(&ctx, "soft", Some(true));
    panel.wait();
    assert_eq!(panel.state_of("soft"), Some(ModState::NotWritten));
    assert!(!panel.up_to_date());
    assert!(!root.join("scripts").exists());
    panel.apply(&ctx);
    panel.wait();
    let Some(dialogs::Dialog::Report(report)) = panel.dialog.take() else {
        panic!("writing ends with a result");
    };
    assert!(report.text.contains("scripts/soft.asi"), "{}", report.text);
    assert_eq!(panel.state_of("soft"), Some(ModState::Ready));
    assert_eq!(
        panel.code_of("soft").unwrap().state,
        Some(NativeState::Installed)
    );
    for file in [
        "scripts/soft.asi",
        "scripts/doctor_platform.asi",
        "dinput8.dll",
    ] {
        assert!(root.join(file).is_file(), "{file}");
    }
    frame(&ctx, &mut panel, &[]);

    // Switching the mod off takes the code away at once.
    panel.switch(&ctx, "soft", false);
    panel.wait();
    assert!(!root.join("scripts").exists());
    assert!(!root.join("dinput8.dll").exists());

    // A mod that needs its code is not written before the answer.
    let hard = build_with_code(&game, temp.path(), "hard", true);
    panel.install_files(&ctx, vec![hard]);
    panel.wait();
    assert!(matches!(panel.dialog, Some(dialogs::Dialog::Code(_))));
    frame(&ctx, &mut panel, &[]);
    panel.dialog = None;
    assert_eq!(
        panel.state_of("hard"),
        Some(ModState::CodeWaiting { required: true })
    );
    panel.apply(&ctx);
    panel.wait();
    let Some(dialogs::Dialog::Report(report)) = panel.dialog.take() else {
        panic!("the refusal is told");
    };
    assert!(report.error, "{}", report.text);
    assert!(!root.join("scripts").exists());
}

/// Stage 103: the question before installing tells what the file holds,
/// and an installed mod is written back as a mod file.
#[test]
fn a_mod_file_is_described_before_installing_and_saved_as_a_file() {
    let (_dir, game) = game(false);
    let temp = tempfile::tempdir().unwrap();
    let a = build(&game, temp.path(), "a", LEVEL, MESH, vec![1; 300]);
    let info = doctor_toolkit::mods::file_info(&a).unwrap();
    assert_eq!(info.id, "a");
    assert_eq!(info.kind, doctor_toolkit::mods::FileKind::Mod);
    assert_eq!((info.resources, info.added), (1, 0));
    assert_eq!(info.streams, [LEVEL]);
    assert!(info.verified && info.payload_bytes == 300, "{info:?}");
    // A file that is not a mod is told about, and cannot be installed.
    let junk = temp.path().join("junk.zip");
    std::fs::write(&junk, "not a zip").unwrap();
    assert!(doctor_toolkit::mods::file_info(&junk).is_err());
    let ctx = egui::Context::default();
    let mut panel = panel(&game, &ctx);
    frame(&ctx, &mut panel, &[]);
    panel.offer_install(vec![a.clone(), junk]);
    let Some(dialogs::Dialog::Install(paths, infos)) = &panel.dialog else {
        panic!("the question is asked");
    };
    assert_eq!(paths.len(), 2);
    assert!(infos[0].as_ref().is_ok_and(|i| i.name == info.name));
    assert!(infos[1].is_err());
    frame(&ctx, &mut panel, &[]);
    frame(&ctx, &mut panel, &[]);
    // Installed, then saved as a file: the same mod.
    panel.dialog = None;
    panel.install_files(&ctx, vec![a]);
    panel.wait();
    frame(&ctx, &mut panel, &[]);
    let out = temp.path().join("copy.zip");
    panel.export_mod(&ctx, "a", out.clone());
    panel.wait();
    frame(&ctx, &mut panel, &[]);
    let (message, error) = panel.message().unwrap();
    assert!(!error && message.contains("copy.zip"), "{message}");
    let copy = doctor_toolkit::mods::file_info(&out).unwrap();
    assert_eq!((copy.id, copy.resources), (info.id, info.resources));
    // A mod that is not installed: an error, nothing written.
    let missing = temp.path().join("missing.zip");
    panel.export_mod(&ctx, "nothing", missing.clone());
    panel.wait();
    frame(&ctx, &mut panel, &[]);
    let (message, error) = panel.message().unwrap();
    assert!(error, "{message}");
    assert!(!missing.exists());
}

/// Stage 110: mods of format version 6 merge. Two mods that set one
/// attribute of one object by deltas are in conflict by that one element,
/// which the panel names; the lower one wins it. Written, the game has the
/// winner's value in the group of the game.
#[test]
fn mods_that_merge_tell_their_conflict_by_element() {
    use doctor_toolkit::resource::simgroup::{CLASS_ENTITY, SimGroup, Value};
    use doctor_toolkit::testing::{YARD, YARD_SIMGROUP};
    let dir = tempfile::tempdir().unwrap();
    testing::write_game(dir.path()).unwrap();
    testing::write_town(dir.path()).unwrap();
    let game = Arc::new(GameInstance::open(dir.path()).unwrap());
    let temp = tempfile::tempdir().unwrap();
    let yard = |game: &GameInstance| {
        let stream = game.open_stream(game.find_stream(YARD).unwrap()).unwrap();
        SimGroup::parse(&stream.resource_data(YARD_SIMGROUP).unwrap()).unwrap()
    };
    // A mod that lights the crate of the yard: one attribute, a delta.
    let lit = |id: &str, value: i32| {
        let mut group = yard(&game);
        group
            .set_class_attr(0, CLASS_ENTITY, 3, Some(Value::Int(value)))
            .unwrap();
        build(&game, temp.path(), id, YARD, YARD_SIMGROUP, group.encode())
    };
    let (a, b) = (lit("a", 5), lit("b", 7));
    let info = doctor_toolkit::mods::file_info(&a).unwrap();
    assert_eq!(
        (info.format_version, info.deltas, info.operations),
        (6, 1, 1)
    );

    let ctx = egui::Context::default();
    let mut panel = panel(&game, &ctx);
    panel.install_files(&ctx, vec![a, b]);
    panel.wait();
    assert_eq!(
        panel.state_of("a"),
        Some(ModState::Conflict(vec![state::Rival {
            name: "Mod b".into(),
            wins: true,
            resources: 1
        }]))
    );
    let conflict = &panel.overview.conflicts[0];
    assert_eq!(conflict.mods, ["a", "b"]);
    assert_eq!(conflict.elements.len(), 1, "{conflict:?}");
    assert!(
        conflict.elements[0].contains("Entity.CMD_SetInteriorLit"),
        "{conflict:?}"
    );
    let manager = panel.manager.as_ref().unwrap();
    assert_eq!(manager.get("a").unwrap().manifest.deltas().count(), 1);
    panel.apply(&ctx);
    panel.wait();
    assert!(panel.up_to_date());
    let group = yard(&game);
    assert_eq!(
        group.attr(&group.objects[0], CLASS_ENTITY, 3),
        Some(&Value::Int(7))
    );
}
