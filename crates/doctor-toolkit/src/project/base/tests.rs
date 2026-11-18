//! The base of a project on the miniature game: the format of
//! `project.toml` with and without it, recording, every kind of difference,
//! bringing the mods to the base and back, the dependencies of the built mod.

use std::path::{Path, PathBuf};

use super::*;
use crate::mods::manager::ModInGame;
use crate::mods::{ModInfo, Relations, build_from_workspace};
use crate::project::config::{BASE_FORMAT, BuildSettings, Dependencies, GameInfo, ProjectInfo};
use crate::project::{NewDocument, PROJECT_FILE, Project, create_document};
use crate::resource::lh2::StringTable;
use crate::testing::{self, LEVEL, STRINGS, TEXT};
use crate::workspace::Workspace;

/// The string table of the text stream and the one of the level: two
/// resources for mods that do not meet.
const IN_TEXT: (&str, usize) = (TEXT, 0);
const IN_LEVEL: (&str, usize) = (LEVEL, STRINGS);

struct World {
    dir: tempfile::TempDir,
    packages: std::cell::Cell<usize>,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("game");
    std::fs::create_dir_all(&game).unwrap();
    testing::write_game(&game).unwrap();
    World {
        dir,
        packages: std::cell::Cell::new(0),
    }
}

fn set_word(game: &GameInstance, ws: &mut Workspace, place: (&str, usize), word: &str) {
    let stream = game.find_stream(place.0).unwrap();
    let mut table = StringTable::parse(&ws.current_data(game, stream, place.1).unwrap()).unwrap();
    table.strings[0][0] = word.into();
    ws.set_string_table(game, stream, place.1, &table).unwrap();
}

impl World {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// The game as its folder has it now.
    fn game(&self) -> GameInstance {
        GameInstance::open(self.path("game")).unwrap()
    }

    fn mods(&self) -> ModManager {
        ModManager::open(&self.game()).unwrap()
    }

    /// The first word of the string table at `place`, in the game files.
    fn word(&self, place: (&str, usize)) -> String {
        let game = self.game();
        let stream = game.find_stream(place.0).unwrap();
        let file = crate::game::StreamFile::open(&game.streams()[stream].path).unwrap();
        StringTable::parse(&file.resource_data(place.1).unwrap())
            .unwrap()
            .strings[0][0]
            .clone()
    }

    /// A mod file `id` of `version` that sets the first word of the string
    /// table at `place`. A version that is no version number gives a mod
    /// of the format that allows one.
    fn package(&self, id: &str, version: &str, place: (&str, usize), word: &str) -> PathBuf {
        let n = self.packages.get();
        self.packages.set(n + 1);
        let game = self.game();
        let mut ws = Workspace::for_game(&game, Some(&self.path(&format!("ws-{n}")))).unwrap();
        set_word(&game, &mut ws, place, word);
        let out = self.path(&format!("{id}-{n}.zip"));
        let relations = match crate::mods::version::parse_version(version) {
            Ok(_) => Relations::default(),
            Err(_) => Relations::none(),
        };
        let info = ModInfo {
            id: id.into(),
            name: format!("Mod {id}"),
            version: version.into(),
            relations,
            ..Default::default()
        };
        build_from_workspace(&ws, &info, None, None, &out).unwrap();
        out
    }

    /// Installs (or updates) the mod of [`Self::package`].
    fn install(&self, id: &str, version: &str, place: (&str, usize), word: &str) {
        let package = self.package(id, version, place, word);
        self.mods()
            .install_file(&self.game(), &package, true)
            .unwrap();
    }

    fn apply(&self) {
        self.mods()
            .apply(&self.game(), ModApplyOptions::default())
            .unwrap();
    }

    fn project(&self) -> Project {
        let new = NewDocument {
            name: "Yard Mod".into(),
            ..Default::default()
        };
        create_document(&self.game(), &self.path("Yard Mod"), &new).unwrap()
    }

    fn in_game(&self) -> GameMods {
        self.mods().in_game(&self.game()).unwrap()
    }

    /// The differences of the project in `root` now.
    fn differences(&self, root: &Path) -> Vec<BaseDifference> {
        ProjectConfig::read(root)
            .unwrap()
            .check_base(&self.game())
            .unwrap()
    }

    fn arrangement(&self) -> Vec<(String, bool)> {
        self.mods()
            .arrangement()
            .into_iter()
            .map(|e| (e.id, e.enabled))
            .collect()
    }

    fn set_names(&self) -> Vec<String> {
        self.mods()
            .sets()
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect()
    }
}

fn pairs(list: &[(&str, bool)]) -> Vec<(String, bool)> {
    list.iter()
        .map(|(id, on)| ((*id).to_owned(), *on))
        .collect()
}

fn base_mod(id: &str, version: &str) -> BaseMod {
    BaseMod {
        id: id.into(),
        version: version.into(),
        built_on: String::new(),
        sha256: String::new(),
    }
}

