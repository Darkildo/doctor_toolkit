//! Deltas of string tables: texts set and strings added, addressed by the
//! hash of the string and the language column.
//!
//! ```json
//! {"format":1,"kind":"lh2","ops":[
//!   {"op":"set","hash":"0266681f","language":"en","old":"Fredo","text":"Alfredo"},
//!   {"op":"add","hash":"5d0c7e11","texts":[{"language":"en","text":"New"},{"language":"cs","text":"Nový"}]}
//! ]}
//! ```
//!
//! A column is named by its language code; a table that does not name its
//! languages (the text streams of the game hold one language each) has the
//! columns `0`, `1`, …. A string cannot be removed and the languages of a
//! table cannot change: such a table is kept whole.

use serde::{Deserialize, Serialize};

use super::{Applied, Delta, FORMAT, Kind, Report, check_head, checked};
use crate::resource::lh2::StringTable;
use crate::resource::simgroup::Hash32;
use crate::{Error, Result};

/// A delta of a string table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StringsDelta {
    pub format: u32,
    pub kind: Kind,
    pub ops: Vec<Op>,
}

/// One operation on a string table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    /// Sets the text of a string in one language column.
    Set {
        hash: Hash32,
        language: String,
        /// The text the author changed, so a change of the same string by
        /// something else can be told.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old: Option<String>,
        text: String,
    },
    /// Adds a string. A column of the table the operation has no text for
    /// gets the first text of the operation, so the string shows in every
    /// language.
    Add { hash: Hash32, texts: Vec<Text> },
}

impl Op {
    /// What the operation addresses, as the notes of [`apply`] name it.
    pub fn target(&self) -> String {
        match self {
            Self::Set { hash, language, .. } => format!("string {hash}, language {language}"),
            Self::Add { hash, .. } => format!("string {hash}"),
        }
    }
}

/// The text of a new string in one language column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub language: String,
    pub text: String,
}

/// The delta that turns `base` into `mine`, or `None` when a string was
/// removed, the languages or the header of the table changed, or the delta
/// does not give `mine` back byte for byte.
pub fn diff(base: &[u8], mine: &[u8]) -> Result<Option<StringsDelta>> {
    let old = StringTable::parse(base)?;
    let new = StringTable::parse(mine)?;
    if (old.version, old.flags, &old.languages, old.strings.len())
        != (new.version, new.flags, &new.languages, new.strings.len())
        || old.hashes.iter().any(|&h| new.find(h).is_none())
    {
        return Ok(None);
    }
    let columns = new.column_names();
    let mut sets = Vec::new();
    let mut adds = Vec::new();
    for (row, &hash) in new.hashes.iter().enumerate() {
        match old.find(hash) {
            Some(was) => {
                for (column, language) in columns.iter().enumerate() {
                    let (before, after) = (&old.strings[column][was], &new.strings[column][row]);
                    if before != after {
                        sets.push(Op::Set {
                            hash: Hash32(hash),
                            language: language.clone(),
                            old: Some(before.clone()),
                            text: after.clone(),
                        });
                    }
                }
            }
            None => adds.push(Op::Add {
                hash: Hash32(hash),
                texts: columns
                    .iter()
                    .enumerate()
                    .map(|(column, language)| Text {
                        language: language.clone(),
                        text: new.strings[column][row].clone(),
                    })
                    .collect(),
            }),
        }
    }
    sets.append(&mut adds);
    let delta = StringsDelta {
        format: FORMAT,
        kind: Kind::Lh2,
        ops: sets,
    };
    Ok(match checked(Delta::Lh2(delta), base, mine) {
        Some(Delta::Lh2(delta)) => Some(delta),
        _ => None,
    })
}

/// Applies `delta` to the table `base`. A text of a string or a language
/// the table does not have is skipped and listed; a new string whose hash is
/// taken is an error.
pub fn apply(delta: &StringsDelta, base: &[u8]) -> Result<Applied> {
    check_head(delta.format, delta.kind, Kind::Lh2)?;
    let mut table = StringTable::parse(base)?;
    let columns = table.column_names();
    let column = |language: &str| columns.iter().position(|c| c == language);
    let mut report = Report::default();
    for (i, op) in delta.ops.iter().enumerate() {
        match op {
            Op::Set {
                hash,
                language,
                old,
                text,
            } => {
                let target = op.target();
                let Some(row) = table.find(hash.0) else {
                    report.skip(i, target, "the table has no such string");
                    continue;
                };
                let Some(column) = column(language) else {
                    report.skip(i, target, "the table has no such language");
                    continue;
                };
                let current = &mut table.strings[column][row];
                if let Some(old) = old
                    && current != old
                    && current != text
                {
                    report.conflict(
                        i,
                        target,
                        format!("the text was {current:?}, not {old:?} as when the delta was made"),
                    );
                }
                current.clone_from(text);
                report.applied();
            }
            Op::Add { hash, texts } => {
                let fallback = texts.first().map_or("", |t| t.text.as_str());
                let row: Vec<String> = columns
                    .iter()
                    .map(|language| {
                        texts
                            .iter()
                            .find(|t| t.language == *language)
                            .map_or(fallback, |t| t.text.as_str())
                            .to_owned()
                    })
                    .collect();
                // A taken hash is refused here: two mods cannot both add it.
                table.insert(hash.0, &row).map_err(|e| {
                    Error::InvalidArgument(format!("operation {} (string {hash}): {e}", i + 1))
                })?;
                report.applied();
            }
        }
    }
    let data = if report.any_applied() {
        table.to_bytes()?
    } else {
        base.to_vec()
    };
    Ok(report.finish(data))
}
