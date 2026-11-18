use super::super::manager::{ModApplyOptions, Problem};
use super::super::{
    ModInfo, ModManifest, NATIVE_VERSION, NativeBuild, Relations, build_from_workspace,
};
use super::*;
use crate::game::GameInstance;
use crate::workspace::Workspace;
use crate::workspace::tests::{LEVEL, MESH, edit, fake_game, snapshot, stream};

/// The smallest thing [`check_module`] takes for a 32-bit DLL.
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

fn info(id: &str, version: &str, code: NativeBuild) -> ModInfo {
    ModInfo {
        id: id.to_owned(),
        name: format!("Mod {id}"),
        version: version.to_owned(),
        author: "tester".to_owned(),
        description: String::new(),
        url: None,
        relations: Relations {
            native: Some(code),
            ..Relations::none()
        },
        whole_resources: false,
    }
}

fn code(modules: &[(&str, &str)], required: bool) -> NativeBuild {
    NativeBuild {
        modules: modules
            .iter()
            .map(|(file, tag)| ((*file).to_owned(), dll(tag)))
            .collect(),
        platform_abi: Some(PLATFORM_ABI),
        required,
    }
}

/// A mod that changes one mesh of the level and carries `code`.
fn build(game: &GameInstance, dir: &Path, info: &ModInfo) -> PathBuf {
    let ws_dir = dir.join(format!("ws-{}-{}", info.id, info.version));
    let mut ws = Workspace::create(&ws_dir, game).unwrap();
    ws.set_resources(
        game,
        "edit",
        stream(game, LEVEL),
        &[edit(MESH, vec![0x5A; 300])],
    )
    .unwrap();
    let out = dir.join(format!("{}-{}.zip", info.id, info.version));
    let report = build_from_workspace(&ws, info, None, None, &out).unwrap();
    assert_eq!(report.format_version, NATIVE_VERSION);
    out
}

/// The files of the toolkit, in `dir`.
fn host(dir: &Path) -> NativeHost {
    let platform = dir.join(PLATFORM_FILE);
    let loader = dir.join(LOADER_BINARY);
    fs::write(&platform, dll("platform")).unwrap();
    fs::write(&loader, dll("loader")).unwrap();
    NativeHost {
        edition: Ok(()),
        loader: None,
        platform_binary: Some(platform),
        loader_binary: Some(loader),
    }
}

fn open(game: &GameInstance, host: &NativeHost) -> ModManager {
    let mut mods = ModManager::open(game).unwrap();
    mods.set_native_host(host.clone());
    mods
}

fn state(mods: &ModManager, id: &str) -> NativeState {
    states(mods, mods.native_host())
        .unwrap()
        .into_iter()
        .find(|(i, _)| i == id)
        .map(|(_, s)| s)
        .expect("the mod has code and is enabled")
}

fn native_problem(mods: &ModManager, game: &GameInstance, id: &str) -> Option<Problem> {
    mods.check(game)
        .unwrap()
        .into_iter()
        .find(|i| i.mod_id == id && matches!(i.problem, Problem::Native { .. }))
        .map(|i| i.problem)
}

fn apply(mods: &mut ModManager, game: &GameInstance) -> crate::workspace::ApplyReport {
    mods.apply(game, ModApplyOptions::default()).unwrap()
}

#[test]
fn the_abi_is_that_of_the_sdk() {
    assert_eq!(PLATFORM_ABI, doctor_sdk::ABI_VERSION);
}