/// `project.toml` as the program wrote it before stage 108.
#[derive(Serialize)]
struct ConfigBefore {
    format: u32,
    project: ProjectInfo,
    game: GameInfo,
    dependencies: Dependencies,
    build: BuildSettings,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    zones: Vec<crate::level::ZoneSet>,
}

#[test]
fn a_project_without_a_base_is_written_as_before() {
    let w = world();
    let project = w.project();
    let file = project.root().join(PROJECT_FILE);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("format = 1\n"), "{text}");
    assert!(!text.contains("base"), "{text}");
    let config = &project.config;
    assert_eq!(config.base, None);
    assert_eq!(config.required_format(), 1);
    // Byte for byte what the structure without the base gave.
    let mut before = ConfigBefore {
        format: 1,
        project: config.project.clone(),
        game: config.game.clone(),
        dependencies: config.dependencies.clone(),
        build: config.build.clone(),
        zones: config.zones.clone(),
    };
    assert_eq!(text, toml::to_string_pretty(&before).unwrap());
    // Also with the mods of stage 36 and a set of zones, read and written again.
    let mut config = config.clone();
    config.dependencies.mods = vec!["other-mod".into()];
    config.zones = vec![crate::level::ZoneSet {
        name: "yard".into(),
        streams: vec!["gf2/level".into()],
    }];
    config.write(project.root()).unwrap();
    before.dependencies = config.dependencies.clone();
    before.zones = config.zones.clone();
    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(text, toml::to_string_pretty(&before).unwrap());
    let read = ProjectConfig::read(project.root()).unwrap();
    assert_eq!(read, config);
    read.write(project.root()).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    // A program that reads up to format 1 reads it.
    ProjectConfig::read_up_to(project.root(), 1).unwrap();
}

#[test]
fn a_base_makes_the_project_format_2_which_an_older_program_refuses() {
    let w = world();
    let project = w.project();
    let root = project.root();
    let file = root.join(PROJECT_FILE);
    // The author's own lines stay where they are.
    let text = std::fs::read_to_string(&file)
        .unwrap()
        .replace("[build]", "# where the edits live\n[build]");
    std::fs::write(&file, &text).unwrap();
    let sha = "ab".repeat(32);
    let base = Base {
        exclusive: true,
        mods: vec![
            BaseMod {
                id: "new-new-york".into(),
                version: "^1.1.0".into(),
                built_on: "1.1.0".into(),
                sha256: sha.clone(),
            },
            base_mod("hd-textures", "*"),
        ],
    };
    ProjectConfig::write_base(root, Some(&base)).unwrap();
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.starts_with("format = 2\n"), "{written}");
    assert!(written.contains("# where the edits live\n[build]"));
    let tables = format!(
        "\n[base]\nexclusive = true\n\n[[base.mod]]\nid = \"new-new-york\"\nversion = \
         \"^1.1.0\"\nbuilt_on = \"1.1.0\"\nsha256 = \"{sha}\"\n\n[[base.mod]]\nid = \
         \"hd-textures\"\nversion = \"*\"\n"
    );
    assert!(written.ends_with(&tables), "{written}");
    assert_eq!(
        written
            .replace(&tables, "")
            .replace("format = 2", "format = 1"),
        text
    );
    let read = ProjectConfig::read(root).unwrap();
    assert_eq!(read.format, BASE_FORMAT);
    assert_eq!(read.base.as_ref(), Some(&base));
    // A program of before this stage would skip the table: it refuses the file.
    let error = ProjectConfig::read_up_to(root, 1).unwrap_err().to_string();
    assert!(
        error.contains("has format 2; this tool reads up to 1"),
        "{error}"
    );
    // Written whole, it is the same project; the sets of zones go after
    // the base and leave it alone.
    read.write(root).unwrap();
    assert_eq!(ProjectConfig::read(root).unwrap(), read);
    std::fs::write(&file, &written).unwrap();
    let zones = vec![crate::level::ZoneSet {
        name: "yard".into(),
        streams: vec!["gf2/level".into()],
    }];
    ProjectConfig::write_zones(root, &zones).unwrap();
    let with_zones = std::fs::read_to_string(&file).unwrap();
    assert!(with_zones.starts_with(written.trim_end()), "{with_zones}");
    let read = ProjectConfig::read(root).unwrap();
    assert_eq!((read.base.as_ref(), &read.zones), (Some(&base), &zones));
    // Another base replaces the tables; an empty one is still a base (the
    // project is made on the game without mods).
    let empty = Base {
        exclusive: true,
        mods: Vec::new(),
    };
    ProjectConfig::write_base(root, Some(&empty)).unwrap();
    let written = std::fs::read_to_string(&file).unwrap();
    assert_eq!(written.matches("[base]").count(), 1, "{written}");
    assert!(!written.contains("base.mod") && written.contains("[[zones]]"));
    assert_eq!(ProjectConfig::read(root).unwrap().base, Some(empty));
    // Without a base the file is of format 1 again, as its author has it.
    ProjectConfig::write_base(root, None).unwrap();
    ProjectConfig::write_zones(root, &[]).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);

    // A base that cannot be: the project itself, a mod twice, a condition
    // or a hash that is none.
    for (bad, why) in [
        (base_mod("yard-mod", "*"), "its own base"),
        (base_mod("Bad Id", "*"), "not a mod id"),
        (base_mod("a", ">=1,<2"), "not a version condition"),
        (
            BaseMod {
                sha256: "xyz".into(),
                ..base_mod("a", "*")
            },
            "not a SHA-256",
        ),
    ] {
        let base = Base {
            exclusive: false,
            mods: vec![bad],
        };
        let error = ProjectConfig::write_base(root, Some(&base))
            .unwrap_err()
            .to_string();
        assert!(error.contains(why), "{error}");
    }
    let twice = Base {
        exclusive: false,
        mods: vec![base_mod("a", "*"), base_mod("a", "^1")],
    };
    assert!(ProjectConfig::write_base(root, Some(&twice)).is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    // Written by hand, it is refused when the project is opened.
    std::fs::write(
        &file,
        text.clone() + "\n[base]\n[[base.mod]]\nid = \"a\"\nversion = \"=>1\"\n",
    )
    .unwrap();
    let error = Project::open(root).unwrap_err().to_string();
    assert!(
        error.contains("project.toml") && error.contains("[base]: mod `a`"),
        "{error}"
    );
    // A base by hand needs no more than the ids.
    std::fs::write(&file, text + "\n[base]\n[[base.mod]]\nid = \"a\"\n").unwrap();
    let base = Project::open(root).unwrap().config.base.unwrap();
    assert_eq!(
        (base.exclusive, &base.mods),
        (false, &vec![base_mod("a", "*")])
    );
}

