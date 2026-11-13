//! `resource_json`: the contents of a resource as JSON (or Lua source),
//! in pages.

use serde::Serialize;
use serde_json::{Value, json};

use doctor_toolkit::dialogue::{self, Report};
use doctor_toolkit::hash::sdbm;
use doctor_toolkit::resource::{
    anim, chv, dlg, ffn, hkx, lh2, logic, lua, material, rcb, sbk, shading, smb, tg4, tob, trinity,
};
use doctor_toolkit::workspace::Workspace;

use super::{Output, ResourceJsonParams, clamp, hex, resolve};
use crate::output::page;
use crate::state::{ToolError, ToolResult, Toolkit, fail};

const TYPE_LH2: u32 = sdbm(b"lh2");
const TYPE_MATERIAL: u32 = sdbm(b"material");
const TYPE_TG4H: u32 = sdbm(b"tg4h");
const TYPE_SBK: u32 = sdbm(b"sbk");

/// What `resource_json` shows for a type, or `None` when it has no view.
pub fn view_name(type_id: u32) -> Option<&'static str> {
    Some(match type_id {
        logic::TYPE_SIMGROUP => "SimGroup: objects, classes, attributes (logic-export JSON)",
        logic::TYPE_METAMODEL => "metamodel: parts, states, properties (logic-export JSON)",
        logic::TYPE_VARIABLE_DICTIONARY => "variable dictionary (logic-export JSON)",
        logic::TYPE_PARTSDB => "parts database (logic-export JSON)",
        logic::TYPE_GRAPH => "navigation graph: nodes and edges (logic-export JSON)",
        t if t == sdbm(b"mobface") => "mobface: faces and sliders",
        TYPE_LH2 => "string table: entries by hash, text per language",
        dlg::TYPE_DLG => "dialogue tree with speech, subtitles and option texts",
        trinity::TYPE_TRINITY_SEQ_MASTER => "cut-scene shot: commands, actors, curves",
        tob::TYPE_TOB => "subtitle timing table",
        rcb::TYPE_RCB => "rig: skeletons, bone names, parents and bind poses",
        anim::TYPE_BNK => "animation bank: clips and events",
        hkx::TYPE_HKX => "Havok packfile: sections and objects",
        chv::TYPE_CHV => "chatter set: lines and events",
        smb::TYPE_SMB => "speech map: entries",
        TYPE_SBK => "sound bank: emitters, parameters, variations",
        ffn::TYPE_FFN => "font: glyph metrics",
        TYPE_MATERIAL => "material: shaders, parameters, textures",
        TYPE_TG4H => "texture header: name, size, format",
        lua::TYPE_LUA => "Lua script: decompiled Lua 5.0 source (text, not JSON)",
        _ => return None,
    })
}

fn to_value<T: Serialize>(value: doctor_toolkit::Result<T>) -> ToolResult<Value> {
    Ok(serde_json::to_value(value?)?)
}

/// The view of one resource: JSON, or Lua source.
enum View {
    Json(Value),
    Source(String),
}

fn decode(
    kit: &Toolkit,
    ws: Option<&Workspace>,
    stream: usize,
    resource: usize,
    type_id: u32,
    data: &[u8],
) -> ToolResult<View> {
    if let Some(json) = logic::to_json(type_id, data) {
        return Ok(View::Json(json?));
    }
    Ok(View::Json(match type_id {
        TYPE_LH2 => strings(&lh2::StringTable::parse(data)?),
        dlg::TYPE_DLG => dialogue(kit, ws, stream, resource, data)?,
        trinity::TYPE_TRINITY_SEQ_MASTER => to_value(trinity::SeqMaster::parse(data))?,
        tob::TYPE_TOB => to_value(tob::Tob::parse(data))?,
        rcb::TYPE_RCB => to_value(rcb::Rig::parse(data))?,
        anim::TYPE_BNK => to_value(anim::AnimBank::parse(data))?,
        hkx::TYPE_HKX => to_value(hkx::Packfile::parse(data))?,
        chv::TYPE_CHV => to_value(chv::ChatterSet::parse(data))?,
        smb::TYPE_SMB => to_value(smb::parse(data))?,
        TYPE_SBK => to_value(sbk::SoundBank::parse(data))?,
        ffn::TYPE_FFN => to_value(ffn::Font::parse(data))?,
        TYPE_MATERIAL => material_json(&material::Material::parse(data)?)?,
        TYPE_TG4H => texture_json(&tg4::Tg4Header::parse(data)?),
        lua::TYPE_LUA => {
            let chunk = lua::Chunk::parse(data)?;
            return Ok(View::Source(lua::decompile(&chunk).source));
        }
        _ => {
            return fail(format!(
                "resources of type {} have no JSON view; resource_info shows their names and references",
                doctor_toolkit::resource::names::type_tags().display(type_id)
            ));
        }
    }))
}

fn strings(table: &lh2::StringTable) -> Value {
    let columns: Vec<String> = (0..table.strings.len())
        .map(|i| {
            table
                .languages
                .get(i)
                .filter(|l| !l.is_empty())
                .cloned()
                .unwrap_or_else(|| format!("column{i}"))
        })
        .collect();
    let entries: Vec<Value> = table
        .hashes
        .iter()
        .enumerate()
        .map(|(e, h)| {
            let mut entry = serde_json::Map::new();
            entry.insert("hash".into(), json!(hex(*h)));
            for (c, name) in columns.iter().enumerate() {
                entry.insert(name.clone(), json!(table.strings[c][e]));
            }
            Value::Object(entry)
        })
        .collect();
    json!({
        "version": table.version,
        "flags": table.flags,
        "languages": columns,
        "entries": entries,
    })
}

