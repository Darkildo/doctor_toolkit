//! Hardware rendering under Wine: DXVK's `d3d9.dll` in the game folder for
//! one launch.
//!
//! Without it Wine draws the game with OpenGL. In a nested X server (the
//! editor's Game tab, the bench) there is no DRI3, so OpenGL is llvmpipe:
//! 1–2 frames a second in a level. DXVK draws with Vulkan, and the Vulkan
//! driver of the graphics card presents into such a server too (stage 33,
//! `docs/analysis/game-bench.md` §19): about 60 frames a second.
//!
//! The toolkit does not ship DXVK. It takes the 32-bit `d3d9.dll` of one
//! already on the computer ([`find`]: `DOCTOR_DXVK`, Lutris, Proton,
//! Bottles, Heroic), copies it into the game folder for the launch with
//! `WINEDLLOVERRIDES` `d3d9=n,b` ([`DxvkInstall`], [`apply_env`]) and takes
//! it away after the game exits, like the live module. The game is 32-bit,
//! so Vulkan needs the 32-bit loader and driver ([`vulkan32`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{LaunchMethod, LaunchPlan};
use crate::install::SearchRoots;
use crate::{Error, Result};

/// The DLL DXVK replaces for a Direct3D 9 game.
pub const DLL: &str = "d3d9.dll";
/// Where the toolkit records what it installed, and where DXVK writes its
/// log (`godfather2_d3d9.log`).
pub const STATE_DIR: &str = "_doctor_toolkit/dxvk";
/// Copy of the installed DLL, to tell it from a player's own `d3d9.dll`.
const WRITTEN: &str = "d3d9.dll.written";

/// A DXVK found on the computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dxvk {
    /// Its 32-bit `d3d9.dll`.
    pub dll: PathBuf,
    /// Where it came from, e.g. `Lutris v2.6.2`.
    pub source: String,
}

/// Finds DXVK's 32-bit `d3d9.dll`: `DOCTOR_DXVK` (the DLL or a DXVK folder
/// with `x32/`), then Lutris, Proton, Bottles and Heroic, newest first.
pub fn find() -> Option<Dxvk> {
    let env = std::env::var_os("DOCTOR_DXVK").filter(|v| !v.is_empty());
    find_in(&SearchRoots::from_env(), env.map(PathBuf::from).as_deref())
}

/// [`find`] in given places.
pub fn find_in(roots: &SearchRoots, explicit: Option<&Path>) -> Option<Dxvk> {
    if let Some(path) = explicit {
        return dll_of(path).map(|dll| Dxvk {
            dll,
            source: "DOCTOR_DXVK".into(),
        });
    }
    let mut found = Vec::new();
    // Releases unpacked by a launcher: <dir>/<version>/x32/d3d9.dll.
    let mut releases = Vec::new();
    if let Some(data) = &roots.data_home {
        releases.push(("Lutris", data.join("lutris/runtime/dxvk")));
        releases.push(("Bottles", data.join("bottles/dxvk")));
    }
    if let Some(home) = &roots.home {
        releases.push((
            "Bottles",
            home.join(".var/app/com.usebottles.bottles/data/bottles/dxvk"),
        ));
    }
    if let Some(config) = &roots.config_home {
        releases.push(("Heroic", config.join("heroic/tools/dxvk")));
    }
    for (launcher, dir) in releases {
        let mut versions: Vec<_> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.join("x32").join(DLL).is_file())
            .collect();
        versions.sort_by_key(|v| std::cmp::Reverse(version_key(v)));
        if let Some(v) = versions.first() {
            let name = v.file_name().unwrap_or_default().to_string_lossy();
            found.push(Dxvk {
                dll: v.join("x32").join(DLL),
                source: format!("{launcher} {name}"),
            });
        }
    }
    // Proton: files/lib/wine/dxvk/i386-windows (8+) or dist/lib/wine/dxvk.
    for steam in crate::install::steam_roots(roots) {
        for library in crate::install::steam_libraries(&steam) {
            let common = library.join("steamapps/common");
            let mut protons: Vec<_> = std::fs::read_dir(&common)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("Proton"))
                })
                .collect();
            protons.sort_by_key(|p| std::cmp::Reverse(version_key(p)));
            for proton in protons {
                for rel in [
                    "files/lib/wine/dxvk/i386-windows",
                    "dist/lib/wine/dxvk",
                    "files/lib/wine/dxvk",
                ] {
                    let dll = proton.join(rel).join(DLL);
                    if dll.is_file() && is_pe32(&dll) {
                        let name = proton.file_name().unwrap_or_default().to_string_lossy();
                        found.push(Dxvk {
                            dll,
                            source: name.into_owned(),
                        });
                        break;
                    }
                }
            }
        }
    }
    found.into_iter().next()
}