#[test]
fn module_names_and_dlls_are_checked() {
    for good in ["doctor_example.asi", "my-mod_2.asi", "a.asi"] {
        assert!(is_module_name(good), "{good}");
    }
    for bad in [
        "",
        ".asi",
        "mod.dll",
        "Mod.asi",
        "../mod.asi",
        "scripts/mod.asi",
        "a\\b.asi",
        "c:mod.asi",
        "mod.asi ",
        "mod..asi",
        "con.asi",
        "doctor_live.asi",
        "doctor_render.asi",
        "doctor_crash.asi",
        "doctor_platform.asi",
        "dinput8.dll",
        "-mod.asi",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.asi",
    ] {
        assert!(!is_module_name(bad), "{bad:?}");
    }
    assert!(check_module(&dll("x")).is_ok());
    assert!(check_module(b"MZ").is_err());
    assert!(check_module(b"#!/bin/sh\nrm -rf ~\n").is_err());
    let mut exe = dll("x");
    exe[0x56..0x58].copy_from_slice(&0x0102u16.to_le_bytes());
    assert!(check_module(&exe).unwrap_err().contains("not a DLL"));
    let mut x64 = dll("x");
    x64[0x44..0x46].copy_from_slice(&0x8664u16.to_le_bytes());
    assert!(
        check_module(&x64)
            .unwrap_err()
            .contains("another processor")
    );
    // A module that is no DLL does not get into a mod.
    let error = ModNative::of_modules(&[("a.asi".into(), b"text".to_vec())], None, false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("native module `a.asi`"), "{error}");
}

#[test]
fn the_section_is_validated_and_needs_format_version_5() {
    let (_dir, game) = fake_game();
    let temp = tempfile::tempdir().unwrap();
    let path = build(
        &game,
        temp.path(),
        &info("code", "1.0", code(&[("a.asi", "a")], true)),
    );
    let archive = super::super::ModArchive::open(&path).unwrap();
    let manifest = archive.manifest().clone();
    assert_eq!(manifest.format_version, 5);
    assert_eq!(manifest.required_version(), 5);
    let native = manifest.native.as_ref().unwrap();
    assert_eq!(native.edition, EDITION);
    assert_eq!(native.platform_abi, Some(PLATFORM_ABI));
    assert!(native.required);
    assert_eq!(native.modules[0].file, "a.asi");
    assert_eq!(native.modules[0].payload, Blob::of(&dll("a")));
    // The module travels as a payload and is checked like one.
    assert!(manifest.payloads().contains(&&native.modules[0].payload));

    let json = |m: &ModManifest| m.to_json().unwrap();
    let refused = |m: &ModManifest| ModManifest::parse(&json(m)).unwrap_err().to_string();

    // A mod manager of the formats 3 and 4 (0.0.5–0.0.7) would skip the
    // section: it must not open the mod at all.
    let old = ModManifest::parse_up_to(&json(&manifest), 4)
        .unwrap_err()
        .to_string();
    assert!(
        old.contains("format version 5") && old.contains("Update Open Mod Manager"),
        "{old}"
    );
    // And the section cannot hide in an older version.
    for version in 1..5 {
        let mut m = manifest.clone();
        m.format_version = version;
        let error = refused(&m);
        assert!(error.contains("version 5 is needed"), "{version}: {error}");
    }

    let with = |change: &dyn Fn(&mut ModNative)| {
        let mut m = manifest.clone();
        change(m.native.as_mut().unwrap());
        refused(&m)
    };
    assert!(with(&|n| n.edition = "1.0.764.0".into()).contains("only into the edition"));
    assert!(with(&|n| n.platform_abi = Some(0)).contains("platform_abi"));
    assert!(with(&|n| n.modules.clear()).contains("1 to 16 modules"));
    for name in [
        "../../godfather2.exe",
        "..\\dinput8.dll",
        "doctor_live.asi",
        "doctor_platform.asi",
        "sub/mod.asi",
        "MOD.ASI",
        "mod.exe",
    ] {
        let error = with(&|n| n.modules[0].file = name.into());
        assert!(error.contains("bad module name"), "{name}: {error}");
    }
    assert!(with(&|n| n.modules.push(n.modules[0].clone())).contains("listed twice"));
    assert!(with(&|n| n.modules[0].payload.sha256 = "zz".into()).contains("SHA-256"));
    assert!(with(&|n| n.modules[0].payload.size = 0).contains("bytes"));
    assert!(with(&|n| n.modules[0].payload.size = MAX_MODULE_SIZE + 1).contains("bytes"));

    // A mod of code alone is a mod.
    let mut alone = manifest.clone();
    alone.resources.clear();
    ModManifest::parse(&json(&alone)).unwrap();
}