#[test]
fn the_condition_of_a_recorded_mod_is_within_its_major_version() {
    for (version, condition) in [
        ("1.1.0", "^1.1.0"),
        ("1.1", "^1.1"),
        ("2", "^2"),
        ("0.3.1", "^0.3.1"),
        ("1.2.0-beta", "^1.2.0-beta"),
        (" 1.4 ", "^1.4"),
        ("beta two", "*"),
        ("v1", "*"),
        ("", "*"),
    ] {
        assert_eq!(default_condition(version), condition, "{version:?}");
    }
    let fits =
        |condition: &str, version: &str| Condition::parse(condition).unwrap().matches_text(version);
    assert!(fits(&default_condition("1.1.0"), "1.9.3"));
    assert!(!fits(&default_condition("1.1.0"), "2.0.0"));
    assert!(!fits(&default_condition("1.1.0"), "1.0.9"));
}

#[test]
fn the_base_is_recorded_from_the_mods_written_into_the_game() {
    let w = world();
    let project = w.project();
    let root = project.root();
    // No mods: nothing to compare with, and nothing recorded by itself.
    assert!(w.in_game().mods.is_empty());
    assert!(w.differences(root).is_empty());
    assert_eq!(ProjectConfig::read(root).unwrap().base, None);

    w.install("city", "1.1.0", IN_TEXT, "City");
    w.install("lights", "beta two", IN_LEVEL, "Lights");
    w.install("off", "1.0", IN_LEVEL, "Off");
    w.mods().set_enabled("off", false).unwrap();
    w.apply();
    // The project itself, installed as a mod and written: it wins the
    // resource of `lights`.
    let game = w.game();
    let mut ws = project.workspace(&game).unwrap();
    set_word(&game, &mut ws, IN_LEVEL, "Mine");
    project
        .install_into(&game, &mut w.mods(), &ws, Default::default())
        .unwrap();
    w.apply();
    // Installed and enabled after the mods were written: not in the game,
    // although the stream it waits for holds `city`.
    w.install("late", "3.0", IN_TEXT, "Late");

    let in_game = w.in_game();
    let state: Vec<(&str, bool, bool)> = in_game
        .mods
        .iter()
        .map(|m| (m.id.as_str(), m.enabled, m.written))
        .collect();
    assert_eq!(
        state,
        [
            ("city", true, true),
            ("lights", true, true),
            ("off", false, false),
            ("yard-mod", true, true),
            ("late", true, false),
        ]
    );
    assert_eq!(in_game.leftover, 0);
    assert_eq!(
        (w.word(IN_TEXT), w.word(IN_LEVEL)),
        ("City".into(), "Mine".into())
    );

    let recorded = ProjectConfig::record_base(root, &w.game()).unwrap();
    assert_eq!(recorded.not_written, ["late"]);
    let mods = w.mods();
    let city = in_game.get("city").unwrap();
    assert_eq!(city.sha256, mods.get("city").unwrap().manifest.sha256());
    assert_eq!(city.sha256.len(), 64);
    assert_eq!(
        recorded.base,
        Base {
            exclusive: true,
            mods: vec![
                BaseMod {
                    id: "city".into(),
                    version: "^1.1.0".into(),
                    built_on: "1.1.0".into(),
                    sha256: city.sha256.clone(),
                },
                BaseMod {
                    id: "lights".into(),
                    version: "*".into(),
                    built_on: "beta two".into(),
                    sha256: in_game.get("lights").unwrap().sha256.clone(),
                },
            ],
        },
        "the load order, without the mod of the project"
    );
    let config = ProjectConfig::read(root).unwrap();
    assert_eq!(config.format, BASE_FORMAT);
    assert_eq!(config.base.as_ref(), Some(&recorded.base));

    // Recorded again: the condition its author wrote stays while the
    // version in the game satisfies it, and so does `exclusive`; a mod that
    // is enabled but not written is left out and named.
    let mut base = recorded.base.clone();
    base.exclusive = false;
    base.mods[0].version = ">=1.1 <3".into();
    ProjectConfig::write_base(root, Some(&base)).unwrap();
    w.install("city", "2.0.0", IN_TEXT, "City two");
    let again = Base::record(&w.in_game(), "yard-mod", Some(&base));
    assert_eq!(again.not_written, ["city", "late"]);
    assert!(!again.base.exclusive);
    assert_eq!(again.base.mods.len(), 1, "{:?}", again.base);
    w.mods().set_enabled("late", false).unwrap();
    w.apply();
    let again = ProjectConfig::record_base(root, &w.game()).unwrap();
    assert!(again.not_written.is_empty());
    let city = &again.base.mods[0];
    assert_eq!(
        (city.version.as_str(), city.built_on.as_str()),
        (">=1.1 <3", "2.0.0")
    );
    assert_ne!(city.sha256, recorded.base.mods[0].sha256);
    assert_eq!(again.base.mods[1], recorded.base.mods[1]);
    // A version the condition does not allow gets the default one.
    w.install("city", "3.0.0", IN_TEXT, "City three");
    w.apply();
    let again = ProjectConfig::record_base(root, &w.game()).unwrap();
    assert_eq!(again.base.mods[0].version, "^3.0.0");
}

