//! How the game shades a material, reduced to what a viewer can draw.
//!
//! A material names five pass shaders (shadow, depth, colour, low-LOD depth,
//! low-LOD colour) by CRC-32 of the lower-case name; the names and the flags of the lit
//! shaders come from `godfather2.exe` (`data/gf2/shaders.txt`). The pixel
//! shaders, compiled for shader model 3 and embedded in the executable, were
//! disassembled to recover the formulas below (`docs/viewers.md` §7):
//!
//! | Family | Colour |
//! |---|---|
//! | tint mask (clothes) | `lerp(R·T0 + G·T1 + B·T2, base, detailMask.z · controls.z)` — the base texture is a mask |
//! | hair | `1 − (1 − R·T0)(1 − G·T1)(1 − B·T2)`, alpha from the gloss map's green, blended |
//! | hair without tints (eyelashes) | base texture, alpha from its alpha, blended |
//! | eyes | `lerp(base, base · eyeTint, base.a)`, opaque |
//! | skin and faces | `lerp(base, base · skinTint, gloss.r)`; detail maps for faces |
//! | other | base texture |
//!
//! Everything is computed in linear space: the base texture is raised to
//! 2.2 (skin) or sampled as sRGB, tint colours of shaders with the
//! `linear-tints` flag are raised to 2.2 when the material loads, and the
//! result is written back with exponent 1/2.2. Gloss maps hold rim (R), the
//! ambient occlusion every light term is multiplied with (G) and the specular
//! mask (B). Normal maps are `DXT5_NM` (X in alpha, Y in green) without a Z:
//! the shader adds `x·T + y·B` to the vertex normal and normalises.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Serialize;

use crate::hash::crc32;
use crate::resource::material::{Material, ParamValue};

static GF2_SHADERS: &str = include_str!("../../../../data/gf2/shaders.txt");

/// A shader of the game with the flags of its table entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Shader {
    pub name: &'static str,
    /// Alpha blended (flag 0x100).
    pub blend: bool,
    /// Tint colours are raised to 2.2 when the material loads.
    pub linear_tints: bool,
    /// UV scale-offset parameters get `w = 1 − w − 1/y` when the material loads.
    pub uv_offsets: bool,
}

fn shaders() -> &'static HashMap<u32, Shader> {
    static TABLE: OnceLock<HashMap<u32, Shader>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut map = HashMap::new();
        for line in GF2_SHADERS.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let mut words = line.split_whitespace();
            let Some(name) = words.next() else { continue };
            let flags: Vec<&str> = words.collect();
            map.insert(
                crc32(name.to_ascii_lowercase().as_bytes()),
                Shader {
                    name,
                    blend: flags.contains(&"blend"),
                    linear_tints: flags.contains(&"linear-tints"),
                    uv_offsets: flags.contains(&"uv-offsets"),
                },
            );
        }
        map
    })
}

/// The shader whose name hash is `hash`.
pub fn shader(hash: u32) -> Option<Shader> {
    shaders().get(&hash).copied()
}

/// Every known shader name, for the name dictionary.
pub fn shader_names() -> impl Iterator<Item = &'static str> {
    shaders().values().map(|s| s.name)
}

/// The shading model of a lit shader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    /// Clothes: the base texture is a mask of three tint colours.
    TintMask,
    /// Hair: tint colours combined like screen layers, alpha blended.
    Hair,
    /// Hair shaders without tints (`gf_char_hair_aniso`, eyelashes): the
    /// base texture is the colour and its alpha the coverage, blended.
    PlainHair,
    /// Skin and faces: base colour tinted where the gloss map's red is set.
    Skin,
    Eye,
    Glass,
    /// Everything else: the base texture is the colour.
    Basic,
}

impl Family {
    fn of(name: &str) -> Self {
        let n = name.to_ascii_lowercase();
        if n.starts_with("gf_char_tint_mask") {
            Self::TintMask
        } else if n.starts_with("gf_char_hair") && n.contains("tint") {
            Self::Hair
        } else if n.starts_with("gf_char_hair") {
            Self::PlainHair
        } else if n.starts_with("gf_char_skin") || n.starts_with("gf_char_face") {
            Self::Skin
        } else if n.starts_with("gf_char_eye") {
            Self::Eye
        } else if n.contains("glass") {
            Self::Glass
        } else {
            Self::Basic
        }
    }
}

/// How to blend a material with what is behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Blend {
    Opaque,
    /// Alpha blended without depth writes.
    Alpha,
}

