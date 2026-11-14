//! The look of the editor and of Open Mod Manager (stage 52): a dark theme
//! in the manner of Unreal Engine 5 — neutral greys with a slight cold
//! tint, blue for what is interactive or has the focus, amber for the
//! selected object, red, green and blue for the X, Y and Z axes — dense
//! spacing, and the icons of the toolbar and the status bar.
//!
//! The icons are simple vector drawings made with the painter: no icon font
//! is bundled, so nothing is added to the licences of the program.

use egui::{
    Color32, CornerRadius, Pos2, Rect, Response, Sense, Shadow, Stroke, Ui, Vec2, pos2, vec2,
};

/// The selected object (outline, rows of the selection, active document tab).
pub const ACCENT: Color32 = Color32::from_rgb(0xd9, 0xa4, 0x41);
/// What is interactive or has the focus.
pub const FOCUS: Color32 = Color32::from_rgb(0x3d, 0x8b, 0xfd);
/// Axis colours: X red, Y green, Z blue.
pub const AXIS_X: Color32 = Color32::from_rgb(0xe5, 0x53, 0x4b);
pub const AXIS_Y: Color32 = Color32::from_rgb(0x57, 0xab, 0x5a);
pub const AXIS_Z: Color32 = Color32::from_rgb(0x53, 0x9b, 0xf5);
/// The Play button.
pub const PLAY: Color32 = Color32::from_rgb(0x2c, 0x6e, 0x3a);
pub const PLAY_HOVER: Color32 = Color32::from_rgb(0x34, 0x84, 0x45);

// Greys of the dark theme, darkest first.
const INK_0: Color32 = Color32::from_rgb(0x0b, 0x0c, 0x0e);
const INK_1: Color32 = Color32::from_rgb(0x15, 0x16, 0x1a);
const INK_2: Color32 = Color32::from_rgb(0x1c, 0x1e, 0x22);
const INK_3: Color32 = Color32::from_rgb(0x25, 0x28, 0x2d);
const INK_4: Color32 = Color32::from_rgb(0x32, 0x36, 0x3c);
const INK_5: Color32 = Color32::from_rgb(0x3e, 0x43, 0x4a);
const FG: Color32 = Color32::from_rgb(0xd6, 0xd4, 0xcf);
const FG_DIM: Color32 = Color32::from_rgb(0xa9, 0xab, 0xaf);

/// Installs the dark and the light theme; the preference of the settings
/// picks one of them.
pub fn install(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Dark, dark());
    ctx.set_visuals_of(egui::Theme::Light, light());
    ctx.all_styles_mut(|style| {
        let spacing = &mut style.spacing;
        spacing.item_spacing = vec2(6.0, 3.0);
        spacing.button_padding = vec2(6.0, 2.0);
        spacing.menu_margin = egui::Margin::same(4);
        spacing.window_margin = egui::Margin::same(8);
        spacing.indent = 14.0;
    });
}