#[test]
fn every_difference_between_the_base_and_the_game_is_told() {
    use BaseDifference as D;
    let w = world();
    let project = w.project();
    let root = project.root();
    w.install("city", "1.1.0", IN_TEXT, "City");
    w.install("lights", "1.0", IN_LEVEL, "Lights");
    w.apply();
    let recorded = ProjectConfig::record_base(root, &w.game()).unwrap();
    assert_eq!(recorded.base.mods.len(), 2);
    assert!(w.differences(root).is_empty());
    let id = |id: &str| id.to_owned();

    // Switched off: its data is still in the files of the game.
    w.mods().set_enabled("city", false).unwrap();
    assert_eq!(
        w.differences(root),
        [D::Disabled { id: id("city") }, D::Leftover { resources: 1 }]
    );
    // Enabled again without a write since: the game has it as it was.
    w.mods().set_enabled("city", true).unwrap();
    assert!(w.differences(root).is_empty());

    // The project's own mod: installed, it is replaced by the project as it
    // is for every launch; written, it is in what the editor reads.
    let game = w.game();
    let mut ws = project.workspace(&game).unwrap();
    set_word(&game, &mut ws, IN_TEXT, "Mine");
    project
        .install_into(&game, &mut w.mods(), &ws, Default::default())
        .unwrap();
    assert!(w.differences(root).is_empty());
    w.apply();
    let own = D::OwnMod { id: id("yard-mod") };
    assert_eq!(w.differences(root), std::slice::from_ref(&own));
    assert!(own.brought() && !own.blocks_the_mod());
    // Removed, its data is still where the data of the base should be.
    w.mods().uninstall("yard-mod").unwrap();
    assert_eq!(w.differences(root), [D::NotWritten { id: id("city") }]);
    w.apply();
    assert!(w.differences(root).is_empty());

    // The originals are back: enabled, not written.
    w.mods().unapply(&w.game(), false).unwrap();
    assert_eq!(
        w.differences(root),
        [
            D::NotWritten { id: id("city") },
            D::NotWritten { id: id("lights") }
        ]
    );
    w.apply();

    // Another order.
    w.mods().move_mod("lights", 0).unwrap();
    assert_eq!(
        w.differences(root),
        [D::Order {
            base: vec![id("city"), id("lights")],
            game: vec![id("lights"), id("city")],
        }]
    );
    w.mods().move_mod("lights", 1).unwrap();

    // The same version built anew, then a newer one the condition allows:
    // the base changed since the project was made on it.
    w.install("city", "1.1.0", IN_TEXT, "City again");
    assert_eq!(
        w.differences(root),
        [
            D::NotWritten { id: id("city") },
            D::Changed {
                id: id("city"),
                built_on: id("1.1.0"),
                found: id("1.1.0"),
            }
        ]
    );
    w.install("city", "1.4.0", IN_TEXT, "City 1.4");
    w.apply();
    let changed = D::Changed {
        id: id("city"),
        built_on: id("1.1.0"),
        found: id("1.4.0"),
    };
    assert_eq!(w.differences(root), std::slice::from_ref(&changed));
    assert!(!changed.brought() && !changed.blocks_the_mod());
    assert!(
        changed
            .describe()
            .contains("changed since the project was made on it"),
        "{}",
        changed.describe()
    );
    // A version outside the condition.
    w.install("city", "2.0.0", IN_TEXT, "City 2");
    w.apply();
    let version = D::Version {
        id: id("city"),
        wanted: id("^1.1.0"),
        found: id("2.0.0"),
    };
    assert_eq!(w.differences(root), std::slice::from_ref(&version));
    assert!(!version.brought() && version.blocks_the_mod());

    // Not installed.
    w.mods().uninstall("lights").unwrap();
    assert_eq!(
        w.differences(root),
        [
            version.clone(),
            D::NotInstalled { id: id("lights") },
            D::Leftover { resources: 1 }
        ]
    );
    w.apply();

    // A mod that is not of the base: enabled, then written.
    w.install("extra", "1.0", IN_LEVEL, "Extra");
    let extra = |written| D::Extra {
        id: id("extra"),
        written,
    };
    let lights_gone = D::NotInstalled { id: id("lights") };
    assert_eq!(
        w.differences(root),
        [version.clone(), lights_gone.clone(), extra(false)]
    );
    w.apply();
    assert_eq!(
        w.differences(root),
        [version.clone(), lights_gone.clone(), extra(true)]
    );
    assert!(extra(true).brought() && !extra(true).blocks_the_mod());
    assert!(lights_gone.blocks_the_mod() && !lights_gone.brought());
    // A base that is not exclusive lets other mods be.
    let mut base = recorded.base.clone();
    base.exclusive = false;
    ProjectConfig::write_base(root, Some(&base)).unwrap();
    assert_eq!(w.differences(root), [version, lights_gone]);

    // The mods of stage 36 are a base of which only "installed and
    // enabled" is asked; a mod the base names is told once.
    let mut config = ProjectConfig::read(root).unwrap();
    config.base = None;
    config.dependencies.mods = vec![id("extra"), id("gone"), id("city")];
    config.write(root).unwrap();
    assert_eq!(ProjectConfig::read(root).unwrap().format, 1);
    assert_eq!(w.differences(root), [D::NotInstalled { id: id("gone") }]);
    w.mods().set_enabled("extra", false).unwrap();
    assert_eq!(
        w.differences(root),
        [
            D::Disabled { id: id("extra") },
            D::NotInstalled { id: id("gone") }
        ]
    );
    let warnings = Project::open(root).unwrap().game_warnings(&w.game());
    assert_eq!(
        warnings,
        [
            "the base mod `extra` is switched off",
            "the base mod `gone` is not installed"
        ]
    );
    config.base = Some(Base {
        exclusive: false,
        mods: vec![base_mod("extra", "*")],
    });
    assert_eq!(
        config.base_differences(&w.in_game()),
        [
            D::Disabled { id: id("extra") },
            D::NotInstalled { id: id("gone") }
        ]
    );
    assert_eq!(
        config.mod_dependencies().into_iter().collect::<Vec<_>>(),
        [
            (id("city"), id("*")),
            (id("extra"), id("*")),
            (id("gone"), id("*"))
        ]
    );
}

