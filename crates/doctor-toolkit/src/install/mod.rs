//! Finds installed copies of the game: Steam libraries (native, Flatpak and
//! Snap Steam, with Proton prefixes and non-Steam shortcuts), Wine prefixes
//! (their registry and the usual install folders, which covers Origin and
//! the EA app installed inside a prefix), Lutris, Heroic and Bottles, and
//! on Windows the registry and the default folders.
//!
//! Detection only reads: launcher configurations, registry files and
//! directory listings. Every candidate is checked with
//! [`crate::game::looks_like_game`] before it is reported.
//!
//! [`SearchRoots`] holds every place searched, so that tests run on fake
//! trees; [`SearchRoots::from_env`] fills it for this computer.

pub mod parse;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::game::looks_like_game;
use crate::play::LaunchMethod;
use parse::{Kv, binary_vdf, text_vdf, wine_reg, yaml_values};

/// Steam application id of The Godfather II (delisted in 2014, still
/// installable from libraries that own it).
pub const STEAM_APP_ID: u32 = 24830;

/// Words in registry keys and launcher entries that name the game.
const NAMES: &[&str] = &["godfather ii", "godfather 2", "godfather2"];

/// Registry value names that hold an install folder.
const DIR_VALUES: &[&str] = &[
    "install dir",
    "installdir",
    "install path",
    "installpath",
    "installlocation",
    "install location",
    "path",
    "gamedir",
];

/// Folders inside `drive_c` (and `C:\` on Windows) where installers put games.
const PROGRAM_DIRS: &[&str] = &[
    "Program Files (x86)",
    "Program Files",
    "Games",
    "GOG Games",
    "EA Games",
];
/// Publisher folders inside them (empty: the program folder itself).
const PUBLISHER_DIRS: &[&str] = &["EA Games", "Origin Games", "Electronic Arts", "EA", ""];

/// Where to look.
#[derive(Debug, Clone, Default)]
pub struct SearchRoots {
    pub home: Option<PathBuf>,
    /// `$XDG_DATA_HOME`, by default `~/.local/share`.
    pub data_home: Option<PathBuf>,
    /// `$XDG_CONFIG_HOME`, by default `~/.config`.
    pub config_home: Option<PathBuf>,
    /// `$WINEPREFIX`.
    pub wine_prefix: Option<PathBuf>,
    /// More Wine prefixes or game folders to check.
    pub extra: Vec<PathBuf>,
    /// Search the Windows registry (`reg query`) and the drives' program folders.
    pub windows: bool,
}

impl SearchRoots {
    /// The places of this computer.
    pub fn from_env() -> Self {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let home = var("HOME").or_else(|| var("USERPROFILE"));
        Self {
            data_home: var("XDG_DATA_HOME")
                .or_else(|| home.as_ref().map(|h| h.join(".local").join("share"))),
            config_home: var("XDG_CONFIG_HOME")
                .or_else(|| home.as_ref().map(|h| h.join(".config"))),
            wine_prefix: var("WINEPREFIX"),
            home,
            extra: Vec::new(),
            windows: cfg!(windows),
        }
    }
}

/// How a copy was found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub kind: OriginKind,
    /// The file or folder that told: a manifest, a registry key, a prefix.
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OriginKind {
    Steam,
    Proton,
    Wine,
    Lutris,
    Heroic,
    Bottles,
    Registry,
    Folder,
}

impl OriginKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Steam => "Steam",
            Self::Proton => "Proton",
            Self::Wine => "Wine",
            Self::Lutris => "Lutris",
            Self::Heroic => "Heroic",
            Self::Bottles => "Bottles",
            Self::Registry => "registry",
            Self::Folder => "folder",
        }
    }
}

/// An installed copy of the game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Install {
    /// The game directory.
    pub path: PathBuf,
    /// Profile name, e.g. `The Godfather II (PC)`.
    pub profile: String,
    pub found_by: Vec<Origin>,
    /// Ways to start it, the most fitting first.
    pub launch: Vec<LaunchMethod>,
}

