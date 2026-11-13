//! The ASI loader of a game folder: the module of the live link needs one.
//!
//! The toolkit never downloads or installs a loader; it only tells whether
//! one is there. Ultimate ASI Loader (ThirteenAG) — what the players of this
//! game use, as `dinput8.dll` — loads `*.asi` from the game folder,
//! `scripts\` and `plugins\`.

use std::path::{Path, PathBuf};

/// Names the loader goes by (it works as any of these system DLLs).
const PROXY_NAMES: &[&str] = &[
    "dinput8.dll",
    "d3d9.dll",
    "winmm.dll",
    "dsound.dll",
    "version.dll",
    "wininet.dll",
    "winhttp.dll",
    "xlive.dll",
];

/// Strings that mark Ultimate ASI Loader.
const MARKS: &[&[u8]] = &[
    b"Ultimate ASI Loader",
    b"IsUltimateASILoader",
    b"Ultimate-ASI-Loader",
];

/// What was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loader {
    /// The DLL, e.g. `dinput8.dll`.
    pub file: PathBuf,
    /// Recognised as Ultimate ASI Loader by its strings.
    pub ultimate: bool,
    /// `.asi` files already in the folder and `scripts/` (other mods).
    pub modules: Vec<String>,
}

impl Loader {
    /// The Wine DLL override that makes Wine load it instead of its own
    /// DLL of that name (`dinput8=n,b`).
    pub fn wine_override(&self) -> String {
        let stem = self
            .file
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        format!("{stem}=n,b")
    }
}

fn contains(data: &[u8], needle: &[u8]) -> bool {
    data.windows(needle.len()).any(|w| w == needle)
}

/// Finds the ASI loader in a game folder.
pub fn find(root: &Path) -> Option<Loader> {
    let entries: Vec<_> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .collect();
    let mut found = None;
    for name in PROXY_NAMES {
        let Some(entry) = entries
            .iter()
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        else {
            continue;
        };
        let data = std::fs::read(entry.path()).unwrap_or_default();
        let ultimate = MARKS.iter().any(|m| contains(&data, m));
        // A `dinput8.dll` that is not the loader is still most likely one
        // (the game itself has none); other names count only when marked.
        if ultimate || name.eq_ignore_ascii_case("dinput8.dll") {
            found = Some((entry.path(), ultimate));
            if ultimate {
                break;
            }
        }
    }
    let (file, ultimate) = found?;
    let mut modules = Vec::new();
    for dir in [root.to_path_buf(), root.join(super::SCRIPTS_DIR)] {
        let Ok(list) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in list.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.to_ascii_lowercase().ends_with(".asi") {
                modules.push(if dir == root {
                    name
                } else {
                    format!("{}/{name}", super::SCRIPTS_DIR)
                });
            }
        }
    }
    modules.sort();
    Some(Loader {
        file,
        ultimate,
        modules,
    })
}
