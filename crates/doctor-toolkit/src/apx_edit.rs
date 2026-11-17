//! Edits of Flash interface files (`apx`) as one operation of the
//! workspace: replace an embedded texture, the Apt movie, the constant
//! table, and point `$name` text references at other strings. What
//! `apx-import` of the command line, the tool `apx_edit` of the MCP server
//! and the apx viewer of the editor do.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::game::GameInstance;
use crate::hash::string_id;
use crate::resource::apx::{AptFile, TAG_APT, TAG_CONSTANTS};
use crate::workspace::Workspace;
use crate::{Error, Result};

const TYPE_APX: u32 = crate::hash::sdbm(b"apx");

/// What to change in an apx file; what is left out stays.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApxEdit {
    /// The embedded texture to replace: its name or its position (from 0).
    /// Goes with `image`.
    #[serde(default)]
    pub texture: Option<String>,
    /// The new image of `texture` (an absolute path): .png or .dds.
    #[serde(default)]
    pub image: Option<PathBuf>,
    /// Replace the Apt movie with this `.apt` file (an absolute path).
    #[serde(default)]
    pub apt: Option<PathBuf>,
    /// Replace the constant table with this `.const` file (an absolute path).
    #[serde(default)]
    pub constants: Option<PathBuf>,
    /// Point `$OLD` text references of the movie at `$NEW`: old name → new
    /// name (the new name no longer than the old one).
    #[serde(default)]
    pub text_refs: BTreeMap<String, String>,
}

/// What [`edit`] did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApxEdited {
    /// Title of the apx file.
    pub title: String,
    /// What changed, in words.
    pub changes: Vec<String>,
    /// Label of the operation; `None` when the workspace already has this data.
    pub operation: Option<String>,
}

/// The apx file of stream `stream` whose title is `name` (as `apx-export`
/// writes it), or at resource index `name`, as the workspace has it.
pub fn find(
    game: &GameInstance,
    ws: &Workspace,
    stream: usize,
    name: &str,
) -> Result<(usize, AptFile)> {
    let file = game.open_stream(stream)?;
    let candidates: Vec<usize> = (0..file.layout().resources.len())
        .filter(|&i| file.layout().resources[i].header.type_id == TYPE_APX)
        .collect();
    drop(file);
    for index in candidates {
        let apx = AptFile::parse(&ws.current_data(game, stream, index)?)?;
        let title = apx.title().unwrap_or_default();
        if title.eq_ignore_ascii_case(name) || name.parse() == Ok(index) {
            return Ok((index, apx));
        }
    }
    Err(Error::InvalidArgument(format!(
        "no apx file `{name}` in {}",
        game.streams()[stream].stream_name()
    )))
}

fn read(path: &std::path::Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| Error::io(path, e))
}

/// Changes the apx file at resource `index` of `stream`: one undoable
/// operation.
pub fn edit(
    game: &GameInstance,
    ws: &mut Workspace,
    stream: usize,
    index: usize,
    edit: &ApxEdit,
) -> Result<ApxEdited> {
    if edit.texture.is_some() != edit.image.is_some() {
        return Err(Error::InvalidArgument(
            "a texture to replace goes with its new image, and an image with the texture".into(),
        ));
    }
    if edit.texture.is_none()
        && edit.apt.is_none()
        && edit.constants.is_none()
        && edit.text_refs.is_empty()
    {
        return Err(Error::InvalidArgument(
            "nothing to import: give a texture and its image, a movie, constants or text references"
                .into(),
        ));
    }
    let file = game.open_stream(stream)?;
    let is_apx = file
        .layout()
        .resources
        .get(index)
        .is_some_and(|r| r.header.type_id == TYPE_APX);
    drop(file);
    if !is_apx {
        return Err(Error::InvalidArgument(format!(
            "resource {index} is not an apx file"
        )));
    }
    let mut apx = AptFile::parse(&ws.current_data(game, stream, index)?)?;
    let title = apx.title().unwrap_or_default();
    let mut changes = Vec::new();
    if let (Some(name), Some(image)) = (&edit.texture, &edit.image) {
        let textures = apx.textures()?;
        let position = textures
            .iter()
            .position(|t| t.header.name.eq_ignore_ascii_case(name))
            .or_else(|| name.parse().ok().filter(|&i: &usize| i < textures.len()))
            .ok_or_else(|| {
                let names: Vec<&str> = textures.iter().map(|t| t.header.name.as_str()).collect();
                Error::InvalidArgument(format!(
                    "{title} has no embedded texture `{name}`; it has: {}",
                    names.join(", ")
                ))
            })?;
        let bytes = read(image)?;
        let is_png = image
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("png"));
        let before = &textures[position].header;
        let after = apx.import_texture(position, &bytes, is_png)?;
        let h = &after.header;
        changes.push(format!(
            "texture {}: {}×{} {} -> {}×{} {}, {} mip level(s)",
            h.name,
            before.width,
            before.height,
            before.format().name(),
            h.width,
            h.height,
            h.format().name(),
            h.mip_count()
        ));
    }
    for (path, is_apt) in [(&edit.apt, true), (&edit.constants, false)] {
        let Some(path) = path else { continue };
        let bytes = read(path)?;
        let len = bytes.len();
        if is_apt {
            apx.set_apt(bytes)?;
        } else {
            apx.set_constants(bytes)?;
        }
        changes.push(format!(
            "{} replaced with {} ({len} bytes)",
            if is_apt {
                "movie (apti)"
            } else {
                "constants (cons)"
            },
            path.display()
        ));
    }
    for (old, new) in &edit.text_refs {
        let (old, new) = (old.trim_start_matches('$'), new.trim_start_matches('$'));
        let changed = apx.retarget_text(old, new)?;
        if changed == 0 {
            return Err(Error::InvalidArgument(format!(
                "the movie has no reference ${old}"
            )));
        }
        changes.push(format!("pointed {changed} reference(s) ${old} at ${new}"));
    }
    let label = format!(
        "Import into {title} ({})",
        game.streams()[stream].stream_name()
    );
    let recorded = ws.set_resource(game, stream, index, apx.to_bytes()?, &label)?;
    Ok(ApxEdited {
        title,
        changes,
        operation: recorded.then_some(label),
    })
}

/// An apx file as JSON: its title, blocks, embedded textures and the
/// `$name` text references of its movie with the hashes of the strings.
pub fn describe(apx: &AptFile) -> Value {
    let textures: Vec<Value> = apx
        .textures()
        .map(|list| {
            list.iter()
                .enumerate()
                .map(|(i, t)| {
                    json!({
                        "position": i,
                        "name": t.header.name,
                        "width": t.header.width,
                        "height": t.header.height,
                        "format": t.header.format().name(),
                        "mip_levels": t.header.mip_count(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut references: Vec<(String, usize)> = Vec::new();
    for (_, _, name) in apx.text_references() {
        match references.iter_mut().find(|(n, _)| *n == name) {
            Some((_, count)) => *count += 1,
            None => references.push((name, 1)),
        }
    }
    json!({
        "title": apx.title(),
        "movie_bytes": apx.block(TAG_APT).map(|b| b.data.len()),
        "constants_bytes": apx.block(TAG_CONSTANTS).map(|b| b.data.len()),
        "textures": textures,
        "text_references": references
            .iter()
            .map(|(name, count)| json!({
                "name": format!("${name}"),
                "hash": format!("{:08x}", string_id(name)),
                "uses": count,
            }))
            .collect::<Vec<_>>(),
    })
}
