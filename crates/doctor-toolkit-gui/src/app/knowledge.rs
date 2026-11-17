//! The Knowledge base tab (stage 103): the notes of `docs/kb/` embedded in
//! the editor as the MCP server embeds them — search by words
//! (`kb::search`), a note shown as formatted text with its links followed —
//! and the section "Data types": the value lists, the messages and the
//! meanings of the attributes of a class (`game_types`).

use std::sync::OnceLock;

use doctor_toolkit::game_types::{self, MessageCatalog, MessageId};
use doctor_toolkit::kb::search::{Query, search};
use doctor_toolkit::kb::{Kb, Note};
use doctor_toolkit::logic_graph::LogicSchema;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::RichText;

use super::dock::{DockOp, Pane};
use super::{BrowserApp, Level};

include!(concat!(env!("OUT_DIR"), "/kb.rs"));

/// The knowledge base built into the editor.
pub(crate) fn knowledge_base() -> &'static Kb {
    static BASE: OnceLock<Kb> = OnceLock::new();
    BASE.get_or_init(|| Kb::from_texts(KB.iter().copied()))
}

/// Notes found at once.
const HITS: usize = 60;

/// The note of a class of objects, when the base has one.
pub(crate) fn class_note(class: &str) -> Option<String> {
    let rel = format!("classes/{class}.md");
    knowledge_base().note(&rel).map(|_| rel)
}

/// The note of a resource type (its tag: `lh2`, `simgroup`), when there is one.
pub(crate) fn resource_note(type_id: u32) -> Option<String> {
    let tag = doctor_toolkit::resource::names::type_tags().get(type_id)?;
    let rel = format!("resources/{}.md", tag.to_ascii_lowercase());
    knowledge_base().note(&rel).map(|_| rel)
}

fn request_id() -> egui::Id {
    egui::Id::unique("knowledge-open-note")
}

/// A small link that opens note `rel` in the Knowledge base tab (panels
/// that cannot reach the app ask through the context).
pub(crate) fn note_link(ui: &mut egui::Ui, rel: &str) {
    if ui
        .link(RichText::new(tr("kb.note_link")).small())
        .on_hover_text(rel)
        .clicked()
    {
        ui.ctx()
            .data_mut(|d| d.insert_temp(request_id(), rel.to_owned()));
    }
}

/// What the tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Section {
    #[default]
    Notes,
    Types,
}

/// A found note.
pub(crate) struct Hit {
    pub rel: String,
    pub title: String,
    pub snippet: String,
}

/// The Knowledge base tab.
#[derive(Default)]
pub(crate) struct Knowledge {
    pub section: Section,
    pub query: String,
    searched: Option<String>,
    pub hits: Vec<Hit>,
    pub total: usize,
    /// The note shown, and the notes shown before it.
    pub note: Option<String>,
    back: Vec<String>,
    /// Data types: the list shown, the filter of its values.
    pub list: Option<&'static str>,
    pub list_filter: String,
    /// Data types: the text messages are searched by, the class.
    pub message: String,
    pub class: String,
}

impl Knowledge {
    /// Searches the notes by `text` (an empty text shows the map).
    pub fn search(&mut self, text: &str) {
        self.query = text.to_owned();
        self.searched = Some(text.to_owned());
        let query = Query::new(text);
        if query.words.is_empty() {
            self.hits.clear();
            self.total = 0;
            return;
        }
        let (hits, total) = search(knowledge_base(), &query, 0, HITS);
        self.total = total;
        self.hits = hits
            .into_iter()
            .map(|h| Hit {
                rel: h.note.rel.clone(),
                title: h.note.title.replace('`', ""),
                snippet: h.snippet,
            })
            .collect();
    }

    /// Shows note `rel`; `false` when the base has no such note.
    pub fn open(&mut self, rel: &str) -> bool {
        let rel = rel
            .trim_start_matches("docs/")
            .trim_start_matches("kb/")
            .to_owned();
        if knowledge_base().note(&rel).is_none() {
            return false;
        }
        if let Some(old) = self.note.replace(rel.clone())
            && old != rel
        {
            self.back.push(old);
        }
        self.section = Section::Notes;
        true
    }
}

/// `target` of a link in note `from`, as a path below the base; `None` for
/// links that leave it (other documents, the web).
pub(crate) fn resolve_link(from: &str, target: &str) -> Option<String> {
    let target = target.split('#').next().unwrap_or("");
    if target.is_empty() || target.contains("://") || !target.ends_with(".md") {
        return None;
    }
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for part in target.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    Some(parts.join("/"))
}