#[test]
fn a_package_whose_module_is_not_a_dll_is_not_installed() {
    let (_dir, game) = fake_game();
    let temp = tempfile::tempdir().unwrap();
    let good = build(
        &game,
        temp.path(),
        &info("code", "1.0", code(&[("a.asi", "a")], false)),
    );
    let mut manifest = super::super::ModArchive::open(&good)
        .unwrap()
        .manifest()
        .clone();
    let script = b"@echo off\r\nformat c:\r\n".to_vec();
    manifest.native.as_mut().unwrap().modules[0].payload = Blob::of(&script);
    let bad = temp.path().join("bad.zip");
    let mesh = vec![0x5Au8; 300];
    super::super::package::write_package(&bad, &manifest, None, |blob| {
        Ok(if *blob == Blob::of(&script) {
            script.clone()
        } else {
            mesh.clone()
        })
    })
    .unwrap();
    let mut mods = open(&game, &host(temp.path()));
    let error = mods
        .install_file(&game, &bad, true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("native module `a.asi`"), "{error}");
    assert!(mods.mods().is_empty());
}

#[test]
fn code_waits_for_the_answer_and_leaves_with_the_mod() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let temp = tempfile::tempdir().unwrap();
    let host = host(temp.path());
    let before = snapshot(root);
    let path = build(
        &game,
        temp.path(),
        &info(
            "code",
            "1.0",
            code(&[("a.asi", "a"), ("b.asi", "b")], false),
        ),
    );
    let mut mods = open(&game, &host);
    let report = mods.install_file(&game, &path, true).unwrap();
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("executable code"))
    );
    let scripts = root.join(SCRIPTS_DIR);

    // No answer: the data is written, nothing executable is.
    assert_eq!(consent(&mods, "code").unwrap(), Consent::Unasked);
    let problem = native_problem(&mods, &game, "code").expect("a notice");
    assert!(problem.is_notice() && !problem.is_fatal());
    let applied = apply(&mut mods, &game);
    assert!(
        applied
            .warnings
            .iter()
            .any(|w| w.contains("waits for your consent"))
    );
    assert!(!scripts.exists());
    assert!(!root.join(LOADER_FILE).exists());
    assert_eq!(
        state(&mods, "code"),
        NativeState::Blocked(NativeBlock::Unasked)
    );
    assert!(
        snapshot(root) != before,
        "the data of the mod is in the game"
    );

    // The question names every file with its hash, and what the toolkit adds.
    let q = question(&mods, mods.native_host(), "code")
        .unwrap()
        .unwrap();
    assert_eq!(
        q.modules
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        ["scripts/a.asi", "scripts/b.asi"]
    );
    assert_eq!(q.modules[0].sha256, Blob::of(&dll("a")).sha256);
    assert_eq!(
        q.toolkit
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        ["scripts/doctor_platform.asi", "dinput8.dll"]
    );
    assert_eq!(q.obstacle, None);
    assert!(q.lines()[0].contains("EXECUTABLE CODE"));

    // "Data only": still nothing executable, and no more question.
    set_consent(&mods, "code", Some(Answer::Data)).unwrap();
    assert_eq!(consent(&mods, "code").unwrap(), Consent::DataOnly);
    apply(&mut mods, &game);
    assert!(!scripts.exists());
    assert_eq!(
        state(&mods, "code"),
        NativeState::Blocked(NativeBlock::Refused)
    );

    // "Yes": the modules, the platform and the loader.
    set_consent(&mods, "code", Some(Answer::Code)).unwrap();
    assert_eq!(state(&mods, "code"), NativeState::NotWritten);
    assert!(native_problem(&mods, &game, "code").is_none());
    let applied = apply(&mut mods, &game);
    for rel in [
        "scripts/a.asi",
        "scripts/b.asi",
        "scripts/doctor_platform.asi",
        "dinput8.dll",
    ] {
        assert!(
            applied.written.iter().any(|w| w == rel),
            "{rel}: {applied:?}"
        );
    }
    assert_eq!(fs::read(scripts.join("a.asi")).unwrap(), dll("a"));
    assert_eq!(fs::read(scripts.join("b.asi")).unwrap(), dll("b"));
    assert_eq!(
        fs::read(scripts.join(PLATFORM_FILE)).unwrap(),
        dll("platform")
    );
    assert_eq!(fs::read(root.join(LOADER_FILE)).unwrap(), dll("loader"));
    assert_eq!(state(&mods, "code"), NativeState::Installed);
    // Applying again writes nothing.
    let again = apply(&mut mods, &game);
    assert!(
        again.written.iter().all(|w| !w.contains("asi")),
        "{again:?}"
    );

    // Switching the mod off takes its code away at once — with the
    // platform, its log and the loader nobody needs any more.
    fs::write(scripts.join(doctor_toolkit_live::platform::LOG_FILE), "log").unwrap();
    mods.set_enabled("code", false).unwrap();
    assert!(!scripts.exists());
    assert!(!root.join(LOADER_FILE).exists());
    // The answer is remembered: switching on and applying brings it back.
    mods.set_enabled("code", true).unwrap();
    assert_eq!(state(&mods, "code"), NativeState::NotWritten);
    apply(&mut mods, &game);
    assert_eq!(state(&mods, "code"), NativeState::Installed);

    // "Restore originals": the game is as it was, byte for byte.
    mods.unapply(&game, false).unwrap();
    assert_eq!(
        mods.native_report().removed,
        [
            "scripts/a.asi",
            "scripts/b.asi",
            "scripts/doctor_platform.asi",
            "dinput8.dll"
        ]
    );
    assert!(snapshot(root) == before, "the game is as it was");
    assert_eq!(state(&mods, "code"), NativeState::NotWritten);

    // Uninstalling forgets the answer and leaves no record.
    assert!(mods.dir().join(RECORD_FILE).is_file());
    mods.uninstall("code").unwrap();
    assert!(!mods.dir().join(RECORD_FILE).exists());
}