/// The dark theme.
pub fn dark() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    let corner = CornerRadius::same(3);
    v.panel_fill = INK_2;
    v.window_fill = Color32::from_rgb(0x20, 0x22, 0x27);
    v.window_stroke = Stroke::new(1.0, INK_4);
    v.window_corner_radius = CornerRadius::same(4);
    v.menu_corner_radius = CornerRadius::same(4);
    v.window_shadow = Shadow {
        offset: [0, 8],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 10,
        spread: 0,
        color: Color32::from_black_alpha(110),
    };
    v.extreme_bg_color = INK_1;
    v.faint_bg_color = Color32::from_rgb(0x20, 0x22, 0x27);
    v.code_bg_color = INK_3;
    v.hyperlink_color = Color32::from_rgb(0x6c, 0xa8, 0xff);
    v.warn_fg_color = ACCENT;
    v.error_fg_color = AXIS_X;
    v.selection.bg_fill = Color32::from_rgb(0x1f, 0x4f, 0x95);
    v.selection.stroke = Stroke::new(1.0, Color32::from_rgb(0xe8, 0xef, 0xfc));
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = INK_2;
    w.noninteractive.weak_bg_fill = INK_2;
    w.noninteractive.bg_stroke = Stroke::new(1.0, INK_4);
    w.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(0xc4, 0xc3, 0xbf));
    w.noninteractive.corner_radius = corner;
    w.inactive.weak_bg_fill = INK_3;
    w.inactive.bg_fill = INK_4;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, FG);
    w.inactive.corner_radius = corner;
    w.hovered.weak_bg_fill = INK_4;
    w.hovered.bg_fill = INK_5;
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x4c, 0x52, 0x5a));
    w.hovered.fg_stroke = Stroke::new(1.5, Color32::from_rgb(0xf2, 0xf1, 0xee));
    w.hovered.corner_radius = corner;
    w.active.weak_bg_fill = Color32::from_rgb(0x22, 0x3c, 0x63);
    w.active.bg_fill = Color32::from_rgb(0x2a, 0x4a, 0x7a);
    w.active.bg_stroke = Stroke::new(1.0, FOCUS);
    w.active.fg_stroke = Stroke::new(2.0, Color32::WHITE);
    w.active.corner_radius = corner;
    w.open.weak_bg_fill = INK_3;
    w.open.bg_fill = INK_2;
    w.open.bg_stroke = Stroke::new(1.0, INK_4);
    w.open.fg_stroke = Stroke::new(1.0, FG);
    w.open.corner_radius = corner;
    v
}

/// The light theme: egui's, with the same accents.
pub fn light() -> egui::Visuals {
    let mut v = egui::Visuals::light();
    let corner = CornerRadius::same(3);
    v.window_corner_radius = CornerRadius::same(4);
    v.menu_corner_radius = CornerRadius::same(4);
    v.selection.bg_fill = Color32::from_rgb(0xb9, 0xd3, 0xfb);
    v.selection.stroke = Stroke::new(1.0, Color32::from_rgb(0x0e, 0x2a, 0x55));
    v.hyperlink_color = Color32::from_rgb(0x1d, 0x5f, 0xc8);
    v.warn_fg_color = Color32::from_rgb(0x9a, 0x6a, 0x10);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = corner;
    }
    v.widgets.active.bg_stroke = Stroke::new(1.0, FOCUS);
    // Buttons and check boxes keep an edge on the light panels.
    v.widgets.inactive.weak_bg_fill = Color32::from_gray(226);
    v.widgets.inactive.bg_fill = Color32::from_gray(236);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_gray(196));
    v
}

/// The bar above or below the dock (menu, status): a shade darker than the
/// panels.
pub fn bar_fill(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode {
        INK_0
    } else {
        Color32::from_gray(232)
    }
}

/// The toolbar: between the bars and the panels.
pub fn toolbar_fill(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode {
        INK_2
    } else {
        Color32::from_gray(242)
    }
}

/// The bar of a tab group and of a drawer's title.
pub fn tab_bar_fill(visuals: &egui::Visuals) -> Color32 {
    if visuals.dark_mode {
        INK_1
    } else {
        Color32::from_gray(225)
    }
}

/// An icon of the toolbar or the status bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Save,
    Select,
    Move,
    Rotate,
    Scale,
    /// World axes.
    World,
    /// Axes of the object.
    Local,
    Snap,
    /// A cube: an object of the level.
    Object,
    Play,
    Apply,
    Folder,
    Undo,
    Log,
    Link,
    /// Shown in the view (the Outliner's eye).
    Eye,
    /// Hidden in the view.
    EyeClosed,
    /// Locked: clicks in the view do not select it.
    Lock,
    Unlock,
    /// A Details panel kept on what it shows.
    Pin,
    /// Frame the selection (F).
    Frame,
    Copy,
    Export,
    /// The Content Browser: tiles.
    Browse,
    /// A graph of nodes.
    Graph,
    Redo,
}