/// A piece of a line of a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Span {
    Text(String),
    Strong(String),
    Code(String),
    Link { text: String, target: String },
}

/// Splits a line into text, `code`, **strong** and [links](target).
pub(crate) fn spans(line: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = line;
    let flush = |text: &mut String, out: &mut Vec<Span>| {
        if !text.is_empty() {
            out.push(Span::Text(std::mem::take(text)));
        }
    };
    while let Some(c) = rest.chars().next() {
        if c == '`'
            && let Some(end) = rest[1..].find('`')
        {
            flush(&mut text, &mut out);
            out.push(Span::Code(rest[1..=end].to_owned()));
            rest = &rest[end + 2..];
            continue;
        }
        if let Some(inner) = rest.strip_prefix("**")
            && let Some(end) = inner.find("**")
        {
            flush(&mut text, &mut out);
            out.push(Span::Strong(inner[..end].replace('`', "")));
            rest = &inner[end + 2..];
            continue;
        }
        if c == '['
            && let Some(close) = rest.find("](")
            && let Some(end) = rest[close..].find(')')
            && !rest[1..close].contains('[')
        {
            flush(&mut text, &mut out);
            out.push(Span::Link {
                text: rest[1..close].replace('`', ""),
                target: rest[close + 2..close + end].to_owned(),
            });
            rest = &rest[close + end + 1..];
            continue;
        }
        text.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut text, &mut out);
    out
}

/// A block of a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Block {
    Heading(usize, String),
    Paragraph(String),
    /// A list item with its nesting depth.
    Item(usize, String),
    Code(String),
    /// The rows of a table: the first is the header.
    Table(Vec<Vec<String>>),
    Rule,
}

fn table_cells(line: &str) -> Vec<String> {
    let line = line.trim().trim_start_matches('|').trim_end_matches('|');
    // `\|` is a bar inside a cell.
    line.replace("\\|", "\u{1}")
        .split('|')
        .map(|c| c.trim().replace('\u{1}', "|"))
        .collect()
}

/// The blocks of the text of a note (Markdown as the notes use it).
pub(crate) fn blocks(body: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut paragraph = String::new();
    let mut code: Option<String> = None;
    let mut table: Vec<Vec<String>> = Vec::new();
    let end_paragraph = |paragraph: &mut String, out: &mut Vec<Block>| {
        if !paragraph.is_empty() {
            out.push(Block::Paragraph(std::mem::take(paragraph)));
        }
    };
    for line in body.lines() {
        if let Some(text) = &mut code {
            if line.trim_start().starts_with("```") {
                out.push(Block::Code(code.take().unwrap_or_default()));
            } else {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(line);
            }
            continue;
        }
        let trimmed = line.trim();
        if !trimmed.starts_with('|') && !table.is_empty() {
            out.push(Block::Table(std::mem::take(&mut table)));
        }
        if trimmed.starts_with("```") {
            end_paragraph(&mut paragraph, &mut out);
            code = Some(String::new());
        } else if trimmed.is_empty() || trimmed.starts_with("<!--") {
            end_paragraph(&mut paragraph, &mut out);
        } else if trimmed.starts_with('|') {
            end_paragraph(&mut paragraph, &mut out);
            let cells = table_cells(trimmed);
            // The line of dashes under the header.
            let ruler = cells
                .iter()
                .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')));
            if !ruler {
                table.push(cells);
            }
        } else if let Some(level) = trimmed
            .find(|c| c != '#')
            .filter(|&n| n > 0 && n <= 6 && trimmed[n..].starts_with(' '))
        {
            end_paragraph(&mut paragraph, &mut out);
            out.push(Block::Heading(level, trimmed[level..].trim().to_owned()));
        } else if trimmed == "---" || trimmed == "***" {
            end_paragraph(&mut paragraph, &mut out);
            out.push(Block::Rule);
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            end_paragraph(&mut paragraph, &mut out);
            let depth = (line.len() - line.trim_start().len()) / 2;
            out.push(Block::Item(depth, item.to_owned()));
        } else if trimmed
            .split_once(". ")
            .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        {
            end_paragraph(&mut paragraph, &mut out);
            let depth = (line.len() - line.trim_start().len()) / 2;
            out.push(Block::Item(depth, trimmed.to_owned()));
        } else if line.starts_with("  ")
            && paragraph.is_empty()
            && let Some(Block::Item(_, item)) = out.last_mut()
        {
            // A wrapped line of a list item.
            item.push(' ');
            item.push_str(trimmed);
        } else {
            if !paragraph.is_empty() {
                paragraph.push(' ');
            }
            paragraph.push_str(trimmed);
        }
    }
    if let Some(text) = code {
        out.push(Block::Code(text));
    }
    if !table.is_empty() {
        out.push(Block::Table(table));
    }
    end_paragraph(&mut paragraph, &mut out);
    out
}

