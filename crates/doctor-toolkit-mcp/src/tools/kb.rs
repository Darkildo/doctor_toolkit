//! `kb_search` and `kb_note`: the knowledge base `docs/kb/` embedded in the
//! binary — what is known about the engine and the game, open questions,
//! experiments, addresses of the executable.

use serde_json::{Value, json};

use doctor_toolkit::kb::search::{Query, search as kb_search_notes};
use doctor_toolkit::kb::{Note, NoteType, Status};

use super::{KbNoteParams, KbSearchParams, Output, clamp};
use crate::resources::{SCHEME, knowledge_base};
use crate::state::{ToolResult, Toolkit, fail};

fn brief(note: &Note) -> Value {
    let mut item = json!({
        "note": note.rel,
        "uri": format!("{SCHEME}kb/{}", note.rel),
        "title": note.title,
    });
    if let Ok(meta) = &note.meta {
        item["type"] = json!(meta.note_type.name());
        item["status"] = json!(meta.status.name());
        item["confidence"] = json!(meta.confidence.name());
        item["stages"] = json!(meta.stages);
        item["tags"] = json!(meta.tags);
    }
    item
}

/// Notes of the knowledge base by words, type and status.
pub fn kb_search(_kit: &Toolkit, p: KbSearchParams) -> ToolResult<Output> {
    let mut query = Query::new(p.query.as_deref().unwrap_or(""));
    if let Some(text) = &p.note_type {
        let Some(note_type) = NoteType::parse(text) else {
            let names: Vec<&str> = NoteType::ALL.iter().map(|t| t.name()).collect();
            return fail(format!(
                "unknown type `{text}`; one of {}",
                names.join(", ")
            ));
        };
        query.note_type = Some(note_type);
    }
    if let Some(text) = &p.status {
        let Some(status) = Status::parse(text) else {
            let names: Vec<&str> = Status::ALL.iter().map(|s| s.name()).collect();
            return fail(format!(
                "unknown status `{text}`; one of {}",
                names.join(", ")
            ));
        };
        query.status = Some(status);
    }
    if query.words.is_empty() && query.note_type.is_none() && query.status.is_none() {
        return fail("give `query` words, or a `type` or `status` to list notes");
    }
    let offset = p.offset.unwrap_or(0);
    let limit = clamp(p.limit, 20, 100);
    let (hits, total) = kb_search_notes(knowledge_base(), &query, offset, limit);
    let results: Vec<Value> = hits
        .iter()
        .map(|hit| {
            let mut item = brief(hit.note);
            item["snippet"] = json!(hit.snippet);
            item
        })
        .collect();
    Ok(Output::Json(json!({
        "total": total,
        "offset": offset,
        "more": offset + results.len() < total,
        "results": results,
    })))
}

/// The text of one note.
pub fn kb_note(_kit: &Toolkit, p: KbNoteParams) -> ToolResult<Output> {
    let rel = p
        .note
        .strip_prefix(SCHEME)
        .unwrap_or(&p.note)
        .trim_start_matches("docs/")
        .trim_start_matches("kb/");
    let Some(note) = knowledge_base().note(rel) else {
        return fail(format!(
            "no note `{rel}` in the knowledge base; kb_search finds notes, `index.md` is the map"
        ));
    };
    Ok(Output::Text(note.text.clone()))
}
