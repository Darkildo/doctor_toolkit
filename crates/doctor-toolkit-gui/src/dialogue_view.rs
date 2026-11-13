//! Dialogue editor (`dlg`): the tree of lines and choices with speakers,
//! texts, speech and subtitles; edits go to the workspace (undo with Ctrl+Z).
//!
//! The game-wide lookups (chatter sets, speech banks, archive listings,
//! SimGroup users, string tables) are read once per game into a
//! [`Catalog`] shared by every dialogue view.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use doctor_toolkit::dialogue::{self, Catalog, DialogueRef, Hit, SpeechLine};
use doctor_toolkit::game::GameInstance;
use doctor_toolkit::resource::chv::event_hash;
use doctor_toolkit::resource::dlg::{
    self, CHOICE_SLOTS, Dialogue, KIND_ASSIGN, KIND_CHOICE, KIND_END, KIND_LINE, KIND_SET_FLAG,
    KIND_SET_FLAG_B, LINE_ANIM_LEAD, LINE_ANIM_LISTENER, LINE_ANIM_SPEAKER, LINE_EVENT,
    LINE_SPEAKER, NONE, TYPE_DLG, Test,
};
use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit::resource::speech;
use doctor_toolkit::workspace::{FileKey, Workspace};
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{Color32, RichText};

use crate::views::{ResourceViewer, ViewAction, ViewContext, ViewEntry, ViewSource};

pub const VIEW: ViewEntry = ViewEntry {
    id: "dialogue",
    load,
};

/// Candidates whose full subtitle table is read for a line.
const READ_CANDIDATES: usize = 3;
/// Candidates listed for a line.
const SHOW_CANDIDATES: usize = 8;

/// The catalog of the open game, built on first use.
static CATALOG: Mutex<Option<(PathBuf, Arc<Catalog>)>> = Mutex::new(None);

fn catalog(game: &GameInstance) -> Result<Arc<Catalog>, String> {
    let mut slot = CATALOG
        .lock()
        .map_err(|_| "catalog lock poisoned".to_owned())?;
    if let Some((root, c)) = slot.as_ref()
        && root == game.root()
    {
        return Ok(c.clone());
    }
    let c = Arc::new(Catalog::build(game, &|_, _| {}).map_err(|e| e.to_string())?);
    *slot = Some((game.root().to_owned(), c.clone()));
    Ok(c)
}

/// One row of the tree outline.
struct Row {
    depth: usize,
    node: usize,
    /// How the parent reaches it (`true`, option text, ...).
    branch: String,
    /// Already shown above: a link, not expanded again.
    link: bool,
}

/// What the view asks for after a click.
enum Edit {
    /// Record the dialogue as edited.
    Save(String),
    Apply(String, crate::views::WorkspaceEdit),
    Open(usize, usize),
}

pub struct DialogueView {
    catalog: Arc<Catalog>,
    dref: DialogueRef,
    dialogue: Dialogue,
    ws_dir: Option<PathBuf>,
    game_root: PathBuf,
    /// `(code, one-language table)` as the workspace has them.
    languages: Vec<(String, StringTable)>,
    lang: usize,
    /// Streams of the SimGroups that start the dialogue.
    near: Vec<usize>,
    /// Subtitle tables of the best candidates, as the workspace has them.
    subtitles: HashMap<String, StringTable>,
    /// Archive entries the workspace changes.
    changed_files: HashSet<String>,
    rows: Vec<Row>,
    selected: usize,
    // Edit buffers of the selected node.
    buffer_for: Option<usize>,
    event_name: String,
    labels: Vec<String>,
    texts: Vec<String>,
    subtitle_edits: HashMap<String, String>,
    search: String,
    hits: Vec<Hit>,
}