/// Shading inputs of one material, already converted the way the game does
/// when it loads the material. Textures are name hashes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Shading {
    /// Name of the lit shader (slot 1), when known.
    pub shader: Option<&'static str>,
    pub family: Family,
    pub blend: Blend,
    pub base: Option<u32>,
    pub normal: Option<u32>,
    pub gloss: Option<u32>,
    pub detail_normal: Option<u32>,
    pub detail_mask: Option<u32>,
    pub detail_gloss: Option<u32>,
    /// Detail colour map of faces (`g_DetailMapSampler`).
    pub detail: Option<u32>,
    /// `g_TintColor0..2`, linear.
    pub tints: [[f32; 3]; 3],
    /// `g_Tint`: multiplies the colour; w is the alpha.
    pub tint: [f32; 4],
    /// `g_SkinTintColor`, or `g_EyeTintColor` for eyes.
    pub skin_tint: [f32; 3],
    /// `g_DetailTintColor`.
    pub detail_tint: [f32; 3],
    /// `g_DetailMaskControls.z`: how much of the base texture shows where the detail mask's blue is set.
    pub detail_mask_strength: f32,
    /// UV of the detail normal and gloss maps: `uv · xy + zw`.
    pub detail_uv: [f32; 4],
    /// UV of the detail mask: `uv · xy + zw`.
    pub detail_mask_uv: [f32; 4],
    /// `g_SpecColorAndScale` rgb · w.
    pub specular: [f32; 3],
    /// Specular exponent derived from `g_SpecParams.x`.
    pub specular_power: f32,
    /// Weight of the detail normal map (`g_SpecParams.w` × detail mask green).
    pub detail_normal_strength: f32,
}

const IDENTITY_UV: [f32; 4] = [1.0, 1.0, 0.0, 0.0];

fn linear(c: f32) -> f32 {
    c.max(0.0).powf(2.2)
}