/// The differences are a matter of the list of mods alone.
#[test]
fn differences_of_a_base_follow_the_mods() {
    let m = |id: &str, version: &str, enabled: bool, written: bool| ModInGame {
        id: id.into(),
        name: id.into(),
        version: version.into(),
        sha256: "00".repeat(32),
        enabled,
        damaged: false,
        written,
    };
    let base = Base {
        exclusive: true,
        mods: vec![base_mod("a", "*"), base_mod("b", "^1"), base_mod("c", "*")],
    };
    let mut game = GameMods {
        mods: vec![
            m("x", "1", false, false),
            m("c", "1", true, true),
            m("a", "1", true, true),
            m("b", "1.5", false, false),
        ],
        leftover: 0,
    };
    // A mod that is off is not part of the order; one that is off and not
    // of the base is no difference.
    assert_eq!(
        base.differences(&game, "me"),
        [
            BaseDifference::Disabled { id: "b".into() },
            BaseDifference::Order {
                base: vec!["a".into(), "c".into()],
                game: vec!["c".into(), "a".into()],
            }
        ]
    );
    // A damaged mod is off, and nothing more is known of it.
    game.mods[3].damaged = true;
    game.mods[3].version = "?".into();
    assert_eq!(base.differences(&game, "me").len(), 2);

    // The arrangement with the base in it.
    let now: Vec<SetEntry> = game
        .mods
        .iter()
        .map(|m| SetEntry {
            id: m.id.clone(),
            enabled: m.enabled,
        })
        .collect();
    let ids = |list: Vec<SetEntry>| -> Vec<(String, bool)> {
        list.into_iter().map(|e| (e.id, e.enabled)).collect()
    };
    assert_eq!(
        ids(base.arrangement(&now)),
        pairs(&[("a", true), ("b", true), ("c", true), ("x", false)])
    );
    // Not exclusive: the other mods stay as and where they are.
    let mut open = base.clone();
    open.exclusive = false;
    let mut with_x = now.clone();
    with_x[0].enabled = true;
    with_x.insert(
        2,
        SetEntry {
            id: "y".into(),
            enabled: false,
        },
    );
    assert_eq!(
        ids(open.arrangement(&with_x)),
        pairs(&[
            ("x", true),
            ("a", true),
            ("y", false),
            ("b", true),
            ("c", true)
        ])
    );
    // A mod of the base that is not installed has no place.
    assert_eq!(
        ids(open.arrangement(&with_x[..3])),
        pairs(&[("x", true), ("c", true), ("y", false)])
    );
}

