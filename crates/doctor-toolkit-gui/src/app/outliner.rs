//! The Outliner (stage 53): the objects of the level in front as a tree —
//! zones, the classes in each zone, the objects of each class.
//!
//! - One row per shown node, drawn only where the list is scrolled to
//!   (`ScrollArea::show_rows`): a level has tens of thousands of objects.
//!   The tree is built again only when the level's objects change
//!   ([`LevelView::revision`]); the rows only when it, a fold or the search
//!   does.
//! - Search by name, class, GUID or number: the matching objects with
//!   their zone and class, unfolded.
//! - Click selects, Ctrl + click adds or takes away, Shift + click selects
//!   the range from the last clicked object (as the rows show it). A
//!   selection made in the view unfolds its zone and class and scrolls to it.
//! - The eye hides a zone, a class of a zone or an object in the view; the
//!   lock keeps clicks in the view from selecting it (and its gizmo away).
//! - "new" and "edited": objects the workspace added or changed.
//! - The context menu (and F, Ctrl+D, Delete over the list): fly to,
//!   duplicate, delete, show logic, open the SimGroup, copy the GUID.

use std::collections::{BTreeSet, HashMap, HashSet};

use doctor_toolkit::level::LevelGroup;
use doctor_toolkit_mods::i18n::{tr, trf, trn};
use egui::{FontId, Pos2, Rect, RichText, Sense, Vec2};

use super::BrowserApp;
use super::dock::{DockOp, Pane};
use super::theme::{self, Icon};
use crate::level_view::{LevelAction, LevelView, Mark, Node, ObjRef};

/// Height of a row.
const ROW: f32 = 19.0;
/// Indent of a level of the tree.
const INDENT: f32 = 14.0;

/// A zone of the tree.
#[derive(Debug)]
struct ZoneNode {
    stream: usize,
    name: String,
    classes: Vec<ClassNode>,
    objects: usize,
}

#[derive(Debug)]
struct ClassNode {
    class: u32,
    name: String,
    objects: Vec<ObjEntry>,
}

#[derive(Debug)]
struct ObjEntry {
    r: ObjRef,
    label: String,
    /// Lower-case name, class, GUID and number, for the search.
    search: String,
}

/// A row of the list: indices into the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Zone(usize),
    Class(usize, usize),
    Object(usize, usize, usize),
}

/// The Outliner of one level view.
#[derive(Default)]
pub struct Outline {
    /// The level revision the tree is built from.
    built: Option<u64>,
    zones: Vec<ZoneNode>,
    /// Where each object is in the tree.
    places: HashMap<ObjRef, (usize, usize, usize)>,
    /// Unfolded zones and classes.
    open: HashSet<Node>,
    pub search: String,
    /// The search the rows were made for.
    searched: String,
    rows: Vec<Row>,
    dirty: bool,
    /// Where Shift + click starts.
    anchor: Option<ObjRef>,
    /// The primary selection the Outliner showed last.
    seen: Option<ObjRef>,
    /// Scroll so that this row shows.
    scroll_to: Option<usize>,
}

/// Ctrl and Shift of a click.
#[derive(Debug, Clone, Copy, Default)]
pub struct Keys {
    pub ctrl: bool,
    pub shift: bool,
}