impl Shading {
    pub fn from_material(material: &Material) -> Self {
        let lit = shader(material.shaders[1].0);
        let family = lit.map_or_else(
            || {
                // Unknown shader: guess from the parameters.
                let has = |n: &str| material.params.iter().any(|p| p.name == n);
                if has("g_TintColor0") && has("g_DetailMaskControls") {
                    Family::TintMask
                } else if has("hair1SpecMulShiftPowColorDodge") {
                    Family::Hair
                } else if has("g_SkinTintColor") {
                    Family::Skin
                } else {
                    Family::Basic
                }
            },
            |s| Family::of(s.name),
        );
        let floats = |name: &str| -> Option<&[f32]> {
            material.params.iter().find_map(|p| match &p.value {
                ParamValue::Floats(v) if p.name == name => Some(v.as_slice()),
                _ => None,
            })
        };
        let texture = |names: &[&str]| -> Option<u32> {
            names.iter().find_map(|n| {
                material.params.iter().find_map(|p| match p.value {
                    ParamValue::Texture(h) if p.name == *n => Some(h),
                    _ => None,
                })
            })
        };
        let color3 = |name: &str, default: [f32; 3]| -> [f32; 3] {
            floats(name).map_or(default, |v| {
                [
                    v.first().copied().unwrap_or(default[0]),
                    v.get(1).copied().unwrap_or(default[1]),
                    v.get(2).copied().unwrap_or(default[2]),
                ]
            })
        };
        let linear_tints = lit.is_some_and(|s| s.linear_tints);
        let tint_color = |name: &str| {
            let c = color3(name, [1.0; 3]);
            if linear_tints { c.map(linear) } else { c }
        };
        let uv_offsets = lit.is_some_and(|s| s.uv_offsets);
        // The shaders compute `(uv − zw) · xy`; with `uv-offsets` the game
        // first replaces w by 1 − w − 1/y.
        let uv = |name: &str| -> [f32; 4] {
            let Some(v) = floats(name).filter(|v| v.len() >= 4) else {
                return IDENTITY_UV;
            };
            let (sx, sy, ox, mut oy) = (v[0], v[1], v[2], v[3]);
            if uv_offsets && sy != 0.0 {
                oy = 1.0 - oy - 1.0 / sy;
            }
            [sx, sy, -ox * sx, -oy * sy]
        };
        let tint = floats("g_Tint").map_or([1.0; 4], |v| {
            let mut t = [1.0; 4];
            for (o, x) in t.iter_mut().zip(v) {
                *o = *x;
            }
            t
        });
        let spec = floats("g_SpecColorAndScale").unwrap_or(&[0.5, 0.5, 0.5, 0.3]);
        let scale = spec.get(3).copied().unwrap_or(1.0);
        let spec_params = floats("g_SpecParams").unwrap_or(&[0.25, 0.0, 0.0, 0.0]);
        let mask_controls = floats("g_DetailMaskControls").unwrap_or(&[0.0, 0.0, 0.0]);
        let blend = if lit.is_some_and(|s| s.blend) {
            Blend::Alpha
        } else {
            Blend::Opaque
        };
        let skin_tint = match family {
            Family::Eye => color3("g_EyeTintColor", [1.0; 3]),
            _ => color3("g_SkinTintColor", [1.0; 3]),
        };
        Self {
            shader: lit.map(|s| s.name),
            family,
            blend,
            base: texture(&["g_BaseSampler"]),
            normal: texture(&["g_NormalMapSampler", "g_materialNormalMap"]),
            gloss: texture(&["g_GlossMapSampler", "g_materialSpecMap"]),
            detail_normal: texture(&["g_DetailNormalMapSampler"]),
            detail_mask: texture(&["g_DetailMaskSampler"]),
            detail_gloss: texture(&["g_DetailGlossMapSampler"]),
            detail: texture(&["g_DetailMapSampler", "g_DetailSampler"]),
            tints: [
                tint_color("g_TintColor0"),
                tint_color("g_TintColor1"),
                tint_color("g_TintColor2"),
            ],
            tint,
            skin_tint,
            detail_tint: tint_color("g_DetailTintColor"),
            detail_mask_strength: mask_controls.get(2).copied().unwrap_or(0.0),
            detail_uv: uv("g_NormalSpecUVScaleOffset"),
            detail_mask_uv: uv("g_DetailMaskUVScaleOffset"),
            specular: [
                spec.first().copied().unwrap_or(0.5) * scale,
                spec.get(1).copied().unwrap_or(0.5) * scale,
                spec.get(2).copied().unwrap_or(0.5) * scale,
            ],
            specular_power: 2f32.powf(1.0 + 10.0 * spec_params[0].clamp(0.0, 1.0)),
            detail_normal_strength: spec_params.get(3).copied().unwrap_or(0.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_names_hash_to_the_material_slots() {
        // Hashes seen in the character materials of the game.
        assert_eq!(shader(0x257C_01C6).unwrap().name, "gf_char_tint_mask_basic");
        assert_eq!(shader(0x44B9_D3C3).unwrap().name, "gf_char_face");
        assert_eq!(shader(0x363A_388F).unwrap().name, "shadowsskinned");
        let hair = shader(crc32(b"gf_char_hair_aniso_tint")).unwrap();
        assert!(hair.blend && hair.linear_tints && !hair.uv_offsets);
        assert!(
            shader(crc32(b"gf_char_tint_mask_basic"))
                .unwrap()
                .uv_offsets
        );
        assert!(shader(1).is_none());
    }

    #[test]
    fn eye_and_eyelash_families() {
        // Lit shaders of the eye and eyelash materials of the game.
        let family = |name: &str| Family::of(name);
        assert_eq!(family("gf_char_eye"), Family::Eye);
        assert_eq!(family("gf_char_hair_aniso"), Family::PlainHair);
        assert_eq!(family("gf_char_hair"), Family::PlainHair);
        assert_eq!(family("gf_char_hair_aniso_tint"), Family::Hair);
        assert_eq!(family("gf_char_hair_tint_LowRes"), Family::Hair);
        let mut m = Material {
            version: 19,
            name: "female_eyes".into(),
            name_hash: 0,
            flags: 0,
            shaders: [
                (0, 0, 0),
                (crc32(b"gf_char_eye"), 0, 0),
                (0, 0, 0),
                (0, 0, 0),
            ],
            shadow_shader: 0,
            params: Vec::new(),
        };
        m.params.push(crate::resource::material::Param {
            hash: crc32(b"g_EyeTintColor"),
            name: "g_EyeTintColor".into(),
            source: String::new(),
            value: ParamValue::Floats(vec![0.5, 0.64, 1.0, 1.0]),
        });
        let s = Shading::from_material(&m);
        assert_eq!(s.family, Family::Eye);
        assert_eq!(s.blend, Blend::Opaque);
        // The eye colour is used as stored (the shader does not convert it).
        assert_eq!(s.skin_tint, [0.5, 0.64, 1.0]);
    }

    #[test]
    fn tint_mask_material() {
        let mut m = Material {
            version: 19,
            name: "shirt".into(),
            name_hash: 0,
            flags: 0,
            shaders: [(0, 0, 0), (0x257C_01C6, 0, 0), (0, 0, 0), (0, 0, 0)],
            shadow_shader: 0,
            params: Vec::new(),
        };
        let p = |name: &str, value: ParamValue| crate::resource::material::Param {
            hash: crc32(name.as_bytes()),
            name: name.into(),
            source: String::new(),
            value,
        };
        m.params.push(p("g_BaseSampler", ParamValue::Texture(7)));
        m.params.push(p(
            "g_TintColor0",
            ParamValue::Floats(vec![0.5, 1.0, 0.0, 1.0]),
        ));
        m.params.push(p(
            "g_NormalSpecUVScaleOffset",
            ParamValue::Floats(vec![20.0, 12.0, 0.0, 0.0]),
        ));
        m.params.push(p(
            "g_DetailMaskControls",
            ParamValue::Floats(vec![0.0, 0.0, 1.0]),
        ));
        let s = Shading::from_material(&m);
        assert_eq!(s.family, Family::TintMask);
        assert_eq!(s.blend, Blend::Opaque);
        assert_eq!(s.base, Some(7));
        assert!((s.tints[0][0] - 0.5f32.powf(2.2)).abs() < 1e-6);
        assert_eq!(s.tints[0][1], 1.0);
        assert_eq!(s.detail_mask_strength, 1.0);
        // w = 1 − 0 − 1/12, offset = −w·12 = −11.
        assert_eq!(s.detail_uv[..2], [20.0, 12.0]);
        assert!((s.detail_uv[3] + 11.0).abs() < 1e-5);
    }
}