impl Icon {
    /// Every icon, for the tests.
    #[cfg(test)]
    pub const ALL: [Icon; 26] = [
        Icon::Frame,
        Icon::Copy,
        Icon::Export,
        Icon::Browse,
        Icon::Graph,
        Icon::Redo,
        Icon::Save,
        Icon::Select,
        Icon::Move,
        Icon::Rotate,
        Icon::Scale,
        Icon::World,
        Icon::Local,
        Icon::Snap,
        Icon::Object,
        Icon::Play,
        Icon::Apply,
        Icon::Folder,
        Icon::Undo,
        Icon::Log,
        Icon::Link,
        Icon::Eye,
        Icon::EyeClosed,
        Icon::Lock,
        Icon::Unlock,
        Icon::Pin,
    ];
}

/// Draws `icon` into `rect` (designed on a 16×16 grid).
pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height()) / 16.0;
    let o = rect.center() - vec2(8.0, 8.0) * s;
    let p = |x: f32, y: f32| -> Pos2 { o + vec2(x, y) * s };
    let stroke = Stroke::new(1.4 * s.max(0.8), color);
    let line = |points: &[(f32, f32)]| {
        painter.add(egui::Shape::line(
            points.iter().map(|&(x, y)| p(x, y)).collect(),
            stroke,
        ));
    };
    let closed = |points: &[(f32, f32)]| {
        painter.add(egui::Shape::closed_line(
            points.iter().map(|&(x, y)| p(x, y)).collect(),
            stroke,
        ));
    };
    match icon {
        Icon::Save => {
            closed(&[
                (2.5, 2.5),
                (11.5, 2.5),
                (13.5, 4.5),
                (13.5, 13.5),
                (2.5, 13.5),
            ]);
            line(&[(5.0, 2.5), (5.0, 6.5), (10.0, 6.5), (10.0, 2.5)]);
            line(&[(5.0, 13.5), (5.0, 9.5), (11.0, 9.5), (11.0, 13.5)]);
        }
        Icon::Select => {
            let arrow = vec![p(3.0, 2.0), p(12.0, 7.0), p(8.0, 8.2), p(6.0, 13.0)];
            painter.add(egui::Shape::convex_polygon(
                arrow.clone(),
                color.gamma_multiply(0.3),
                Stroke::NONE,
            ));
            painter.add(egui::Shape::closed_line(arrow, stroke));
        }
        Icon::Move => {
            line(&[(8.0, 1.5), (8.0, 14.5)]);
            line(&[(1.5, 8.0), (14.5, 8.0)]);
            for (a, b, c) in [
                ((6.0, 3.5), (8.0, 1.5), (10.0, 3.5)),
                ((6.0, 12.5), (8.0, 14.5), (10.0, 12.5)),
                ((3.5, 6.0), (1.5, 8.0), (3.5, 10.0)),
                ((12.5, 6.0), (14.5, 8.0), (12.5, 10.0)),
            ] {
                line(&[a, b, c]);
            }
        }
        Icon::Rotate => {
            let points: Vec<(f32, f32)> = (0..=20)
                .map(|i| {
                    let a = -0.75 + i as f32 / 20.0 * 5.2;
                    (8.0 + 5.0 * a.cos(), 8.0 + 5.0 * a.sin())
                })
                .collect();
            line(&points);
            line(&[(12.2, 1.8), (12.2, 4.8), (9.2, 4.8)]);
        }
        Icon::Scale => {
            closed(&[(2.0, 7.0), (9.0, 7.0), (9.0, 14.0), (2.0, 14.0)]);
            line(&[(8.0, 2.0), (14.0, 2.0), (14.0, 8.0)]);
            line(&[(14.0, 2.0), (9.0, 7.0)]);
        }
        Icon::World => {
            painter.circle_stroke(p(8.0, 8.0), 6.0 * s, stroke);
            line(&[(2.0, 8.0), (14.0, 8.0)]);
            let meridian = |dir: f32| -> Vec<(f32, f32)> {
                (0..=12)
                    .map(|i| {
                        let t = i as f32 / 12.0 * std::f32::consts::PI;
                        (8.0 + dir * 2.6 * t.sin(), 8.0 - 6.0 * t.cos())
                    })
                    .collect()
            };
            line(&meridian(1.0));
            line(&meridian(-1.0));
        }
        Icon::Local | Icon::Object => {
            closed(&[
                (8.0, 1.8),
                (13.5, 4.8),
                (13.5, 11.2),
                (8.0, 14.2),
                (2.5, 11.2),
                (2.5, 4.8),
            ]);
            line(&[(2.5, 4.8), (8.0, 8.0), (13.5, 4.8)]);
            line(&[(8.0, 8.0), (8.0, 14.2)]);
        }
        Icon::Snap => {
            line(&[(2.0, 5.5), (14.0, 5.5)]);
            line(&[(2.0, 10.5), (14.0, 10.5)]);
            line(&[(5.5, 2.0), (5.5, 14.0)]);
            line(&[(10.5, 2.0), (10.5, 14.0)]);
        }
        Icon::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![p(4.0, 2.5), p(13.0, 8.0), p(4.0, 13.5)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Apply => line(&[(3.0, 8.5), (6.0, 11.5), (13.0, 4.5)]),
        Icon::Folder => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    p(1.5, 3.5),
                    p(6.5, 3.5),
                    p(8.0, 5.0),
                    p(14.5, 5.0),
                    p(14.5, 13.0),
                    p(1.5, 13.0),
                ],
                color.gamma_multiply(0.75),
                Stroke::NONE,
            ));
        }
        Icon::Undo => {
            line(&[(5.0, 3.0), (2.0, 6.0), (5.0, 9.0)]);
            let mut arc = vec![(2.0, 6.0), (9.5, 6.0)];
            arc.extend((0..=8).map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 + i as f32 / 8.0 * std::f32::consts::PI;
                (9.5 + 4.0 * a.cos(), 10.0 + 4.0 * a.sin())
            }));
            arc.push((7.0, 14.0));
            line(&arc);
        }
        Icon::Log => {
            closed(&[(2.5, 2.5), (13.5, 2.5), (13.5, 13.5), (2.5, 13.5)]);
            line(&[(5.0, 6.0), (11.0, 6.0)]);
            line(&[(5.0, 8.5), (11.0, 8.5)]);
            line(&[(5.0, 11.0), (9.0, 11.0)]);
        }
        Icon::Link => {
            painter.circle_stroke(p(5.5, 10.5), 3.0 * s, stroke);
            painter.circle_stroke(p(10.5, 5.5), 3.0 * s, stroke);
            line(&[(7.0, 9.0), (9.0, 7.0)]);
        }
        Icon::Eye | Icon::EyeClosed => {
            let lid = |dir: f32| -> Vec<(f32, f32)> {
                (0..=12)
                    .map(|i| {
                        let t = i as f32 / 12.0 * std::f32::consts::PI;
                        (8.0 - 6.5 * t.cos(), 8.0 + dir * 4.0 * t.sin())
                    })
                    .collect()
            };
            line(&lid(-1.0));
            line(&lid(1.0));
            if icon == Icon::Eye {
                painter.circle_filled(p(8.0, 8.0), 2.2 * s, color);
            } else {
                line(&[(2.5, 13.5), (13.5, 2.5)]);
            }
        }
        Icon::Lock | Icon::Unlock => {
            closed(&[(3.5, 7.5), (12.5, 7.5), (12.5, 14.0), (3.5, 14.0)]);
            let right = if icon == Icon::Lock { 11.0 } else { 14.5 };
            let left = if icon == Icon::Lock { 5.0 } else { 8.5 };
            let mut arc = vec![(left, 7.5)];
            arc.extend((0..=8).map(|i| {
                let a = std::f32::consts::PI + i as f32 / 8.0 * std::f32::consts::PI;
                let (cx, r) = ((left + right) / 2.0, (right - left) / 2.0);
                (cx + r * a.cos(), 5.0 + 3.0 * a.sin())
            }));
            arc.push((right, if icon == Icon::Lock { 7.5 } else { 5.5 }));
            line(&arc);
        }
        Icon::Pin => {
            closed(&[
                (5.5, 2.0),
                (10.5, 2.0),
                (10.0, 7.5),
                (12.5, 10.0),
                (3.5, 10.0),
                (6.0, 7.5),
            ]);
            line(&[(8.0, 10.0), (8.0, 14.5)]);
        }
        Icon::Frame => {
            for (a, b, c) in [
                ((2.0, 6.0), (2.0, 2.0), (6.0, 2.0)),
                ((10.0, 2.0), (14.0, 2.0), (14.0, 6.0)),
                ((14.0, 10.0), (14.0, 14.0), (10.0, 14.0)),
                ((6.0, 14.0), (2.0, 14.0), (2.0, 10.0)),
            ] {
                line(&[a, b, c]);
            }
            painter.circle_filled(p(8.0, 8.0), 1.8 * s, color);
        }
        Icon::Copy => {
            closed(&[(5.5, 5.5), (13.5, 5.5), (13.5, 14.0), (5.5, 14.0)]);
            line(&[
                (3.0, 11.0),
                (2.5, 11.0),
                (2.5, 2.5),
                (10.5, 2.5),
                (10.5, 3.0),
            ]);
        }
        Icon::Export => {
            line(&[
                (5.0, 7.0),
                (2.5, 7.0),
                (2.5, 14.0),
                (13.5, 14.0),
                (13.5, 7.0),
                (11.0, 7.0),
            ]);
            line(&[(8.0, 10.5), (8.0, 1.5)]);
            line(&[(5.0, 4.5), (8.0, 1.5), (11.0, 4.5)]);
        }
        Icon::Browse => {
            for (x, y) in [(2.0, 2.0), (9.0, 2.0), (2.0, 9.0), (9.0, 9.0)] {
                closed(&[(x, y), (x + 5.0, y), (x + 5.0, y + 5.0), (x, y + 5.0)]);
            }
        }
        Icon::Graph => {
            closed(&[(1.5, 2.5), (6.5, 2.5), (6.5, 6.5), (1.5, 6.5)]);
            closed(&[(9.5, 9.5), (14.5, 9.5), (14.5, 13.5), (9.5, 13.5)]);
            line(&[(6.5, 4.5), (8.5, 4.5), (8.5, 11.5), (9.5, 11.5)]);
        }
        Icon::Redo => {
            line(&[(11.0, 3.0), (14.0, 6.0), (11.0, 9.0)]);
            let mut arc = vec![(14.0, 6.0), (6.5, 6.0)];
            arc.extend((0..=8).map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 - i as f32 / 8.0 * std::f32::consts::PI;
                (6.5 + 4.0 * a.cos(), 10.0 + 4.0 * a.sin())
            }));
            arc.push((9.0, 14.0));
            line(&arc);
        }
    }
}