#[test]
fn the_mods_are_brought_to_the_base_and_back() {
    let w = world();
    let project = w.project();
    let root = project.root();
    w.install("city", "1.1.0", IN_TEXT, "City");
    w.install("lights", "1.0", IN_LEVEL, "Lights");
    w.apply();
    let base = ProjectConfig::record_base(root, &w.game()).unwrap().base;
    let bring = |w: &World| {
        base.bring(
            &w.game(),
            &mut w.mods(),
            "Yard Mod",
            ModApplyOptions::default(),
            &mut |_| {},
        )
    };

    // The game holds the base: nothing to do, no set saved.
    let brought = bring(&w).unwrap();
    assert_eq!((brought.set, brought.saved), (None, false));
    assert!(brought.report.written.is_empty());
    assert!(w.set_names().is_empty());

    // The player's own arrangement: another mod on top, a base mod off.
    w.install("extra", "1.0", IN_TEXT, "Extra");
    w.mods().set_enabled("lights", false).unwrap();
    w.apply();
    let mine = pairs(&[("city", true), ("lights", false), ("extra", true)]);
    assert_eq!(w.arrangement(), mine);
    assert_eq!(
        (w.word(IN_TEXT), w.word(IN_LEVEL)),
        ("Extra".into(), "Hello".into())
    );
    let streams = |w: &World| {
        let game = w.game();
        [TEXT, LEVEL].map(|name| {
            std::fs::read(&game.streams()[game.find_stream(name).unwrap()].path).unwrap()
        })
    };
    let before = streams(&w);
    assert_eq!(w.differences(root).len(), 2);

    let mut progress = Vec::new();
    let brought = base
        .bring(
            &w.game(),
            &mut w.mods(),
            "Yard Mod",
            ModApplyOptions::default(),
            &mut |p| progress.push(p.stream.to_owned()),
        )
        .unwrap();
    assert_eq!(
        (brought.set.as_deref(), brought.saved),
        (Some("Before Yard Mod"), true)
    );
    assert!(!progress.is_empty());
    assert_eq!(
        w.arrangement(),
        pairs(&[("city", true), ("lights", true), ("extra", false)])
    );
    assert_eq!(
        (w.word(IN_TEXT), w.word(IN_LEVEL)),
        ("City".into(), "Lights".into())
    );
    assert!(w.differences(root).is_empty());
    assert_eq!(w.set_names(), ["Before Yard Mod"]);

    // Back: the saved set, written.
    assert!(w.mods().use_set("Before Yard Mod").unwrap().is_empty());
    w.apply();
    assert_eq!(w.arrangement(), mine);
    assert!(streams(&w) == before, "the game is not as it was before");

    // Brought again from a saved set: that set is the way back, no new one.
    let brought = bring(&w).unwrap();
    assert_eq!(
        (brought.set.as_deref(), brought.saved),
        (Some("Before Yard Mod"), false)
    );
    assert_eq!(w.set_names(), ["Before Yard Mod"]);
    // From an arrangement that is no saved set: a set of another name.
    w.mods().set_enabled("extra", true).unwrap();
    let brought = bring(&w).unwrap();
    assert_eq!(brought.set.as_deref(), Some("Before Yard Mod (2)"));
    assert_eq!(w.set_names(), ["Before Yard Mod", "Before Yard Mod (2)"]);
    assert!(w.differences(root).is_empty());

    // The write is refused (the workspace of the game is in its files):
    // the arrangement stays, and the set saved for the way back goes.
    w.mods().unapply(&w.game(), false).unwrap();
    w.mods().set_enabled("extra", true).unwrap();
    let game = w.game();
    let mut ws = Workspace::for_game(&game, None).unwrap();
    set_word(&game, &mut ws, IN_LEVEL, "Workspace");
    ws.apply(&game, Default::default()).unwrap();
    let arrangement = w.arrangement();
    let error = bring(&w).unwrap_err().to_string();
    assert!(error.contains("workspace"), "{error}");
    assert_eq!(w.arrangement(), arrangement);
    assert_eq!(w.set_names().len(), 2);
    ws.unapply(&game, false).unwrap();

    // A mod of the base that is not installed cannot be brought: it is
    // named, and nothing changes.
    w.mods().uninstall("lights").unwrap();
    let arrangement = w.arrangement();
    let error = bring(&w).unwrap_err().to_string();
    assert!(
        error.contains("`lights` 1.0 is not installed") && error.contains("install it first"),
        "{error}"
    );
    assert_eq!(w.arrangement(), arrangement);
    assert_eq!(w.set_names().len(), 2);

    // Not exclusive: the other mods stay on.
    let mut open = base.clone();
    open.exclusive = false;
    open.mods.pop();
    w.mods().set_enabled("city", false).unwrap();
    open.bring(
        &w.game(),
        &mut w.mods(),
        "Yard Mod",
        ModApplyOptions::default(),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(w.arrangement(), pairs(&[("city", true), ("extra", true)]));
    assert_eq!(w.word(IN_TEXT), "Extra");
}

/// The mod of a project depends on its base — with the conditions written
/// there, also on a mod that is enabled but not written into the game (the
/// guess by what is written misses it) — and on nothing else; the data of
/// the base it replaces is its original, so that the mod manager takes it
/// as a patch on top of the base.
#[test]
fn the_mod_of_a_project_depends_on_its_base() {
    let w = world();
    let project = w.project();
    let root = project.root().to_owned();
    // The base is installed and enabled, and not written.
    w.install("city", "1.1.0", IN_TEXT, "City");
    // Another mod is written into the game.
    w.mods().set_enabled("city", false).unwrap();
    w.install("extra", "2.0", IN_LEVEL, "Extra");
    w.apply();
    w.mods().set_enabled("city", true).unwrap();
    let game = w.game();
    let mut ws = project.workspace(&game).unwrap();
    set_word(&game, &mut ws, IN_TEXT, "Mine");
    set_word(&game, &mut ws, IN_LEVEL, "Mine too");
    let depends = |project: &Project| -> Vec<(String, String)> {
        let (info, keys) = project.mod_of(&game, &ws).unwrap();
        assert_eq!(keys, None, "all of its own workspace");
        info.relations.depends.into_iter().collect()
    };

    // Without a base: the guess. It finds the mod that is written and
    // misses the one that is only enabled.
    assert_eq!(depends(&project), [("extra".into(), ">=2.0.0".into())]);

    let city = w.mods().get("city").unwrap().manifest.clone();
    let base = Base {
        exclusive: false,
        mods: vec![BaseMod {
            id: "city".into(),
            version: "^1.1.0".into(),
            built_on: "1.1.0".into(),
            sha256: city.sha256(),
        }],
    };
    ProjectConfig::write_base(&root, Some(&base)).unwrap();
    let project = Project::open(&root).unwrap();
    assert_eq!(
        project
            .mod_info()
            .relations
            .depends
            .into_iter()
            .collect::<Vec<_>>(),
        [("city".to_owned(), "^1.1.0".to_owned())]
    );
    assert_eq!(depends(&project), [("city".into(), "^1.1.0".into())]);
    let (info, _) = project.mod_of(&game, &ws).unwrap();
    let made_on: Vec<_> = info.relations.made_on.iter().collect();
    assert_eq!(made_on.len(), 1, "{made_on:?}");
    assert_eq!(made_on[0].0.stream, TEXT);
    assert_eq!(made_on[0].1, &city.resources[0].payload);

    // Installed, it is a patch on top of the base that fits.
    let mut mods = w.mods();
    let report = project
        .install_into(&game, &mut mods, &ws, Default::default())
        .unwrap();
    let issues: Vec<String> = report.issues.iter().map(|i| i.describe()).collect();
    assert!(issues.is_empty(), "{issues:?}");
    let installed = &mods.get("yard-mod").unwrap().manifest;
    assert_eq!(
        installed.depends.iter().collect::<Vec<_>>(),
        [(&"city".to_owned(), &"^1.1.0".to_owned())]
    );
    let text = installed
        .resources
        .iter()
        .find(|r| r.key.stream == TEXT)
        .unwrap();
    assert_eq!(text.original, city.resources[0].payload);
    assert_ne!(text.original, city.resources[0].original);
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    assert_eq!(
        (w.word(IN_TEXT), w.word(IN_LEVEL)),
        ("Mine".into(), "Mine too".into())
    );
    mods.uninstall("yard-mod").unwrap();
    mods.apply(&game, ModApplyOptions::default()).unwrap();

    // The mod file carries the same.
    let out = w.path("yard.zip");
    project.build_mod_on(&w.game(), &ws, &out, None).unwrap();
    let built = crate::mods::ModArchive::open(&out)
        .unwrap()
        .manifest()
        .clone();
    assert_eq!(built.depends, installed_depends(&[("city", "^1.1.0")]));
    let text = built
        .resources
        .iter()
        .find(|r| r.key.stream == TEXT)
        .unwrap();
    assert_eq!(text.original, city.resources[0].payload);

    // Played: staged on top of the mods, the base among them, without a
    // word about originals — whether the base is written or only enabled.
    use crate::play::overlay::{Overlay, Source};
    let overlay = Overlay::build(
        &w.game(),
        Some(&Source::Project(root.clone())),
        None,
        &w.path("overlay"),
    )
    .unwrap();
    assert!(!overlay.redirects().is_empty());
    overlay.remove().unwrap();

    // The mods of stage 36 next to the base: dependencies of any version.
    let mut config = project.config.clone();
    config.dependencies.mods = vec!["extra".into(), "city".into()];
    assert_eq!(
        config.mod_dependencies(),
        installed_depends(&[("city", "^1.1.0"), ("extra", "*")])
    );
}

fn installed_depends(list: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
    list.iter()
        .map(|(id, condition)| ((*id).to_owned(), (*condition).to_owned()))
        .collect()
}

/// Stages 108 and 110 together: the mod of a project whose base is a mod
/// that changes a SimGroup by a delta is itself a delta on what the base
/// gives — the fold of the base's delta, not its document — so both sets of
/// objects are in the game, and the project's mod is checked against the
/// base without a difference.
#[test]
fn a_project_on_a_base_that_stores_deltas_merges_with_it() {
    use crate::mods::build_from_workspace;
    use crate::mods::fold::tests::{add_crate, edit_group, has, info, town, yard_mod};
    use crate::mods::manager::ModApplyOptions;
    use crate::resource::simgroup::SimGroup;
    use crate::testing::{YARD, YARD_SIMGROUP};
    use crate::workspace::ResourceEdit;

    let (dir, game) = town();
    let temp = tempfile::tempdir().unwrap();
    let yard = game.find_stream(YARD).unwrap();
    let yard_of = |game: &GameInstance| {
        let data = game
            .open_stream(yard)
            .unwrap()
            .resource_data(YARD_SIMGROUP)
            .unwrap();
        SimGroup::parse(&data).unwrap()
    };

    // The base mod adds a crate to the yard: a delta, written into the game.
    let city = yard_mod(&game, temp.path(), &info("city", "1.0.0"), |g| {
        add_crate(g, 21)
    });
    let mut mods = ModManager::open(&game).unwrap();
    mods.install_file(&game, &city, true).unwrap();
    assert!(
        mods.get("city").unwrap().manifest.resources[0].is_delta(),
        "the base stores a delta"
    );
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    drop(mods);
    let game = GameInstance::open(dir.path()).unwrap();
    assert!(has(&yard_of(&game), 21));

    // The project is made on the game with the base, and names it.
    let root = temp.path().join("Extension");
    let new = NewDocument {
        name: "Extension".into(),
        ..Default::default()
    };
    create_document(&game, &root, &new).unwrap();
    let recorded = ProjectConfig::record_base(&root, &game).unwrap();
    assert_eq!(recorded.base.mods.len(), 1);
    let project = Project::open(&root).unwrap();

    // Its edit: one more crate in the same SimGroup, next to the base's.
    let mut ws = project.workspace(&game).unwrap();
    let data = ws.current_data(&game, yard, YARD_SIMGROUP).unwrap();
    assert!(
        has(&SimGroup::parse(&data).unwrap(), 21),
        "made on the base"
    );
    let mine = edit_group(&data, |g| add_crate(g, 22));
    let edit = ResourceEdit {
        index: YARD_SIMGROUP,
        data: mine,
        storage: Default::default(),
    };
    ws.set_resources(&game, "one more crate", yard, &[edit])
        .unwrap();

    let (mod_info, keys) = project.mod_of(&game, &ws).unwrap();
    let depends: Vec<_> = mod_info.relations.depends.iter().collect();
    assert_eq!(depends.len(), 1);
    assert_eq!(depends[0].0, "city");
    let out = temp.path().join("extension.zip");
    let report = build_from_workspace(&ws, &mod_info, keys.as_deref(), None, &out).unwrap();
    assert_eq!(
        report.deltas, 1,
        "the edit goes as a delta on the base's data"
    );

    // Installed above the base: no difference with it, and the game has
    // the crates of both.
    let mut mods = ModManager::open(&game).unwrap();
    let installed = mods.install_file(&game, &out, true).unwrap();
    assert!(installed.issues.is_empty(), "{:?}", installed.issues);
    assert!(mods.check(&game).unwrap().is_empty());
    mods.apply(&game, ModApplyOptions::default()).unwrap();
    let group = yard_of(&GameInstance::open(dir.path()).unwrap());
    assert!(has(&group, 21) && has(&group, 22));

    // Without the base's crate the extension still adds its own.
    mods.set_enabled("city", false).unwrap();
    let issues = mods.check(&game).unwrap();
    assert!(!issues.is_empty(), "the dependency is switched off");
}
