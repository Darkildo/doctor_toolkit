//! Operations on the sound of the game shared by the editor, the command
//! line and the MCP server: the streamed sounds and speech of the `.viv`
//! archives (list, read, replace, subtitles), the records of the speech
//! event table (`get`), the mix categories of the audio configuration
//! (`auc`) and the channels of the mixer snapshots (`mix`).
//!
//! Every function that changes something records one undoable operation of
//! the workspace.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::container::big::{BigArchive, find_archives, is_tool_entry, normalize_name};
use crate::game::GameInstance;
use crate::resource::auc::AudioConfig;
use crate::resource::audio::Sample;
use crate::resource::audio_encode::encode_snu_like;
use crate::resource::mix::Mix;
use crate::resource::speech;
use crate::resource::video::input::Pcm;
use crate::sound_edit;
use crate::workspace::{FILES, FileKey, Workspace};
use crate::{Error, Result};

/// Where a resource is: stream index and resource index.
pub type At = (usize, usize);

/// One file of the `.viv` archives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveSound {
    /// Entry name as the archive stores it
    /// (`audiostreams\e_screen\title_screen_01.exa.snu`).
    pub name: String,
    /// File name of the archive that the game takes the entry from.
    pub archive: String,
    /// Size of the entry in the archive, bytes.
    pub size: u64,
}

/// The files of the `.viv` archives of a game directory, each once (the
/// game takes a file from the first archive that has it), without the
/// toolkit's own entries. `filter` keeps the names that contain it (case
/// ignored, `/` and `\` equal).
pub fn archive_sounds(game_root: &Path, filter: Option<&str>) -> Result<Vec<ArchiveSound>> {
    let filter = filter.map(normalize_name).filter(|f| !f.is_empty());
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for path in find_archives(game_root)? {
        let archive = BigArchive::open(&path)?;
        let file = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        for entry in &archive.entries {
            let name = normalize_name(&entry.name);
            if is_tool_entry(&entry.name) || !seen.insert(name.clone()) {
                continue;
            }
            if filter.as_ref().is_some_and(|f| !name.contains(f)) {
                continue;
            }
            found.push(ArchiveSound {
                name: entry.name.clone(),
                archive: file.clone(),
                size: entry.size,
            });
        }
    }
    Ok(found)
}

/// What a streamed sound holds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SoundInfo {
    pub channels: u8,
    pub sample_rate: u32,
    pub seconds: f64,
    /// `(language, text)` of the subtitle of a speech file; `None` when the
    /// file has no subtitles.
    pub subtitles: Option<Vec<(String, String)>>,
}

/// Reads the header and the subtitles of a streamed sound (`.snu`).
pub fn sound_info(data: &[u8]) -> Result<SoundInfo> {
    let sample = Sample::parse_snu(data)?;
    let subtitles = speech::subtitles(data).ok().flatten().map(|table| {
        table
            .languages
            .iter()
            .zip(&table.strings)
            .map(|(language, column)| {
                (
                    language.clone(),
                    column.first().cloned().unwrap_or_default(),
                )
            })
            .collect()
    });
    Ok(SoundInfo {
        channels: sample.header.channels,
        sample_rate: sample.header.sample_rate,
        seconds: sample.duration(),
        subtitles,
    })
}

/// The data of archive entry `name`: as the workspace has it, or the
/// game's original without a workspace.
pub fn archive_sound_data(
    game: &GameInstance,
    ws: Option<&Workspace>,
    name: &str,
) -> Result<Vec<u8>> {
    let key = FileKey::archive_entry(name.trim());
    match ws {
        Some(ws) => ws.current_file(game, &key),
        None => crate::workspace::files::original_data(game.root(), &key),
    }
}

/// Replaces the streamed sound or speech file `name` of the archives with
/// `pcm`, converted to the channels, sample rate and loop of the sound it
/// replaces; the subtitles of a speech file stay. Returns the new sound.
pub fn replace_archive_sound(
    game: &GameInstance,
    ws: &mut Workspace,
    name: &str,
    pcm: &Pcm,
) -> Result<SoundInfo> {
    if pcm.frames() == 0 {
        return Err(Error::InvalidArgument("the sound is empty".into()));
    }
    let key = FileKey::archive_entry(name.trim());
    let original = ws.current_file(game, &key)?;
    let data = encode_snu_like(pcm, &original)?;
    let info = sound_info(&data)?;
    let label = format!("Import sound {}", key.name());
    ws.set_file(game, &key, data, &label)?;
    Ok(info)
}