#[test]
fn an_update_of_a_mod_is_not_asked_about_again() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let temp = tempfile::tempdir().unwrap();
    let host = host(temp.path());
    let mut mods = open(&game, &host);
    let build_version = |version: &str, tag: &str| {
        build(
            &game,
            temp.path(),
            &info("code", version, code(&[("a.asi", tag)], false)),
        )
    };
    mods.install_file(&game, &build_version("1.0", "one"), true)
        .unwrap();
    set_consent(&mods, "code", Some(Answer::Code)).unwrap();
    apply(&mut mods, &game);
    assert_eq!(state(&mods, "code"), NativeState::Installed);

    // The update carries another DLL: it is installed without a new
    // question — the answer is for the mod, not for a build of its modules.
    // Like the data of the mod, the code changes with the next write.
    mods.install_file(&game, &build_version("1.1", "two"), true)
        .unwrap();
    assert_eq!(consent(&mods, "code").unwrap(), Consent::Code);
    assert_eq!(fs::read(root.join("scripts/a.asi")).unwrap(), dll("one"));
    apply(&mut mods, &game);
    assert_eq!(fs::read(root.join("scripts/a.asi")).unwrap(), dll("two"));

    // The same DLL in a new version of the mod is not asked about again.
    mods.install_file(&game, &build_version("1.2", "two"), true)
        .unwrap();
    assert_eq!(consent(&mods, "code").unwrap(), Consent::Code);
    assert_eq!(state(&mods, "code"), NativeState::Installed);
}

