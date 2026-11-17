//! The knowledge base `docs/kb/`: short Markdown notes with properties
//! (front matter) and plain relative links, readable in Obsidian, on GitHub
//! and by agents. See `docs/kb/README.md`.
//!
//! * [`Kb`] reads the notes (from a folder, or from texts embedded in a
//!   binary) and parses their properties;
//! * [`lint`] checks them: the schema of the properties, broken links,
//!   orphans, questions without a next step, names of closed files;
//! * [`generate`] writes the notes of classes, resource types and messages
//!   from `data/gf2/*` and keeps the hand-written sections between markers;
//! * [`search`] finds notes by words;
//! * [`symbols`] makes the table of named addresses of the executable
//!   (`data/gf2/exe_symbols.tsv`) from the notes of `exe/`.

pub mod generate;
pub mod lint;
pub mod search;
pub mod symbols;

use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// The only edition of the game the knowledge base describes.
pub const TARGET_EDITION: &str = "1.0.789.0";

/// Files of the repository that are left out of the public export. The
/// knowledge base is public: a note may neither link to them nor name them
/// ([`lint`] checks it). Paths from the repository root; this is the one
/// place that lists them.
pub const CLOSED_PATHS: &[&str] = &["PLAN.md", "docs/analysis/visceral-plugin.md"];

/// The properties every note has, in the order they are written.
pub const PROPERTIES: [&str; 8] = [
    "type",
    "status",
    "confidence",
    "editions",
    "stages",
    "sources",
    "tags",
    "updated",
];

/// What a note is about; it follows from the folder of the note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NoteType {
    /// A map of a section: links with a few words each.
    Map,
    /// A generated list of the notes of one folder.
    Index,
    /// How to use the knowledge base.
    Meta,
    Engine,
    System,
    Class,
    Resource,
    Message,
    /// A value list (families, cities, weapons…): generated.
    Enum,
    Exe,
    Question,
    Experiment,
    /// A research stage: planned, or done.
    Research,
}

impl NoteType {
    pub const ALL: [Self; 13] = [
        Self::Map,
        Self::Index,
        Self::Meta,
        Self::Engine,
        Self::System,
        Self::Class,
        Self::Resource,
        Self::Message,
        Self::Enum,
        Self::Exe,
        Self::Question,
        Self::Experiment,
        Self::Research,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Map => "map",
            Self::Index => "index",
            Self::Meta => "meta",
            Self::Engine => "engine",
            Self::System => "system",
            Self::Class => "class",
            Self::Resource => "resource",
            Self::Message => "message",
            Self::Enum => "enum",
            Self::Exe => "exe",
            Self::Question => "question",
            Self::Experiment => "experiment",
            Self::Research => "research",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name() == text)
    }

    /// The folder below the knowledge base that holds notes of this type;
    /// `None` for the types that do not have a folder of their own.
    pub fn folder(self) -> Option<&'static str> {
        Some(match self {
            Self::Map => "maps",
            Self::Engine => "engine",
            Self::System => "systems",
            Self::Class => "classes",
            Self::Resource => "resources",
            Self::Message => "messages",
            Self::Enum => "enums",
            Self::Exe => "exe",
            Self::Question => "questions",
            Self::Experiment => "experiments",
            Self::Research => "research",
            Self::Index | Self::Meta => return None,
        })
    }

    /// The type a note at `rel` (path below the knowledge base) must have.
    pub fn of_path(rel: &str) -> Option<Self> {
        let (folder, file) = match rel.rsplit_once('/') {
            Some((folder, file)) => (folder, file),
            None => ("", rel),
        };
        match (folder, file) {
            ("", "index.md") => Some(Self::Map),
            ("", "README.md") => Some(Self::Meta),
            ("", _) => None,
            (_, "index.md") => Some(Self::Index),
            _ => Self::ALL.into_iter().find(|t| t.folder() == Some(folder)),
        }
    }
}

/// How much is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Known,
    Partial,
    Hypothesis,
    Unknown,
    Planned,
}