/// The bar of tools of a document (stage 56): the strip under the tab with
/// the tools of the editor in it, in the colours and spacing of the main
/// toolbar.
pub fn doc_bar<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let fill = toolbar_fill(ui.visuals());
    egui::Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(4, 3))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.horizontal_wrapped(add).inner
        })
        .inner
}

/// A button "text ▾" (with an icon) that opens a menu, as "Show ▾" over
/// the level view.
pub fn menu_button(
    ui: &mut Ui,
    icon: Option<Icon>,
    text: &str,
    hint: &str,
    contents: impl FnOnce(&mut Ui),
) -> Response {
    let button = tool_button(ui, icon, &format!("{text} ▾"), false, Tone::Plain);
    let button = if hint.is_empty() {
        button
    } else {
        button.on_hover_text(hint)
    };
    egui::Popup::menu(&button)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(contents);
    button
}

/// An icon button with a hint; true when clicked.
pub fn icon_button(ui: &mut Ui, icon: Icon, hint: &str) -> bool {
    tool_button(ui, Some(icon), "", false, Tone::Plain)
        .on_hover_text(hint)
        .clicked()
}

/// "?" at the end of a bar: the keys of the editor, (keys, text key).
pub fn help_button(ui: &mut Ui, keys: &[(&str, &str)]) {
    let help = tool_button(ui, None, "?", false, Tone::Plain)
        .on_hover_text(crate::i18n::tr("doc.keys_hint"));
    egui::Popup::menu(&help).show(|ui| keys_grid(ui, keys));
}

