//! The view registry: how the details of a resource are shown.
//!
//! A resource view is a type that implements [`ResourceViewer`] and is listed
//! once in [`REGISTRY`] with a function that loads it for a resource. When a
//! resource is opened (in the Details panel or in its own tab), every entry is
//! asked in order; each one that returns a viewer is shown, top to bottom,
//! followed by the hex view unless a viewer hides it.
//!
//! Adding a view:
//!
//! 1. Write the view in its own module (`foo_view.rs`): a struct with the
//!    state of one opened resource.
//! 2. Implement [`ResourceViewer`] for it: `show` draws it and may return a
//!    [`ViewAction`] (record new data in the workspace, import a texture, …).
//! 3. Add `pub const VIEW: ViewEntry = ViewEntry { id: "foo", load };` to the
//!    module, where `load` returns `None` for resources the view does not
//!    handle (check [`ViewSource::type_id`] first, it is cheap).
//! 4. Add `crate::foo_view::VIEW` to [`REGISTRY`].
//!
//! The view never writes to the workspace or the game itself: the app records
//! the returned action, then loads the views of the resource again from the
//! workspace (keeping what [`ResourceViewer::state`] returned).

use std::any::Any;
use std::path::PathBuf;

use doctor_toolkit::export::gltf::AssetSource;
use doctor_toolkit::game::{GameInstance, StreamFile};
use doctor_toolkit::resource::lh2::StringTable;
use doctor_toolkit::resource::texture::TexturePair;
use doctor_toolkit::workspace::{ResourceKey, Status, Workspace};

use crate::render::Gpu;

/// Every resource view, in the order they are shown.
pub const REGISTRY: &[ViewEntry] = &[
    crate::wizard_view::VIEW,
    crate::external_view::VIEW,
    crate::model_view::VIEW,
    crate::apx_view::VIEW,
    crate::resource_view::VIEW,
    crate::anim_view::VIEW,
    crate::script_view::VIEW,
    crate::character_view::VIEW,
    crate::logic_view::VIEW,
    crate::texture_view::VIEW,
    crate::text_view::VIEW,
    crate::cutscene_view::VIEW,
    crate::dialogue_view::VIEW,
];

/// One registered view.
pub struct ViewEntry {
    /// Stable name of the view; keeps the state of a view across reloads.
    pub id: &'static str,
    /// Loads the view for a resource; `None` when it does not apply.
    pub load: fn(&mut ViewSource<'_>) -> Option<Box<dyn ResourceViewer>>,
}

/// The view of one resource.
pub trait ResourceViewer {
    /// Draws the view. `cx.status` takes a message for the status bar.
    fn show(&mut self, ui: &mut egui::Ui, cx: &mut ViewContext<'_>) -> Option<ViewAction>;

    /// The view fills the panel: the hex view is loaded on request only.
    fn hides_hex(&self) -> bool {
        false
    }

    /// The view holds edits that are not recorded yet.
    fn has_unsaved_edits(&self) -> bool {
        false
    }

    /// Selection and similar state to carry over when the resource is loaded
    /// again after an edit.
    fn state(&self) -> Option<Box<dyn Any>> {
        None
    }

    /// Takes back what [`Self::state`] returned, from the view before the reload.
    fn restore(&mut self, _state: Box<dyn Any>) {}
}

/// What a view asks the app to do.
pub enum ViewAction {
    /// Record new data for the shown resource.
    SaveData { data: Vec<u8>, label: String },
    /// Record an edited string table for the shown resource.
    SaveStrings(StringTable),
    /// Add a string named `name` to the shown table, or to the tables of
    /// every language (`everywhere`).
    AddString {
        name: String,
        text: String,
        everywhere: bool,
    },
    /// Import an image file over a texture of the stream.
    ImportTexture { pair: TexturePair, path: PathBuf },
    /// Open the shown SimGroup in the logic graph editor.
    OpenLogicGraph { object: Option<usize> },
    /// Record an edit that touches other resources or files (string tables
    /// of every language, speech in the archives): `run` makes it on the
    /// workspace, as one or more undoable operations.
    Apply { label: String, run: WorkspaceEdit },
    /// Open another resource.
    Open { stream: usize, resource: usize },
    /// Export the resource, open it in another program and import its saves.
    OpenExternal {
        format: doctor_toolkit::external::Format,
    },
    /// Stop watching the working file of a resource.
    Unlink(ResourceKey),
    /// Start a wizard (`retexture`, `model`, `outfit`, `texts`, `params`,
    /// `place`) for the shown resource.
    StartWizard(&'static str),
}

/// An edit a view hands to the app, see [`ViewAction::Apply`].
pub type WorkspaceEdit =
    Box<dyn FnOnce(&GameInstance, &mut Workspace) -> doctor_toolkit::Result<()> + Send>;

/// What a view can use while it is drawn.
pub struct ViewContext<'a> {
    /// The status bar message; set it to report what happened.
    pub status: &'a mut String,
    /// Resources linked to working files of other programs.
    pub links: &'a [crate::external_view::LinkInfo],
}

/// Where a view loads its resource from: the stream, and the workspace
/// version of resources the workspace changes.
pub struct ViewSource<'a> {
    pub game: &'a GameInstance,
    /// Index of the stream in the game.
    pub stream_index: usize,
    pub stream: &'a StreamFile,
    /// The resource opened.
    pub index: usize,
    pub type_id: u32,
    pub workspace: Option<&'a Workspace>,
    pub ws_status: Option<&'a Status>,
    /// Materials and textures across the game, for models.
    pub assets: &'a mut dyn AssetSource,
    /// The 3D renderer; `None` when the window runs on OpenGL.
    pub gpu: Option<&'a Gpu>,
}

impl ViewSource<'_> {
    /// Workspace key of resource `index` of the stream.
    pub fn key(&self, index: usize) -> Option<ResourceKey> {
        let header = self.stream.layout().resources.get(index)?.header;
        Some(ResourceKey::new(
            &self.game.streams()[self.stream_index].stream_name(),
            &header,
        ))
    }

