//! Command line interface of doctor_toolkit.

mod anim_cmd;
mod class_cmd;
mod collision_cmd;
mod crash_cmd;
mod create_cmd;
mod cutscene_cmd;
mod dialogue_cmd;
mod edit_cmd;
mod graph_cmd;
mod kb_cmd;
mod level_cmd;
mod live_cmd;
mod live_object_cmd;
mod live_state_cmd;
mod logic_cmd;
mod logic_graph_cmd;
mod logic_run_cmd;
mod lua_cmd;
mod material_cmd;
mod mcp_cmd;
mod model_cmd;
mod mods_cmd;
mod nav_cmd;
mod partsdb_cmd;
mod play_cmd;
mod project_cmd;
mod render_cmd;
mod save_cmd;
mod sbk_cmd;
mod script_cmd;
mod sound_cmd;
mod text_cmd;
mod tunables_cmd;
mod types_cmd;
mod venue_cmd;
mod video_cmd;
mod voice_cmd;
mod workspace_cmd;

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rayon::prelude::*;

use doctor_toolkit::container::ustream::{self, BlockKind, Replacement, Storage, StreamLayout};
use doctor_toolkit::game::{GameInstance, ReplaceOptions, StreamFile};
use doctor_toolkit::report;
use doctor_toolkit::resource::names::{mem_policies, type_tags};
use doctor_toolkit::toc::Toc;
use doctor_toolkit::workspace::{ResourceEdit, StorageChoice};

use workspace_cmd::{EditMode, WorkspaceCommand};

mod formats;

