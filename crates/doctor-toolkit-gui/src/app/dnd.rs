//! Drag and drop between panels (stage 53).
//!
//! An asset of the game is dragged as a [`DraggedAsset`] payload of egui's
//! drag and drop: a list (the Content Browser of stage 55) starts the drag
//! with `ui.dnd_drag_source(id, DraggedAsset { .. }, |ui| …)` (or
//! `egui::DragAndDrop::set_payload`), and a field takes it with
//! `ui.dnd_drop_zone::<DraggedAsset, _>(…)`. The model field of Details
//! takes `metamodel` assets: dropping one gives the selected object that
//! model (one edit of the workspace, undone with Ctrl+Z).

/// A resource of the game (or of the workspace) being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DraggedAsset {
    /// Type id of the resource (`sdbm` of the type name, e.g. `metamodel`).
    pub type_id: u32,
    /// The resource's own id: the last of its four header ids (the id
    /// objects refer to, e.g. `CMD_SetModel`).
    pub id: u32,
    /// Where it is: stream index of the game and resource index in it.
    pub stream: usize,
    pub resource: usize,
}
