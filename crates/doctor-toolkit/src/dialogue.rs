//! Face-to-face dialogues across the game: the dialogue trees (`dlg`), the
//! speech they play and the texts they show, and who starts them.
//!
//! - A dialogue is resource `dlg` in its own stream `gf2/globalfolder/dlg/<hash>`
//!   (with the animation banks of its clips). SimGroup objects name it by the
//!   stream path in a string attribute (`gf2\globalfolder\dlg\0823c6d1`):
//!   `NPC` attribute 5 (`CommonNPC` objects), `ChangeNPCSettings` 27 and
//!   `ChangeDialog` 3.
//! - A line plays a speech event (`P2_LINE1`, `F2F_ENJOY_FLIGHT`) by hash;
//!   the speaker's chatter sets (`chv`) give the recorded lines of the event,
//!   a speech bank (`smb`) the file of each line, and the file in the `.viv`
//!   archives carries the subtitle in every language
//!   ([`crate::resource::speech`]).
//! - Choice options show strings of the string tables (`lh2`) by name.
//!
//! [`Catalog::build`] reads all of this once (a few seconds); edits go
//! through the workspace: the dialogue resource, the string tables and, for
//! subtitles and audio, the overlay archive ([`crate::workspace::files`]).

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use rayon::prelude::*;
use serde::Serialize;

use crate::container::big::{BigArchive, normalize_name};
use crate::game::{GameInstance, IndexEntry, ResourceIndex, StreamFile};
use crate::hash::{sdbm, string_id};
use crate::resource::chv::{ChatterLine, ChatterSet, TYPE_CHV};
use crate::resource::dlg::{
    self, CHOICE_SLOTS, Dialogue, KIND_CHOICE, KIND_LINE, LINE_ANIM_LEAD, LINE_ANIM_LISTENER,
    LINE_ANIM_SPEAKER, LINE_EVENT, LINE_SPEAKER, NONE, TYPE_DLG, Test,
};
use crate::resource::lh2::StringTable;
use crate::resource::simgroup::{SimGroup, Value};
use crate::resource::smb::{self, TYPE_SMB};
use crate::resource::speech;
use crate::text::{TYPE_LH2, TYPE_LHR, TableRef};
use crate::workspace::{FileKey, ResourceEdit, ResourceKey, StorageChoice, Workspace, files};
use crate::{Error, Result};

const TYPE_SIMGROUP: u32 = sdbm(b"simgroup");
/// Start of the SimGroup string that names a dialogue.
pub const DIALOGUE_PATH_PREFIX: &str = "gf2\\globalfolder\\dlg\\";

/// A dialogue resource of the game.
#[derive(Debug, Clone, Serialize)]
pub struct DialogueRef {
    pub stream: usize,
    pub resource: usize,
    /// Stream name, `gf2/globalfolder/dlg/0823c6d1`.
    pub name: String,
}

impl DialogueRef {
    /// The path SimGroup attributes use for it, lower case.
    pub fn path(&self) -> String {
        self.name.replace('/', "\\").to_lowercase()
    }
}

/// One recorded line a speech event can play.
#[derive(Debug, Clone, Serialize)]
pub struct SpeechLine {
    /// The chatter set: stream and resource.
    pub chv: (usize, usize),
    pub event: String,
    pub line: ChatterLine,
    /// File named by the speech bank (`d_mob3_xxx_0001806.exa`).
    pub file: Option<String>,
    /// Archive entries of the file, the first voice first
    /// (`audiostreams\b3_xxx_0\d_mob3_xxx_0001806.exa.snu`, then `\es\`).
    pub entries: Vec<String>,
}

/// A SimGroup attribute that names a dialogue.
#[derive(Debug, Clone, Serialize)]
pub struct Usage {
    pub stream: usize,
    pub resource: usize,
    /// Object name, or its GUID.
    pub object: String,
    pub class: String,
    /// `Class.attribute`.
    pub attribute: String,
}

