//! Per-game format parameters.
//!
//! Formats differ between games and platforms: container backend, table of
//! contents version, byte order. A [`GameProfile`] gathers those choices in
//! one place so that format code does not branch on the game by itself.

use std::path::Path;

use crate::io::Endian;

/// Games built on the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Game {
    /// The Godfather II (2009).
    Godfather2,
}

/// Target platform of a game build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    Pc,
}

/// Container backend used for `.str` files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerKind {
    /// Block container with `SHOC` blocks, see [`crate::container::ustream`].
    UStream,
}

/// Format parameters of one game build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameProfile {
    pub game: Game,
    pub platform: Platform,
    /// Human-readable name.
    pub name: &'static str,
    pub endian: Endian,
    pub container: ContainerKind,
    pub toc_version: u32,
    /// Name of the game executable, used to recognise a game directory.
    pub executable: &'static str,
}

impl GameProfile {
    /// The Godfather II for Windows.
    pub const GF2_PC: Self = Self {
        game: Game::Godfather2,
        platform: Platform::Pc,
        name: "The Godfather II (PC)",
        endian: Endian::Little,
        container: ContainerKind::UStream,
        toc_version: 10,
        executable: "godfather2.exe",
    };

    /// All known profiles.
    pub const ALL: &'static [Self] = &[Self::GF2_PC];

    /// Recognises the game installed in `root` by its executable.
    pub fn detect(root: &Path) -> Option<Self> {
        let entries = std::fs::read_dir(root).ok()?;
        let files: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .map(|name| name.to_ascii_lowercase())
            .collect();
        Self::ALL
            .iter()
            .copied()
            .find(|profile| files.iter().any(|f| f == profile.executable))
    }
}
