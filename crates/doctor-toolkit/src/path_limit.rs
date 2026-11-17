//! The limit of the game on the length of the path to its folder (stage 77).
//!
//! The game opens a stream by the text `<game folder>/<stream path>`, which
//! its file library builds with `sprintf` in a 256-byte buffer on the stack
//! of the file thread (exe 1.0.789.0, `0x00AC37C0`, called by the "open"
//! operation `0x00AC3AE0`). Nothing checks the length: a longer text
//! overwrites the return address of that function and the game crashes at an
//! address made of the letters of the path (`0x6E756F70` is "poun" of
//! "compound"; docs/analysis/story.md §9).
//!
//! The text fits when `folder + 1 + stream <= 255`: the folder as the game
//! sees it (under Wine `Z:` and the Unix path), one separator, the path of
//! the stream below the folder with `.str`.

use std::path::Path;

use crate::game::GameInstance;

/// Size of the buffer of the game, with the terminating zero.
pub const BUFFER: usize = 256;

/// The longest stream path of the unchanged game:
/// `gf2/miami/global/objectives/chapter70-mission11-rothsassassination/`
/// `m11_common_stream_including_restart/m11_common_inc_restart_folderstream.str`.
pub const LONGEST_GAME_STREAM: usize = 142;

/// The longest folder path every stream of the unchanged game opens from.
pub const LONGEST_SAFE_FOLDER: usize = BUFFER - 1 - 1 - LONGEST_GAME_STREAM;

/// What the limit says about one game folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathCheck {
    /// Length of the folder path as the game sees it.
    pub folder: usize,
    /// The longest stream path below the folder (with `.str`) and its length.
    pub stream: String,
    pub stream_len: usize,
}

impl PathCheck {
    /// Length of the longest text the game builds.
    pub fn longest(&self) -> usize {
        self.folder + 1 + self.stream_len
    }

    /// By how many characters the longest text does not fit; 0 when it fits.
    pub fn over(&self) -> usize {
        self.longest().saturating_sub(BUFFER - 1)
    }

    /// How many streams of `lens` (lengths of stream paths) do not fit.
    pub fn too_long(&self, lens: impl IntoIterator<Item = usize>) -> usize {
        lens.into_iter()
            .filter(|len| self.folder + 1 + len > BUFFER - 1)
            .count()
    }
}

/// Length of a folder path as the game sees it: the path itself on Windows;
/// elsewhere the game runs under Wine, whose drive `Z:` is the root of the
/// file system, so the path gains the two characters of the drive.
pub fn seen_len(folder: &Path) -> usize {
    let folder = std::path::absolute(folder).unwrap_or_else(|_| folder.to_path_buf());
    let len = folder
        .to_string_lossy()
        .trim_end_matches(['/', '\\'])
        .chars()
        .count();
    if cfg!(windows) { len } else { len + 2 }
}

/// The longest stream path of a game (with `.str`) and its length.
pub fn longest_stream(game: &GameInstance) -> Option<(String, usize)> {
    game.streams()
        .iter()
        .map(|file| (file.rel_path.clone(), file.rel_path.chars().count()))
        .max_by_key(|(_, len)| *len)
}

/// Checks the folder of a game against the limit.
pub fn check(game: &GameInstance) -> Option<PathCheck> {
    let (stream, stream_len) = longest_stream(game)?;
    Some(PathCheck {
        folder: seen_len(game.root()),
        stream,
        stream_len,
    })
}

/// One line for the user when streams of the game do not fit; `None` when all fit.
pub fn warning(game: &GameInstance) -> Option<String> {
    let check = check(game)?;
    if check.over() == 0 {
        return None;
    }
    let count = check.too_long(game.streams().iter().map(|f| f.rel_path.chars().count()));
    Some(format!(
        "the path of the game folder is too long: {} character(s) as the game sees it, and with it \
         {count} stream(s) do not fit the game's 255-character file path (the longest, {}, by {}). \
         The game crashes when it opens such a stream; a folder path of up to {} character(s) is safe",
        check.folder,
        check.stream,
        check.over(),
        (BUFFER - 2).saturating_sub(check.stream_len),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(folder: usize, stream_len: usize) -> PathCheck {
        PathCheck {
            folder,
            stream: String::new(),
            stream_len,
        }
    }

    #[test]
    fn the_text_fits_up_to_255_characters() {
        assert_eq!(LONGEST_SAFE_FOLDER, 112);
        assert_eq!(with(112, 142).over(), 0);
        assert_eq!(with(113, 142).over(), 1);
        // The bench shadow of stage 99 (136 characters) and the stream of checkpoint 326 (132).
        assert_eq!(with(136, 132).longest(), 269);
        assert_eq!(with(136, 132).over(), 14);
        assert_eq!(with(136, 132).too_long([132, 118, 119, 20]), 2);
    }

    #[cfg(not(windows))]
    #[test]
    fn a_folder_is_measured_as_wine_shows_it() {
        assert_eq!(seen_len(Path::new("/games/gf2/")), "Z:/games/gf2".len());
    }
}
