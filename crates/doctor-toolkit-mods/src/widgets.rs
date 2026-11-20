//! Small widgets of the mod cards: the on/off switch, the state chip, the
//! drag handle, the picture placeholder and the big action buttons.

use egui::{
    Color32, CornerRadius, FontId, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
    pos2, vec2,
};

/// An on/off switch; returns its response, `changed` when flipped.
pub fn switch(ui: &mut Ui, on: &mut bool, enabled: bool) -> Response {
    let size = vec2(36.0, 20.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut response) = ui.allocate_exact_size(size, sense);
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, *on, ""));
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let visuals = ui.style().interact_selectable(&response, *on);
        let radius = rect.height() / 2.0;
        let fill = if *on {
            crate::theme::FOCUS.gamma_multiply(if enabled { 1.0 } else { 0.5 })
        } else {
            visuals.bg_fill
        };
        ui.painter()
            .rect(rect, radius, fill, visuals.bg_stroke, StrokeKind::Inside);
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        let knob = if enabled {
            Color32::from_gray(0xf0)
        } else {
            Color32::from_gray(0x90)
        };
        ui.painter()
            .circle(pos2(x, rect.center().y), radius - 3.0, knob, Stroke::NONE);
    }
    response
}

/// A rounded label in the colour of a state.
pub fn chip(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = FontId::proportional(12.5);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, color.gamma_multiply(1.15));
    let padding = vec2(8.0, 3.0);
    let (rect, response) = ui.allocate_exact_size(galley.size() + padding * 2.0, Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect(
            rect,
            CornerRadius::same(10),
            color.gamma_multiply(0.18),
            Stroke::new(1.0, color.gamma_multiply(0.6)),
            StrokeKind::Inside,
        );
        ui.painter()
            .galley(rect.min + padding, galley, Color32::PLACEHOLDER);
    }
    response
}

/// Six dots to drag a card by.
pub fn paint_handle(ui: &Ui, rect: Rect, hovered: bool) {
    let color = if hovered {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let c = rect.center();
    for dx in [-3.0, 3.0] {
        for dy in [-6.0, 0.0, 6.0] {
            ui.painter()
                .circle_filled(pos2(c.x + dx, c.y + dy), 1.6, color);
        }
    }
}

/// The picture of a mod without one: a dark tile with the first letter of
/// its name.
pub fn paint_placeholder(ui: &Ui, rect: Rect, name: &str) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(4), ui.visuals().extreme_bg_color);
    let letter = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map_or_else(|| "?".to_owned(), |c| c.to_uppercase().to_string());
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        FontId::proportional(rect.height() * 0.45),
        ui.visuals().weak_text_color(),
    );
}

/// How a big action button looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prominence {
    /// The main action: green, like the editor's Play button.
    Main,
    /// Other actions of the bar.
    Normal,
}

/// A big button of the action bar.
pub fn action_button(ui: &mut Ui, enabled: bool, text: &str, prominence: Prominence) -> Response {
    let rich = RichText::new(text).size(15.0);
    let mut button = egui::Button::new(match prominence {
        Prominence::Main => rich.color(Color32::WHITE).strong(),
        Prominence::Normal => rich,
    })
    .min_size(Vec2::new(0.0, 32.0))
    .corner_radius(CornerRadius::same(4));
    // In Open Mod Manager the main action is the rust of the game's panels.
    let gf2 = crate::skin::active(ui.ctx());
    let (fill, hover) = if gf2 {
        (crate::skin::RUST, crate::skin::OXBLOOD)
    } else {
        (crate::theme::PLAY, crate::theme::PLAY_HOVER)
    };
    if prominence == Prominence::Main && enabled {
        button = button.fill(fill);
        if gf2 {
            button = button.corner_radius(CornerRadius::ZERO);
        }
    }
    let response = ui.add_enabled(enabled, button);
    if prominence == Prominence::Main && enabled && response.hovered() {
        ui.painter().rect_stroke(
            response.rect,
            CornerRadius::same(if gf2 { 0 } else { 4 }),
            Stroke::new(1.0, hover),
            StrokeKind::Inside,
        );
    }
    response
}

/// Paints `texture` into `rect`, cropped to fill it.
pub fn paint_picture(ui: &Ui, rect: Rect, texture: &egui::TextureHandle) {
    let size = texture.size_vec2();
    if size.x <= 0.0 || size.y <= 0.0 {
        return;
    }
    let (a, t) = (size.x / size.y, rect.width() / rect.height());
    let uv = if a > t {
        let cut = (1.0 - t / a) / 2.0;
        Rect::from_min_max(pos2(cut, 0.0), pos2(1.0 - cut, 1.0))
    } else {
        let cut = (1.0 - a / t) / 2.0;
        Rect::from_min_max(pos2(0.0, cut), pos2(1.0, 1.0 - cut))
    };
    ui.painter().image(texture.id(), rect, uv, Color32::WHITE);
}

/// A warning triangle with an exclamation mark, for a mod in conflict.
pub fn warning_icon(ui: &mut Ui, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let r = rect.shrink(1.0);
        let points = vec![
            pos2(r.center().x, r.top()),
            pos2(r.right(), r.bottom()),
            pos2(r.left(), r.bottom()),
        ];
        ui.painter()
            .add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
        let dark = Color32::from_gray(0x18);
        let x = r.center().x;
        ui.painter().line_segment(
            [pos2(x, r.top() + 5.0), pos2(x, r.bottom() - 5.0)],
            Stroke::new(1.6, dark),
        );
        ui.painter()
            .circle_filled(pos2(x, r.bottom() - 2.6), 1.0, dark);
    }
    response
}

/// A window over the panel, with room around its text.
pub fn modal(ctx: &egui::Context, id: &str) -> egui::Modal {
    let style = ctx.style_of(ctx.theme());
    egui::Modal::new(egui::Id::unique(id)).frame(
        egui::Frame::window(&style)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::same(18)),
    )
}

/// The main action with the Play icon of the editor's toolbar.
pub fn play_button(ui: &mut Ui, enabled: bool, text: &str) -> Response {
    let response = action_button(ui, enabled, &format!("      {text}"), Prominence::Main);
    let color = if enabled {
        Color32::WHITE
    } else {
        ui.visuals().weak_text_color()
    };
    let icon = Rect::from_center_size(
        pos2(response.rect.left() + 18.0, response.rect.center().y),
        vec2(16.0, 16.0),
    );
    crate::theme::paint_icon(ui.painter(), icon, crate::theme::Icon::Play, color);
    response
}