impl Status {
    pub const ALL: [Self; 5] = [
        Self::Known,
        Self::Partial,
        Self::Hypothesis,
        Self::Unknown,
        Self::Planned,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Known => "known",
            Self::Partial => "partial",
            Self::Hypothesis => "hypothesis",
            Self::Unknown => "unknown",
            Self::Planned => "planned",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == text)
    }
}

/// What the main statement of a note rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Confidence {
    /// The code of the executable.
    Exe,
    /// The game files.
    Data,
    /// An experiment in the running game.
    Bench,
    Guess,
}

impl Confidence {
    pub const ALL: [Self; 4] = [Self::Exe, Self::Data, Self::Bench, Self::Guess];

    pub fn name(self) -> &'static str {
        match self {
            Self::Exe => "exe",
            Self::Data => "data",
            Self::Bench => "bench",
            Self::Guess => "guess",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == text)
    }
}

/// The properties of a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub note_type: NoteType,
    pub status: Status,
    pub confidence: Confidence,
    pub editions: Vec<String>,
    pub stages: Vec<u32>,
    pub sources: Vec<String>,
    pub tags: Vec<String>,
    /// `YYYY-MM-DD`.
    pub updated: String,
}

impl Meta {
    /// The front matter block, with the `---` lines.
    pub fn render(&self) -> String {
        let quoted = |list: &[String]| {
            let items: Vec<String> = list
                .iter()
                .map(|s| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")))
                .collect();
            format!("[{}]", items.join(", "))
        };
        let stages: Vec<String> = self.stages.iter().map(u32::to_string).collect();
        format!(
            "---\ntype: {}\nstatus: {}\nconfidence: {}\neditions: {}\nstages: [{}]\nsources: {}\n\
             tags: [{}]\nupdated: {}\n---\n",
            self.note_type.name(),
            self.status.name(),
            self.confidence.name(),
            quoted(&self.editions),
            stages.join(", "),
            quoted(&self.sources),
            self.tags.join(", "),
            self.updated,
        )
    }

    /// The properties of front matter `fields`, or everything wrong with
    /// them.
    pub fn from_fields(fields: &[(String, Field)]) -> std::result::Result<Self, Vec<String>> {
        let mut problems = Vec::new();
        for (key, _) in fields {
            if !PROPERTIES.contains(&key.as_str()) {
                problems.push(format!("unknown property `{key}`"));
            }
            if fields.iter().filter(|(k, _)| k == key).count() > 1 {
                problems.push(format!("property `{key}` is given twice"));
            }
        }
        problems.dedup();
        let get = |key: &str| fields.iter().find(|(k, _)| k == key).map(|(_, v)| v);
        let mut scalar = |key: &str| match get(key) {
            Some(Field::Scalar(s)) if !s.is_empty() => Some(s.clone()),
            Some(_) => {
                problems.push(format!("property `{key}` must be one value"));
                None
            }
            None => {
                problems.push(format!("property `{key}` is missing"));
                None
            }
        };
        let note_type = scalar("type");
        let status = scalar("status");
        let confidence = scalar("confidence");
        let updated = scalar("updated");
        let mut list = |key: &str| match get(key) {
            Some(Field::List(items)) => Some(items.clone()),
            Some(Field::Scalar(_)) => {
                problems.push(format!("property `{key}` must be a list"));
                None
            }
            None => {
                problems.push(format!("property `{key}` is missing"));
                None
            }
        };
        let editions = list("editions");
        let stages = list("stages");
        let sources = list("sources");
        let tags = list("tags");

        let note_type = note_type.and_then(|t| {
            let parsed = NoteType::parse(&t);
            if parsed.is_none() {
                problems.push(format!("unknown type `{t}`"));
            }
            parsed
        });
        let status = status.and_then(|s| {
            let parsed = Status::parse(&s);
            if parsed.is_none() {
                problems.push(format!("unknown status `{s}`"));
            }
            parsed
        });
        let confidence = confidence.and_then(|c| {
            let parsed = Confidence::parse(&c);
            if parsed.is_none() {
                problems.push(format!("unknown confidence `{c}`"));
            }
            parsed
        });
        if let Some(date) = &updated
            && !is_date(date)
        {
            problems.push(format!("`updated: {date}` is not a date YYYY-MM-DD"));
        }
        if let Some(editions) = &editions {
            if editions.is_empty() {
                problems.push("`editions` is empty".into());
            }
            for e in editions {
                if e != TARGET_EDITION {
                    problems.push(format!(
                        "edition `{e}`: the only edition described is {TARGET_EDITION}"
                    ));
                }
            }
        }
        let stages = stages.map(|list| {
            if list.is_empty() {
                problems.push("`stages` is empty".into());
            }
            list.iter()
                .filter_map(|s| {
                    let n = s.parse::<u32>().ok();
                    if n.is_none() {
                        problems.push(format!("stage `{s}` is not a number"));
                    }
                    n
                })
                .collect::<Vec<u32>>()
        });
        if let Some(tags) = &tags {
            for t in tags {
                let ok = !t.is_empty()
                    && t.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
                if !ok {
                    problems.push(format!(
                        "tag `{t}`: lower-case Latin letters, digits and hyphens only"
                    ));
                }
            }
        }
        match (
            note_type, status, confidence, editions, stages, sources, tags, updated,
        ) {
            (
                Some(note_type),
                Some(status),
                Some(confidence),
                Some(editions),
                Some(stages),
                Some(sources),
                Some(tags),
                Some(updated),
            ) if problems.is_empty() => Ok(Self {
                note_type,
                status,
                confidence,
                editions,
                stages,
                sources,
                tags,
                updated,
            }),
            _ => Err(problems),
        }
    }
}

fn is_date(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
        && matches!(text[5..7].parse::<u32>(), Ok(1..=12))
        && matches!(text[8..10].parse::<u32>(), Ok(1..=31))
}

/// A value of the front matter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    Scalar(String),
    List(Vec<String>),
}