impl Install {
    /// `Steam, Wine` — how it was found, for one line of text.
    pub fn sources(&self) -> String {
        let mut kinds: Vec<OriginKind> = self.found_by.iter().map(|o| o.kind).collect();
        kinds.sort();
        kinds.dedup();
        kinds
            .iter()
            .map(|k| k.label())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Collects candidates; [`Finder::finish`] checks and merges them.
#[derive(Default)]
struct Finder {
    candidates: Vec<(PathBuf, Origin, Option<LaunchMethod>)>,
}

impl Finder {
    fn add(
        &mut self,
        path: PathBuf,
        kind: OriginKind,
        detail: String,
        launch: Option<LaunchMethod>,
    ) {
        self.candidates
            .push((path, Origin { kind, detail }, launch));
    }

    fn finish(self) -> Vec<Install> {
        let mut by_path: BTreeMap<PathBuf, Install> = BTreeMap::new();
        for (path, origin, launch) in self.candidates {
            let Some(profile) = looks_like_game(&path) else {
                continue;
            };
            let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            let install = by_path.entry(key).or_insert_with(|| Install {
                path: path.clone(),
                profile: profile.name.to_owned(),
                found_by: Vec::new(),
                launch: Vec::new(),
            });
            if !install.found_by.contains(&origin) {
                install.found_by.push(origin);
            }
            if let Some(launch) = launch
                && !install.launch.contains(&launch)
            {
                install.launch.push(launch);
            }
        }
        let mut installs: Vec<Install> = by_path.into_values().collect();
        for install in &mut installs {
            // A prefix found by a launcher's configuration is not also listed
            // as a plain Wine prefix.
            let launcher_details: Vec<String> = install
                .found_by
                .iter()
                .filter(|o| o.kind != OriginKind::Wine)
                .map(|o| o.detail.clone())
                .collect();
            install
                .found_by
                .retain(|o| o.kind != OriginKind::Wine || !launcher_details.contains(&o.detail));
            install.launch.sort_by_key(LaunchMethod::preference);
            let fallback = crate::play::fallback_method(&install.path);
            if !install.launch.contains(&fallback) {
                install.launch.push(fallback);
            }
        }
        installs
    }
}

/// Every copy of the game found in `roots`, each listed once (by its real path).
pub fn detect(roots: &SearchRoots) -> Vec<Install> {
    let mut finder = Finder::default();
    for steam in steam_roots(roots) {
        search_steam(&steam, &mut finder);
    }
    for (prefix, launch) in wine_prefixes(roots) {
        search_prefix(&prefix, OriginKind::Wine, launch, &mut finder);
    }
    search_lutris(roots, &mut finder);
    search_heroic(roots, &mut finder);
    search_bottles(roots, &mut finder);
    for dir in &roots.extra {
        if dir.join("drive_c").is_dir() {
            search_prefix(dir, OriginKind::Wine, None, &mut finder);
        } else {
            finder.add(
                dir.clone(),
                OriginKind::Folder,
                dir.display().to_string(),
                None,
            );
        }
    }
    if roots.windows {
        search_windows(&mut finder);
    }
    finder.finish()
}

// ------------------------------------------------------------------ Steam

/// Steam installations: native, Flatpak and Snap.
pub(crate) fn steam_roots(roots: &SearchRoots) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = &roots.home {
        for rel in [
            ".steam/steam",
            ".steam/root",
            ".local/share/Steam",
            ".var/app/com.valvesoftware.Steam/.local/share/Steam",
            ".var/app/com.valvesoftware.Steam/data/Steam",
            "snap/steam/common/.local/share/Steam",
            "Library/Application Support/Steam",
        ] {
            out.push(home.join(rel));
        }
    }
    if let Some(data) = &roots.data_home {
        out.push(data.join("Steam"));
    }
    if roots.windows {
        out.extend(windows_steam_dirs());
    }
    let mut seen = Vec::new();
    out.retain(|p| {
        let real = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        p.join("steamapps").is_dir() && !seen.contains(&real) && {
            seen.push(real);
            true
        }
    });
    out
}

/// The library folders of a Steam installation, itself first.
pub(crate) fn steam_libraries(steam: &Path) -> Vec<PathBuf> {
    let mut libraries = vec![steam.to_path_buf()];
    for file in [
        steam.join("steamapps/libraryfolders.vdf"),
        steam.join("config/libraryfolders.vdf"),
    ] {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let kv = text_vdf(&text);
        let Some(folders) = kv
            .get("libraryfolders")
            .or_else(|| kv.get("LibraryFolders"))
        else {
            continue;
        };
        for (_, folder) in folders.children() {
            // New format: a table with `path`; old format: the path itself.
            let path = folder
                .get("path")
                .and_then(Kv::str)
                .or_else(|| folder.str());
            if let Some(path) = path.filter(|p| p.contains(['/', '\\'])) {
                let path = PathBuf::from(path);
                if !libraries.contains(&path) {
                    libraries.push(path);
                }
            }
        }
    }
    libraries
}

fn search_steam(steam: &Path, finder: &mut Finder) {
    for library in steam_libraries(steam) {
        let apps = library.join("steamapps");
        let manifest = apps.join(format!("appmanifest_{STEAM_APP_ID}.acf"));
        if let Ok(text) = std::fs::read_to_string(&manifest) {
            let kv = text_vdf(&text);
            if let Some(dir) = kv.at(&["AppState", "installdir"]).and_then(Kv::str) {
                finder.add(
                    apps.join("common").join(dir),
                    OriginKind::Steam,
                    manifest.display().to_string(),
                    Some(LaunchMethod::Steam {
                        app_id: STEAM_APP_ID,
                    }),
                );
            }
        }
        // Any copy in the library, also one put there by hand.
        for dir in list_dirs(&apps.join("common")) {
            finder.add(
                dir,
                OriginKind::Steam,
                apps.join("common").display().to_string(),
                None,
            );
        }
        // Proton prefixes: of the Steam game and of non-Steam shortcuts.
        for compat in list_dirs(&apps.join("compatdata")) {
            let prefix = compat.join("pfx");
            if !prefix.join("drive_c").is_dir() {
                continue;
            }
            let launch = compat
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.parse::<u32>().ok())
                .map(steam_launch);
            search_prefix(&prefix, OriginKind::Proton, launch, finder);
        }
    }
    search_shortcuts(steam, finder);
}

