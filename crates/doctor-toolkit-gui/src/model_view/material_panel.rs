//! Material inspector: shaders, the shading the viewer derives and every
//! parameter and sampler of the material of one part.

use doctor_toolkit::resource::shading::{Blend, Family};
use doctor_toolkit_mods::i18n::{tr, trf};
use egui::{Color32, RichText};

use super::load::PartInfo;

/// A colour swatch for a linear colour.
fn swatch(ui: &mut egui::Ui, linear: [f32; 3]) {
    let srgb = linear.map(|c| (c.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 14.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 2.0, Color32::from_rgb(srgb[0], srgb[1], srgb[2]));
    response.on_hover_text(trf(
        "material.linear",
        &[(
            "rgb",
            &format!("{:.3} {:.3} {:.3}", linear[0], linear[1], linear[2]),
        )],
    ));
}

fn family_name(family: Family) -> &'static str {
    match family {
        Family::TintMask => tr("material.family_tint_mask"),
        Family::Hair => tr("material.family_hair"),
        Family::PlainHair => tr("material.family_plain_hair"),
        Family::Skin => tr("material.family_skin"),
        Family::Eye => tr("material.family_eye"),
        Family::Glass => tr("material.family_glass"),
        Family::Basic => tr("material.family_basic"),
    }
}

/// Shows the material of `info`; `id` keeps the widgets of several panels apart.
pub fn show(ui: &mut egui::Ui, info: &PartInfo, id: impl std::hash::Hash + std::fmt::Debug) {
    let name = info.material.clone().unwrap_or_else(|| {
        trf(
            "model.not_found",
            &[("hash", &format!("{:08x}", info.material_hash))],
        )
    });
    ui.horizontal_wrapped(|ui| {
        ui.strong(&name);
        if let Some(from) = info.swapped_from {
            ui.label(
                RichText::new(trf(
                    "material.swapped_from",
                    &[("hash", &format!("{from:08x}"))],
                ))
                .weak(),
            );
        }
    });
    if info.material.is_some()
        && ui
            .small_button(tr("material.edit"))
            .on_hover_text(tr("material.edit_hint"))
            .clicked()
    {
        crate::material_view::ask_edit(ui.ctx(), info.material_hash);
    }
    let Some(shading) = &info.shading else {
        return;
    };
    egui::Grid::new(("material shading", &id))
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            let slots = [
                "material.slot_depth",
                "material.slot_lit",
                "material.slot_normals",
                "material.slot_low",
                "material.slot_shadow",
            ];
            for (slot, shader) in slots.iter().zip(&info.shaders) {
                ui.label(tr(slot));
                ui.monospace(shader);
                ui.end_row();
            }
            ui.label(tr("material.shading"));
            ui.label(family_name(shading.family));
            ui.end_row();
            ui.label(tr("material.blending"));
            ui.label(match shading.blend {
                Blend::Opaque => tr("material.opaque"),
                Blend::Alpha => tr("material.alpha_blended"),
            });
            ui.end_row();
            if shading.family == Family::TintMask || shading.family == Family::Hair {
                ui.label(tr("material.tints"));
                ui.horizontal(|ui| {
                    for t in shading.tints {
                        swatch(ui, t);
                    }
                });
                ui.end_row();
            }
            if shading.family == Family::Skin || shading.family == Family::Eye {
                ui.label(tr("material.skin_tint"));
                swatch(ui, shading.skin_tint);
                ui.end_row();
            }
            ui.label(tr("material.specular"));
            ui.horizontal(|ui| {
                swatch(ui, shading.specular);
                ui.label(trf(
                    "material.exponent",
                    &[("value", &format!("{:.1}", shading.specular_power))],
                ));
            });
            ui.end_row();
        });
    egui::CollapsingHeader::new(trf("material.parameters", &[("count", &info.params.len())]))
        .id_salt(("material params", &id))
        .default_open(true)
        .show(ui, |ui| {
            egui::Grid::new(("material params grid", &id))
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    for (name, value, source) in &info.params {
                        ui.monospace(name);
                        let label = ui.monospace(value);
                        if !source.is_empty() {
                            label.on_hover_text(source);
                        }
                        ui.end_row();
                    }
                });
        });
}
