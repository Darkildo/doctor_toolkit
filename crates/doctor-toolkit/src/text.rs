//! New strings: entries added to the string tables (`lh2`) of every
//! language, and references to them.
//!
//! A string is identified by the hash of its name ([`crate::hash::string_id`]).
//! Interface movies (`apx`) show `$name`; SimGroup attributes of type hash
//! (`UIHudObjective`, `UIJobInfo`, `NPC` names, ...) store the hash. The game
//! looks a hash up in every loaded table, the most recently loaded first, so
//! a new entry must have a hash that no table of the game uses.
//!
//! The main tables are one per language, in the streams the `lhr` resource
//! names (`en` → `gf2/text/DD7D7050`, `es`, `ss`, `ll`); [`plan_add`] adds the
//! entry to all of them (or to tables the caller chooses), each in its own
//! language when a translation is given, and records one workspace operation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::game::{GameInstance, ResourceIndex};
use crate::hash::{sdbm, string_id};
use crate::resource::apx::AptFile;
use crate::resource::lh2::StringTable;
use crate::workspace::{ResourceEdit, ResourceKey, StorageChoice, Workspace};
use crate::{Error, Result};

/// Type id of `lh2` resources.
pub const TYPE_LH2: u32 = sdbm(b"lh2");
/// Type id of `lhr` resources (language → string table stream).
pub const TYPE_LHR: u32 = sdbm(b"lhr");
/// Type id of `apx` resources.
pub const TYPE_APX: u32 = sdbm(b"apx");

/// A string table of the game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableRef {
    pub stream: usize,
    pub resource: usize,
    /// Language of a one-language table (from the `lhr`), `None` otherwise.
    pub language: Option<String>,
}

/// Languages of the `lhr` text: `(code, stream path)`. The text is a count
/// line, then a code, a stream path and a hash line per language.
pub fn parse_lhr(text: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    lines
        .get(1..)
        .unwrap_or_default()
        .chunks(3)
        .filter(|c| c.len() >= 2 && !c[0].is_empty())
        .map(|c| (c[0].to_owned(), c[1].replace('\\', "/").to_lowercase()))
        .collect()
}

/// The one-language tables of the game's languages, from its first `lhr`.
pub fn language_tables(game: &GameInstance, index: &ResourceIndex) -> Result<Vec<TableRef>> {
    let lhr = index
        .entries
        .iter()
        .find(|e| e.header.type_id == TYPE_LHR)
        .ok_or_else(|| Error::InvalidArgument("the game has no lhr resource".into()))?;
    let data = game
        .open_stream(lhr.stream as usize)?
        .resource_data(lhr.resource as usize)?;
    let mut out = Vec::new();
    for (language, path) in parse_lhr(&String::from_utf8_lossy(&data)) {
        let Some(stream) = game.find_stream(&path) else {
            continue;
        };
        if let Some(e) = index
            .entries
            .iter()
            .find(|e| e.stream as usize == stream && e.header.type_id == TYPE_LH2)
        {
            out.push(TableRef {
                stream,
                resource: e.resource as usize,
                language: Some(language),
            });
        }
    }
    if out.is_empty() {
        return Err(Error::InvalidArgument(
            "no string table of the lhr languages was found".into(),
        ));
    }
    Ok(out)
}

/// Every string table of the game.
pub fn all_tables(index: &ResourceIndex) -> Vec<TableRef> {
    index
        .entries
        .iter()
        .filter(|e| e.header.type_id == TYPE_LH2)
        .map(|e| TableRef {
            stream: e.stream as usize,
            resource: e.resource as usize,
            language: None,
        })
        .collect()
}

/// A string to add.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewString {
    /// Entry name: letters, digits and `_`; its hash is the entry id.
    pub name: String,
    /// Text for languages without a translation.
    pub text: String,
    /// Language code → text.
    pub translations: BTreeMap<String, String>,
}

/// Whether `name` can be used in `$name` references.
pub fn valid_name(name: &str) -> bool {
    name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// What [`plan_add`] does.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AddedString {
    pub name: String,
    pub hash: u32,
    /// `(table key, language codes and texts)`.
    pub tables: Vec<(String, Vec<(String, String)>)>,
}

/// An addition ready to be recorded.
#[derive(Debug, Clone)]
pub struct StringPlan {
    pub added: AddedString,
    pub label: String,
    edits: Vec<(usize, Vec<ResourceEdit>)>,
}