#[test]
fn a_mod_that_needs_its_code_is_not_written_without_it() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let temp = tempfile::tempdir().unwrap();
    let mut host = host(temp.path());
    let before = snapshot(root);
    let path = build(
        &game,
        temp.path(),
        &info("code", "1.0", code(&[("a.asi", "a")], true)),
    );
    let mut mods = open(&game, &host);
    mods.install_file(&game, &path, true).unwrap();

    // No answer, or "data only": nothing of the mod is written.
    for answer in [None, Some(Answer::Data)] {
        set_consent(&mods, "code", answer).unwrap();
        assert!(native_problem(&mods, &game, "code").unwrap().is_fatal());
        let error = mods
            .apply(&game, ModApplyOptions::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not work without its code"), "{error}");
        // Not even when compatibility is ignored.
        let options = ModApplyOptions {
            ignore_compatibility: true,
            ..Default::default()
        };
        assert!(mods.apply(&game, options).is_err());
        assert!(snapshot(root) == before, "the game is as it was");
    }

    // Another edition of the game: agreed to, and still not written.
    set_consent(&mods, "code", Some(Answer::Code)).unwrap();
    host.edition = Err("not PC 1.0.789.0: link time 0x1".into());
    mods.set_native_host(host.clone());
    let Some(Problem::Native { block, required }) = native_problem(&mods, &game, "code") else {
        panic!("the edition is a problem");
    };
    assert!(required && matches!(block, NativeBlock::Edition(_)));
    assert!(mods.apply(&game, ModApplyOptions::default()).is_err());
    assert!(snapshot(root) == before, "the game is as it was");

    // A mod that can do without its code gets its data there.
    let soft = build(
        &game,
        temp.path(),
        &info("soft", "1.0", code(&[("s.asi", "s")], false)),
    );
    mods.set_enabled("code", false).unwrap();
    mods.install_file(&game, &soft, true).unwrap();
    set_consent(&mods, "soft", Some(Answer::Code)).unwrap();
    let applied = apply(&mut mods, &game);
    assert!(
        applied
            .warnings
            .iter()
            .any(|w| w.contains("only for the edition")),
        "{applied:?}"
    );
    assert!(!root.join(SCRIPTS_DIR).exists());
    assert!(
        snapshot(root) != before,
        "the data of the mod is in the game"
    );

    // A newer platform than this program has: refused whatever the mod says.
    let mut newer = code(&[("n.asi", "n")], false);
    newer.platform_abi = Some(PLATFORM_ABI + 1);
    let newer = build(&game, temp.path(), &info("newer", "1.0", newer));
    mods.install_file(&game, &newer, true).unwrap();
    let problem = native_problem(&mods, &game, "newer").unwrap();
    assert!(problem.is_fatal(), "{problem:?}");
    assert!(matches!(
        problem,
        Problem::Native {
            block: NativeBlock::Abi { .. },
            ..
        }
    ));
}

