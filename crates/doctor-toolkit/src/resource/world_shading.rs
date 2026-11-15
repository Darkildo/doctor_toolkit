//! How the game shades the world: buildings, roads, ground, props, windows,
//! water (`docs/analysis/renderer.md` §10.7).
//!
//! A world material names a normal pass shader (slot 0) and a lit pass
//! shader (slot 1). The normal pass writes the normal (from the vertex normal
//! and a normal map) and the sharpness `s` of the sun's highlight; the light
//! pre-pass turns them into light and highlight `pow(n·h, s³·255)`; the lit
//! pass colours the surface:
//!
//! | Model | Lit shaders | Colour |
//! |---|---|---|
//! | [`WorldModel::Basic`] | `gf_basic`, `gf_normal_spec*`, `UVanimation*`, `gf_vehicles*` | `base` |
//! | [`WorldModel::Building`] | `gf_building`, `gf_basic_tint_reflect` | `lerp(base, base·tint, base.a)` |
//! | [`WorldModel::Detail`] | `gf_building_detail`, `gf_road` | `lerp(detail layer, base layer, mask.r · scale)`, each layer tinted where its alpha is set |
//! | [`WorldModel::Panorama`] | `gf_pano` | `lerp(base, base·tint, detail.b)` |
//! | [`WorldModel::Overlay`] | `gf_overlay_vertex_ao` | darkens what is below by the vertex colour's green |
//! | [`WorldModel::Window`], [`WorldModel::Glass`] | `WindowGlow`, `WindowParallax`, `WindowGlass` | front map and a reflection of the sky |
//! | [`WorldModel::Water`] | `WaterOcean*` | `WaterColor`, lit |
//!
//! Every lit world shader multiplies the colour by the green of the vertex
//! colour (baked occlusion) and the alpha by its alpha; colours are linear
//! (textures raised to 2.2, tints converted when the material loads).

use serde::Serialize;

use crate::resource::material::{FLAG_ALPHA_TEST, Material, Param, ParamValue};
use crate::resource::shading::shader;

/// A texture the engine makes at start-up instead of loading it: materials
/// name these by a bare name (`color`, `normal`, `specular`) or a path under
/// `runtime_generated` (the environment cube maps). The values are the
/// table at `0x0110A450` of `godfather2.exe` (`0xAARRGGBB`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Builtin {
    /// `color`: white.
    Color,
    /// `normal`: 0x80808080, a flat normal.
    Normal,
    /// `specular`: 0xFF000000, black (no highlight).
    Specular,
    /// `~cubemap`, `~cubemapsky`: the environment the game renders at run time.
    Cubemap,
}

impl Builtin {
    /// Name hashes of the built-in textures as materials store them.
    pub const HASHES: [(u32, Builtin); 5] = [
        (0x3D11_5391, Builtin::Color),
        (0x7F00_D6F1, Builtin::Normal),
        (0xE993_0443, Builtin::Specular),
        (0x0665_231C, Builtin::Cubemap),
        (0x906B_CC4B, Builtin::Cubemap),
    ];

    /// The built-in texture a material's texture hash names.
    pub fn of(hash: u32) -> Option<Self> {
        Self::HASHES
            .iter()
            .find(|(h, _)| *h == hash)
            .map(|(_, b)| *b)
    }

    /// The built-in texture a parameter's source names (`color`, a
    /// `runtime_generated` path), for parameters whose hash is not listed.
    pub fn named(source: &str) -> Option<Self> {
        match source {
            "color" => Some(Self::Color),
            "normal" => Some(Self::Normal),
            "specular" => Some(Self::Specular),
            s if s.contains("runtime_generated/~cubemap") => Some(Self::Cubemap),
            _ => None,
        }
    }
}

/// How a world shader forms the colour (see the module documentation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldModel {
    Basic,
    Building,
    Detail,
    Panorama,
    Overlay,
    /// Lit window: front map lit, plus the reflection where its alpha is set.
    Window,
    /// `WindowGlass`: not lit, front map times its alpha plus the reflection.
    Glass,
    Water,
}