impl StringPlan {
    /// Records the addition as one undoable operation.
    pub fn commit(&self, ws: &mut Workspace, game: &GameInstance) -> Result<Option<u64>> {
        ws.change_resources(game, &self.label, &self.edits, &[])
    }
}

fn table_key(game: &GameInstance, table: &TableRef) -> Result<ResourceKey> {
    let stream = game.open_stream(table.stream)?;
    let header = stream
        .layout()
        .resources
        .get(table.resource)
        .ok_or_else(|| Error::InvalidArgument(format!("no resource {}", table.resource)))?
        .header;
    if header.type_id != TYPE_LH2 {
        return Err(Error::InvalidArgument(format!(
            "resource {} of {} is not a string table",
            table.resource,
            game.streams()[table.stream].stream_name()
        )));
    }
    Ok(ResourceKey::new(
        &game.streams()[table.stream].stream_name(),
        &header,
    ))
}

/// Works out adding `new` to `tables` (as the workspace has them). Every
/// table of the game (`index`) is checked: the hash must be free in all of
/// them.
pub fn plan_add(
    game: &GameInstance,
    ws: &Workspace,
    index: &ResourceIndex,
    tables: &[TableRef],
    new: &NewString,
) -> Result<StringPlan> {
    if !valid_name(&new.name) {
        return Err(Error::InvalidArgument(format!(
            "`{}` is not a string name: use letters, digits and _, starting with a letter",
            new.name
        )));
    }
    if tables.is_empty() {
        return Err(Error::InvalidArgument(
            "no table to add the string to".into(),
        ));
    }
    let hash = string_id(&new.name);
    for t in all_tables(index) {
        let data = ws.current_data(game, t.stream, t.resource)?;
        if StringTable::parse(&data)?.find(hash).is_some() {
            return Err(Error::InvalidArgument(format!(
                "`{}` (hash {hash:08x}) is already an entry of {}",
                new.name,
                game.streams()[t.stream].stream_name()
            )));
        }
    }
    let mut by_stream: BTreeMap<usize, Vec<ResourceEdit>> = BTreeMap::new();
    let mut report = Vec::new();
    for t in tables {
        let key = table_key(game, t)?;
        let mut table = StringTable::parse(&ws.current_data(game, t.stream, t.resource)?)?;
        let languages: Vec<String> = if table.languages.is_empty() {
            match &t.language {
                Some(code) => vec![code.clone(); table.strings.len()],
                None => (0..table.strings.len()).map(|i| format!("#{i}")).collect(),
            }
        } else {
            table.languages.clone()
        };
        let texts: Vec<String> = languages
            .iter()
            .map(|code| {
                new.translations
                    .get(code)
                    .cloned()
                    .unwrap_or_else(|| new.text.clone())
            })
            .collect();
        table.insert(hash, &texts)?;
        report.push((key.to_string(), languages.into_iter().zip(texts).collect()));
        by_stream.entry(t.stream).or_default().push(ResourceEdit {
            index: t.resource,
            data: table.to_bytes()?,
            storage: StorageChoice::Keep,
        });
    }
    Ok(StringPlan {
        label: format!(
            "Add string {} ({hash:08x}) to {} table(s)",
            new.name,
            tables.len()
        ),
        added: AddedString {
            name: new.name.clone(),
            hash,
            tables: report,
        },
        edits: by_stream.into_iter().collect(),
    })
}

/// A new text for an entry of the string tables.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StringEdit {
    /// Hash of the entry ([`string_id`] of its name).
    pub hash: u32,
    pub text: String,
    /// Only the columns of this language (a code as the tables name their
    /// columns, any case); every column otherwise.
    #[serde(default)]
    pub language: Option<String>,
}

/// One text [`plan_edit`] changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StringChange {
    /// `<stream>#<resource>` of the table.
    pub table: String,
    /// The column: a language code, or `text`, `text1`, … of a table
    /// without languages.
    pub language: String,
    pub old: String,
    pub new: String,
}

/// An edit of one entry in several tables, ready to be recorded.
#[derive(Debug, Clone)]
pub struct EditPlan {
    /// Tables that have the entry.
    pub found: usize,
    pub changes: Vec<StringChange>,
    pub label: String,
    edits: Vec<(usize, Vec<ResourceEdit>)>,
}