impl Outline {
    /// Builds the tree of `groups` (zone names by stream) again.
    pub fn build(&mut self, groups: &[LevelGroup], zone_name: &dyn Fn(usize) -> String) {
        let mut zones: Vec<ZoneNode> = Vec::new();
        let mut zone_of: HashMap<usize, usize> = HashMap::new();
        let mut class_of: HashMap<(usize, u32), usize> = HashMap::new();
        for (g, group) in groups.iter().enumerate() {
            let stream = group.location.stream;
            let z = *zone_of.entry(stream).or_insert_with(|| {
                zones.push(ZoneNode {
                    stream,
                    name: zone_name(stream),
                    classes: Vec::new(),
                    objects: 0,
                });
                zones.len() - 1
            });
            for o in &group.objects {
                let c = *class_of.entry((z, o.class)).or_insert_with(|| {
                    zones[z].classes.push(ClassNode {
                        class: o.class,
                        name: o
                            .class_name
                            .map_or_else(|| format!("{:08x}", o.class), str::to_owned),
                        objects: Vec::new(),
                    });
                    zones[z].classes.len() - 1
                });
                let class = &mut zones[z].classes[c];
                let label = if o.name.is_empty() {
                    format!("{} #{}", class.name, o.index)
                } else {
                    o.name.clone()
                };
                let search = format!(
                    "{} {} {:08x} {} #{}",
                    o.name, class.name, o.class, o.guid, o.index
                )
                .to_lowercase();
                class.objects.push(ObjEntry {
                    r: ObjRef {
                        group: g,
                        object: o.index,
                    },
                    label,
                    search,
                });
                zones[z].objects += 1;
            }
        }
        // Zones and classes by name; objects stay in their order.
        for zone in &mut zones {
            zone.classes.sort_by(|a, b| a.name.cmp(&b.name));
        }
        zones.sort_by(|a, b| a.name.cmp(&b.name));
        self.places.clear();
        for (z, zone) in zones.iter().enumerate() {
            for (c, class) in zone.classes.iter().enumerate() {
                for (o, entry) in class.objects.iter().enumerate() {
                    self.places.insert(entry.r, (z, c, o));
                }
            }
        }
        self.zones = zones;
        self.dirty = true;
    }

    /// The rows to show (made again when something changed).
    pub fn rows(&mut self) -> &[Row] {
        if self.dirty || self.searched != self.search {
            self.make_rows();
        }
        &self.rows
    }

    fn make_rows(&mut self) {
        self.dirty = false;
        self.searched.clone_from(&self.search);
        self.rows.clear();
        let needle = self.search.trim().to_lowercase();
        for (z, zone) in self.zones.iter().enumerate() {
            if needle.is_empty() {
                self.rows.push(Row::Zone(z));
                if !self.open.contains(&Node::Zone(zone.stream)) {
                    continue;
                }
                for (c, class) in zone.classes.iter().enumerate() {
                    self.rows.push(Row::Class(z, c));
                    if self.open.contains(&Node::Class(zone.stream, class.class)) {
                        self.rows
                            .extend((0..class.objects.len()).map(|o| Row::Object(z, c, o)));
                    }
                }
                continue;
            }
            let mut zone_row = false;
            for (c, class) in zone.classes.iter().enumerate() {
                let matching: Vec<usize> = (0..class.objects.len())
                    .filter(|&o| class.objects[o].search.contains(&needle))
                    .collect();
                if matching.is_empty() {
                    continue;
                }
                if !zone_row {
                    self.rows.push(Row::Zone(z));
                    zone_row = true;
                }
                self.rows.push(Row::Class(z, c));
                self.rows
                    .extend(matching.into_iter().map(|o| Row::Object(z, c, o)));
            }
        }
    }

    /// The node of a row.
    pub fn node(&self, row: Row) -> Node {
        match row {
            Row::Zone(z) => Node::Zone(self.zones[z].stream),
            Row::Class(z, c) => Node::Class(self.zones[z].stream, self.zones[z].classes[c].class),
            Row::Object(z, c, o) => Node::Object(self.zones[z].classes[c].objects[o].r),
        }
    }

    /// Every object under a node.
    pub fn objects_of(&self, node: Node) -> Vec<ObjRef> {
        let zone = |stream| self.zones.iter().find(|z| z.stream == stream);
        match node {
            Node::Zone(stream) => zone(stream)
                .map(|z| {
                    z.classes
                        .iter()
                        .flat_map(|c| c.objects.iter().map(|o| o.r))
                        .collect()
                })
                .unwrap_or_default(),
            Node::Class(stream, class) => zone(stream)
                .and_then(|z| z.classes.iter().find(|c| c.class == class))
                .map(|c| c.objects.iter().map(|o| o.r).collect())
                .unwrap_or_default(),
            Node::Object(r) => vec![r],
        }
    }