/// The 32-bit DLL of `path`: the DLL itself or a DXVK folder (`x32/`).
fn dll_of(path: &Path) -> Option<PathBuf> {
    let candidates = [
        path.to_path_buf(),
        path.join("x32").join(DLL),
        path.join(DLL),
    ];
    candidates.into_iter().find(|p| p.is_file() && is_pe32(p))
}

/// Orders `v2.6.2`, `dxvk-2.3`, `Proton 9.0` by their numbers.
fn version_key(path: &Path) -> Vec<u64> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    name.split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// Whether `path` is a 32-bit (i386) Windows DLL.
fn is_pe32(path: &Path) -> bool {
    let Ok(data) = std::fs::read(path) else {
        return false;
    };
    let Some(pe) = data
        .get(0x3C..0x40)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
    else {
        return false;
    };
    data.get(pe..pe + 6)
        .is_some_and(|h| &h[..4] == b"PE\0\0" && h[4..6] == [0x4C, 0x01])
}

/// The 32-bit Vulkan of the computer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vulkan32 {
    /// The 32-bit Vulkan loader (`libvulkan.so.1`).
    pub loader: Option<PathBuf>,
    /// Drivers of graphics cards with a 32-bit library (ICD file names,
    /// e.g. `nvidia_icd.json`).
    pub drivers: Vec<String>,
    /// Only a software driver (llvmpipe) has a 32-bit library.
    pub software_only: bool,
}

impl Vulkan32 {
    /// Whether DXVK can draw with a graphics card.
    pub fn usable(&self) -> bool {
        self.loader.is_some() && !self.drivers.is_empty()
    }
}

/// Library folders where 32-bit libraries live on Linux distributions.
const LIB32_DIRS: &[&str] = &[
    "usr/lib/i386-linux-gnu",
    "lib/i386-linux-gnu",
    "usr/lib32",
    "lib32",
    "usr/lib",
];
/// Folders of Vulkan driver manifests (ICDs).
const ICD_DIRS: &[&str] = &[
    "usr/share/vulkan/icd.d",
    "etc/vulkan/icd.d",
    "usr/local/share/vulkan/icd.d",
];

/// What 32-bit Vulkan the system under `/` has.
pub fn vulkan32() -> Vulkan32 {
    vulkan32_in(Path::new("/"))
}

/// [`vulkan32`] under `root` (tests use a fake tree).
pub fn vulkan32_in(root: &Path) -> Vulkan32 {
    let find32 = |name: &str| -> Option<PathBuf> {
        let path = Path::new(name);
        if path.is_absolute() {
            let p = root.join(path.strip_prefix("/").unwrap_or(path));
            return is_elf32(&p).then_some(p);
        }
        LIB32_DIRS
            .iter()
            .map(|d| root.join(d).join(name))
            .find(|p| is_elf32(p))
    };
    let mut out = Vulkan32 {
        loader: find32("libvulkan.so.1"),
        ..Vulkan32::default()
    };
    let mut software = false;
    let mut seen = Vec::new();
    for dir in ICD_DIRS {
        let Ok(list) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        let mut files: Vec<_> = list.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        files.sort();
        for file in files {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let Some(library) = json
                .get("ICD")
                .and_then(|i| i.get("library_path"))
                .and_then(|l| l.as_str())
            else {
                continue;
            };
            if find32(library).is_none() {
                continue;
            }
            let name = file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if name.contains("lvp") || library.contains("lvp") {
                software = true;
            } else if !seen.contains(&name) {
                seen.push(name.clone());
                out.drivers.push(name);
            }
        }
    }
    out.software_only = software && out.drivers.is_empty();
    out
}