impl EditPlan {
    /// Tables the edit changes.
    pub fn tables(&self) -> usize {
        self.edits.iter().map(|(_, list)| list.len()).sum()
    }

    /// Records the edit as one undoable operation; nothing when no text
    /// changes.
    pub fn commit(&self, ws: &mut Workspace, game: &GameInstance) -> Result<Option<u64>> {
        if self.edits.is_empty() {
            return Ok(None);
        }
        ws.change_resources(game, &self.label, &self.edits, &[])
    }
}

/// Column names of a string table as the game index names them: its
/// language codes, else `text`, `text1`, …
pub fn column_names(table: &StringTable) -> Vec<String> {
    if table.languages.is_empty() {
        (0..table.strings.len())
            .map(|i| {
                if i == 0 {
                    "text".to_owned()
                } else {
                    format!("text{i}")
                }
            })
            .collect()
    } else {
        table.languages.clone()
    }
}

/// Works out giving the entry of `edit` its new text in every one of
/// `tables` that has it (as the workspace has them) — all tables of the
/// game with [`all_tables`]: an entry usually lives in the table of every
/// language. An entry no table has is an error; an entry that already has
/// the text gives a plan without changes.
pub fn plan_edit(
    game: &GameInstance,
    ws: &Workspace,
    tables: &[TableRef],
    edit: &StringEdit,
) -> Result<EditPlan> {
    plan_edits(game, ws, tables, std::slice::from_ref(edit))
}

/// [`plan_edit`] for several entries at once: one operation.
pub fn plan_edits(
    game: &GameInstance,
    ws: &Workspace,
    tables: &[TableRef],
    edits: &[StringEdit],
) -> Result<EditPlan> {
    if edits.iter().any(|e| e.text.contains('\0')) {
        return Err(Error::InvalidArgument(
            "the text contains a NUL character".into(),
        ));
    }
    let mut by_stream: BTreeMap<usize, Vec<ResourceEdit>> = BTreeMap::new();
    let mut changes = Vec::new();
    // Tables that have the entry, per edit.
    let mut found = vec![0usize; edits.len()];
    for t in tables {
        let mut table = StringTable::parse(&ws.current_data(game, t.stream, t.resource)?)?;
        let names = column_names(&table);
        let mut changed = false;
        for (e, edit) in edits.iter().enumerate() {
            let Some(at) = table.find(edit.hash) else {
                continue;
            };
            found[e] += 1;
            for (c, name) in names.iter().enumerate() {
                if edit
                    .language
                    .as_deref()
                    .is_some_and(|l| !name.eq_ignore_ascii_case(l))
                {
                    continue;
                }
                if table.strings[c][at] != edit.text {
                    changes.push(StringChange {
                        table: format!("{}#{}", game.streams()[t.stream].stream_name(), t.resource),
                        language: name.clone(),
                        old: std::mem::replace(&mut table.strings[c][at], edit.text.clone()),
                        new: edit.text.clone(),
                    });
                    changed = true;
                }
            }
        }
        if changed {
            by_stream.entry(t.stream).or_default().push(ResourceEdit {
                index: t.resource,
                data: table.to_bytes()?,
                storage: StorageChoice::Keep,
            });
        }
    }
    if let Some(e) = found.iter().position(|&n| n == 0) {
        return Err(Error::InvalidArgument(format!(
            "no string table has the entry {:08x}",
            edits[e].hash
        )));
    }
    let edits_out: Vec<(usize, Vec<ResourceEdit>)> = by_stream.into_iter().collect();
    let count: usize = edits_out.iter().map(|(_, list)| list.len()).sum();
    let label = match edits {
        [one] => format!("Edit string 0x{:08x} in {count} table(s)", one.hash),
        many => format!("Edit {} strings in {count} table(s)", many.len()),
    };
    Ok(EditPlan {
        found: found.iter().copied().max().unwrap_or(0),
        changes,
        label,
        edits: edits_out,
    })
}

