//! The animation graph of a bank (stage 43) on the node-graph widget: the
//! named entries, the node tree of the chosen entry, an inspector for the
//! parameters and extra data of the selected node, adding clip nodes,
//! removing and reordering children, and the checks of
//! `resource::anim_graph`. "Apply" records the new bank in the workspace as
//! one undoable change.
//!
//! Stage 56: the graph is a part of the bank's tab (a window before); the
//! inspector is in the Details panel while it is open.

use doctor_toolkit::resource::anim_graph::{
    self, AnimGraph, Field, FieldType, GraphNode, Issue, KindInfo, Severity,
};
use doctor_toolkit::resource::anim_image::BankImage;
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::{Color32, RichText};

use crate::node_graph::{
    self as ng, Badge, GraphEvent, GraphView, LayoutOptions, Node, NodeId, Pin, Wire,
};

/// The graph window of an animation bank.
pub struct GraphEditor {
    image: BankImage,
    original: AnimGraph,
    graph: AnimGraph,
    clip_names: Vec<String>,
    entry: usize,
    filter: String,
    shown: ng::Graph,
    view: GraphView,
    selected: Option<usize>,
    issues: Vec<Issue>,
    stale: bool,
    add_clip: usize,
    rename: String,
    confirm_remove_entry: bool,
    error: Option<String>,
}

/// The keys of the graph, for the "?" of the bank's tab.
pub const KEYS: &[(&str, &str)] = &[
    ("F", "keys.frame"),
    ("Home", "keys.frame_all"),
    ("Delete", "keys.delete"),
    ("Ctrl+A", "keys.select_all"),
    ("Ctrl+Z / Ctrl+Y", "keys.undo_applied"),
];

/// Header colour by what a node does.
fn color(kind: u8) -> Color32 {
    match kind & 0x3F {
        0x16 => Color32::from_rgb(60, 110, 70),
        0x02 | 0x09 | 0x19 => Color32::from_rgb(70, 80, 110),
        0x03 | 0x06 => Color32::from_rgb(120, 90, 50),
        0x05 | 0x0D | 0x0E | 0x12 => Color32::from_rgb(100, 60, 110),
        _ => Color32::from_rgb(80, 80, 80),
    }
}

/// A parameter word as text, with the control name where it is one.
fn word_text(field: Field, w: u32, kind: u8, i: usize) -> String {
    let value = match field.ty {
        FieldType::U32 if w > 0xFFFF => format!("{w:#010x}"),
        FieldType::U32 => w.to_string(),
        FieldType::F32 => format!("{}", f32::from_bits(w)),
        FieldType::Pair => format!("{}/{}", w & 0xFFFF, w >> 16),
    };
    match control_of(kind, i, w) {
        Some(c) => format!("{} {value} ({c})", field.name),
        None => format!("{} {value}", field.name),
    }
}

/// The control a parameter word names, if it names one.
fn control_of(kind: u8, i: usize, w: u32) -> Option<&'static str> {
    match (kind & 0x3F, i) {
        (0x05, 0) | (0x21, 0 | 1) => anim_graph::control_name(w),
        (0x06, 1) => anim_graph::control_name(w & 0xFFFF),
        _ => None,
    }
}

impl GraphEditor {
    /// Opens the graph of the bank `data`; `clip_names` name its clips.
    pub fn new(data: &[u8], clip_names: Vec<String>) -> Result<Self, String> {
        let image = BankImage::parse(data).map_err(|e| e.to_string())?;
        let graph = AnimGraph::parse(&image).map_err(|e| e.to_string())?;
        let mut editor = Self {
            original: graph.clone(),
            issues: graph.check(&image),
            graph,
            image,
            clip_names,
            entry: 0,
            filter: String::new(),
            shown: ng::Graph::default(),
            view: GraphView::default(),
            selected: None,
            stale: true,
            add_clip: 0,
            rename: String::new(),
            confirm_remove_entry: false,
            error: None,
        };
        // Wires are the tree; the user does not draw them.
        editor.view.can_connect = Some(Box::new(|_, _, _| false));
        editor.rename = editor
            .graph
            .entries
            .first()
            .map(|e| e.name.clone())
            .unwrap_or_default();
        Ok(editor)
    }

    /// Shows entry `entry` (by name) and selects its node `node` (depth-first
    /// position in the entry) for screenshots.
    pub fn preset(&mut self, entry: Option<&str>, node: Option<usize>) {
        if let Some(name) = entry
            && let Some(e) = self
                .graph
                .entries
                .iter()
                .position(|e| e.name.eq_ignore_ascii_case(name))
        {
            self.entry = e;
            self.rename = name.to_owned();
        }
        if let (Some(k), Some(e)) = (node, self.graph.entries.get(self.entry)) {
            self.selected = self.graph.walk(e.root).get(k).map(|&(n, _)| n);
        }
        self.stale = true;
    }