/// Whether `path` is a 32-bit ELF file (following links).
fn is_elf32(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 5];
    file.read_exact(&mut head).is_ok() && &head[..4] == b"\x7fELF" && head[4] == 1
}

/// Why DXVK cannot be used for a launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// Not Linux: the game draws with the system's Direct3D.
    NotLinux,
    /// Proton draws with its own DXVK already.
    ProtonHasIt,
    /// Only the Wine method passes the DLL override to the game.
    Method(String),
    /// No DXVK on the computer.
    NoDxvk,
    /// No 32-bit Vulkan loader.
    NoLoader,
    /// No 32-bit Vulkan driver of a graphics card.
    NoDriver { software_only: bool },
    /// A `d3d9.dll` the toolkit did not put there is in the game folder.
    Occupied(PathBuf),
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotLinux => write!(f, "DXVK is for Wine on Linux"),
            Self::ProtonHasIt => write!(f, "Proton draws with its own DXVK already"),
            Self::Method(kind) => write!(
                f,
                "only the `wine` launch method can load DXVK from the game folder (method: {kind})"
            ),
            Self::NoDxvk => write!(
                f,
                "no DXVK found (Lutris, Proton, Bottles, Heroic); set DOCTOR_DXVK to a DXVK folder"
            ),
            Self::NoLoader => write!(
                f,
                "no 32-bit Vulkan loader: install libvulkan1:i386 (Debian, Ubuntu, Mint) or lib32-vulkan-icd-loader (Arch)"
            ),
            Self::NoDriver { software_only } => write!(
                f,
                "no 32-bit Vulkan driver of the graphics card{}: NVIDIA — libnvidia-gl-<driver version>:i386 \
                 (Arch: lib32-nvidia-utils); AMD, Intel — mesa-vulkan-drivers:i386 (Arch: lib32-vulkan-radeon, lib32-vulkan-intel)",
                if *software_only {
                    " (only the software one, llvmpipe)"
                } else {
                    ""
                }
            ),
            Self::Occupied(path) => write!(
                f,
                "{} is in the game folder and was not put there by the toolkit",
                path.display()
            ),
        }
    }
}

/// Everything needed for a launch with DXVK, or the first thing missing.
pub fn check(method: &LaunchMethod, root: &Path) -> std::result::Result<Dxvk, Problem> {
    check_with(method, root, find(), &vulkan32())
}

/// [`check`] with given findings.
pub fn check_with(
    method: &LaunchMethod,
    root: &Path,
    dxvk: Option<Dxvk>,
    vulkan: &Vulkan32,
) -> std::result::Result<Dxvk, Problem> {
    if cfg!(windows) {
        return Err(Problem::NotLinux);
    }
    match method {
        LaunchMethod::Wine { .. } => {}
        LaunchMethod::Proton { .. } => return Err(Problem::ProtonHasIt),
        other => return Err(Problem::Method(other.kind().to_owned())),
    }
    let dxvk = dxvk.ok_or(Problem::NoDxvk)?;
    if vulkan.loader.is_none() {
        return Err(Problem::NoLoader);
    }
    if vulkan.drivers.is_empty() {
        return Err(Problem::NoDriver {
            software_only: vulkan.software_only,
        });
    }
    let target = root.join(DLL);
    if target.exists() && !ours(root) {
        return Err(Problem::Occupied(target));
    }
    Ok(dxvk)
}

/// Whether the `d3d9.dll` of `root` is the one the toolkit put there.
fn ours(root: &Path) -> bool {
    let mine = std::fs::read(root.join(STATE_DIR).join(WRITTEN)).ok();
    mine.is_some() && std::fs::read(root.join(DLL)).ok() == mine
}