fn load(source: &mut ViewSource<'_>) -> Option<Box<dyn ResourceViewer>> {
    if source.type_id != TYPE_DLG {
        return None;
    }
    let data = source.data(source.index).ok()?;
    let dialogue = Dialogue::parse(&data).ok()?;
    let catalog = match catalog(source.game) {
        Ok(c) => c,
        Err(error) => {
            eprintln!("dialogue catalog: {error}");
            return None;
        }
    };
    let dref = catalog
        .dialogues
        .iter()
        .find(|d| d.stream == source.stream_index && d.resource == source.index)
        .cloned()
        .unwrap_or(DialogueRef {
            stream: source.stream_index,
            resource: source.index,
            name: source.game.streams()[source.stream_index].stream_name(),
        });
    let ws = source.workspace;
    let languages = dialogue::language_strings(source.game, ws, &catalog.languages);
    let near = catalog.user_streams(&dref);
    let changed_files: HashSet<String> = ws
        .map(|w| {
            w.file_changes()
                .filter_map(|c| match &c.key {
                    FileKey::Archive(name) => Some(name.clone()),
                    FileKey::Loose(_) => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let mut subtitles = HashMap::new();
    for (i, node) in dialogue.nodes.iter().enumerate() {
        if node.kind != KIND_LINE {
            continue;
        }
        let event = node.word(LINE_EVENT).unwrap_or(0);
        let speaker = node.word(LINE_SPEAKER).unwrap_or(0);
        let _ = i;
        for (k, line) in catalog
            .candidates(event, speaker, &near)
            .into_iter()
            .enumerate()
        {
            let Some(entry) = line.entries.first() else {
                continue;
            };
            if (k < READ_CANDIDATES || changed_files.contains(entry))
                && !subtitles.contains_key(entry)
                && let Ok(Some(table)) = catalog.subtitles_of(source.game, ws, entry)
            {
                subtitles.insert(entry.clone(), table);
            }
        }
    }
    let mut view = DialogueView {
        catalog,
        dref,
        dialogue,
        ws_dir: ws.map(|w| w.dir().to_owned()),
        game_root: source.game.root().to_owned(),
        languages,
        lang: 0,
        near,
        subtitles,
        changed_files,
        rows: Vec::new(),
        selected: 0,
        buffer_for: None,
        event_name: String::new(),
        labels: Vec::new(),
        texts: Vec::new(),
        subtitle_edits: HashMap::new(),
        search: String::new(),
        hits: Vec::new(),
    };
    view.rows = view.outline();
    view.preset();
    Some(Box::new(view))
}

/// State kept across reloads.
struct Kept {
    selected: usize,
    lang: usize,
    search: String,
    hits: Vec<Hit>,
}

impl DialogueView {
    /// `DOCTOR_TOOLKIT_GUI_DIALOGUE=select=<node>,lang=<code>,search=<text>`
    /// presets the view for screenshots.
    fn preset(&mut self) {
        let Ok(spec) = std::env::var("DOCTOR_TOOLKIT_GUI_DIALOGUE") else {
            return;
        };
        for part in spec.split(',') {
            match part.split_once('=') {
                Some(("select", n)) => {
                    if let Ok(n) = n.parse::<usize>() {
                        self.selected = n.min(self.dialogue.nodes.len().saturating_sub(1));
                    }
                }
                Some(("lang", code)) => {
                    if let Some(k) = self.languages.iter().position(|(c, _)| c == code) {
                        self.lang = k;
                    }
                }
                Some(("search", q)) => {
                    self.search = q.to_owned();
                    self.run_search();
                }
                _ => {}
            }
        }
    }

    fn strings(&self) -> Option<&StringTable> {
        self.languages.get(self.lang).map(|(_, t)| t)
    }

    fn lang_code(&self) -> &str {
        self.languages
            .get(self.lang)
            .map_or("en", |(c, _)| c.as_str())
    }

    fn text_of(&self, name: &str) -> Option<String> {
        dialogue::lookup(self.strings()?, name)
    }

    fn candidates(&self, node: usize) -> Vec<&SpeechLine> {
        let n = &self.dialogue.nodes[node];
        self.catalog.candidates(
            n.word(LINE_EVENT).unwrap_or(0),
            n.word(LINE_SPEAKER).unwrap_or(0),
            &self.near,
        )
    }

    /// Subtitle of a speech file in the chosen language: the workspace
    /// version when read, else the game's English one.
    fn subtitle(&self, entry: &str) -> Option<String> {
        if let Some(table) = self.subtitles.get(entry) {
            let col = speech::language_column(table, self.lang_code())
                .or_else(|| speech::language_column(table, "en"))?;
            return table.strings.get(col)?.first().cloned();
        }
        self.catalog.subtitles.get(entry).cloned()
    }

    /// Short text of a node for the outline.
    fn caption(&self, i: usize) -> String {
        let d = &self.dialogue;
        let node = &d.nodes[i];
        match node.kind {
            KIND_LINE => {
                let event = self.catalog.event_name(node.word(LINE_EVENT).unwrap_or(0));
                let speaker = node.word(LINE_SPEAKER).unwrap_or(0);
                let who = if speaker == 1 {
                    tr("dialogue.player")
                } else {
                    tr("dialogue.npc")
                };
                let text = self
                    .candidates(i)
                    .into_iter()
                    .filter_map(|l| self.subtitle(l.entries.first()?))
                    .next()
                    .map(|t| format!(" “{t}”"))
                    .unwrap_or_default();
                format!("{who}: {event}{text}")
            }
            KIND_CHOICE => {
                let timeout = f32::from_bits(node.word(0).unwrap_or(0));
                if timeout > 0.0 {
                    trf("dialogue.choice_timeout", &[("s", &timeout)])
                } else {
                    tr("dialogue.choice").to_owned()
                }
            }
            _ => dlg::summary(d, i),
        }
    }

    fn branch_label(&self, parent: usize, slot: usize) -> String {
        let node = &self.dialogue.nodes[parent];
        match node.kind {
            KIND_CHOICE => {
                let label = self.dialogue.option_label(parent, slot);
                match label {
                    Some(l) => self.text_of(l).unwrap_or_else(|| l.to_owned()),
                    None => trf("dialogue.option_n", &[("n", &(slot + 1))]),
                }
            }
            _ if node.children.len() == 2 && Test::of(node).is_some() => {
                if slot == 1 { "true" } else { "false" }.to_owned()
            }
            _ if node.children.len() > 1 => format!("{slot}"),
            _ => String::new(),
        }
    }

    /// The tree from node 0, depth first; nodes seen before become links,
    /// nodes nothing reaches are listed at the end.
    fn outline(&self) -> Vec<Row> {
        let n = self.dialogue.nodes.len();
        let mut rows = Vec::new();
        let mut seen = vec![false; n];
        let mut stack = vec![(0usize, 0usize, String::new())];
        let mut roots: Vec<usize> = Vec::new();
        loop {
            while let Some((node, depth, branch)) = stack.pop() {
                if node >= n {
                    continue;
                }
                if seen[node] {
                    rows.push(Row {
                        depth,
                        node,
                        branch,
                        link: true,
                    });
                    continue;
                }
                seen[node] = true;
                rows.push(Row {
                    depth,
                    node,
                    branch,
                    link: false,
                });
                let kids = &self.dialogue.nodes[node].children;
                // A single branch continues at the same depth: lines read
                // like a script.
                let next_depth = if kids.len() > 1 { depth + 1 } else { depth };
                for (k, child) in kids.iter().enumerate().rev() {
                    if let Some(c) = child {
                        stack.push((*c, next_depth, self.branch_label(node, k)));
                    }
                }
            }
            match (0..n).find(|&i| !seen[i] && !roots.contains(&i)) {
                Some(i) => {
                    roots.push(i);
                    stack.push((i, 0, tr("dialogue.unreachable").to_owned()));
                }
                None => break,
            }
        }
        rows
    }

    fn fill_buffers(&mut self) {
        if self.buffer_for == Some(self.selected) {
            return;
        }
        self.buffer_for = Some(self.selected);
        let i = self.selected;
        let node = &self.dialogue.nodes[i];
        self.event_name = node
            .word(LINE_EVENT)
            .filter(|_| node.kind == KIND_LINE)
            .map(|e| self.catalog.event_name(e))
            .unwrap_or_default();
        self.labels = (0..CHOICE_SLOTS)
            .map(|o| self.dialogue.option_label(i, o).unwrap_or("").to_owned())
            .collect();
        self.texts = self
            .labels
            .iter()
            .map(|l| self.text_of(l).unwrap_or_default())
            .collect();
        self.subtitle_edits.clear();
    }

    fn save(&self, label: String) -> Option<ViewAction> {
        match self.dialogue.to_bytes() {
            Ok(data) => Some(ViewAction::SaveData { data, label }),
            Err(_) => None,
        }
    }

    fn show_header(&mut self, ui: &mut egui::Ui, edit: &mut Option<Edit>) {
        ui.horizontal_wrapped(|ui| {
            ui.heading(&self.dref.name);
            ui.label(trf(
                "dialogue.counts",
                &[
                    (
                        "nodes",
                        &trn("count.nodes", self.dialogue.nodes.len() as u64, &[]),
                    ),
                    (
                        "lines",
                        &trn("dialogue.lines", self.dialogue.count(KIND_LINE) as u64, &[]),
                    ),
                    (
                        "choices",
                        &trn(
                            "count.choices",
                            self.dialogue.count(KIND_CHOICE) as u64,
                            &[],
                        ),
                    ),
                ],
            ));
            if self.languages.len() > 1 {
                let codes: Vec<String> = self.languages.iter().map(|(c, _)| c.clone()).collect();
                egui::ComboBox::from_id_salt("lang")
                    .selected_text(codes[self.lang.min(codes.len() - 1)].clone())
                    .show_ui(ui, |ui| {
                        for (k, c) in codes.iter().enumerate() {
                            ui.selectable_value(&mut self.lang, k, c);
                        }
                    })
                    .response
                    .on_hover_text(tr("dialogue.language_hint"));
            }
        });
        let usages = self.catalog.usages_of(&self.dref);
        egui::CollapsingHeader::new(trn("dialogue.used_by_count", usages.len() as u64, &[]))
            .id_salt("used")
            .show(ui, |ui| {
                if usages.is_empty() {
                    ui.weak(tr("dialogue.used_by_none"));
                }
                for u in usages.iter().take(200) {
                    ui.horizontal(|ui| {
                        if ui.small_button(tr("dialogue.open")).clicked() {
                            *edit = Some(Edit::Open(u.stream, u.resource));
                        }
                        ui.add(
                            egui::Label::new(format!("{} {} ({})", u.class, u.object, u.attribute))
                                .truncate(),
                        );
                    });
                }
            });
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text(tr("dialogue.search_hint"))
                    .desired_width(260.0),
            );
            let go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button(tr("dialogue.search")).clicked() || go {
                self.run_search();
            }
            if !self.hits.is_empty() && ui.small_button(tr("dialogue.clear")).clicked() {
                self.hits.clear();
            }
        });
        if !self.hits.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("hits")
                .max_height(140.0)
                .show(ui, |ui| {
                    for hit in &self.hits {
                        let d = &self.catalog.dialogues[hit.dialogue];
                        ui.horizontal(|ui| {
                            let here = d.stream == self.dref.stream;
                            if here {
                                if let Some(n) = hit.node
                                    && ui.small_button(format!("#{n}")).clicked()
                                {
                                    self.selected = n;
                                }
                            } else if ui.small_button(tr("dialogue.open")).clicked() {
                                *edit = Some(Edit::Open(d.stream, d.resource));
                            }
                            let name = d.name.rsplit('/').next().unwrap_or(&d.name);
                            ui.add(
                                egui::Label::new(format!(
                                    "{name}{}: {}",
                                    hit.node.map(|n| format!(" #{n}")).unwrap_or_default(),
                                    hit.what
                                ))
                                .truncate(),
                            );
                        });
                    }
                });
        }
    }

    fn run_search(&mut self) {
        let Ok(game) = GameInstance::open(&self.game_root) else {
            return;
        };
        let ws = self
            .ws_dir
            .as_deref()
            .filter(|d| Workspace::exists(d))
            .and_then(|d| Workspace::open(d).ok());
        self.hits = dialogue::search(
            &self.catalog,
            &game,
            ws.as_ref(),
            self.strings(),
            self.search.trim(),
        );
    }

    fn show_tree(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("tree")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for r in 0..self.rows.len() {
                    let row = &self.rows[r];
                    let (node, depth, link) = (row.node, row.depth, row.link);
                    let mut text = String::new();
                    if !row.branch.is_empty() {
                        text.push_str(&format!("[{}] ", row.branch));
                    }
                    let kind = self.dialogue.nodes[node].kind;
                    if link {
                        text.push_str(&format!("-> #{node}"));
                    } else {
                        text.push_str(&format!("#{node} {}", self.caption(node)));
                    }
                    let color = match kind {
                        KIND_LINE => Color32::from_rgb(120, 180, 255),
                        KIND_CHOICE => Color32::from_rgb(255, 190, 90),
                        KIND_END => Color32::GRAY,
                        _ => Color32::from_rgb(150, 210, 150),
                    };
                    ui.horizontal(|ui| {
                        ui.add_space(14.0 * depth as f32);
                        let label =
                            RichText::new(text).color(if link { Color32::GRAY } else { color });
                        let button =
                            egui::Button::selectable(self.selected == node && !link, label)
                                .wrap_mode(egui::TextWrapMode::Truncate);
                        if ui.add(button).clicked() {
                            self.selected = node;
                        }
                    });
                }
            });
    }

    fn show_details(&mut self, ui: &mut egui::Ui, edit: &mut Option<Edit>) {
        self.fill_buffers();
        let i = self.selected;
        let kind = self.dialogue.nodes[i].kind;
        ui.heading(format!("#{i} {}", dlg::kind_name(kind)));
        let parents = self.dialogue.parents(i);
        if !parents.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.weak(tr("dialogue.reached_from"));
                for (p, _) in parents {
                    if ui.small_button(format!("#{p}")).clicked() {
                        self.selected = p;
                    }
                }
            });
        }
        ui.separator();
        match kind {
            KIND_LINE => self.line_details(ui, edit),
            KIND_CHOICE => self.choice_details(ui, edit),
            KIND_SET_FLAG | KIND_SET_FLAG_B => {
                let mut v = self.dialogue.nodes[i].word(0).unwrap_or(0);
                ui.horizontal(|ui| {
                    ui.label(tr("dialogue.flag_value"));
                    if ui.add(egui::DragValue::new(&mut v)).changed() {
                        if let Some(p) = self.dialogue.nodes[i].payload.as_mut() {
                            p[0] = v;
                        }
                        *edit = Some(Edit::Save(trf("dialogue.op_flag", &[("node", &i)])));
                    }
                });
            }
            KIND_END => {
                ui.label(tr("dialogue.end_hint"));
            }
            _ => self.test_details(ui, edit),
        }
        ui.separator();
        self.branch_editor(ui, edit);
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            let can_insert = !self.dialogue.nodes[i].children.is_empty();
            let template = if kind == KIND_LINE {
                Some(i)
            } else {
                self.dialogue.nodes.iter().position(|n| n.kind == KIND_LINE)
            };
            if ui
                .add_enabled(
                    can_insert && template.is_some(),
                    egui::Button::new(tr("dialogue.insert_line")),
                )
                .on_hover_text(tr("dialogue.insert_line_hint"))
                .clicked()
                && let Some(t) = template
            {
                let event = self.dialogue.nodes[t].word(LINE_EVENT).unwrap_or(0);
                if let Ok(new) = self.dialogue.insert_line(i, 0, t, event) {
                    self.selected = new;
                    *edit = Some(Edit::Save(trf(
                        "dialogue.op_insert",
                        &[("new", &new), ("node", &i)],
                    )));
                }
            }
            let removable = i != 0 && !matches!(kind, KIND_CHOICE | KIND_END);
            if ui
                .add_enabled(removable, egui::Button::new(tr("dialogue.remove_node")))
                .on_hover_text(tr("dialogue.remove_node_hint"))
                .clicked()
                && self.dialogue.remove_node(i).is_ok()
            {
                self.selected = 0;
                *edit = Some(Edit::Save(trf("dialogue.op_remove", &[("node", &i)])));
            }
        });
    }

    fn line_details(&mut self, ui: &mut egui::Ui, edit: &mut Option<Edit>) {
        let i = self.selected;
        let node = self.dialogue.nodes[i].clone();
        let mut speaker = node.word(LINE_SPEAKER).unwrap_or(2);
        ui.horizontal(|ui| {
            ui.label(tr("dialogue.speaker"));
            let before = speaker;
            ui.radio_value(&mut speaker, 1, tr("dialogue.player"));
            ui.radio_value(&mut speaker, 2, tr("dialogue.npc"));
            if speaker != before {
                if let Some(p) = self.dialogue.nodes[i].payload.as_mut() {
                    p[LINE_SPEAKER] = speaker;
                }
                *edit = Some(Edit::Save(trf("dialogue.op_speaker", &[("node", &i)])));
            }
        });
        ui.horizontal(|ui| {
            ui.label(tr("dialogue.event"));
            ui.add(egui::TextEdit::singleline(&mut self.event_name).desired_width(220.0));
            let hash = parse_event(&self.event_name);
            let known = !self.catalog.lines(hash).is_empty();
            if ui
                .add_enabled(
                    hash != node.word(LINE_EVENT).unwrap_or(0),
                    egui::Button::new(tr("dialogue.set")),
                )
                .on_hover_text(tr("dialogue.event_hint"))
                .clicked()
            {
                if let Some(p) = self.dialogue.nodes[i].payload.as_mut() {
                    p[LINE_EVENT] = hash;
                }
                *edit = Some(Edit::Save(trf("dialogue.op_speech", &[("node", &i)])));
            }
            if !known {
                ui.colored_label(Color32::YELLOW, tr("dialogue.event_unknown"));
            }
        });
        for (label, w) in [
            ("dialogue.anim_lead", LINE_ANIM_LEAD),
            ("dialogue.anim_speaker", LINE_ANIM_SPEAKER),
            ("dialogue.anim_listener", LINE_ANIM_LISTENER),
        ] {
            if let Some(g) = node.word(w).filter(|&g| g != NONE) {
                ui.horizontal_wrapped(|ui| {
                    ui.weak(tr(label));
                    ui.label(self.dialogue.group_clips(g).join(", "));
                });
            }
        }
        ui.separator();
        ui.label(RichText::new(tr("dialogue.speech")).strong())
            .on_hover_text(tr("dialogue.speech_hint"));
        let code = self.lang_code().to_owned();
        let candidates: Vec<SpeechLine> = self
            .candidates(i)
            .into_iter()
            .take(SHOW_CANDIDATES)
            .cloned()
            .collect();
        if candidates.is_empty() {
            ui.weak(tr("dialogue.no_speech"));
        }
        for line in &candidates {
            let Some(entry) = line.entries.first().cloned() else {
                continue;
            };
            let short = entry.rsplit('\\').next().unwrap_or(&entry).to_owned();
            ui.horizontal_wrapped(|ui| {
                let changed = self.changed_files.contains(&entry);
                let mut name = RichText::new(&short).monospace();
                if changed {
                    name = name.color(Color32::from_rgb(255, 190, 90));
                }
                ui.label(name).on_hover_text(&entry);
                if ui
                    .small_button(tr("dialogue.export_wav"))
                    .on_hover_text(tr("dialogue.export_wav_hint"))
                    .clicked()
                {
                    export_wav(
                        &self.catalog,
                        &self.game_root,
                        self.ws_dir.as_deref(),
                        &entry,
                    );
                }
                if ui
                    .small_button(tr("dialogue.replace_audio"))
                    .on_hover_text(tr("dialogue.replace_audio_hint"))
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter(tr("sound.filter"), &["wav", "wave", "mp3", "ogg", "flac"])
                        .pick_file()
                {
                    let catalog = self.catalog.clone();
                    let entry2 = entry.clone();
                    *edit = Some(Edit::Apply(
                        trf("sound.import_file", &[("name", &short)]),
                        Box::new(move |game, ws| replace_audio(&catalog, game, ws, &entry2, &path)),
                    ));
                }
            });
            if let Some(table) = self.subtitles.get(&entry) {
                let col = speech::language_column(table, &code)
                    .or_else(|| speech::language_column(table, "en"))
                    .unwrap_or(0);
                let col_code = table.languages.get(col).cloned().unwrap_or_default();
                let current = table.strings[col].first().cloned().unwrap_or_default();
                let buffer = self
                    .subtitle_edits
                    .entry(entry.clone())
                    .or_insert_with(|| current.clone());
                ui.horizontal(|ui| {
                    ui.weak(&col_code);
                    ui.add(
                        egui::TextEdit::multiline(buffer)
                            .desired_rows(1)
                            .desired_width(ui.available_width() - 60.0),
                    );
                    if ui
                        .add_enabled(*buffer != current, egui::Button::new(tr("dialogue.save")))
                        .clicked()
                    {
                        let catalog = self.catalog.clone();
                        let (entry2, text, lc) = (entry.clone(), buffer.clone(), col_code.clone());
                        *edit = Some(Edit::Apply(
                            trf(
                                "dialogue.op_subtitle",
                                &[("name", &short), ("language", &lc)],
                            ),
                            Box::new(move |game, ws| {
                                dialogue::set_subtitle(&catalog, game, ws, &entry2, &lc, &text)
                                    .map(|_| ())
                            }),
                        ));
                    }
                });
            } else if let Some(text) = self.catalog.subtitles.get(&entry) {
                ui.label(format!("“{text}”"));
            } else {
                ui.weak(tr("dialogue.no_subtitle"));
            }
        }
        let total = self.candidates(i).len();
        if total > SHOW_CANDIDATES {
            ui.weak(trn(
                "dialogue.more_speech_count",
                (total - SHOW_CANDIDATES) as u64,
                &[],
            ));
        }
    }

    fn choice_details(&mut self, ui: &mut egui::Ui, edit: &mut Option<Edit>) {
        let i = self.selected;
        let mut timeout = f32::from_bits(self.dialogue.nodes[i].word(0).unwrap_or(0));
        ui.horizontal(|ui| {
            ui.label(tr("dialogue.timeout"));
            if ui
                .add(
                    egui::DragValue::new(&mut timeout)
                        .speed(0.5)
                        .range(-1.0..=3600.0),
                )
                .on_hover_text(tr("dialogue.timeout_hint"))
                .changed()
            {
                if let Some(p) = self.dialogue.nodes[i].payload.as_mut() {
                    p[0] = timeout.to_bits();
                }
                *edit = Some(Edit::Save(trf("dialogue.op_timeout", &[("node", &i)])));
            }
        });
        let code = self.lang_code().to_owned();
        let has_lang = !self.languages.is_empty();
        egui::Grid::new("options").striped(true).show(ui, |ui| {
            ui.strong(tr("dialogue.option"));
            ui.strong(tr("dialogue.label"));
            ui.strong(trf("dialogue.text_in", &[("lang", &code)]));
            ui.strong(tr("dialogue.target"));
            ui.end_row();
            for o in 0..CHOICE_SLOTS {
                ui.horizontal(|ui| {
                    ui.label(format!("{}", o + 1));
                    if o > 0
                        && ui.small_button(tr("dialogue.up")).clicked()
                        && self.dialogue.swap_options(i, o, o - 1).is_ok()
                    {
                        *edit = Some(Edit::Save(trf("dialogue.op_move_option", &[("node", &i)])));
                    }
                    if o + 1 < CHOICE_SLOTS
                        && ui.small_button(tr("dialogue.down")).clicked()
                        && self.dialogue.swap_options(i, o, o + 1).is_ok()
                    {
                        *edit = Some(Edit::Save(trf("dialogue.op_move_option", &[("node", &i)])));
                    }
                });
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.labels[o]).desired_width(170.0));
                    let current = self.dialogue.option_label(i, o).unwrap_or("").to_owned();
                    if ui
                        .add_enabled(
                            self.labels[o] != current,
                            egui::Button::new(tr("dialogue.set")),
                        )
                        .on_hover_text(tr("dialogue.label_hint"))
                        .clicked()
                        && self
                            .dialogue
                            .set_option_label(i, o, self.labels[o].trim())
                            .is_ok()
                    {
                        *edit = Some(Edit::Save(trf(
                            "dialogue.op_option_label",
                            &[("option", &(o + 1)), ("node", &i)],
                        )));
                    }
                });
                ui.horizontal(|ui| {
                    let label = self.labels[o].trim().to_owned();
                    let exists = !label.is_empty() && self.text_of(&label).is_some();
                    let current = self.text_of(&label).unwrap_or_default();
                    ui.add_enabled(
                        !label.is_empty() && has_lang,
                        egui::TextEdit::singleline(&mut self.texts[o]).desired_width(240.0),
                    );
                    if exists {
                        if ui
                            .add_enabled(
                                self.texts[o] != current,
                                egui::Button::new(tr("dialogue.save")),
                            )
                            .clicked()
                        {
                            let tables = self.catalog.languages.clone();
                            let (lc, text) = (code.clone(), self.texts[o].clone());
                            *edit = Some(Edit::Apply(
                                trf("dialogue.op_text", &[("name", &label), ("language", &lc)]),
                                Box::new(move |game, ws| {
                                    dialogue::set_string(game, ws, &tables, &lc, &label, &text)
                                        .map(|_| ())
                                }),
                            ));
                        }
                    } else if !label.is_empty()
                        && ui
                            .add_enabled(
                                !self.texts[o].is_empty(),
                                egui::Button::new(tr("dialogue.add_string")),
                            )
                            .on_hover_text(tr("dialogue.add_string_hint"))
                            .clicked()
                    {
                        let catalog = self.catalog.clone();
                        let text = self.texts[o].clone();
                        *edit = Some(Edit::Apply(
                            trf("dialogue.op_add_string", &[("name", &label)]),
                            Box::new(move |game, ws| add_string(&catalog, game, ws, &label, &text)),
                        ));
                    }
                });
                let target = self.dialogue.nodes[i].children.get(o).copied().flatten();
                ui.label(target.map_or("–".into(), |t| format!("#{t}")));
                ui.end_row();
            }
        });
    }

    fn test_details(&mut self, ui: &mut egui::Ui, edit: &mut Option<Edit>) {
        let i = self.selected;
        let kind = self.dialogue.nodes[i].kind;
        let Some(t) = Test::of(&self.dialogue.nodes[i]) else {
            ui.label(dlg::summary(&self.dialogue, i));
            return;
        };
        ui.label(if kind == KIND_ASSIGN {
            tr("dialogue.assign_hint")
        } else {
            tr("dialogue.test_hint")
        });
        let (mut ty, mut var, mut raw) = (t.value_type, t.variable, t.raw_value);
        let mut changed = false;
        egui::Grid::new("test").show(ui, |ui| {
            ui.label(tr("dialogue.variable"));
            changed |= ui
                .add(egui::DragValue::new(&mut var).hexadecimal(2, false, false))
                .changed();
            ui.end_row();
            ui.label(tr("dialogue.value_type"));
            egui::ComboBox::from_id_salt("ty")
                .selected_text(
                    ["bool", "int", "float"]
                        .get(ty as usize)
                        .copied()
                        .unwrap_or("?"),
                )
                .show_ui(ui, |ui| {
                    for (k, name) in ["bool", "int", "float"].iter().enumerate() {
                        changed |= ui.selectable_value(&mut ty, k as u32, *name).changed();
                    }
                });
            ui.end_row();
            ui.label(tr("dialogue.value"));
            match ty {
                0 => {
                    let mut b = raw & 0xFF != 0;
                    if ui.checkbox(&mut b, "").changed() {
                        raw = b as u32;
                        changed = true;
                    }
                }
                2 => {
                    let mut f = f32::from_bits(raw);
                    if ui.add(egui::DragValue::new(&mut f).speed(0.1)).changed() {
                        raw = f.to_bits();
                        changed = true;
                    }
                }
                _ => {
                    let mut v = raw as i32;
                    if ui.add(egui::DragValue::new(&mut v)).changed() {
                        raw = v as u32;
                        changed = true;
                    }
                }
            }
            ui.end_row();
        });
        if changed {
            if let Some(p) = self.dialogue.nodes[i].payload.as_mut() {
                p[0] = ty;
                p[2] = var;
                p[3] = raw;
            }
            *edit = Some(Edit::Save(trf("dialogue.op_values", &[("node", &i)])));
        }
    }

    fn branch_editor(&mut self, ui: &mut egui::Ui, edit: &mut Option<Edit>) {
        let i = self.selected;
        let n = self.dialogue.nodes.len();
        let kids = self.dialogue.nodes[i].children.clone();
        if kids.is_empty() {
            return;
        }
        ui.label(RichText::new(tr("dialogue.branches")).strong())
            .on_hover_text(tr("dialogue.branches_hint"));
        let names: Vec<String> = (0..n)
            .map(|t| {
                let mut c = self.caption(t);
                if c.chars().count() > 50 {
                    c = c.chars().take(50).collect::<String>() + "…";
                }
                format!("#{t} {c}")
            })
            .collect();
        for (k, child) in kids.iter().enumerate() {
            ui.horizontal(|ui| {
                let label = self.branch_label(i, k);
                ui.label(if label.is_empty() {
                    tr("dialogue.next").to_owned()
                } else {
                    label
                });
                let mut target = *child;
                egui::ComboBox::from_id_salt(("branch", k))
                    .width(320.0)
                    .selected_text(target.map_or("–".to_owned(), |t| names[t].clone()))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut target, None, "–");
                        for (t, name) in names.iter().enumerate() {
                            ui.selectable_value(&mut target, Some(t), name);
                        }
                    });
                if target != *child && self.dialogue.set_child(i, k, target).is_ok() {
                    *edit = Some(Edit::Save(trf(
                        "dialogue.op_link",
                        &[("node", &i), ("branch", &k)],
                    )));
                }
                if let Some(t) = child
                    && ui.small_button(tr("dialogue.go")).clicked()
                {
                    self.selected = *t;
                }
            });
        }
    }
}