impl WorldModel {
    /// The model of a lit shader; `None` for shaders that are not world
    /// surfaces (characters, sky, trees, effects).
    pub fn of(lit: &str) -> Option<Self> {
        let n = lit.to_ascii_lowercase();
        Some(if n.starts_with("gf_building_detail") || n == "gf_road" {
            Self::Detail
        } else if n.starts_with("gf_building") || n.starts_with("gf_basic_tint_reflect") {
            Self::Building
        } else if n == "gf_pano" {
            Self::Panorama
        } else if n == "gf_overlay_vertex_ao" {
            Self::Overlay
        } else if n.starts_with("windowglass") {
            Self::Glass
        } else if n.starts_with("window") {
            Self::Window
        } else if n.starts_with("waterocean") {
            Self::Water
        } else if n.starts_with("gf_basic_non")
            || n.starts_with("gf_basic_vfx")
            || n.starts_with("gf_vehicles_glass")
        {
            return None;
        } else if n.starts_with("gf_basic")
            || n.starts_with("gf_normal_spec")
            || n.starts_with("uvanimation")
            || n.starts_with("gf_vehicles")
        {
            Self::Basic
        } else {
            return None;
        })
    }
}

/// Shading inputs of a world material, converted the way the game does when
/// it loads the material. Textures are name hashes; built-in textures are
/// left out (`None`: white colour, flat normal, black gloss).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorldShading {
    /// Lit shader (slot 1).
    pub shader: &'static str,
    /// Normal pass shader (slot 0), when the material has one.
    pub normal_pass: Option<&'static str>,
    pub model: WorldModel,
    /// `g_BaseSampler`; windows: `g_FrontMap`.
    pub base: Option<u32>,
    /// `g_DetailSampler` (second layer), `g_DetailMapSampler` (panorama: B is the tint mask).
    pub detail: Option<u32>,
    /// `g_MaskSampler`: R weighs the base layer over the detail layer.
    pub mask: Option<u32>,
    /// Normal map of the normal pass (`DXT5_NM`).
    pub normal: Option<u32>,
    /// `g_materialNormalMapDetail`, blended with `normal` by the mask.
    pub detail_normal: Option<u32>,
    /// `g_GlossMapSampler` of the lit pass: R masks the highlight, G the reflection.
    pub gloss: Option<u32>,
    /// Map of the normal pass whose blue is the highlight's sharpness.
    pub exponent_map: Option<u32>,
    /// `g_BaseTintColor` (water: `WaterColor`), linear; w: alpha.
    pub base_tint: [f32; 4],
    /// `g_DetailTintColor`, linear; w: alpha of the detail layer.
    pub detail_tint: [f32; 4],
    /// `g_Tint`: w multiplies the alpha.
    pub tint: [f32; 4],
    /// `g_MaskRangeScale`.
    pub mask_scale: f32,
    /// UV of the detail layer and detail normal: `uv · xy + zw`.
    pub detail_uv: [f32; 4],
    /// UV of the mask: `uv · xy + zw`.
    pub mask_uv: [f32; 4],
    /// `g_materialNormalMapScale`: how far the normal map tilts the normal.
    pub normal_scale: f32,
    /// Sharpness `s` of the highlight, or the factor on the exponent map's blue.
    pub exponent: f32,
    /// Colour of the highlight the lit pass adds.
    pub specular: [f32; 3],
    /// The highlight is masked by the gloss map's red.
    pub specular_masked: bool,
    /// The highlight is also multiplied by the light (`gf_building_detail`).
    pub specular_lit: bool,
    /// Reflection of the environment: strength facing the camera and the
    /// extra at grazing angles (`ReflectionRange`); `[0, 0]`: none.
    pub reflection: [f32; 2],
    /// The reflection is masked by the gloss map's green.
    pub reflection_masked: bool,
    /// Cut where the base texture's alpha is below one half (material flag 1).
    pub alpha_test: bool,
    /// `g_VertexAOParams.y`: how much the overlay darkens.
    pub overlay: f32,
    /// Light the surface gives off by day and at night (`g_NightControl`
    /// blends them; stage 70): panoramas `g_DayGlowColor` and
    /// `g_NightGlowColor` (rgb times w, where the detail map's green is set;
    /// at night also the mask's green), lit windows, lamps and signs
    /// `m_LightColor` at night only (where the front map's alpha is set).
    /// Added to the light before it multiplies the colour.
    pub glow: [[f32; 4]; 2],
}

const IDENTITY_UV: [f32; 4] = [1.0, 1.0, 0.0, 0.0];

fn linear(c: f32) -> f32 {
    c.max(0.0).powf(2.2)
}

