//! Search of the knowledge base by words.
//!
//! Every word of the query must occur in the note (title, path, tags or
//! text; case-insensitive). A word in the title or the file name weighs
//! most, then a tag, then the text.

use super::{Kb, Note, NoteType, Status};

/// What to look for.
#[derive(Debug, Clone, Default)]
pub struct Query {
    /// Words; a query without words lists the notes the filters leave.
    pub words: Vec<String>,
    pub note_type: Option<NoteType>,
    pub status: Option<Status>,
}

impl Query {
    /// Splits `text` into lower-case words.
    pub fn new(text: &str) -> Self {
        Self {
            words: text.split_whitespace().map(str::to_lowercase).collect(),
            ..Self::default()
        }
    }
}

/// A note the query found.
#[derive(Debug, Clone)]
pub struct Hit<'a> {
    pub note: &'a Note,
    pub score: u32,
    /// The first line of the text with a word of the query.
    pub snippet: String,
}

const SNIPPET_CHARS: usize = 240;

fn snippet(note: &Note, words: &[String]) -> String {
    let line = note
        .body()
        .lines()
        .filter(|l| !l.starts_with("# ") && !l.trim().is_empty() && !l.starts_with("<!--"))
        .find(|l| {
            let lower = l.to_lowercase();
            words.iter().any(|w| lower.contains(w.as_str()))
        })
        .or_else(|| {
            note.body()
                .lines()
                .find(|l| !l.starts_with('#') && !l.trim().is_empty() && !l.starts_with("<!--"))
        })
        .unwrap_or("");
    let mut out: String = line.trim().chars().take(SNIPPET_CHARS).collect();
    if line.trim().chars().count() > SNIPPET_CHARS {
        out.push('…');
    }
    out
}

/// The notes that match `query`, best first; equal scores keep the order of
/// paths. Returns the hits `offset..offset + limit` and the total.
pub fn search<'a>(kb: &'a Kb, query: &Query, offset: usize, limit: usize) -> (Vec<Hit<'a>>, usize) {
    let mut hits: Vec<Hit<'a>> = Vec::new();
    for note in &kb.notes {
        let meta = note.meta.as_ref().ok();
        if query
            .note_type
            .is_some_and(|t| meta.is_none_or(|m| m.note_type != t))
            || query
                .status
                .is_some_and(|s| meta.is_none_or(|m| m.status != s))
        {
            continue;
        }
        let title = note.title.to_lowercase();
        let path = note.rel.to_lowercase();
        let body = note.body().to_lowercase();
        let mut score = 0u32;
        let mut all = true;
        for word in &query.words {
            let mut s = 0;
            if title.contains(word.as_str()) {
                s += 8;
            }
            if path.contains(word.as_str()) {
                s += 6;
            }
            if meta.is_some_and(|m| m.tags.iter().any(|t| t == word)) {
                s += 4;
            }
            s += body.matches(word.as_str()).count().min(5) as u32;
            if s == 0 {
                all = false;
                break;
            }
            score += s;
        }
        if all {
            hits.push(Hit {
                note,
                score,
                snippet: String::new(),
            });
        }
    }
    hits.sort_by_key(|hit| std::cmp::Reverse(hit.score));
    let total = hits.len();
    let mut page: Vec<Hit<'a>> = hits.into_iter().skip(offset).take(limit).collect();
    for hit in &mut page {
        hit.snippet = snippet(hit.note, &query.words);
    }
    (page, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(note_type: &str, status: &str, tags: &str, body: &str) -> String {
        format!(
            "---\ntype: {note_type}\nstatus: {status}\nconfidence: data\n\
             editions: [\"1.0.789.0\"]\nstages: [72]\nsources: [\"x\"]\ntags: [{tags}]\n\
             updated: 2026-11-16\n---\n{body}"
        )
    }

    #[test]
    fn words_filters_and_order() {
        let a = note("engine", "known", "bus", "# Шина сообщений\n\nХэш имени.\n");
        let b = note(
            "question",
            "unknown",
            "bus",
            "# Данные сообщений\n\nЧто несёт сообщение шины?\n",
        );
        let c = note("class", "partial", "simgroup", "# Класс Gun\n\nУрон.\n");
        let kb = Kb::from_texts([
            ("engine/message-bus.md", a.as_str()),
            ("questions/message-data.md", b.as_str()),
            ("classes/Gun.md", c.as_str()),
        ]);
        let (hits, total) = search(&kb, &Query::new("Шин"), 0, 10);
        assert_eq!(total, 2);
        assert_eq!(hits[0].note.rel, "engine/message-bus.md");
        assert_eq!(hits[1].snippet, "Что несёт сообщение шины?");

        let mut query = Query::new("bus");
        query.status = Some(Status::Unknown);
        let (hits, _) = search(&kb, &query, 0, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].note.rel, "questions/message-data.md");

        let (hits, total) = search(&kb, &Query::new("шина gun"), 0, 10);
        assert!(hits.is_empty() && total == 0);

        let all = Query {
            note_type: Some(NoteType::Class),
            ..Query::default()
        };
        assert_eq!(search(&kb, &all, 0, 10).1, 1);
        assert_eq!(search(&kb, &Query::default(), 1, 1).0.len(), 1);
    }
}
