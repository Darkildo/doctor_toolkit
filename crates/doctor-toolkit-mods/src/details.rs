//! The selected mod in full (big picture, description, state and what to
//! do), and the folded "Details" section with the technical side: streams,
//! resource keys, conflicts per resource, the files of the integrity check.

use egui::RichText;

use egui::Color32;

use doctor_toolkit::mods::ModManager;
use doctor_toolkit::mods::integrity::Condition;
use doctor_toolkit::mods::manager::{
    CompatIssue, InstalledMod, Relation, RelationKind, RelationState,
};
use doctor_toolkit::mods::version::Condition as Versions;

use crate::i18n::{tr, trf, trn};
use crate::state::{ModState, versions_text};
use crate::{BAD, GOOD, ModsPanel, WARN, cards, condition_text, widgets};

/// What the buttons of the selected mod ask for.
enum Ask {
    Toggle(String, bool),
    Remove(String),
    EnableWith(String),
    /// The answer about the executable code of the mod.
    Code(String, crate::native::CodeAnswer),
}

/// A relation of a mod in words, with the colour of its state.
pub(crate) fn relation_words(r: &Relation) -> (String, Color32) {
    let mut name = r.name.clone().unwrap_or_else(|| r.id.clone());
    if let Ok(c) = Versions::parse(&r.versions)
        && !c.is_any()
    {
        name = format!("{name} {}", versions_text(&c));
    }
    let kind = tr(match r.kind {
        RelationKind::Depends => "mods.rel_depends",
        RelationKind::Recommends => "mods.rel_recommends",
        RelationKind::Incompatible => "mods.rel_incompatible",
        RelationKind::LoadAfter => "mods.rel_load_after",
        RelationKind::LoadBefore => "mods.rel_load_before",
        RelationKind::NeededBy => "mods.rel_needed_by",
    });
    let gray = Color32::GRAY;
    let (state, color) = match (&r.kind, &r.state) {
        (RelationKind::Incompatible, RelationState::Clashing) => {
            (tr("mods.rel_both_on").to_owned(), BAD)
        }
        (RelationKind::Incompatible, RelationState::Version(found)) => {
            (trf("mods.rel_other_version", &[("version", found)]), GOOD)
        }
        (RelationKind::Incompatible, _) => (tr("mods.rel_not_on").to_owned(), GOOD),
        (RelationKind::NeededBy, RelationState::Off) => (tr("mods.rel_off").to_owned(), gray),
        (RelationKind::NeededBy, _) => (tr("mods.rel_on").to_owned(), GOOD),
        (_, RelationState::Fine) => (tr("mods.rel_fine").to_owned(), GOOD),
        (RelationKind::LoadAfter | RelationKind::LoadBefore, RelationState::Missing) => {
            (tr("mods.rel_rule_idle").to_owned(), gray)
        }
        (kind, state) => {
            let color = if *kind == RelationKind::Depends {
                BAD
            } else {
                WARN
            };
            let text = match state {
                RelationState::Missing => tr("mods.rel_missing").to_owned(),
                RelationState::Off => tr("mods.rel_off").to_owned(),
                RelationState::Version(found) => trf("mods.rel_version", &[("version", found)]),
                _ => tr("mods.rel_out_of_order").to_owned(),
            };
            (text, color)
        }
    };
    let mut text = format!("{kind} {name}: {state}");
    if !r.reason.is_empty() {
        text += &format!(" ({})", r.reason);
    }
    (text, color)
}

