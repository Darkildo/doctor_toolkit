//! Drop-down lists of the typed game data (`doctor_toolkit::game_types`):
//! message names for message pins and the values of a list for attributes
//! that hold one. Used by the logic editor and the script editor.

use doctor_toolkit::game_types::{EnumDef, EnumKind, MessageCatalog, MessageId, message};
use doctor_toolkit_mods::i18n::tr;
use egui::RichText;

/// Engine messages listed before the messages of the game files.
const ENGINE_SHOWN: usize = 12;
/// Messages of the game files listed.
const GAME_SHOWN: usize = 28;

/// A `▾` button listing message names that contain `typed` (engine messages
/// first, then messages of the game files). Returns the name picked.
pub fn message_menu(ui: &mut egui::Ui, typed: &str) -> Option<String> {
    let mut picked = None;
    ui.menu_button("▾", |ui| {
        ui.set_min_width(320.0);
        let needle = typed.trim().to_ascii_lowercase();
        if needle.is_empty() {
            ui.label(RichText::new(tr("logic.pick_hint")).weak());
        }
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                let mut engine: Vec<&str> = message::engine_messages()
                    .iter()
                    .map(|e| e.1)
                    .filter(|n| n.to_ascii_lowercase().contains(&needle))
                    .collect();
                engine.sort_by_key(|n| n.to_ascii_lowercase());
                let more_engine = engine.len().saturating_sub(ENGINE_SHOWN);
                for name in engine.into_iter().take(ENGINE_SHOWN) {
                    if ui
                        .button(RichText::new(name).strong())
                        .on_hover_text(hover(name))
                        .clicked()
                    {
                        picked = Some(name.to_owned());
                    }
                }
                if more_engine > 0 {
                    ui.label(RichText::new(format!("… +{more_engine}")).weak());
                }
                if !needle.is_empty() {
                    ui.separator();
                    let found = MessageCatalog::gf2().search(&needle, GAME_SHOWN + 1);
                    let more = found.len() > GAME_SHOWN;
                    for m in found.into_iter().take(GAME_SHOWN) {
                        if message::engine_name(m.hash).is_some() {
                            continue;
                        }
                        if ui.button(m.name).on_hover_text(hover(m.name)).clicked() {
                            picked = Some(m.name.to_owned());
                        }
                    }
                    if more {
                        ui.label(RichText::new("…").weak());
                    }
                }
            });
        if picked.is_some() {
            ui.close();
        }
    })
    .response
    .on_hover_text(tr("logic.pick_message"));
    picked
}

/// What is known about a message, for a tooltip.
fn hover(name: &str) -> String {
    let info = MessageId::from_name(name).info();
    let count = |uses: &[doctor_toolkit::game_types::PinUse]| -> u64 {
        uses.iter().map(|u| u64::from(u.count)).sum()
    };
    let mut text = format!(
        "{} · sdbm {:08x}\n{}: {} · {}: {}",
        info.kind,
        info.hash,
        tr("logic.sent_by"),
        count(&info.senders),
        tr("logic.heard_by"),
        count(&info.listeners),
    );
    if let Some(scope) = info.scope {
        text.push_str(&format!("\n{scope}"));
    }
    if let Some(summary) = info.summary {
        text.push_str(&format!("\n{summary}"));
    }
    text
}

/// A `▾` button listing the values of a list. Returns the text of the value
/// picked, as a value field takes it (a hash as eight hexadecimal digits, a
/// number, a name).
pub fn enum_menu(ui: &mut egui::Ui, list: &'static EnumDef) -> Option<String> {
    let mut picked = None;
    ui.menu_button("▾", |ui| {
        ui.set_min_width(240.0);
        ui.label(RichText::new(list.title).weak());
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                for v in list.values {
                    let label = match (v.name, v.value, list.kind) {
                        (Some(n), Some(x), EnumKind::Bits) => format!("{n} ({x:#04x})"),
                        (Some(n), _, _) => n.to_owned(),
                        (None, Some(x), _) => x.to_string(),
                        (None, None, _) => continue,
                    };
                    let mut button = ui.button(label);
                    if !v.note.is_empty() {
                        button = button.on_hover_text(v.note);
                    }
                    if button.clicked() {
                        picked = list.stored_text(v);
                    }
                }
            });
        if picked.is_some() {
            ui.close();
        }
    })
    .response
    .on_hover_text(tr("logic.pick_value"));
    picked
}