/// How Steam starts an application id: a Steam game or a shortcut.
fn steam_launch(app_id: u32) -> LaunchMethod {
    if app_id >= 0x8000_0000 {
        LaunchMethod::SteamShortcut { app_id }
    } else {
        LaunchMethod::Steam { app_id }
    }
}

/// Non-Steam games added to Steam (`userdata/*/config/shortcuts.vdf`).
fn search_shortcuts(steam: &Path, finder: &mut Finder) {
    for user in list_dirs(&steam.join("userdata")) {
        let file = user.join("config/shortcuts.vdf");
        let Ok(data) = std::fs::read(&file) else {
            continue;
        };
        if data.len() > 16 << 20 {
            continue;
        }
        let kv = binary_vdf(&data);
        let Some(shortcuts) = kv.get("shortcuts") else {
            continue;
        };
        for (_, entry) in shortcuts.children() {
            let exe = entry
                .get("exe")
                .and_then(Kv::str)
                .unwrap_or_default()
                .trim_matches('"');
            let start = entry
                .get("startdir")
                .and_then(Kv::str)
                .unwrap_or_default()
                .trim_matches('"');
            let names_game = [exe, start]
                .iter()
                .any(|s| NAMES.iter().any(|n| s.to_lowercase().contains(n)));
            if !names_game {
                continue;
            }
            let launch = entry
                .get("appid")
                .and_then(Kv::int)
                .map(|id| steam_launch(id as u32));
            for dir in [
                Path::new(exe).parent().map(Path::to_path_buf),
                (!start.is_empty()).then(|| PathBuf::from(start)),
            ]
            .into_iter()
            .flatten()
            {
                finder.add(
                    dir,
                    OriginKind::Steam,
                    file.display().to_string(),
                    launch.clone(),
                );
            }
        }
    }
}

// ------------------------------------------------------------------- Wine