fn parse_event(text: &str) -> u32 {
    let t = text.trim();
    if let Some(hex) = t.strip_prefix("0x")
        && let Ok(v) = u32::from_str_radix(hex, 16)
    {
        return v;
    }
    if t.len() == 8
        && t.chars().all(|c| c.is_ascii_hexdigit())
        && let Ok(v) = u32::from_str_radix(t, 16)
    {
        return v;
    }
    event_hash(t)
}

fn workspace_at(dir: Option<&Path>) -> Option<Workspace> {
    dir.filter(|d| Workspace::exists(d))
        .and_then(|d| Workspace::open(d).ok())
}

/// Writes a speech file as WAV where the user picks.
fn export_wav(catalog: &Catalog, root: &Path, ws_dir: Option<&Path>, entry: &str) {
    use doctor_toolkit::resource::audio::Sample;
    let Ok(game) = GameInstance::open(root) else {
        return;
    };
    let ws = workspace_at(ws_dir);
    let Ok(data) = catalog.read_entry(&game, ws.as_ref(), entry) else {
        return;
    };
    let Ok(wav) = Sample::parse_snu(&data).and_then(|s| s.to_wav()) else {
        return;
    };
    let stem = entry
        .rsplit('\\')
        .next()
        .unwrap_or(entry)
        .split('.')
        .next()
        .unwrap_or("speech");
    if let Some(path) = rfd::FileDialog::new()
        .set_file_name(format!("{stem}.wav"))
        .save_file()
    {
        let _ = std::fs::write(path, wav);
    }
}