/// The keys of an editor as a table (also in the shortcut window).
pub fn keys_grid(ui: &mut Ui, keys: &[(&str, &str)]) {
    egui::Grid::new(ui.next_auto_id())
        .num_columns(2)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
            for (key, what) in keys {
                ui.monospace(*key);
                ui.label(crate::i18n::tr(what));
                ui.end_row();
            }
        });
}

/// How a tool button looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Plain,
    /// The main action: blue.
    Primary,
    /// Play: green.
    Play,
    /// A bare button of the status bar.
    Flat,
}

/// A button of the toolbar or the status bar: an icon, a text or both;
/// `selected` marks a switch that is on.
pub fn tool_button(
    ui: &mut Ui,
    icon: Option<Icon>,
    text: &str,
    selected: bool,
    tone: Tone,
) -> Response {
    let enabled = ui.is_enabled();
    tool_button_if(ui, enabled, icon, text, selected, tone)
}

/// [`tool_button`] that is greyed out and does not click unless `enabled`
/// — without a child `Ui`, so a wrapping bar of tools wraps it as any other
/// button (stage 56).
pub fn tool_button_if(
    ui: &mut Ui,
    enabled: bool,
    icon: Option<Icon>,
    text: &str,
    selected: bool,
    tone: Tone,
) -> Response {
    let enabled = enabled && ui.is_enabled();
    let height = if tone == Tone::Flat { 20.0 } else { 26.0 };
    let icon_size = if tone == Tone::Flat { 13.0 } else { 16.0 };
    // A trailing "▾" is drawn, not written: the default fonts lack it.
    let (text, chevron) = match text.strip_suffix('▾') {
        Some(rest) => (rest.trim_end(), true),
        None => (text, false),
    };
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = (!text.is_empty()).then(|| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER)
    });
    let pad = if tone == Tone::Flat { 6.0 } else { 7.0 };
    let gap = 5.0;
    let chevron_size = 7.0;
    let mut width = pad * 2.0;
    if icon.is_some() {
        width += icon_size;
    }
    if let Some(g) = &galley {
        width += g.size().x + if icon.is_some() { gap } else { 0.0 };
    }
    if chevron {
        width += chevron_size
            + if icon.is_some() || galley.is_some() {
                gap
            } else {
                0.0
            };
    }
    let min_width = if icon.is_none() && galley.is_none() {
        16.0
    } else {
        height + 2.0
    };
    let width = width.max(min_width);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), sense);
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let dark = visuals.dark_mode;
        let hovered = response.hovered() && enabled;
        let (fill, stroke, fg) = match tone {
            Tone::Play => (
                if hovered { PLAY_HOVER } else { PLAY },
                Stroke::NONE,
                Color32::from_rgb(0xe8, 0xf5, 0xea),
            ),
            Tone::Primary => (
                if hovered {
                    Color32::from_rgb(0x55, 0x9b, 0xff)
                } else {
                    FOCUS
                },
                Stroke::NONE,
                Color32::WHITE,
            ),
            Tone::Plain | Tone::Flat => {
                let w = if hovered {
                    &visuals.widgets.hovered
                } else {
                    &visuals.widgets.inactive
                };
                let fill = if selected {
                    FOCUS.gamma_multiply(if dark { 0.22 } else { 0.25 })
                } else if tone == Tone::Flat && !hovered {
                    Color32::TRANSPARENT
                } else {
                    w.weak_bg_fill
                };
                let stroke = if selected {
                    Stroke::new(1.0, FOCUS)
                } else {
                    Stroke::NONE
                };
                let fg = if selected || hovered {
                    w.fg_stroke.color
                } else if dark {
                    FG_DIM
                } else {
                    Color32::from_gray(60)
                };
                (fill, stroke, fg)
            }
        };
        let fg = if enabled { fg } else { fg.gamma_multiply(0.45) };
        let fill = if enabled {
            fill
        } else {
            fill.gamma_multiply(0.6)
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(3),
            fill,
            stroke,
            egui::StrokeKind::Inside,
        );
        let mut x = rect.left() + pad;
        if let Some(icon) = icon {
            let icon_rect = Rect::from_min_size(
                pos2(x, rect.center().y - icon_size / 2.0),
                Vec2::splat(icon_size),
            );
            paint_icon(ui.painter(), icon_rect, icon, fg);
            x += icon_size + gap;
        }
        if let Some(g) = galley {
            let y = rect.center().y - g.size().y / 2.0;
            let w = g.size().x;
            ui.painter().galley(pos2(x, y), g, fg);
            x += w + gap;
        }
        if chevron {
            let c = pos2(
                if icon.is_none() && text.is_empty() {
                    rect.center().x
                } else {
                    x + chevron_size / 2.0
                },
                rect.center().y + 1.0,
            );
            let h = chevron_size / 2.0;
            ui.painter().add(egui::Shape::convex_polygon(
                vec![
                    pos2(c.x - h, c.y - h * 0.6),
                    pos2(c.x + h, c.y - h * 0.6),
                    pos2(c.x, c.y + h * 0.6),
                ],
                fg,
                Stroke::NONE,
            ));
        }
    }
    response
}