#[test]
fn files_of_others_are_left_alone() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let temp = tempfile::tempdir().unwrap();
    let mut host = host(temp.path());
    let scripts = root.join(SCRIPTS_DIR);
    fs::create_dir_all(&scripts).unwrap();
    // The player's own loader, a plug-in of his, a platform module an author
    // built himself, and a file with the name of a module of the mod in
    // other letters.
    fs::write(root.join("dinput8.dll"), b"the player's loader").unwrap();
    fs::write(scripts.join("GF2ASI.asi"), b"a plug-in").unwrap();
    fs::write(scripts.join(PLATFORM_FILE), b"an author's build").unwrap();
    fs::write(scripts.join("A.ASI"), b"somebody's").unwrap();
    host.loader = Some("dinput8.dll".into());
    let before = snapshot(root);

    let path = build(
        &game,
        temp.path(),
        &info(
            "code",
            "1.0",
            code(&[("a.asi", "a"), ("b.asi", "b")], false),
        ),
    );
    let mut mods = open(&game, &host);
    mods.install_file(&game, &path, true).unwrap();
    set_consent(&mods, "code", Some(Answer::Code)).unwrap();
    // `a.asi` is taken: none of the mod's code is installed.
    assert_eq!(
        state(&mods, "code"),
        NativeState::Blocked(NativeBlock::FileTaken {
            file: "a.asi".into()
        })
    );
    apply(&mut mods, &game);
    assert!(!scripts.join("b.asi").exists());

    // With the name free, the mod's modules go in; the loader and the
    // platform module of others are used as they are.
    fs::remove_file(scripts.join("A.ASI")).unwrap();
    let applied = apply(&mut mods, &game);
    assert_eq!(
        applied
            .written
            .iter()
            .filter(|w| w.contains("scripts/"))
            .collect::<Vec<_>>(),
        ["scripts/a.asi", "scripts/b.asi"]
    );
    assert_eq!(state(&mods, "code"), NativeState::Installed);
    assert_eq!(
        fs::read(root.join("dinput8.dll")).unwrap(),
        b"the player's loader"
    );
    assert_eq!(
        fs::read(scripts.join(PLATFORM_FILE)).unwrap(),
        b"an author's build"
    );

    // A module changed by another program is not the mod manager's any
    // more: it stays, and the report says so.
    fs::write(scripts.join("b.asi"), b"patched by a tool").unwrap();
    mods.unapply(&game, false).unwrap();
    let report = mods.native_report().clone();
    assert_eq!(report.removed, ["scripts/a.asi"]);
    assert!(
        report.warnings.iter().any(|w| w.contains("scripts/b.asi")),
        "{report:?}"
    );
    fs::remove_file(scripts.join("b.asi")).unwrap();
    fs::write(scripts.join("A.ASI"), b"somebody's").unwrap();
    assert!(snapshot(root) == before, "the game is as it was");
}

#[test]
fn the_loader_of_the_toolkit_stays_while_another_module_needs_it() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let temp = tempfile::tempdir().unwrap();
    let host = host(temp.path());
    let path = build(
        &game,
        temp.path(),
        &info("code", "1.0", code(&[("a.asi", "a")], false)),
    );
    let mut mods = open(&game, &host);
    mods.install_file(&game, &path, true).unwrap();
    set_consent(&mods, "code", Some(Answer::Code)).unwrap();
    apply(&mut mods, &game);
    assert!(root.join(LOADER_FILE).is_file());
    // The crash module came in the meantime and needs a loader too.
    let crash = root.join(SCRIPTS_DIR).join("doctor_crash.asi");
    fs::write(&crash, b"crash").unwrap();
    mods.unapply(&game, false).unwrap();
    assert!(root.join(LOADER_FILE).is_file());
    assert!(!root.join("scripts/a.asi").exists());
    // When it is gone, the loader goes with the next change.
    fs::remove_file(&crash).unwrap();
    mods.set_enabled("code", false).unwrap();
    assert!(!root.join(LOADER_FILE).exists());
    assert!(!root.join(SCRIPTS_DIR).exists());
}

#[test]
fn without_a_loader_or_the_platform_nothing_is_installed() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let temp = tempfile::tempdir().unwrap();
    let full = host(temp.path());
    let path = build(
        &game,
        temp.path(),
        &info("code", "1.0", code(&[("a.asi", "a")], false)),
    );
    let mut mods = open(&game, &full);
    mods.install_file(&game, &path, true).unwrap();
    set_consent(&mods, "code", Some(Answer::Code)).unwrap();

    mods.set_native_host(NativeHost {
        loader_binary: None,
        ..full.clone()
    });
    assert_eq!(
        state(&mods, "code"),
        NativeState::Blocked(NativeBlock::NoLoader)
    );
    let applied = apply(&mut mods, &game);
    assert!(
        applied.warnings.iter().any(|w| w.contains("no ASI loader")),
        "{applied:?}"
    );
    assert!(!root.join(SCRIPTS_DIR).exists());

    mods.set_native_host(NativeHost {
        platform_binary: None,
        ..full.clone()
    });
    assert_eq!(
        state(&mods, "code"),
        NativeState::Blocked(NativeBlock::NoPlatform)
    );
    apply(&mut mods, &game);
    assert!(!root.join(SCRIPTS_DIR).exists());
    assert!(!root.join(LOADER_FILE).exists());
}