/// Adds what DXVK needs to a Wine launch: `d3d9=n,b` in
/// `WINEDLLOVERRIDES` (after what is there, e.g. `dinput8=n,b` of the ASI
/// loader) and its log in [`STATE_DIR`].
pub fn apply_env(plan: &mut LaunchPlan, root: &Path) {
    let current = plan
        .env
        .iter()
        .position(|(k, _)| k == "WINEDLLOVERRIDES")
        .map(|i| plan.env.remove(i).1.to_string_lossy().into_owned())
        .or_else(|| std::env::var("WINEDLLOVERRIDES").ok())
        .unwrap_or_default();
    let mut parts: Vec<&str> = current
        .split(';')
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let names = p.split('=').next().unwrap_or("");
            !names
                .split(',')
                .any(|n| n.trim().eq_ignore_ascii_case("d3d9"))
        })
        .collect();
    parts.push("d3d9=n,b");
    plan.env
        .push(("WINEDLLOVERRIDES".into(), OsString::from(parts.join(";"))));
    let set = |plan: &mut LaunchPlan, key: &str, value: OsString| {
        if std::env::var_os(key).is_none() && !plan.env.iter().any(|(k, _)| k == key) {
            plan.env.push((key.into(), value));
        }
    };
    set(plan, "DXVK_LOG_PATH", root.join(STATE_DIR).into_os_string());
    set(plan, "DXVK_LOG_LEVEL", "warn".into());
}

/// DXVK installed in a game folder for one launch. [`DxvkInstall::restore`]
/// (or [`restore_stale`] after a crash of the toolkit) takes it away; a
/// `d3d9.dll` someone changed in between stays.
#[derive(Debug)]
pub struct DxvkInstall {
    root: PathBuf,
    source: String,
    restored: bool,
}

impl DxvkInstall {
    /// Copies `dxvk` into `root`.
    pub fn write(root: &Path, dxvk: &Dxvk) -> Result<Self> {
        restore_stale(root)?;
        let target = root.join(DLL);
        if target.exists() {
            return Err(Error::InvalidArgument(format!(
                "{} exists and was not installed by the toolkit",
                target.display()
            )));
        }
        let bytes = std::fs::read(&dxvk.dll).map_err(|e| Error::io(&dxvk.dll, e))?;
        let state = root.join(STATE_DIR);
        std::fs::create_dir_all(&state).map_err(|e| Error::io(&state, e))?;
        crate::atomic::write(&state.join(WRITTEN), &bytes)?;
        crate::atomic::write(&target, &bytes)?;
        Ok(Self {
            root: root.to_path_buf(),
            source: dxvk.source.clone(),
            restored: false,
        })
    }

    /// Where the DLL came from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Takes the DLL away; returns whether the game folder changed.
    pub fn restore(mut self) -> Result<bool> {
        self.restored = true;
        restore_stale(&self.root)
    }
}

impl Drop for DxvkInstall {
    fn drop(&mut self) {
        if !self.restored {
            let _ = restore_stale(&self.root);
        }
    }
}

/// Whether the toolkit's DXVK is in `root` now.
pub fn installed(root: &Path) -> bool {
    ours(root)
}

