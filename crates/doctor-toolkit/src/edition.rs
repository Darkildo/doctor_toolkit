//! Which edition of the game is installed: the executable's size, SHA-256,
//! version resource, build date and section names, the languages of
//! `text/localetable.txt`, and add-ons found next to it.
//!
//! [`KNOWN`] lists the builds verified so far. Only what could be checked
//! on a real copy goes in: the size and hash of the executable, its version
//! and build date. To add a build, run `doctor-toolkit detect DIR` on it
//! (it prints these values) and add a [`KnownEdition`] with a short name;
//! an unknown build is still described by its version and date.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::profile::GameProfile;

/// A build of the game verified on a real copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownEdition {
    /// Short stable name, e.g. `1.0.789.0-2009-10-05`.
    pub id: &'static str,
    /// For people.
    pub name: &'static str,
    pub executable_size: u64,
    pub executable_sha256: &'static str,
    /// What is known about it and how it was verified.
    pub notes: &'static str,
}

/// Builds verified so far; see the module documentation to add one.
pub const KNOWN: &[KnownEdition] = &[
    KnownEdition {
        id: "1.0.789.0-2009-10-05",
        name: "PC 1.0.789.0 (build of 2009-10-05)",
        executable_size: 13_840_384,
        executable_sha256: "31f579a80cfd14f42c0379bbfa5b5b7f151bb06ec5163593023ab5002b1f1286",
        notes: "Version 1.0.789.0, linked 2009-10-05 (the day before the Steam release, \
            app 24830), no copy-protection sections, EA Download Manager files in Support/. \
            English and Russian text and speech. All format work of doctor_toolkit is \
            checked on this build.",
    },
    KnownEdition {
        id: "1.0.764.0-2009-01-27",
        name: "PC 1.0.764.0, retail DVD (build of 2009-01-27)",
        executable_size: 46_079_728,
        executable_sha256: "a5632356c94a270dab518e0e1fa569ad40fd6520c1013cf99b023e95252ad338",
        notes: "Version 1.0.764.0, linked 2009-01-27, SecuROM sections (.securom, Stext, \
                Sdata). Installed from a DVD image with Lutris; English and Spanish text. \
                The formats are not checked on this build.",
    },
];

impl KnownEdition {
    /// The version of the build, `1.0.789.0`: how mods name the edition
    /// (`game.editions` of `mod.json`).
    pub fn version(&self) -> &'static str {
        self.id.split('-').next().unwrap_or(self.id)
    }
}

/// The edition of the game in `root` as mods name it: the version of a
/// known build (`1.0.789.0`), or the version in the executable of an
/// unknown one; `None` without an executable or a version.
pub fn game_edition(root: &Path) -> Option<String> {
    let profile = GameProfile::detect(root).unwrap_or(GameProfile::GF2_PC);
    let exe = find_file(root, profile.executable).and_then(|p| executable_info(&p))?;
    match known_edition(exe.size, &exe.sha256) {
        Some(known) => Some(known.version().to_owned()),
        None => exe.version,
    }
}

/// What the executable says about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutableInfo {
    pub file_name: String,
    pub size: u64,
    pub sha256: String,
    /// `1.0.789.0`, from the version resource.
    pub version: Option<String>,
    /// Link time of the PE header, `2009-10-05`.
    pub built: Option<String>,
    /// Names of the PE sections.
    pub sections: Vec<String>,
}

/// The installed edition.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edition {
    pub executable: Option<ExecutableInfo>,
    /// Id of the [`KnownEdition`] it matches.
    pub known: Option<String>,
    /// Copy protection recognised by section names (`SecuROM`, `Steam DRM`).
    pub protection: Option<String>,
    /// Languages switched on in `text/localetable.txt`.
    pub languages: Vec<String>,
    /// Add-ons in the game folder: script loaders, other modding tools.
    pub extras: Vec<String>,
}

impl Edition {
    /// Looks at the game installed in `root`.
    pub fn identify(root: &Path) -> Self {
        let profile = GameProfile::detect(root).unwrap_or(GameProfile::GF2_PC);
        let executable = find_file(root, profile.executable).and_then(|p| executable_info(&p));
        let known = executable
            .as_ref()
            .and_then(|e| known_edition(e.size, &e.sha256));
        let protection = executable.as_ref().and_then(|e| protection(&e.sections));
        Self {
            known: known.map(|k| k.id.to_owned()),
            executable,
            protection,
            languages: languages(root),
            extras: extras(root),
        }
    }