/// Splits a note into the text of its front matter (between the `---`
/// lines) and the rest.
pub fn split_front_matter(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("---\n")?;
    if let Some(end) = rest.find("\n---\n") {
        return Some((&rest[..end + 1], &rest[end + 5..]));
    }
    let front = rest.strip_suffix("\n---")?;
    Some((&rest[..front.len() + 1], ""))
}

fn unquote(text: &str) -> String {
    let text = text.trim();
    let quoted = |q: char| text.len() >= 2 && text.starts_with(q) && text.ends_with(q);
    if quoted('"') {
        let mut out = String::new();
        let mut chars = text[1..text.len() - 1].chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => out.extend(chars.next()),
                c => out.push(c),
            }
        }
        out
    } else if quoted('\'') {
        text[1..text.len() - 1].replace("''", "'")
    } else {
        text.to_owned()
    }
}

/// The items of an inline list `[a, "b, c"]`.
fn inline_list(text: &str) -> Vec<String> {
    let inner = &text[1..text.len() - 1];
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in inner.chars() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }
        match (quote, c) {
            (Some('"'), '\\') => {
                current.push(c);
                escaped = true;
            }
            (Some(q), c) if c == q => {
                current.push(c);
                quote = None;
            }
            (None, '"' | '\'') => {
                current.push(c);
                quote = Some(c);
            }
            (None, ',') => items.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    items.push(current);
    items
        .iter()
        .map(|i| unquote(i))
        .filter(|i| !i.is_empty())
        .collect()
}

/// Parses the subset of YAML the properties use: `key: value`, inline lists
/// `key: [a, b]` and block lists of `- item` lines.
pub fn parse_front_matter(front: &str) -> std::result::Result<Vec<(String, Field)>, String> {
    let mut fields: Vec<(String, Field)> = Vec::new();
    let mut open_list = false;
    for line in front.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(item) = trimmed
            .strip_prefix("- ")
            .or((trimmed == "-").then_some(""))
        {
            match fields.last_mut() {
                Some((_, Field::List(items))) if open_list => {
                    let item = unquote(item);
                    if !item.is_empty() {
                        items.push(item);
                    }
                    continue;
                }
                _ => return Err(format!("list item without a property: `{trimmed}`")),
            }
        }
        if line.starts_with([' ', '\t']) {
            return Err(format!("unexpected indented line: `{trimmed}`"));
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            return Err(format!("not a `key: value` line: `{trimmed}`"));
        };
        let value = value.trim();
        open_list = value.is_empty();
        let field = if value.is_empty() {
            Field::List(Vec::new())
        } else if value.starts_with('[') && value.ends_with(']') {
            Field::List(inline_list(value))
        } else {
            Field::Scalar(unquote(value))
        };
        fields.push((key.trim().to_owned(), field));
    }
    Ok(fields)
}