    /// Folds or unfolds a zone or class.
    /// Unfolds and scrolls to the primary selection again ("Show in the
    /// Outliner" of the view's context menu).
    pub fn reveal_selection(&mut self) {
        self.seen = None;
    }

    pub fn toggle(&mut self, node: Node) {
        if !self.open.remove(&node) {
            self.open.insert(node);
        }
        self.dirty = true;
    }

    pub fn is_open(&self, node: Node) -> bool {
        !self.search.trim().is_empty() || self.open.contains(&node)
    }

    /// Unfolds the zone and class of `r` and scrolls to it.
    pub fn reveal(&mut self, r: ObjRef) {
        let Some(&(z, c, _)) = self.places.get(&r) else {
            return;
        };
        let zone = &self.zones[z];
        let zone_node = Node::Zone(zone.stream);
        let class_node = Node::Class(zone.stream, zone.classes[c].class);
        if !self.open.contains(&zone_node) || !self.open.contains(&class_node) {
            self.open.insert(zone_node);
            self.open.insert(class_node);
            self.dirty = true;
        }
        let place = self.places[&r];
        self.scroll_to = self
            .rows()
            .iter()
            .position(|&row| row == Row::Object(place.0, place.1, place.2));
    }

    /// The objects of the rows in their order.
    fn shown_objects(&mut self) -> Vec<ObjRef> {
        let rows: Vec<Row> = self.rows().to_vec();
        rows.into_iter()
            .filter_map(|row| match row {
                Row::Object(z, c, o) => Some(self.zones[z].classes[c].objects[o].r),
                _ => None,
            })
            .collect()
    }

    /// The selection after a click on object `r`: alone, added or taken
    /// away (Ctrl), the range from the anchor (Shift, with Ctrl added to
    /// the selection). Returns the primary object and the set.
    pub fn click(
        &mut self,
        r: ObjRef,
        keys: Keys,
        primary: Option<ObjRef>,
        selection: &BTreeSet<ObjRef>,
    ) -> (Option<ObjRef>, BTreeSet<ObjRef>) {
        if keys.shift
            && let Some(anchor) = self.anchor
        {
            let shown = self.shown_objects();
            if let (Some(a), Some(b)) = (
                shown.iter().position(|&x| x == anchor),
                shown.iter().position(|&x| x == r),
            ) {
                let (from, to) = (a.min(b), a.max(b));
                let mut set: BTreeSet<ObjRef> = if keys.ctrl {
                    selection.clone()
                } else {
                    BTreeSet::new()
                };
                set.extend(shown[from..=to].iter().copied());
                return (Some(r), set);
            }
        }
        self.anchor = Some(r);
        if keys.ctrl {
            let mut set = selection.clone();
            if set.remove(&r) {
                let primary = primary
                    .filter(|&p| p != r)
                    .or_else(|| set.iter().next_back().copied());
                return (primary, set);
            }
            set.insert(r);
            return (Some(r), set);
        }
        (Some(r), BTreeSet::from([r]))
    }

    fn zone_label(&self, z: usize) -> (String, String) {
        let zone = &self.zones[z];
        (zone.name.clone(), zone.objects.to_string())
    }
}

/// What a row of the list asks for, done once the list is drawn.
enum Op {
    Click(ObjRef, Keys),
    Toggle(Node),
    Hide(Node, bool),
    Lock(Node, bool),
    Select(Vec<ObjRef>),
    Focus(ObjRef),
    Edit(LevelAction),
    Duplicate,
    Remove,
    Copy(String),
}