#[derive(Parser)]
#[command(
    name = "doctor-toolkit",
    version,
    about = "Tools for the game files of the EARS engine"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print a summary of a stream (.str) or a table of contents (.toc)
    Info { file: PathBuf },

    /// Add a new resource to a stream (recorded in the workspace)
    ///
    /// The memory policy and alignment default to those of the type; the ids
    /// default to a new id no resource of the game uses.
    AddResource(create_cmd::AddResource),

    /// Clone resources under new ids (recorded in the workspace)
    ///
    /// References between the clones are rewritten: a cloned mesh uses the
    /// cloned materials, a cloned material the cloned textures, a cloned
    /// metamodel the cloned meshes. `--retarget` points existing resources at
    /// the clones.
    CloneResource(create_cmd::CloneResource),

    /// Create a new stream with entries in the tables of contents (recorded in the workspace)
    AddStream(create_cmd::AddStream),

    /// Add a new SimGroup to a stream: empty, or with copies of the objects
    /// of another (recorded in the workspace)
    ///
    /// The stream is one of the game or one made with `add-stream`; `level
    /// add` then places objects in it by its stream and resource index.
    AddSimgroup(create_cmd::AddSimGroup),

    /// Import a glTF model over a model of the game or as a new model set
    /// (recorded in the workspace)
    ///
    /// Skinned meshes bind to the game skeleton by bone name; materials
    /// named like the model's reuse them, other materials are cloned with
    /// the glTF textures. `--new NAME` clones the model under new ids;
    /// `--retarget` points a SimGroup or metamodel at the clone.
    ModelImport(model_cmd::ModelImport),
    /// Show what a model file (glTF, FBX, OBJ, USD) holds as the importer reads it
    ModelInfo(model_cmd::ModelInfo),

    /// Build new Havok collision for a metamodel (recorded in the workspace)
    ///
    /// The geometry is a glTF file or the metamodel's own mesh; the new `hkx`
    /// (a static triangle mesh with MOPP code, a convex hull or a box) goes
    /// before the metamodel and its collision parts point at it.
    CollisionBuild(collision_cmd::CollisionBuild),

    /// Export Havok collision (`hkx`) as OBJ or glTF with a JSON description
    CollisionExport(collision_cmd::CollisionExport),

    /// Remove a stream made by `add-stream` and everything added to it
    RemoveStream(create_cmd::RemoveStream),

    /// The world grid of a level (`wgi`): the streams the game loads by the
    /// position of the player; add a new stream to them
    #[command(subcommand)]
    WorldGrid(create_cmd::WorldGridCommand),

    /// The venues of a city (`bricknmortarres`): list them, add a new venue
    /// made after one of the game, change its owner, type or position
    #[command(subcommand)]
    Venue(venue_cmd::VenueCommand),

    /// Check that metamodel, simgroup, mesh and material references of streams resolve
    ///
    /// A reference resolves when the stream itself or a stream loaded before
    /// it (table-of-contents prerequisites) holds the target.
    CheckRefs(create_cmd::CheckRefs),

    /// Check the workspace's edits before they are written into the game
    ///
    /// Every changed resource is read by its parser, objects can show their
    /// models, references an edit adds resolve, conflicts with the game
    /// files, messages of changed SimGroups, the project. Fails when there
    /// are errors (warnings do not count).
    CheckEdits(create_cmd::CheckEdits),

    /// List the resources of a stream
    List {
        stream: PathBuf,
        /// Only resources of this type: a tag such as `tg4h` or a hexadecimal id
        #[arg(long = "type")]
        type_filter: Option<String>,
    },

    /// Extract the raw, decompressed data of resources
    Extract {
        stream: PathBuf,
        /// Output directory
        #[arg(short, long)]
        output: PathBuf,
        /// Resource index to extract; may be repeated. Default: all
        #[arg(long)]
        index: Vec<usize>,
        /// Only resources of this type
        #[arg(long = "type")]
        type_filter: Option<String>,
    },

    /// Print the entries of a table of contents
    Toc { file: PathBuf },

    /// Check every stream and table of contents of a game directory
    Validate {
        game_dir: PathBuf,
        /// Also decompress every resource
        #[arg(long)]
        decompress: bool,
        /// Also check that every file is reproduced byte for byte when rewritten
        #[arg(long)]
        roundtrip: bool,
    },

    /// Print the table of resource types found in a game directory
    Types {
        game_dir: PathBuf,
        /// Print a Markdown table
        #[arg(long)]
        markdown: bool,
    },

    /// Rewrite a stream into a new file
    Repack {
        input: PathBuf,
        output: PathBuf,
        /// Lay out all blocks again instead of copying the file structure
        #[arg(long)]
        relayout: bool,
    },

    /// Replace the data of one resource inside a game directory
    ///
    /// The change is recorded in the workspace; `--direct` also writes it into
    /// the game, after copying the originals to `_doctor_toolkit/backups`.
    Replace {
        game_dir: PathBuf,
        /// Stream name, e.g. `gf2/frontend`
        stream: String,
        /// Index of the resource to replace
        #[arg(long)]
        index: usize,
        /// File with the new raw data
        #[arg(long)]
        data: PathBuf,
        /// Store the data RefPack-compressed [default: like the original]
        #[arg(long, conflicts_with = "raw")]
        compress: bool,
        /// Store the data uncompressed [default: like the original]
        #[arg(long)]
        raw: bool,
        /// Write straight into the game without the workspace and do not
        /// update the stream size in the tables of contents (for experiments)
        #[arg(long, conflicts_with_all = ["workspace", "direct", "workspace_dir"])]
        no_toc: bool,
        #[command(flatten)]
        mode: EditMode,
    },

    /// Rewrite every resource of a stream inside a game directory with the given storage
    ///
    /// The data stays the same; only the way it is stored changes. Useful to
    /// check that the game accepts files written by this tool.
    Rewrite {
        game_dir: PathBuf,
        /// Stream name, e.g. `gf2/frontend`
        stream: String,
        /// Store resources RefPack-compressed instead of raw
        #[arg(long)]
        compress: bool,
    },

    /// Export textures as DDS (lossless) or PNG (largest mip level)
    ///
    /// SOURCE is a stream file or a game directory. Files are written to
    /// `<output>/<stream path>/<texture name>.dds`.
    Textures {
        source: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// Write PNG images instead of DDS files
        #[arg(long)]
        png: bool,
        /// Only textures whose name contains this text
        #[arg(long)]
        name: Option<String>,
    },

    /// Export models (`mesh`) and trees (`treemodel`) of a stream with
    /// materials, textures and skins (glTF, FBX, OBJ or USD)
    Models {
        game_dir: PathBuf,
        /// Stream name, e.g. `gf2/miami/local/danny/high/building_004`
        stream: String,
        #[arg(short, long)]
        output: PathBuf,
        /// File format: glb, fbx, obj, usd, usda, usdc or usdz
        #[arg(long, default_value = "glb")]
        format: doctor_toolkit::scene::ModelFormat,
        /// Only models whose name contains this text
        #[arg(long)]
        name: Option<String>,
        /// Geometry only, without materials, textures and skeletons
        #[arg(long)]
        no_textures: bool,
    },

    /// Replace a texture with a DDS or PNG file
    ///
    /// PNG images are encoded in the texture's format with a full mip chain;
    /// DDS files are used as they are. The change is recorded in the
    /// workspace; `--direct` also writes it into the game.
    TextureImport {
        game_dir: PathBuf,
        /// Stream name, e.g. `gf2/frontend`
        stream: String,
        /// Texture name, as shown by `textures` or the GUI. When several
        /// textures share the name (clones), `NAME_HASH` as in the file
        /// names of `textures`, e.g. `white_c_noalpha_d7027d83`
        texture: String,
        /// The new image (.dds or .png)
        file: PathBuf,
        /// Replace every copy of the texture: the one with the same name
        /// hash in every stream of the game (as the Retexture wizard does)
        #[arg(long)]
        all_copies: bool,
        #[command(flatten)]
        mode: EditMode,
    },

    /// Export Flash interface files (`apx`): `.apt` and `.const` data plus embedded textures
    ApxExport {
        game_dir: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// Write embedded textures as PNG instead of DDS
        #[arg(long)]
        png: bool,
    },

    /// Export localised string tables (`lh2`) as editable JSON files
    TextExport {
        game_dir: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Write edited JSON string tables back into the game
    ///
    /// Each file must come from `text-export`. The changes are recorded in the
    /// workspace; `--direct` also writes them into the game.
    TextImport {
        game_dir: PathBuf,
        /// JSON files written by `text-export`
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[command(flatten)]
        mode: EditMode,
    },

    /// Add a new string to the string tables of every language (recorded in the workspace)
    ///
    /// The entry id is the hash of NAME; interface movies show the string as
    /// `$NAME`, SimGroup hash attributes by its hash.
    TextAdd(text_cmd::TextAdd),

    /// List the `$name` text references of an interface movie (apx), or point one at another string
    ApxTextRefs(text_cmd::ApxTextRefs),

    /// Show the parameters of a material, or set them: tint colours and other numbers, textures (recorded in the workspace)
    ///
    /// A material is shared by every model that names it. Without `--set`
    /// the parameters are listed as `--set` takes them.
    Material(material_cmd::MaterialArgs),

    /// Export game-logic resources (simgroup, metamodel, variabledictionary, partsdb, graph) as JSON
    ///
    /// Files are written to `<output>/<stream path>/<index>_<type>_<id>.json`.
    /// Resources the workspace changes are exported as edited. Edited files
    /// of the first three types go back with `logic-import`.
    LogicExport {
        game_dir: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// Only this type, e.g. `simgroup`. Default: all supported types
        #[arg(long = "type")]
        type_filter: Option<String>,
        /// Only streams whose path contains this text, e.g. `mp_mall`
        #[arg(long)]
        stream: Option<String>,
    },

    /// Edit a resource in another program; every save is imported back
    ///
    /// The resource is exported to a working file (PNG or DDS for textures,
    /// CSV or JSON for string tables, JSON for logic, Lua source for
    /// scripts, GLB for models) and opened with `--with`, the program of the
    /// settings or the desktop's program. Each save is imported into the
    /// workspace; saves close together make one undoable operation. Runs
    /// until Ctrl+C (or until the program exits, with `--wait`).
    Edit(edit_cmd::Edit),

    /// Write edited JSON files from `logic-export` back (simgroup, metamodel, variabledictionary, partsdb)
    ///
    /// Only values that differ from the resource are written: SimGroup
    /// attribute values (a removed attribute becomes the default), object
    /// `position` and `model`, metamodel property and condition values,
    /// variable values. A parts database is rewritten whole from its file
    /// (new parts, variants, sets). The changes are recorded in the workspace; `--direct`
    /// also writes them into the game.
    LogicImport {
        game_dir: PathBuf,
        /// JSON files written by `logic-export`
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[command(flatten)]
        mode: EditMode,
    },

    /// Change attributes, position, rotation, scale or model of one SimGroup object
    ///
    /// The change is recorded in the workspace; `--direct` also writes it into the game.
    SimgroupSet(logic_cmd::SimGroupSet),

    /// Add an object of a class with empty attributes to a SimGroup
    ///
    /// The object gets the packets the schema lists for the class and a new
    /// GUID; set its attributes with `simgroup-set`. Classes of native mods
    /// are known when DOCTOR_TOOLKIT_CLASSES names their class files.
    SimgroupAdd(class_cmd::SimGroupAdd),

    /// Check the class files of native mods and print their classes
    ClassSchema(class_cmd::ClassSchema),

    /// Replace an embedded texture, the `.apt` movie or the `.const` table of an apx file
    ///
    /// Textures are encoded like `texture-import`. The change is recorded in
    /// the workspace; `--direct` also writes it into the game.
    ApxImport(logic_cmd::ApxImport),

    /// Extract resources of one type from every stream of a game directory
    ///
    /// Files are written to `<output>/<stream path>/<index>_<type>_<id>.bin`.
    Dump {
        game_dir: PathBuf,
        /// Resource type: a tag such as `tg4h` or a hexadecimal id
        #[arg(long = "type")]
        type_filter: String,
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Restore all files changed by this tool from their backups
    ///
    /// Changes recorded in the workspace stay there, pending.
    Restore { game_dir: PathBuf },

    /// Edits kept on disk with history: status, diff, apply, revert, undo, redo
    #[command(subcommand)]
    Workspace(WorkspaceCommand),
    /// Export rigs (`rcb`) as JSON: bone names, parents and bind poses
    ///
    /// Files are written to `<output>/<stream path>/<index>_rcb_<id>.json`.
    /// `models` attaches the matching skeleton to skinned models automatically.
    Skeletons {
        game_dir: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Export sounds: `sbk` banks and the streamed audio of the `.viv` archives
    ///
    /// Sounds are written as `.snu` files (EA SNR/SNS, EA-XAS v1), which
    /// vgmstream plays, or decoded to WAV with `--wav`. Bank sounds go to
    /// `<output>/<stream path>/<index>_sbk_<id>/<sound>.snu`, archive files keep
    /// their paths (`<output>/audiostreams/...`).
    Audio {
        game_dir: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// Decode to 16-bit PCM WAV instead of copying the encoded data
        #[arg(long)]
        wav: bool,
        /// Only sounds whose path contains this text
        #[arg(long)]
        name: Option<String>,
        /// Skip the `.viv` archives (4.7 GB of streamed speech and music)
        #[arg(long)]
        banks_only: bool,
    },

    /// Export fonts (`ffn`): the glyph atlas as PNG and glyph metrics as JSON
    ///
    /// Files are written to `<output>/<stream path>/<font name>.png` and `.json`.
    Fonts {
        game_dir: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Export scripts (`lua`) as Lua 5.0 bytecode files (`.luac`)
    ///
    /// The chunks use 4-byte float numbers; decompilers need that setting.
    LuaExport {
        game_dir: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Decompile scripts (`lua`) to Lua 5.0 source and check the round trip
    ///
    /// The input is a `.luac` file or a game directory (every script, written
    /// to `<output>/<stream path>/<name>.lua`). Each script is compiled again
    /// and compared with the original: byte-identical, same instructions, or different.
    LuaDecompile(lua_cmd::Decompile),

    /// Compile Lua 5.0 source to a chunk with the game's 4-byte float numbers
    LuaCompile(lua_cmd::Compile),

    /// Print the instructions of a Lua 5.0 chunk
    LuaList { input: PathBuf },

    /// Compile Lua sources and replace the game's scripts (recorded in the workspace)
    ///
    /// Each file replaces the script with the same file name; the chunk keeps
    /// the source name of the original.
    LuaImport(lua_cmd::Import),

    /// List the nodes of visual scripts: kinds, pins and settings
    ScriptNodes(script_cmd::Nodes),

    /// Compile a visual script (graph JSON) to Lua 5.0 source and a game chunk
    ScriptCompile(script_cmd::Compile),

    /// Run a visual script (or a Lua file) against a mock of the game: send
    /// messages, let time pass, print what the script does
    ScriptRun(script_cmd::Run),

    /// Put a visual script into a level: a `lua` resource and a `LuaAudioScript`
    /// object (recorded in the workspace; deploying again updates it)
    ScriptDeploy(script_cmd::Deploy),

    /// Take a deployed visual script out of a level (recorded in the workspace)
    ScriptRemove(script_cmd::Remove),

    /// List the visual scripts deployed to a level stream
    ScriptList(script_cmd::List),

    /// Mods: build from the workspace, install (.zip, .vpat), order, conflicts, apply
    #[command(subcommand)]
    Mods(mods_cmd::ModsCommand),

    /// Check the game files against their originals and tell who changed them
    Integrity(mods_cmd::IntegrityArgs),

    /// Find installed copies of the game and tell their edition
    ///
    /// Searches Steam libraries (native, Flatpak, Snap; Proton prefixes and
    /// non-Steam shortcuts), Wine prefixes (registry and install folders,
    /// also of Origin and the EA app), Lutris, Heroic and Bottles; on
    /// Windows the registry and the program folders. Only reads.
    Detect(play_cmd::DetectArgs),

    /// Start the game: by Steam, Wine, Proton, Lutris, Heroic, Bottles or a
    /// command of your own
    ///
    /// `--apply` writes the workspace into the game first, `--revert-after`
    /// waits for the game to exit and puts the original files back.
    Play(play_cmd::PlayArgs),

    /// The graphics module (stage 59): its configuration, keeping it in the
    /// game for good, its log
    ///
    /// `doctor_render.asi` fixes the time of day, makes the game's vsync 60
    /// frames a second instead of 30, limits frames, widens the field of
    /// view and draws FXAA and sharpening over the frame. `play --render`
    /// puts it into the game for one launch. See
    /// docs/analysis/performance.md §10.
    #[command(subcommand)]
    Render(render_cmd::RenderCommand),

    /// Crash reports of the game (stage 99): show and list them, the crash
    /// module, a crash on purpose
    ///
    /// `doctor_crash.asi` (loaded by the ASI loader, PC 1.0.789.0 only)
    /// writes a minidump and a text report into `_doctor_toolkit/crashes/`
    /// of the game folder when the game crashes: the place by module and
    /// by the names of the knowledge base, the stack, the last messages of
    /// the bus and errors of Lua, the streams, the enabled mods. It needs
    /// no live module. See docs/analysis/crash-reports.md.
    #[command(subcommand)]
    CrashReport(crash_cmd::CrashCommand),

    /// Live link with the running game: player, camera, objects, teleport,
    /// messages, Lua (stage 34)
    ///
    /// Talks to the toolkit's module in the game (`scripts/doctor_live.asi`,
    /// loaded by the ASI loader) on 127.0.0.1. Reads are free; teleport,
    /// send and lua change the running game and need `--yes`. See
    /// docs/analysis/live-link.md.
    #[command(subcommand)]
    Live(live_cmd::LiveCommand),

    /// Describe a movie (`.vp6`), or list all movies of a game directory
    VideoInfo(video_cmd::InfoArgs),

    /// Export a movie: audio as WAV, frames as PNG, or everything through ffmpeg
    VideoExport(video_cmd::ExportArgs),

    /// Replace a movie of the game: new video (Y4M encoded to VP6, or VP6 frames
    /// from another movie, AVI or FLV), new or kept audio, new frame rate
    ///
    /// The original is backed up first; `restore` puts it back.
    VideoImport(video_cmd::ImportArgs),

    /// Encode a sound (WAV or anything ffmpeg reads) as EA-XAS: `.snu` or `.snr`
    SoundEncode(sound_cmd::EncodeArgs),

    /// Replace a sound of an `sbk` bank or a streamed sound of the `.viv`
    /// archives (recorded in the workspace)
    ///
    /// The new sound is converted to the channels, rate and loop of the old one.
    /// Bank sounds rebuild their `sbk` resource; streamed sounds go into an
    /// overlay archive (`GODFATH<n>.VIV`) written by `workspace apply`, the
    /// original archives are never rewritten.
    SoundImport(sound_cmd::ImportArgs),

    /// List the files of the `.viv` archives
    VivList(sound_cmd::VivListArgs),

    /// Show the structure of an `sbk` sound bank: emitters (sounds the game
    /// plays by `.emx` name), their parameters, variations and samples
    SbkInfo(sbk_cmd::InfoArgs),

    /// Change the parameters of a sound of an `sbk` bank: volume, pitch,
    /// random spreads, falloff, priority, flags (recorded in the workspace)
    SbkEdit(sbk_cmd::EditArgs),

    /// Add sounds to an `sbk` bank: as new variations of an emitter, or as a
    /// new emitter the game plays by its name (recorded in the workspace)
    ///
    /// The sounds are converted to the format of the emitter's first sound.
    /// A new emitter takes the parameters of `--like` (default: the first).
    SoundAdd(sbk_cmd::AddArgs),

    /// Add a voice line made from a recording: the sound file in the overlay
    /// archive, its entry in a speech bank (`smb`), the line in the
    /// speaker's chatter set (`chv`) and, for a new event, its record in the
    /// event table (`get`) (recorded in the workspace)
    VoiceAdd(voice_cmd::AddArgs),

    /// List the speech banks (`smb`) or find their entries: sound id, file,
    /// mix category, archive path
    SpeechBank(voice_cmd::BankArgs),

    /// Show the table of speech events (`get`), add records or change
    /// cool-down, priority, flags and rank (recorded in the workspace)
    SpeechEvents(voice_cmd::EventsArgs),

    /// Show the audio configuration (`auc`): mix categories, bank pools and
    /// ducks; add a mix category (recorded in the workspace)
    AudioConfig(voice_cmd::ConfigArgs),

    /// List the mixer snapshots (`mix`), show one, or set the volume,
    /// low-pass frequency and times of a category in it (recorded in the
    /// workspace)
    Mix(voice_cmd::MixArgs),

    /// Game parameters of the curated list (damage, health, prices, ...):
    /// list them with their values, show one, or change it in every object
    /// that has it with `--set ID=VALUE` or `--multiply ID=FACTOR`
    /// (recorded in the workspace)
    Tunables(tunables_cmd::TunablesArgs),

    /// Show a navigation graph (`graph`): type, nodes, links, neighbouring
    /// graphs, properties, R-tree and precomputed paths
    GraphInfo(graph_cmd::InfoArgs),

    /// Export a navigation graph as JSON (`graph-import` reads it back)
    GraphExport(graph_cmd::ExportArgs),

    /// Import a navigation graph from JSON (recorded in the workspace); link
    /// lengths, R-tree and precomputed paths follow edited positions and links
    GraphImport(graph_cmd::ImportArgs),

    /// Edit a navigation graph: move, add, attach and remove nodes, add and
    /// remove links, set flags (recorded in the workspace)
    GraphEdit(graph_cmd::EditArgs),

    /// List the face-to-face dialogues (`dlg`) and who starts them, or search them
    Dialogues(dialogue_cmd::ListArgs),

    /// Show a dialogue tree: lines with speech and subtitles, choices with texts
    Dialogue(dialogue_cmd::ShowArgs),

    /// Export every dialogue as JSON (tree, texts, speech, users)
    DialogueExport(dialogue_cmd::ExportArgs),

    /// Write dialogue trees edited as JSON back (recorded in the workspace)
    DialogueImport(dialogue_cmd::ImportArgs),

    /// Show or change the subtitles of a speech file of the `.viv` archives
    ///
    /// The file goes into the overlay archive like `sound-import`; the audio
    /// is kept.
    Subtitle(dialogue_cmd::SubtitleArgs),
    #[command(flatten)]
    Nav(nav_cmd::NavCommand),

    /// Projects: a mod as a folder of source files, built into the workspace when they change
    #[command(subcommand)]
    Project(project_cmd::ProjectCommand),

    /// Levels: zones, what the 3D view shows, duplicate, remove and add objects
    #[command(subcommand)]
    Level(level_cmd::LevelCommand),

    /// Parts databases: show, add parts and variants to sets, assemble a character as glTF
    #[command(subcommand)]
    Partsdb(partsdb_cmd::PartsdbCommand),

    /// Character animations (`bnk` banks): list, export (glTF, FBX, USD), import into banks, events
    #[command(subcommand)]
    Anim(anim_cmd::AnimCommand),

    /// In-engine cut-scenes: shots, time line, JSON, retiming, clips, subtitles, glTF
    #[command(subcommand)]
    Cutscene(cutscene_cmd::CutsceneCommand),

    /// The logic graph of a mission, checkpoint, stream or level (objects and messages)
    ///
    /// Prints a summary, `--dot` for Graphviz or `--json`. See docs/logic-graph.md.
    LogicGraph(logic_graph_cmd::GraphArgs),

    /// Who sends and who hears a message, across the whole game
    Messages(logic_graph_cmd::MessagesArgs),

    /// Wire a message sender to a listener in the logic graph (or `--disconnect`)
    ///
    /// Sets the message attributes as the logic editor does: the empty side
    /// takes the other's message, or the listener switches to the sender's.
    LogicConnect(logic_graph_cmd::ConnectArgs),

    /// List the missions (directories of streams under objectives/ and checkpoints/)
    LogicMissions(logic_graph_cmd::MissionsArgs),

    /// Run the logic of a mission without the game: messages, variables, timers, Lua
    ///
    /// Prints the journal of who sent what to whom and when (`--json` for
    /// JSON), stops at a time, a step count or breakpoints, and compares with
    /// a journal of the game (`--compare`). See docs/logic-graph.md §9.
    LogicRun(logic_run_cmd::RunArgs),

    /// What the schema of SimGroup pins (messages heard and sent, links) covers
    LogicSchema {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },

    /// Serve the game to an AI client (Claude Code, Claude Desktop) over the
    /// Model Context Protocol on stdin/stdout
    ///
    /// Read-only by default: search, resource info and JSON, hash names,
    /// strings, the logic graph, level objects, workspace and project
    /// status, the running game over the live link. `--allow-edits` adds
    /// edits recorded in the workspace (undoable), project and mod builds
    /// and checks; `--allow-game` writing the workspace into the game,
    /// running the game on an X server of its own, screenshots and writes
    /// into the running game. See docs/mcp.md.
    Mcp(mcp_cmd::McpArgs),

    /// Write the generated notes of the knowledge base docs/kb
    ///
    /// A note of every SimGroup class, of every resource type and the list
    /// of the engine's messages are made from data/gf2; every folder gets
    /// its list (index.md). The hand-written section of a generated note
    /// (between the kb:manual markers) is kept; a run that changes nothing
    /// writes nothing. See docs/kb/README.md.
    KbGen(kb_cmd::KbGen),

    /// Typed game data: value lists, meanings of attributes, messages
    ///
    /// `enums [KEY]` — families, cities, venues, weapons, crew specialties,
    /// sound categories, emotions; `message NAME` — where a message name
    /// comes from, who sends and hears it in the game files; `attrs CLASS` —
    /// what the attributes of a class hold; `stats`. Needs no game: the data
    /// is built in (data/gf2). See docs/logic-graph.md §2.1.
    #[command(subcommand)]
    GameTypes(types_cmd::TypesCommand),

    /// Write the files generated from data/gf2: JSON Schema and the Python
    /// module of the value lists
    ///
    /// `--check` writes nothing and fails when a file is out of date.
    TypesGen(types_cmd::TypesGen),

    /// Write docs/parity.md from data/toolkit/capabilities.toml: what the
    /// toolkit can do and where (MCP, editor, command line, Python)
    ///
    /// `--check` writes nothing and fails when the document is out of date.
    ParityGen(kb_cmd::ParityGen),

    /// Check the messages of the SimGroups the workspace changes
    ///
    /// An error for a new message name whose hash is the hash of a message
    /// of the game; warnings for messages left with one end (heard but never
    /// sent, sent but never heard). Lists the messages the workspace adds.
    LogicCheck(types_cmd::LogicCheck),

    /// Check the knowledge base docs/kb
    ///
    /// The properties of every note, broken links, orphans, questions
    /// without a next step and names of files left out of the public
    /// repository.
    KbLint(kb_cmd::KbLint),

    /// Look into a save file of the game (read only)
    ///
    /// Shows the name, profile, city, play time, checksums and the chunks
    /// of a save; `--chunk` shows what a chunk holds: story variables,
    /// money and family members, families, buildings, cities, weapons, the
    /// history log. With `--game` variables and objects get their names.
    /// A folder lists the saves in it. See docs/formats/save.md.
    Save(save_cmd::SaveArgs),
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    report::init("doctor-toolkit");
    if cli.command.may_write_game()
        && let Some(warning) = report::game_running_warning()
    {
        eprintln!("warning: {warning}");
    }
    let result = run(cli.command);
    workspace_cmd::release_locks();
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => std::process::ExitCode::from(report_error(&error)),
    }
}

impl Command {
    /// Whether the command can change the files of the game.
    fn may_write_game(&self) -> bool {
        use mods_cmd::ModsCommand as M;
        use workspace_cmd::WorkspaceCommand as W;
        match self {
            Self::Replace { no_toc, mode, .. } => *no_toc || mode.direct,
            Self::TextureImport { mode, .. } | Self::TextImport { mode, .. } => mode.direct,
            Self::Edit(args) => args.is_direct(),
            Self::Rewrite { .. } | Self::Restore { .. } => true,
            Self::Play(args) => args.writes(),
            Self::Render(command) => command.may_write_game(),
            Self::CrashReport(command) => command.may_write_game(),
            Self::Live(command) => command.may_write_game(),
            Self::Workspace(command) => matches!(command, W::Apply { .. } | W::Revert { .. }),
            Self::Mods(command) => matches!(command, M::Apply { .. } | M::Unapply { .. }),
            Self::Partsdb(command) => command.may_write_game(),
            Self::Tunables(args) => args.may_write_game(),
            Self::Project(command) => command.may_write_game(),
            Self::Mcp(args) => args.allow_game,
            _ => false,
        }
    }
}

/// Prints `error` with a hint, logs it and returns the exit code: 1, or the
/// code of a known problem (see [`report::Problem::exit_code`]).
fn report_error(error: &anyhow::Error) -> u8 {
    // Library I/O errors already include their source; print each message once.
    let mut text = String::new();
    for cause in error.chain() {
        let message = cause.to_string();
        if !text.contains(&message) {
            if !text.is_empty() {
                text.push_str(": ");
            }
            text.push_str(&message);
        }
    }
    eprintln!("error: {text}");
    let problem = error.chain().find_map(report::classify);
    if let Some(problem) = problem {
        eprintln!("hint: {}", problem.hint());
    }
    report::log(&format!("error: {text}"));
    if let Some(path) = report::log_path() {
        eprintln!("(logged to {})", path.display());
    }
    problem.map_or(1, report::Problem::exit_code)
}

fn run(command: Command) -> Result<()> {
    match command {
        Command::Info { file } => info(&file),
        Command::List {
            stream,
            type_filter,
        } => list(&stream, type_filter.as_deref()),
        Command::Extract {
            stream,
            output,
            index,
            type_filter,
        } => extract(&stream, &output, &index, type_filter.as_deref()),
        Command::Toc { file } => print_toc(&file),
        Command::Validate {
            game_dir,
            decompress,
            roundtrip,
        } => validate(&game_dir, decompress, roundtrip),
        Command::Types { game_dir, markdown } => types(&game_dir, markdown),
        Command::Repack {
            input,
            output,
            relayout,
        } => repack(&input, &output, relayout),
        Command::Replace {
            game_dir,
            stream,
            index,
            data,
            compress,
            raw,
            no_toc,
            mode,
        } => {
            let storage = match (compress, raw) {
                (true, _) => StorageChoice::Refpack,
                (_, true) => StorageChoice::Raw,
                _ => StorageChoice::Keep,
            };
            if no_toc {
                replace_without_workspace(&game_dir, &stream, index, &data, compress)
            } else {
                replace(&game_dir, &stream, index, &data, storage, &mode)
            }
        }
        Command::Rewrite {
            game_dir,
            stream,
            compress,
        } => rewrite(&game_dir, &stream, compress),
        Command::Textures {
            source,
            output,
            png,
            name,
        } => export_textures(&source, &output, png, name.as_deref()),
        Command::Models {
            game_dir,
            stream,
            output,
            format,
            name,
            no_textures,
        } => export_models(
            &game_dir,
            &stream,
            &output,
            format,
            name.as_deref(),
            no_textures,
        ),
        Command::TextureImport {
            game_dir,
            stream,
            texture,
            file,
            all_copies,
            mode,
        } => texture_import(&game_dir, &stream, &texture, &file, all_copies, &mode),
        Command::ApxExport {
            game_dir,
            output,
            png,
        } => apx_export(&game_dir, &output, png),
        Command::TextExport { game_dir, output } => text_export(&game_dir, &output),
        Command::TextImport {
            game_dir,
            files,
            mode,
        } => text_import(&game_dir, &files, &mode),
        Command::Dump {
            game_dir,
            type_filter,
            output,
        } => dump(&game_dir, &type_filter, &output),
        Command::LogicExport {
            game_dir,
            output,
            type_filter,
            stream,
        } => logic_export(
            &game_dir,
            &output,
            type_filter.as_deref(),
            stream.as_deref(),
        ),
        Command::LogicImport {
            game_dir,
            files,
            mode,
        } => logic_cmd::logic_import(&game_dir, &files, &mode),
        Command::SimgroupSet(args) => logic_cmd::simgroup_set(&args),
        Command::SimgroupAdd(args) => class_cmd::simgroup_add(&args),
        Command::ClassSchema(args) => class_cmd::class_schema(&args),
        Command::ApxImport(args) => logic_cmd::apx_import(&args),
        Command::Restore { game_dir } => restore(&game_dir),
        Command::Workspace(command) => workspace_cmd::run(command),
        Command::AddResource(args) => create_cmd::add_resource(args),
        Command::CloneResource(args) => create_cmd::clone_resource(args),
        Command::AddStream(args) => create_cmd::add_stream(args),
        Command::AddSimgroup(args) => create_cmd::add_simgroup(args),
        Command::ModelImport(args) => model_cmd::model_import(args),
        Command::Edit(args) => edit_cmd::edit(args),
        Command::ModelInfo(args) => model_cmd::model_info(args),
        Command::CollisionBuild(args) => collision_cmd::collision_build(args),
        Command::TextAdd(args) => text_cmd::text_add(args),
        Command::ApxTextRefs(args) => text_cmd::apx_text_refs(args),
        Command::Material(args) => material_cmd::material(args),
        Command::CollisionExport(args) => collision_cmd::collision_export(args),
        Command::RemoveStream(args) => create_cmd::remove_stream(args),
        Command::WorldGrid(command) => create_cmd::world_grid(command),
        Command::Venue(command) => venue_cmd::venue(command),
        Command::CheckRefs(args) => create_cmd::check_refs(args),
        Command::CheckEdits(args) => create_cmd::check_edits(args),
        Command::Skeletons { game_dir, output } => formats::skeletons(&game_dir, &output),
        Command::Audio {
            game_dir,
            output,
            wav,
            name,
            banks_only,
        } => formats::audio(&game_dir, &output, wav, name.as_deref(), banks_only),
        Command::Fonts { game_dir, output } => formats::fonts(&game_dir, &output),
        Command::LuaExport { game_dir, output } => formats::lua_export(&game_dir, &output),
        Command::LuaDecompile(args) => lua_cmd::decompile(&args),
        Command::LuaCompile(args) => lua_cmd::compile(&args),
        Command::LuaList { input } => lua_cmd::list(&input),
        Command::LuaImport(args) => lua_cmd::import(&args),
        Command::ScriptNodes(args) => script_cmd::nodes(&args),
        Command::ScriptCompile(args) => script_cmd::compile(&args),
        Command::ScriptRun(args) => script_cmd::run(&args),
        Command::ScriptDeploy(args) => script_cmd::deploy(&args),
        Command::ScriptRemove(args) => script_cmd::remove(&args),
        Command::ScriptList(args) => script_cmd::list(&args),
        Command::Mods(command) => mods_cmd::run(command),
        Command::Detect(args) => play_cmd::detect(&args),
        Command::Play(args) => play_cmd::play(&args),
        Command::Render(command) => render_cmd::run(&command),
        Command::CrashReport(command) => crash_cmd::run(&command),
        Command::Live(command) => live_cmd::run(command),
        Command::Integrity(args) => mods_cmd::run_integrity(args),
        Command::VideoInfo(args) => video_cmd::info(&args),
        Command::VideoExport(args) => video_cmd::export(&args),
        Command::VideoImport(args) => video_cmd::import(&args),
        Command::SoundEncode(args) => sound_cmd::encode(&args),
        Command::SoundImport(args) => sound_cmd::import(&args),
        Command::VivList(args) => sound_cmd::viv_list(&args),
        Command::SbkInfo(args) => sbk_cmd::info(&args),
        Command::SbkEdit(args) => sbk_cmd::edit(&args),
        Command::SoundAdd(args) => sbk_cmd::add(&args),
        Command::VoiceAdd(args) => voice_cmd::add(&args),
        Command::SpeechBank(args) => voice_cmd::bank(&args),
        Command::SpeechEvents(args) => voice_cmd::events(&args),
        Command::AudioConfig(args) => voice_cmd::config(&args),
        Command::Mix(args) => voice_cmd::mix(&args),
        Command::Tunables(args) => tunables_cmd::run(&args),
        Command::GraphInfo(args) => graph_cmd::info(&args),
        Command::GraphExport(args) => graph_cmd::export(&args),
        Command::GraphImport(args) => graph_cmd::import(&args),
        Command::GraphEdit(args) => graph_cmd::edit(&args),
        Command::Dialogues(args) => dialogue_cmd::list(args),
        Command::Dialogue(args) => dialogue_cmd::show(args),
        Command::DialogueExport(args) => dialogue_cmd::export(args),
        Command::DialogueImport(args) => dialogue_cmd::import(args),
        Command::Subtitle(args) => dialogue_cmd::subtitle(args),
        Command::Nav(command) => nav_cmd::run(command),
        Command::Project(command) => project_cmd::run(command),
        Command::Level(command) => level_cmd::run(command),
        Command::Partsdb(command) => partsdb_cmd::run(command),
        Command::Anim(command) => anim_cmd::run(command),
        Command::Cutscene(command) => cutscene_cmd::run(command),
        Command::LogicGraph(args) => logic_graph_cmd::graph(&args),
        Command::Messages(args) => logic_graph_cmd::messages(&args),
        Command::LogicMissions(args) => logic_graph_cmd::missions(&args),
        Command::LogicRun(args) => logic_run_cmd::run(&args),
        Command::LogicConnect(args) => logic_graph_cmd::connect(&args),
        Command::LogicSchema { json } => logic_graph_cmd::schema(json),
        Command::Mcp(args) => mcp_cmd::run(args),
        Command::KbGen(args) => kb_cmd::generate(args),
        Command::KbLint(args) => kb_cmd::check(args),
        Command::Save(args) => save_cmd::run(args),
        Command::GameTypes(command) => types_cmd::run(command),
        Command::TypesGen(args) => types_cmd::generate(args),
        Command::ParityGen(args) => kb_cmd::parity(args),
        Command::LogicCheck(args) => types_cmd::logic_check(&args),
    }
}

/// Writes a line to stdout and exits quietly when the pipe is closed (`| head`).
macro_rules! out {
    ($($arg:tt)*) => {
        if writeln!(std::io::stdout(), $($arg)*).is_err() {
            std::process::exit(0);
        }
    };
}

fn parse_type(filter: Option<&str>) -> Result<Option<u32>> {
    match filter {
        None => Ok(None),
        Some(text) => type_tags()
            .resolve(text)
            .map(Some)
            .with_context(|| format!("`{text}` is neither a known type tag nor a hexadecimal id")),
    }
}

fn open_stream(path: &Path) -> Result<StreamFile> {
    StreamFile::open(path).with_context(|| format!("cannot open stream {}", path.display()))
}

fn is_toc(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("toc"))
}

fn info(path: &Path) -> Result<()> {
    if is_toc(path) {
        let table = load_toc(path)?;
        out!("StreamTOC version {}", table.version);
        out!("files:            {}", table.files.len());
        out!("names:            {}", table.names.len());
        out!("dependency words: {}", table.dep_words.len());
        return Ok(());
    }
    let stream = open_stream(path)?;
    let layout = stream.layout();
    let count = |kind| layout.blocks.iter().filter(|b| b.kind == kind).count();
    let stored: usize = (0..layout.resources.len())
        .map(|i| layout.stored_size(i))
        .sum();
    out!(
        "UStream version {}, {:?}-endian",
        layout.header.version,
        layout.endian
    );
    out!("file size:        {}", layout.file_len);
    out!("pages:            {}", layout.file_len / ustream::PAGE_SIZE);
    out!("resources:        {}", layout.resources.len());
    out!("data size:        {}", layout.total_data_size());
    out!("stored size:      {stored}");
    out!(
        "blocks:           {} raw, {} RefPack, {} LZX, {} fill",
        count(BlockKind::Raw),
        count(BlockKind::RefPack),
        count(BlockKind::Lzx),
        count(BlockKind::Fill)
    );
    Ok(())
}

fn list(path: &Path, type_filter: Option<&str>) -> Result<()> {
    let wanted = parse_type(type_filter)?;
    let stream = open_stream(path)?;
    let layout = stream.layout();
    out!(
        "{:>6}  {:<24} {:<8} {:>5}  {:<35} {:>10} {:>10}",
        "index",
        "type",
        "policy",
        "align",
        "ids",
        "size",
        "stored"
    );
    for (index, resource) in layout.resources.iter().enumerate() {
        let h = &resource.header;
        if wanted.is_some_and(|t| t != h.type_id) {
            continue;
        }
        let ids = if h.ids.iter().all(|&id| id == h.ids[0]) {
            format!("{:08x}", h.ids[0])
        } else {
            h.ids.map(|id| format!("{id:08x}")).join(" ")
        };
        out!(
            "{index:>6}  {:<24} {:<8} {:>5x}  {ids:<35} {:>10} {:>10}{}",
            type_tags().display(h.type_id),
            mem_policies().display(h.mem_policy),
            h.alignment,
            h.data_size,
            layout.stored_size(index),
            if layout.is_compressed(index) {
                "  refpack"
            } else {
                ""
            }
        );
    }
    Ok(())
}

fn extract(path: &Path, output: &Path, indices: &[usize], type_filter: Option<&str>) -> Result<()> {
    let wanted = parse_type(type_filter)?;
    let stream = open_stream(path)?;
    let layout = stream.layout();
    if let Some(&bad) = indices.iter().find(|&&i| i >= layout.resources.len()) {
        bail!(
            "resource index {bad} out of range, the stream has {}",
            layout.resources.len()
        );
    }
    fs::create_dir_all(output).with_context(|| format!("cannot create {}", output.display()))?;

    let selected: Vec<usize> = (0..layout.resources.len())
        .filter(|i| indices.is_empty() || indices.contains(i))
        .filter(|&i| wanted.is_none_or(|t| t == layout.resources[i].header.type_id))
        .collect();
    let bytes = AtomicU64::new(0);
    selected.par_iter().try_for_each(|&index| -> Result<()> {
        let header = &layout.resources[index].header;
        let data = stream
            .resource_data(index)
            .with_context(|| format!("cannot read resource {index}"))?;
        // The last id distinguishes the resources of a texture pair.
        let name = format!(
            "{index:05}_{}_{:08x}.bin",
            type_tags().display(header.type_id),
            header.ids[3]
        );
        let target = output.join(name);
        fs::write(&target, &data).with_context(|| format!("cannot write {}", target.display()))?;
        bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
        Ok(())
    })?;
    out!(
        "extracted {} resource(s), {} byte(s)",
        selected.len(),
        bytes.into_inner()
    );
    Ok(())
}

fn load_toc(path: &Path) -> Result<Toc> {
    let data = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    Toc::parse(&data).with_context(|| format!("cannot parse {}", path.display()))
}

fn print_toc(path: &Path) -> Result<()> {
    let table = load_toc(path)?;
    out!(
        "{:>5}  {:<8} {:>12}  {:<11} name -> dependencies",
        "index",
        "hash",
        "data size",
        "flags"
    );
    for (index, file) in table.files.iter().enumerate() {
        let name = table.file_name(index);
        let deps: Vec<String> = table.file_deps(index).map(|n| n.name_lossy()).collect();
        out!(
            "{index:>5}  {:08x} {:>12}  {:<11} {}{}",
            name.hash,
            file.data_size,
            file.flags.map(|f| f.to_string()).join(","),
            name.name_lossy(),
            if deps.is_empty() {
                String::new()
            } else {
                format!(" -> {}", deps.join(", "))
            }
        );
    }
    let referenced: Vec<usize> = table.files.iter().map(|f| f.name).collect();
    for (index, name) in table.names.iter().enumerate() {
        if !referenced.contains(&index) {
            out!(
                "       {:08x} {:>12}  {:<11} {} (external)",
                name.hash,
                "-",
                "-",
                name.name_lossy()
            );
        }
    }
    Ok(())
}

fn validate(game_dir: &Path, decompress: bool, roundtrip: bool) -> Result<()> {
    let game = GameInstance::open(game_dir)?;
    out!(
        "{}: {} stream(s), {} toc(s)",
        game.profile().name,
        game.streams().len(),
        game.tocs().len()
    );
    for file in game.skipped() {
        out!(
            "skipped {}: named like a game file but not one (no magic)",
            file.rel_path
        );
    }
    if let Some(warning) = doctor_toolkit::path_limit::warning(&game) {
        eprintln!("WARN {warning}");
    }

    let failures = AtomicUsize::new(0);
    let resources = AtomicU64::new(0);
    let bytes = AtomicU64::new(0);
    let warnings = AtomicUsize::new(0);
    let fail = |message: String| {
        failures.fetch_add(1, Ordering::Relaxed);
        eprintln!("FAIL {message}");
    };

    game.streams().par_iter().for_each(|file| {
        let stream = match StreamFile::open(&file.path) {
            Ok(stream) => stream,
            Err(error) => return fail(format!("{}: {error}", file.rel_path)),
        };
        let layout = stream.layout();
        resources.fetch_add(layout.resources.len() as u64, Ordering::Relaxed);
        if let Err(message) = check_structure(stream.data(), layout) {
            fail(format!("{}: {message}", file.rel_path));
        }
        if decompress {
            for index in 0..layout.resources.len() {
                match layout.resource_data_with_excess(stream.data(), index) {
                    Ok((data, excess)) => {
                        bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
                        if excess > 0 {
                            warnings.fetch_add(1, Ordering::Relaxed);
                            eprintln!(
                                "WARN {} resource {index}: {excess} byte(s) beyond the declared size",
                                file.rel_path
                            );
                        }
                    }
                    Err(error) => fail(format!("{} resource {index}: {error}", file.rel_path)),
                }
            }
        }
        if roundtrip {
            match ustream::rebuild(stream.data(), layout, &[]) {
                Ok(rebuilt) if rebuilt == stream.data() => {}
                Ok(_) => fail(format!("{}: rewrite differs from the original", file.rel_path)),
                Err(error) => fail(format!("{}: rewrite failed: {error}", file.rel_path)),
            }
        }
    });

    let mut stale = 0usize;
    let mut missing = 0usize;
    for file in game.tocs() {
        let data = match fs::read(&file.path) {
            Ok(data) => data,
            Err(error) => {
                fail(format!("{}: {error}", file.rel_path));
                continue;
            }
        };
        let table = match Toc::parse(&data) {
            Ok(table) => table,
            Err(error) => {
                fail(format!("{}: {error}", file.rel_path));
                continue;
            }
        };
        if roundtrip {
            match table.to_bytes() {
                Ok(bytes) if bytes == data => {}
                Ok(_) => fail(format!(
                    "{}: rewrite differs from the original",
                    file.rel_path
                )),
                Err(error) => fail(format!("{}: rewrite failed: {error}", file.rel_path)),
            }
        }
        for index in 0..table.files.len() {
            let name = table.file_name(index).name_lossy();
            let Some(stream_index) = game.find_stream(&name) else {
                missing += 1;
                eprintln!("WARN {}: stream `{name}` not found on disk", file.rel_path);
                continue;
            };
            let Ok(stream) = game.open_stream(stream_index) else {
                continue;
            };
            let actual = stream.layout().total_data_size();
            if actual != table.files[index].data_size as u64 {
                stale += 1;
                eprintln!(
                    "WARN {}: `{name}` lists {} data byte(s), the stream holds {actual}",
                    file.rel_path, table.files[index].data_size
                );
            }
        }
    }

    out!("resources: {}", resources.into_inner());
    if decompress {
        out!("decompressed: {} byte(s)", bytes.into_inner());
    }
    out!("resources with surplus data:   {}", warnings.into_inner());
    out!("toc entries with a stale size: {stale}");
    out!("toc entries without a stream:  {missing}");
    let failures = failures.into_inner();
    if failures > 0 {
        bail!("{failures} failure(s)");
    }
    out!("OK");
    Ok(())
}

/// Checks the structural rules every game file obeys.
fn check_structure(data: &[u8], layout: &StreamLayout) -> std::result::Result<(), String> {
    if !data.len().is_multiple_of(ustream::PAGE_SIZE) {
        return Err("file size is not a multiple of the page size".into());
    }
    for block in &layout.blocks {
        let end = block.offset + block.size;
        if !block.size.is_multiple_of(4) {
            return Err(format!(
                "block at {:#x} has an unaligned size",
                block.offset
            ));
        }
        if block.offset / ustream::PAGE_SIZE != (end - 1) / ustream::PAGE_SIZE {
            return Err(format!(
                "block at {:#x} crosses a page boundary",
                block.offset
            ));
        }
        if block.kind == BlockKind::Fill && !end.is_multiple_of(ustream::PAGE_SIZE) {
            return Err(format!(
                "FILL block at {:#x} does not end its page",
                block.offset
            ));
        }
    }
    Ok(())
}

fn types(game_dir: &Path, markdown: bool) -> Result<()> {
    let game = GameInstance::open(game_dir)?;
    let (index, errors) = game.build_index(|_| {});
    for (stream, error) in &errors {
        eprintln!("FAIL {}: {error}", game.streams()[*stream].rel_path);
    }
    let summary = index.type_summary();
    if markdown {
        out!("| Type id | Tag | Resources | Data bytes | Stored bytes |");
        out!("|---|---|---:|---:|---:|");
    } else {
        out!(
            "{:<8}  {:<24} {:>9} {:>14} {:>14}",
            "type id",
            "tag",
            "resources",
            "data bytes",
            "stored bytes"
        );
    }
    for t in &summary {
        let tag = type_tags().get(t.type_id).unwrap_or("?");
        if markdown {
            out!(
                "| `{:08x}` | {} | {} | {} | {} |",
                t.type_id,
                if tag == "?" {
                    "?".to_owned()
                } else {
                    format!("`{tag}`")
                },
                t.count,
                t.data_bytes,
                t.stored_bytes
            );
        } else {
            out!(
                "{:08x}  {tag:<24} {:>9} {:>14} {:>14}",
                t.type_id,
                t.count,
                t.data_bytes,
                t.stored_bytes
            );
        }
    }
    if !markdown {
        out!(
            "{} type(s), {} resource(s)",
            summary.len(),
            index.entries.len()
        );
    }
    if !errors.is_empty() {
        bail!("{} stream(s) could not be read", errors.len());
    }
    Ok(())
}

fn repack(input: &Path, output: &Path, relayout: bool) -> Result<()> {
    let stream = open_stream(input)?;
    let bytes = if relayout {
        ustream::relayout(stream.data(), stream.layout())?
    } else {
        ustream::rebuild(stream.data(), stream.layout(), &[])?
    };
    let identical = bytes == stream.data();
    fs::write(output, &bytes).with_context(|| format!("cannot write {}", output.display()))?;
    out!(
        "wrote {} byte(s), {}",
        bytes.len(),
        if identical {
            "identical to the input"
        } else {
            "differs from the input"
        }
    );
    Ok(())
}

fn replace(
    game_dir: &Path,
    stream: &str,
    index: usize,
    data_path: &Path,
    storage: StorageChoice,
    mode: &EditMode,
) -> Result<()> {
    let game = GameInstance::open(game_dir)?;
    let stream_index = game
        .find_stream(stream)
        .with_context(|| format!("stream `{stream}` not found in {}", game_dir.display()))?;
    let data =
        fs::read(data_path).with_context(|| format!("cannot read {}", data_path.display()))?;
    let mut ws = workspace_cmd::open_for_edit(&game, mode)?;
    let name = game.streams()[stream_index].stream_name();
    let label = format!(
        "Replace resource {index} of {name} with {}",
        data_path.display()
    );
    let edit = ResourceEdit {
        index,
        data,
        storage,
    };
    if ws
        .set_resources(&game, &label, stream_index, &[edit])?
        .is_none()
    {
        out!("no change: the workspace already has this data");
    }
    workspace_cmd::finish_edit(&game, &mut ws, mode, vec![name])
}

/// The old `replace --no-toc`: writes into the game at once, bypassing the
/// workspace, and leaves the tables of contents alone.
fn replace_without_workspace(
    game_dir: &Path,
    stream: &str,
    index: usize,
    data_path: &Path,
    compress: bool,
) -> Result<()> {
    let game = GameInstance::open(game_dir)?;
    let stream_index = game
        .find_stream(stream)
        .with_context(|| format!("stream `{stream}` not found in {}", game_dir.display()))?;
    let data =
        fs::read(data_path).with_context(|| format!("cannot read {}", data_path.display()))?;
    let storage = if compress {
        Storage::RefPack
    } else {
        Storage::Raw
    };
    let report = game.replace_resources(
        stream_index,
        &[Replacement {
            index,
            data: &data,
            storage,
        }],
        ReplaceOptions { update_toc: false },
    )?;
    out!(
        "stream size: {} -> {} byte(s)",
        report.old_len,
        report.new_len
    );
    out!(
        "data size:   {} -> {} byte(s)",
        report.old_data_size,
        report.new_data_size
    );
    for path in &report.backed_up {
        out!("backed up {path}");
    }
    for path in &report.written {
        out!("wrote     {path}");
    }
    Ok(())
}

fn rewrite(game_dir: &Path, stream: &str, compress: bool) -> Result<()> {
    let game = GameInstance::open(game_dir)?;
    let stream_index = game
        .find_stream(stream)
        .with_context(|| format!("stream `{stream}` not found in {}", game_dir.display()))?;
    let file = game.open_stream(stream_index)?;
    let data: Vec<Vec<u8>> = (0..file.layout().resources.len())
        .into_par_iter()
        .map(|i| file.resource_data(i))
        .collect::<Result<_, _>>()?;
    drop(file);
    let storage = if compress {
        Storage::RefPack
    } else {
        Storage::Raw
    };
    let replacements: Vec<Replacement> = data
        .iter()
        .enumerate()
        .map(|(index, data)| Replacement {
            index,
            data,
            storage,
        })
        .collect();
    let report = game.replace_resources(stream_index, &replacements, ReplaceOptions::default())?;
    out!("rewrote {} resource(s)", replacements.len());
    out!(
        "stream size: {} -> {} byte(s)",
        report.old_len,
        report.new_len
    );
    for path in &report.backed_up {
        out!("backed up {path}");
    }
    for path in &report.written {
        out!("wrote     {path}");
    }
    Ok(())
}

fn export_textures(
    source: &Path,
    output: &Path,
    png: bool,
    name_filter: Option<&str>,
) -> Result<()> {
    use doctor_toolkit::resource::texture::{self, Texture};
    use doctor_toolkit::resource::tg4::Kind;

    let streams: Vec<(String, PathBuf)> = if source.is_dir() {
        let game = GameInstance::open(source)?;
        game.streams()
            .iter()
            .map(|f| (f.rel_path.clone(), f.path.clone()))
            .collect()
    } else {
        let name = source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        vec![(name, source.to_path_buf())]
    };
    let filter = name_filter.map(str::to_ascii_lowercase);
    let written = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    streams
        .par_iter()
        .try_for_each(|(rel, path)| -> Result<()> {
            let stream = open_stream(path)?;
            let dir = output.join(rel.trim_end_matches(".str").trim_end_matches(".STR"));
            let mut used = std::collections::HashSet::new();
            for pair in texture::pairs(stream.layout()) {
                let tex = match Texture::load(stream.data(), stream.layout(), pair) {
                    Ok(tex) => tex,
                    Err(error) => {
                        eprintln!("FAIL {rel} #{}: {error}", pair.header);
                        failed.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                };
                let name = &tex.header.name;
                if filter
                    .as_ref()
                    .is_some_and(|f| !name.to_ascii_lowercase().contains(f))
                {
                    continue;
                }
                // Names repeat inside a stream now and then; the hash keeps files apart.
                let base = sanitize(name);
                let base = if used.insert(base.clone()) {
                    base
                } else {
                    format!("{base}_{:08x}", tex.header.name_hash)
                };
                fs::create_dir_all(&dir)
                    .with_context(|| format!("cannot create {}", dir.display()))?;
                let result = if png {
                    let layers = if tex.header.kind() == Kind::Flat {
                        1
                    } else {
                        tex.header.layer_count()
                    };
                    (0..layers).try_for_each(|layer| {
                        let file = if layers == 1 {
                            format!("{base}.png")
                        } else {
                            format!("{base}_{layer}.png")
                        };
                        let bytes = tex.to_png(layer)?;
                        fs::write(dir.join(file), bytes)
                            .map_err(|e| doctor_toolkit::Error::io(&dir, e))
                    })
                } else {
                    tex.to_dds().and_then(|bytes| {
                        fs::write(dir.join(format!("{base}.dds")), bytes)
                            .map_err(|e| doctor_toolkit::Error::io(&dir, e))
                    })
                };
                match result {
                    Ok(()) => {
                        written.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(error) => {
                        eprintln!("FAIL {rel} {name}: {error}");
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Ok(())
        })?;
    out!("exported {} texture(s)", written.into_inner());
    let failed = failed.into_inner();
    if failed > 0 {
        bail!("{failed} texture(s) failed");
    }
    Ok(())
}

/// Replaces characters that are not safe in file names.
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unnamed".to_owned()
    } else {
        cleaned
    }
}

const LH2: u32 = doctor_toolkit::hash::sdbm(b"lh2");
const APX: u32 = doctor_toolkit::hash::sdbm(b"apx");

fn export_models(
    game_dir: &Path,
    stream_name: &str,
    output: &Path,
    format: doctor_toolkit::scene::ModelFormat,
    name_filter: Option<&str>,
    no_textures: bool,
) -> Result<()> {
    use doctor_toolkit::export::gltf::{AssetSource, NoAssets};
    use doctor_toolkit::game::GameAssets;
    use doctor_toolkit::resource::mesh::{self, TYPE_MESH};
    use doctor_toolkit::resource::treemodel::{TYPE_TREE, TreeModel};
    use doctor_toolkit::scene;

    let game = std::sync::Arc::new(GameInstance::open(game_dir)?);
    let stream_index = game
        .find_stream(stream_name)
        .with_context(|| format!("stream `{stream_name}` not found"))?;
    let stream = game.open_stream(stream_index)?;
    let index = if no_textures {
        None
    } else {
        Some(game.build_index(|_| {}).0)
    };
    let mut assets: Box<dyn AssetSource> = match &index {
        Some(index) => Box::new(GameAssets::new(game.clone(), index)),
        None => Box::new(NoAssets),
    };
    let filter = name_filter.map(str::to_ascii_lowercase);
    fs::create_dir_all(output)?;
    // Models whose name repeats in the stream (`model`, `lodmodel`) get
    // their id in the file name, as `textures` names repeated textures:
    // one file each instead of the last one over the others.
    let mut names: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, resource) in stream.layout().resources.iter().enumerate() {
        if resource.header.type_id == TYPE_MESH
            && let Ok(model) = mesh::load(&stream, i)
        {
            *names
                .entry(sanitize(&model.name).to_ascii_lowercase())
                .or_default() += 1;
        }
    }
    let mut written = 0;
    for (i, resource) in stream.layout().resources.iter().enumerate() {
        if resource.header.type_id == TYPE_TREE {
            // Trees: named after their folder and id (stage 68).
            let tree = TreeModel::parse(&stream.resource_data(i)?)
                .with_context(|| format!("resource {i}"))?;
            let folder = tree
                .source_path()
                .rsplit(['\\', '/'])
                .nth(1)
                .unwrap_or("tree");
            let name = format!("{folder}_{:08x}", resource.header.ids[3]);
            if filter
                .as_ref()
                .is_some_and(|f| !name.to_ascii_lowercase().contains(f))
            {
                continue;
            }
            let built = scene::game::tree_scene(&tree, &name, assets.as_mut())
                .with_context(|| name.clone())?;
            let path = output.join(format!("{}.{}", sanitize(&name), format.extension()));
            scene::write_file(&built, &path, Some(format)).with_context(|| name.clone())?;
            out!("{} ({} part(s))", path.display(), tree.parts.len());
            written += 1;
            continue;
        }
        if resource.header.type_id != TYPE_MESH {
            continue;
        }
        let model = mesh::load(&stream, i).with_context(|| format!("resource {i}"))?;
        if filter
            .as_ref()
            .is_some_and(|f| !model.name.to_ascii_lowercase().contains(f))
        {
            continue;
        }
        let built = scene::game::model_scene(&model, assets.as_mut())
            .with_context(|| model.name.clone())?;
        if !format.has_skeletons() && !built.joints.is_empty() && written == 0 {
            out!(
                "note: OBJ keeps no skeleton or bone weights; skinned models are written in bind pose"
            );
        }
        let mut file_name = sanitize(&model.name);
        if names
            .get(&file_name.to_ascii_lowercase())
            .copied()
            .unwrap_or(0)
            > 1
        {
            file_name = format!("{file_name}_{:08x}", resource.header.ids[3]);
        }
        let path = output.join(format!("{file_name}.{}", format.extension()));
        scene::write_file(&built, &path, Some(format)).with_context(|| model.name.clone())?;
        out!("{} ({} part(s))", path.display(), model.records.len());
        written += 1;
    }
    out!("exported {written} model(s)");
    Ok(())
}

/// Whether QUERY names a texture called NAME with name hash HASH: the name
/// itself, or `NAME_HASH` (eight hex digits), the file name `textures` gives
/// a texture whose name repeats in the stream, e.g. a clone.
fn texture_name_matches(name: &str, hash: u32, query: &str) -> bool {
    if name.eq_ignore_ascii_case(query) {
        return true;
    }
    query.rsplit_once('_').is_some_and(|(base, h)| {
        h.len() == 8
            && u32::from_str_radix(h, 16) == Ok(hash)
            && sanitize(name).eq_ignore_ascii_case(base)
    })
}

fn texture_import(
    game_dir: &Path,
    stream_name: &str,
    name: &str,
    file: &Path,
    all_copies: bool,
    mode: &EditMode,
) -> Result<()> {
    use doctor_toolkit::resource::texture::{self, Texture};

    let game = GameInstance::open(game_dir)?;
    let stream_index = game
        .find_stream(stream_name)
        .with_context(|| format!("stream `{stream_name}` not found"))?;
    let stream = game.open_stream(stream_index)?;
    let mut found = None;
    for pair in texture::pairs(stream.layout()) {
        let tex = Texture::load(stream.data(), stream.layout(), pair)?;
        if texture_name_matches(&tex.header.name, tex.header.name_hash, name) {
            if found.is_some() {
                bail!(
                    "several textures named `{name}` in {stream_name}; \
                     name one as NAME_HASH, as in the file names of `textures`"
                );
            }
            found = Some((pair, tex.header));
        }
    }
    drop(stream);
    let (pair, before) = found.with_context(|| format!("no texture `{name}` in {stream_name}"))?;
    let input = fs::read(file).with_context(|| format!("cannot read {}", file.display()))?;
    let is_png = file
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    let mut ws = workspace_cmd::open_for_edit(&game, mode)?;
    if all_copies {
        let (index, _) = game.build_index(|_| {});
        let done = doctor_toolkit::retexture::replace_all_copies(
            &game,
            &mut ws,
            &index,
            before.name_hash,
            &input,
            is_png,
        )?;
        out!("{}: {} -> {}", done.name, done.before, done.after);
        for stream in &done.streams {
            out!("  {stream}");
        }
        out!("{} cop(ies) replaced", done.streams.len());
        return workspace_cmd::finish_edit(&game, &mut ws, mode, done.streams);
    }
    let after = ws.import_texture(&game, stream_index, pair, &input, is_png)?;
    let h = &after.header;
    out!(
        "{}: {}×{} {} -> {}×{} {}, {} mip level(s)",
        h.name,
        before.width,
        before.height,
        before.format().name(),
        h.width,
        h.height,
        h.format().name(),
        h.mip_count()
    );
    let stream = game.streams()[stream_index].stream_name();
    workspace_cmd::finish_edit(&game, &mut ws, mode, vec![stream])
}

fn apx_export(game_dir: &Path, output: &Path, png: bool) -> Result<()> {
    use doctor_toolkit::resource::apx::{AptFile, TAG_APT, TAG_CONSTANTS};

    let game = GameInstance::open(game_dir)?;
    let (mut files, mut textures) = (0, 0);
    for file in game.streams() {
        let stream = open_stream(&file.path)?;
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != APX {
                continue;
            }
            let at = || format!("{} resource {index}", file.rel_path);
            let apx = AptFile::parse(&stream.resource_data(index)?).with_context(at)?;
            let title = sanitize(&apx.title().unwrap_or_else(|| format!("{index:05}")));
            let dir = output.join(file.rel_path.trim_end_matches(".str"));
            fs::create_dir_all(&dir)?;
            if let Some(block) = apx.block(TAG_APT) {
                fs::write(dir.join(format!("{title}.apt")), &block.data)?;
            }
            if let Some(block) = apx.block(TAG_CONSTANTS) {
                fs::write(dir.join(format!("{title}.const")), &block.data)?;
            }
            let embedded = apx.textures().with_context(at)?;
            if !embedded.is_empty() {
                let tex_dir = dir.join(format!("{title}_textures"));
                fs::create_dir_all(&tex_dir)?;
                for (i, texture) in embedded.iter().enumerate() {
                    let base = format!("{i:03}_{}", sanitize(&texture.header.name));
                    let (name, bytes) = if png {
                        (format!("{base}.png"), texture.to_png(0)?)
                    } else {
                        (format!("{base}.dds"), texture.to_dds()?)
                    };
                    fs::write(tex_dir.join(name), bytes)?;
                    textures += 1;
                }
            }
            files += 1;
        }
    }
    out!("exported {files} apx file(s) with {textures} embedded texture(s)");
    Ok(())
}

fn text_export(game_dir: &Path, output: &Path) -> Result<()> {
    use doctor_toolkit::resource::lh2::StringTable;

    let game = GameInstance::open(game_dir)?;
    let mut written = 0;
    for file in game.streams() {
        let stream = open_stream(&file.path)?;
        for (index, resource) in stream.layout().resources.iter().enumerate() {
            if resource.header.type_id != LH2 {
                continue;
            }
            let table = StringTable::parse(&stream.resource_data(index)?)
                .with_context(|| format!("{} resource {index}", file.rel_path))?;
            let document = doctor_toolkit::text::export_json(
                &table,
                &file.rel_path,
                index,
                resource.header.ids[3],
            );
            let dir = output.join(file.rel_path.trim_end_matches(".str"));
            fs::create_dir_all(&dir)?;
            let path = dir.join(format!(
                "{index:05}_lh2_{:08x}.json",
                resource.header.ids[3]
            ));
            fs::write(&path, serde_json::to_string_pretty(&document)? + "\n")?;
            written += 1;
        }
    }
    out!("exported {written} string table(s)");
    Ok(())
}

fn text_import(game_dir: &Path, files: &[PathBuf], mode: &EditMode) -> Result<()> {
    use doctor_toolkit::resource::lh2::StringTable;
    use serde_json::Value;

    let game = GameInstance::open(game_dir)?;
    let mut ws = workspace_cmd::open_for_edit(&game, mode)?;
    let mut touched = Vec::new();
    for path in files {
        let text =
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
        let document: Value = serde_json::from_str(&text)
            .with_context(|| format!("{} is not valid JSON", path.display()))?;
        let field = |name: &str| {
            document
                .get(name)
                .with_context(|| format!("{}: missing `{name}`", path.display()))
        };
        let stream_name = field("stream")?
            .as_str()
            .context("`stream` must be a string")?;
        let index = field("resource")?
            .as_u64()
            .context("`resource` must be a number")? as usize;
        let id = field("id")?.as_str().context("`id` must be a string")?;

        let stream_index = game
            .find_stream(stream_name)
            .with_context(|| format!("stream `{stream_name}` not found"))?;
        let stream = game.open_stream(stream_index)?;
        let resource = stream
            .layout()
            .resources
            .get(index)
            .with_context(|| format!("no resource {index} in {stream_name}"))?;
        if resource.header.type_id != LH2 || format!("{:08x}", resource.header.ids[3]) != id {
            bail!(
                "{}: resource {index} of {stream_name} is not string table {id}",
                path.display()
            );
        }
        // Edits build on the workspace's version of the table.
        let key = doctor_toolkit::workspace::ResourceKey::new(stream_name, &resource.header);
        let data = match ws.data(&key)? {
            Some(data) => data,
            None => stream.resource_data(index)?,
        };
        let mut table = StringTable::parse(&data)?;
        drop(stream);

        let changed = doctor_toolkit::text::import_json(&mut table, &document)
            .with_context(|| path.display().to_string())?;
        if changed == 0 {
            out!("{}: no changes", path.display());
            continue;
        }
        ws.set_string_table(&game, stream_index, index, &table)?;
        out!("{}: {changed} string(s) changed", path.display());
        touched.push(game.streams()[stream_index].stream_name());
    }
    if touched.is_empty() {
        return Ok(());
    }
    workspace_cmd::finish_edit(&game, &mut ws, mode, touched)
}

fn dump(game_dir: &Path, type_filter: &str, output: &Path) -> Result<()> {
    let wanted = parse_type(Some(type_filter))?.expect("filter given");
    let game = GameInstance::open(game_dir)?;
    let count = AtomicUsize::new(0);
    let bytes = AtomicU64::new(0);
    game.streams()
        .par_iter()
        .try_for_each(|file| -> Result<()> {
            let stream = open_stream(&file.path)?;
            let layout = stream.layout();
            let dir = output.join(
                file.rel_path
                    .trim_end_matches(".str")
                    .trim_end_matches(".STR"),
            );
            let mut created = false;
            for (index, resource) in layout.resources.iter().enumerate() {
                let header = &resource.header;
                if header.type_id != wanted {
                    continue;
                }
                if !created {
                    fs::create_dir_all(&dir)
                        .with_context(|| format!("cannot create {}", dir.display()))?;
                    created = true;
                }
                let data = stream
                    .resource_data(index)
                    .with_context(|| format!("{} resource {index}", file.rel_path))?;
                let name = format!(
                    "{index:05}_{}_{:08x}.bin",
                    type_tags().display(header.type_id),
                    header.ids[3]
                );
                fs::write(dir.join(name), &data)?;
                count.fetch_add(1, Ordering::Relaxed);
                bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
            }
            Ok(())
        })?;
    out!(
        "extracted {} resource(s), {} byte(s)",
        count.into_inner(),
        bytes.into_inner()
    );
    Ok(())
}

fn logic_export(
    game_dir: &Path,
    output: &Path,
    type_filter: Option<&str>,
    stream_filter: Option<&str>,
) -> Result<()> {
    use doctor_toolkit::resource::logic;

    let wanted = parse_type(type_filter)?;
    if let Some(id) = wanted
        && !logic::is_supported(id)
    {
        bail!(
            "no JSON export for type {}; supported: {}",
            type_tags().display(id),
            logic::TAGS.join(", ")
        );
    }
    let game = GameInstance::open(game_dir)?;
    let ws_dir = doctor_toolkit::workspace::Workspace::default_dir(game.root());
    let ws = doctor_toolkit::workspace::Workspace::exists(&ws_dir)
        .then(|| doctor_toolkit::workspace::Workspace::open(&ws_dir))
        .transpose()?;
    let count = AtomicUsize::new(0);
    game.streams()
        .par_iter()
        .filter(|file| stream_filter.is_none_or(|f| file.rel_path.contains(f)))
        .try_for_each(|file| -> Result<()> {
            let stream = open_stream(&file.path)?;
            let dir = output.join(
                file.rel_path
                    .trim_end_matches(".str")
                    .trim_end_matches(".STR"),
            );
            for (index, resource) in stream.layout().resources.iter().enumerate() {
                let header = &resource.header;
                if wanted.is_some_and(|id| id != header.type_id)
                    || !logic::is_supported(header.type_id)
                {
                    continue;
                }
                let at = || format!("{} resource {index}", file.rel_path);
                let data = logic_cmd::export_data(ws.as_ref(), &file.stream_name(), &stream, index)
                    .with_context(at)?;
                let mut json = logic::to_json(header.type_id, &data)
                    .expect("supported type")
                    .with_context(at)?;
                // Where the resource is, for `logic-import`.
                if let serde_json::Value::Object(map) = &mut json {
                    let fields = [
                        ("stream", file.stream_name().into()),
                        ("resource", index.into()),
                        (
                            "type",
                            type_tags().display(header.type_id).to_string().into(),
                        ),
                        ("id", format!("{:08x}", header.ids[3]).into()),
                    ];
                    for (i, (key, value)) in fields.into_iter().enumerate() {
                        map.shift_insert(i, key.to_owned(), value);
                    }
                }
                fs::create_dir_all(&dir)
                    .with_context(|| format!("cannot create {}", dir.display()))?;
                let name = format!(
                    "{index:05}_{}_{:08x}.json",
                    type_tags().display(header.type_id),
                    header.ids[3]
                );
                fs::write(dir.join(name), serde_json::to_string_pretty(&json)? + "\n")?;
                count.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        })?;
    out!("exported {} resource(s)", count.into_inner());
    Ok(())
}

fn restore(game_dir: &Path) -> Result<()> {
    let game = GameInstance::open(game_dir)?;
    let restored = game.restore_backups()?;
    for path in &restored {
        out!("restored {path}");
    }
    out!("{} file(s) restored", restored.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::texture_name_matches;

    /// Every command path of `command`, depth first: `level`, `level add`.
    /// Returns the paths with whether each is a group (has subcommands).
    fn command_paths(command: &clap::Command, prefix: &str, out: &mut Vec<(String, bool)>) {
        for sub in command.get_subcommands() {
            if sub.get_name() == "help" {
                continue;
            }
            let path = if prefix.is_empty() {
                sub.get_name().to_owned()
            } else {
                format!("{prefix} {}", sub.get_name())
            };
            out.push((path.clone(), sub.has_subcommands()));
            command_paths(sub, &path, out);
        }
    }

    /// The table of capabilities (`data/toolkit/capabilities.toml`, see
    /// `docs/parity.md`) has every command and subcommand, and names none
    /// the command line does not have.
    #[test]
    fn every_command_is_in_the_table_of_capabilities() {
        use clap::CommandFactory as _;
        use doctor_toolkit::parity::{self, Kind};

        let mut paths = Vec::new();
        command_paths(&super::Cli::command(), "", &mut paths);
        assert!(paths.len() > 200, "{}", paths.len());
        assert!(paths.iter().any(|(p, group)| p == "live object" && *group));
        assert!(
            paths
                .iter()
                .any(|(p, group)| p == "live object move" && !*group)
        );
        // A group is there for its subcommands: those are what a row names.
        let problems = parity::table().check_names(
            Kind::Cli,
            paths.iter().map(|(p, _)| p.as_str()),
            paths
                .iter()
                .filter(|(_, group)| !group)
                .map(|(p, _)| p.as_str()),
        );
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    #[test]
    fn a_clone_of_a_texture_is_named_with_its_hash() {
        assert!(texture_name_matches(
            "white_c_noalpha",
            0xd702_7d83,
            "white_c_noalpha"
        ));
        assert!(texture_name_matches(
            "White_C_NoAlpha",
            0xd702_7d83,
            "white_c_noalpha"
        ));
        assert!(texture_name_matches(
            "white_c_noalpha",
            0xd702_7d83,
            "white_c_noalpha_d7027d83"
        ));
        assert!(!texture_name_matches(
            "white_c_noalpha",
            0x0f83_3658,
            "white_c_noalpha_d7027d83"
        ));
        assert!(!texture_name_matches(
            "white_c",
            0xd702_7d83,
            "white_c_noalpha_d7027d83"
        ));
        assert!(!texture_name_matches(
            "white_c_noalpha",
            0xd702_7d83,
            "white_c_noalpha_d7027d8"
        ));
    }
}