fn dialogue(
    kit: &Toolkit,
    ws: Option<&Workspace>,
    stream: usize,
    resource: usize,
    data: &[u8],
) -> ToolResult<Value> {
    let parsed = dlg::Dialogue::parse(data)?;
    let catalog = kit.dialogues()?;
    let Some(d) = catalog
        .dialogues
        .iter()
        .find(|d| d.stream == stream && d.resource == resource)
    else {
        return Ok(serde_json::to_value(&parsed)?);
    };
    let english = dialogue::language_strings(&kit.game, ws, &catalog.languages)
        .into_iter()
        .find(|(code, _)| code.eq_ignore_ascii_case("en"))
        .map(|(_, t)| t);
    Ok(serde_json::to_value(Report::of(
        catalog,
        d,
        &parsed,
        english.as_ref(),
    ))?)
}

fn material_json(m: &material::Material) -> ToolResult<Value> {
    let params: Vec<Value> = m
        .params
        .iter()
        .map(|p| {
            let (kind, value) = match &p.value {
                material::ParamValue::Floats(v) => ("floats", json!(v)),
                material::ParamValue::Texture(h) => ("texture", json!(hex(*h))),
                material::ParamValue::Unknown(t) => ("unknown", json!(t)),
            };
            json!({ "hash": hex(p.hash), "name": p.name, "source": p.source, "type": kind, "value": value })
        })
        .collect();
    Ok(json!({
        "version": m.version,
        "name": m.name,
        "name_hash": hex(m.name_hash),
        "flags": m.flags,
        "shaders": m.shaders.iter().map(|(h, first, count)| json!({ "shader": hex(*h), "first_param": first, "params": count })).collect::<Vec<_>>(),
        "shadow_shader": hex(m.shadow_shader),
        "params": params,
        "shading": serde_json::to_value(shading::Shading::from_material(m))?,
    }))
}

fn texture_json(h: &tg4::Tg4Header) -> Value {
    json!({
        "name": h.name,
        "package": h.package,
        "name_hash": hex(h.name_hash),
        "width": h.width,
        "height": h.height,
        "format": h.format().name(),
        "kind": format!("{:?}", h.kind()).to_lowercase(),
        "mip_count": h.mip_count(),
        "layer_count": h.layer_count(),
        "srgb": h.is_srgb(),
        "data_size": h.data_size,
    })
}

/// Follows a JSON Pointer, with a hint when a step is missing.
fn select<'a>(value: &'a Value, pointer: &str) -> ToolResult<&'a Value> {
    if pointer.is_empty() || pointer == "/" {
        return Ok(value);
    }
    if !pointer.starts_with('/') {
        return fail(format!(
            "`{pointer}` is not a JSON Pointer; write it like /objects/3"
        ));
    }
    value.pointer(pointer).ok_or_else(|| {
        let keys = match value {
            Value::Object(m) => m.keys().take(30).cloned().collect::<Vec<_>>().join(", "),
            _ => String::new(),
        };
        ToolError(format!(
            "nothing at `{pointer}`; the top-level fields are: {keys}"
        ))
    })
}

/// The contents of a resource, one page at a time.
pub fn resource_json(kit: &Toolkit, p: ResourceJsonParams) -> ToolResult<Output> {
    let id = resolve(kit, &p.resource)?;
    let index = kit.index();
    let record = index.record(id);
    let type_id = record.header.type_id;
    if view_name(type_id).is_none() && !logic::is_supported(type_id) {
        decode(kit, None, 0, 0, type_id, &[])?;
    }
    let stream = index.stream_of(id);
    let resource = record.resource as usize;
    let ws = kit.workspace_unless(p.original.unwrap_or(false))?;
    let data = kit.data(ws.as_ref(), stream, resource)?;
    let view = decode(kit, ws.as_ref(), stream, resource, type_id, &data)?;
    let pointer = p.pointer.clone().unwrap_or_default();
    let (text, kind) = match view {
        View::Json(value) => {
            let part = select(&value, &pointer)?;
            (serde_json::to_string_pretty(part)?, "json")
        }
        View::Source(source) => {
            if !pointer.is_empty() {
                return fail("a Lua script is text; leave out `pointer`");
            }
            (source, "lua")
        }
    };
    let max = clamp(
        p.max_chars,
        20_000,
        kit.max_chars.saturating_sub(1_000).max(1_000),
    );
    let piece = page(&text, p.offset.unwrap_or(0), max);
    let head = json!({
        "resource": index.key(id).to_string(),
        "label": index.label(id),
        "type": doctor_toolkit::resource::names::type_tags().display(type_id),
        "view": view_name(type_id),
        "format": kind,
        "pointer": if pointer.is_empty() { Value::Null } else { json!(pointer) },
        "from_workspace": ws.as_ref().is_some_and(|w| w.change(&index.key(id)).is_some()),
        "total_chars": piece.total,
        "offset": piece.offset,
        "returned_chars": piece.text.chars().count(),
        "next_offset": piece.next,
        "note": if piece.next.is_some() {
            "more follows: call again with offset = next_offset, or narrow with pointer"
        } else {
            "complete"
        },
    });
    Ok(Output::Text(format!(
        "{}\n----- {kind} -----\n{}",
        serde_json::to_string_pretty(&head)?,
        piece.text
    )))
}
