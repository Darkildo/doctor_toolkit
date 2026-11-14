//! In-engine cut-scenes: Trinity sequences of shots with their actors,
//! animation clips, camera paths, sound track and subtitles.
//!
//! A cut-scene is a SimGroup object of class `TrinityGameSequence` in a
//! "folder stream" (`…/cp_009_hotel_interior_folderstream`). Its resource
//! links name the shots in playing order: `trinity_seq_master` resources of
//! the same stream ([`crate::resource::trinity`]). Every shot has a child
//! stream named after its source XML (`…_folderstream/igc01_s01`, loaded on
//! request) with the SimGroup objects the shot's actors stand for (`Animated`,
//! `PartedAnimated`) and one `bnk` bank per actor with its clip. A
//! `PartedAnimated` names a set of the parts database its metamodel's
//! `PartedModelPart.Assembly` names, and a preset item.
//!
//! The sound track is a streamed sound of the `.viv` archives
//! (`audiostreams\igc01_cu\aud_igc01_cubancelebration.exa.snu`); its metadata
//! holds the subtitle table (`TOB1`, [`Tob::from_snu`]) with the keys of the
//! cut-scene's string table (`lh2` of the folder stream). The tool finds the
//! sound by the cut-scene's folder name (`igc01`), and the music of the
//! same folders (`audiostreams\igc06_01\mus_igc06_01.exa.snu`), which the
//! audio scripts start on messages of the cut-scene ([`sound`]).

pub mod cast;
pub mod gltf;
pub mod sound;
pub mod world;

use std::collections::HashMap;

use serde::Serialize;

use crate::container::big::BigArchive;
use crate::game::{GameInstance, StreamFile};
use crate::hash::sdbm;
use crate::resource::anim::{AnimBank, TYPE_BNK};
use crate::resource::lh2::StringTable;
use crate::resource::metamodel::{MetaModel, PropValue};
use crate::resource::simgroup::{SimGroup, Value};
use crate::resource::tob::Tob;
use crate::resource::trinity::{
    Command, Guid, SeqBlock, SeqMaster, TYPE_TRINITY_SEQ_MASTER, camera_matrix, eval_curve,
    eval_curves, eval_positions, eval_rotations,
};
use crate::text::TYPE_LH2;
use crate::workspace::{FileKey, ResourceKey, Workspace};
use crate::{Error, Result};

const TYPE_SIMGROUP: u32 = sdbm(b"simgroup");
const TYPE_METAMODEL: u32 = sdbm(b"metamodel");
/// Class hash of `TrinityGameSequence` SimGroup objects.
pub const CLASS_TRINITY_GAME_SEQUENCE: u32 = 0x3043_1D5E;

/// Where cut-scene data is read from: the game, and the workspace versions of
/// what the workspace changes.
#[derive(Clone, Copy)]
pub struct Source<'a> {
    pub game: &'a GameInstance,
    pub workspace: Option<&'a Workspace>,
}

impl<'a> Source<'a> {
    pub fn new(game: &'a GameInstance, workspace: Option<&'a Workspace>) -> Self {
        Self { game, workspace }
    }

    /// Data of resource `index` of an opened stream.
    pub fn resource(&self, stream: usize, file: &StreamFile, index: usize) -> Result<Vec<u8>> {
        if let Some(ws) = self.workspace {
            let header = file
                .layout()
                .resources
                .get(index)
                .ok_or_else(|| Error::InvalidArgument(format!("no resource {index}")))?
                .header;
            let key = ResourceKey::new(&self.game.streams()[stream].stream_name(), &header);
            if ws.change(&key).is_some()
                && let Some(data) = ws.data(&key)?
            {
                return Ok(data);
            }
        }
        file.resource_data(index)
    }

    /// A file of the `.viv` archives or a loose file.
    pub fn file(&self, key: &FileKey) -> Result<Vec<u8>> {
        match self.workspace {
            Some(ws) => ws.current_file(self.game, key),
            None => crate::workspace::files::original_data(self.game.root(), key),
        }
    }
}

/// A `TrinityGameSequence` object.
#[derive(Debug, Clone, Serialize)]
pub struct SequenceObject {
    /// Resource index of its SimGroup in the folder stream.
    pub group: usize,
    pub object: usize,
    pub guid: String,
    /// Attribute 0: the message that starts the cut-scene.
    pub start_message: Option<String>,
    /// Attribute 1: the message sent when it ends.
    pub end_message: Option<String>,
    /// Attribute 6: another message sent at the end.
    pub next_message: Option<String>,
    /// Shot resource ids in playing order (the object's links).
    pub shots: Vec<u32>,
    /// The object's matrix (`CSystemCommands.CMD_LoadMatrix`), the anchor
    /// of the keys ([`world::anchor`]); no object of the game has one.
    pub matrix: Option<[[f32; 4]; 4]>,
}