/// One note.
#[derive(Debug, Clone)]
pub struct Note {
    /// Path below the knowledge base, with `/`: `classes/Gun.md`.
    pub rel: String,
    /// The whole file.
    pub text: String,
    /// The properties, or what is wrong with them.
    pub meta: std::result::Result<Meta, Vec<String>>,
    /// The first `# ` heading; the file name when there is none.
    pub title: String,
}

impl Note {
    pub fn parse(rel: &str, text: &str) -> Self {
        let text = text.replace("\r\n", "\n");
        let meta = match split_front_matter(&text) {
            None => Err(vec![
                "no front matter (the note must start with `---`)".to_owned(),
            ]),
            Some((front, _)) => match parse_front_matter(front) {
                Ok(fields) => Meta::from_fields(&fields),
                Err(e) => Err(vec![format!("front matter: {e}")]),
            },
        };
        let title = body_of(&text)
            .lines()
            .find_map(|l| l.strip_prefix("# "))
            .map_or_else(|| rel.to_owned(), |t| t.trim().to_owned());
        Self {
            rel: rel.to_owned(),
            text,
            meta,
            title,
        }
    }

    /// The text after the front matter.
    pub fn body(&self) -> &str {
        body_of(&self.text)
    }
}

fn body_of(text: &str) -> &str {
    split_front_matter(text).map_or(text, |(_, body)| body)
}

/// The notes of a knowledge base, sorted by path.
#[derive(Debug, Clone, Default)]
pub struct Kb {
    pub notes: Vec<Note>,
}

fn io_error(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.to_owned(),
        source,
    }
}

fn collect(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| io_error(dir, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_error(dir, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if path.is_dir() {
            collect(&path, &rel, out)?;
        } else if name.ends_with(".md") {
            out.push((rel, path));
        }
    }
    Ok(())
}

impl Kb {
    /// Reads every `.md` file below `dir`.
    pub fn load(dir: &Path) -> Result<Self> {
        let mut files = Vec::new();
        collect(dir, "", &mut files)?;
        files.sort();
        let mut notes = Vec::with_capacity(files.len());
        for (rel, path) in files {
            let text = std::fs::read_to_string(&path).map_err(|e| io_error(&path, e))?;
            notes.push(Note::parse(&rel, &text));
        }
        Ok(Self { notes })
    }

    /// A knowledge base of `(path below the base, text)` pairs, as a binary
    /// embeds them.
    pub fn from_texts<'a>(texts: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut notes: Vec<Note> = texts
            .into_iter()
            .map(|(rel, text)| Note::parse(rel, text))
            .collect();
        notes.sort_by(|a, b| a.rel.cmp(&b.rel));
        Self { notes }
    }

    pub fn note(&self, rel: &str) -> Option<&Note> {
        self.notes
            .binary_search_by(|n| n.rel.as_str().cmp(rel))
            .ok()
            .map(|i| &self.notes[i])
    }
}

/// The lines of a note outside fenced code blocks, with inline code blanked
/// out: links and markers are looked for only there. Yields the 1-based line
/// number and the line.
pub(crate) fn prose_lines(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut fenced = false;
    for (i, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let mut clean = String::with_capacity(line.len());
        let mut code = false;
        for c in line.chars() {
            if c == '`' {
                code = !code;
                clean.push(' ');
            } else if code {
                clean.push(' ');
            } else {
                clean.push(c);
            }
        }
        out.push((i + 1, clean));
    }
    out
}

