//! Details (stage 53): one inspector for what was selected last — an object
//! of a level (in the view or the Outliner) or a resource (in a list of
//! resources), as Unreal Engine's Details panel follows the selection.
//!
//! - **Object** ([`object`]): groups of properties — Transform, Model,
//!   Logic, Collision, Advanced (raw attributes, hashes, offsets) — that
//!   fold; edits are the workspace edits of the level ([`LevelAction`]), so
//!   Ctrl+Z takes them back.
//! - **Resource** ([`resource`]): the resource's header, references and
//!   views, as the Details panel showed up to stage 52.
//!
//! **Pin** keeps a Details panel on what it shows while the selection moves
//! on. View → "Another Details panel" opens one more ([`Pane::DetailsCopy`]),
//! which starts pinned to what the first one shows: two objects side by
//! side.
//!
//! [`LevelAction`]: crate::level_view::LevelAction
//! [`Pane::DetailsCopy`]: super::dock::Pane::DetailsCopy

mod object;
mod resource;
#[cfg(test)]
pub(super) mod tests;

use std::collections::HashMap;

use doctor_toolkit::level::Location;
use doctor_toolkit_mods::i18n::tr;
use egui::RichText;

use super::BrowserApp;
use super::docs::ResourceDoc;
use super::theme;
pub use object::ObjectState;

/// What was selected last: Details follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Nothing,
    /// The primary selected object of a level view.
    Level(u64),
    /// The resource selected in a list (the Files panel, a stream tab, a
    /// search).
    Resource,
}

/// What a pinned Details panel keeps showing.
pub enum Pinned {
    Object {
        level: u64,
        group: Location,
        object: usize,
    },
    Resource(Box<ResourceDoc>),
}

/// One Details panel.
#[derive(Default)]
pub struct Inspector {
    pub pinned: Option<Pinned>,
    pub object: ObjectState,
}

/// Every Details panel: the main one, more copies, and the ones inside the
/// tabs of wizards (by level view).
#[derive(Default)]
pub struct Inspectors {
    pub main: Inspector,
    pub copies: HashMap<u64, Inspector>,
    pub embedded: HashMap<u64, Inspector>,
}

/// What a Details panel shows this frame.
#[derive(Debug, Clone, Copy)]
enum Shown {
    Object(u64, crate::level_view::ObjRef),
    /// A level is followed but nothing of it is selected.
    NoObject(u64),
    /// The resource of the Details selection (`self.details`).
    Resource,
    Pinned,
    Nothing,
}

impl BrowserApp {
    /// The Details panel; `copy` is the pane id of another one.
    pub(super) fn details_pane(&mut self, ui: &mut egui::Ui, copy: Option<u64>) {
        let mut inspector = match copy {
            None => std::mem::take(&mut self.inspectors.main),
            Some(id) => self.inspectors.copies.remove(&id).unwrap_or_default(),
        };
        self.inspector_ui(ui, &mut inspector, None, copy.unwrap_or(0));
        match copy {
            None => self.inspectors.main = inspector,
            Some(id) => {
                self.inspectors.copies.insert(id, inspector);
            }
        }
    }

    /// Details of level view `level` inside a wizard's tab.
    pub(super) fn embedded_details(&mut self, ui: &mut egui::Ui, level: u64) {
        let mut inspector = self.inspectors.embedded.remove(&level).unwrap_or_default();
        self.inspector_ui(ui, &mut inspector, Some(level), level);
        self.inspectors.embedded.insert(level, inspector);
    }

    /// Opens another Details panel, pinned to what the main one shows.
    pub(super) fn new_details(&mut self) {
        let id = self.new_pane_id();
        let main = std::mem::take(&mut self.inspectors.main);
        let shown = self.shown(&main, None);
        let pinned = self.pin_target(shown, main.pinned.as_ref());
        self.inspectors.main = main;
        self.inspectors.copies.insert(
            id,
            Inspector {
                pinned,
                ..Inspector::default()
            },
        );
        self.dock_ops.push(super::dock::DockOp::ShowPanel(
            super::dock::Pane::DetailsCopy { id },
        ));
    }