/// A clip of a bank of a shot stream.
#[derive(Debug, Clone, Serialize)]
pub struct ClipInfo {
    /// Resource index of the bank in the shot stream.
    pub bank: usize,
    pub clip: usize,
    pub hash: u32,
    pub name: Option<String>,
    pub frames: u32,
}

/// An actor of a shot: what its SimGroup object says.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ActorInfo {
    pub guid: Guid,
    /// Name given by the shot (`igc01_dominic`).
    pub name: String,
    /// SimGroup class (`PartedAnimated`, `Animated`); `None` when the
    /// object was not found.
    pub class: Option<String>,
    /// The metamodel its `Entity` shows.
    pub metamodel: Option<u32>,
    /// The parts database (`PartedModelPart.Assembly` of the metamodel).
    pub assembly: Option<String>,
    /// Set of the database (`PartedAnimated` attribute 2).
    pub set: Option<String>,
    /// Preset and item (`PartedAnimated` attributes 0 and 1).
    pub preset: Option<(String, String)>,
}

/// A shot of a cut-scene.
#[derive(Debug, Clone, Serialize)]
pub struct Shot {
    /// Resource index in the folder stream.
    pub resource: usize,
    pub id: u32,
    pub seq: SeqMaster,
    /// Seconds from the start of the cut-scene.
    pub start: f32,
    /// The shot's own stream (`…/igc01_s01`).
    pub stream: Option<usize>,
    pub stream_name: String,
    pub actors: Vec<ActorInfo>,
    /// Clips of the banks of the shot stream.
    pub clips: Vec<ClipInfo>,
}

impl Shot {
    pub fn block(&self) -> &SeqBlock {
        &self.seq.blocks[0]
    }

    /// Length of the shot on the timeline.
    pub fn duration(&self) -> f32 {
        self.block().duration.max(self.block().wait_total())
    }

    pub fn end(&self) -> f32 {
        self.start + self.duration()
    }

    /// The clip named `name` (case-insensitive SDBM) in the shot's banks.
    pub fn clip(&self, name: &str) -> Option<&ClipInfo> {
        let hash = sdbm(name.to_ascii_lowercase().as_bytes());
        self.clips.iter().find(|c| c.hash == hash)
    }

    pub fn actor(&self, guid: &Guid) -> Option<&ActorInfo> {
        self.actors.iter().find(|a| a.guid == *guid)
    }
}

/// A subtitle or title line of the sound track.
#[derive(Debug, Clone, Serialize)]
pub struct Subtitle {
    /// Group: `subtitles` or `titles`.
    pub group: String,
    pub group_index: usize,
    pub entry: usize,
    /// Key of the string table.
    pub string: u32,
    pub start_ms: u32,
    pub duration_ms: u32,
    /// The English text when the cut-scene's string table has it.
    pub text: Option<String>,
}

/// A streamed sound of the `.viv` archives.
#[derive(Debug, Clone, Serialize)]
pub struct AudioTrack {
    /// Archive entry (`audiostreams\igc01_cu\aud_igc01_cubancelebration.exa.snu`).
    pub entry: String,
    pub duration_ms: u32,
    pub channels: u8,
    /// The subtitle table of the sound's metadata.
    #[serde(skip)]
    pub tob: Option<Tob>,
    pub subtitles: Vec<Subtitle>,
    /// When the track starts and how loud it plays ([`sound::place`]).
    pub placement: sound::Placement,
}

/// A cut-scene: its sequence object, shots in order and sound track.
#[derive(Debug, Clone, Serialize)]
pub struct Cutscene {
    /// The folder stream.
    pub stream: usize,
    pub stream_name: String,
    /// The cut-scene folder of the source files (`igc01`).
    pub name: String,
    pub sequence: Option<SequenceObject>,
    pub shots: Vec<Shot>,
    pub audio: Vec<AudioTrack>,
}

/// A shot resource found in the game.
#[derive(Debug, Clone, Serialize)]
pub struct ShotSummary {
    pub stream: usize,
    pub stream_name: String,
    pub resource: usize,
    pub id: u32,
    pub name: String,
    pub duration: f32,
}

