//! `kb-lint`: the checks every note of the knowledge base passes.
//!
//! * the properties follow the schema ([`super::Meta`]) and the type of a
//!   note is the type of its folder;
//! * one `# ` heading; no `[[wikilinks]]`;
//! * every relative link leads to a file of the repository, every source
//!   that is a path exists;
//! * no orphans: every note but the root map is linked from another note;
//! * an open question says what the next step is;
//! * nothing names a closed file ([`super::CLOSED_PATHS`]) — the rule of
//!   the public export.

use std::collections::HashSet;
use std::path::Path;

use super::{CLOSED_PATHS, Kb, NoteType, Status, join_rel, links, prose_lines};
use crate::Result;

/// The heading of the section of a question that says what to do next.
pub const NEXT_STEP_HEADING: &str = "## Следующий шаг";

/// Top-level folders of the repository a source may name; such a source
/// must exist.
const SOURCE_ROOTS: [&str; 6] = [
    "docs/",
    "data/",
    "crates/",
    "research/",
    "tools/",
    "examples/",
];

/// One thing wrong with a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Path of the note below the knowledge base.
    pub note: String,
    /// Line of the note, when the problem has one.
    pub line: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}:{line}: {}", self.note, self.message),
            None => write!(f, "{}: {}", self.note, self.message),
        }
    }
}

/// Where the knowledge base is in the repository.
#[derive(Debug, Clone)]
pub struct Layout<'a> {
    /// The repository root: links and sources may not leave it.
    pub root: &'a Path,
    /// Path of the knowledge base from the root, with `/`: `docs/kb`.
    pub kb: &'a str,
}

/// Whether `text` names a closed file: its path or its file name, in any
/// letter case.
pub fn names_closed_file(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    CLOSED_PATHS.iter().copied().find(|closed| {
        let closed = closed.to_lowercase();
        let name = closed.rsplit('/').next().unwrap_or(&closed);
        lower.contains(name)
    })
}

/// The text of the section under `heading` (up to the next heading).
fn section<'a>(body: &'a str, heading: &str) -> Option<&'a str> {
    let mut start = None;
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        match start {
            None if line.trim_end() == heading => start = Some(offset + line.len()),
            Some(s) if line.starts_with('#') => return Some(&body[s..offset]),
            _ => {}
        }
        offset += line.len();
    }
    start.map(|s| &body[s..])
}

/// Checks the notes of `kb`, read from `<root>/<kb path>`. The problems come
/// sorted by note and line.
pub fn lint(kb: &Kb, layout: &Layout) -> Result<Vec<Problem>> {
    let mut problems = Vec::new();
    let mut linked: HashSet<String> = HashSet::new();
    let known: HashSet<&str> = kb.notes.iter().map(|n| n.rel.as_str()).collect();

    for note in &kb.notes {
        let mut push = |line: Option<usize>, message: String| {
            problems.push(Problem {
                note: note.rel.clone(),
                line,
                message,
            });
        };
        let folder = note.rel.rsplit_once('/').map_or("", |(f, _)| f);
        let expected = NoteType::of_path(&note.rel);
        if expected.is_none() {
            push(
                None,
                "the folder is not a section of the knowledge base (see README.md)".into(),
            );
        }
        match &note.meta {
            Err(list) => {
                for message in list {
                    push(None, message.clone());
                }
            }
            Ok(meta) => {
                if let Some(expected) = expected
                    && expected != meta.note_type
                {
                    push(
                        None,
                        format!(
                            "type `{}`, but a note here must be `{}`",
                            meta.note_type.name(),
                            expected.name()
                        ),
                    );
                }
                let needs_sources = !matches!(meta.note_type, NoteType::Map | NoteType::Meta);
                if needs_sources && meta.sources.is_empty() {
                    push(
                        None,
                        "`sources` is empty: say where the facts come from".into(),
                    );
                }
                for source in &meta.sources {
                    let path = source.split_whitespace().next().unwrap_or("");
                    if !SOURCE_ROOTS.iter().any(|r| path.starts_with(r)) {
                        continue;
                    }
                    let inside = join_rel("", path).is_some_and(|p| layout.root.join(p).exists());
                    if !inside {
                        push(None, format!("source `{path}` does not exist"));
                    }
                }
                if meta.note_type == NoteType::Question && meta.status != Status::Known {
                    let next = section(note.body(), NEXT_STEP_HEADING);
                    if next.is_none_or(|s| s.trim().is_empty()) {
                        push(
                            None,
                            format!("an open question needs a `{NEXT_STEP_HEADING}` section"),
                        );
                    }
                }
            }
        }

        let headings = prose_lines(note.body())
            .iter()
            .filter(|(_, l)| l.starts_with("# "))
            .count();
        if headings != 1 {
            push(
                None,
                format!("{headings} `# ` headings; a note has exactly one"),
            );
        }
        if let Some(closed) = names_closed_file(&note.text) {
            push(
                None,
                format!(
                    "names the closed file `{closed}`: the knowledge base is public \
                     (public-export)"
                ),
            );
        }
        for (line, text) in prose_lines(&note.text) {
            if text.contains("[[") {
                push(
                    Some(line),
                    "wikilink `[[...]]`: use a relative Markdown link".into(),
                );
            }
        }
        for (line, target) in links(&note.text) {
            if target.contains("://") || target.starts_with("mailto:") {
                continue;
            }
            let path = target.split('#').next().unwrap_or("");
            if path.is_empty() {
                continue;
            }
            if path.starts_with('/') {
                push(Some(line), format!("link `{target}` is not relative"));
                continue;
            }
            let base = if folder.is_empty() {
                layout.kb.to_owned()
            } else {
                format!("{}/{folder}", layout.kb)
            };
            let Some(resolved) = join_rel(&base, path) else {
                push(Some(line), format!("link `{target}` leaves the repository"));
                continue;
            };
            if !layout.root.join(&resolved).exists() {
                push(Some(line), format!("broken link `{target}`"));
                continue;
            }
            if let Some(rel) = resolved
                .strip_prefix(layout.kb)
                .and_then(|r| r.strip_prefix('/'))
                && rel != note.rel
                && known.contains(rel)
            {
                linked.insert(rel.to_owned());
            }
        }
    }

    for note in &kb.notes {
        if note.rel != "index.md" && !linked.contains(&note.rel) {
            problems.push(Problem {
                note: note.rel.clone(),
                line: None,
                message: "orphan: no other note links here".into(),
            });
        }
    }
    problems.sort_by(|a, b| (&a.note, a.line).cmp(&(&b.note, b.line)));
    Ok(problems)
}