impl BrowserApp {
    /// The Outliner panel of the level in front.
    pub(super) fn outliner_pane(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.panel_level.filter(|id| self.levels.contains_key(id)) else {
            self.no_level(ui);
            return;
        };
        let mut outline = self.outliners.remove(&id).unwrap_or_default();
        let actions = {
            let level = self.levels.get_mut(&id).expect("checked");
            outliner_ui(ui, &mut outline, level, id)
        };
        self.outliners.insert(id, outline);
        for action in actions {
            if matches!(action, LevelAction::ShowZones) {
                self.dock_ops.push(DockOp::ShowPanel(Pane::Zones));
            } else {
                self.level_action(id, action);
            }
        }
    }
}

/// Draws the Outliner of `level`; returns the edits to record.
fn outliner_ui(
    ui: &mut egui::Ui,
    outline: &mut Outline,
    level: &mut LevelView,
    id: u64,
) -> Vec<LevelAction> {
    let mut out = Vec::new();
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
    // The bar: search, what is loaded, the Zones panel.
    ui.horizontal(|ui| {
        let zones = ui
            .button(tr("outliner.zones"))
            .on_hover_text(tr("outliner.zones_hint"));
        if zones.clicked() {
            out.push(LevelAction::ShowZones);
        }
        ui.add(
            egui::TextEdit::singleline(&mut outline.search)
                .hint_text(tr("outliner.search"))
                .desired_width(ui.available_width()),
        );
    });
    if let Some((done, total)) = level.progress() {
        ui.add(
            egui::ProgressBar::new(done as f32 / total.max(1) as f32).text(trf(
                "level.meshes_progress",
                &[("done", &done), ("total", &total)],
            )),
        );
    }
    let Some(groups) = level.level().map(|l| &l.groups) else {
        ui.add_space(6.0);
        ui.label(RichText::new(tr("outliner.no_zones")).weak());
        return out;
    };
    if outline.built != Some(level.revision()) {
        let game = level.game().map(|(g, _)| g.clone());
        let name = |stream: usize| {
            game.as_ref().map_or_else(
                || stream.to_string(),
                |g| {
                    let name = g.streams()[stream].stream_name();
                    name.strip_prefix("gf2/")
                        .map_or_else(|| name.clone(), str::to_owned)
                },
            )
        };
        outline.build(groups, &name);
        outline.built = Some(level.revision());
        outline.seen = None;
    }
    let objects: usize = outline.zones.iter().map(|z| z.objects).sum();
    let selected = level.selection();
    ui.label(
        RichText::new(trf(
            "outliner.summary",
            &[
                ("objects", &trn("count.objects", objects as u64, &[])),
                (
                    "zones",
                    &trn("count.zones", outline.zones.len() as u64, &[]),
                ),
                ("selected", &selected.len()),
            ],
        ))
        .weak()
        .small(),
    );
    // A selection made elsewhere shows here.
    let primary = level.primary();
    if primary != outline.seen {
        outline.seen = primary;
        if let Some(r) = primary {
            outline.reveal(r);
        }
    }
    let mut ops = Vec::new();
    let rows: Vec<Row> = outline.rows().to_vec();
    let mut area = egui::ScrollArea::vertical()
        .id_salt(("outliner", id))
        .auto_shrink(false);
    if let Some(row) = outline.scroll_to.take() {
        let spacing = ui.spacing().item_spacing.y;
        let target = row as f32 * (ROW + spacing);
        area = area.vertical_scroll_offset((target - ui.available_height() / 2.0).max(0.0));
    }
    let list = area.show_rows(ui, ROW, rows.len(), |ui, range| {
        for &row in &rows[range] {
            row_ui(ui, outline, level, row, &mut ops);
        }
    });
    // Keys over the list.
    let over = ui.rect_contains_pointer(list.inner_rect);
    if over && !ui.ctx().egui_wants_keyboard_input() {
        let (f, d, del) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::F) && !i.modifiers.any(),
                i.key_pressed(egui::Key::D) && i.modifiers.command,
                i.key_pressed(egui::Key::Delete),
            )
        });
        if f && let Some(r) = level.primary() {
            ops.push(Op::Focus(r));
        }
        if d {
            ops.push(Op::Duplicate);
        }
        if del {
            ops.push(Op::Remove);
        }
    }
    for op in ops {
        match op {
            Op::Click(r, keys) => {
                let (primary, set) = outline.click(r, keys, level.primary(), &level.selection());
                outline.seen = primary;
                level.set_selection(primary, set);
            }
            Op::Toggle(node) => outline.toggle(node),
            Op::Hide(node, on) => level.set_hidden(node, on),
            Op::Lock(node, on) => level.set_locked(node, on),
            Op::Select(objects) => {
                let primary = objects.first().copied();
                outline.seen = primary;
                level.set_selection(primary, objects.into_iter().collect());
            }
            Op::Focus(r) => level.focus_on(r),
            Op::Edit(action) => out.push(action),
            Op::Duplicate => out.extend(level.edit_selection(false)),
            Op::Remove => out.extend(level.edit_selection(true)),
            Op::Copy(text) => ui.ctx().copy_text(text),
        }
    }
    out
}