/// Every `trinity_seq_master` of the game, stream by stream.
pub fn list_shots(game: &GameInstance) -> Result<Vec<ShotSummary>> {
    let mut out = Vec::new();
    for (stream, file) in game.streams().iter().enumerate() {
        let opened = game.open_stream(stream)?;
        for (index, r) in opened.layout().resources.iter().enumerate() {
            if r.header.type_id != TYPE_TRINITY_SEQ_MASTER {
                continue;
            }
            let seq = SeqMaster::parse(&opened.resource_data(index)?)?;
            out.push(ShotSummary {
                stream,
                stream_name: file.stream_name(),
                resource: index,
                id: r.header.ids[3],
                name: seq.source_name().to_owned(),
                duration: seq.block().map_or(0.0, |b| b.duration),
            });
        }
    }
    Ok(out)
}

fn string_attr(group: &SimGroup, object: usize, class: u32, index: u16) -> Option<String> {
    match group.attr(&group.objects[object], class, index) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// The `TrinityGameSequence` objects of a stream.
pub fn sequences_in(source: &Source<'_>, stream: usize) -> Result<Vec<SequenceObject>> {
    let file = source.game.open_stream(stream)?;
    let mut out = Vec::new();
    for (index, r) in file.layout().resources.iter().enumerate() {
        if r.header.type_id != TYPE_SIMGROUP {
            continue;
        }
        let group = SimGroup::parse(&source.resource(stream, &file, index)?)?;
        for (o, object) in group.objects.iter().enumerate() {
            if object.class != CLASS_TRINITY_GAME_SEQUENCE {
                continue;
            }
            out.push(SequenceObject {
                group: index,
                object: o,
                guid: object.guid.to_string(),
                start_message: string_attr(&group, o, CLASS_TRINITY_GAME_SEQUENCE, 0),
                end_message: string_attr(&group, o, CLASS_TRINITY_GAME_SEQUENCE, 1),
                next_message: string_attr(&group, o, CLASS_TRINITY_GAME_SEQUENCE, 6),
                shots: object
                    .links
                    .iter()
                    .filter_map(|l| l.resource_id())
                    .collect(),
                matrix: group.transform(object),
            });
        }
    }
    Ok(out)
}

/// `PartedModelPart.Assembly` of a metamodel: the parts database name.
fn assembly_of(model: &MetaModel) -> Option<String> {
    model.properties.iter().find_map(|p| match &p.value {
        PropValue::String(s) if p.name == "Assembly" => Some(s.clone()),
        _ => None,
    })
}

impl Cutscene {
    /// The cut-scene of shot resource `shot` of folder stream `stream`: the
    /// sequence that links the shot (all of its shots), or the shot alone.
    pub fn load(source: &Source<'_>, stream: usize, shot: Option<usize>) -> Result<Self> {
        let game = source.game;
        let file = game.open_stream(stream)?;
        let stream_name = game.streams()[stream].stream_name();
        // Shot resources of the stream by id.
        let mut by_id: HashMap<u32, usize> = HashMap::new();
        let mut order = Vec::new();
        for (index, r) in file.layout().resources.iter().enumerate() {
            if r.header.type_id == TYPE_TRINITY_SEQ_MASTER {
                by_id.insert(r.header.ids[3], index);
                order.push(index);
            }
        }
        let wanted_id = shot.and_then(|s| file.layout().resources.get(s).map(|r| r.header.ids[3]));
        let sequences = sequences_in(source, stream)?;
        let sequence = sequences
            .into_iter()
            .find(|s| wanted_id.is_none_or(|id| s.shots.contains(&id)));
        let resources: Vec<usize> = match (&sequence, shot) {
            (Some(seq), _) => seq
                .shots
                .iter()
                .filter_map(|id| by_id.get(id).copied())
                .collect(),
            (None, Some(s)) => vec![s],
            (None, None) => order,
        };
        if resources.is_empty() {
            return Err(Error::InvalidArgument(format!(
                "{stream_name} has no cut-scene shots"
            )));
        }
        // Metamodels of the folder stream (actors use them).
        let mut models = models_in(source, stream, &file)?;
        let mut shots = Vec::new();
        let mut start = 0.0f32;
        for index in resources {
            let seq = SeqMaster::parse(&source.resource(stream, &file, index)?)?;
            if seq.blocks.is_empty() {
                continue;
            }
            let shot_stream_name = format!("{stream_name}/{}", seq.source_name().to_lowercase());
            let shot_stream = game.find_stream(&shot_stream_name);
            let mut shot = Shot {
                resource: index,
                id: file.layout().resources[index].header.ids[3],
                seq,
                start,
                stream: shot_stream,
                stream_name: shot_stream_name,
                actors: Vec::new(),
                clips: Vec::new(),
            };
            shot.load_cast(source, &mut models)?;
            start += shot.duration();
            shots.push(shot);
        }
        let name = shots
            .iter()
            .find_map(|s| s.seq.scene_name().map(str::to_lowercase))
            .unwrap_or_else(|| shots[0].seq.source_name().to_lowercase());
        let strings = strings_in(source, stream, &file);
        let mut audio = find_audio(source, &name, strings.as_ref())?;
        sound::place(source, &shots, &mut audio)?;
        Ok(Self {
            stream,
            stream_name,
            name,
            sequence,
            shots,
            audio,
        })
    }

    /// Total length in seconds.
    pub fn duration(&self) -> f32 {
        self.shots.last().map_or(0.0, |s| s.end())
    }

    /// The shot playing at `t` seconds.
    pub fn shot_at(&self, t: f32) -> Option<usize> {
        if self.shots.is_empty() {
            return None;
        }
        Some(
            self.shots
                .iter()
                .position(|s| t < s.end())
                .unwrap_or(self.shots.len() - 1),
        )
    }

    /// The camera at `t` seconds.
    pub fn camera_at(&self, t: f32) -> Option<CameraState> {
        let shot = self.shot_at(t)?;
        camera_state(&self.shots[shot], t - self.shots[shot].start).map(|mut c| {
            c.shot = shot;
            c
        })
    }
}

impl Shot {
    /// Actors (SimGroup objects of the shot stream) and the clips of its banks.
    fn load_cast(
        &mut self,
        source: &Source<'_>,
        models: &mut HashMap<u32, MetaModel>,
    ) -> Result<()> {
        let block = self.seq.blocks[0].clone();
        let mut objects: HashMap<[u8; 16], ActorInfo> = HashMap::new();
        if let Some(stream) = self.stream {
            let file = source.game.open_stream(stream)?;
            models.extend(models_in(source, stream, &file)?);
            for (index, r) in file.layout().resources.iter().enumerate() {
                match r.header.type_id {
                    TYPE_SIMGROUP => {
                        let group = SimGroup::parse(&source.resource(stream, &file, index)?)?;
                        for (o, object) in group.objects.iter().enumerate() {
                            let class = object.class_name().map(str::to_owned);
                            let metamodel = group.model(object);
                            let parted = sdbm(b"partedanimated");
                            let preset = string_attr(&group, o, parted, 0)
                                .zip(string_attr(&group, o, parted, 1));
                            objects.insert(
                                object.guid.0,
                                ActorInfo {
                                    guid: Guid(object.guid.0),
                                    name: String::new(),
                                    class,
                                    metamodel,
                                    assembly: None,
                                    set: string_attr(&group, o, parted, 2),
                                    preset,
                                },
                            );
                        }
                    }
                    TYPE_BNK => {
                        let Ok(bank) = AnimBank::parse(&source.resource(stream, &file, index)?)
                        else {
                            continue;
                        };
                        for (k, clip) in bank.clips.iter().enumerate() {
                            self.clips.push(ClipInfo {
                                bank: index,
                                clip: k,
                                hash: clip.hash,
                                name: clip.name.clone(),
                                frames: clip.frames,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        for command in &block.commands {
            if let Command::ActorCreate { actor, name, .. } = command {
                let mut info = objects.get(&actor.0).cloned().unwrap_or(ActorInfo {
                    guid: *actor,
                    ..Default::default()
                });
                info.name = name.text.clone();
                info.assembly = info
                    .metamodel
                    .and_then(|m| models.get(&m))
                    .and_then(assembly_of);
                self.actors.push(info);
            }
        }
        Ok(())
    }
}

fn models_in(
    source: &Source<'_>,
    stream: usize,
    file: &StreamFile,
) -> Result<HashMap<u32, MetaModel>> {
    let mut out = HashMap::new();
    for (index, r) in file.layout().resources.iter().enumerate() {
        if r.header.type_id == TYPE_METAMODEL
            && let Ok(model) = MetaModel::parse(&source.resource(stream, file, index)?)
        {
            out.insert(r.header.ids[3], model);
        }
    }
    Ok(out)
}

/// The string table of the folder stream (subtitles of its cut-scenes).
fn strings_in(source: &Source<'_>, stream: usize, file: &StreamFile) -> Option<StringTable> {
    let index = file
        .layout()
        .resources
        .iter()
        .position(|r| r.header.type_id == TYPE_LH2)?;
    StringTable::parse(&source.resource(stream, file, index).ok()?).ok()
}

/// Streamed sounds of a cut-scene: `audiostreams\<scene>…\*.snu` of the
/// archives (the first voice, not `audiostreams\es\`).
pub fn find_audio(
    source: &Source<'_>,
    scene: &str,
    strings: Option<&StringTable>,
) -> Result<Vec<AudioTrack>> {
    let scene = scene.to_ascii_lowercase();
    let mut out: Vec<AudioTrack> = Vec::new();
    let archives =
        crate::workspace::files::original_archives(source.game.root()).unwrap_or_default();
    for path in archives {
        let Ok(archive) = BigArchive::open(&path) else {
            continue;
        };
        for entry in &archive.entries {
            let name = entry.name.to_ascii_lowercase();
            let mut parts = name.split('\\');
            if parts.next() != Some("audiostreams") {
                continue;
            }
            let Some(dir) = parts.next() else { continue };
            let matches = dir == scene
                || dir
                    .strip_prefix(&scene)
                    .is_some_and(|rest| rest.starts_with('_'));
            if !matches || !name.ends_with(".snu") || out.iter().any(|a| a.entry == name) {
                continue;
            }
            let key = FileKey::archive_entry(&name);
            let data = source.file(&key)?;
            out.push(audio_track(&name, &data, strings));
        }
    }
    out.sort_by(|a, b| a.entry.cmp(&b.entry));
    Ok(out)
}

/// What a streamed sound says about itself.
pub fn audio_track(entry: &str, snu: &[u8], strings: Option<&StringTable>) -> AudioTrack {
    let duration_ms = snu
        .get(4..8)
        .map_or(0, |b| u32::from_le_bytes(b.try_into().expect("4 bytes")));
    let tob = Tob::from_snu(snu).map(|(t, _)| t);
    let subtitles = tob
        .as_ref()
        .map(|t| subtitles(t, strings))
        .unwrap_or_default();
    AudioTrack {
        entry: entry.to_owned(),
        duration_ms,
        channels: snu.get(3).copied().unwrap_or(0),
        tob,
        subtitles,
        placement: sound::Placement::default(),
    }
}

/// The lines of a subtitle table with their English text.
pub fn subtitles(tob: &Tob, strings: Option<&StringTable>) -> Vec<Subtitle> {
    let mut out = Vec::new();
    for (g, group) in tob.groups.iter().enumerate() {
        for (e, entry) in group.entries.iter().enumerate() {
            let text = strings
                .and_then(|s| s.texts(entry.string))
                .and_then(|t| t.first().map(|s| (*s).to_owned()));
            out.push(Subtitle {
                group: group
                    .name
                    .map_or_else(|| format!("{:08x}", group.name_hash), str::to_owned),
                group_index: g,
                entry: e,
                string: entry.string,
                start_ms: entry.start_ms,
                duration_ms: entry.duration_ms,
                text,
            });
        }
    }
    out
}

/// The camera at some moment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CameraState {
    pub shot: usize,
    pub camera: u32,
    /// x, y, z, rotation x, y, z (degrees).
    pub values: [f32; 6],
    /// The `0x307` curve as stored: the focal length of the Maya camera in
    /// millimetres (70, 50, 35 …), not an angle; see [`Self::vertical_fov`].
    pub fov: f32,
    pub aspect: f32,
}

/// Half the vertical film aperture of the shots' Maya cameras, mm: the film
/// back is 1.778 × 1 inch (16:9), so the 16:9 picture spans
/// `2·atan(12.7 / focal length)` vertically and `2·atan(22.58 / focal
/// length)` across. The game on a 4:3 screen keeps the width and adds bars:
/// its frustum then spans `2·atan(16.93 / focal length)` vertically, which
/// the live link measured during `igc16` (70 → 27.2°, 50 → 37.4°,
/// 35 → 51.6°; `docs/formats/cutscenes.md` §9.4).
pub const HALF_APERTURE_MM: f32 = 12.7;

impl CameraState {
    /// Vertical field of view in degrees of the 16:9 picture, from the
    /// focal length.
    pub fn vertical_fov(&self) -> f32 {
        let focal = self.fov.max(1.0);
        (2.0 * (HALF_APERTURE_MM / focal).atan()).to_degrees()
    }

    /// Camera to world, column-major.
    pub fn matrix(&self) -> [f32; 16] {
        camera_matrix(self.values)
    }
}

/// The camera of a shot `t` seconds after its start: the latest path and
/// field of view commands.
pub fn camera_state(shot: &Shot, t: f32) -> Option<CameraState> {
    let block = shot.block();
    let times = block.times();
    let mut state: Option<CameraState> = None;
    let mut aspect = 16.0 / 9.0;
    let mut fov = 45.0;
    for (command, &at) in block.commands.iter().zip(&times) {
        if at > t {
            break;
        }
        match command {
            Command::CameraCreate { aspect: a, .. } if *a > 0.0 => aspect = *a,
            Command::CameraCurves { camera, curves } => {
                state = Some(CameraState {
                    shot: 0,
                    camera: *camera,
                    values: eval_curves(curves, t - at),
                    fov,
                    aspect,
                });
            }
            Command::CameraFov { keys, .. } => {
                fov = eval_curve(keys, t - at);
                if let Some(s) = &mut state {
                    s.fov = fov;
                }
            }
            _ => {}
        }
    }
    state.map(|mut s| {
        s.aspect = aspect;
        s
    })
}

/// An actor at some moment of a shot.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActorState {
    pub position: [f32; 3],
    /// Quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    /// The clip playing and the time into it (seconds).
    pub clip: Option<(String, f32)>,
    pub speed: f32,
}

impl ActorState {
    /// Actor to world, column-major.
    pub fn matrix(&self) -> [f32; 16] {
        let [x, y, z, w] = self.rotation;
        let [px, py, pz] = self.position;
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
            0.0,
            2.0 * (x * y - w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + w * x),
            0.0,
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
            0.0,
            px,
            py,
            pz,
            1.0,
        ]
    }
}

/// Where `actor` is and what it plays `t` seconds after the start of the shot.
pub fn actor_state(shot: &Shot, actor: &Guid, t: f32) -> Option<ActorState> {
    let block = shot.block();
    let times = block.times();
    let mut state: Option<ActorState> = None;
    for (command, &at) in block.commands.iter().zip(&times) {
        if at > t {
            break;
        }
        match command {
            Command::ActorCreate {
                actor: a, position, ..
            } if a == actor => {
                state = Some(ActorState {
                    position: *position,
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    clip: None,
                    speed: 1.0,
                });
            }
            Command::ActorTransform {
                actor: a,
                positions,
                rotations,
            } if a == actor => {
                if let Some(s) = &mut state {
                    if let Some(p) = eval_positions(positions, t - at) {
                        s.position = p;
                    }
                    if let Some(r) = eval_rotations(rotations, t - at) {
                        s.rotation = r;
                    }
                }
            }
            Command::ActorAnim {
                actor: a,
                clip,
                speed,
                ..
            } if a == actor => {
                if let Some(s) = &mut state {
                    s.clip = Some((clip.text.clone(), (t - at) * speed));
                    s.speed = *speed;
                }
            }
            _ => {}
        }
    }
    state
}

/// The command `index` of a shot on the cut-scene timeline: start time
/// (seconds from the start of the cut-scene) and length in seconds (camera
/// paths and animation clips last until their last key, frame or the end of
/// the shot).
pub fn command_span(shot: &Shot, index: usize, time: f32) -> (f32, f32) {
    let block = shot.block();
    let end = shot.duration();
    let length = match &block.commands[index] {
        Command::CameraCurves { curves, .. } => {
            curves.iter().flatten().map(|k| k.time).fold(0.0, f32::max)
        }
        Command::CameraFov { keys, .. } => keys.iter().map(|k| k.time).fold(0.0, f32::max),
        Command::ActorTransform { positions, .. } => {
            positions.iter().map(|k| k.time).fold(0.0, f32::max)
        }
        Command::ActorAnim { clip, speed, .. } => shot
            .clip(&clip.text)
            .map_or(end - time, |c| c.frames as f32 / 30.0 / speed.max(1e-3)),
        Command::Fade { seconds, .. } => *seconds,
        _ => 0.0,
    };
    (shot.start + time, length.min(end - time).max(0.0))
}

#[cfg(test)]
mod tests;