/// The parameters of pass `pass` (0 normal, 1 lit) of a material.
fn pass_params(material: &Material, pass: usize) -> &[Param] {
    let (_, first, count) = material.shaders[pass];
    let first = first as usize;
    let end = (first + count as usize).min(material.params.len());
    material.params.get(first..end).unwrap_or(&[])
}

fn floats<'a>(params: &'a [Param], name: &str) -> Option<&'a [f32]> {
    params.iter().find_map(|p| match &p.value {
        ParamValue::Floats(v) if p.name == name => Some(v.as_slice()),
        _ => None,
    })
}

/// A texture parameter that names a texture of the game files.
fn texture(params: &[Param], names: &[&str]) -> Option<u32> {
    names.iter().find_map(|n| {
        params.iter().find_map(|p| match p.value {
            ParamValue::Texture(h) if p.name == *n => {
                let builtin = Builtin::of(h).or_else(|| Builtin::named(&p.source));
                (builtin.is_none()).then_some(h)
            }
            _ => None,
        })
    })
}

impl WorldShading {
    /// The world shading of a material; `None` when its lit shader is not a
    /// world shader.
    pub fn from_material(material: &Material) -> Option<Self> {
        let lit = shader(material.shaders[1].0)?;
        let model = WorldModel::of(lit.name)?;
        let normal_pass = shader(material.shaders[0].0).map(|s| s.name);
        let colour = pass_params(material, 1);
        let normals = pass_params(material, 0);
        let first = |name: &str, default: f32| -> f32 {
            floats(colour, name)
                .or_else(|| floats(normals, name))
                .and_then(|v| v.first().copied())
                .unwrap_or(default)
        };
        let tint_color = |name: &str| -> [f32; 4] {
            let mut c = [1.0; 4];
            if let Some(v) = floats(colour, name) {
                for (o, x) in c.iter_mut().zip(v) {
                    *o = *x;
                }
            }
            if lit.linear_tints {
                for x in &mut c[..3] {
                    *x = linear(*x);
                }
            }
            c
        };
        let uv = |name: &str| -> [f32; 4] {
            floats(colour, name)
                .or_else(|| floats(normals, name))
                .filter(|v| v.len() >= 4)
                .map_or(IDENTITY_UV, |v| [v[0], v[1], v[2], v[3]])
        };
        let tint = floats(colour, "g_Tint").map_or([1.0; 4], |v| {
            let mut t = [1.0; 4];
            for (o, x) in t.iter_mut().zip(v) {
                *o = *x;
            }
            t
        });
        let mut out = Self {
            shader: lit.name,
            normal_pass,
            model,
            base: texture(colour, &["g_BaseSampler"]),
            detail: None,
            mask: None,
            normal: None,
            detail_normal: None,
            gloss: texture(colour, &["g_GlossMapSampler"]),
            exponent_map: None,
            base_tint: tint_color("g_BaseTintColor"),
            detail_tint: [1.0; 4],
            tint,
            mask_scale: 1.0,
            detail_uv: IDENTITY_UV,
            mask_uv: IDENTITY_UV,
            normal_scale: first("g_materialNormalMapScale", 1.0),
            exponent: 0.0,
            specular: [0.0; 3],
            specular_masked: false,
            specular_lit: false,
            reflection: [0.0; 2],
            reflection_masked: false,
            alpha_test: material.flags & FLAG_ALPHA_TEST != 0,
            overlay: 0.0,
            glow: [[0.0; 4]; 2],
        };
        // The normal pass: normal map and the highlight's sharpness.
        match normal_pass {
            Some("DeferredNormals" | "DeferredNormalsAlpha") => {
                out.normal = texture(normals, &["g_materialNormalMap"]);
                out.exponent_map = texture(normals, &["g_materialSpecMap"]);
                // The blue of the map is `s`; the built-in black gives 0.
                out.exponent = if out.exponent_map.is_some() { 1.0 } else { 0.0 };
            }
            Some("gf_building_detail_depth") => {
                out.normal = texture(normals, &["g_materialNormalMap"]);
                out.detail_normal = texture(normals, &["g_materialNormalMapDetail"]);
                out.exponent = first("g_SpecularExponent", 1.0);
            }
            Some(name) if name.starts_with("Depth") => {
                out.normal = texture(normals, &["g_NormalMapSampler", "g_materialNormalMap"]);
                out.exponent_map = texture(normals, &["g_GlossMapSampler"]);
                out.exponent = floats(normals, "g_SpecParams")
                    .and_then(|v| v.first().copied())
                    .unwrap_or(0.75);
            }
            // DeferredNormalsFast*, water and passes without a normal pass:
            // the vertex normal; the sharpness is not a parameter.
            _ => out.exponent = DEFAULT_EXPONENT,
        }
        match model {
            WorldModel::Basic => {
                if let Some(scale) = floats(colour, "g_SpecularScale") {
                    // gf_basic: a grey highlight, not masked.
                    out.specular = [scale.first().copied().unwrap_or(0.0); 3];
                } else if let Some(c) = floats(colour, "g_SpecColorAndScale") {
                    // gf_normal_spec*: coloured, masked by the gloss map.
                    out.specular = [
                        c[0],
                        c.get(1).copied().unwrap_or(0.0),
                        c.get(2).copied().unwrap_or(0.0),
                    ];
                    out.specular_masked = true;
                }
                if let Some(r) = floats(colour, "g_ReflectionScale") {
                    out.reflection = [r.first().copied().unwrap_or(0.0), 0.0];
                    out.reflection_masked = true;
                }
            }
            WorldModel::Building => {
                if let Some(range) = floats(colour, "g_SpecMapRange") {
                    out.specular = [range.first().copied().unwrap_or(0.0); 3];
                    out.specular_masked = true;
                } else if let Some(c) = floats(colour, "g_SpecColorAndScale") {
                    // gf_basic_tint_reflect: coloured, not masked.
                    out.specular = [
                        c[0],
                        c.get(1).copied().unwrap_or(0.0),
                        c.get(2).copied().unwrap_or(0.0),
                    ];
                }
                if let Some(r) = floats(colour, "g_ReflectionScale") {
                    out.reflection = [r.first().copied().unwrap_or(0.0), 0.0];
                }
            }
            WorldModel::Detail => {
                out.detail = texture(colour, &["g_DetailSampler"]);
                out.mask = texture(colour, &["g_MaskSampler"]);
                out.detail_tint = tint_color("g_DetailTintColor");
                out.mask_scale = first("g_MaskRangeScale", 1.0);
                out.detail_uv = uv("g_DetailUVScaleAndOffset");
                out.mask_uv = uv("g_MaskUVScaleAndOffset");
                let z = floats(colour, "g_SpecNormMapParams")
                    .and_then(|v| v.get(2).copied())
                    .unwrap_or(0.0);
                out.specular = [z; 3];
                out.specular_masked = true;
                out.specular_lit = true;
            }
            WorldModel::Panorama => {
                out.detail = texture(colour, &["g_DetailMapSampler"]);
                // The night glow's mask, at `(uv − zw) · xy`.
                out.mask = texture(colour, &["g_DetailMaskSampler"]);
                if let Some(v) =
                    floats(colour, "g_DetailMaskUVScaleOffset").filter(|v| v.len() >= 4)
                {
                    out.mask_uv = [v[0], v[1], -v[2] * v[0], -v[3] * v[1]];
                }
                let glow = |name: &str| {
                    floats(colour, name)
                        .filter(|v| v.len() >= 4)
                        .map_or([0.0; 4], |v| [v[0], v[1], v[2], v[3]])
                };
                out.glow = [glow("g_DayGlowColor"), glow("g_NightGlowColor")];
            }
            WorldModel::Overlay => {
                out.overlay = floats(colour, "g_VertexAOParams")
                    .and_then(|v| v.get(1).copied())
                    .unwrap_or(1.0);
            }
            WorldModel::Window | WorldModel::Glass => {
                out.base = texture(colour, &["g_FrontMap"]);
                let range = floats(colour, "ReflectionRange").unwrap_or(&[0.0, 0.0]);
                out.reflection = [
                    range.first().copied().unwrap_or(0.0),
                    range.get(1).copied().unwrap_or(0.0),
                ];
                out.specular = [first("g_SpecScale", 0.0); 3];
                if model == WorldModel::Window
                    && let Some(c) = floats(colour, "m_LightColor").filter(|v| v.len() >= 3)
                {
                    out.glow[1] = [c[0], c[1], c[2], 1.0];
                }
            }
            WorldModel::Water => {
                out.base_tint = tint_color("WaterColor");
                // The low-quality water adds the highlight as it is.
                out.specular = [1.0; 3];
            }
        }
        Some(out)
    }

