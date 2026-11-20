//! The whole game as a project: every resource of every stream unpacked into
//! sources, then every entry built back into an empty workspace
//! (`docs/analysis/whole-game-project.md`).
//!
//! Heavy (tens of thousands of PNG and glTF files), so it runs only with
//! `DOCTOR_WHOLE_GAME=1` besides `EARS_GF2_DIR`. `DOCTOR_WHOLE_GAME_DIR=<dir>`
//! keeps the project there (an empty or new folder); `DOCTOR_WHOLE_GAME_REPORT=<file>`
//! writes the report to the file. The game directory is only read.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::AtomicBool;
use std::time::{Instant, SystemTime};

use doctor_toolkit::game::GameInstance;
use doctor_toolkit::navigation::{GameIndex, unpack};
use doctor_toolkit::project::{self, BuildOptions, CreateOptions, Kind};
use doctor_toolkit::workspace::Workspace;

#[derive(Default)]
struct Row {
    entries: usize,
    unchanged: usize,
    resources: usize,
    changed_resources: usize,
}

#[test]
fn the_whole_game_builds_back_from_a_project() {
    let Some(dir) = super::game_dir() else { return };
    if std::env::var_os("DOCTOR_WHOLE_GAME").is_none() {
        return;
    }
    let game = GameInstance::open(&dir).unwrap();
    let stamps: Vec<(u64, SystemTime)> = game
        .streams()
        .iter()
        .chain(game.tocs())
        .map(|f| {
            let m = std::fs::metadata(&f.path).unwrap();
            (m.len(), m.modified().unwrap())
        })
        .collect();

    let temp = tempfile::tempdir().unwrap();
    let base = std::env::var_os("DOCTOR_WHOLE_GAME_DIR")
        .map_or_else(|| temp.path().to_path_buf(), std::path::PathBuf::from);
    let root = base.join("project");
    let ws_dir = base.join("workspace");

    let mut out = String::new();
    let started = Instant::now();
    let index = GameIndex::build(&game, &|_, _| {});
    writeln!(out, "index: {:.0} s", started.elapsed().as_secs_f64()).unwrap();

    let mut ws = Workspace::for_game(&game, Some(&ws_dir)).unwrap();
    let selection = unpack::select(&game, &[], &[]).unwrap();
    let total: usize = selection.iter().map(|(_, l)| l.len()).sum();
    let started = Instant::now();
    let (p, report) = project::create(
        &game,
        &ws,
        Some(&index),
        &root,
        &CreateOptions {
            name: "The whole game".into(),
            id: None,
            author: String::new(),
            selection,
            settings: Default::default(),
        },
        &|_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    writeln!(
        out,
        "create: {} resource(s) of {total}, {} entr(ies), {} shared, {} file(s), {:.0} MiB, {:.0} s",
        report.resources,
        report.entries,
        report.shared,
        report.files,
        report.bytes as f64 / (1 << 20) as f64,
        started.elapsed().as_secs_f64()
    )
    .unwrap();
    writeln!(out, "conversions kept as raw data: {}", report.notes.len()).unwrap();
    let mut note_kinds: BTreeMap<String, usize> = BTreeMap::new();
    for note in &report.notes {
        let head: String = note
            .split(':')
            .next()
            .unwrap_or_default()
            .chars()
            .take(80)
            .collect();
        *note_kinds.entry(head).or_default() += 1;
    }
    for (head, n) in note_kinds.iter().take(40) {
        writeln!(out, "  {n} × {head}").unwrap();
    }

    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut entries: std::collections::HashMap<String, (Kind, String, usize)> =
        std::collections::HashMap::new();
    for (id, entry, _) in p.entries() {
        entries.insert(
            id.clone(),
            (entry.kind, entry.type_tag.clone(), entry.keys.len()),
        );
        *by_kind
            .entry(format!("{:?} {}", entry.kind, entry.type_tag))
            .or_default() += 1;
    }

    let started = Instant::now();
    let full = p
        .build(
            &game,
            &mut ws,
            &BuildOptions {
                force: true,
                ..BuildOptions::default()
            },
        )
        .unwrap();
    writeln!(
        out,
        "forced build: {} built, {} error(s), {:.0} s",
        full.built.len(),
        full.errors.len(),
        started.elapsed().as_secs_f64()
    )
    .unwrap();
    for e in full.errors.iter().take(30) {
        writeln!(out, "  error: {e:?}").unwrap();
    }

    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    let mut changed_samples: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for built in &full.built {
        let (kind, type_tag, keys) = &entries[&built.id];
        let key = format!("{kind:?} {type_tag}");
        let row = rows.entry(key.clone()).or_default();
        row.entries += 1;
        row.resources += keys;
        row.changed_resources += built.changed.len();
        if built.changed.is_empty() {
            row.unchanged += 1;
        } else if *kind != Kind::Texture && *kind != Kind::Model {
            let samples = changed_samples.entry(key).or_default();
            if samples.len() < 8 {
                samples.push(built.id.clone());
            }
        }
    }
    writeln!(
        out,
        "\nkind type | entries | built the same | resources | changed resources"
    )
    .unwrap();
    for (key, row) in &rows {
        writeln!(
            out,
            "{key} | {} | {} | {} | {}",
            row.entries, row.unchanged, row.resources, row.changed_resources
        )
        .unwrap();
    }
    let raw: usize = by_kind
        .iter()
        .filter(|(k, _)| k.starts_with("Raw "))
        .map(|(_, n)| n)
        .sum();
    writeln!(
        out,
        "\nentries: {} ({} raw)",
        by_kind.values().sum::<usize>(),
        raw
    )
    .unwrap();
    writeln!(out, "raw entries by type:").unwrap();
    for (k, n) in by_kind.iter().filter(|(k, _)| k.starts_with("Raw ")) {
        writeln!(out, "  {k}: {n}").unwrap();
    }
    writeln!(out, "\nlossless kinds that built different data, samples:").unwrap();
    for (k, samples) in &changed_samples {
        for s in samples {
            writeln!(out, "  {k}: {s}").unwrap();
        }
    }
    if let Some(path) = std::env::var_os("DOCTOR_WHOLE_GAME_REPORT") {
        std::fs::write(path, &out).unwrap();
    }
    println!("{out}");

    // The game files are untouched.
    let after: Vec<(u64, SystemTime)> = game
        .streams()
        .iter()
        .chain(game.tocs())
        .map(|f| {
            let m = std::fs::metadata(&f.path).unwrap();
            (m.len(), m.modified().unwrap())
        })
        .collect();
    assert_eq!(stamps, after, "the game files did not change");
    assert_eq!(report.resources, total, "every resource is in the project");
    assert!(
        report.notes.is_empty(),
        "no conversion fell back to raw data"
    );
    // Known on 2026-11-20 (docs/analysis/whole-game-project.md §3): ten
    // shared shadow meshes, three Lua scripts. Anything else is new.
    assert!(full.errors.len() <= 10, "{:#?}", full.errors);
    let lossless_changed: usize = changed_samples
        .iter()
        .filter(|(k, _)| k.as_str() != "Script lua")
        .map(|(_, v)| v.len())
        .sum();
    assert_eq!(lossless_changed, 0, "{changed_samples:#?}");
    assert!(
        rows.get("Script lua")
            .is_none_or(|r| r.entries - r.unchanged <= 3)
    );
}