/// Wine prefixes outside Steam, Lutris, Heroic and Bottles configurations,
/// with how to start a game in them when known.
fn wine_prefixes(roots: &SearchRoots) -> Vec<(PathBuf, Option<LaunchMethod>)> {
    let mut out: Vec<(PathBuf, Option<LaunchMethod>)> = Vec::new();
    let wine = |prefix: &Path| LaunchMethod::Wine {
        prefix: Some(prefix.to_path_buf()),
        wine: None,
        dll_overrides: None,
    };
    let mut push = |prefix: PathBuf, launch: Option<LaunchMethod>| {
        if prefix.join("drive_c").is_dir() && !out.iter().any(|(p, _)| *p == prefix) {
            let launch = launch.or_else(|| Some(wine(&prefix)));
            out.push((prefix, launch));
        }
    };
    if let Some(prefix) = &roots.wine_prefix {
        push(prefix.clone(), None);
    }
    if let Some(home) = &roots.home {
        push(home.join(".wine"), None);
        for parent in [
            ".local/share/wineprefixes",
            ".PlayOnLinux/wineprefix",
            "Games",
            "Games/Heroic/Prefixes",
            "Games/Heroic/Prefixes/default",
            ".cxoffice",
        ] {
            for prefix in list_dirs(&home.join(parent)) {
                push(prefix, None);
            }
        }
    }
    if let Some(data) = &roots.data_home {
        for parent in ["wineprefixes", "doctor_toolkit"] {
            for prefix in list_dirs(&data.join(parent)) {
                push(prefix, None);
            }
        }
    }
    out
}

/// Searches one prefix: its registry, then the usual install folders.
fn search_prefix(
    prefix: &Path,
    kind: OriginKind,
    launch: Option<LaunchMethod>,
    finder: &mut Finder,
) {
    let detail = prefix.display().to_string();
    for file in ["system.reg", "user.reg"] {
        let path = prefix.join(file);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.len() > 256 << 20 {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let keys = wine_reg(&String::from_utf8_lossy(&bytes));
        for (key, value) in registry_dirs(keys.iter().map(|(k, v)| (k.as_str(), v.as_slice()))) {
            if let Some(dir) = windows_path_in_prefix(prefix, &value) {
                finder.add(
                    dir,
                    kind,
                    format!("{} [{key}]", path.display()),
                    launch.clone(),
                );
            }
        }
    }
    for dir in program_dirs(&prefix.join("drive_c")) {
        finder.add(dir, kind, detail.clone(), launch.clone());
    }
}

/// Install folders named by registry keys about the game: `(key, folder)`.
fn registry_dirs<'a>(
    keys: impl Iterator<Item = (&'a str, &'a [(String, String)])>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (key, values) in keys {
        let about_game = NAMES.iter().any(|n| key.contains(n))
            || values.iter().any(|(name, value)| {
                name.eq_ignore_ascii_case("DisplayName")
                    && NAMES.iter().any(|n| value.to_lowercase().contains(n))
            });
        if !about_game {
            continue;
        }
        for (name, value) in values {
            let lower = name.to_lowercase();
            if DIR_VALUES.contains(&lower.as_str()) && value.contains(['\\', '/']) {
                out.push((key.to_owned(), value.clone()));
            }
            // `Exe`, `DisplayIcon`: the folder of a path to the executable.
            if value.to_lowercase().ends_with("godfather2.exe")
                && let Some((dir, _)) = value.rsplit_once(['\\', '/'])
            {
                out.push((key.to_owned(), dir.to_owned()));
            }
        }
    }
    out
}

/// A Windows path inside a Wine prefix: `C:\…` is `drive_c`, other drives
/// follow `dosdevices` (`Z:` is usually `/`).
pub fn windows_path_in_prefix(prefix: &Path, path: &str) -> Option<PathBuf> {
    let path = path.trim().trim_matches('"');
    let mut chars = path.chars();
    let (drive, colon) = (chars.next()?, chars.next()?);
    if colon != ':' || !drive.is_ascii_alphabetic() {
        return None;
    }
    let rest = &path[2..];
    let base = if drive.eq_ignore_ascii_case(&'c') && prefix.join("drive_c").is_dir() {
        prefix.join("drive_c")
    } else {
        let link = prefix
            .join("dosdevices")
            .join(format!("{}:", drive.to_ascii_lowercase()));
        std::fs::canonicalize(link).ok()?
    };
    let mut out = base;
    for part in rest.split(['\\', '/']).filter(|p| !p.is_empty()) {
        if part == ".." {
            return None;
        }
        out.push(part);
    }
    Some(out)
}