/// Everything the dialogue editor looks up, read once from the game.
pub struct Catalog {
    pub dialogues: Vec<DialogueRef>,
    /// Speech event names by hash.
    pub event_names: HashMap<u32, String>,
    /// Recorded lines by speech event hash.
    pub speech: HashMap<u32, Vec<SpeechLine>>,
    /// SimGroup attributes by dialogue path (lower case).
    pub usages: HashMap<String, Vec<Usage>>,
    /// The one-language string tables of the game (`lhr`).
    pub languages: Vec<TableRef>,
    /// English subtitles of the speech files the dialogues play, by archive entry.
    pub subtitles: HashMap<String, String>,
    /// The string tables (`lh2`, `lhr`) of the game, for [`crate::text::plan_add`].
    pub text_index: ResourceIndex,
    archives: Vec<BigArchive>,
    /// Normalised entry name → (archive, entry).
    entries: HashMap<String, (usize, usize)>,
}

#[derive(Default)]
struct Scan {
    dialogues: Vec<DialogueRef>,
    chatter: Vec<((usize, usize), ChatterSet)>,
    sounds: Vec<(u32, String)>,
    usages: Vec<(String, Usage)>,
    text: Vec<IndexEntry>,
}

fn scan_stream(game: &GameInstance, s: usize) -> Result<Scan> {
    let mut out = Scan::default();
    let wanted = [
        TYPE_DLG,
        TYPE_CHV,
        TYPE_SMB,
        TYPE_SIMGROUP,
        TYPE_LH2,
        TYPE_LHR,
    ];
    let file = &game.streams()[s];
    let stream = StreamFile::open(&file.path)?;
    let layout = stream.layout();
    for (r, res) in layout.resources.iter().enumerate() {
        let type_id = res.header.type_id;
        if !wanted.contains(&type_id) {
            continue;
        }
        if type_id == TYPE_LH2 || type_id == TYPE_LHR {
            out.text.push(IndexEntry {
                stream: s as u32,
                resource: r as u32,
                header: res.header,
                stored_size: layout.stored_size(r) as u32,
                compressed: layout.is_compressed(r),
            });
            continue;
        }
        let data = stream.resource_data(r)?;
        match type_id {
            TYPE_DLG => out.dialogues.push(DialogueRef {
                stream: s,
                resource: r,
                name: file.stream_name(),
            }),
            TYPE_CHV => out.chatter.push(((s, r), ChatterSet::parse(&data)?)),
            TYPE_SMB => out
                .sounds
                .extend(smb::parse(&data)?.into_iter().map(|e| (e.sound, e.file))),
            _ => {
                if contains(&data, b"\\dlg\\") || contains(&data, b"\\DLG\\") {
                    usages_of(&data, s, r, &mut out.usages)?;
                }
            }
        }
    }
    Ok(out)
}

fn contains(data: &[u8], needle: &[u8]) -> bool {
    data.windows(needle.len()).any(|w| w == needle)
}