/// The targets of the Markdown links `[text](target)` of a note, with their
/// line numbers.
pub(crate) fn links(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (line_no, line) in prose_lines(text) {
        let mut rest = line.as_str();
        while let Some(at) = rest.find("](") {
            rest = &rest[at + 2..];
            let Some(end) = rest.find(')') else { break };
            let target = rest[..end].trim();
            let target = target
                .strip_prefix('<')
                .and_then(|t| t.strip_suffix('>'))
                .unwrap_or(target);
            // `[text](path "title")`
            let target = target.split_whitespace().next().unwrap_or("");
            out.push((line_no, target.to_owned()));
            rest = &rest[end + 1..];
        }
    }
    out
}

/// `base` (a folder, `/`-separated, may be empty) joined with the relative
/// `target`, with `.` and `..` resolved. `None` when the path leaves the
/// root.
pub(crate) fn join_rel(base: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

/// Today's date (UTC) as `YYYY-MM-DD`.
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "---\ntype: question\nstatus: unknown\nconfidence: guess\n\
        editions: [\"1.0.789.0\"]\nstages: [72, 84]\nsources: [\"docs/a.md §1, §2\", \"x\"]\n\
        tags: [simgroup, zones]\nupdated: 2026-11-16\n---\n# Title here\n\nText.\n";

    #[test]
    fn a_note_parses_and_renders_the_same_front_matter() {
        let note = Note::parse("questions/a.md", NOTE);
        let meta = note.meta.clone().unwrap();
        assert_eq!(note.title, "Title here");
        assert_eq!(meta.note_type, NoteType::Question);
        assert_eq!(meta.stages, [72, 84]);
        assert_eq!(meta.sources, ["docs/a.md §1, §2", "x"]);
        assert_eq!(meta.tags, ["simgroup", "zones"]);
        assert!(NOTE.starts_with(&meta.render()));
        assert_eq!(note.body(), "# Title here\n\nText.\n");
    }

    #[test]
    fn block_lists_are_read() {
        let front = "type: map\nsources:\n  - \"a b\"\n  - c\ntags: []\n";
        let fields = parse_front_matter(front).unwrap();
        assert_eq!(
            fields[1],
            (
                "sources".to_owned(),
                Field::List(vec!["a b".into(), "c".into()])
            )
        );
        assert_eq!(fields[2].1, Field::List(Vec::new()));
    }

    #[test]
    fn wrong_properties_are_named() {
        let text = NOTE
            .replace("status: unknown", "status: solved")
            .replace("1.0.789.0", "1.0.764.0")
            .replace("tags: [simgroup, zones]", "tags: [Sim Group]\nextra: 1")
            .replace("2026-11-16", "16.11.2026");
        let problems = Note::parse("questions/a.md", &text).meta.unwrap_err();
        let all = problems.join("\n");
        for part in [
            "unknown status `solved`",
            "edition `1.0.764.0`",
            "tag `Sim Group`",
            "unknown property `extra`",
            "not a date",
        ] {
            assert!(all.contains(part), "{part}: {all}");
        }
        let missing = Note::parse("a.md", "# No properties\n").meta.unwrap_err();
        assert!(missing[0].contains("no front matter"));
    }

    #[test]
    fn the_type_follows_from_the_folder() {
        assert_eq!(NoteType::of_path("index.md"), Some(NoteType::Map));
        assert_eq!(NoteType::of_path("README.md"), Some(NoteType::Meta));
        assert_eq!(NoteType::of_path("classes/Gun.md"), Some(NoteType::Class));
        assert_eq!(NoteType::of_path("classes/index.md"), Some(NoteType::Index));
        assert_eq!(NoteType::of_path("maps/engine.md"), Some(NoteType::Map));
        assert_eq!(NoteType::of_path("other/x.md"), None);
    }

    #[test]
    fn links_skip_code_and_paths_resolve() {
        let text = "See [a](../a.md) and `[no](x.md)`.\n```\n[no](y.md)\n```\n\
                    [b](<b c.md>) [c](c.md \"title\")\n";
        let found: Vec<String> = links(text).into_iter().map(|(_, t)| t).collect();
        assert_eq!(found, ["../a.md", "b", "c.md"]);
        assert_eq!(join_rel("docs/kb/exe", "../../a.md").unwrap(), "docs/a.md");
        assert_eq!(join_rel("", "../a.md"), None);
    }

    #[test]
    fn today_is_a_date() {
        assert!(is_date(&today()));
    }
}
