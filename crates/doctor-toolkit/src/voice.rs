//! New voice lines: a recorded line made from a sound file and everything
//! the game needs to say it.
//!
//! The game says a speech event of a speaker (`docs/formats/dialogues.md`):
//!
//! 1. the event must have a record in the event table `get` — the game reads
//!    it before it speaks;
//! 2. a chatter set (`chv`) of the speaker lists the lines of the event; a
//!    line holds a 128-bit sound id and the id of its subtitle;
//! 3. a loaded speech bank (`smb`) has an entry with that id: the file name
//!    and the playback settings (mix category, distances);
//! 4. the file is in the `.viv` archives at the path built from the name
//!    ([`crate::resource::smb::stream_path`]); its metadata carries the
//!    subtitle in every language.
//!
//! [`add_line`] makes all four as one undoable operation of the workspace:
//! the encoded sound goes into the overlay archive as a new entry, the bank
//! gets an entry modelled on a line the speaker already has, the chatter set
//! gets the line (and the event when it is new), and a new event gets a
//! record in `get`.

use rayon::prelude::*;
use serde::Serialize;

use crate::game::{GameInstance, StreamFile};
use crate::hash::string_id;
use crate::resource::audio::Sample;
use crate::resource::audio_encode::{SampleSpec, build_snu, fit_pcm};
use crate::resource::chv::{ChatterSet, TYPE_CHV, event_hash};
use crate::resource::get::{SpeechEvent, SpeechEventTable, TYPE_GET};
use crate::resource::smb::{
    ENTRY_TAG, FLAG_LOCALISED, FLAG_SNU, SpeechBank, SpeechBankEntry, TYPE_SMB, VERSION, name_hash,
    stream_path,
};
use crate::resource::speech;
use crate::resource::video::input::Pcm;
use crate::workspace::{FileKey, ResourceEdit, ResourceKey, StorageChoice, Workspace};
use crate::{Error, Result};

/// The stream whose speech bank is always loaded with a level.
pub const GLOBAL_STREAM: &str = "gf2/globals_global";
/// Sample rate of the speech of the game.
pub const SPEECH_RATE: u32 = 32_000;
/// Longest line name: `d_<name>.exa` must fit the 40-byte name of a bank entry.
pub const MAX_NAME: usize = 33;

/// A voice line to add.
#[derive(Debug, Clone)]
pub struct NewVoiceLine {
    /// The chatter set (`chv`) of the speaker.
    pub chv: ResourceKey,
    /// Speech event name (`F2F_ENJOY_FLIGHT`, or a new one).
    pub event: String,
    /// Line name: lower-case letters, digits and `_`
    /// (`doc1_xxx_0000001`); the file becomes `d_<name>.exa.snu`.
    pub name: String,
    /// `(language code, text)`; see [`speech::new_table`].
    pub subtitles: Vec<(String, String)>,
    /// Weight of the line among the lines of its event (1..3 in the game).
    pub weight: f32,
    /// Drop the other lines of the event, so that the event always says
    /// this one.
    pub only: bool,
    /// Mix category of the line by name (`DIALOG`); `None` keeps the
    /// category of the model line.
    pub category: Option<String>,
}

/// What [`add_line`] did.
#[derive(Debug, Clone, Serialize)]
pub struct VoiceLineReport {
    /// The sound id of the new line.
    pub sound: u32,
    /// File name in the bank (`d_doc1_xxx_0000001.exa`).
    pub file: String,
    /// The new entry of the archives.
    pub entry: String,
    /// The speech bank that got the entry.
    pub bank: String,
    /// The file the playback settings were taken from, if any.
    pub model: Option<String>,
    /// The event is new in the chatter set.
    pub new_event: bool,
    /// The event got a record in `get`.
    pub new_record: bool,
    pub seconds: f64,
    pub sample_rate: u32,
    pub channels: u8,
    /// Streams the operation changed.
    pub streams: Vec<String>,
}

