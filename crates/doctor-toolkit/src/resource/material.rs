//! `material` resources (magic 0x0AA0DEED, version 19).
//!
//! ```text
//! 0x00  u32  magic 0x0AA0DEED
//! 0x04  u32  version, 19
//! 0x08  u32  name hash: CRC-32 of the lower-case name; draw records refer to it
//! 0x0C  u32  name hash again
//! 0x14  u32  flags: 0x1 alpha test (see [`FLAG_ALPHA_TEST`]); 0x20, 0x80 on
//!            glass and glow, 0x80000…0x200000 on clothing
//! 0x1C  5 × pass {u32 shader hash, u8 first parameter, u8 parameter count, u16 0}:
//!            shadow, depth, colour, low-LOD depth, low-LOD colour
//! 0x46  u16  parameter count
//! 0x48  6 × u16  table offset, table size, data offset, data size, string offset, string size
//! table: parameter {u32 hash, u16 type, u16 data offset, u16 name offset, u16 source offset}
//! ```
//!
//! Parameter types: 0 = f32, 1 = 8 bytes (assumed f32×2), 2 = f32×3, 3 = f32×4,
//! 5 = texture (the texture's name hash, equal to the last id of its tg4h/tg4d).
//! The material name is the first string of the string table.

use crate::io::{self, Endian};
use crate::{Error, Result};

const MAGIC: u32 = 0x0AA0_DEED;

/// Type id of `material` resources.
pub const TYPE_MATERIAL: u32 = crate::hash::sdbm(b"material");

/// Value of a material parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    Floats(Vec<f32>),
    /// Name hash of a texture.
    Texture(u32),
    /// A type not seen in the game files; raw bytes are not kept.
    Unknown(u16),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub hash: u32,
    /// Shader constant name, e.g. `g_BaseSampler`.
    pub name: String,
    /// Source the value was authored from, e.g. a texture path.
    pub source: String,
    pub value: ParamValue,
}

/// A new value of a material parameter.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParamEdit {
    /// Shader constant name, e.g. `g_TintColor0`.
    pub name: String,
    pub value: ParamEditValue,
}

/// See [`ParamEdit`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamEditValue {
    /// As many numbers as the parameter holds (a colour has three or four).
    Floats(Vec<f32>),
    /// Name hash of a texture.
    Texture(u32),
}

impl ParamEdit {
    /// Reads `name=<number>,<number>…` (numbers also separated by `/` or
    /// spaces) or `name=tex:<hex name hash of a texture>`.
    pub fn parse(text: &str) -> Result<Self> {
        let bad = || {
            Error::InvalidArgument(format!(
                "`{text}`: write <parameter>=<number>,<number>… or <parameter>=tex:<hex name hash of the texture>"
            ))
        };
        let (name, value) = text.split_once('=').ok_or_else(bad)?;
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() || value.is_empty() {
            return Err(bad());
        }
        let value = if let Some(texture) = value.strip_prefix("tex:") {
            let texture = texture.trim();
            let hex = texture.strip_prefix("0x").unwrap_or(texture);
            ParamEditValue::Texture(u32::from_str_radix(hex, 16).map_err(|_| bad())?)
        } else {
            let numbers: Option<Vec<f32>> = value
                .split([',', '/', ' '])
                .filter(|v| !v.is_empty())
                .map(|v| v.parse().ok())
                .collect();
            ParamEditValue::Floats(numbers.filter(|n| !n.is_empty()).ok_or_else(bad)?)
        };
        Ok(Self {
            name: name.to_owned(),
            value,
        })
    }
}

/// What [`set_params`] did.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ParamsEdited {
    /// The material resource (its workspace key).
    pub resource: String,
    pub material: String,
    /// False when the material already had these values.
    pub changed: bool,
    pub label: String,
}

/// Gives the material `target` (a material of the game, or one the
/// workspace added) the values of `edits` and records one undoable
/// operation named `label`. Returns the name of the material and whether
/// anything changed (nothing is recorded when not). The one place that
/// writes material parameters: the editor, the `material` command and the
/// MCP tool `set_material` ([`crate::retexture::set_material_params`]) go
/// through it.
pub fn apply_params(
    game: &crate::game::GameInstance,
    ws: &mut crate::workspace::Workspace,
    target: &crate::import::file::FileTarget,
    edits: &[ParamEdit],
    label: impl FnOnce(&Material) -> String,
) -> Result<(String, bool, String)> {
    let key = target.key();
    if key.type_id != TYPE_MATERIAL {
        return Err(Error::InvalidArgument(format!("{key} is not a material")));
    }
    if edits.is_empty() {
        return Err(Error::InvalidArgument(
            "give at least one parameter with its values".into(),
        ));
    }
    let data = target.current(game, ws)?;
    let material = Material::parse(&data)?;
    let new = Material::edit(&data, edits)?;
    let label = label(&material);
    let changed = new != data;
    if changed {
        target.record(game, ws, new, &label)?;
    }
    Ok((material.name, changed, label))
}