/// The "Dependencies" part of the selected mod: what it needs, recommends,
/// does not work with, the rules of its order, the mods that need it, the
/// editions and the mod manager it is made for. Returns whether a button
/// to turn on what it needs was pressed.
fn dependencies(ui: &mut egui::Ui, manager: &ModManager, m: &InstalledMod, idle: bool) -> bool {
    let relations = manager.relations(&m.id);
    let editions = &m.manifest.game.editions;
    let required = m.manifest.requires_manager.as_deref();
    if relations.is_empty() && editions.is_empty() && required.is_none() {
        return false;
    }
    ui.add_space(6.0);
    ui.label(RichText::new(tr("mods.dependencies")).strong());
    for r in &relations {
        let (text, color) = relation_words(r);
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(color, "●");
            ui.add(egui::Label::new(text).wrap());
        });
    }
    if !editions.is_empty() {
        ui.label(
            RichText::new(trf("mods.tested_on", &[("editions", &editions.join(", "))])).weak(),
        );
    }
    if let Some(required) = required {
        let shown =
            Versions::parse(required).map_or_else(|_| required.to_owned(), |c| versions_text(&c));
        ui.label(RichText::new(trf("mods.needs_manager", &[("versions", &shown)])).weak());
    }
    let off = manager.disabled_dependencies(&m.id);
    !off.is_empty()
        && ui
            .add_enabled(idle, egui::Button::new(tr("mods.enable_needed")))
            .on_hover_text(trf(
                "mods.enable_needed_hint",
                &[(
                    "mods",
                    &off.iter()
                        .map(|id| {
                            manager
                                .get(id)
                                .map_or(id.as_str(), |x| x.manifest.name.as_str())
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                )],
            ))
            .clicked()
}

impl ModsPanel {
    pub(crate) fn details(&mut self, ui: &mut egui::Ui) {
        self.selected_mod(ui);
        let title = tr("mods.details");
        egui::CollapsingHeader::new(RichText::new(title).strong())
            .id_salt("mods-details")
            .default_open(false)
            .show(ui, |ui| {
                ui.label(RichText::new(tr("mods.details_hint")).weak().small());
                self.technical(ui);
                self.conflicts_section(ui);
                self.integrity_section(ui);
            });
    }

    /// The selected mod for players.
    fn selected_mod(&mut self, ui: &mut egui::Ui) {
        let (Some(manager), Some(id)) = (&self.manager, self.selected.clone()) else {
            return;
        };
        let Some(m) = manager.get(&id) else { return };
        let state = ModState::of(m, manager, &self.overview);
        let picture = cards::preview(&mut self.previews, ui.ctx(), m);
        let idle = self.running.is_none();
        let mut ask = None;
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    if let Some(texture) = &picture {
                        let size = texture.size_vec2();
                        let scale = (320.0 / size.x).min(180.0 / size.y).min(2.0);
                        ui.image((texture.id(), size * scale));
                        ui.add_space(8.0);
                    }
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&m.manifest.name).size(20.0).strong());
                            ui.label(RichText::new(&m.manifest.version).weak());
                        });
                        if !m.manifest.author.is_empty() {
                            ui.label(
                                RichText::new(trf("mods.by", &[("author", &m.manifest.author)]))
                                    .weak(),
                            );
                        }
                        if let Some(url) = &m.manifest.url {
                            ui.hyperlink(url);
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            widgets::chip(ui, &state.text(), state.color());
                        });
                        ui.add(
                            egui::Label::new(RichText::new(state.hint()).color(state.color()))
                                .wrap(),
                        );
                        if !m.manifest.description.is_empty() {
                            ui.add_space(4.0);
                            ui.add(egui::Label::new(&m.manifest.description).wrap());
                        }
                        if dependencies(ui, manager, m, idle) {
                            ask = Some(Ask::EnableWith(id.clone()));
                        }
                        if let Some(view) = self.overview.code.iter().find(|c| c.question.id == id)
                            && let Some(answer) = crate::native::section_ui(ui, view, idle)
                        {
                            ask = Some(Ask::Code(id.clone(), answer));
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let label = if m.enabled {
                                tr("mods.switch_off")
                            } else {
                                tr("mods.switch_on")
                            };
                            if ui
                                .add_enabled(idle && m.problem.is_none(), egui::Button::new(label))
                                .clicked()
                            {
                                ask = Some(Ask::Toggle(id.clone(), !m.enabled));
                            }
                            if ui
                                .add_enabled(idle, egui::Button::new(tr("mods.remove")))
                                .clicked()
                            {
                                ask = Some(Ask::Remove(id.clone()));
                            }
                        });
                    });
                });
            });
        ui.add_space(6.0);
        let ctx = ui.ctx().clone();
        match ask {
            Some(Ask::Toggle(id, on)) => self.set_enabled(&ctx, &id, on),
            Some(Ask::Remove(id)) => self.ask_remove(&id),
            Some(Ask::EnableWith(id)) => self.enable_with_dependencies(&ctx, &id),
            Some(Ask::Code(id, answer)) => self.code_answered(&ctx, &id, answer),
            None => {}
        }
    }

    /// Streams and resources of the selected mod.
    fn technical(&mut self, ui: &mut egui::Ui) {
        let (Some(manager), Some(id)) = (&self.manager, self.selected.clone()) else {
            return;
        };
        let Some(m) = manager.get(&id) else { return };
        ui.add_space(4.0);
        ui.strong(&m.manifest.name);
        let mut about = format!("id {}", m.id);
        if !m.source.is_empty() {
            about += ", ";
            about += &trf("mods.installed_from", &[("source", &m.source)]);
        }
        ui.label(RichText::new(about).weak());
        if let Some(problem) = &m.problem {
            ui.colored_label(BAD, problem);
        }
        ui.label(trf(
            "mods.resources_in",
            &[
                (
                    "resources",
                    &trn("count.resources", m.manifest.resources.len() as u64, &[]),
                ),
                ("streams", &m.manifest.streams().join(", ")),
            ],
        ));
        let added = m.manifest.added().count();
        if added > 0 {
            ui.label(trn("mods.adds", added as u64, &[]));
        }
        for stream in &m.manifest.streams {
            let loaded = if stream.with_parent {
                trf("mods.loaded_with", &[("parent", &stream.parent)])
            } else {
                trf("mods.loaded_after", &[("parent", &stream.parent)])
            };
            ui.label(trf(
                "mods.creates_stream",
                &[
                    ("name", &stream.name),
                    ("loaded", &loaded),
                    ("tocs", &stream.tocs.join(", ")),
                ],
            ));
        }
        let issues: Vec<String> = self
            .overview
            .issues_of(&id)
            .map(CompatIssue::describe)
            .collect();
        if !issues.is_empty() {
            ui.colored_label(WARN, trn("mods.misfits_list", issues.len() as u64, &[]));
            for issue in issues.iter().take(20) {
                ui.label(RichText::new(issue).small());
            }
        }
        egui::CollapsingHeader::new(trf("mods.resources_of", &[("name", &m.manifest.name)]))
            .id_salt(("mod-resources", &id))
            .show(ui, |ui| {
                for resource in m.manifest.resources.iter().take(500) {
                    let text = if resource.is_added() {
                        trf(
                            "mods.resource_added",
                            &[
                                ("key", &resource.key),
                                ("bytes", &trn("count.bytes", resource.payload.size, &[])),
                            ],
                        )
                    } else {
                        trf(
                            "mods.resource_replaced",
                            &[
                                ("key", &resource.key),
                                ("old", &resource.original.size),
                                ("bytes", &trn("count.bytes", resource.payload.size, &[])),
                            ],
                        )
                    };
                    ui.label(RichText::new(text).monospace().small());
                }
                if m.manifest.resources.len() > 500 {
                    ui.weak(trf(
                        "mods.and_more",
                        &[("count", &(m.manifest.resources.len() - 500))],
                    ));
                }
            });
        ui.separator();
    }

    fn conflicts_section(&mut self, ui: &mut egui::Ui) {
        let (list, clashes) = if self.show_all_conflicts {
            (&self.overview.all_conflicts, &self.overview.all_clashes)
        } else {
            (&self.overview.conflicts, &self.overview.clashes)
        };
        let title = trf(
            "mods.conflicts",
            &[("count", &(list.len() + clashes.len()))],
        );
        let mut show_all = self.show_all_conflicts;
        egui::CollapsingHeader::new(title)
            .id_salt("mods-conflicts")
            .show(ui, |ui| {
                ui.checkbox(&mut show_all, tr("mods.include_disabled"));
                for clash in clashes {
                    let (color, kind) = if clash.is_fatal() {
                        (BAD, tr("mods.clash_fatal"))
                    } else {
                        (WARN, tr("mods.clash_note"))
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(color, kind);
                        ui.label(RichText::new(clash.describe()).small());
                    });
                }
                if list.is_empty() {
                    ui.weak(tr("mods.no_conflicts"));
                }
                for conflict in list.iter().take(300) {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(conflict.key.to_string()).monospace().small());
                        ui.label(trf(
                            "mods.conflict_used",
                            &[("mods", &conflict.mods.join(", "))],
                        ));
                        ui.colored_label(GOOD, conflict.winner());
                        if conflict.same_data {
                            ui.weak(tr("mods.same_data"));
                        }
                    });
                }
                if list.len() > 300 {
                    ui.weak(trf("mods.and_more", &[("count", &(list.len() - 300))]));
                }
            });
        self.show_all_conflicts = show_all;
    }

    fn integrity_section(&mut self, ui: &mut egui::Ui) {
        let Some(report) = &self.integrity else {
            return;
        };
        egui::CollapsingHeader::new(tr("mods.integrity_files"))
            .id_salt("mods-integrity")
            .show(ui, |ui| {
                ui.label(trf(
                    "mods.integrity_counts",
                    &[
                        ("files", &trn("count.files", report.files.len() as u64, &[])),
                        ("original", &report.count(Condition::Original)),
                        ("changed", &report.changed().count()),
                        ("unverified", &report.count(Condition::Unverified)),
                    ],
                ));
                for file in report.changed() {
                    let color = match file.condition {
                        Condition::Mods | Condition::Workspace => GOOD,
                        Condition::OldToolkit => WARN,
                        _ => BAD,
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&file.rel_path).monospace());
                        ui.colored_label(color, condition_text(file.condition));
                    });
                }
                if let Some(old) = &report.old_toolkit {
                    ui.label(trf(
                        "mods.old_toolkit_present",
                        &[
                            (
                                "patches",
                                &trn("count.patches", old.installed_patches.len() as u64, &[]),
                            ),
                            (
                                "backups",
                                &trn("count.backups", old.backups.len() as u64, &[]),
                            ),
                        ],
                    ));
                }
                if !report.reference && report.count(Condition::Unverified) > 0 {
                    ui.weak(tr("mods.unverified_hint"));
                }
                for path in &report.temp_files {
                    ui.weak(trf("mods.leftover", &[("path", path)]));
                }
            });
    }
}