/// Encodes a sound file like a speech file and records it (overlay archive);
/// the subtitles stay.
fn replace_audio(
    catalog: &Catalog,
    game: &GameInstance,
    ws: &mut Workspace,
    entry: &str,
    path: &Path,
) -> doctor_toolkit::Result<()> {
    let pcm = doctor_toolkit::resource::video::import::read_audio_file(path)?;
    let original = catalog.read_entry(game, Some(ws), entry)?;
    let data = doctor_toolkit::resource::audio_encode::encode_snu_like(&pcm, &original)?;
    let key = FileKey::archive_entry(entry);
    ws.set_file(
        game,
        &key,
        data,
        &trf("sound.import_file", &[("name", &key.name())]),
    )?;
    Ok(())
}

/// Adds a string to the tables of every language (`text-add`).
fn add_string(
    catalog: &Catalog,
    game: &GameInstance,
    ws: &mut Workspace,
    name: &str,
    text: &str,
) -> doctor_toolkit::Result<()> {
    let new = doctor_toolkit::text::NewString {
        name: name.to_owned(),
        text: text.to_owned(),
        ..Default::default()
    };
    let plan =
        doctor_toolkit::text::plan_add(game, ws, &catalog.text_index, &catalog.languages, &new)?;
    plan.commit(ws, game)?;
    Ok(())
}

