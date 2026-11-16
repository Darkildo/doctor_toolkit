//! Turns the typed game data of `data/gf2/` into Rust: value lists
//! (`enums.toml`), meanings of SimGroup attributes (`attr_types.toml`),
//! messages named by the executable (`engine_messages.txt`), facts about
//! single messages (`messages.toml`) and the catalogue of the messages of the
//! game files (`game_messages.tsv`, stored deflated).
//!
//! The generated file is included by `src/game_types/mod.rs`. The data is
//! checked here, so a broken list fails the build instead of a run.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Deserialize)]
struct EnumFile {
    title: String,
    ru: String,
    kind: String,
    source: String,
    source_ru: String,
    conf: String,
    values: Vec<EnumValueFile>,
}

#[derive(Deserialize)]
struct EnumValueFile {
    name: Option<String>,
    value: Option<i64>,
    #[serde(default)]
    note: String,
}

#[derive(Deserialize)]
struct AttrTypesFile {
    packet: BTreeMap<String, AttrPacketFile>,
}

#[derive(Deserialize)]
struct AttrPacketFile {
    hash: String,
    attrs: Vec<AttrTypeFile>,
}

#[derive(Deserialize)]
struct AttrTypeFile {
    i: u16,
    #[serde(rename = "type")]
    ty: String,
    conf: String,
    seen: String,
    #[serde(default)]
    source: String,
}

#[derive(Deserialize)]
struct MessagesFile {
    message: BTreeMap<String, MessageNoteFile>,
}

#[derive(Deserialize)]
struct MessageNoteFile {
    summary: String,
    ru: String,
    data: String,
    #[serde(default)]
    sent_by: Vec<String>,
    #[serde(default)]
    sent_by_ru: Vec<String>,
    conf: String,
    source: String,
}

fn sdbm_lower(name: &str) -> u32 {
    name.bytes().fold(0u32, |h, b| {
        u32::from(b.to_ascii_lowercase()).wrapping_add(h.wrapping_mul(65599))
    })
}

fn read(dir: &Path, name: &str) -> String {
    let path = dir.join(name);
    println!("cargo:rerun-if-changed={}", path.display());
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn list_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
}

/// `rosato_carmine` → `RosatoCarmine`, `Health Pack` → `HealthPack`.
fn variant(name: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if upper {
                out.push(c.to_ascii_uppercase());
            } else {
                out.push(c.to_ascii_lowercase());
            }
            upper = false;
        } else {
            upper = true;
        }
    }
    if out.chars().next().is_none_or(|c| c.is_ascii_digit()) {
        out.insert(0, 'V');
    }
    out
}

fn confidence(conf: &str, what: &str) -> &'static str {
    match conf {
        "exe" => "Confidence::Exe",
        "data" => "Confidence::Data",
        "guess" => "Confidence::Guess",
        _ => panic!("{what}: unknown confidence `{conf}`"),
    }
}