/// Draws a line of a note; returns the target of the link clicked.
fn line_ui(ui: &mut egui::Ui, line: &str, size: Option<f32>) -> Option<String> {
    let mut clicked = None;
    let sized = |text: RichText| match size {
        Some(s) => text.size(s),
        None => text,
    };
    for span in spans(line) {
        match span {
            Span::Text(t) => {
                ui.label(sized(RichText::new(t)));
            }
            Span::Strong(t) => {
                ui.label(sized(RichText::new(t).strong()));
            }
            Span::Code(t) => {
                ui.label(sized(RichText::new(t).monospace()));
            }
            Span::Link { text, target } => {
                if ui
                    .link(sized(RichText::new(text)))
                    .on_hover_text(&target)
                    .clicked()
                {
                    clicked = Some(target);
                }
            }
        }
    }
    clicked
}

/// Draws the text of a note; returns the target of the link clicked.
fn note_ui(ui: &mut egui::Ui, note: &Note) -> Option<String> {
    let mut clicked = None;
    ui.spacing_mut().item_spacing.x = 0.0;
    for (n, block) in blocks(note.body()).into_iter().enumerate() {
        match block {
            Block::Heading(level, text) => {
                ui.add_space(if level == 1 { 2.0 } else { 8.0 });
                let size = match level {
                    1 => 22.0,
                    2 => 17.0,
                    _ => 14.5,
                };
                ui.horizontal_wrapped(|ui| {
                    for span in spans(&text) {
                        let text = match span {
                            Span::Text(t) | Span::Strong(t) | Span::Link { text: t, .. } => {
                                RichText::new(t)
                            }
                            Span::Code(t) => RichText::new(t).monospace(),
                        };
                        ui.label(text.size(size).strong());
                    }
                });
                ui.add_space(2.0);
            }
            Block::Paragraph(text) => {
                ui.horizontal_wrapped(|ui| clicked = line_ui(ui, &text, None).or(clicked.take()));
                ui.add_space(4.0);
            }
            Block::Item(depth, text) => {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(10.0 + depth as f32 * 14.0);
                    ui.label("•  ");
                    clicked = line_ui(ui, &text, None).or(clicked.take());
                });
            }
            Block::Code(text) => {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    egui::ScrollArea::horizontal()
                        .id_salt(("kb-code", n))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(RichText::new(text).monospace())
                                    .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        });
                });
                ui.add_space(4.0);
            }
            Block::Table(rows) => {
                let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
                let width = (ui.available_width() / columns.max(1) as f32 - 12.0).max(90.0);
                egui::ScrollArea::horizontal()
                    .id_salt(("kb-table", n))
                    .show(ui, |ui| {
                        egui::Grid::new(("kb-table-grid", n))
                            .num_columns(columns)
                            .striped(true)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                for (r, row) in rows.iter().enumerate() {
                                    for cell in row {
                                        ui.scope(|ui| {
                                            ui.set_max_width(width);
                                            ui.horizontal_wrapped(|ui| {
                                                if r == 0 {
                                                    ui.label(
                                                        RichText::new(cell.replace(['`', '*'], ""))
                                                            .strong(),
                                                    );
                                                } else {
                                                    clicked =
                                                        line_ui(ui, cell, None).or(clicked.take());
                                                }
                                            });
                                        });
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                ui.add_space(6.0);
            }
            Block::Rule => {
                ui.separator();
            }
        }
    }
    clicked
}

impl BrowserApp {
    /// Shows the Knowledge base tab.
    pub(crate) fn open_knowledge(&mut self) {
        self.dock_ops.push(DockOp::Open(Pane::Knowledge));
    }

    /// Shows note `rel` in the Knowledge base tab; an error when the base
    /// has no such note.
    pub(crate) fn open_note(&mut self, rel: &str) -> bool {
        if !self.knowledge.open(rel) {
            self.notify(Level::Error, trf("kb.no_note", &[("note", &rel)]));
            return false;
        }
        self.open_knowledge();
        true
    }

    /// Notes asked for by panels through the context ([`note_link`]).
    pub(crate) fn poll_knowledge(&mut self, ctx: &egui::Context) {
        let asked: Option<String> = ctx.data_mut(|d| d.remove_temp(request_id()));
        if let Some(rel) = asked {
            self.open_note(&rel);
        }
    }

    /// The Knowledge base tab.
    pub(super) fn knowledge_pane(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let k = &mut self.knowledge;
            ui.selectable_value(&mut k.section, Section::Notes, tr("kb.section_notes"));
            ui.selectable_value(&mut k.section, Section::Types, tr("kb.section_types"));
            ui.separator();
            ui.label(
                RichText::new(trn(
                    "kb.notes_count",
                    knowledge_base().notes.len() as u64,
                    &[],
                ))
                .weak(),
            );
        });
        ui.separator();
        match self.knowledge.section {
            Section::Notes => self.notes_section(ui),
            Section::Types => self.types_section(ui),
        }
    }

    fn notes_section(&mut self, ui: &mut egui::Ui) {
        let mut open: Option<String> = None;
        let mut external: Option<String> = None;
        egui::Panel::left("kb-search")
            .resizable(true)
            .default_size(300.0)
            .show(ui, |ui| {
                let k = &mut self.knowledge;
                ui.add(
                    egui::TextEdit::singleline(&mut k.query)
                        .desired_width(f32::INFINITY)
                        .hint_text(tr("kb.search_hint")),
                );
                if k.searched.as_deref() != Some(k.query.as_str()) {
                    let text = k.query.clone();
                    k.search(&text);
                }
                ui.horizontal(|ui| {
                    if ui.link(tr("kb.map")).clicked() {
                        open = Some("index.md".to_owned());
                    }
                    if !k.query.trim().is_empty() {
                        ui.label(
                            RichText::new(trf(
                                "kb.found",
                                &[("shown", &k.hits.len()), ("total", &k.total)],
                            ))
                            .weak(),
                        );
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("kb-hits")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        for hit in &k.hits {
                            let shown = k.note.as_deref() == Some(hit.rel.as_str());
                            if ui
                                .selectable_label(shown, RichText::new(&hit.title).strong())
                                .on_hover_text(&hit.rel)
                                .clicked()
                            {
                                open = Some(hit.rel.clone());
                            }
                            ui.add(
                                egui::Label::new(RichText::new(&hit.snippet).weak().small()).wrap(),
                            );
                            ui.add_space(4.0);
                        }
                    });
            });
        egui::CentralPanel::default().show(ui, |ui| {
            let k = &mut self.knowledge;
            let rel = k.note.clone().unwrap_or_else(|| "index.md".to_owned());
            let Some(note) = knowledge_base().note(&rel) else {
                ui.label(RichText::new(tr("kb.empty")).weak());
                return;
            };
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!k.back.is_empty(), egui::Button::new("<"))
                    .on_hover_text(tr("kb.back"))
                    .clicked()
                    && let Some(previous) = k.back.pop()
                {
                    k.note = Some(previous);
                }
                ui.label(RichText::new(&note.rel).monospace().weak());
                if let Ok(meta) = &note.meta {
                    ui.label(
                        RichText::new(format!(
                            "{} · {} · {}",
                            meta.note_type.name(),
                            meta.status.name(),
                            meta.confidence.name()
                        ))
                        .weak()
                        .small(),
                    );
                }
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt(("kb-note", &note.rel))
                .auto_shrink(false)
                .show(ui, |ui| {
                    if let Some(target) = note_ui(ui, note) {
                        match resolve_link(&note.rel, &target) {
                            Some(rel) if knowledge_base().note(&rel).is_some() => open = Some(rel),
                            _ => external = Some(target),
                        }
                    }
                });
        });
        if let Some(rel) = open {
            self.knowledge.open(&rel);
        }
        if let Some(target) = external {
            if target.contains("://") {
                if let Err(error) = super::shell::open_external(std::ffi::OsStr::new(&target)) {
                    self.notify(
                        Level::Error,
                        trf(
                            "status.cannot_open",
                            &[("what", &target), ("error", &error)],
                        ),
                    );
                }
            } else {
                self.notify(Level::Info, trf("kb.outside", &[("target", &target)]));
            }
        }
    }

    fn types_section(&mut self, ui: &mut egui::Ui) {
        let k = &mut self.knowledge;
        egui::Panel::left("kb-lists")
            .resizable(true)
            .default_size(260.0)
            .show(ui, |ui| {
                ui.strong(tr("kb.lists"));
                egui::ScrollArea::vertical()
                    .id_salt("kb-lists")
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        for e in game_types::enums() {
                            let label = format!("{} ({})", e.key, e.values.len());
                            if ui
                                .selectable_label(k.list == Some(e.key), label)
                                .on_hover_text(e.title)
                                .clicked()
                            {
                                k.list = Some(e.key);
                            }
                        }
                    });
            });
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt("kb-types")
                .auto_shrink(false)
                .show(ui, |ui| {
                    // Messages by a part of the name.
                    ui.horizontal(|ui| {
                        ui.strong(tr("kb.messages"));
                        ui.add(
                            egui::TextEdit::singleline(&mut k.message)
                                .desired_width(220.0)
                                .hint_text(tr("kb.message_hint")),
                        );
                    });
                    let text = k.message.trim();
                    if !text.is_empty() {
                        let id = MessageId::from_name(text);
                        ui.label(trf(
                            "kb.message_exact",
                            &[
                                ("name", &text),
                                ("hash", &format!("{:08x}", id.hash())),
                                ("kind", &id.kind()),
                            ],
                        ));
                        let found = MessageCatalog::gf2().search(text, 40);
                        egui::Grid::new("kb-messages")
                            .num_columns(4)
                            .striped(true)
                            .show(ui, |ui| {
                                for m in found {
                                    let count = |uses: Vec<game_types::PinUse>| {
                                        uses.iter().map(|u| u64::from(u.count)).sum::<u64>()
                                    };
                                    ui.label(RichText::new(m.name).monospace());
                                    ui.label(MessageId::from_hash(m.hash).kind());
                                    ui.label(trf(
                                        "kb.message_uses",
                                        &[
                                            ("senders", &count(m.senders())),
                                            ("listeners", &count(m.listeners())),
                                        ],
                                    ));
                                    ui.label(RichText::new(m.scope).weak());
                                    ui.end_row();
                                }
                            });
                    }
                    ui.separator();
                    // The attributes of a class.
                    ui.horizontal(|ui| {
                        ui.strong(tr("kb.class"));
                        ui.add(
                            egui::TextEdit::singleline(&mut k.class)
                                .desired_width(220.0)
                                .hint_text(tr("kb.class_hint")),
                        );
                    });
                    let class = k.class.trim();
                    if !class.is_empty() {
                        let schema = LogicSchema::gf2();
                        let hash = game_types::class_hash(class);
                        let attrs = schema.typed_attrs(hash);
                        ui.horizontal(|ui| {
                            ui.label(format!(
                                "{} ({hash:08x})",
                                schema.class_name(hash).unwrap_or(class)
                            ));
                            if let Some(rel) = class_note(schema.class_name(hash).unwrap_or(class))
                            {
                                note_link(ui, &rel);
                            }
                        });
                        if attrs.is_empty() {
                            ui.label(RichText::new(tr("kb.no_attrs")).weak());
                        }
                        egui::Grid::new("kb-attrs")
                            .num_columns(4)
                            .striped(true)
                            .show(ui, |ui| {
                                for a in &attrs {
                                    ui.label(a.index.to_string());
                                    ui.label(a.name.as_deref().unwrap_or("—"));
                                    ui.label(RichText::new(a.ty.to_string()).monospace());
                                    ui.label(
                                        RichText::new(a.source.unwrap_or_default()).weak().small(),
                                    );
                                    ui.end_row();
                                }
                            });
                    }
                    ui.separator();
                    // The values of the chosen list.
                    let Some(list) = k.list.and_then(game_types::enum_def) else {
                        ui.label(RichText::new(tr("kb.choose_list")).weak());
                        return;
                    };
                    ui.horizontal(|ui| {
                        ui.strong(format!("{} — {}", list.key, list.title));
                        ui.label(RichText::new(list.kind.name()).weak());
                    });
                    ui.add(egui::Label::new(RichText::new(list.source).weak().small()).wrap());
                    ui.add(
                        egui::TextEdit::singleline(&mut k.list_filter)
                            .desired_width(220.0)
                            .hint_text(tr("kb.filter_hint")),
                    );
                    let filter = k.list_filter.to_lowercase();
                    egui::Grid::new(("kb-values", list.key))
                        .num_columns(4)
                        .striped(true)
                        .show(ui, |ui| {
                            for v in list.values {
                                let label = v.label();
                                if !filter.is_empty()
                                    && !label.to_lowercase().contains(&filter)
                                    && !v.note.to_lowercase().contains(&filter)
                                {
                                    continue;
                                }
                                ui.label(RichText::new(label).monospace());
                                ui.label(v.value.map(|n| n.to_string()).unwrap_or_default());
                                ui.label(
                                    RichText::new(
                                        v.hash.map(|h| format!("{h:08x}")).unwrap_or_default(),
                                    )
                                    .monospace()
                                    .weak(),
                                );
                                ui.add(egui::Label::new(RichText::new(v.note).weak()).wrap());
                                ui.end_row();
                            }
                        });
                });
        });
    }
}