    /// The known edition, if any.
    pub fn known_edition(&self) -> Option<&'static KnownEdition> {
        let id = self.known.as_deref()?;
        KNOWN.iter().find(|k| k.id == id)
    }

    /// A short name: the known edition, or the version and date of an
    /// unknown build.
    pub fn name(&self) -> String {
        if let Some(known) = self.known_edition() {
            return known.name.to_owned();
        }
        match &self.executable {
            None => "unknown (no game executable)".to_owned(),
            Some(exe) => {
                let mut text = format!(
                    "unknown build {}",
                    exe.version.as_deref().unwrap_or("without version")
                );
                if let Some(built) = &exe.built {
                    text += &format!(" of {built}");
                }
                text
            }
        }
    }

    /// One line: name, protection, languages, add-ons.
    pub fn summary(&self) -> String {
        let mut parts = vec![self.name()];
        if let Some(p) = &self.protection {
            parts.push(p.clone());
        }
        if !self.languages.is_empty() {
            parts.push(self.languages.join(", "));
        }
        parts.extend(self.extras.iter().cloned());
        parts.join("; ")
    }
}

/// The known edition with this executable.
pub fn known_edition(size: u64, sha256: &str) -> Option<&'static KnownEdition> {
    KNOWN
        .iter()
        .find(|k| k.executable_size == size && k.executable_sha256.eq_ignore_ascii_case(sha256))
}

/// The name of the build with this executable: a known edition, or `None`.
pub fn edition_name(size: u64, sha256: &str) -> Option<&'static str> {
    known_edition(size, sha256).map(|k| k.name)
}

fn find_file(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
}

/// Largest executable read (the game's is 14 MB).
const MAX_EXECUTABLE: u64 = 256 << 20;

/// Size, hash and PE details of an executable.
pub fn executable_info(path: &Path) -> Option<ExecutableInfo> {
    let meta = fs::metadata(path).ok()?;
    if meta.len() > MAX_EXECUTABLE {
        return None;
    }
    let data = fs::read(path).ok()?;
    let pe = pe_details(&data);
    Some(ExecutableInfo {
        file_name: path.file_name()?.to_string_lossy().into_owned(),
        size: data.len() as u64,
        sha256: crate::workspace::store::sha256_hex(&data),
        version: pe.as_ref().and_then(|p| p.version.clone()),
        built: pe
            .as_ref()
            .and_then(|p| p.timestamp)
            .filter(|&t| t > 0)
            .map(|t| {
                let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(t));
                crate::report::timestamp(time)[..10].to_owned()
            }),
        sections: pe.map(|p| p.sections).unwrap_or_default(),
    })
}