/// Game folders inside the usual program folders of a drive.
fn program_dirs(drive: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for program in PROGRAM_DIRS {
        for publisher in PUBLISHER_DIRS {
            let parent = drive.join(program).join(publisher);
            for dir in list_dirs(&parent) {
                let name = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if NAMES.iter().any(|n| name.contains(n)) || name.contains("godfather") {
                    out.push(dir);
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------- launchers

fn search_lutris(roots: &SearchRoots, finder: &mut Finder) {
    let mut dirs = Vec::new();
    for base in [roots.config_home.as_ref(), roots.data_home.as_ref()]
        .into_iter()
        .flatten()
    {
        dirs.push(base.join("lutris/games"));
    }
    if let Some(home) = &roots.home {
        dirs.push(home.join(".var/app/net.lutris.Lutris/config/lutris/games"));
        dirs.push(home.join(".var/app/net.lutris.Lutris/data/lutris/games"));
    }
    for dir in dirs {
        for file in list_files(&dir, "yml") {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let exe = yaml_values(&text, "exe");
            let names_game = exe
                .iter()
                .any(|e| e.to_lowercase().ends_with("godfather2.exe"))
                || NAMES.iter().any(|n| {
                    file.file_name().is_some_and(|f| {
                        f.to_string_lossy()
                            .to_lowercase()
                            .contains(&n.replace(' ', "-"))
                    })
                });
            if !names_game {
                continue;
            }
            // `slug:` at the top level, else from `<slug>-<timestamp>.yml`.
            let stem = file
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let top_slug = parse::yaml_entries(&text, "slug")
                .into_iter()
                .find(|(indent, _)| *indent == 0)
                .map(|(_, slug)| slug);
            let slug = match stem.rsplit_once('-') {
                _ if top_slug.is_some() => top_slug.unwrap_or_default(),
                Some((slug, stamp)) if stamp.chars().all(|c| c.is_ascii_digit()) => slug.to_owned(),
                _ => stem.clone(),
            };
            let launch = Some(LaunchMethod::Lutris { game: slug });
            let prefix = yaml_values(&text, "prefix")
                .into_iter()
                .next()
                .map(PathBuf::from);
            for e in &exe {
                let path = PathBuf::from(e);
                let path = match (&prefix, path.is_absolute()) {
                    (Some(prefix), false) => prefix.join(path),
                    _ => path,
                };
                if let Some(dir) = path.parent() {
                    finder.add(
                        dir.to_path_buf(),
                        OriginKind::Lutris,
                        file.display().to_string(),
                        launch.clone(),
                    );
                }
            }
            if let Some(prefix) = &prefix {
                search_prefix(prefix, OriginKind::Lutris, launch.clone(), finder);
            }
        }
    }
}

fn search_heroic(roots: &SearchRoots, finder: &mut Finder) {
    let mut configs = Vec::new();
    if let Some(config) = &roots.config_home {
        configs.push(config.join("heroic"));
    }
    if let Some(home) = &roots.home {
        configs.push(home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic"));
    }
    for heroic in configs {
        // Sideloaded games and GOG installs name their folders.
        for (file, runner) in [
            ("sideload_apps/library.json", "sideload"),
            ("gog_store/installed.json", "gog"),
        ] {
            let path = heroic.join(file);
            let Some(json) = read_json(&path) else {
                continue;
            };
            let entries = json
                .get("games")
                .or_else(|| json.get("installed"))
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            for entry in entries {
                let app = entry
                    .get("app_name")
                    .or_else(|| entry.get("appName"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_owned();
                let mut dirs = Vec::new();
                for field in ["install_path", "folder_name"] {
                    if let Some(dir) = entry
                        .get(field)
                        .or_else(|| entry.get("install").and_then(|i| i.get(field)))
                        .and_then(|v| v.as_str())
                    {
                        dirs.push(PathBuf::from(dir));
                    }
                }
                if let Some(exe) = entry
                    .get("install")
                    .and_then(|i| i.get("executable"))
                    .and_then(|v| v.as_str())
                    && let Some(dir) = Path::new(exe).parent()
                {
                    dirs.push(dir.to_path_buf());
                }
                let launch = (!app.is_empty()).then(|| LaunchMethod::Heroic {
                    app: app.clone(),
                    runner: runner.to_owned(),
                });
                for dir in dirs {
                    finder.add(
                        dir,
                        OriginKind::Heroic,
                        path.display().to_string(),
                        launch.clone(),
                    );
                }
            }
        }
        // Per-game settings name the Wine prefix.
        for file in list_files(&heroic.join("GamesConfig"), "json") {
            let Some(json) = read_json(&file) else {
                continue;
            };
            let Some(obj) = json.as_object() else {
                continue;
            };
            for (app, settings) in obj {
                if let Some(prefix) = settings.get("winePrefix").and_then(|v| v.as_str()) {
                    let launch = Some(LaunchMethod::Heroic {
                        app: app.clone(),
                        runner: "sideload".to_owned(),
                    });
                    search_prefix(Path::new(prefix), OriginKind::Heroic, launch, finder);
                }
            }
        }
    }
}

fn search_bottles(roots: &SearchRoots, finder: &mut Finder) {
    let mut parents = Vec::new();
    if let Some(data) = &roots.data_home {
        parents.push((data.join("bottles/bottles"), false));
    }
    if let Some(home) = &roots.home {
        parents.push((
            home.join(".var/app/com.usebottles.bottles/data/bottles/bottles"),
            true,
        ));
    }
    for (parent, flatpak) in parents {
        for bottle in list_dirs(&parent) {
            let name = std::fs::read_to_string(bottle.join("bottle.yml"))
                .ok()
                .and_then(|t| yaml_values(&t, "Name").into_iter().next())
                .or_else(|| bottle.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default();
            let launch = Some(LaunchMethod::Bottles {
                bottle: name,
                flatpak,
            });
            search_prefix(&bottle, OriginKind::Bottles, launch, finder);
        }
    }
}

// ---------------------------------------------------------------- Windows

#[cfg(windows)]
fn reg_query_values(key: &str) -> Vec<(String, String, String)> {
    let output = std::process::Command::new("reg")
        .args(["query", key, "/s"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(out) if out.status.success() => parse::reg_query(&String::from_utf8_lossy(&out.stdout)),
        _ => Vec::new(),
    }
}

#[cfg(not(windows))]
fn reg_query_values(_key: &str) -> Vec<(String, String, String)> {
    Vec::new()
}

/// Steam folders named by the Windows registry and the default folder.
fn windows_steam_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for key in [
        r"HKCU\Software\Valve\Steam",
        r"HKLM\SOFTWARE\WOW6432Node\Valve\Steam",
        r"HKLM\SOFTWARE\Valve\Steam",
    ] {
        for (_, name, value) in reg_query_values(key) {
            if name.eq_ignore_ascii_case("SteamPath") || name.eq_ignore_ascii_case("InstallPath") {
                out.push(PathBuf::from(value.replace('/', "\\")));
            }
        }
    }
    if let Some(dir) = std::env::var_os("ProgramFiles(x86)") {
        out.push(PathBuf::from(dir).join("Steam"));
    }
    out
}

fn search_windows(finder: &mut Finder) {
    for root in [
        r"HKLM\SOFTWARE\WOW6432Node\EA Games",
        r"HKLM\SOFTWARE\EA Games",
        r"HKLM\SOFTWARE\WOW6432Node\Electronic Arts",
        r"HKLM\SOFTWARE\Electronic Arts",
        r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    ] {
        let values = reg_query_values(root);
        let mut keys: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for (key, name, value) in values {
            keys.entry(key).or_default().push((name, value));
        }
        for (key, dir) in registry_dirs(keys.iter().map(|(k, v)| (k.as_str(), v.as_slice()))) {
            finder.add(
                PathBuf::from(dir),
                OriginKind::Registry,
                key,
                Some(LaunchMethod::Direct),
            );
        }
    }
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(dir) = std::env::var_os(var) {
            let dir = PathBuf::from(dir);
            for publisher in PUBLISHER_DIRS {
                for game in list_dirs(&dir.join(publisher)) {
                    finder.add(
                        game,
                        OriginKind::Folder,
                        dir.display().to_string(),
                        Some(LaunchMethod::Direct),
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------- helpers

/// Sub-directories of `dir` (following links), at most a few thousand.
fn list_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .take(4096)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

fn list_files(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .take(4096)
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case(extension))
        })
        .collect();
    out.sort();
    out
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > 64 << 20 {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

#[cfg(test)]
mod tests;