    fn clip_name(&self, c: u32) -> String {
        self.clip_names
            .get(c as usize)
            .cloned()
            .unwrap_or_else(|| format!("#{c}"))
    }

    /// Rebuilds the drawn graph from the chosen entry.
    fn rebuild(&mut self) {
        self.stale = false;
        self.issues = self.graph.check(&self.image);
        self.shown = ng::Graph::default();
        let Some(entry) = self.graph.entries.get(self.entry) else {
            return;
        };
        let walk = self.graph.walk(entry.root);
        for &(n, parent) in &walk {
            let node = &self.graph.nodes[n];
            let info = anim_graph::kind_info(node.kind);
            let label = info.map_or("?", |k| k.label);
            let mut shown = Node::new(NodeId(n as u64), format!("#{} {label}", node.number));
            shown.header_color = color(node.kind);
            shown.subtitle = match node.clip {
                Some(c) => format!("{:#04x}  {}", node.kind, self.clip_name(c)),
                None => format!("{:#04x}", node.kind),
            };
            let pfields = info.and_then(|k| k.params);
            for (i, &w) in node.params.iter().flatten().enumerate() {
                shown
                    .lines
                    .push(word_text(anim_graph::field(pfields, i), w, node.kind, i));
            }
            let xfields = parent
                .and_then(|p| anim_graph::kind_info(self.graph.nodes[p].kind))
                .and_then(|k| k.child_extra);
            for (i, &w) in node.extra.iter().flatten().enumerate() {
                shown.lines.push(format!(
                    "↳ {}",
                    word_text(anim_graph::field(xfields, i), w, 0, i)
                ));
            }
            if parent.is_some() {
                shown = shown.input(Pin::new(tr("anim_graph.parent"), 0));
            }
            for k in 0..node.children.len() {
                shown = shown.output(Pin::new(format!("{}", k + 1), 0));
            }
            for issue in self.issues.iter().filter(|i| i.node == Some(n)) {
                shown.badges.push(Badge {
                    text: "!".into(),
                    color: match issue.severity {
                        Severity::Error => Color32::from_rgb(220, 60, 60),
                        Severity::Warning => Color32::from_rgb(220, 180, 60),
                    },
                    tooltip: issue.text.clone(),
                });
            }
            self.shown.nodes.push(shown);
            if let Some(p) = parent {
                let k = self.graph.nodes[p]
                    .children
                    .iter()
                    .position(|&c| c == n)
                    .unwrap_or(0);
                self.shown
                    .wires
                    .push(Wire::new((NodeId(p as u64), k), (NodeId(n as u64), 0)));
            }
        }
        let pos = ng::layered(&self.shown, None, &LayoutOptions::default());
        for node in &mut self.shown.nodes {
            if let Some(p) = pos.get(&node.id) {
                node.pos = *p;
            }
        }
        self.view.frame_all();
        if let Some(s) = self.selected {
            self.view.set_selection([NodeId(s as u64)]);
        }
    }

    /// Runs an edit of the graph; shows its error.
    fn edit(&mut self, f: impl FnOnce(&mut AnimGraph) -> doctor_toolkit::Result<()>) {
        match f(&mut self.graph) {
            Ok(()) => self.error = None,
            Err(e) => self.error = Some(e.to_string()),
        }
        self.stale = true;
    }

    /// Frames the selected node, or the whole entry.
    pub fn frame(&mut self) {
        if self.selected.is_some() {
            self.view.frame_selection();
        } else {
            self.view.frame_all();
        }
    }

    /// The inspector for the Details panel; returns the new bank and a
    /// label when the user applies.
    pub fn inspector_ui(&mut self, ui: &mut egui::Ui, bank: &str) -> Option<(Vec<u8>, String)> {
        if self.stale {
            self.rebuild();
        }
        self.inspector(ui, bank)
    }