#[test]
fn a_module_damaged_after_the_installation_is_not_written() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let temp = tempfile::tempdir().unwrap();
    let host = host(temp.path());
    let path = build(
        &game,
        temp.path(),
        &info("code", "1.0", code(&[("a.asi", "a")], false)),
    );
    let mut mods = open(&game, &host);
    mods.install_file(&game, &path, true).unwrap();
    set_consent(&mods, "code", Some(Answer::Code)).unwrap();
    // Somebody swaps the DLL in the folder of the installed mod.
    let blob = Blob::of(&dll("a"));
    let stored = mods.get("code").unwrap().dir().join(format!(
        "payloads/{}/{}",
        &blob.sha256[..2],
        blob.sha256
    ));
    assert!(stored.is_file());
    fs::write(&stored, dll("b")).unwrap();
    let applied = apply(&mut mods, &game);
    assert!(
        applied.warnings.iter().any(|w| w.contains("damaged")),
        "{applied:?}"
    );
    assert!(!root.join("scripts/a.asi").exists());
}

#[test]
fn two_mods_with_one_module_name() {
    let (dir, game) = fake_game();
    let root = dir.path();
    let temp = tempfile::tempdir().unwrap();
    let host = host(temp.path());
    let mut mods = open(&game, &host);
    for (id, tag) in [("one", "1"), ("two", "2")] {
        let path = build(
            &game,
            temp.path(),
            &info(id, "1.0", code(&[("same.asi", tag)], false)),
        );
        mods.install_file(&game, &path, true).unwrap();
        set_consent(&mods, id, Some(Answer::Code)).unwrap();
    }
    assert_eq!(state(&mods, "one"), NativeState::NotWritten);
    assert_eq!(
        state(&mods, "two"),
        NativeState::Blocked(NativeBlock::FileTaken {
            file: "same.asi".into()
        })
    );
    apply(&mut mods, &game);
    assert_eq!(fs::read(root.join("scripts/same.asi")).unwrap(), dll("1"));
}

/// The game holds its DLLs open while it runs (Windows refuses to delete
/// them): here the folder is read-only instead.
#[cfg(unix)]
#[test]
fn a_module_that_cannot_be_removed_now_goes_the_next_time() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, game) = fake_game();
    let root = dir.path();
    let temp = tempfile::tempdir().unwrap();
    let mut host = host(temp.path());
    // The player's own loader: only `scripts/` is the mod manager's business.
    fs::write(root.join("dinput8.dll"), b"the player's loader").unwrap();
    host.loader = Some("dinput8.dll".into());
    let path = build(
        &game,
        temp.path(),
        &info("code", "1.0", code(&[("a.asi", "a")], false)),
    );
    let mut mods = open(&game, &host);
    mods.install_file(&game, &path, true).unwrap();
    set_consent(&mods, "code", Some(Answer::Code)).unwrap();
    apply(&mut mods, &game);
    let scripts = root.join(SCRIPTS_DIR);
    let lock = |mode: u32| {
        fs::set_permissions(&scripts, fs::Permissions::from_mode(mode)).unwrap();
    };
    lock(0o555);
    if fs::write(scripts.join("probe"), b"").is_ok() {
        // Running as root: nothing is refused, nothing to check.
        lock(0o755);
        return;
    }
    mods.set_enabled("code", false).unwrap();
    let report = prune(&mods).unwrap();
    assert!(report.removed.is_empty());
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("could not be removed")),
        "{report:?}"
    );
    // Still there, and still known as the mod manager's.
    assert!(scripts.join("a.asi").is_file());
    assert!(
        written(&mods)
            .unwrap()
            .iter()
            .any(|(rel, _)| rel == "scripts/a.asi")
    );
    lock(0o755);
    let report = prune(&mods).unwrap();
    assert_eq!(
        report.removed,
        ["scripts/a.asi", "scripts/doctor_platform.asi"]
    );
    assert!(!scripts.exists());
}