/// Sets parameters of the material resource `resource` of stream `stream`
/// (as the workspace has it) and records one undoable operation. The
/// material is shared by every model that names it.
pub fn set_params(
    game: &crate::game::GameInstance,
    ws: &mut crate::workspace::Workspace,
    stream: usize,
    resource: usize,
    edits: &[ParamEdit],
) -> Result<ParamsEdited> {
    let target = crate::import::file::FileTarget::game(game, stream, resource)?;
    let names: Vec<&str> = edits.iter().map(|e| e.name.as_str()).collect();
    let (material, changed, label) = apply_params(game, ws, &target, edits, |m| {
        format!("Set {} of material {}", names.join(", "), m.name)
    })?;
    Ok(ParamsEdited {
        resource: target.key().to_string(),
        material,
        changed,
        label,
    })
}

/// Role of a texture, derived from the sampler name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureRole {
    Diffuse,
    Normal,
    Specular,
    Gloss,
    Detail,
    Mask,
    Other,
}

impl TextureRole {
    pub fn from_sampler(name: &str) -> Self {
        match name {
            "g_BaseSampler" => Self::Diffuse,
            "g_materialNormalMap" | "g_NormalMapSampler" => Self::Normal,
            "g_materialSpecMap" => Self::Specular,
            "g_GlossMapSampler" => Self::Gloss,
            "g_DetailSampler" => Self::Detail,
            "g_MaskSampler" => Self::Mask,
            _ => Self::Other,
        }
    }
}

/// Material flag (0x14): alpha-tested; exactly the 4 383 materials whose
/// shadow pass has parameters (alpha or leaf shadow shaders).
pub const FLAG_ALPHA_TEST: u32 = 0x1;

/// A parsed material.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    pub version: u32,
    pub name: String,
    pub name_hash: u32,
    /// Flags at 0x14, see [`FLAG_ALPHA_TEST`].
    pub flags: u32,
    /// `(shader hash, first parameter, parameter count)` of passes 1–4: depth
    /// (deferred normals), colour, low-LOD depth, low-LOD colour.
    pub shaders: [(u32, u8, u8); 4],
    /// Shader of pass 0, the shadow pass (0x1C); its parameters (alpha-tested
    /// materials) are not read.
    pub shadow_shader: u32,
    pub params: Vec<Param>,
}