/// Takes away a DXVK the toolkit installed for an earlier launch. Returns
/// whether the game folder changed.
pub fn restore_stale(root: &Path) -> Result<bool> {
    let written = root.join(STATE_DIR).join(WRITTEN);
    if !written.exists() {
        return Ok(false);
    }
    let mut changed = false;
    if ours(root) {
        let target = root.join(DLL);
        std::fs::remove_file(&target).map_err(|e| Error::io(&target, e))?;
        changed = true;
    }
    match std::fs::remove_file(&written) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io(&written, e)),
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A minimal 32-bit PE: `MZ`, the header offset, `PE\0\0` and i386.
    fn pe32(tag: u8) -> Vec<u8> {
        let mut data = vec![0u8; 0x48];
        data[..2].copy_from_slice(b"MZ");
        data[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        data[0x40..0x44].copy_from_slice(b"PE\0\0");
        data[0x44..0x46].copy_from_slice(&[0x4C, 0x01]);
        data[0x47] = tag;
        data
    }

    fn elf(class: u8) -> Vec<u8> {
        vec![0x7F, b'E', b'L', b'F', class, 1, 1, 0]
    }

    fn wine() -> LaunchMethod {
        LaunchMethod::Wine {
            prefix: None,
            wine: None,
            dll_overrides: None,
        }
    }

    fn roots(home: &Path) -> SearchRoots {
        SearchRoots {
            home: Some(home.to_path_buf()),
            data_home: Some(home.join(".local/share")),
            config_home: Some(home.join(".config")),
            ..SearchRoots::default()
        }
    }

    #[test]
    fn finds_the_newest_launcher_release_then_proton() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let lutris = home.join(".local/share/lutris/runtime/dxvk");
        for v in ["v1.10.3", "v2.6.2", "v2.10"] {
            fs::create_dir_all(lutris.join(v).join("x32")).unwrap();
            fs::write(lutris.join(v).join("x32").join(DLL), pe32(1)).unwrap();
        }
        // A release without a 32-bit DLL does not count.
        fs::create_dir_all(lutris.join("v9.9/x64")).unwrap();
        let found = find_in(&roots(home), None).unwrap();
        assert_eq!(found.source, "Lutris v2.10");
        assert!(found.dll.ends_with("v2.10/x32/d3d9.dll"));

        // Without launchers: Proton of a Steam library.
        fs::remove_dir_all(home.join(".local/share/lutris")).unwrap();
        let proton = home.join(".steam/steam/steamapps/common/Proton 9.0");
        fs::create_dir_all(proton.join("files/lib/wine/dxvk/i386-windows")).unwrap();
        fs::write(
            proton.join("files/lib/wine/dxvk/i386-windows").join(DLL),
            pe32(2),
        )
        .unwrap();
        let found = find_in(&roots(home), None).unwrap();
        assert_eq!(found.source, "Proton 9.0");

        // DOCTOR_DXVK wins; a folder with x32/ is enough; a 64-bit file is not.
        let own = home.join("own");
        fs::create_dir_all(own.join("x32")).unwrap();
        fs::write(own.join("x32").join(DLL), pe32(3)).unwrap();
        assert_eq!(
            find_in(&roots(home), Some(&own)).unwrap().dll,
            own.join("x32").join(DLL)
        );
        let mut pe64 = pe32(4);
        pe64[0x44..0x46].copy_from_slice(&[0x64, 0x86]);
        fs::write(home.join("x64.dll"), pe64).unwrap();
        assert_eq!(find_in(&roots(home), Some(&home.join("x64.dll"))), None);
    }

    #[test]
    fn tells_hardware_vulkan_from_software_and_64_bit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let lib32 = root.join("usr/lib/i386-linux-gnu");
        let lib64 = root.join("usr/lib/x86_64-linux-gnu");
        let icd = root.join("usr/share/vulkan/icd.d");
        for d in [&lib32, &lib64, &icd] {
            fs::create_dir_all(d).unwrap();
        }
        let manifest = |name: &str, lib: &str| {
            fs::write(
                icd.join(name),
                format!(r#"{{"ICD": {{"library_path": "{lib}"}}}}"#),
            )
            .unwrap();
        };
        manifest("nvidia_icd.json", "libGLX_nvidia.so.0");
        manifest("lvp_icd.json", "libvulkan_lvp.so");

        // Nothing 32-bit yet.
        fs::write(lib64.join("libGLX_nvidia.so.0"), elf(2)).unwrap();
        let v = vulkan32_in(root);
        assert_eq!(v, Vulkan32::default());
        assert!(!v.usable());

        // The loader and llvmpipe only.
        fs::write(lib32.join("libvulkan.so.1"), elf(1)).unwrap();
        fs::write(lib32.join("libvulkan_lvp.so"), elf(1)).unwrap();
        let v = vulkan32_in(root);
        assert!(v.loader.is_some() && v.drivers.is_empty() && v.software_only);
        assert_eq!(
            check_with(&wine(), root, Some(fake_dxvk(root)), &v),
            Err(Problem::NoDriver {
                software_only: true
            })
        );

        // The 32-bit NVIDIA library: usable.
        fs::write(lib32.join("libGLX_nvidia.so.0"), elf(1)).unwrap();
        let v = vulkan32_in(root);
        assert_eq!(v.drivers, ["nvidia_icd.json"]);
        assert!(v.usable() && !v.software_only);
    }

    fn fake_dxvk(dir: &Path) -> Dxvk {
        let dll = dir.join("dxvk-d3d9.dll");
        fs::write(&dll, pe32(7)).unwrap();
        Dxvk {
            dll,
            source: "test".into(),
        }
    }

    fn good_vulkan() -> Vulkan32 {
        Vulkan32 {
            loader: Some("/l".into()),
            drivers: vec!["nvidia_icd.json".into()],
            software_only: false,
        }
    }

    #[test]
    fn only_wine_launches_take_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let dxvk = || Some(fake_dxvk(root));
        let vk = good_vulkan();
        assert!(check_with(&wine(), root, dxvk(), &vk).is_ok());
        let proton = LaunchMethod::Proton {
            proton: "/p".into(),
            compat_data: "/c".into(),
        };
        assert_eq!(
            check_with(&proton, root, dxvk(), &vk),
            Err(Problem::ProtonHasIt)
        );
        assert!(matches!(
            check_with(&LaunchMethod::Direct, root, dxvk(), &vk),
            Err(Problem::Method(_))
        ));
        assert_eq!(check_with(&wine(), root, None, &vk), Err(Problem::NoDxvk));
        let no_loader = Vulkan32 {
            loader: None,
            ..good_vulkan()
        };
        assert_eq!(
            check_with(&wine(), root, dxvk(), &no_loader),
            Err(Problem::NoLoader)
        );
        // A player's own d3d9.dll (another DXVK, a loader) is left alone.
        fs::write(root.join(DLL), b"player's").unwrap();
        assert_eq!(
            check_with(&wine(), root, dxvk(), &vk),
            Err(Problem::Occupied(root.join(DLL)))
        );
    }

    #[test]
    fn installs_for_one_launch_and_takes_it_away() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("game");
        fs::create_dir_all(&root).unwrap();
        let dxvk = fake_dxvk(dir.path());
        let install = DxvkInstall::write(&root, &dxvk).unwrap();
        assert!(installed(&root));
        assert_eq!(fs::read(root.join(DLL)).unwrap(), pe32(7));
        // A second launch over it (the same files) is fine after a crash.
        assert!(check_with(&wine(), &root, Some(dxvk.clone()), &good_vulkan()).is_ok());
        assert!(install.restore().unwrap());
        assert!(!root.join(DLL).exists());
        assert!(!installed(&root));

        // Dropped without restore (a crash of the launch code): taken away.
        drop(DxvkInstall::write(&root, &dxvk).unwrap());
        assert!(!root.join(DLL).exists());

        // Left behind by a crashed toolkit: restore_stale takes it away.
        let install = DxvkInstall::write(&root, &dxvk).unwrap();
        std::mem::forget(install);
        assert!(restore_stale(&root).unwrap());
        assert!(!root.join(DLL).exists());

        // Changed by someone in between: stays.
        let install = DxvkInstall::write(&root, &dxvk).unwrap();
        fs::write(root.join(DLL), b"changed").unwrap();
        assert!(!install.restore().unwrap());
        assert_eq!(fs::read(root.join(DLL)).unwrap(), b"changed");
        // And a later install refuses to overwrite it.
        assert!(DxvkInstall::write(&root, &dxvk).is_err());
    }

    #[test]
    fn the_override_joins_the_asi_loader() {
        let root = Path::new("/game");
        let mut plan = LaunchPlan {
            program: "wine".into(),
            args: vec![],
            env: vec![("WINEDLLOVERRIDES".into(), "dinput8=n,b;d3d9=b".into())],
            dir: root.into(),
            detached: false,
        };
        apply_env(&mut plan, root);
        let get = |k: &str| {
            plan.env
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.to_string_lossy().into_owned())
        };
        assert_eq!(get("WINEDLLOVERRIDES").unwrap(), "dinput8=n,b;d3d9=n,b");
        assert_eq!(
            plan.env
                .iter()
                .filter(|(k, _)| k == "WINEDLLOVERRIDES")
                .count(),
            1
        );
        if std::env::var_os("DXVK_LOG_PATH").is_none() {
            assert_eq!(get("DXVK_LOG_PATH").unwrap(), "/game/_doctor_toolkit/dxvk");
        }
    }
}