/// One row: eye, lock, the fold arrow, the name, the marks.
fn row_ui(ui: &mut egui::Ui, outline: &Outline, level: &LevelView, row: Row, ops: &mut Vec<Op>) {
    let node = outline.node(row);
    let (depth, label, count) = match row {
        Row::Zone(z) => {
            let (name, count) = outline.zone_label(z);
            (0, name, Some(count))
        }
        Row::Class(z, c) => {
            let class = &outline.zones[z].classes[c];
            (1, class.name.clone(), Some(class.objects.len().to_string()))
        }
        Row::Object(z, c, o) => (
            2,
            outline.zones[z].classes[c].objects[o].label.clone(),
            None,
        ),
    };
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ROW), Sense::click());
    let r = match node {
        Node::Object(r) => Some(r),
        _ => None,
    };
    let selected = r.is_some_and(|r| level.is_selected(r));
    let primary = r.is_some() && r == level.primary();
    let visuals = ui.visuals().clone();
    if selected {
        let fill = theme::FOCUS.gamma_multiply(if primary { 0.45 } else { 0.28 });
        ui.painter().rect_filled(rect, 2.0, fill);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, 2.0, visuals.widgets.hovered.weak_bg_fill);
    }
    // Eye and lock, in their own column on the left.
    let hidden = level.is_hidden(node);
    let locked = level.is_locked(node);
    let mut x = rect.left() + 2.0;
    let eye_rect = Rect::from_min_size(Pos2::new(x, rect.top() + 1.0), Vec2::splat(ROW - 2.0));
    x += ROW;
    let lock_rect = Rect::from_min_size(Pos2::new(x, rect.top() + 1.0), Vec2::splat(ROW - 2.0));
    x += ROW + 2.0;
    let eye = ui
        .interact(eye_rect, response.id.with("eye"), Sense::click())
        .on_hover_text(if hidden {
            tr("outliner.show_hint")
        } else {
            tr("outliner.hide_hint")
        });
    let lock = ui
        .interact(lock_rect, response.id.with("lock"), Sense::click())
        .on_hover_text(if locked {
            tr("outliner.unlock_hint")
        } else {
            tr("outliner.lock_hint")
        });
    let icon_color = |on: bool, hovered: bool| {
        if hovered {
            visuals.strong_text_color()
        } else if on {
            visuals.text_color()
        } else {
            visuals.weak_text_color().gamma_multiply(0.6)
        }
    };
    theme::paint_icon(
        ui.painter(),
        eye_rect.shrink(1.5),
        if hidden { Icon::EyeClosed } else { Icon::Eye },
        icon_color(!hidden, eye.hovered()),
    );
    // The lock shows only where it is on (or under the pointer), as in UE.
    if locked || lock.hovered() || response.hovered() {
        theme::paint_icon(
            ui.painter(),
            lock_rect.shrink(1.5),
            if locked { Icon::Lock } else { Icon::Unlock },
            icon_color(locked, lock.hovered()),
        );
    }
    if eye.clicked() {
        ops.push(Op::Hide(node, !hidden));
    }
    if lock.clicked() {
        ops.push(Op::Lock(node, !locked));
    }
    x += depth as f32 * INDENT;
    // The fold arrow of zones and classes.
    let arrow = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(INDENT, ROW));
    if r.is_none() {
        let open = outline.is_open(node);
        let c = arrow.center();
        let h = 3.5;
        let points = if open {
            vec![
                Pos2::new(c.x - h, c.y - h * 0.5),
                Pos2::new(c.x + h, c.y - h * 0.5),
                Pos2::new(c.x, c.y + h * 0.7),
            ]
        } else {
            vec![
                Pos2::new(c.x - h * 0.5, c.y - h),
                Pos2::new(c.x + h * 0.7, c.y),
                Pos2::new(c.x - h * 0.5, c.y + h),
            ]
        };
        ui.painter().add(egui::Shape::convex_polygon(
            points,
            visuals.text_color(),
            egui::Stroke::NONE,
        ));
    }
    x += INDENT + 2.0;
    // The name; the object count or the marks on the right.
    let effectively_hidden = r.is_some_and(|r| level.is_object_hidden(r));
    let color = if effectively_hidden || (r.is_none() && hidden) {
        visuals.weak_text_color()
    } else if depth == 0 {
        visuals.strong_text_color()
    } else {
        visuals.text_color()
    };
    let font = FontId::proportional(13.0);
    let mut right = rect.right() - 4.0;
    let mark = r.and_then(|r| level.mark(r));
    let tail = match (&count, mark) {
        (Some(count), _) => Some((count.clone(), visuals.weak_text_color())),
        (None, Some(Mark::New)) => Some((tr("outliner.new").to_owned(), theme::AXIS_Y)),
        (None, Some(Mark::Edited)) => Some((tr("outliner.edited").to_owned(), theme::ACCENT)),
        (None, None) => None,
    };
    if let Some((text, tail_color)) = tail {
        let galley = ui
            .painter()
            .layout_no_wrap(text, FontId::proportional(11.0), tail_color);
        right -= galley.size().x;
        ui.painter().galley(
            Pos2::new(right, rect.center().y - galley.size().y / 2.0),
            galley,
            tail_color,
        );
        right -= 6.0;
    }
    let text_rect = Rect::from_min_max(Pos2::new(x, rect.top()), Pos2::new(right, rect.bottom()));
    let galley = ui.painter().layout(label, font, color, f32::INFINITY);
    ui.painter().with_clip_rect(text_rect).galley(
        Pos2::new(x, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    // Clicks: objects select, zones and classes fold.
    if response.clicked() {
        let keys = ui.input(|i| Keys {
            ctrl: i.modifiers.command,
            shift: i.modifiers.shift,
        });
        match r {
            Some(r) => ops.push(Op::Click(r, keys)),
            None => ops.push(Op::Toggle(node)),
        }
    }
    if response.double_clicked()
        && let Some(r) = r
    {
        ops.push(Op::Focus(r));
    }
    if response.secondary_clicked()
        && let Some(r) = r
        && !level.is_selected(r)
    {
        ops.push(Op::Click(r, Keys::default()));
    }
    response.context_menu(|ui| context_menu(ui, outline, level, node, hidden, locked, ops));
}

fn context_menu(
    ui: &mut egui::Ui,
    outline: &Outline,
    level: &LevelView,
    node: Node,
    hidden: bool,
    locked: bool,
    ops: &mut Vec<Op>,
) {
    ui.set_min_width(220.0);
    let item = |ui: &mut egui::Ui, text: &str, keys: &str| {
        let mut button = egui::Button::new(text);
        if !keys.is_empty() {
            button = button.shortcut_text(keys);
        }
        ui.add(button).clicked()
    };
    match node {
        Node::Object(r) => {
            let count = level.selection().len().max(1);
            if item(ui, tr("outliner.focus"), "F") {
                ops.push(Op::Focus(r));
            }
            let many = |key: &str| {
                if count > 1 {
                    format!("{} ({count})", tr(key))
                } else {
                    tr(key).to_owned()
                }
            };
            if item(ui, &many("outliner.duplicate"), "Ctrl+D") {
                ops.push(Op::Duplicate);
            }
            if item(ui, &many("outliner.delete"), "Del") {
                ops.push(Op::Remove);
            }
            ui.separator();
            if let Some((group, object)) = level.location_of(r) {
                if item(ui, tr("outliner.show_logic"), "") {
                    ops.push(Op::Edit(LevelAction::ShowLogic { group, object }));
                }
                if item(ui, tr("outliner.open_simgroup"), "") {
                    ops.push(Op::Edit(LevelAction::OpenResource { group }));
                }
            }
            if let Some(o) = level.object(r)
                && item(ui, tr("outliner.copy_guid"), "")
            {
                ops.push(Op::Copy(o.guid.to_string()));
            }
        }
        Node::Zone(_) | Node::Class(..) => {
            let objects = outline.objects_of(node);
            if item(
                ui,
                &trn("outliner.select_all", objects.len() as u64, &[]),
                "",
            ) {
                ops.push(Op::Select(objects));
            }
            let fold = if outline.is_open(node) {
                tr("outliner.collapse")
            } else {
                tr("outliner.expand")
            };
            if item(ui, fold, "") {
                ops.push(Op::Toggle(node));
            }
        }
    }
    ui.separator();
    let hide = if hidden {
        tr("outliner.show")
    } else {
        tr("outliner.hide")
    };
    if item(ui, hide, "") {
        ops.push(Op::Hide(node, !hidden));
    }
    let lock = if locked {
        tr("outliner.unlock")
    } else {
        tr("outliner.lock")
    };
    if item(ui, lock, "") {
        ops.push(Op::Lock(node, !locked));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctor_toolkit::level::Location;
    use doctor_toolkit::resource::simgroup::{
        ATTR_LOAD_MATRIX, CLASS_SYSTEM_COMMANDS, Guid, SimGroup, Value,
    };

    /// A SimGroup with objects of classes `classes` (each placed).
    fn group(classes: &[u32], seed: u8) -> SimGroup {
        let mut g = SimGroup::empty();
        for (n, &class) in classes.iter().enumerate() {
            let mut guid = [seed; 16];
            guid[0] = n as u8;
            let o = g
                .push_object(class, Guid(guid), &[(CLASS_SYSTEM_COMMANDS, 2)])
                .unwrap();
            g.set_class_attr(
                o,
                CLASS_SYSTEM_COMMANDS,
                ATTR_LOAD_MATRIX,
                Some(Value::Matrix(doctor_toolkit::level::IDENTITY)),
            )
            .unwrap();
        }
        SimGroup::parse(&g.encode()).unwrap()
    }

    /// Two zones: "b" (stream 1) with classes 2, 1, 2 and "a" (stream 0)
    /// with class 1 twice.
    fn outline() -> Outline {
        let groups = vec![
            LevelGroup::new(
                Location {
                    stream: 1,
                    resource: 0,
                },
                group(&[2, 1, 2], 0xb0),
            ),
            LevelGroup::new(
                Location {
                    stream: 0,
                    resource: 3,
                },
                group(&[1, 1], 0xa0),
            ),
        ];
        let mut o = Outline::default();
        o.build(&groups, &|s| ["gf2/a", "gf2/b"][s].to_owned());
        o
    }

    fn r(group: usize, object: usize) -> ObjRef {
        ObjRef { group, object }
    }

    #[test]
    fn the_tree_folds_and_shows_only_what_is_open() {
        let mut o = outline();
        // Zones by name: "a" first.
        assert_eq!(o.rows(), [Row::Zone(0), Row::Zone(1)]);
        assert_eq!(o.node(Row::Zone(0)), Node::Zone(0));
        o.toggle(Node::Zone(1));
        assert_eq!(o.rows().len(), 4, "zone b and its two classes");
        o.toggle(Node::Class(1, 2));
        let rows = o.rows().to_vec();
        assert_eq!(rows.len(), 6);
        assert_eq!(o.node(rows[5]), Node::Object(r(0, 2)));
        assert_eq!(o.objects_of(Node::Zone(1)).len(), 3);
        assert_eq!(o.objects_of(Node::Class(1, 2)), [r(0, 0), r(0, 2)]);
        o.toggle(Node::Zone(1));
        assert_eq!(o.rows().len(), 2, "folded again");
    }

    #[test]
    fn search_finds_by_class_guid_and_number() {
        let mut o = outline();
        o.search = "a0a0a0".into();
        // GUIDs of zone "a" objects contain a0 bytes: both objects there.
        let rows = o.rows().to_vec();
        assert!(rows.contains(&Row::Zone(0)) && !rows.contains(&Row::Zone(1)));
        assert_eq!(
            rows.iter().filter(|r| matches!(r, Row::Object(..))).count(),
            2
        );
        o.search = "#2".into();
        let rows = o.rows().to_vec();
        assert_eq!(rows, [Row::Zone(1), Row::Class(1, 1), Row::Object(1, 1, 1)]);
        o.search = "00000001".into();
        let objects = o
            .rows()
            .iter()
            .filter(|r| matches!(r, Row::Object(..)))
            .count();
        assert_eq!(objects, 3, "class 1 by its hash, in both zones");
        o.search.clear();
        assert_eq!(o.rows().len(), 2);
    }

    #[test]
    fn clicks_select_one_add_with_ctrl_and_ranges_with_shift() {
        let mut o = outline();
        o.toggle(Node::Zone(1));
        o.toggle(Node::Class(1, 1));
        o.toggle(Node::Class(1, 2));
        // Shown objects: class 1 → (0,1); class 2 → (0,0), (0,2).
        let none = BTreeSet::new();
        let (p, set) = o.click(r(0, 1), Keys::default(), None, &none);
        assert_eq!((p, set.len()), (Some(r(0, 1)), 1));
        let (p, set) = o.click(
            r(0, 2),
            Keys {
                shift: true,
                ctrl: false,
            },
            p,
            &set,
        );
        assert_eq!(p, Some(r(0, 2)));
        assert_eq!(set, BTreeSet::from([r(0, 0), r(0, 1), r(0, 2)]));
        // Ctrl takes one away, and adds it back.
        let ctrl = Keys {
            ctrl: true,
            shift: false,
        };
        let (p, set) = o.click(r(0, 0), ctrl, p, &set);
        assert_eq!(set, BTreeSet::from([r(0, 1), r(0, 2)]));
        assert_eq!(p, Some(r(0, 2)), "the primary stays");
        let (p, set) = o.click(r(0, 0), ctrl, p, &set);
        assert_eq!((p, set.len()), (Some(r(0, 0)), 3));
        // A plain click selects one again.
        let (_, set) = o.click(r(0, 2), Keys::default(), p, &set);
        assert_eq!(set, BTreeSet::from([r(0, 2)]));
    }

    #[test]
    fn a_selection_made_elsewhere_is_unfolded_and_scrolled_to() {
        let mut o = outline();
        o.reveal(r(1, 1));
        assert!(o.is_open(Node::Zone(0)) && o.is_open(Node::Class(0, 1)));
        assert_eq!(o.scroll_to, Some(3), "zone a, class 1, object 0, object 1");
    }
}