impl Material {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let le = Endian::Little;
        let u32_at = |o: usize| io::read_u32(data, o, le);
        let u16_at = |o: usize| io::read_u16(data, o, le);
        if u32_at(0)? != MAGIC {
            return Err(Error::BadMagic {
                what: "material",
                found: u32_at(0)?,
            });
        }
        let count = u16_at(0x46)? as usize;
        let table_at = u16_at(0x48)? as usize;
        let data_at = u16_at(0x4C)? as usize;
        let strings_at = u16_at(0x50)? as usize;
        let string = |offset: u16| -> Result<String> {
            Ok(String::from_utf8_lossy(io::cstr(data, strings_at + offset as usize)?).into_owned())
        };
        let mut shaders = [(0, 0, 0); 4];
        for (i, slot) in shaders.iter_mut().enumerate() {
            let at = 0x24 + 8 * i;
            *slot = (
                u32_at(at)?,
                io::slice(data, at + 4, 1)?[0],
                io::slice(data, at + 5, 1)?[0],
            );
        }
        let mut params = Vec::with_capacity(count);
        for i in 0..count {
            let at = table_at + 12 * i;
            let kind = u16_at(at + 4)?;
            let value_at = data_at + u16_at(at + 6)? as usize;
            let floats = |n: usize| -> Result<ParamValue> {
                Ok(ParamValue::Floats(
                    (0..n)
                        .map(|k| u32_at(value_at + 4 * k).map(f32::from_bits))
                        .collect::<Result<_>>()?,
                ))
            };
            let value = match kind {
                0 => floats(1)?,
                1 => floats(2)?,
                2 => floats(3)?,
                3 => floats(4)?,
                5 => ParamValue::Texture(u32_at(value_at)?),
                other => ParamValue::Unknown(other),
            };
            params.push(Param {
                hash: u32_at(at)?,
                name: string(u16_at(at + 8)?)?,
                source: string(u16_at(at + 10)?)?,
                value,
            });
        }
        Ok(Self {
            version: u32_at(4)?,
            name: string(0)?,
            name_hash: u32_at(8)?,
            flags: u32_at(0x14)?,
            shaders,
            shadow_shader: u32_at(0x1C)?,
            params,
        })
    }

    /// Texture name hashes with their roles, first occurrence of each sampler.
    pub fn textures(&self) -> Vec<(TextureRole, u32, &Param)> {
        let mut seen = Vec::new();
        let mut out = Vec::new();
        for param in &self.params {
            if let ParamValue::Texture(hash) = param.value
                && !seen.contains(&(param.name.as_str(), hash))
            {
                seen.push((param.name.as_str(), hash));
                out.push((TextureRole::from_sampler(&param.name), hash, param));
            }
        }
        out
    }

    /// Offsets of the values of every parameter named `name` (a material
    /// lists a parameter once per shader pass that takes it, each with its
    /// own value): all of them must hold what `fits` accepts.
    fn value_offsets(
        &self,
        data: &[u8],
        name: &str,
        fits: impl Fn(&ParamValue) -> std::result::Result<(), String>,
    ) -> Result<Vec<usize>> {
        let le = Endian::Little;
        let table_at = io::read_u16(data, 0x48, le)? as usize;
        let data_at = io::read_u16(data, 0x4C, le)? as usize;
        let mut out = Vec::new();
        for (position, p) in self.params.iter().enumerate() {
            if p.name != name {
                continue;
            }
            fits(&p.value).map_err(|why| {
                Error::InvalidArgument(format!("parameter {name} of material {} {why}", self.name))
            })?;
            let at = data_at + io::read_u16(data, table_at + 12 * position + 6, le)? as usize;
            if !out.contains(&at) {
                out.push(at);
            }
        }
        if out.is_empty() {
            return Err(Error::InvalidArgument(format!(
                "material {} has no parameter {name}",
                self.name
            )));
        }
        Ok(out)
    }

    /// Material data with the float parameter `name` set to `values` (as
    /// many as the parameter holds: a tint colour `g_TintColor0` has four),
    /// in every shader pass that lists it. Every other byte is kept.
    pub fn set_floats(data: &[u8], name: &str, values: &[f32]) -> Result<Vec<u8>> {
        let material = Self::parse(data)?;
        let offsets = material.value_offsets(data, name, |value| match value {
            ParamValue::Floats(current) if current.len() == values.len() => Ok(()),
            ParamValue::Floats(current) => Err(format!(
                "holds {} value(s), not {}",
                current.len(),
                values.len()
            )),
            _ => Err("is not a number".to_owned()),
        })?;
        let mut out = data.to_vec();
        for value_at in offsets {
            for (k, v) in values.iter().enumerate() {
                let at = value_at + 4 * k;
                out.get_mut(at..at + 4)
                    .ok_or_else(|| Error::InvalidArgument("parameter data out of range".into()))?
                    .copy_from_slice(&v.to_le_bytes());
            }
        }
        Ok(out)
    }

    /// Material data with the texture parameter `name` (a sampler such as
    /// `g_BaseSampler`) naming the texture with name hash `texture`, in
    /// every shader pass that lists it. Every other byte is kept.
    pub fn set_texture(data: &[u8], name: &str, texture: u32) -> Result<Vec<u8>> {
        let material = Self::parse(data)?;
        let offsets = material.value_offsets(data, name, |value| match value {
            ParamValue::Texture(_) => Ok(()),
            _ => Err("is not a texture".to_owned()),
        })?;
        let mut out = data.to_vec();
        for value_at in offsets {
            out.get_mut(value_at..value_at + 4)
                .ok_or_else(|| Error::InvalidArgument("parameter data out of range".into()))?
                .copy_from_slice(&texture.to_le_bytes());
        }
        Ok(out)
    }

    /// The parameters with each name once (the first of the passes that
    /// list it) and how many passes list it.
    pub fn unique_params(&self) -> Vec<(&Param, usize)> {
        let mut out: Vec<(&Param, usize)> = Vec::new();
        for p in &self.params {
            match out.iter_mut().find(|(q, _)| q.name == p.name) {
                Some((_, n)) => *n += 1,
                None => out.push((p, 1)),
            }
        }
        out
    }

    /// Material data with every one of `edits` made (see [`ParamEdit`]).
    pub fn edit(data: &[u8], edits: &[ParamEdit]) -> Result<Vec<u8>> {
        let mut out = data.to_vec();
        for e in edits {
            out = match &e.value {
                ParamEditValue::Floats(values) => Self::set_floats(&out, &e.name, values)?,
                ParamEditValue::Texture(hash) => Self::set_texture(&out, &e.name, *hash)?,
            };
        }
        Ok(out)
    }

    /// The float values of parameter `name`.
    pub fn floats(&self, name: &str) -> Option<&[f32]> {
        self.params.iter().find_map(|p| match &p.value {
            ParamValue::Floats(v) if p.name == name => Some(v.as_slice()),
            _ => None,
        })
    }

    /// The first texture with the given role.
    pub fn texture(&self, role: TextureRole) -> Option<u32> {
        self.textures()
            .into_iter()
            .find(|t| t.0 == role)
            .map(|t| t.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A material with one texture and one colour parameter, laid out like the game files.
    fn sample() -> Vec<u8> {
        let mut d = vec![0u8; 0x60];
        d[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        d[4..8].copy_from_slice(&19u32.to_le_bytes());
        let strings: &[u8] = b"brick_wall\0g_BaseSampler\0textures/brick.tga\0g_Color\0\0";
        d[8..12].copy_from_slice(&crate::hash::crc32(b"brick_wall").to_le_bytes());
        d[0x46..0x48].copy_from_slice(&2u16.to_le_bytes());
        let (table, data_at) = (0x60usize, 0x80usize);
        let strings_at = data_at + 32;
        for (i, v) in [table, 24, data_at, 32, strings_at, strings.len()]
            .iter()
            .enumerate()
        {
            d[0x48 + 2 * i..0x4A + 2 * i].copy_from_slice(&(*v as u16).to_le_bytes());
        }
        d.resize(strings_at + strings.len(), 0);
        let entry = |hash: u32, kind: u16, data: u16, name: u16, source: u16| {
            let mut e = hash.to_le_bytes().to_vec();
            for v in [kind, data, name, source] {
                e.extend_from_slice(&v.to_le_bytes());
            }
            e
        };
        d[table..table + 12].copy_from_slice(&entry(1, 5, 0, 11, 25));
        d[table + 12..table + 24].copy_from_slice(&entry(2, 3, 16, 44, 52));
        d[data_at..data_at + 4].copy_from_slice(&0xA068_8794u32.to_le_bytes());
        for (k, f) in [0.5f32, 0.25, 1.0, 1.0].iter().enumerate() {
            d[data_at + 16 + 4 * k..data_at + 20 + 4 * k].copy_from_slice(&f.to_le_bytes());
        }
        d[strings_at..].copy_from_slice(strings);
        d
    }

    #[test]
    fn parses_names_textures_and_values() {
        let m = Material::parse(&sample()).unwrap();
        assert_eq!(m.name, "brick_wall");
        assert_eq!(m.name_hash, crate::hash::crc32(b"brick_wall"));
        assert_eq!(m.texture(TextureRole::Diffuse), Some(0xA068_8794));
        assert_eq!(m.params[0].source, "textures/brick.tga");
        assert_eq!(
            m.params[1].value,
            ParamValue::Floats(vec![0.5, 0.25, 1.0, 1.0])
        );
        assert_eq!(m.texture(TextureRole::Normal), None);
    }

    #[test]
    fn sets_float_parameters_in_place() {
        let data = sample();
        let out = Material::set_floats(&data, "g_Color", &[0.1, 0.2, 0.3, 1.0]).unwrap();
        assert_eq!(out.len(), data.len());
        let m = Material::parse(&out).unwrap();
        assert_eq!(m.floats("g_Color"), Some(&[0.1, 0.2, 0.3, 1.0][..]));
        assert_eq!(m.texture(TextureRole::Diffuse), Some(0xA068_8794));
        assert!(Material::set_floats(&data, "g_Color", &[1.0]).is_err());
        assert!(Material::set_floats(&data, "g_BaseSampler", &[1.0]).is_err());
        assert!(Material::set_floats(&data, "g_None", &[1.0]).is_err());
    }

    #[test]
    fn rejects_bad_input() {
        assert!(Material::parse(&[]).is_err());
        let data = sample();
        for len in 0..data.len() {
            let _ = Material::parse(&data[..len]);
        }
    }
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    #[test]
    fn parameter_edits_are_read_from_text() {
        assert_eq!(
            ParamEdit::parse("g_TintColor0 = 0.8, 0.6/0.3 1").unwrap(),
            ParamEdit {
                name: "g_TintColor0".into(),
                value: ParamEditValue::Floats(vec![0.8, 0.6, 0.3, 1.0]),
            }
        );
        assert_eq!(
            ParamEdit::parse("g_BaseSampler=tex:0x00000055")
                .unwrap()
                .value,
            ParamEditValue::Texture(0x55)
        );
        for bad in ["g_TintColor0", "=1", "a=", "a=x", "a=tex:wall"] {
            assert!(ParamEdit::parse(bad).is_err(), "{bad}");
        }
    }
}