    /// What an inspector shows now.
    fn shown(&self, inspector: &Inspector, level: Option<u64>) -> Shown {
        match &inspector.pinned {
            Some(Pinned::Resource(_)) => return Shown::Pinned,
            Some(Pinned::Object {
                level,
                group,
                object,
            }) => {
                return match self.levels.get(level).and_then(|l| l.find(*group, *object)) {
                    Some(r) => Shown::Object(*level, r),
                    None => Shown::NoObject(*level),
                };
            }
            None => {}
        }
        let of_level = |id: u64| match self.levels.get(&id).and_then(|l| l.primary()) {
            Some(r) => Shown::Object(id, r),
            None => Shown::NoObject(id),
        };
        if let Some(id) = level {
            return of_level(id);
        }
        match self.inspect {
            Focus::Level(id) if self.levels.contains_key(&id) => of_level(id),
            Focus::Resource if self.details.is_some() => Shown::Resource,
            _ if self.details.is_some() => Shown::Resource,
            _ => match self.panel_level {
                Some(id) => of_level(id),
                None => Shown::Nothing,
            },
        }
    }

    /// What Pin would keep: the object or resource shown now (`pinned`:
    /// what the panel keeps already).
    fn pin_target(&mut self, shown: Shown, pinned: Option<&Pinned>) -> Option<Pinned> {
        match shown {
            Shown::Object(id, r) => {
                let (group, object) = self.levels.get(&id)?.location_of(r)?;
                Some(Pinned::Object {
                    level: id,
                    group,
                    object,
                })
            }
            Shown::Resource => {
                let loc = self.details.as_ref()?.loc;
                Some(Pinned::Resource(Box::new(self.load_doc(loc, None))))
            }
            Shown::Pinned => match pinned {
                Some(Pinned::Resource(doc)) => {
                    Some(Pinned::Resource(Box::new(self.load_doc(doc.loc, None))))
                }
                Some(Pinned::Object {
                    level,
                    group,
                    object,
                }) => Some(Pinned::Object {
                    level: *level,
                    group: *group,
                    object: *object,
                }),
                None => None,
            },
            Shown::NoObject(_) | Shown::Nothing => None,
        }
    }

    fn inspector_ui(
        &mut self,
        ui: &mut egui::Ui,
        inspector: &mut Inspector,
        level: Option<u64>,
        salt: u64,
    ) {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        let shown = self.shown(inspector, level);
        // The bar: Pin.
        let pinned = inspector.pinned.is_some();
        let can_pin = pinned || matches!(shown, Shown::Object(..) | Shown::Resource);
        let clicked = ui
            .horizontal(|ui| {
                let response = ui
                    .add_enabled_ui(can_pin, |ui| {
                        theme::tool_button(
                            ui,
                            Some(theme::Icon::Pin),
                            tr("details.pin"),
                            pinned,
                            theme::Tone::Flat,
                        )
                    })
                    .inner
                    .on_hover_text(tr("details.pin_hint"));
                if pinned {
                    ui.label(RichText::new(tr("details.pinned")).weak().small());
                }
                response.clicked()
            })
            .inner;
        if clicked {
            inspector.pinned = if pinned {
                None
            } else {
                self.pin_target(shown, None)
            };
        }
        ui.separator();
        let shown = self.shown(inspector, level);
        egui::ScrollArea::vertical()
            .id_salt(("details-scroll", salt))
            .auto_shrink(false)
            .show(ui, |ui| match shown {
                Shown::Object(id, r) => {
                    let actions = self.object_details(ui, &mut inspector.object, id, r);
                    for action in actions {
                        self.level_action(id, action);
                    }
                }
                Shown::NoObject(id) => {
                    ui.add_space(6.0);
                    let loaded = self.levels.get(&id).is_some_and(|l| l.loaded());
                    let text = if loaded {
                        tr("details.select_object")
                    } else {
                        tr("level.details_load_first")
                    };
                    ui.label(RichText::new(text).weak());
                }
                Shown::Resource => self.resource_details(ui),
                Shown::Pinned => {
                    if let Some(Pinned::Resource(doc)) = &mut inspector.pinned {
                        self.pinned_resource_details(ui, doc);
                    }
                }
                Shown::Nothing => {
                    ui.add_space(6.0);
                    ui.label(RichText::new(tr("details.select")).weak());
                }
            });
    }
}