/// The object's name, else its first string attribute that is not a path
/// (`ChangeNPCSettings` 0: `poi_p01_change_foreman_to_good`), else its GUID.
fn display_name(group: &SimGroup, object: &crate::resource::simgroup::Object) -> String {
    if let Some(name) = group.object_name(object) {
        return name.to_owned();
    }
    group
        .object_packets(object)
        .flat_map(|p| &p.attrs)
        .find_map(|a| match &a.value {
            Value::String(s) if !s.is_empty() && !s.contains('\\') => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| object.guid.to_string())
}

fn usages_of(
    data: &[u8],
    stream: usize,
    resource: usize,
    out: &mut Vec<(String, Usage)>,
) -> Result<()> {
    let group = SimGroup::parse(data)?;
    for object in &group.objects {
        for packet in group.object_packets(object) {
            for attr in &packet.attrs {
                let Value::String(text) = &attr.value else {
                    continue;
                };
                let lower = text.to_lowercase();
                if !lower.starts_with(DIALOGUE_PATH_PREFIX) {
                    continue;
                }
                let class = packet
                    .class_name
                    .map_or_else(|| format!("{:08x}", packet.class.0), str::to_owned);
                let attribute = match attr.name {
                    Some(name) => format!("{class}.{name}"),
                    None => format!("{class}.{}", attr.index),
                };
                out.push((
                    lower,
                    Usage {
                        stream,
                        resource,
                        object: display_name(&group, object),
                        class: object
                            .class_name()
                            .map_or_else(|| format!("{:08x}", object.class), str::to_owned),
                        attribute,
                    },
                ));
            }
        }
    }
    Ok(())
}

impl Catalog {
    /// Reads the dialogues, chatter sets, speech banks, SimGroups, string
    /// tables and archive listings of the game. `progress(done, total)` is
    /// called as streams are read.
    pub fn build(game: &GameInstance, progress: &(dyn Fn(usize, usize) + Sync)) -> Result<Self> {
        let total = game.streams().len();
        let done = std::sync::atomic::AtomicUsize::new(0);
        let scans: Vec<Result<Scan>> = (0..total)
            .into_par_iter()
            .map(|s| {
                let r = scan_stream(game, s);
                let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if n.is_multiple_of(50) || n == total {
                    progress(n, total);
                }
                r
            })
            .collect();
        let mut all = Scan::default();
        for scan in scans {
            // A stream that does not parse is left out, as the index does.
            let Ok(scan) = scan else { continue };
            all.dialogues.extend(scan.dialogues);
            all.chatter.extend(scan.chatter);
            all.sounds.extend(scan.sounds);
            all.usages.extend(scan.usages);
            all.text.extend(scan.text);
        }
        all.dialogues.sort_by(|a, b| a.name.cmp(&b.name));

        // Archive entries by file name.
        let mut archives = Vec::new();
        let mut entries = HashMap::new();
        let mut by_file: HashMap<String, Vec<String>> = HashMap::new();
        for path in files::original_archives(game.root())? {
            let archive = BigArchive::open(&path)?;
            let a = archives.len();
            for (e, entry) in archive.entries.iter().enumerate() {
                let name = normalize_name(&entry.name);
                let base = name.rsplit('\\').next().unwrap_or(&name).to_owned();
                entries.entry(name.clone()).or_insert((a, e));
                by_file.entry(base).or_default().push(name);
            }
            archives.push(archive);
        }
        for list in by_file.values_mut() {
            // The first voice (no language directory) first.
            list.sort_by_key(|n| (n.contains("\\es\\"), n.clone()));
            list.dedup();
        }
        let sounds: HashMap<u32, String> = all.sounds.into_iter().collect();
        let mut speech: HashMap<u32, Vec<SpeechLine>> = HashMap::new();
        let mut event_names = HashMap::new();
        for (at, set) in &all.chatter {
            for event in &set.events {
                event_names.insert(event.hash, event.name.clone());
                for line in &event.lines {
                    let file = sounds.get(&line.sound()).cloned();
                    let entries = file
                        .as_ref()
                        .and_then(|f| by_file.get(&format!("{}.snu", f.to_lowercase())))
                        .cloned()
                        .unwrap_or_default();
                    speech.entry(event.hash).or_default().push(SpeechLine {
                        chv: *at,
                        event: event.name.clone(),
                        line: line.clone(),
                        file,
                        entries,
                    });
                }
            }
        }
        let mut usages: HashMap<String, Vec<Usage>> = HashMap::new();
        for (path, usage) in all.usages {
            usages.entry(path).or_default().push(usage);
        }
        let index = ResourceIndex { entries: all.text };
        let languages = crate::text::language_tables(game, &index).unwrap_or_default();
        let mut catalog = Self {
            dialogues: all.dialogues,
            event_names,
            speech,
            usages,
            languages,
            subtitles: HashMap::new(),
            text_index: index,
            archives,
            entries,
        };
        catalog.subtitles = catalog.read_dialogue_subtitles(game);
        Ok(catalog)
    }

    /// English subtitles of every speech file the dialogues of the game play.
    fn read_dialogue_subtitles(&self, game: &GameInstance) -> HashMap<String, String> {
        let mut wanted = Vec::new();
        for d in &self.dialogues {
            let Ok(data) = game
                .open_stream(d.stream)
                .and_then(|s| s.resource_data(d.resource))
            else {
                continue;
            };
            let Ok(dialogue) = Dialogue::parse(&data) else {
                continue;
            };
            for (_, event) in dialogue.events() {
                for line in self.speech.get(&event).into_iter().flatten() {
                    wanted.extend(line.entries.first().cloned());
                }
            }
        }
        wanted.sort();
        wanted.dedup();
        wanted
            .par_iter()
            .filter_map(|entry| {
                let data = self.read_head(entry).ok()?;
                let table = speech::subtitles(&data).ok()??;
                let col = speech::language_column(&table, "en").unwrap_or(0);
                Some((entry.clone(), table.strings.get(col)?.first()?.clone()))
            })
            .collect()
    }

    /// The beginning of an archive entry of the game, enough for its metadata.
    fn read_head(&self, entry: &str) -> Result<Vec<u8>> {
        let &(a, e) = self
            .entries
            .get(&normalize_name(entry))
            .ok_or_else(|| Error::InvalidArgument(format!("no archive entry {entry}")))?;
        let archive = &self.archives[a];
        let item = &archive.entries[e];
        let mut file =
            std::fs::File::open(&archive.path).map_err(|x| Error::io(&archive.path, x))?;
        let read = |file: &mut std::fs::File, len: u64| -> Result<Vec<u8>> {
            let mut buf = vec![0u8; len.min(item.size) as usize];
            file.seek(SeekFrom::Start(item.offset))
                .and_then(|_| file.read_exact(&mut buf))
                .map_err(|x| Error::io(&archive.path, x))?;
            Ok(buf)
        };
        let head = read(&mut file, 0x10000)?;
        let blocks = if head.len() >= 12 {
            u32::from_le_bytes(head[8..12].try_into().expect("4 bytes")) as u64
        } else {
            0
        };
        if blocks > head.len() as u64 {
            return read(&mut file, blocks);
        }
        Ok(head)
    }

    /// The whole archive entry as the workspace has it: the changed file, or
    /// the game's.
    pub fn read_entry(
        &self,
        game: &GameInstance,
        ws: Option<&Workspace>,
        entry: &str,
    ) -> Result<Vec<u8>> {
        let key = FileKey::archive_entry(entry);
        if let Some(ws) = ws
            && ws.file_change(&key).is_some()
        {
            return ws.current_file(game, &key);
        }
        let &(a, e) = self
            .entries
            .get(key.name())
            .ok_or_else(|| Error::InvalidArgument(format!("no archive entry {entry}")))?;
        self.archives[a].read(&self.archives[a].entries[e])
    }

    /// The subtitle table of a speech file as the workspace has it.
    pub fn subtitles_of(
        &self,
        game: &GameInstance,
        ws: Option<&Workspace>,
        entry: &str,
    ) -> Result<Option<StringTable>> {
        let key = FileKey::archive_entry(entry);
        let data = match ws.filter(|ws| ws.file_change(&key).is_some()) {
            Some(ws) => ws.current_file(game, &key)?,
            None => self.read_head(entry)?,
        };
        speech::subtitles(&data)
    }

    /// The recorded lines of a speech event.
    pub fn lines(&self, event: u32) -> &[SpeechLine] {
        self.speech.get(&event).map_or(&[], Vec::as_slice)
    }

    /// The recorded lines a line of a dialogue most likely plays, best
    /// first, one per file. The event names are shared by many characters
    /// (`N3_LINE1`); the speaker's chatter set decides at run time. Lines of
    /// chatter sets in the streams of the dialogue's users come first, and
    /// for actor 1 (the player) Dominic's voice (`dmnc`), for actor 2 the others.
    pub fn candidates(&self, event: u32, speaker: u32, near: &[usize]) -> Vec<&SpeechLine> {
        let mut lines: Vec<&SpeechLine> = self.lines(event).iter().collect();
        let player = |l: &SpeechLine| l.file.as_deref().is_some_and(|f| f.contains("_dmnc_"));
        lines.sort_by_key(|l| {
            (
                !near.contains(&l.chv.0),
                (speaker == 1) != player(l),
                l.entries.first().cloned(),
            )
        });
        let mut seen = std::collections::HashSet::new();
        lines.retain(|l| seen.insert(l.entries.first().cloned().unwrap_or_default()));
        lines
    }

    /// Streams of the SimGroups that start a dialogue.
    pub fn user_streams(&self, dialogue: &DialogueRef) -> Vec<usize> {
        let mut s: Vec<usize> = self.usages_of(dialogue).iter().map(|u| u.stream).collect();
        s.sort_unstable();
        s.dedup();
        s
    }

    /// Name of a speech event, or its hash.
    pub fn event_name(&self, event: u32) -> String {
        self.event_names
            .get(&event)
            .cloned()
            .unwrap_or_else(|| format!("{event:08x}"))
    }

    /// The SimGroup attributes that start a dialogue.
    pub fn usages_of(&self, dialogue: &DialogueRef) -> &[Usage] {
        self.usages.get(&dialogue.path()).map_or(&[], Vec::as_slice)
    }

    /// The dialogue in stream `stream`.
    pub fn find(&self, name_or_hash: &str) -> Option<&DialogueRef> {
        let q = name_or_hash
            .replace('\\', "/")
            .to_lowercase()
            .trim_start_matches("0x")
            .to_owned();
        self.dialogues
            .iter()
            .find(|d| d.name == q || d.name.ends_with(&format!("/{q}")))
    }
}

/// The game's language tables as the workspace has them: `(code, table)`.
pub fn language_strings(
    game: &GameInstance,
    ws: Option<&Workspace>,
    tables: &[TableRef],
) -> Vec<(String, StringTable)> {
    tables
        .iter()
        .filter_map(|t| {
            let data = match ws {
                Some(ws) => ws.current_data(game, t.stream, t.resource).ok()?,
                None => game
                    .open_stream(t.stream)
                    .ok()?
                    .resource_data(t.resource)
                    .ok()?,
            };
            let table = StringTable::parse(&data).ok()?;
            Some((t.language.clone().unwrap_or_default(), table))
        })
        .collect()
}

/// Text of string `name` in a one-language table.
pub fn lookup(table: &StringTable, name: &str) -> Option<String> {
    let i = table.find(string_id(name))?;
    table.strings.first()?.get(i).cloned()
}

/// Sets the text of an existing string in the table of one language (one
/// undoable workspace operation). Returns whether anything changed.
pub fn set_string(
    game: &GameInstance,
    ws: &mut Workspace,
    tables: &[TableRef],
    language: &str,
    name: &str,
    text: &str,
) -> Result<bool> {
    let t = tables
        .iter()
        .find(|t| t.language.as_deref() == Some(language))
        .ok_or_else(|| Error::InvalidArgument(format!("no string table for `{language}`")))?;
    let mut table = StringTable::parse(&ws.current_data(game, t.stream, t.resource)?)?;
    let i = table.find(string_id(name)).ok_or_else(|| {
        Error::InvalidArgument(format!(
            "`{name}` is not in the `{language}` table; add it first"
        ))
    })?;
    if table.strings[0][i] == text {
        return Ok(false);
    }
    table.strings[0][i] = text.to_owned();
    let label = format!("Text of {name} ({language})");
    let edit = ResourceEdit {
        index: t.resource,
        data: table.to_bytes()?,
        storage: StorageChoice::Keep,
    };
    Ok(ws
        .change_resources(game, &label, &[(t.stream, vec![edit])], &[])?
        .is_some())
}

/// Sets the subtitle of a speech file in one language column (`en`, `es`,
/// ...) and records the file in the workspace (overlay archive). Returns
/// whether anything changed.
pub fn set_subtitle(
    catalog: &Catalog,
    game: &GameInstance,
    ws: &mut Workspace,
    entry: &str,
    language: &str,
    text: &str,
) -> Result<bool> {
    let data = catalog.read_entry(game, Some(ws), entry)?;
    let mut table = speech::subtitles(&data)?
        .ok_or_else(|| Error::InvalidArgument(format!("{entry} has no subtitles")))?;
    let col = speech::language_column(&table, language)
        .ok_or_else(|| Error::InvalidArgument(format!("{entry} has no `{language}` subtitle")))?;
    let cell = table.strings[col]
        .first_mut()
        .ok_or_else(|| Error::InvalidArgument(format!("{entry} has an empty subtitle table")))?;
    if cell == text {
        return Ok(false);
    }
    *cell = text.to_owned();
    let new = speech::with_subtitles(&data, &table)?;
    let name = entry.rsplit('\\').next().unwrap_or(entry);
    ws.set_file(
        game,
        &FileKey::archive_entry(entry),
        new,
        &format!("Subtitle of {name} ({language})"),
    )?;
    Ok(true)
}

/// Records a new version of a dialogue (one undoable operation).
pub fn save(
    game: &GameInstance,
    ws: &mut Workspace,
    dialogue: &DialogueRef,
    data: &Dialogue,
    label: &str,
) -> Result<bool> {
    ws.set_resource(
        game,
        dialogue.stream,
        dialogue.resource,
        data.to_bytes()?,
        label,
    )
}

/// The workspace key of a dialogue.
pub fn key(game: &GameInstance, d: &DialogueRef) -> Result<ResourceKey> {
    let stream = game.open_stream(d.stream)?;
    let header = stream
        .layout()
        .resources
        .get(d.resource)
        .ok_or_else(|| Error::InvalidArgument(format!("no resource {}", d.resource)))?
        .header;
    Ok(ResourceKey::new(&d.name, &header))
}

/// A dialogue described for people and tools: [`Report::of`].
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub name: String,
    pub stream: String,
    pub used_by: Vec<Usage>,
    pub nodes: Vec<NodeReport>,
    pub animations: Vec<String>,
    /// The tree as data: what `dialogue-import` reads back.
    pub data: Dialogue,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeReport {
    pub id: usize,
    pub kind: String,
    pub summary: String,
    pub children: Vec<Option<usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<LineReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<OptionReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LineReport {
    pub speaker: u32,
    pub event: String,
    pub animations: BTreeMap<String, Vec<String>>,
    /// Recorded lines of the event: archive entry and English subtitle.
    pub speech: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OptionReport {
    pub option: usize,
    pub label: Option<String>,
    pub text: Option<String>,
    pub target: Option<usize>,
}

impl Report {
    /// Describes `d` with names and texts from the catalog and `strings`
    /// (a one-language table for option texts).
    pub fn of(
        catalog: &Catalog,
        dialogue: &DialogueRef,
        d: &Dialogue,
        strings: Option<&StringTable>,
    ) -> Self {
        let near = catalog.user_streams(dialogue);
        let nodes = (0..d.nodes.len())
            .map(|i| node_report(catalog, d, i, strings, &near))
            .collect();
        Self {
            name: dialogue
                .name
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned(),
            stream: dialogue.name.clone(),
            used_by: catalog.usages_of(dialogue).to_vec(),
            nodes,
            animations: d.animations.iter().map(|a| a.name.clone()).collect(),
            data: d.clone(),
        }
    }
}

/// Describes node `i` (see [`Report`]).
pub fn node_report(
    catalog: &Catalog,
    d: &Dialogue,
    i: usize,
    strings: Option<&StringTable>,
    near: &[usize],
) -> NodeReport {
    let node = &d.nodes[i];
    let mut summary = dlg::summary(d, i);
    let line = (node.kind == KIND_LINE).then(|| {
        let event = node.word(LINE_EVENT).unwrap_or(0);
        let mut animations = BTreeMap::new();
        for (label, w) in [
            ("lead", LINE_ANIM_LEAD),
            ("speaker", LINE_ANIM_SPEAKER),
            ("listener", LINE_ANIM_LISTENER),
        ] {
            if let Some(g) = node.word(w).filter(|&g| g != NONE) {
                animations.insert(
                    label.to_owned(),
                    d.group_clips(g).into_iter().map(str::to_owned).collect(),
                );
            }
        }
        let speaker = node.word(LINE_SPEAKER).unwrap_or(0);
        let speech = catalog
            .candidates(event, speaker, near)
            .into_iter()
            .filter_map(|l| {
                let entry = l.entries.first()?;
                Some((entry.clone(), catalog.subtitles.get(entry).cloned()))
            })
            .collect();
        let name = catalog.event_name(event);
        summary = format!("actor {speaker} says {name}");
        LineReport {
            speaker,
            event: name,
            animations,
            speech,
        }
    });
    let options = if node.kind == KIND_CHOICE {
        (0..CHOICE_SLOTS.max(node.children.len()))
            .filter_map(|o| {
                let label = d.option_label(i, o).map(str::to_owned);
                let target = node.children.get(o).copied().flatten();
                if label.is_none() && target.is_none() {
                    return None;
                }
                let text = label
                    .as_deref()
                    .and_then(|l| strings.and_then(|t| lookup(t, l)));
                Some(OptionReport {
                    option: o,
                    label,
                    text,
                    target,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    NodeReport {
        id: i,
        kind: dlg::kind_name(node.kind),
        summary,
        children: node.children.clone(),
        line,
        options,
    }
}

/// A search hit.
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    /// Index into [`Catalog::dialogues`].
    pub dialogue: usize,
    pub node: Option<usize>,
    pub what: String,
}

/// Searches every dialogue (as the workspace has it) for `query` in option
/// labels and texts, speech event names, English subtitles and clip names;
/// case is ignored.
pub fn search(
    catalog: &Catalog,
    game: &GameInstance,
    ws: Option<&Workspace>,
    strings: Option<&StringTable>,
    query: &str,
) -> Vec<Hit> {
    let q = query.to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let has = |s: &str| s.to_lowercase().contains(&q);
    let mut hits: Vec<Hit> = Vec::new();
    for (di, dref) in catalog.dialogues.iter().enumerate() {
        let first = hits.len();
        let data = match ws {
            Some(ws) => ws.current_data(game, dref.stream, dref.resource),
            None => game
                .open_stream(dref.stream)
                .and_then(|s| s.resource_data(dref.resource)),
        };
        let Ok(d) = data.and_then(|b| Dialogue::parse(&b)) else {
            continue;
        };
        if has(&dref.name) {
            hits.push(Hit {
                dialogue: di,
                node: None,
                what: dref.name.clone(),
            });
        }
        for (i, node) in d.nodes.iter().enumerate() {
            if node.kind == KIND_LINE {
                let event = node.word(LINE_EVENT).unwrap_or(0);
                let name = catalog.event_name(event);
                if has(&name) {
                    hits.push(Hit {
                        dialogue: di,
                        node: Some(i),
                        what: name.clone(),
                    });
                }
                for line in catalog.lines(event) {
                    if let Some(text) = line.entries.first().and_then(|e| catalog.subtitles.get(e))
                        && has(text)
                        && !hits[first..]
                            .iter()
                            .any(|h| h.node == Some(i) && &h.what == text)
                    {
                        hits.push(Hit {
                            dialogue: di,
                            node: Some(i),
                            what: text.clone(),
                        });
                    }
                }
            }
            if node.kind == KIND_CHOICE {
                for o in 0..CHOICE_SLOTS {
                    let Some(label) = d.option_label(i, o) else {
                        continue;
                    };
                    let text = strings.and_then(|t| lookup(t, label)).unwrap_or_default();
                    if has(label) || has(&text) {
                        hits.push(Hit {
                            dialogue: di,
                            node: Some(i),
                            what: format!("{label}: {text}"),
                        });
                    }
                }
            }
            if let Some(t) = Test::of(node)
                && has(&format!("{:#x}", t.variable))
            {
                hits.push(Hit {
                    dialogue: di,
                    node: Some(i),
                    what: dlg::summary(&d, i),
                });
            }
        }
        for a in &d.animations {
            if has(&a.name) && !hits[first..].iter().any(|h| h.what == a.name) {
                hits.push(Hit {
                    dialogue: di,
                    node: None,
                    what: a.name.clone(),
                });
            }
        }
    }
    hits
}

/// Writes the [`Report`] of every dialogue as JSON files
/// `<dir>/<hash>.json`; returns the number written.
pub fn export_all(
    catalog: &Catalog,
    game: &GameInstance,
    ws: Option<&Workspace>,
    strings: Option<&StringTable>,
    dir: &Path,
) -> Result<usize> {
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let mut n = 0;
    for dref in &catalog.dialogues {
        let data = match ws {
            Some(ws) => ws.current_data(game, dref.stream, dref.resource)?,
            None => game
                .open_stream(dref.stream)?
                .resource_data(dref.resource)?,
        };
        let d = Dialogue::parse(&data)?;
        let report = Report::of(catalog, dref, &d, strings);
        let path = dir.join(format!("{}.json", report.name));
        let json = serde_json::to_vec_pretty(&report)
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        crate::atomic::write(&path, &json)?;
        n += 1;
    }
    Ok(n)
}