fn enums(text: &str, out: &mut String) -> Vec<String> {
    // The order of the file is kept: it is the order of the documents.
    let order: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix('[')?.strip_suffix(']'))
        .collect();
    let file: HashMap<String, EnumFile> = toml::from_str(text).expect("enums.toml");
    assert_eq!(order.len(), file.len(), "enums.toml: unexpected tables");
    out.push_str("/// The value lists of `data/gf2/enums.toml`, in the order of the file.\n");
    out.push_str("pub static ENUMS: &[EnumDef] = &[\n");
    let mut typed = String::new();
    for key in &order {
        let e = &file[*key];
        let kind = match e.kind.as_str() {
            "hash" => "EnumKind::Hash",
            "name" => "EnumKind::Name",
            "int" => "EnumKind::Int",
            "bits" => "EnumKind::Bits",
            k => panic!("enums.toml [{key}]: unknown kind `{k}`"),
        };
        writeln!(
            out,
            "    EnumDef {{ key: {key:?}, title: {:?}, ru: {:?}, kind: {kind}, source: {:?}, source_ru: {:?}, conf: {}, values: &[",
            e.title,
            e.ru,
            e.source,
            e.source_ru,
            confidence(&e.conf, key)
        )
        .unwrap();
        let mut seen = HashSet::new();
        for v in &e.values {
            assert!(
                v.name.is_some() || v.value.is_some(),
                "enums.toml [{key}]: a value without name and number"
            );
            let stored = match e.kind.as_str() {
                "hash" | "name" => {
                    let name = v
                        .name
                        .as_deref()
                        .unwrap_or_else(|| panic!("enums.toml [{key}]: a value without a name"));
                    name.to_ascii_lowercase()
                }
                _ => v
                    .value
                    .unwrap_or_else(|| panic!("enums.toml [{key}]: a value without a number"))
                    .to_string(),
            };
            assert!(
                seen.insert(stored.clone()),
                "enums.toml [{key}]: `{stored}` twice"
            );
            let hash = match (&v.name, e.kind.as_str()) {
                (Some(n), "hash") => format!("Some({:#010x})", sdbm_lower(n)),
                _ => "None".to_owned(),
            };
            writeln!(
                out,
                "        EnumValue {{ name: {:?}, value: {:?}, hash: {hash}, note: {:?} }},",
                v.name, v.value, v.note
            )
            .unwrap();
        }
        out.push_str("    ] },\n");
        // A Rust type for lists whose every value has a name.
        if e.values.iter().all(|v| v.name.is_some()) {
            let ty = variant(key);
            let names: Vec<&str> = e
                .values
                .iter()
                .map(|v| v.name.as_deref().unwrap())
                .collect();
            let variants: Vec<String> = names.iter().map(|n| variant(n)).collect();
            let unique: HashSet<&String> = variants.iter().collect();
            assert_eq!(
                unique.len(),
                variants.len(),
                "enums.toml [{key}]: two names give one Rust identifier"
            );
            writeln!(
                typed,
                "\n/// {} (`{key}` of `data/gf2/enums.toml`; {}).",
                e.title, e.conf
            )
            .unwrap();
            typed.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]\n");
            writeln!(typed, "pub enum {ty} {{").unwrap();
            for (v, n) in variants.iter().zip(&names) {
                writeln!(typed, "    /// `{n}`\n    {v},").unwrap();
            }
            typed.push_str("}\n");
            writeln!(typed, "\nimpl {ty} {{").unwrap();
            writeln!(typed, "    /// Every value, in the order of the list.").unwrap();
            writeln!(
                typed,
                "    pub const ALL: &'static [Self] = &[{}];",
                variants
                    .iter()
                    .map(|v| format!("Self::{v}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .unwrap();
            writeln!(typed, "    /// Key of the list in [`ENUMS`].").unwrap();
            writeln!(typed, "    pub const KEY: &'static str = {key:?};").unwrap();
            writeln!(typed, "\n    /// The name as the game spells it.").unwrap();
            writeln!(
                typed,
                "    pub const fn name(self) -> &'static str {{\n        match self {{"
            )
            .unwrap();
            for (v, n) in variants.iter().zip(&names) {
                writeln!(typed, "            Self::{v} => {n:?},").unwrap();
            }
            typed.push_str("        }\n    }\n");
            writeln!(
                typed,
                "\n    /// `sdbm(lower(name))`: what a hash attribute stores."
            )
            .unwrap();
            writeln!(
                typed,
                "    pub const fn hash(self) -> u32 {{\n        match self {{"
            )
            .unwrap();
            for (v, n) in variants.iter().zip(&names) {
                writeln!(typed, "            Self::{v} => {:#010x},", sdbm_lower(n)).unwrap();
            }
            typed.push_str("        }\n    }\n");
            if e.values.iter().any(|v| v.value.is_some()) {
                writeln!(
                    typed,
                    "\n    /// The number of the value, when the list gives one."
                )
                .unwrap();
                writeln!(
                    typed,
                    "    pub const fn value(self) -> Option<i64> {{\n        match self {{"
                )
                .unwrap();
                for (v, x) in variants.iter().zip(&e.values) {
                    writeln!(typed, "            Self::{v} => {:?},", x.value).unwrap();
                }
                typed.push_str("        }\n    }\n");
            }
            writeln!(
                typed,
                "\n    /// The value with this name; letter case does not matter."
            )
            .unwrap();
            writeln!(
                typed,
                "    pub fn from_name(name: &str) -> Option<Self> {{\n        Self::ALL.iter().copied().find(|v| v.name().eq_ignore_ascii_case(name))\n    }}"
            )
            .unwrap();
            writeln!(typed, "\n    /// The value whose name hashes to `hash`.").unwrap();
            writeln!(
                typed,
                "    pub fn from_hash(hash: u32) -> Option<Self> {{\n        Self::ALL.iter().copied().find(|v| v.hash() == hash)\n    }}"
            )
            .unwrap();
            typed.push_str("}\n");
            writeln!(
                typed,
                "\nimpl std::fmt::Display for {ty} {{\n    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{\n        f.write_str(self.name())\n    }}\n}}"
            )
            .unwrap();
        }
    }
    out.push_str("];\n");
    out.push_str(&typed);
    order.into_iter().map(str::to_owned).collect()
}

fn attr_types(text: &str, enum_keys: &[String], resource_types: &HashSet<&str>, out: &mut String) {
    let file: AttrTypesFile = toml::from_str(text).expect("attr_types.toml");
    let mut rows: Vec<(u32, u16, String)> = Vec::new();
    for (class, packet) in &file.packet {
        let hash = u32::from_str_radix(&packet.hash, 16)
            .unwrap_or_else(|_| panic!("attr_types.toml [{class}]: bad hash"));
        for a in &packet.attrs {
            let what = format!("attr_types.toml {class}.{}", a.i);
            let ty = match a.ty.split_once(':') {
                Some(("ref", c)) if !c.is_empty() => format!("SemanticType::Ref({c:?})"),
                Some(("res", t)) => {
                    assert!(
                        resource_types.contains(t),
                        "{what}: unknown resource type `{t}`"
                    );
                    format!("SemanticType::Resource({t:?})")
                }
                Some(("enum", e)) => {
                    assert!(
                        enum_keys.iter().any(|k| k == e),
                        "{what}: unknown list `{e}`"
                    );
                    format!("SemanticType::Enum({e:?})")
                }
                None if a.ty == "seconds" => "SemanticType::Seconds".to_owned(),
                None if a.ty == "money" => "SemanticType::Money".to_owned(),
                None if a.ty == "bool" => "SemanticType::Bool".to_owned(),
                _ => panic!("{what}: unknown type `{}`", a.ty),
            };
            let (fit, total) = a
                .seen
                .split_once('/')
                .and_then(|(f, t)| Some((f.parse::<u32>().ok()?, t.parse::<u32>().ok()?)))
                .unwrap_or_else(|| panic!("{what}: bad `seen`"));
            rows.push((
                hash,
                a.i,
                format!(
                    "AttrType {{ ty: {ty}, conf: {}, seen: ({fit}, {total}), source: {:?} }}",
                    confidence(&a.conf, &what),
                    a.source
                ),
            ));
        }
    }
    rows.sort_by_key(|r| (r.0, r.1));
    for pair in rows.windows(2) {
        assert!(
            (pair[0].0, pair[0].1) != (pair[1].0, pair[1].1),
            "attr_types.toml: attribute {:08x}.{} twice",
            pair[0].0,
            pair[0].1
        );
    }
    out.push_str(
        "\n/// `data/gf2/attr_types.toml`: (packet class, attribute index, meaning), sorted.\n",
    );
    out.push_str("pub static ATTR_TYPES: &[(u32, u16, AttrType)] = &[\n");
    for (h, i, t) in rows {
        writeln!(out, "    ({h:#010x}, {i}, {t}),").unwrap();
    }
    out.push_str("];\n");
}

/// Returns the hashes of the names of the catalogue.
fn catalogue(text: &str, out_dir: &Path, out: &mut String) -> HashMap<u32, String> {
    let mut hashes: HashMap<u32, String> = HashMap::new();
    let mut body = String::with_capacity(text.len());
    for line in list_lines_keep(text) {
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(fields.len(), 6, "game_messages.tsv: bad line `{line}`");
        let name = fields[0];
        if let Some(other) = hashes.insert(sdbm_lower(name), name.to_owned()) {
            panic!("game_messages.tsv: `{name}` and `{other}` have one hash");
        }
        body.push_str(line);
        body.push('\n');
    }
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(body.as_bytes()).unwrap();
    let packed = encoder.finish().unwrap();
    std::fs::write(out_dir.join("game_messages.bin"), packed).unwrap();
    writeln!(
        out,
        "\n/// `data/gf2/game_messages.tsv` without comments, deflated.\npub static GAME_MESSAGES_DEFLATED: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/game_messages.bin\"));\n/// Size of the inflated catalogue, bytes.\npub const GAME_MESSAGES_SIZE: usize = {};\n/// Number of messages in the catalogue.\npub const GAME_MESSAGES_COUNT: usize = {};",
        body.len(),
        hashes.len()
    )
    .unwrap();
    hashes
}

/// Lines of a list whose fields may be empty at the ends: only comments and
/// empty lines are dropped, nothing is trimmed.
fn list_lines_keep(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let data = manifest.join("../../data/gf2");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let mut out = String::from("// Generated by build.rs from data/gf2/; do not edit.\n\n");

    let enum_keys = enums(&read(&data, "enums.toml"), &mut out);

    let tags = read(&data, "type_tags.txt");
    let resource_types: HashSet<&str> = list_lines(&tags).collect();
    attr_types(
        &read(&data, "attr_types.toml"),
        &enum_keys,
        &resource_types,
        &mut out,
    );

    // Messages named by the executable.
    let engine_text = read(&data, "engine_messages.txt");
    let mut engine: Vec<(u32, &str)> = list_lines(&engine_text)
        .map(|n| (sdbm_lower(n), n))
        .collect();
    engine.sort();
    for pair in engine.windows(2) {
        assert!(
            pair[0].0 != pair[1].0,
            "engine_messages.txt: `{}` and `{}` have one hash",
            pair[0].1,
            pair[1].1
        );
    }
    out.push_str("\n/// `data/gf2/engine_messages.txt`: (hash, name), sorted by hash.\n");
    out.push_str("pub static ENGINE_MESSAGES: &[(u32, &str)] = &[\n");
    for (h, n) in &engine {
        writeln!(out, "    ({h:#010x}, {n:?}),").unwrap();
    }
    out.push_str("];\n");

    let game = catalogue(&read(&data, "game_messages.tsv"), &out_dir, &mut out);
    // One hash, two different names: the engine could not tell them apart.
    for (h, n) in &engine {
        if let Some(other) = game.get(h) {
            assert!(
                other.eq_ignore_ascii_case(n),
                "`{n}` (engine) and `{other}` (game files) have one hash"
            );
        }
    }

    let notes: MessagesFile = toml::from_str(&read(&data, "messages.toml")).expect("messages.toml");
    out.push_str("\n/// `data/gf2/messages.toml`: facts about single messages, sorted by hash.\n");
    out.push_str("pub static MESSAGE_NOTES: &[MessageNote] = &[\n");
    let mut rows: Vec<(u32, String)> = Vec::new();
    for (name, n) in &notes.message {
        let h = sdbm_lower(name);
        assert_eq!(
            n.sent_by.len(),
            n.sent_by_ru.len(),
            "messages.toml [{name}]: sent_by and sent_by_ru differ in length"
        );
        assert!(
            game.contains_key(&h) || engine.binary_search_by_key(&h, |e| e.0).is_ok(),
            "messages.toml: `{name}` is neither in engine_messages.txt nor in game_messages.tsv"
        );
        let conf = match n.conf.as_str() {
            "exe" => "NoteConfidence::Exe",
            "data" => "NoteConfidence::Data",
            "bench" => "NoteConfidence::Bench",
            "guess" => "NoteConfidence::Guess",
            c => panic!("messages.toml [{name}]: unknown confidence `{c}`"),
        };
        rows.push((
            h,
            format!(
                "MessageNote {{ name: {name:?}, hash: {h:#010x}, summary: {:?}, ru: {:?}, data: {:?}, sent_by: &{:?}, sent_by_ru: &{:?}, conf: {conf}, source: {:?} }}",
                n.summary, n.ru, n.data, n.sent_by, n.sent_by_ru, n.source
            ),
        ));
    }
    rows.sort();
    for (_, r) in rows {
        writeln!(out, "    {r},").unwrap();
    }
    out.push_str("];\n");

    std::fs::write(out_dir.join("gf2_types.rs"), out).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