/// Sets the subtitle of the speech file `name` in the given languages
/// (`(code, text)`, codes as in the file: `en`, `fr`, …).
pub fn set_subtitles(
    game: &GameInstance,
    ws: &mut Workspace,
    name: &str,
    texts: &[(String, String)],
) -> Result<()> {
    if texts.is_empty() {
        return Err(Error::InvalidArgument("give the texts by language".into()));
    }
    let key = FileKey::archive_entry(name.trim());
    let data = ws.current_file(game, &key)?;
    let mut table = speech::subtitles(&data)?
        .ok_or_else(|| Error::InvalidArgument(format!("{} has no subtitles", key.name())))?;
    for (lang, text) in texts {
        let col = speech::language_column(&table, lang).ok_or_else(|| {
            Error::InvalidArgument(format!(
                "no `{lang}` column ({})",
                table.languages.join(" ")
            ))
        })?;
        let cell = table.strings[col].first_mut().ok_or_else(|| {
            Error::InvalidArgument(format!("{} has an empty subtitle table", key.name()))
        })?;
        cell.clone_from(text);
    }
    let new = speech::with_subtitles(&data, &table)?;
    let label = format!(
        "Subtitle of {}",
        key.name().rsplit('\\').next().unwrap_or_default()
    );
    ws.set_file(game, &key, new, &label)?;
    Ok(())
}

/// The stream name that operations on archive files report.
pub fn files_stream() -> String {
    FILES.to_owned()
}

// The tables of speech and of the mixer are edited by `sound_edit` (the
// command line and the MCP tools call it); the functions below give the
// Sound window of the editor the same operations with the parameters its
// forms have.

/// What to set in records of the speech event table; `None` keeps the
/// value (a new record gets the common values of the game).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EventEdit {
    /// Seconds before the event may be said again.
    pub cooldown: Option<f32>,
    /// Priority (300–500 in the game).
    pub priority: Option<u32>,
    pub flags: Option<u32>,
    pub rank: Option<i32>,
}

/// Outcome of [`set_speech_events`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventsReport {
    /// `(event, whether its record is new)`.
    pub events: Vec<(String, bool)>,
    /// The stream of the table.
    pub stream: String,
}

/// Adds a record of the speech event table for each named event that has
/// none and applies `edit` to the records of all of them
/// ([`sound_edit::set_speech_events`]).
pub fn set_speech_events(
    game: &GameInstance,
    ws: &mut Workspace,
    names: &[String],
    edit: &EventEdit,
) -> Result<EventsReport> {
    let (events, stream) = sound_edit::set_speech_events(
        game,
        ws,
        &sound_edit::SetSpeechEvents {
            events: names.to_vec(),
            cooldown: edit.cooldown,
            priority: edit.priority,
            flags: edit.flags,
            rank: edit.rank,
        },
    )?;
    Ok(EventsReport { events, stream })
}

/// Names of the speech events by hash ([`sound_edit::event_names`]).
pub fn speech_event_names(
    game: &GameInstance,
    ws: &Workspace,
) -> Result<std::collections::HashMap<u32, String>> {
    sound_edit::event_names(game, ws)
}

/// The audio configurations (`auc`) of the game as the workspace has them.
pub fn audio_configs(game: &GameInstance, ws: &Workspace) -> Result<Vec<(At, AudioConfig)>> {
    sound_edit::audio_configs(game, ws)
}

/// Adds mix category `name` under `parent` to every audio configuration of
/// the game ([`sound_edit::add_mix_category`]). Returns the streams changed.
pub fn add_mix_category(
    game: &GameInstance,
    ws: &mut Workspace,
    name: &str,
    parent: &str,
) -> Result<Vec<String>> {
    sound_edit::add_mix_category(game, ws, name, parent)
}

