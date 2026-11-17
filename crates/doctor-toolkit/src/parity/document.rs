//! `docs/parity.md`: the table of capabilities for people, written from
//! `data/toolkit/capabilities.toml` by `doctor-toolkit parity-gen`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::{Capability, Gate, Kind, Size, Status, Surface, TABLE_FILE, Table};
use crate::{Error, Result};

/// Path of the document from the repository root.
pub const DOCUMENT: &str = "docs/parity.md";

/// The English texts of the editor: labels of its commands (`[cmd]`).
const EDITOR_TEXTS: &str = "crates/doctor-toolkit-mods/i18n/en.toml";

/// Labels of the editor's commands by key (`cmd.open_game` → `Open game`).
pub type Labels = HashMap<String, String>;

/// Reads the labels of the editor's commands from its English texts.
pub fn labels(root: &Path) -> Result<Labels> {
    let path = root.join(EDITOR_TEXTS);
    let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
    let value: toml::Table = toml::from_str(&text)
        .map_err(|e| Error::InvalidArgument(format!("{EDITOR_TEXTS}: {e}")))?;
    let mut out = Labels::new();
    for section in ["cmd", "project"] {
        let Some(table) = value.get(section).and_then(|v| v.as_table()) else {
            continue;
        };
        for (key, label) in table {
            if let Some(label) = label.as_str() {
                out.insert(
                    format!("{section}.{key}"),
                    label.trim_end_matches('…').to_owned(),
                );
            }
        }
    }
    Ok(out)
}

fn size(size: Size) -> &'static str {
    match size {
        Size::Small => "малая",
        Size::Medium => "средняя",
        Size::Large => "большая",
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "да" } else { "нет" }
}

/// A cell of a Markdown table: no line breaks, no column separators.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

/// One editor name as a person finds it: the label of the menu command, or
/// the wizard by the label of its command.
fn editor_name(name: &str, labels: &Labels) -> String {
    if let Some(slug) = name.strip_prefix("wizard.") {
        return match labels.get(&format!("cmd.wizard_{slug}")) {
            Some(label) => format!("мастер «{label}»"),
            None => format!("мастер `{slug}`"),
        };
    }
    match labels.get(name) {
        Some(label) => format!("«{label}»"),
        None => format!("`{name}`"),
    }
}

/// What a surface has, without its status: names and where.
fn has(table: &Table, kind: Kind, s: &Surface, labels: &Labels) -> String {
    let mut parts: Vec<String> = Vec::new();
    for name in &s.names {
        parts.push(match kind {
            Kind::Editor => editor_name(name, labels),
            Kind::Mcp => match table.gate(name) {
                Gate::Read => format!("`{name}`"),
                Gate::Edits => format!("`{name}`ᴱ"),
                Gate::Game => format!("`{name}`ᴳ"),
            },
            Kind::Cli | Kind::Python => format!("`{name}`"),
        });
    }
    // A wizard and its menu command read the same: keep one.
    parts.dedup_by(|b, a| b.strip_prefix("мастер ") == Some(a.as_str()));
    let mut out = parts.join(", ");
    if !s.at.is_empty() {
        if !out.is_empty() {
            out.push_str("; ");
        }
        out.push_str(&s.at);
    }
    out
}

fn surface_cell(table: &Table, kind: Kind, s: &Surface, labels: &Labels) -> String {
    let mut out = match s.status() {
        Status::Absent => return "—".to_owned(),
        Status::Never => return format!("**намеренно нет**: {}", cell(&s.never)),
        Status::Full => has(table, kind, s, labels),
        Status::Partial => format!(
            "{} — **частично**: {}",
            has(table, kind, s, labels),
            s.partial
        ),
    };
    if kind == Kind::Editor {
        if s.mouse_only() {
            out.push_str(" — **только мышью**");
        }
        if !s.preset.is_empty() {
            let _ = write!(out, ". Без рук: `{}`", s.preset);
        }
    }
    cell(&out)
}

fn gap_line(cap: &Capability, kind: Kind) -> Option<String> {
    let gap = cap.surface(kind).gap.as_ref()?;
    let mut line = format!(
        "- **{}** (`{}`). Добавить: {}. Объём: {} — {}.",
        cap.title,
        cap.id,
        gap.add,
        size(gap.size),
        gap.why
    );
    if !gap.needs.is_empty() {
        let _ = write!(line, " Зависит от: {}.", gap.needs);
    }
    let _ = write!(line, " Стенд игры: {}.", yes_no(gap.bench));
    Some(line)
}