/// A small "▾" button that opens a menu.
pub fn menu_arrow(ui: &mut Ui, tone: Tone) -> Response {
    tool_button(ui, None, "▾", false, tone)
}

/// A thin vertical separator of the toolbar.
pub fn separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 22.0), Sense::hover());
    let color = ui.visuals().widgets.noninteractive.bg_stroke.color;
    ui.painter()
        .vline(rect.center().x, rect.y_range(), Stroke::new(1.0, color));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_keep_light_and_dark_apart() {
        let dark = dark();
        let light = light();
        assert!(dark.dark_mode && !light.dark_mode);
        // Text stays readable on the panels of both themes.
        let contrast = |a: Color32, b: Color32| {
            let l = |c: Color32| c.r() as f32 * 0.3 + c.g() as f32 * 0.59 + c.b() as f32 * 0.11;
            (l(a) - l(b)).abs()
        };
        assert!(contrast(dark.text_color(), dark.panel_fill) > 120.0);
        assert!(contrast(light.text_color(), light.panel_fill) > 120.0);
        assert_eq!(dark.widgets.active.bg_stroke.color, FOCUS);
    }

    #[test]
    fn icons_paint_inside_their_rect() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let rect = Rect::from_min_size(pos2(10.0, 10.0), Vec2::splat(16.0));
            for icon in Icon::ALL {
                let painter = ui.painter().with_clip_rect(rect);
                paint_icon(&painter, rect, icon, Color32::WHITE);
            }
        });
        output.textures_delta.clear();
    }
}