/// The mixer snapshots (`mix`) of the game as the workspace has them, every
/// copy of every stream.
pub fn mixes(game: &GameInstance, ws: &Workspace) -> Result<Vec<(At, Mix)>> {
    sound_edit::mixes(game, ws)
}

/// What to set in the channel of a mix category; `None` keeps the value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MixEdit {
    /// Volume of the category in the snapshot, 0–1.
    pub volume: Option<f32>,
    /// Low-pass frequency, Hz.
    pub frequency: Option<u32>,
    /// The two times of the channel, seconds.
    pub time: Option<f32>,
}

impl MixEdit {
    pub fn is_empty(&self) -> bool {
        self.volume.is_none() && self.frequency.is_none() && self.time.is_none()
    }
}

/// Changes the channel of `category` in every copy of mix `name`
/// ([`sound_edit::edit_mix`]). Returns the streams changed.
pub fn set_mix(
    game: &GameInstance,
    ws: &mut Workspace,
    name: &str,
    category: &str,
    edit: &MixEdit,
) -> Result<Vec<String>> {
    sound_edit::edit_mix(
        game,
        ws,
        &sound_edit::MixEdit {
            name: name.to_owned(),
            category: category.to_owned(),
            volume: edit.volume,
            frequency: edit.frequency,
            time: edit.time,
        },
    )
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;
    use crate::hash::string_id;
    use crate::resource::mix::LEVEL_TIMES;
    use crate::testing as mini;
    use crate::voice;

    fn game() -> (tempfile::TempDir, GameInstance) {
        let dir = tempfile::tempdir().unwrap();
        mini::write_game(dir.path()).unwrap();
        mini::write_sound(dir.path()).unwrap();
        let game = GameInstance::open(dir.path()).unwrap();
        (dir, game)
    }

    #[test]
    fn archive_sounds_are_listed_read_replaced_and_subtitled() {
        let (_dir, game) = game();
        let all = archive_sounds(game.root(), None).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].archive, "GODFATH0.viv");
        let music = archive_sounds(game.root(), Some("Music/")).unwrap();
        assert_eq!(music.len(), 1);
        assert_eq!(music[0].name, mini::MUSIC_ENTRY);

        let mut ws = Workspace::for_game(&game, None).unwrap();
        let data = archive_sound_data(&game, Some(&ws), mini::SPEECH_ENTRY).unwrap();
        assert_eq!(
            data,
            archive_sound_data(&game, None, "AudioStreams/d_one/d_one.exa.snu").unwrap()
        );
        let info = sound_info(&data).unwrap();
        assert_eq!((info.channels, info.sample_rate), (1, 32_000));
        assert!((info.seconds - 0.5).abs() < 0.01, "{}", info.seconds);
        let texts = info.subtitles.unwrap();
        assert!(texts.contains(&("EN".to_owned(), "Hello there.".to_owned())));
        let plain = archive_sound_data(&game, None, mini::MUSIC_ENTRY).unwrap();
        assert_eq!(sound_info(&plain).unwrap().subtitles, None);

        // A longer sound at another rate: converted, the subtitles stay.
        let tone = mini::tone(1.0, 44_100);
        let new = replace_archive_sound(&game, &mut ws, mini::SPEECH_ENTRY, &tone).unwrap();
        assert_eq!((new.channels, new.sample_rate), (1, 32_000));
        assert!((new.seconds - 1.0).abs() < 0.01);
        assert_eq!(new.subtitles.as_ref(), Some(&texts));
        // The subtitle in two languages, one operation.
        set_subtitles(
            &game,
            &mut ws,
            mini::SPEECH_ENTRY,
            &[
                ("en".to_owned(), "Good evening.".to_owned()),
                ("ES".to_owned(), "Buenas noches.".to_owned()),
            ],
        )
        .unwrap();
        assert_eq!(ws.operations().len(), 2);
        let data = archive_sound_data(&game, Some(&ws), mini::SPEECH_ENTRY).unwrap();
        let texts = sound_info(&data).unwrap().subtitles.unwrap();
        assert!(texts.contains(&("EN".to_owned(), "Good evening.".to_owned())));
        assert!(texts.contains(&("ES".to_owned(), "Buenas noches.".to_owned())));
        // Refusals record nothing.
        let none: [(String, String); 0] = [];
        assert!(set_subtitles(&game, &mut ws, mini::SPEECH_ENTRY, &none).is_err());
        let xx = [("xx".to_owned(), "?".to_owned())];
        assert!(set_subtitles(&game, &mut ws, mini::SPEECH_ENTRY, &xx).is_err());
        let en = [("en".to_owned(), "?".to_owned())];
        assert!(set_subtitles(&game, &mut ws, mini::MUSIC_ENTRY, &en).is_err());
        assert!(replace_archive_sound(&game, &mut ws, "no\\such.snu", &tone).is_err());
        assert_eq!(ws.operations().len(), 2);
    }

    #[test]
    fn events_categories_and_mixes_change_in_one_operation_each() {
        let (_dir, game) = game();
        let mut ws = Workspace::for_game(&game, None).unwrap();
        let edit = EventEdit {
            cooldown: Some(12.5),
            priority: Some(450),
            ..EventEdit::default()
        };
        let names = [mini::SPEECH_EVENT.to_owned(), "DOCTOR_HELLO".to_owned()];
        let report = set_speech_events(&game, &mut ws, &names, &edit).unwrap();
        assert_eq!(report.stream, mini::SOUND);
        assert_eq!(
            report.events,
            [(names[0].clone(), false), (names[1].clone(), true)]
        );
        let (_, table) = voice::event_table(&game, &ws).unwrap();
        assert_eq!(table.events.len(), 2);
        for name in &names {
            let e = table.find(string_id(name)).unwrap();
            assert_eq!((e.cooldown, e.priority), (12.5, 450));
        }
        assert!(set_speech_events(&game, &mut ws, &[], &edit).is_err());
        assert!(set_speech_events(&game, &mut ws, &["bad name".to_owned()], &edit).is_err());

        assert_eq!(
            add_mix_category(&game, &mut ws, "DOCTOR_VOICE", "DIALOG").unwrap(),
            [mini::SOUND]
        );
        assert!(add_mix_category(&game, &mut ws, "X", "NOPE").is_err());
        let configs = audio_configs(&game, &ws).unwrap();
        assert_eq!(configs[0].1.categories.len(), 3);

        // A channel of the mix, and a channel for the new category.
        let volume = |v| MixEdit {
            volume: Some(v),
            ..MixEdit::default()
        };
        set_mix(&game, &mut ws, mini::MIX_NAME, "DIALOG", &volume(0.25)).unwrap();
        let edit = MixEdit {
            volume: Some(0.75),
            frequency: Some(8000),
            time: Some(2.0),
        };
        set_mix(&game, &mut ws, mini::MIX_NAME, "DOCTOR_VOICE", &edit).unwrap();
        let all = mixes(&game, &ws).unwrap();
        let mix = &all[0].1;
        assert_eq!(mix.channels.len(), 2);
        assert_eq!(mix.channel(string_id("DIALOG")).unwrap().volume(), 0.25);
        let new = mix.channel(string_id("DOCTOR_VOICE")).unwrap();
        assert_eq!((new.volume(), new.frequency), (0.75, 8000));
        assert_eq!(new.levels[LEVEL_TIMES[0]], 2.0);
        assert!(set_mix(&game, &mut ws, mini::MIX_NAME, "NOPE", &volume(0.5)).is_err());
        assert!(set_mix(&game, &mut ws, "none.mix", "DIALOG", &volume(0.5)).is_err());
        assert!(set_mix(&game, &mut ws, mini::MIX_NAME, "DIALOG", &volume(1.5)).is_err());
        assert!(
            set_mix(
                &game,
                &mut ws,
                mini::MIX_NAME,
                "DIALOG",
                &MixEdit::default()
            )
            .is_err()
        );
        assert_eq!(ws.operations().len(), 4);
        ws.undo().unwrap();
        assert_eq!(mixes(&game, &ws).unwrap()[0].1.channels.len(), 1);
    }
}