/// Reads the knowledge base at `<root>/<kb>` and checks it.
pub fn lint_dir(root: &Path, kb: &str) -> Result<Vec<Problem>> {
    let base = Kb::load(&root.join(kb))?;
    lint(&base, &Layout { root, kb })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn front(note_type: &str, status: &str) -> String {
        format!(
            "---\ntype: {note_type}\nstatus: {status}\nconfidence: data\n\
             editions: [\"1.0.789.0\"]\nstages: [72]\nsources: [\"docs/a.md §1\"]\n\
             tags: [x]\nupdated: 2026-11-16\n---\n"
        )
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn messages(root: &Path) -> Vec<String> {
        lint_dir(root, "docs/kb")
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn a_small_base_passes_and_each_rule_fires() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "docs/a.md", "# A\n");
        write(
            root,
            "docs/kb/index.md",
            &format!(
                "{}# Map\n\n[q](questions/q.md) [e](engine/bus.md)\n",
                front("map", "known")
            ),
        );
        write(
            root,
            "docs/kb/questions/q.md",
            &format!(
                "{}# Q\n\nSee [doc](../../a.md#part).\n\n{NEXT_STEP_HEADING}\n\nMeasure it.\n",
                front("question", "unknown")
            ),
        );
        write(
            root,
            "docs/kb/engine/bus.md",
            &format!("{}# Bus\n\n`[[not a link]]`\n", front("engine", "known")),
        );
        assert_eq!(messages(root), Vec::<String>::new());

        // A question without a next step, a broken link, a wikilink.
        write(
            root,
            "docs/kb/questions/q.md",
            &format!(
                "{}# Q\n\n[gone](../gone.md) [[wiki]]\n\n{NEXT_STEP_HEADING}\n\n## Other\n",
                front("question", "unknown")
            ),
        );
        // An orphan of the wrong type that names a closed file and a source
        // that does not exist.
        write(
            root,
            "docs/kb/exe/lost.md",
            &(front("engine", "known").replace("docs/a.md", "docs/none.md")
                + "# Lost\n\nplan.md\n"),
        );
        let all = messages(root).join("\n");
        for part in [
            "questions/q.md: an open question needs",
            "questions/q.md:13: broken link `../gone.md`",
            "questions/q.md:13: wikilink",
            "exe/lost.md: type `engine`, but a note here must be `exe`",
            "exe/lost.md: source `docs/none.md` does not exist",
            "exe/lost.md: names the closed file `PLAN.md`",
            "exe/lost.md: orphan",
        ] {
            assert!(all.contains(part), "{part}\n{all}");
        }
    }

    #[test]
    fn closed_files_are_found_by_name_in_any_case() {
        assert_eq!(names_closed_file("see Plan.md §10"), Some("PLAN.md"));
        assert_eq!(
            names_closed_file("../../analysis/visceral-plugin.md"),
            Some("docs/analysis/visceral-plugin.md")
        );
        assert_eq!(names_closed_file("the plan of the stage"), None);
    }
}