    /// The graph in the bank's tab: the entries on the left, the graph, and
    /// the inspector on the right unless `details` shows it. Returns the
    /// new bank and a label when the user applies.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        bank: &str,
        details: bool,
    ) -> Option<(Vec<u8>, String)> {
        if self.stale {
            self.rebuild();
        }
        let mut result = None;
        egui::Panel::left(ui.scope_id().with("anim-graph-entries"))
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| self.entries_panel(ui));
        if !details {
            egui::Panel::right(ui.scope_id().with("anim-graph-inspector"))
                .resizable(true)
                .default_size(340.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("anim graph inspector")
                        .auto_shrink(false)
                        .show(ui, |ui| result = self.inspector(ui, bank));
                });
        }
        let response = self.view.show(ui, &mut self.shown, &mut |_, _| {});
        for event in response.events {
            match event {
                GraphEvent::Selection(ids) => {
                    self.selected = ids.first().map(|id| id.0 as usize);
                }
                GraphEvent::Delete(ids) => {
                    if let Some(&id) = ids.first() {
                        self.edit(|g| g.remove_node(id.0 as usize));
                        self.selected = None;
                    }
                }
                _ => {}
            }
        }
        result
    }

    fn entries_panel(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text(tr("anim_graph.filter"))
                .desired_width(f32::INFINITY),
        );
        let filter = self.filter.to_ascii_lowercase();
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .id_salt("anim graph entries")
            .auto_shrink(false)
            .show(ui, |ui| {
                for (i, e) in self.graph.entries.iter().enumerate() {
                    if !filter.is_empty() && !e.name.to_ascii_lowercase().contains(&filter) {
                        continue;
                    }
                    let nodes = self.graph.walk(e.root).len();
                    if ui
                        .selectable_label(i == self.entry, format!("{}  ({nodes})", e.name))
                        .clicked()
                    {
                        chosen = Some(i);
                    }
                }
            });
        if let Some(i) = chosen {
            self.entry = i;
            self.selected = None;
            self.rename = self.graph.entries[i].name.clone();
            self.confirm_remove_entry = false;
            self.stale = true;
        }
    }

    /// An editor for a parameter word; true when changed.
    fn word_editor(
        ui: &mut egui::Ui,
        field: Field,
        word: &mut u32,
        control: Option<&'static str>,
    ) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(field.name).on_hover_text(field.help);
            match field.ty {
                FieldType::U32 => {
                    changed |= ui.add(egui::DragValue::new(word)).changed();
                }
                FieldType::F32 => {
                    let mut v = f32::from_bits(*word);
                    if ui.add(egui::DragValue::new(&mut v).speed(0.01)).changed() {
                        *word = v.to_bits();
                        changed = true;
                    }
                }
                FieldType::Pair => {
                    let (mut lo, mut hi) = (*word as u16, (*word >> 16) as u16);
                    changed |= ui.add(egui::DragValue::new(&mut lo)).changed();
                    changed |= ui.add(egui::DragValue::new(&mut hi)).changed();
                    *word = lo as u32 | (hi as u32) << 16;
                }
            }
            if let Some(c) = control {
                ui.label(RichText::new(c).weak());
            }
        });
        changed
    }

    fn inspector(&mut self, ui: &mut egui::Ui, bank: &str) -> Option<(Vec<u8>, String)> {
        let mut result = None;
        // Entry.
        if let Some(entry) = self.graph.entries.get(self.entry) {
            ui.strong(trf("anim_graph.entry", &[("name", &entry.name)]));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.rename).desired_width(180.0));
                if ui.button(tr("anim_graph.rename")).clicked() {
                    let (e, name) = (self.entry, self.rename.clone());
                    self.edit(|g| g.rename_entry(e, &name));
                }
            });
            ui.label(RichText::new(tr("anim_graph.rename_hint")).weak().small());
            let label = if self.confirm_remove_entry {
                tr("anim_graph.remove_entry_confirm")
            } else {
                tr("anim_graph.remove_entry")
            };
            if ui.button(label).clicked() {
                if self.confirm_remove_entry {
                    let e = self.entry;
                    self.edit(|g| g.remove_entry(e));
                    self.entry = 0;
                    self.selected = None;
                    self.confirm_remove_entry = false;
                } else {
                    self.confirm_remove_entry = true;
                }
            }
            ui.separator();
        }
        // Node.
        if let Some(n) = self.selected.filter(|&n| n < self.graph.nodes.len()) {
            self.node_inspector(ui, n);
            ui.separator();
        } else {
            ui.label(RichText::new(tr("anim_graph.select_hint")).weak());
            ui.separator();
        }
        if let Some(e) = &self.error {
            ui.colored_label(Color32::from_rgb(220, 80, 80), e);
        }
        // Checks.
        let errors = self
            .issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .count();
        ui.label(trf(
            "anim_graph.issues",
            &[("errors", &errors), ("all", &self.issues.len())],
        ));
        let mut jump = None;
        for issue in self.issues.iter().take(50) {
            let color = match issue.severity {
                Severity::Error => Color32::from_rgb(220, 80, 80),
                Severity::Warning => Color32::from_rgb(210, 170, 60),
            };
            let text = match issue.node {
                Some(n) => format!("#{} {}", self.graph.nodes[n].number, issue.text),
                None => issue.text.clone(),
            };
            if ui
                .add(egui::Label::new(RichText::new(text).color(color)).sense(egui::Sense::click()))
                .clicked()
            {
                jump = issue.node;
            }
        }
        if let Some(n) = jump {
            if let Some(e) = self.graph.entry_of(n) {
                self.entry = e;
            }
            self.selected = Some(n);
            self.stale = true;
        }
        ui.separator();
        let changed = self.graph != self.original;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    changed && errors == 0,
                    egui::Button::new(tr("anim_graph.apply")),
                )
                .on_hover_text(tr("anim_graph.apply_hint"))
                .clicked()
            {
                let mut image = self.image.clone();
                match self.graph.store(&mut image).and_then(|()| image.encode()) {
                    Ok(data) => {
                        self.image = image;
                        self.original = self.graph.clone();
                        result = Some((data, trf("anim_graph.label", &[("bank", &bank)])));
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            if ui
                .add_enabled(changed, egui::Button::new(tr("anim_graph.revert")))
                .clicked()
            {
                self.graph = self.original.clone();
                self.selected = None;
                self.stale = true;
            }
        });
        result
    }

    fn node_inspector(&mut self, ui: &mut egui::Ui, n: usize) {
        let parents = self.graph.parents();
        let node: GraphNode = self.graph.nodes[n].clone();
        let info: Option<&'static KindInfo> = anim_graph::kind_info(node.kind);
        ui.strong(format!(
            "#{} {:#04x} {}",
            node.number,
            node.kind,
            info.map_or("?", |k| k.label)
        ));
        if let Some(k) = info {
            ui.label(RichText::new(k.help).weak());
            ui.label(
                RichText::new(trf(
                    "anim_graph.exe_fn",
                    &[("address", &format!("{:#010x}", k.sample_fn))],
                ))
                .weak()
                .small(),
            );
        }
        // Parameters.
        if let Some(params) = &node.params {
            ui.label(tr("anim_graph.params"));
            let fields = info.and_then(|k| k.params);
            for (i, &w) in params.iter().enumerate() {
                let mut v = w;
                if Self::word_editor(
                    ui,
                    anim_graph::field(fields, i),
                    &mut v,
                    control_of(node.kind, i, w),
                ) {
                    self.edit(|g| g.set_param(n, i, v));
                }
            }
        }
        // Extra data for the parent.
        if let Some(extra) = &node.extra {
            ui.label(tr("anim_graph.extra"));
            let fields = parents[n]
                .and_then(|p| anim_graph::kind_info(self.graph.nodes[p].kind))
                .and_then(|k| k.child_extra);
            for (i, &w) in extra.iter().enumerate() {
                let mut v = w;
                if Self::word_editor(ui, anim_graph::field(fields, i), &mut v, None) {
                    self.edit(|g| g.set_extra(n, i, v));
                }
            }
        }
        // The clip of a clip node.
        if let Some(c) = node.clip {
            let mut chosen = c;
            egui::ComboBox::from_id_salt("anim graph clip")
                .selected_text(self.clip_name(c))
                .height(400.0)
                .show_ui(ui, |ui| {
                    for (i, name) in self.clip_names.iter().enumerate() {
                        ui.selectable_value(&mut chosen, i as u32, name);
                    }
                });
            if chosen != c {
                self.edit(|g| g.set_clip(n, chosen));
            }
            if ui
                .button(tr("anim_graph.duration_from_clip"))
                .on_hover_text(tr("anim_graph.duration_hint"))
                .clicked()
                && let Some(clip) = self.image.clips.get(chosen as usize)
            {
                let frames = clip.frames() as f32;
                let rate = node.param_f32(2).unwrap_or(2.0);
                let d = if clip.looping() {
                    frames
                } else {
                    (frames - 1.0).max(0.0)
                } * rate;
                self.edit(|g| g.set_param(n, 1, d.to_bits()));
            }
        }
        // Children.
        if info.is_some_and(|k| k.arity.variable()) {
            ui.separator();
            ui.label(tr("anim_graph.add_clip_node"));
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("anim graph add clip")
                    .selected_text(self.clip_name(self.add_clip as u32))
                    .height(400.0)
                    .show_ui(ui, |ui| {
                        for (i, name) in self.clip_names.iter().enumerate() {
                            ui.selectable_value(&mut self.add_clip, i, name);
                        }
                    });
                if ui.button(tr("anim_graph.add")).clicked()
                    && let Some(clip) = self.image.clips.get(self.add_clip)
                {
                    let (frames, looping, c) = (clip.frames(), clip.looping(), self.add_clip);
                    let mut added = None;
                    self.edit(|g| {
                        added = Some(g.add_clip_node(n, c as u32, frames, looping)?);
                        Ok(())
                    });
                    if let Some(a) = added {
                        self.selected = Some(a);
                    }
                }
            });
        }
        if let Some(p) = parents[n]
            && anim_graph::kind_info(self.graph.nodes[p].kind).is_some_and(|k| k.arity.variable())
        {
            let k = self.graph.nodes[p]
                .children
                .iter()
                .position(|&c| c == n)
                .unwrap_or(0);
            let count = self.graph.nodes[p].children.len();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(k > 0, egui::Button::new(tr("anim_graph.move_up")))
                    .clicked()
                {
                    self.edit(|g| g.move_child(p, k, k - 1));
                    self.selected = None;
                }
                if ui
                    .add_enabled(k + 1 < count, egui::Button::new(tr("anim_graph.move_down")))
                    .clicked()
                {
                    self.edit(|g| g.move_child(p, k, k + 1));
                    self.selected = None;
                }
                if ui
                    .add_enabled(count > 1, egui::Button::new(tr("anim_graph.remove_node")))
                    .on_hover_text(tr("anim_graph.remove_hint"))
                    .clicked()
                {
                    self.edit(|g| g.remove_node(n));
                    self.selected = None;
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bank of the corpus test shape: entry `idle` → sequence → random of
    /// two clip nodes.
    fn bank() -> Vec<u8> {
        use doctor_toolkit::resource::anim_graph::{GraphEntry, NODE_CLIP};
        use doctor_toolkit::resource::anim_image::{Block, ClipImage};
        let clip = |hash| ClipImage {
            record: ClipImage::new_record(hash, 3, 0x78E0_3D81, 0x8000_0004),
            streams: vec![0x2C, 0x00, 0, 0, 0],
            events: Vec::new(),
        };
        let mut image = BankImage {
            hash: 1,
            skeleton: 0x50F9_2C54,
            words: [0, 0, 0, 1],
            node_count: 0,
            entries: Vec::new(),
            event_objects: Vec::new(),
            graph: Block::default(),
            clips: vec![clip(0x10), clip(0x20)],
            tail: Block::default(),
        };
        let leaf = |c: u32| GraphNode {
            kind: NODE_CLIP,
            number: 0,
            params: Some(vec![0, 4f32.to_bits(), 2f32.to_bits()]),
            extra: Some(vec![1f32.to_bits()]),
            children: vec![],
            clip: Some(c),
        };
        let mut g = AnimGraph {
            entries: vec![GraphEntry {
                name: "idle".into(),
                root: 0,
            }],
            nodes: vec![
                GraphNode {
                    kind: 2,
                    number: 0,
                    params: Some(vec![0, 0x1203_2045]),
                    extra: None,
                    children: vec![1],
                    clip: None,
                },
                GraphNode {
                    kind: 3,
                    number: 0,
                    params: Some(vec![0, 1, 20f32.to_bits()]),
                    extra: None,
                    children: vec![2, 3],
                    clip: None,
                },
                leaf(0),
                leaf(1),
            ],
        };
        g.renumber();
        g.store(&mut image).unwrap();
        image.encode().unwrap()
    }

    #[test]
    fn shows_the_tree_of_an_entry() {
        let mut ed = GraphEditor::new(&bank(), vec!["a".into(), "b".into()]).unwrap();
        ed.rebuild();
        assert_eq!(ed.shown.nodes.len(), 4);
        assert_eq!(ed.shown.wires.len(), 3);
        assert!(ed.shown.nodes[1].title.contains("random"));
        assert_eq!(ed.shown.nodes[1].outputs.len(), 2);
        assert!(ed.shown.nodes[2].subtitle.contains('a'));
    }

    #[test]
    fn edits_and_applies_a_new_bank() {
        let data = bank();
        let mut ed = GraphEditor::new(&data, vec!["a".into(), "b".into()]).unwrap();
        ed.edit(|g| g.add_clip_node(1, 1, 3, false).map(|_| ()));
        assert!(ed.error.is_none());
        ed.rebuild();
        assert_eq!(ed.shown.nodes.len(), 5);
        let mut image = ed.image.clone();
        ed.graph.store(&mut image).unwrap();
        let out = image.encode().unwrap();
        let back = BankImage::parse(&out).unwrap();
        assert_eq!(back.entry_list()[0].clips, [0, 1, 1]);
        // A fixed child count refuses removal.
        ed.edit(|g| g.remove_node(1));
        assert!(ed.error.is_some());
    }
}