    /// The workspace version of resource `index`; `None` when the editor
    /// shows the game's data for it.
    pub fn edited(&self, index: usize) -> Option<Result<Vec<u8>, String>> {
        let ws = self.workspace?;
        let key = self.key(index)?;
        if ws.change(&key).is_some() {
            return ws.data(&key).map_err(|e| e.to_string()).transpose();
        }
        // Removed from the workspace but still in the game: the workspace has the original.
        if self
            .ws_status
            .is_some_and(|s| s.pending_reverts.contains(&key))
        {
            let file = &self.game.streams()[self.stream_index];
            let original = StreamFile::open(self.game.original_path(file));
            return Some(
                original
                    .and_then(|s| {
                        let i = key.find_in(s.layout()).ok_or_else(|| {
                            doctor_toolkit::Error::InvalidArgument(
                                doctor_toolkit_mods::i18n::tr("docs.not_in_original").into(),
                            )
                        })?;
                        s.resource_data(i)
                    })
                    .map_err(|e| e.to_string()),
            );
        }
        None
    }

    /// Data of resource `index` as the editor shows it.
    pub fn data(&self, index: usize) -> Result<Vec<u8>, String> {
        match self.edited(index) {
            Some(data) => data,
            None => self.stream.resource_data(index).map_err(|e| e.to_string()),
        }
    }

    /// The game's original data of a resource the workspace changes.
    pub fn original(&self, index: usize) -> Option<Vec<u8>> {
        self.workspace?
            .original_data(&self.key(index)?)
            .ok()
            .flatten()
    }
}

/// Loads every registered view of a resource, with the state of the views
/// it replaces.
pub fn load_all(
    source: &mut ViewSource<'_>,
    mut previous: Vec<(&'static str, Box<dyn ResourceViewer>)>,
) -> Vec<(&'static str, Box<dyn ResourceViewer>)> {
    let mut views = Vec::new();
    for entry in REGISTRY {
        let Some(mut view) = (entry.load)(source) else {
            continue;
        };
        if let Some(i) = previous.iter().position(|(id, _)| *id == entry.id)
            && let Some(state) = previous.swap_remove(i).1.state()
        {
            view.restore(state);
        }
        views.push((entry.id, view));
    }
    views
}

#[cfg(test)]
mod tests {
    use super::REGISTRY;

    #[test]
    fn view_ids_are_unique() {
        let mut ids: Vec<&str> = REGISTRY.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "two views share an id");
    }
}