    /// Every texture of the game files the shading samples.
    pub fn textures(&self) -> impl Iterator<Item = u32> + '_ {
        [
            self.base,
            self.detail,
            self.mask,
            self.normal,
            self.detail_normal,
            self.gloss,
            self.exponent_map,
        ]
        .into_iter()
        .flatten()
    }
}

/// Sharpness of the highlight of passes that do not name one
/// (`DeferredNormalsFast*`, water): not found in the shaders' parameters.
const DEFAULT_EXPONENT: f32 = 0.5;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::crc32;

    fn param(name: &str, value: ParamValue) -> Param {
        Param {
            hash: crc32(name.as_bytes()),
            name: name.into(),
            source: String::new(),
            value,
        }
    }

    fn material(depth: &str, lit: &str, normals: Vec<Param>, colour: Vec<Param>) -> Material {
        let n = normals.len() as u8;
        let c = colour.len() as u8;
        let mut params = normals;
        params.extend(colour);
        Material {
            version: 19,
            name: "test".into(),
            name_hash: 0,
            flags: 0,
            shaders: [
                (crc32(depth.to_ascii_lowercase().as_bytes()), 0, n),
                (crc32(lit.to_ascii_lowercase().as_bytes()), n, c),
                (0, 0, 0),
                (0, 0, 0),
            ],
            shadow_shader: 0,
            params,
        }
    }

    #[test]
    fn models_of_the_lit_shaders() {
        assert_eq!(
            WorldModel::of("gf_building_detail"),
            Some(WorldModel::Detail)
        );
        assert_eq!(WorldModel::of("gf_road"), Some(WorldModel::Detail));
        assert_eq!(WorldModel::of("gf_building"), Some(WorldModel::Building));
        assert_eq!(
            WorldModel::of("gf_basic_tint_reflect"),
            Some(WorldModel::Building)
        );
        assert_eq!(WorldModel::of("gf_basic"), Some(WorldModel::Basic));
        assert_eq!(
            WorldModel::of("gf_normal_spec_reflect"),
            Some(WorldModel::Basic)
        );
        assert_eq!(WorldModel::of("WindowGlass"), Some(WorldModel::Glass));
        assert_eq!(WorldModel::of("WindowGlow"), Some(WorldModel::Window));
        assert_eq!(WorldModel::of("WaterOceanHiRes"), Some(WorldModel::Water));
        assert_eq!(WorldModel::of("gf_char_skin"), None);
        assert_eq!(WorldModel::of("sky_dome"), None);
        assert_eq!(WorldModel::of("TreeTrunk"), None);
        assert_eq!(WorldModel::of("gf_basic_non_lit_vfx"), None);
    }

    #[test]
    fn building_detail_material() {
        let normals = vec![
            param("g_materialNormalMap", ParamValue::Texture(0x7F00_D6F1)),
            param("g_materialNormalMapScale", ParamValue::Floats(vec![1.0])),
            param("g_materialSpecMap", ParamValue::Texture(5)),
        ];
        let colour = vec![
            param("g_BaseSampler", ParamValue::Texture(1)),
            param(
                "g_BaseTintColor",
                ParamValue::Floats(vec![0.5, 0.5, 0.5, 1.0]),
            ),
            param("g_DetailSampler", ParamValue::Texture(2)),
            param(
                "g_DetailTintColor",
                ParamValue::Floats(vec![1.0, 1.0, 1.0, 1.0]),
            ),
            param(
                "g_DetailUVScaleAndOffset",
                ParamValue::Floats(vec![1.5, 1.5, 0.25, 0.0]),
            ),
            param("g_GlossMapSampler", ParamValue::Texture(0xE993_0443)),
            param("g_MaskRangeScale", ParamValue::Floats(vec![0.65])),
            param("g_MaskSampler", ParamValue::Texture(3)),
            param(
                "g_MaskUVScaleAndOffset",
                ParamValue::Floats(vec![0.3, 0.25, -3.0, 0.4]),
            ),
            param(
                "g_SpecNormMapParams",
                ParamValue::Floats(vec![0.0, 0.0, 0.2]),
            ),
        ];
        let m = material("DeferredNormals", "gf_building_detail", normals, colour);
        let s = WorldShading::from_material(&m).unwrap();
        assert_eq!(s.model, WorldModel::Detail);
        assert_eq!((s.base, s.detail, s.mask), (Some(1), Some(2), Some(3)));
        // Built-in textures are left out: flat normal, black gloss.
        assert_eq!((s.normal, s.gloss), (None, None));
        assert_eq!(s.exponent_map, Some(5));
        assert!((s.base_tint[0] - 0.5f32.powf(2.2)).abs() < 1e-6);
        assert_eq!(s.base_tint[3], 1.0);
        assert_eq!(s.mask_scale, 0.65);
        assert_eq!(s.detail_uv, [1.5, 1.5, 0.25, 0.0]);
        assert_eq!(s.mask_uv, [0.3, 0.25, -3.0, 0.4]);
        assert_eq!(s.specular, [0.2; 3]);
        assert!(s.specular_masked && s.specular_lit && !s.alpha_test);
        assert_eq!(s.textures().collect::<Vec<_>>(), [1, 2, 3, 5]);
    }

    #[test]
    fn basic_and_window_materials() {
        let normals = vec![param("g_SpecParams", ParamValue::Floats(vec![0.3]))];
        let colour = vec![
            param("g_BaseSampler", ParamValue::Texture(1)),
            param("g_SpecularScale", ParamValue::Floats(vec![0.4])),
        ];
        let mut m = material("DepthAlphaTestSpecExpLowResPS", "gf_basic", normals, colour);
        m.flags = FLAG_ALPHA_TEST;
        let s = WorldShading::from_material(&m).unwrap();
        assert_eq!(s.model, WorldModel::Basic);
        assert_eq!(s.exponent, 0.3);
        assert_eq!(s.specular, [0.4; 3]);
        assert!(s.alpha_test && !s.specular_masked);

        let colour = vec![
            param("ReflectionRange", ParamValue::Floats(vec![0.1, 0.03])),
            param("g_FrontMap", ParamValue::Texture(9)),
            param("g_SpecScale", ParamValue::Floats(vec![0.25])),
            param("m_LightColor", ParamValue::Floats(vec![0.7, 0.6, 0.5, 1.0])),
        ];
        let m = material("DeferredNormalsFast", "WindowGlow", Vec::new(), colour);
        let s = WorldShading::from_material(&m).unwrap();
        assert_eq!(s.model, WorldModel::Window);
        assert_eq!(s.base, Some(9));
        assert_eq!(s.reflection, [0.1, 0.03]);
        assert_eq!(s.specular, [0.25; 3]);
        assert_eq!(
            s.glow,
            [[0.0; 4], [0.7, 0.6, 0.5, 1.0]],
            "lit only at night"
        );
    }

    #[test]
    fn panoramas_glow_by_day_and_night() {
        let colour = vec![
            param("g_BaseSampler", ParamValue::Texture(1)),
            param("g_DetailMapSampler", ParamValue::Texture(2)),
            param("g_DetailMaskSampler", ParamValue::Texture(3)),
            param(
                "g_DetailMaskUVScaleOffset",
                ParamValue::Floats(vec![2.0, 4.0, 0.5, 0.25]),
            ),
            param(
                "g_DayGlowColor",
                ParamValue::Floats(vec![0.5, 0.5, 0.5, 0.4]),
            ),
            param(
                "g_NightGlowColor",
                ParamValue::Floats(vec![1.0, 0.9, 0.7, 1.0]),
            ),
        ];
        let m = material("DeferredNormalsFast", "gf_pano", Vec::new(), colour);
        let s = WorldShading::from_material(&m).unwrap();
        assert_eq!(s.model, WorldModel::Panorama);
        assert_eq!((s.detail, s.mask), (Some(2), Some(3)));
        // (uv − zw) · xy as uv · xy + zw.
        assert_eq!(s.mask_uv, [2.0, 4.0, -1.0, -1.0]);
        assert_eq!(s.glow[0], [0.5, 0.5, 0.5, 0.4]);
        assert_eq!(s.glow[1][2], 0.7);
    }

    #[test]
    fn builtin_textures_by_hash_and_name() {
        assert_eq!(Builtin::of(0xE993_0443), Some(Builtin::Specular));
        assert_eq!(Builtin::of(1), None);
        assert_eq!(Builtin::named("normal"), Some(Builtin::Normal));
        assert_eq!(
            Builtin::named("x:/assets/_library/textures/runtime_generated/~cubemapsky.tga"),
            Some(Builtin::Cubemap)
        );
    }
}