impl ResourceViewer for DialogueView {
    fn show(&mut self, ui: &mut egui::Ui, _cx: &mut ViewContext<'_>) -> Option<ViewAction> {
        let mut edit = None;
        self.show_header(ui, &mut edit);
        ui.separator();
        let before = self.selected;
        ui.columns(2, |cols| {
            let [left, right] = cols else { return };
            left.push_id("tree", |ui| self.show_tree(ui));
            right.push_id("details", |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("details")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.show_details(ui, &mut edit));
            });
        });
        if self.selected != before {
            self.buffer_for = None;
        }
        match edit? {
            Edit::Save(label) => {
                self.rows = self.outline();
                self.save(label)
            }
            Edit::Apply(label, run) => Some(ViewAction::Apply { label, run }),
            Edit::Open(stream, resource) => Some(ViewAction::Open { stream, resource }),
        }
    }

    fn hides_hex(&self) -> bool {
        true
    }

    fn state(&self) -> Option<Box<dyn std::any::Any>> {
        Some(Box::new(Kept {
            selected: self.selected,
            lang: self.lang,
            search: self.search.clone(),
            hits: self.hits.clone(),
        }))
    }

    fn restore(&mut self, state: Box<dyn std::any::Any>) {
        if let Ok(kept) = state.downcast::<Kept>() {
            self.selected = kept
                .selected
                .min(self.dialogue.nodes.len().saturating_sub(1));
            self.lang = kept.lang.min(self.languages.len().saturating_sub(1));
            self.search = kept.search;
            self.hits = kept.hits;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_event;
    use doctor_toolkit::resource::chv::event_hash;

    #[test]
    fn events_by_name_or_hash() {
        assert_eq!(parse_event("GUARD_BEFORE_RIOT"), 0x44C5_F8C2);
        assert_eq!(parse_event("0x44c5f8c2"), 0x44C5_F8C2);
        assert_eq!(parse_event("44c5f8c2"), 0x44C5_F8C2);
        assert_eq!(parse_event("N3_LINE1"), event_hash("n3_line1"));
    }
}
