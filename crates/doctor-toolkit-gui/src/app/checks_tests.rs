//! Stage 103: the checks and the reference panels driven without a window
//! on the miniature game with a SimGroup of three placed objects in the
//! workspace: "Check edits" and the Findings panel, the check before
//! Apply, "Check references", the Hashes panel, the Knowledge base tab and
//! its data types, and their items of the shell preset.

use std::time::{Duration, Instant};

use doctor_toolkit::navigation::HashFn;
use doctor_toolkit::testing as mini;
use doctor_toolkit::workspace::ChangeState;

use super::details::tests::{Fixture, fixture, frame};
use super::dock::{DockOp, Pane};
use super::knowledge::{self, Block, Section, Span};
use super::{Command, shell};

fn frames(f: &mut Fixture, n: usize) {
    for _ in 0..n {
        frame(&f.ctx, &mut f.app);
    }
}

/// Waits for the check of references.
fn wait_refs(f: &mut Fixture) {
    let started = Instant::now();
    while f.app.findings.busy() {
        frame(&f.ctx, &mut f.app);
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the check ends"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn pending(f: &Fixture) -> usize {
    let game = f.app.game.as_ref().unwrap();
    let ws = f.app.workspace.as_ref().unwrap();
    ws.status(game).unwrap().count(ChangeState::Pending)
}

#[test]
fn check_edits_fills_the_findings_panel_and_holds_apply_back() {
    let mut f = fixture("check-edits");
    // The three placed objects: nothing wrong.
    f.app.pending.push(Command::CheckEdits);
    frames(&mut f, 2);
    assert_eq!(f.app.findings.errors(), 0, "{:?}", f.app.findings.rows);
    assert!(!f.app.findings.title.is_empty());
    assert!(f.app.findings.summary.contains('1'), "one change checked");
    let docked = super::dock::find(f.app.dock.as_ref().unwrap(), |p| *p == Pane::Findings);
    assert!(docked.is_some() || f.app.drawers.contains(&Pane::Findings));
    assert_eq!(f.app.preset_error(), None);
    // A SimGroup that cannot be read is an error with the key of the change.
    let game = f.app.game.clone().unwrap();
    f.app
        .workspace_mut()
        .unwrap()
        .set_resource(
            &game,
            f.group.stream,
            f.group.resource,
            vec![1, 2, 3],
            "broken",
        )
        .unwrap();
    assert!(!f.app.check_edits());
    assert_eq!(f.app.findings.errors(), 1, "{:?}", f.app.findings.rows);
    let row = &f.app.findings.rows[0];
    assert!(row.error && row.place.starts_with(mini::LEVEL), "{row:?}");
    assert!(row.what.contains("cannot be read"), "{row:?}");
    assert!(f.app.preset_error().is_some(), "a run without input fails");
    frames(&mut f, 2);
    // Apply is held back; "Apply anyway" of the panel is offered.
    let before = pending(&f);
    assert!(before > 0);
    f.app.apply_checked();
    assert!(f.app.findings.apply_held);
    assert_eq!(pending(&f), before, "nothing was written");
    frames(&mut f, 2);
    // The wizard that packages a mod tells the same error.
    let mut package = super::wizard::package_for_tests(&mut f.app);
    package.run_checks(&f.app);
    let checked = package.checked.clone().unwrap();
    assert!(
        checked.errors.iter().any(|e| e.contains("cannot be read")),
        "{checked:?}"
    );
}

#[test]
fn check_references_of_a_stream_and_a_folder() {
    let mut f = fixture("check-refs");
    assert!(f.app.check_refs_at(mini::LEVEL));
    assert!(f.app.findings.busy());
    wait_refs(&mut f);
    assert!(f.app.findings.title.contains(mini::LEVEL));
    assert!(!f.app.findings.summary.is_empty());
    let unresolved = f.app.findings.rows.len();
    // The folder of the stream: the same stream among its streams.
    assert!(f.app.check_refs_at("gf2"));
    wait_refs(&mut f);
    assert!(f.app.findings.rows.len() >= unresolved);
    assert_eq!(f.app.preset_error(), None);
    frames(&mut f, 2);
    // No such stream: an error, no check.
    assert!(!f.app.check_refs_at("nowhere/at/all"));
    assert!(!f.app.findings.busy());
    assert!(f.app.preset_error().unwrap().contains("nowhere/at/all"));
}

#[test]
fn the_hashes_panel_tells_hashes_and_names() {
    let mut f = fixture("hashes");
    f.app.pending.push(Command::Hashes);
    f.app.look_up_hash("brick_wall");
    frames(&mut f, 3);
    let result = &f.app.hashes.result;
    assert_eq!(result.of_name.len(), HashFn::ALL.len());
    assert!(result.hash.is_none(), "not a hash");
    let sdbm = doctor_toolkit::hash::sdbm(b"brick_wall");
    assert!(result.of_name.iter().any(|(_, h, _)| *h == sdbm));
    // A hash: eight digits, with 0x, decimal.
    for text in [
        format!("{sdbm:08x}"),
        format!("0x{sdbm:08x}"),
        sdbm.to_string(),
    ] {
        f.app.look_up_hash(&text);
        assert_eq!(f.app.hashes.result.hash, Some(sdbm), "{text}");
    }
    frames(&mut f, 2);
    // The type tag of a resource type is told.
    let lh2 = doctor_toolkit::text::TYPE_LH2;
    f.app.look_up_hash(&format!("0x{lh2:08x}"));
    assert!(f.app.hashes.result.type_tag.is_some());
    f.app.look_up_hash("");
    assert!(f.app.hashes.result.of_name.is_empty());
    frames(&mut f, 2);
}

#[test]
fn notes_are_split_into_blocks_and_spans() {
    let text = "# Title\n\nA `code` and **strong** [link](../maps/a.md) end.\nsecond line\n\n\
                - item one\n  wrapped\n- item [two](b.md)\n\n| A | B |\n|---|---|\n| `x` | y \\| z |\n\n\
                ```\nlet a = 1;\n```\n## Next\n";
    let blocks = knowledge::blocks(text);
    assert_eq!(blocks[0], Block::Heading(1, "Title".to_owned()));
    assert!(matches!(&blocks[1], Block::Paragraph(p) if p.ends_with("end. second line")));
    assert_eq!(blocks[2], Block::Item(0, "item one wrapped".to_owned()));
    assert_eq!(blocks[3], Block::Item(0, "item [two](b.md)".to_owned()));
    assert_eq!(
        blocks[4],
        Block::Table(vec![
            vec!["A".to_owned(), "B".to_owned()],
            vec!["`x`".to_owned(), "y | z".to_owned()],
        ])
    );
    assert_eq!(blocks[5], Block::Code("let a = 1;".to_owned()));
    assert_eq!(blocks[6], Block::Heading(2, "Next".to_owned()));
    let Block::Paragraph(p) = &blocks[1] else {
        panic!()
    };
    let spans = knowledge::spans(p);
    assert_eq!(spans[1], Span::Code("code".to_owned()));
    assert_eq!(spans[3], Span::Strong("strong".to_owned()));
    assert_eq!(
        spans[5],
        Span::Link {
            text: "link".to_owned(),
            target: "../maps/a.md".to_owned()
        }
    );
    assert_eq!(
        knowledge::resolve_link("classes/Gun.md", "../maps/a.md#x").as_deref(),
        Some("maps/a.md")
    );
    assert_eq!(
        knowledge::resolve_link("index.md", "maps/engine.md").as_deref(),
        Some("maps/engine.md")
    );
    assert_eq!(
        knowledge::resolve_link("index.md", "https://a.b/c.md"),
        None
    );
    assert_eq!(knowledge::resolve_link("index.md", "../formats/x.md"), None);
}

#[test]
fn the_knowledge_base_tab_searches_reads_and_follows_notes() {
    let mut f = fixture("knowledge");
    let base = knowledge::knowledge_base();
    assert!(base.notes.len() > 100, "the base is built in");
    assert!(base.note("index.md").is_some());
    f.app.pending.push(Command::KnowledgeBase);
    frames(&mut f, 3);
    // The search finds notes by words.
    f.app.knowledge.search("simgroup");
    assert!(f.app.knowledge.total > 0 && !f.app.knowledge.hits.is_empty());
    frames(&mut f, 2);
    // A note is opened; a link of it leads to another note, and back.
    assert!(f.app.open_note("index.md"));
    assert_eq!(f.app.knowledge.note.as_deref(), Some("index.md"));
    frames(&mut f, 2);
    let target = knowledge::resolve_link("index.md", "maps/engine.md").unwrap();
    assert!(f.app.open_note(&target));
    assert_eq!(f.app.knowledge.note.as_deref(), Some("maps/engine.md"));
    frames(&mut f, 2);
    // Every note of the base is drawn without a panic in its blocks.
    for note in &base.notes {
        let _ = knowledge::blocks(note.body());
    }
    // The notes of a class and of a resource type are found for Details.
    let class = base
        .notes
        .iter()
        .find_map(|n| n.rel.strip_prefix("classes/")?.strip_suffix(".md"))
        .filter(|name| *name != "index")
        .expect("a class note");
    assert!(knowledge::class_note(class).is_some());
    assert!(knowledge::class_note("NoSuchClassAtAll").is_none());
    assert_eq!(
        knowledge::resource_note(doctor_toolkit::text::TYPE_LH2).as_deref(),
        Some("resources/lh2.md")
    );
    // A panel asks for a note through the context.
    f.ctx.data_mut(|d| {
        d.insert_temp(
            egui::Id::unique("knowledge-open-note"),
            "resources/lh2.md".to_owned(),
        );
    });
    frames(&mut f, 2);
    assert_eq!(f.app.knowledge.note.as_deref(), Some("resources/lh2.md"));
    assert_eq!(f.app.preset_error(), None);
    // No such note: an error.
    assert!(!f.app.open_note("nothing/here.md"));
    assert!(f.app.preset_error().unwrap().contains("nothing/here.md"));
    // Data types: a list, messages, the attributes of a class.
    f.app.knowledge.section = Section::Types;
    f.app.knowledge.list = Some("family");
    f.app.knowledge.message = "start".to_owned();
    f.app.knowledge.class = "Gun".to_owned();
    frames(&mut f, 3);
}

#[test]
fn the_shell_preset_runs_the_checks_and_opens_the_reference() {
    let mut f = fixture("checks-preset");
    shell::apply_shell_preset(
        &mut f.app,
        "check,refs=gf2/level,hash=brick_wall,kb=simgroup+object,types=family",
    );
    assert_eq!(f.app.knowledge.section, Section::Types);
    assert_eq!(f.app.knowledge.list, Some("family"));
    assert!(!f.app.knowledge.hits.is_empty());
    assert_eq!(f.app.checks_preset.len(), 3);
    let started = Instant::now();
    while !f.app.checks_preset.is_empty() || f.app.findings.busy() {
        frame(&f.ctx, &mut f.app);
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the presets end"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    frames(&mut f, 2);
    assert!(f.app.findings.title.contains("gf2/level"), "the last check");
    assert_eq!(f.app.hashes.text, "brick_wall");
    assert_eq!(f.app.preset_error(), None);
    // What does not exist fails the run.
    shell::apply_shell_preset(&mut f.app, "types=nolist");
    assert!(f.app.preset_error().unwrap().contains("nolist"));
    let mut f = fixture("checks-preset-note");
    shell::apply_shell_preset(&mut f.app, "kb=classes/Missing.md");
    assert!(f.app.preset_error().unwrap().contains("classes/Missing.md"));
    assert!(!f.app.dock_ops.contains(&DockOp::Open(Pane::Knowledge)));
}