/// A list of capabilities with what the surface that has it gives and, for
/// the one that lacks it, what is missing.
fn list(
    out: &mut String,
    table: &Table,
    labels: &Labels,
    caps: &[&Capability],
    have: Kind,
    lack: Kind,
) {
    if caps.is_empty() {
        out.push_str("Нет.\n\n");
        return;
    }
    for cap in caps {
        let _ = write!(
            out,
            "- **{}** (`{}`): {}",
            cap.title,
            cap.id,
            has(table, have, cap.surface(have), labels)
        );
        let lacking = cap.surface(lack);
        if lacking.status() == Status::Partial {
            let _ = write!(
                out,
                ". В {} не хватает: {}",
                surface_in(lack),
                lacking.partial
            );
        }
        out.push_str(".\n");
    }
    out.push('\n');
}

fn surface_in(kind: Kind) -> &'static str {
    match kind {
        Kind::Mcp => "MCP",
        Kind::Editor => "редакторе",
        Kind::Cli => "командной строке",
        Kind::Python => "Python",
    }
}

/// The counts the document opens with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub capabilities: usize,
    /// Full and partial, by [`Kind::ALL`].
    pub full: [usize; 4],
    pub partial: [usize; 4],
    pub never: [usize; 4],
    pub mcp_not_editor: usize,
    pub editor_not_mcp: usize,
    pub editor_partial: usize,
    pub mcp_partial: usize,
    pub cli_only: usize,
    pub mouse_only: usize,
}

struct Lists<'a> {
    mcp_not_editor: Vec<&'a Capability>,
    editor_not_mcp: Vec<&'a Capability>,
    editor_partial: Vec<&'a Capability>,
    mcp_partial: Vec<&'a Capability>,
    cli_only: Vec<&'a Capability>,
    never: Vec<&'a Capability>,
    mouse_only: Vec<&'a Capability>,
}

fn lists(table: &Table) -> Lists<'_> {
    let caps = || table.caps.iter();
    Lists {
        mcp_not_editor: caps()
            .filter(|c| c.mcp.present() && c.editor.status() == Status::Absent)
            .collect(),
        editor_not_mcp: caps()
            .filter(|c| c.editor.present() && c.mcp.status() == Status::Absent)
            .collect(),
        editor_partial: caps()
            .filter(|c| c.editor.status() == Status::Partial && c.mcp.present())
            .collect(),
        mcp_partial: caps()
            .filter(|c| c.mcp.status() == Status::Partial && c.editor.present())
            .collect(),
        cli_only: caps()
            .filter(|c| {
                c.mcp.status() == Status::Absent
                    && c.editor.status() == Status::Absent
                    && c.cli.present()
            })
            .collect(),
        never: caps()
            .filter(|c| {
                Kind::ALL
                    .iter()
                    .any(|&k| c.surface(k).status() == Status::Never)
            })
            .collect(),
        mouse_only: caps().filter(|c| c.editor.mouse_only()).collect(),
    }
}

/// The counts of the table.
pub fn counts(table: &Table) -> Counts {
    let l = lists(table);
    let mut out = Counts {
        capabilities: table.caps.len(),
        mcp_not_editor: l.mcp_not_editor.len(),
        editor_not_mcp: l.editor_not_mcp.len(),
        editor_partial: l.editor_partial.len(),
        mcp_partial: l.mcp_partial.len(),
        cli_only: l.cli_only.len(),
        mouse_only: l.mouse_only.len(),
        ..Counts::default()
    };
    for cap in &table.caps {
        for (i, kind) in Kind::ALL.into_iter().enumerate() {
            match cap.surface(kind).status() {
                Status::Full => out.full[i] += 1,
                Status::Partial => out.partial[i] += 1,
                Status::Never => out.never[i] += 1,
                Status::Absent => {}
            }
        }
    }
    out
}