/// Points the `$old` text references of `apx` resource `resource` of
/// `stream` at `$new` (see [`AptFile::retarget_text`]). Returns the number of
/// references changed; nothing is recorded when it is 0.
pub fn retarget_apx(
    game: &GameInstance,
    ws: &mut Workspace,
    stream: usize,
    resource: usize,
    old: &str,
    new: &str,
) -> Result<usize> {
    let file = game.open_stream(stream)?;
    let header = file
        .layout()
        .resources
        .get(resource)
        .ok_or_else(|| Error::InvalidArgument(format!("no resource {resource}")))?
        .header;
    if header.type_id != TYPE_APX {
        return Err(Error::InvalidArgument(format!(
            "resource {resource} is not an apx movie"
        )));
    }
    drop(file);
    let mut movie = AptFile::parse(&ws.current_data(game, stream, resource)?)?;
    let changed = movie.retarget_text(old, new)?;
    if changed > 0 {
        let label = format!(
            "Point ${old} at ${new} in {} #{resource}",
            game.streams()[stream].stream_name()
        );
        ws.set_resource(game, stream, resource, movie.to_bytes()?, &label)?;
    }
    Ok(changed)
}

/// The column names `text-export` writes for a table: its language codes,
/// else `text` (one column) or `text0`, `text1`, ...
pub fn json_columns(table: &StringTable) -> Vec<String> {
    if table.languages.is_empty() && table.strings.len() == 1 {
        vec!["text".to_owned()]
    } else if table.languages.is_empty() {
        (0..table.strings.len())
            .map(|i| format!("text{i}"))
            .collect()
    } else {
        table.languages.clone()
    }
}

/// A string table as the editable JSON of `text-export`: where it is
/// (`stream`, `resource` index, `id`), the column names and the entries by
/// hash. [`import_json`] reads it back.
pub fn export_json(
    table: &StringTable,
    stream: &str,
    resource: usize,
    id: u32,
) -> serde_json::Value {
    use serde_json::{Map, Value, json};
    let columns = json_columns(table);
    let entries: Vec<Value> = table
        .hashes
        .iter()
        .enumerate()
        .map(|(entry, hash)| {
            let mut object = Map::new();
            object.insert("hash".into(), json!(format!("{hash:08x}")));
            for (lang, column) in columns.iter().enumerate() {
                object.insert(column.clone(), json!(table.strings[lang][entry]));
            }
            Value::Object(object)
        })
        .collect();
    json!({
        "stream": stream,
        "resource": resource,
        "id": format!("{id:08x}"),
        "languages": columns,
        "entries": entries,
    })
}

/// Takes the texts of a `text-export` document into `table` (entries by
/// hash; adding entries is not supported). Returns how many strings changed.
pub fn import_json(table: &mut StringTable, document: &serde_json::Value) -> Result<usize> {
    let bad = Error::InvalidArgument;
    let columns: Vec<&str> = document
        .get("languages")
        .and_then(|v| v.as_array())
        .ok_or_else(|| bad("missing `languages` list".into()))?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| bad("language codes must be strings".into()))
        })
        .collect::<Result<_>>()?;
    if columns.len() != table.strings.len() {
        return Err(bad(format!(
            "{} language column(s) in the file, {} in the table",
            columns.len(),
            table.strings.len()
        )));
    }
    let mut changed = 0;
    for entry in document
        .get("entries")
        .and_then(|v| v.as_array())
        .ok_or_else(|| bad("missing `entries` list".into()))?
    {
        let hash = entry
            .get("hash")
            .and_then(|v| v.as_str())
            .ok_or_else(|| bad("entry without `hash`".into()))?;
        let hash = u32::from_str_radix(hash, 16).map_err(|_| bad(format!("bad hash `{hash}`")))?;
        let row = table.find(hash).ok_or_else(|| {
            bad(format!(
                "hash {hash:08x} is not in the table; adding entries is not supported"
            ))
        })?;
        for (lang, column) in columns.iter().enumerate() {
            let value = entry
                .get(*column)
                .and_then(|v| v.as_str())
                .ok_or_else(|| bad(format!("entry {hash:08x} has no `{column}`")))?;
            if table.strings[lang][row] != value {
                table.strings[lang][row] = value.to_owned();
                changed += 1;
            }
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lhr_lists_languages_and_streams() {
        let text = "2\nen\ngf2\\text\\DD7D7050\n-578981808\nes\ngf2\\text\\AD8BF024\n-1383337948\n";
        assert_eq!(
            parse_lhr(text),
            vec![
                ("en".to_owned(), "gf2/text/dd7d7050".to_owned()),
                ("es".to_owned(), "gf2/text/ad8bf024".to_owned()),
            ]
        );
        assert!(parse_lhr("").is_empty());
        assert!(valid_name("doctor_new_1"));
        assert!(!valid_name("1abc") && !valid_name("a b") && !valid_name(""));
    }
}
