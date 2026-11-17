//! The knowledge base of the repository (`docs/kb/`): every note passes
//! `kb-lint`, and the generated notes are what `kb-gen` makes of the data
//! now. Needs no game files.

use std::path::{Path, PathBuf};

use doctor_toolkit::kb::generate::{self, GENERATED_MARK, Inputs, MANUAL_BEGIN, MANUAL_END};
use doctor_toolkit::kb::{CLOSED_PATHS, Kb, NoteType, Status, lint};

const KB: &str = "docs/kb";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn the_knowledge_base_passes_the_lint() {
    let problems = lint::lint_dir(&root(), KB).unwrap();
    let text: Vec<String> = problems.iter().map(ToString::to_string).collect();
    assert!(
        problems.is_empty(),
        "{} problem(s) in {KB} (`doctor-toolkit kb-lint`):\n{}",
        problems.len(),
        text.join("\n")
    );
}

#[test]
fn the_generated_notes_are_up_to_date() {
    let out = generate::run(&root().join(KB), &Inputs::gf2(), "1970-01-01", true).unwrap();
    assert!(
        out.is_clean(),
        "run `doctor-toolkit kb-gen`: stale {:?}, extra {:?}",
        out.changed,
        out.removed
    );
}

#[test]
fn the_table_of_named_addresses_is_up_to_date() {
    let stale = doctor_toolkit::kb::symbols::run(&root(), &root().join(KB), true).unwrap();
    assert!(
        !stale,
        "run `doctor-toolkit kb-gen`: {} is out of date",
        doctor_toolkit::kb::symbols::SYMBOLS_FILE
    );
}

#[test]
fn every_section_has_notes_and_the_plan_is_there() {
    let kb = Kb::load(&root().join(KB)).unwrap();
    let count = |t: NoteType| {
        kb.notes
            .iter()
            .filter(|n| n.meta.as_ref().is_ok_and(|m| m.note_type == t))
            .count()
    };
    for (note_type, at_least) in [
        (NoteType::Map, 7),
        (NoteType::Engine, 5),
        (NoteType::System, 8),
        (NoteType::Class, 300),
        (NoteType::Resource, 45),
        (NoteType::Message, 2),
        (NoteType::Exe, 10),
        (NoteType::Question, 50),
        (NoteType::Experiment, 10),
        (NoteType::Research, 27),
    ] {
        assert!(
            count(note_type) >= at_least,
            "{}: {} note(s)",
            note_type.name(),
            count(note_type)
        );
    }
    // The research stages 73–101 are notes; a stage that is not done yet is
    // `planned`.
    for stage in 73..=101 {
        let rel = format!("research/stage-{stage}.md");
        let note = kb.note(&rel).unwrap_or_else(|| panic!("no {rel}"));
        let meta = note.meta.as_ref().unwrap();
        assert!(meta.stages.contains(&stage), "{rel}");
        assert!(
            matches!(
                meta.status,
                Status::Planned | Status::Known | Status::Partial
            ),
            "{rel}"
        );
    }
    // A generated note keeps one hand-written section.
    for note in &kb.notes {
        if note.text.contains(GENERATED_MARK) {
            assert_eq!(note.text.matches(MANUAL_BEGIN).count(), 1, "{}", note.rel);
            assert_eq!(note.text.matches(MANUAL_END).count(), 1, "{}", note.rel);
        }
    }
}

/// The rule of the public export, from the other side: the closed files are
/// the ones the list names, and the list is not empty.
#[test]
fn the_closed_files_are_listed_once() {
    assert!(!CLOSED_PATHS.is_empty());
    for closed in CLOSED_PATHS {
        assert!(lint::names_closed_file(&format!("see {closed}")).is_some());
        assert!(
            !closed.starts_with(KB),
            "{closed} is inside the knowledge base"
        );
    }
}