/// The text of the document.
pub fn document(table: &Table, labels: &Labels) -> String {
    let l = lists(table);
    let n = counts(table);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "<!-- Написано командой `doctor-toolkit parity-gen` из {TABLE_FILE}. Руками не править. -->"
    );
    out.push_str("# Паритет поверхностей: что где доступно\n\n");
    out.push_str(
        "У тулкита четыре поверхности: MCP-сервер для агентов, окно редактора, командная строка \
         и привязки Python. Все они зовут одну библиотеку и пишут в одно рабочее пространство; \
         расходится только то, что на каждой из них выведено наружу. Здесь — каждая возможность \
         тулкита и где она есть.\n\n",
    );
    let _ = writeln!(
        out,
        "Источник — таблица `{TABLE_FILE}`. Тесты сервера MCP, редактора и командной строки \
         падают, если инструмент, команда меню, мастер или команда CLI не упомянуты ни в одной \
         строке, если строка называет то, чего в коде нет, или у строки нет ни одной \
         поверхности. Новая возможность начинается со строки в таблице; этот документ \
         обновляет `doctor-toolkit parity-gen`.\n"
    );
    out.push_str("Как читать:\n\n");
    out.push_str(
        "- **MCP** — инструменты сервера ([MCP](mcp.md)). Знак ᴱ — инструмент есть только с \
         `--allow-edits`, ᴳ — только с `--allow-game`.\n",
    );
    out.push_str(
        "- **Редактор** — команда меню или палитры команд в кавычках (её английское название), \
         мастер, либо путь по панелям и кнопкам ([оболочка](shell.md), \
         [руководство](guide/README.md)). «Только мышью» — действие нельзя задать полями, \
         клавишей или предустановкой. «Без рук» — предустановка `DOCTOR_TOOLKIT_GUI_…` \
         (приставка опущена), которой действие делается без человека.\n",
    );
    out.push_str(
        "- **CLI** — команда `doctor-toolkit …`. **Python** — имя из модуля `doctor_toolkit` \
         ([Python](python.md)).\n",
    );
    out.push_str(
        "- «—» — поверхности у возможности нет. «Частично» — есть не всё, и сказано, чего не \
         хватает. «Намеренно нет» — не будет, и сказано почему.\n\n",
    );

    out.push_str("## Числа\n\n");
    let _ = writeln!(out, "Возможностей в таблице: **{}**.\n", n.capabilities);
    out.push_str(
        "| Поверхность | Полностью | Частично | Нет | Намеренно нет |\n|---|---|---|---|---|\n",
    );
    for (i, name) in ["MCP", "Редактор", "CLI", "Python"].into_iter().enumerate() {
        let _ = writeln!(
            out,
            "| {name} | {} | {} | {} | {} |",
            n.full[i],
            n.partial[i],
            n.capabilities - n.full[i] - n.partial[i] - n.never[i],
            n.never[i]
        );
    }
    out.push('\n');
    let _ = writeln!(
        out,
        "- Есть в MCP, нет в редакторе: **{}**.",
        n.mcp_not_editor
    );
    let _ = writeln!(
        out,
        "- Есть в редакторе, нет в MCP: **{}**.",
        n.editor_not_mcp
    );
    let _ = writeln!(
        out,
        "- Есть в обоих, но в редакторе частично: **{}**; в MCP частично: **{}**.",
        n.editor_partial, n.mcp_partial
    );
    let _ = writeln!(
        out,
        "- Нет ни в MCP, ни в редакторе — только командная строка: **{}**.",
        n.cli_only
    );
    let _ = writeln!(out, "- Строк с «намеренно нет»: **{}**.", l.never.len());
    let _ = writeln!(out, "- В редакторе только мышью: **{}**.\n", n.mouse_only);

    out.push_str("## Сводная таблица\n\n");
    for area in &table.areas {
        let _ = writeln!(out, "### {}\n", area.title);
        out.push_str("| Возможность | MCP | Редактор | CLI | Python |\n|---|---|---|---|---|\n");
        for cap in table.caps.iter().filter(|c| c.area == area.key) {
            let _ = write!(out, "| {} |", cell(&cap.title));
            for kind in Kind::ALL {
                let _ = write!(
                    out,
                    " {} |",
                    surface_cell(table, kind, cap.surface(kind), labels)
                );
            }
            out.push('\n');
        }
        out.push('\n');
    }

    out.push_str("## Есть в MCP, нет в редакторе\n\n");
    list(
        &mut out,
        table,
        labels,
        &l.mcp_not_editor,
        Kind::Mcp,
        Kind::Editor,
    );
    out.push_str("### В редакторе есть только часть\n\n");
    list(
        &mut out,
        table,
        labels,
        &l.editor_partial,
        Kind::Mcp,
        Kind::Editor,
    );

    out.push_str("## Есть в редакторе, нет в MCP\n\n");
    list(
        &mut out,
        table,
        labels,
        &l.editor_not_mcp,
        Kind::Editor,
        Kind::Mcp,
    );
    out.push_str("### В MCP есть только часть\n\n");
    list(
        &mut out,
        table,
        labels,
        &l.mcp_partial,
        Kind::Editor,
        Kind::Mcp,
    );

    out.push_str("## Есть только в командной строке\n\n");
    out.push_str(
        "Ни сервер MCP, ни редактор этого не дают; то, чего у них не будет, — в следующем разделе.\n\n",
    );
    list(&mut out, table, labels, &l.cli_only, Kind::Cli, Kind::Mcp);

    out.push_str("## Намеренно нет\n\n");
    for cap in &l.never {
        for kind in Kind::ALL {
            let s = cap.surface(kind);
            if s.status() == Status::Never {
                let _ = writeln!(
                    out,
                    "- **{}** (`{}`) — нет в {}: {}.",
                    cap.title,
                    cap.id,
                    surface_in(kind),
                    s.never
                );
            }
        }
    }
    out.push('\n');

    out.push_str("## В редакторе только мышью\n\n");
    out.push_str(
        "Эти действия нельзя задать ни полями, ни клавишей, ни предустановкой: агент, который \
         ведёт редактор без рук, их не сделает и может не заметить.\n\n",
    );
    for cap in &l.mouse_only {
        let _ = writeln!(
            out,
            "- **{}** (`{}`): {}.",
            cap.title,
            cap.id,
            has(table, Kind::Editor, &cap.editor, labels)
        );
    }
    out.push('\n');

    out.push_str("## Что редактор делает без рук\n\n");
    let with_preset: Vec<&Capability> = table
        .caps
        .iter()
        .filter(|c| !c.editor.preset.is_empty())
        .collect();
    let _ = writeln!(
        out,
        "Предустановки `DOCTOR_TOOLKIT_GUI_…` ([оболочка](shell.md)) есть у {} возможностей \
         редактора из {}; остальным нужен человек. Правки записывают только `LEVEL …;act=` и \
         `WIZARD=…,step=N`, прочие открывают и показывают.\n",
        with_preset.len(),
        table.caps.iter().filter(|c| c.editor.present()).count()
    );
    for cap in &with_preset {
        let _ = writeln!(
            out,
            "- **{}** (`{}`): `{}`.",
            cap.title, cap.id, cap.editor.preset
        );
    }
    out.push('\n');

    out.push_str("## Что нужно, чтобы закрыть дыры\n\n");
    out.push_str(
        "Оценка объёма: малая — до дня работы над готовой функцией библиотеки; средняя — \
         несколько дней, новая панель или перенос логики в библиотеку; большая — затрагивает \
         устройство библиотеки или редактора. Причины блокирующих находок и разбор общего слоя \
         операций — в [разборе паритета](analysis/parity.md).\n\n",
    );
    for (stage, kind, title) in [
        (103, Kind::Editor, "Редактор догоняет MCP (этап 103)"),
        (104, Kind::Mcp, "MCP догоняет редактор (этап 104)"),
    ] {
        let _ = writeln!(out, "### {title}\n");
        out.push_str("| Пакет | Что | Объём | Почему | Зависит от | Стенд | Дыр |\n|---|---|---|---|---|---|---|\n");
        let in_package = |key: &str| -> Vec<&Capability> {
            table
                .caps
                .iter()
                .filter(|c| {
                    c.surface(kind)
                        .gap
                        .as_ref()
                        .is_some_and(|g| g.package == key)
                })
                .collect()
        };
        for p in table.packages.iter().filter(|p| p.stage == stage) {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} |",
                p.key,
                cell(&p.title),
                size(p.size),
                cell(&p.why),
                if p.needs.is_empty() {
                    "—".to_owned()
                } else {
                    cell(&p.needs)
                },
                yes_no(p.bench),
                in_package(&p.key).len()
            );
        }
        out.push('\n');
        for p in table.packages.iter().filter(|p| p.stage == stage) {
            let caps = in_package(&p.key);
            if caps.is_empty() {
                continue;
            }
            let _ = writeln!(out, "**{} — {}**\n", p.key, p.title);
            for cap in caps {
                if let Some(line) = gap_line(cap, kind) {
                    out.push_str(&line);
                    out.push('\n');
                }
            }
            out.push('\n');
        }
    }
    // One trailing newline.
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Writes the document below `root`, or with `check` only compares. Returns
/// the path when the document differs (after writing it, without `check`).
pub fn write(root: &Path, check: bool) -> Result<Option<PathBuf>> {
    let text = document(super::table(), &labels(root)?);
    let path = root.join(DOCUMENT);
    if std::fs::read_to_string(&path).is_ok_and(|old| old == text) {
        return Ok(None);
    }
    if !check {
        std::fs::write(&path, &text).map_err(|e| Error::io(&path, e))?;
    }
    Ok(Some(path))
}