struct PeDetails {
    timestamp: Option<u32>,
    sections: Vec<String>,
    version: Option<String>,
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// Header fields of a PE file; `None` when it is not one.
fn pe_details(data: &[u8]) -> Option<PeDetails> {
    if data.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32_at(data, 0x3c)? as usize;
    if data.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let count = usize::from(u16_at(data, pe + 6)?).min(96);
    let timestamp = u32_at(data, pe + 8);
    let optional = usize::from(u16_at(data, pe + 20)?);
    let table = pe + 24 + optional;
    let sections = (0..count)
        .filter_map(|i| {
            let raw = data.get(table + i * 40..table + i * 40 + 8)?;
            let end = raw.iter().position(|&b| b == 0).unwrap_or(8);
            Some(String::from_utf8_lossy(&raw[..end]).into_owned())
        })
        .collect();
    Some(PeDetails {
        timestamp,
        sections,
        version: fixed_file_version(data),
    })
}

/// `VS_FIXEDFILEINFO.dwFileVersion` as `a.b.c.d`.
fn fixed_file_version(data: &[u8]) -> Option<String> {
    const SIGNATURE: [u8; 4] = 0xFEEF_04BDu32.to_le_bytes();
    let at = data.windows(4).position(|w| w == SIGNATURE)?;
    let ms = u32_at(data, at + 8)?;
    let ls = u32_at(data, at + 12)?;
    Some(format!(
        "{}.{}.{}.{}",
        ms >> 16,
        ms & 0xffff,
        ls >> 16,
        ls & 0xffff
    ))
}

/// Copy protection recognised by section names.
fn protection(sections: &[String]) -> Option<String> {
    let has = |name: &str| sections.iter().any(|s| s.eq_ignore_ascii_case(name));
    if has(".securom") || has(".sec") {
        Some("SecuROM".to_owned())
    } else if has(".bind") {
        Some("Steam DRM".to_owned())
    } else {
        None
    }
}

/// Names of the languages switched on in `text/localetable.txt`: after a
/// header of six lines, six lines per language: code, fallback, name,
/// switched on (`1`), region, locale.
fn languages(root: &Path) -> Vec<String> {
    let Some(path) = find_file(root, "text").and_then(|d| find_file(&d, "localetable.txt")) else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    lines
        .get(6..)
        .unwrap_or_default()
        .chunks(6)
        .filter(|c| c.len() == 6 && c[3] == "1" && !c[2].is_empty())
        .map(|c| c[2].to_owned())
        .take(32)
        .collect()
}

/// Add-ons next to the game.
fn extras(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if find_file(root, "dinput8.dll").is_some() {
        let scripts = find_file(root, "scripts")
            .and_then(|d| fs::read_dir(d).ok())
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        e.path()
                            .extension()
                            .is_some_and(|x| x.eq_ignore_ascii_case("asi"))
                    })
                    .count()
            })
            .unwrap_or(0);
        out.push(format!("ASI loader (dinput8.dll) with {scripts} script(s)"));
    }
    if root.join(crate::mods::integrity::OLD_TOOLKIT_DIR).is_dir() {
        out.push("old toolkit folder (_toolkit)".to_owned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal PE file with two sections and a version resource.
    fn fake_pe(sections: &[&str], version: [u16; 4], timestamp: u32) -> Vec<u8> {
        let mut data = vec![0u8; 0x200];
        data[..2].copy_from_slice(b"MZ");
        data[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        data[0x80..0x84].copy_from_slice(b"PE\0\0");
        data[0x86..0x88].copy_from_slice(&(sections.len() as u16).to_le_bytes());
        data[0x88..0x8c].copy_from_slice(&timestamp.to_le_bytes());
        data[0x94..0x96].copy_from_slice(&0xe0u16.to_le_bytes());
        let table = 0x80 + 24 + 0xe0;
        for (i, name) in sections.iter().enumerate() {
            data[table + i * 40..table + i * 40 + name.len()].copy_from_slice(name.as_bytes());
        }
        data.extend_from_slice(&0xFEEF_04BDu32.to_le_bytes());
        data.extend_from_slice(&0x0001_0000u32.to_le_bytes());
        let ms = (u32::from(version[0]) << 16) | u32::from(version[1]);
        let ls = (u32::from(version[2]) << 16) | u32::from(version[3]);
        data.extend_from_slice(&ms.to_le_bytes());
        data.extend_from_slice(&ls.to_le_bytes());
        data
    }

    #[test]
    fn reads_pe_details() {
        let data = fake_pe(&[".text", ".securom"], [1, 0, 766, 0], 1_234_567_890);
        let pe = pe_details(&data).unwrap();
        assert_eq!(pe.sections, [".text", ".securom"]);
        assert_eq!(pe.version.as_deref(), Some("1.0.766.0"));
        assert_eq!(protection(&pe.sections).as_deref(), Some("SecuROM"));
        assert!(pe_details(b"MZ").is_none());
        assert!(pe_details(b"not a PE").is_none());
        for cut in 0..data.len() {
            let _ = pe_details(&data[..cut]);
        }
    }

    #[test]
    fn identifies_an_unknown_build_with_languages_and_add_ons() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let exe = fake_pe(&[".text", ".rsrc"], [1, 0, 766, 0], 1_234_567_890);
        fs::write(root.join("godfather2.exe"), &exe).unwrap();
        fs::create_dir_all(root.join("text")).unwrap();
        fs::write(
            root.join("text/localetable.txt"),
            "7\n454107DA\nntsc_en\nNOT SET\nNOT SET\nNOT SET\n\
             en\nen\nEnglish\n1\n(none)\nen_US\n\
             ru\nru\nRussian\n1\nRU\nru\n\
             cs\nen\nStringIDs\n0\n(none)\nen_US\n",
        )
        .unwrap();
        fs::write(root.join("dinput8.dll"), b"x").unwrap();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::write(root.join("scripts/a.asi"), b"x").unwrap();
        let edition = Edition::identify(root);
        let exe_info = edition.executable.as_ref().unwrap();
        assert_eq!(exe_info.size, exe.len() as u64);
        assert_eq!(exe_info.built.as_deref(), Some("2009-02-13"));
        assert_eq!(edition.known, None);
        assert_eq!(edition.languages, ["English", "Russian"]);
        assert_eq!(edition.name(), "unknown build 1.0.766.0 of 2009-02-13");
        assert_eq!(
            edition.summary(),
            "unknown build 1.0.766.0 of 2009-02-13; English, Russian; \
             ASI loader (dinput8.dll) with 1 script(s)"
        );
    }

    #[test]
    fn known_editions_are_found_by_size_and_hash() {
        let k = &KNOWN[0];
        assert_eq!(
            known_edition(k.executable_size, &k.executable_sha256.to_uppercase()),
            Some(k)
        );
        assert_eq!(
            known_edition(k.executable_size + 1, k.executable_sha256),
            None
        );
        for k in KNOWN {
            assert_eq!(k.executable_sha256.len(), 64);
            assert_eq!(KNOWN.iter().filter(|o| o.id == k.id).count(), 1);
        }
    }
}