/// Checks a line name, see [`NewVoiceLine::name`].
pub fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "bad line name `{name}`: use 1 to {MAX_NAME} lower-case letters, digits and `_`"
        )))
    }
}

/// Checks an event name: letters, digits and `_`; the game compares names
/// without case.
pub fn check_event(event: &str) -> Result<()> {
    let ok = !event.is_empty()
        && event.len() <= 63
        && event
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "bad event name `{event}`: use 1 to 63 letters, digits and `_`"
        )))
    }
}

/// Where a resource is: stream index and resource index.
type At = (usize, usize);

/// Resources of the given types in every stream of the game, as
/// `(type id, stream, resource)`.
pub fn locate(game: &GameInstance, types: &[u32]) -> Vec<(u32, usize, usize)> {
    let mut found: Vec<(u32, usize, usize)> = (0..game.streams().len())
        .into_par_iter()
        .flat_map_iter(|s| {
            let resources = StreamFile::open(&game.streams()[s].path)
                .map(|stream| {
                    stream
                        .layout()
                        .resources
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| types.contains(&r.header.type_id))
                        .map(|(r, res)| (res.header.type_id, s, r))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            resources.into_iter()
        })
        .collect();
    found.sort_unstable();
    found
}

fn resolve(game: &GameInstance, key: &ResourceKey) -> Result<At> {
    let s = game
        .find_stream(&key.stream)
        .ok_or_else(|| Error::InvalidArgument(format!("no stream {}", key.stream)))?;
    let stream = StreamFile::open(&game.streams()[s].path)?;
    let r = key
        .find_in(stream.layout())
        .ok_or_else(|| Error::InvalidArgument(format!("{key} is not in its stream")))?;
    Ok((s, r))
}

/// The speech banks of the game as the workspace has them.
pub fn banks(game: &GameInstance, ws: &Workspace) -> Result<Vec<(At, SpeechBank)>> {
    locate(game, &[TYPE_SMB])
        .into_iter()
        .map(|(_, s, r)| Ok(((s, r), SpeechBank::parse(&ws.current_data(game, s, r)?)?)))
        .collect()
}

/// The event table of the game as the workspace has it.
pub fn event_table(game: &GameInstance, ws: &Workspace) -> Result<(At, SpeechEventTable)> {
    let found = locate(game, &[TYPE_GET]);
    let &(_, s, r) = found
        .first()
        .ok_or_else(|| Error::InvalidArgument("the game has no `get` resource".into()))?;
    Ok((
        (s, r),
        SpeechEventTable::parse(&ws.current_data(game, s, r)?)?,
    ))
}

/// Adds a voice line made from `pcm`: see the module documentation. One
/// undoable operation labelled `label`.
pub fn add_line(
    game: &GameInstance,
    ws: &mut Workspace,
    pcm: &Pcm,
    line: &NewVoiceLine,
    label: &str,
) -> Result<VoiceLineReport> {
    check_name(&line.name)?;
    check_event(&line.event)?;
    if !(line.weight > 0.0 && line.weight.is_finite()) {
        return Err(Error::InvalidArgument("the weight must be positive".into()));
    }
    if pcm.frames() == 0 {
        return Err(Error::InvalidArgument("the sound is empty".into()));
    }
    if line.chv.type_id != TYPE_CHV {
        return Err(Error::InvalidArgument(format!(
            "{} is not a chatter set (chv)",
            line.chv
        )));
    }
    let chv_at = resolve(game, &line.chv)?;
    let mut set = ChatterSet::parse(&ws.current_data(game, chv_at.0, chv_at.1)?)?;
    let mut banks = banks(game, ws)?;
    if banks.is_empty() {
        return Err(Error::InvalidArgument("the game has no speech bank".into()));
    }
    let file = format!("d_{}.exa", line.name);
    if banks
        .iter()
        .flat_map(|(_, b)| &b.entries)
        .any(|e| e.file.eq_ignore_ascii_case(&file))
    {
        return Err(Error::InvalidArgument(format!(
            "a speech bank already has {file}: choose another line name"
        )));
    }

    // The model: a line the speaker already has, of this event if it can be.
    let hash = event_hash(&line.event);
    let model_id = set
        .event(hash)
        .and_then(|e| e.lines.first())
        .or_else(|| set.events.iter().find_map(|e| e.lines.first()))
        .map(|l| l.sounds);
    let model = model_id.and_then(|id| {
        banks
            .iter()
            .enumerate()
            .find_map(|(b, (_, bank))| bank.find(&id).map(|e| (b, e.clone())))
    });
    let (bank_index, model) = match model {
        Some((b, entry)) => (b, Some(entry)),
        None => {
            let global = game.find_stream(GLOBAL_STREAM);
            let b = banks
                .iter()
                .position(|((s, _), _)| Some(*s) == global)
                .unwrap_or(0);
            (b, None)
        }
    };

    // An id no bank uses.
    let mut sound = string_id(&file).max(1);
    while banks.iter().any(|(_, b)| b.find(&[sound; 4]).is_some()) {
        sound = sound.wrapping_add(1).max(1);
    }

    let mut flags = model.as_ref().map_or(FLAG_SNU | 0x09, |m| m.flags);
    // One file for every language of the game.
    flags = (flags & !FLAG_LOCALISED) | FLAG_SNU;
    let entry_path = stream_path(&file, flags, None);

    // The sound, in the format of the model line.
    let model_path = model.as_ref().map(|m| m.stream_path(None));
    let model_header = model_path.as_ref().and_then(|path| {
        let data = ws.current_file(game, &FileKey::archive_entry(path)).ok()?;
        Sample::parse_snu(&data).ok().map(|s| s.header)
    });
    let (channels, sample_rate) =
        model_header.map_or((1, SPEECH_RATE), |h| (h.channels, h.sample_rate));
    let spec = SampleSpec {
        channels,
        sample_rate,
        kind: 1,
        loop_start: None,
    };
    let fitted = fit_pcm(pcm, channels, sample_rate);
    let table = speech::new_table(&line.name, &line.subtitles)?;
    let snu = speech::attach_subtitles(&build_snu(&fitted, &spec, None)?, &table)?;

    // The bank entry.
    let category = match &line.category {
        Some(name) => string_id(name),
        None => model
            .as_ref()
            .map_or_else(|| string_id("DIALOG"), |m| m.category),
    };
    let entry = match &model {
        Some(m) => {
            // The two times of an entry (0x28, 0x2C) belong to its recording:
            // the new one gets the values of entries without them.
            let mut settings = m.settings;
            settings[4] = 0.0;
            settings[5] = -1.0;
            SpeechBankEntry {
                flags,
                id: [sound; 4],
                settings,
                category,
                file: file.clone(),
                name_hash: name_hash(&file),
                ..m.clone()
            }
        }
        None => SpeechBankEntry {
            tag: ENTRY_TAG,
            flags,
            id: [sound; 4],
            reserved: 0,
            settings: [2.0, 100.0, 0.0, 0.0, 0.0, -1.0],
            level: 90,
            category,
            category2: 0,
            duck: 0,
            file: file.clone(),
            extra: 0,
            name_hash: name_hash(&file),
        },
    };
    let (bank_at, bank) = &mut banks[bank_index];
    let bank_at = *bank_at;
    if bank.version != VERSION {
        return Err(Error::InvalidArgument(format!(
            "speech bank version {} is not supported",
            bank.version
        )));
    }
    bank.insert(entry)?;
    if let Some(problem) = bank.validate().first() {
        return Err(Error::InvalidArgument(format!("speech bank: {problem}")));
    }

    // The chatter set.
    if line.only
        && let Some(event) = set.events.iter_mut().find(|e| e.hash == hash)
    {
        event.lines.clear();
    }
    let (new_event, _) = set.add_line(&line.event, sound, string_id(&line.name), line.weight)?;
    if let Some(problem) = set.validate().first() {
        return Err(Error::InvalidArgument(format!("chatter set: {problem}")));
    }

    // The event table.
    let (get_at, mut events) = event_table(game, ws)?;
    let new_record = events.find(hash).is_none();
    if new_record {
        events.insert(SpeechEvent::new(&line.event));
    }

    let mut edits: Vec<(usize, Vec<ResourceEdit>)> = Vec::new();
    let mut push = |at: At, data: Vec<u8>| {
        let edit = ResourceEdit {
            index: at.1,
            data,
            storage: StorageChoice::Keep,
        };
        match edits.iter_mut().find(|(s, _)| *s == at.0) {
            Some((_, list)) => list.push(edit),
            None => edits.push((at.0, vec![edit])),
        }
    };
    push(chv_at, set.to_bytes()?);
    push(bank_at, bank.to_bytes()?);
    if new_record {
        push(get_at, events.to_bytes()?);
    }
    let mut streams: Vec<String> = edits
        .iter()
        .map(|(s, _)| game.streams()[*s].stream_name())
        .collect();
    streams.push(crate::workspace::FILES.to_owned());
    ws.change_resources_and_new_files(
        game,
        label,
        &edits,
        vec![(FileKey::archive_entry(&entry_path), snu)],
    )?;
    Ok(VoiceLineReport {
        sound,
        file,
        entry: entry_path,
        bank: game.streams()[bank_at.0].stream_name(),
        model: model.map(|m| m.file),
        new_event,
        new_record,
        seconds: fitted.frames() as f64 / sample_rate as f64,
        sample_rate,
        channels,
        streams,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::container::big::BigArchive;
    use crate::container::ustream::{FileHeader, Storage, StreamLayout, StreamWriter};
    use crate::workspace::ApplyOptions;
    use crate::workspace::tests::{LEVEL, TEXT, fake_game_with_files, header, stream, toc_bytes};

    const CHV_ID: u32 = 0x18C7_062E;
    const OLD_SOUND: u32 = 0x0900_0001;

    /// The fake game with a chatter set (one event, one line), a speech bank
    /// with the entry of that line and an event table at the end of
    /// `gf2/level`. Returns the indices of the three resources.
    fn voice_game() -> (tempfile::TempDir, GameInstance, [usize; 3]) {
        let (dir, game) = fake_game_with_files();
        let root = dir.path();
        let mut set = ChatterSet {
            version: 3,
            events: Vec::new(),
        };
        set.add_line("F2F_HELLO", OLD_SOUND, string_id("one"), 1.0)
            .unwrap();
        let bank = SpeechBank {
            version: VERSION,
            entries: vec![SpeechBankEntry {
                tag: ENTRY_TAG,
                flags: 0x49,
                id: [OLD_SOUND; 4],
                reserved: 0,
                settings: [3.0, 45.0, 0.0, 0.0, 0.056, 1.219],
                level: 80,
                category: string_id("CHATTER"),
                category2: 0x3504_8678,
                duck: 0,
                file: "d_one.exa".into(),
                extra: 0,
                name_hash: name_hash("d_one.exa"),
            }],
        };
        let mut events = SpeechEventTable {
            version: 3,
            events: Vec::new(),
        };
        events.insert(SpeechEvent::new("F2F_HELLO"));

        let file = game.open_stream(stream(&game, LEVEL)).unwrap();
        let mut writer = StreamWriter::new(&FileHeader::GF2_PC, crate::io::Endian::Little);
        let count = file.layout().resources.len();
        for (i, resource) in file.layout().resources.iter().enumerate() {
            let data = file.resource_data(i).unwrap();
            writer
                .push_resource(&resource.header, &data, Storage::Raw)
                .unwrap();
        }
        for (tag, ids, data) in [
            (&b"chv"[..], [CHV_ID; 4], set.to_bytes().unwrap()),
            (b"smb", [0x51; 4], bank.to_bytes().unwrap()),
            (b"get", [0x52; 4], events.to_bytes().unwrap()),
        ] {
            writer
                .push_resource(&header(tag, ids), &data, Storage::Raw)
                .unwrap();
        }
        let level = writer.finish();
        drop(file);
        fs::write(root.join("gf2/level.str"), &level).unwrap();
        let size = |data: &[u8]| StreamLayout::parse(data).unwrap().total_data_size();
        let text = fs::read(root.join("gf2/text/abcd.str")).unwrap();
        let toc = toc_bytes(&[(LEVEL, size(&level)), (TEXT, size(&text))]);
        let toc = crate::toc::Toc::parse(&toc).unwrap();
        fs::write(root.join("gf2/level.toc"), toc.to_bytes().unwrap()).unwrap();
        let game = GameInstance::open(root).unwrap();
        (dir, game, [count, count + 1, count + 2])
    }

    fn tone(seconds: f64, rate: u32) -> Pcm {
        let n = (seconds * rate as f64) as usize;
        Pcm {
            channels: 1,
            sample_rate: rate,
            samples: (0..n)
                .map(|i| ((i as f64 * 0.3).sin() * 12_000.0) as i16)
                .collect(),
        }
    }

    fn line(event: &str, name: &str) -> NewVoiceLine {
        NewVoiceLine {
            chv: ResourceKey {
                stream: LEVEL.to_owned(),
                type_id: TYPE_CHV,
                ids: [CHV_ID; 4],
            },
            event: event.to_owned(),
            name: name.to_owned(),
            subtitles: vec![("en".to_owned(), "Hello there.".to_owned())],
            weight: 1.0,
            only: false,
            category: None,
        }
    }

    #[test]
    fn a_new_line_gets_its_file_bank_entry_event_and_record() {
        let (dir, game, [chv, smb, get]) = voice_game();
        let root = dir.path();
        let s = stream(&game, LEVEL);
        let mut ws = Workspace::for_game(&game, None).unwrap();
        assert_eq!(
            locate(&game, &[TYPE_CHV, TYPE_SMB, TYPE_GET]).len(),
            3,
            "the three resources are found by type"
        );

        // A new event: four things change in one operation.
        let new = line("DOCTOR_HELLO", "doc1_xxx_0000001");
        let report = add_line(&game, &mut ws, &tone(0.5, 44_100), &new, "New line").unwrap();
        assert!(report.new_event && report.new_record);
        assert_eq!(report.file, "d_doc1_xxx_0000001.exa");
        assert_eq!(
            report.entry,
            "audiostreams\\c1_xxx_0\\d_doc1_xxx_0000001.exa.snu"
        );
        assert_eq!(report.model.as_deref(), Some("d_one.exa"));
        assert_eq!((report.sample_rate, report.channels), (SPEECH_RATE, 1));
        assert_eq!(ws.operations().len(), 1);
        assert_eq!((ws.len(), ws.file_changes().count()), (3, 1));

        let set = ChatterSet::parse(&ws.current_data(&game, s, chv).unwrap()).unwrap();
        assert!(set.validate().is_empty());
        let event = set.event(event_hash("DOCTOR_HELLO")).unwrap();
        assert_eq!(event.lines.len(), 1);
        assert_eq!(event.lines[0].sounds, [report.sound; 4]);
        assert_eq!(event.lines[0].subtitle, string_id("doc1_xxx_0000001"));

        let bank = SpeechBank::parse(&ws.current_data(&game, s, smb).unwrap()).unwrap();
        assert!(bank.validate().is_empty());
        let entry = bank.find(&[report.sound; 4]).unwrap();
        // The settings of the speaker's line; one file for every language.
        assert_eq!(entry.settings, [3.0, 45.0, 0.0, 0.0, 0.0, -1.0]);
        assert_eq!((entry.level, entry.category), (80, string_id("CHATTER")));
        assert_eq!(entry.flags, 0x48);
        assert_eq!(entry.name_hash, name_hash(&entry.file));
        assert_eq!(entry.stream_path(Some("es")), report.entry);

        let events = SpeechEventTable::parse(&ws.current_data(&game, s, get).unwrap()).unwrap();
        assert!(events.validate().is_empty());
        assert!(events.find(event_hash("doctor_hello")).is_some());
        assert_eq!(events.events.len(), 2);

        let key = FileKey::archive_entry(&report.entry);
        let snu = ws.current_file(&game, &key).unwrap();
        let sample = Sample::parse_snu(&snu).unwrap();
        assert_eq!(sample.header.sample_rate, SPEECH_RATE);
        assert_eq!(sample.header.sample_count, 16_000);
        assert_eq!(sample.decode().unwrap().len(), 16_000);
        let table = speech::subtitles(&snu).unwrap().unwrap();
        assert_eq!(table.hashes, vec![string_id("doc1_xxx_0000001")]);
        assert_eq!(table.strings[0][0], "Hello there.");

        // One more line of an event the table knows: no new record.
        let mut second = line("f2f_hello", "doc1_xxx_0000002");
        second.category = Some("DIALOG".to_owned());
        second.weight = 2.0;
        let report2 = add_line(&game, &mut ws, &tone(0.2, 32_000), &second, "Second").unwrap();
        assert!(!report2.new_event && !report2.new_record);
        assert_ne!(report2.sound, report.sound);
        let set = ChatterSet::parse(&ws.current_data(&game, s, chv).unwrap()).unwrap();
        let hello = set.event(event_hash("F2F_HELLO")).unwrap();
        assert_eq!(hello.name, "F2F_HELLO");
        assert_eq!(hello.lines.len(), 2);
        assert_eq!((hello.lines[1].weight, hello.lines[1].index), (2.0, 1));
        let bank = SpeechBank::parse(&ws.current_data(&game, s, smb).unwrap()).unwrap();
        assert_eq!(bank.entries.len(), 3);
        assert_eq!(
            bank.find(&[report2.sound; 4]).unwrap().category,
            string_id("DIALOG")
        );

        // `only` leaves the event with the new line alone.
        let mut third = line("F2F_HELLO", "doc1_xxx_0000003");
        third.only = true;
        add_line(&game, &mut ws, &tone(0.2, 32_000), &third, "Third").unwrap();
        let set = ChatterSet::parse(&ws.current_data(&game, s, chv).unwrap()).unwrap();
        assert_eq!(set.event(event_hash("F2F_HELLO")).unwrap().lines.len(), 1);

        // Refused: a name twice, bad names, an empty sound, another type.
        assert!(add_line(&game, &mut ws, &tone(0.2, 32_000), &third, "x").is_err());
        for bad in ["", "Upper", "with space", &"x".repeat(MAX_NAME + 1)] {
            let l = line("F2F_HELLO", bad);
            assert!(add_line(&game, &mut ws, &tone(0.2, 32_000), &l, "x").is_err());
        }
        let l = line("bad event!", "doc1_xxx_0000009");
        assert!(add_line(&game, &mut ws, &tone(0.2, 32_000), &l, "x").is_err());
        let l = line("F2F_HELLO", "doc1_xxx_0000009");
        assert!(add_line(&game, &mut ws, &tone(0.0, 32_000), &l, "x").is_err());
        let mut l = line("F2F_HELLO", "doc1_xxx_0000009");
        l.chv.type_id = TYPE_SMB;
        assert!(add_line(&game, &mut ws, &tone(0.2, 32_000), &l, "x").is_err());
        assert_eq!(ws.operations().len(), 3, "a refused line records nothing");

        // In the game: the streams and the overlay archive.
        ws.apply(&game, ApplyOptions::default()).unwrap();
        let overlay = BigArchive::open(root.join("GODFATH0.viv")).unwrap();
        let names: Vec<&str> = overlay.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&report.entry.as_str()), "{names:?}");
        assert_eq!(overlay.entries.len(), 4);
        let applied = game.open_stream(s).unwrap().resource_data(smb).unwrap();
        assert_eq!(SpeechBank::parse(&applied).unwrap().entries.len(), 4);
        // Undoing the first line takes its file away again.
        while ws.undo().unwrap().is_some() {}
        assert!(ws.is_empty() && ws.file_changes().next().is_none());
    }
}
