// Scene shader: lit, textured meshes, an id pass for picking and lines.

struct Globals {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // xyz: direction towards the light, w: ambient term.
    light: vec4<f32>,
    // x: flags (FLAG_*), y: bone shown by the weight view,
    // z: highlighted pick id (0 = none), w: highlighted part (0xFFFFFFFF = all).
    params: vec4<u32>,
    // x: depth offset of lines in clip space (fraction of w).
    line: vec4<f32>,
    // rgb: colour of the light (linear), w: weight of the light from the camera.
    sun: vec4<f32>,
    // rgb: ambient colour multiplying light.w.
    ambient: vec4<f32>,
    // rgb: fog colour (linear).
    fog_color: vec4<f32>,
    // x: fog start, y: fog end (metres from the eye), z: fog at the end (0 = none).
    fog: vec4<f32>,
    // Camera right, up and forward in the world (leaf cards).
    camera: array<vec4<f32>, 3>,
    game: Game,
};

// "As in the game" shading (game_look.rs, docs/analysis/renderer.md §10).
struct Game {
    inv_view_proj: mat4x4<f32>,
    // rgb: sun colour in the light buffer (linear), w: wrap (g_diffuseAmbient).
    sun: vec4<f32>,
    // x: exposure (2), y: ambient scale, z: fog on, w: sky on.
    params: vec4<f32>,
    // Rows of g_SHMatrixR (0-3), G (4-7), B (8-11).
    ambient: array<vec4<f32>, 12>,
    // rgb: m_FogColor (linear), w: amount at the start.
    fog_near: vec4<f32>,
    // rgb: m_FogEndColor (linear), w: amount at the end.
    fog_far: vec4<f32>,
    // x: start, y: 1 / (end - start).
    fog_range: vec4<f32>,
    sky_low: vec4<f32>,
    sky_high: vec4<f32>,
    glow_low: vec4<f32>,
    glow_high: vec4<f32>,
    // x: haze band (rad), y: glow strength, z: glow exponent above the band, w: at the horizon.
    sky_params: vec4<f32>,
};

const FLAG_TEXTURES: u32 = 1u;
const FLAG_LIGHTING: u32 = 2u;
const FLAG_VERTEX_COLORS: u32 = 4u;
const FLAG_ALPHA_TEST: u32 = 8u;
const FLAG_BONE_WEIGHTS: u32 = 16u;
const FLAG_SURFACES: u32 = 32u;
const FLAG_GAME: u32 = 64u;

// Surface models (Material.flags.y); 0 = plain texture × colour.
const MODEL_BASIC: u32 = 1u;
const MODEL_TINT_MASK: u32 = 2u;
const MODEL_HAIR: u32 = 3u;
const MODEL_SKIN: u32 = 4u;
const MODEL_EYE: u32 = 5u;
// World surfaces, shaded only as in the game (docs/analysis/renderer.md §10.7).
const MODEL_WORLD_BASIC: u32 = 6u;
const MODEL_BUILDING: u32 = 7u;
const MODEL_DETAIL: u32 = 8u;
const MODEL_PANORAMA: u32 = 9u;
const MODEL_OVERLAY: u32 = 10u;
const MODEL_WINDOW: u32 = 11u;
const MODEL_GLASS: u32 = 12u;
const MODEL_WATER: u32 = 13u;

// Textures a surface has (Material.flags.z).
const HAS_NORMAL: u32 = 1u;
const HAS_GLOSS: u32 = 2u;
const HAS_DETAIL_NORMAL: u32 = 4u;
const HAS_DETAIL_MASK: u32 = 8u;
const HAS_DETAIL_GLOSS: u32 = 16u;
const IS_SKY: u32 = 32u;
// World surfaces: an exponent map is bound; the highlight is masked by the
// gloss map's red, multiplied by the light; the reflection is masked by the
// gloss map's green; the base texture's alpha cuts; base and detail textures
// are sampled as sRGB (already linear).
const HAS_EXPONENT: u32 = 64u;
const SPEC_MASKED: u32 = 128u;
const SPEC_LIT: u32 = 256u;
const REFLECT_MASKED: u32 = 512u;
const ALPHA_CUT: u32 = 1024u;
const BASE_SRGB: u32 = 2048u;
const DETAIL_SRGB: u32 = 4096u;

struct Material {
    color: vec4<f32>,
    // x: 1 when the material has a texture, y: surface model, z: HAS_* bits, w: blended.
    flags: vec4<u32>,
    tint0: vec4<f32>,
    tint1: vec4<f32>,
    tint2: vec4<f32>,
    tint: vec4<f32>,
    // rgb: skin tint, w: exponent of the base texture.
    skin: vec4<f32>,
    detail_uv: vec4<f32>,
    mask_uv: vec4<f32>,
    // rgb: specular colour, w: exponent.
    spec: vec4<f32>,
    // x: base weight under the detail mask, y: detail normal weight.
    params: vec4<f32>,
    // World surfaces: tint of the base layer where its alpha is set (water:
    // its colour), w: alpha; tint of the detail layer, w: its alpha.
    base_tint: vec4<f32>,
    detail_tint: vec4<f32>,
    // x: mask scale, y: normal map scale, z: highlight sharpness (or factor
    // on the exponent map's blue), w: how much the overlay darkens.
    world: vec4<f32>,
    // x: reflection facing the camera, y: extra at grazing angles.
    reflection: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var diffuse: texture_2d<f32>;
@group(1) @binding(2) var diffuse_sampler: sampler;
@group(1) @binding(3) var normal_map: texture_2d<f32>;
@group(1) @binding(4) var gloss_map: texture_2d<f32>;
@group(1) @binding(5) var detail_normal_map: texture_2d<f32>;
// World surfaces: their mask.
@group(1) @binding(6) var detail_mask_map: texture_2d<f32>;
// World surfaces: the detail layer.
@group(1) @binding(7) var detail_gloss_map: texture_2d<f32>;

struct InstanceIn {
    @location(8) m0: vec4<f32>,
    @location(9) m1: vec4<f32>,
    @location(10) m2: vec4<f32>,
    @location(11) m3: vec4<f32>,
    @location(12) tint: vec4<f32>,
    // x: pick id (instance index + 1), y: flags (unused).
    @location(13) ids: vec2<u32>,
};

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) joints: vec4<u32>,
    @location(5) weights: vec4<f32>,
    @location(6) part: u32,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) tint: vec4<f32>,
    @location(5) @interpolate(flat) ids: vec2<u32>,
    @location(6) @interpolate(flat) part: u32,
    @location(7) bone_weight: f32,
};

// Vertex.part bit: a corner of a leaf card turned to the camera (trees,
// docs/formats/treemodel.md). position: card centre, normal: corner offset in
// camera space, weights: card normal in camera space as n / 2 + 1/2.
const BILLBOARD: u32 = 0x80000000u;

fn camera_vector(v: vec3<f32>) -> vec3<f32> {
    return v.x * globals.camera[0].xyz + v.y * globals.camera[1].xyz + v.z * globals.camera[2].xyz;
}

// World position and normal of a vertex.
struct Placed {
    world: vec4<f32>,
    normal: vec3<f32>,
};

fn place(v: VertexIn, model: mat4x4<f32>) -> Placed {
    var out: Placed;
    out.world = model * vec4<f32>(v.position, 1.0);
    out.normal = (model * vec4<f32>(v.normal, 0.0)).xyz;
    if ((v.part & BILLBOARD) != 0u) {
        // The game scales the corners by the object's vertical scale.
        let scale = length(model[1].xyz);
        out.world = vec4<f32>(out.world.xyz + camera_vector(v.normal) * scale, out.world.w);
        out.normal = camera_vector(v.weights.xyz * 2.0 - 1.0);
    }
    return out;
}

@vertex
fn vs_mesh(v: VertexIn, inst: InstanceIn) -> VertexOut {
    let model = mat4x4<f32>(inst.m0, inst.m1, inst.m2, inst.m3);
    let placed = place(v, model);
    let world = placed.world;
    var out: VertexOut;
    out.clip = globals.view_proj * world;
    out.world = world.xyz;
    out.normal = placed.normal;
    out.uv = v.uv;
    out.color = v.color;
    out.tint = inst.tint;
    out.ids = inst.ids;
    out.part = v.part & ~BILLBOARD;
    var weight = 0.0;
    if ((v.part & BILLBOARD) == 0u) {
        for (var i = 0; i < 4; i++) {
            if (v.joints[i] == globals.params.y) {
                weight += v.weights[i];
            }
        }
    }
    out.bone_weight = weight;
    return out;
}

fn base_color(v: VertexOut) -> vec4<f32> {
    var color = material.color;
    let texel = textureSample(diffuse, diffuse_sampler, v.uv);
    if ((globals.params.x & FLAG_TEXTURES) != 0u && material.flags.x != 0u) {
        color = color * texel;
    }
    if ((globals.params.x & FLAG_VERTEX_COLORS) != 0u) {
        color = vec4<f32>(color.rgb * v.color.rgb, color.a);
    }
    return color * v.tint;
}

fn has(bit: u32) -> bool {
    return (material.flags.z & bit) != 0u;
}

fn uses_surface() -> bool {
    return (globals.params.x & FLAG_SURFACES) != 0u && material.flags.y != 0u
        && (globals.params.x & FLAG_TEXTURES) != 0u
        && (material.flags.y < MODEL_WORLD_BASIC || (globals.params.x & FLAG_GAME) != 0u);
}

fn is_world() -> bool {
    return material.flags.y >= MODEL_WORLD_BASIC;
}

// Colour and alpha of a surface the way the game's pixel shaders form them
// (linear space); see docs/viewers.md §7.
struct SurfaceColor {
    albedo: vec3<f32>,
    alpha: f32,
};

fn surface_color(base: vec4<f32>, gloss: vec4<f32>, mask: vec4<f32>) -> SurfaceColor {
    var out: SurfaceColor;
    let model = material.flags.y;
    let colour = pow(max(base.rgb, vec3<f32>(0.0)), vec3<f32>(material.skin.w));
    out.albedo = colour;
    out.alpha = base.a;
    if (model == MODEL_TINT_MASK) {
        let mixed = base.r * material.tint0.rgb + base.g * material.tint1.rgb
            + base.b * material.tint2.rgb;
        var weight = 0.0;
        if (has(HAS_DETAIL_MASK)) {
            weight = mask.b * material.params.x;
        }
        out.albedo = mix(mixed, base.rgb, weight);
    } else if (model == MODEL_HAIR) {
        let one = vec3<f32>(1.0);
        out.albedo = one - (one - base.r * material.tint0.rgb) * (one - base.g * material.tint1.rgb)
            * (one - base.b * material.tint2.rgb);
        if (has(HAS_GLOSS)) {
            out.alpha = gloss.g;
        }
    } else if (model == MODEL_SKIN) {
        var weight = 0.0;
        if (has(HAS_GLOSS)) {
            weight = gloss.r;
        }
        out.albedo = mix(colour, colour * material.skin.rgb, weight);
    } else if (model == MODEL_EYE) {
        // gf_char_eye: the iris (base alpha) takes the eye colour; opaque.
        out.albedo = mix(colour, colour * material.skin.rgb, base.a);
        out.alpha = 1.0;
    }
    out.albedo = out.albedo * material.tint.rgb;
    out.alpha = out.alpha * material.tint.a;
    return out;
}

// Normal from a tangent-space map with a tangent frame from screen-space
// derivatives (the meshes' own tangents are not uploaded).
fn perturb(n: vec3<f32>, p: vec3<f32>, uv: vec2<f32>, t: vec3<f32>) -> vec3<f32> {
    let dp1 = dpdx(p);
    let dp2 = dpdy(p);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let tangent = dp2perp * duv1.x + dp1perp * duv2.x;
    let bitangent = dp2perp * duv1.y + dp1perp * duv2.y;
    let scale = inverseSqrt(max(max(dot(tangent, tangent), dot(bitangent, bitangent)), 1e-20));
    return normalize(tangent * scale * t.x + bitangent * scale * t.y + n * t.z);
}

fn is_highlighted(v: VertexOut) -> bool {
    return globals.params.z != 0u && v.ids.x == globals.params.z
        && (globals.params.w == 0xFFFFFFFFu || globals.params.w == v.part);
}

@fragment
fn fs_mesh(v: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Every texture is sampled in uniform control flow.
    let detail_uv = v.uv * material.detail_uv.xy + material.detail_uv.zw;
    let mask_uv = v.uv * material.mask_uv.xy + material.mask_uv.zw;
    let base = textureSample(diffuse, diffuse_sampler, v.uv);
    // Alpha-tested plain surfaces (leaves of trees, most of the world) are
    // cut before the other maps are read: the same test as below.
    if (!uses_surface() && (globals.params.x & FLAG_ALPHA_TEST) != 0u && base_color(v).a < 0.5) {
        discard;
    }
    let normal_texel = textureSample(normal_map, diffuse_sampler, v.uv);
    let gloss_texel = textureSample(gloss_map, diffuse_sampler, v.uv);
    let detail_normal_texel = textureSample(detail_normal_map, diffuse_sampler, detail_uv);
    let mask_texel = textureSample(detail_mask_map, diffuse_sampler, mask_uv);
    let detail_gloss_texel = textureSample(detail_gloss_map, diffuse_sampler, detail_uv);
    if ((globals.params.x & FLAG_GAME) != 0u && (globals.params.x & FLAG_BONE_WEIGHTS) == 0u
        && uses_surface() && is_world()) {
        let w = world_fragment(
            v,
            base,
            normal_texel,
            gloss_texel,
            detail_normal_texel,
            mask_texel,
            detail_gloss_texel,
        );
        var lit = w.rgb;
        if (is_highlighted(v)) {
            lit = mix(lit, vec3<f32>(1.0, 0.35, 0.02), 0.45);
        }
        let out = pow(clamp(lit, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2));
        if (material.flags.w == 0u) {
            return vec4<f32>(out, 1.0);
        }
        // Blended (premultiplied): glass gives its colour and how much it
        // covers; the others colour × alpha.
        let alpha = clamp(w.a, 0.0, 1.0);
        if (material.flags.y == MODEL_GLASS) {
            return vec4<f32>(out, alpha);
        }
        return vec4<f32>(out * alpha, alpha);
    }
    var geometric = normalize(v.normal);
    if (!front) {
        geometric = -geometric;
    }
    // Tangent-space normal with the detail normal blended in.
    var t = normal_texel.xyz * 2.0 - 1.0;
    if (has(HAS_DETAIL_NORMAL)) {
        var weight = material.params.y;
        if (has(HAS_DETAIL_MASK)) {
            weight = weight * mask_texel.g;
        }
        t = vec3<f32>(t.xy + (detail_normal_texel.xy * 2.0 - 1.0) * weight, t.z);
    }
    let mapped = perturb(geometric, v.world, v.uv, normalize(t));

    var color = base_color(v);
    var gloss = vec4<f32>(0.0, 1.0, 0.5, 1.0);
    if (has(HAS_GLOSS)) {
        gloss = gloss_texel;
    }
    if (has(HAS_DETAIL_GLOSS)) {
        gloss = vec4<f32>(gloss.rgb * detail_gloss_texel.rgb, gloss.a);
    }
    let surface = uses_surface();
    if (surface) {
        let s = surface_color(base, gloss, mask_texel);
        color = vec4<f32>(s.albedo * material.color.rgb, s.alpha * material.color.a) * v.tint;
        if ((globals.params.x & FLAG_VERTEX_COLORS) != 0u) {
            color = vec4<f32>(color.rgb * v.color.rgb, color.a);
        }
    }
    if ((globals.params.x & FLAG_GAME) != 0u && has(IS_SKY)) {
        discard;
    }
    let blended = material.flags.w != 0u && surface;
    if (blended && color.a < 0.02) {
        discard;
    }
    if (!blended && (globals.params.x & FLAG_ALPHA_TEST) != 0u && color.a < 0.5) {
        discard;
    }
    if ((globals.params.x & FLAG_BONE_WEIGHTS) != 0u) {
        let w = clamp(v.bone_weight, 0.0, 1.0);
        let heat = mix(vec3<f32>(0.08, 0.1, 0.35), vec3<f32>(1.0, 0.25, 0.05), w);
        color = vec4<f32>(mix(heat, vec3<f32>(1.0, 1.0, 0.3), max(w - 0.8, 0.0) * 5.0), 1.0);
    }
    var lit = color.rgb;
    let game = (globals.params.x & FLAG_GAME) != 0u && (globals.params.x & FLAG_BONE_WEIGHTS) == 0u;
    if (game) {
        // The game lights the normals as they are: no turning towards the
        // viewer (world meshes are wound the other way round than characters).
        var n = select(-geometric, geometric, front);
        var ao = 1.0;
        var spec_mask = 0.0;
        if (surface) {
            if (has(HAS_NORMAL) || has(HAS_DETAIL_NORMAL)) {
                n = select(-mapped, mapped, front);
            }
            if (material.flags.y == MODEL_TINT_MASK || material.flags.y == MODEL_SKIN) {
                ao = gloss.g;
            }
            spec_mask = gloss.b;
        }
        let light = game_light(n, v.world);
        lit = color.rgb * light.rgb * globals.game.params.x * ao;
        if (surface) {
            // The light buffer's alpha is the sun's highlight; the lit pass adds
            // it with the specular colour and mask.
            lit = lit + material.spec.rgb * spec_mask * light.a * ao;
        }
        lit = game_fog(lit, v.world);
    } else if ((globals.params.x & FLAG_LIGHTING) != 0u && surface && (globals.params.x & FLAG_BONE_WEIGHTS) == 0u) {
        var n = geometric;
        if (has(HAS_NORMAL) || has(HAS_DETAIL_NORMAL)) {
            n = mapped;
        }
        let l = normalize(globals.light.xyz);
        let view_dir = normalize(globals.eye.xyz - v.world);
        let diffuse_term = max(dot(n, l), 0.0);
        let head = max(dot(n, view_dir), 0.0) * 0.35 * globals.sun.w;
        let h = normalize(l + view_dir);
        // Clothes and skin multiply every light term by the gloss map's green.
        var ao = 1.0;
        if (material.flags.y == MODEL_TINT_MASK || material.flags.y == MODEL_SKIN) {
            ao = gloss.g;
        }
        let specular = pow(max(dot(n, h), 0.0), max(material.spec.w, 1.0)) * material.spec.rgb
            * gloss.b * diffuse_term * globals.sun.rgb;
        let light = globals.light.w * globals.ambient.rgb + diffuse_term * 0.65 * globals.sun.rgb
            + vec3<f32>(head);
        lit = (color.rgb * light + specular) * ao;
    } else if ((globals.params.x & FLAG_LIGHTING) != 0u) {
        let n = geometric;
        let l = normalize(globals.light.xyz);
        let view_dir = normalize(globals.eye.xyz - v.world);
        let diffuse_term = max(dot(n, l), 0.0);
        // Light from the camera keeps faces turned away from the sun readable.
        let head = max(dot(n, view_dir), 0.0) * 0.35 * globals.sun.w;
        let h = normalize(l + view_dir);
        let specular = pow(max(dot(n, h), 0.0), 48.0) * 0.12 * globals.sun.rgb;
        let light = globals.light.w * globals.ambient.rgb + diffuse_term * 0.65 * globals.sun.rgb
            + vec3<f32>(head);
        lit = color.rgb * light + specular;
    }
    if (globals.fog.z > 0.0 && !game) {
        let distance = length(globals.eye.xyz - v.world);
        let f = clamp((distance - globals.fog.x) / (globals.fog.y - globals.fog.x), 0.0, 1.0);
        lit = mix(lit, globals.fog_color.rgb, f * globals.fog.z);
    }
    if (is_highlighted(v)) {
        lit = mix(lit, vec3<f32>(1.0, 0.35, 0.02), 0.45);
    }
    // The target holds gamma-encoded values, as egui expects of its images.
    var alpha = 1.0;
    if (blended) {
        alpha = clamp(color.a, 0.0, 1.0);
    }
    // Blended parts are drawn premultiplied.
    if (game) {
        // The lit pass ends with pow(x, 1/2.2), not the sRGB curve.
        return vec4<f32>(pow(clamp(lit, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2)) * alpha, alpha);
    }
    return vec4<f32>(linear_to_srgb(lit) * alpha, alpha);
}

// Colour of a world texture in linear space: the game raises it to 2.2; a
// texture sampled as sRGB is linear already.
fn to_linear(c: vec3<f32>, srgb: bool) -> vec3<f32> {
    if (srgb) {
        return c;
    }
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.2));
}

// A world surface as the game's normal, light and lit passes draw it
// (docs/analysis/renderer.md §10.7): linear colour before the gamma, alpha.
// The textures are the ones fs_mesh samples: the mask is bound as the detail
// mask (at the mask UV), the detail layer as the detail gloss map (at the
// detail UV), the exponent map is the gloss map.
fn world_fragment(
    v: VertexOut,
    base: vec4<f32>,
    normal_texel: vec4<f32>,
    gloss_texel: vec4<f32>,
    detail_normal_texel: vec4<f32>,
    mask: vec4<f32>,
    detail: vec4<f32>,
) -> vec4<f32> {
    let model = material.flags.y;
    // Weight of the base layer over the detail layer (and of the normal map
    // over the detail normal map).
    let m = mask.r * material.world.x;
    // Vertex colour: green is the baked occlusion, alpha multiplies the alpha.
    let ao = v.color.g;
    let b = to_linear(base.rgb, has(BASE_SRGB));
    let tinted = mix(b, b * material.base_tint.rgb, base.a);
    let d = to_linear(detail.rgb, has(DETAIL_SRGB));
    var albedo = b;
    var alpha = base.a * v.color.a;
    if (model == MODEL_BUILDING) {
        albedo = tinted;
        alpha = material.base_tint.a * v.color.a;
    } else if (model == MODEL_DETAIL) {
        let layer = mix(d, d * material.detail_tint.rgb, detail.a);
        albedo = mix(layer, tinted, m);
        alpha = mix(material.detail_tint.a, material.base_tint.a, m) * v.color.a;
    } else if (model == MODEL_PANORAMA) {
        albedo = mix(b, b * material.base_tint.rgb, detail.b);
        alpha = base.a;
    } else if (model == MODEL_WINDOW || model == MODEL_GLASS) {
        // The front map is bound as the detail texture.
        albedo = d;
        alpha = 1.0;
    } else if (model == MODEL_WATER) {
        albedo = material.base_tint.rgb;
        alpha = 1.0;
    }
    alpha = alpha * material.tint.a;
    if (has(ALPHA_CUT) && base.a < 0.5) {
        discard;
    }

    // The normal pass: the vertex normal tilted by the normal map (the
    // detail normal map where the mask is low).
    var n = normalize(v.normal);
    if (has(HAS_NORMAL) || has(HAS_DETAIL_NORMAL)) {
        var xy = normal_texel.xy * 2.0 - 1.0;
        if (has(HAS_DETAIL_NORMAL)) {
            xy = mix(detail_normal_texel.xy * 2.0 - 1.0, xy, m);
        }
        n = perturb(n, v.world, v.uv, vec3<f32>(xy * material.world.y, 1.0));
    }
    var s = material.world.z;
    if (has(HAS_EXPONENT)) {
        s = gloss_texel.b * s;
    }
    let light = game_light_power(n, v.world, s * s * s * 255.0 + 1.5e-5);
    let lit_light = light.rgb * globals.game.params.x;

    // The lit pass.
    var spec_mask = 1.0;
    if (has(SPEC_MASKED)) {
        spec_mask = select(0.0, gloss_texel.r, has(HAS_GLOSS));
    }
    var spec = material.spec.rgb * spec_mask * light.a;
    if (has(SPEC_LIT)) {
        spec = spec * lit_light;
    }
    var colour = (albedo * lit_light + spec) * ao;
    // Reflection of the environment (the game's run-time cube map; here the sky).
    var env = vec3<f32>(0.0);
    let window = model == MODEL_WINDOW || model == MODEL_GLASS;
    if (material.reflection.x > 0.0 || material.reflection.y > 0.0) {
        let view = normalize(v.world - globals.eye.xyz);
        let vn = dot(view, n);
        var dir = view - 2.0 * vn * n;
        if (window) {
            // Windows look the sky up mirrored above the horizon.
            dir.y = abs(dir.y);
        }
        var k = material.reflection.x + (1.0 - abs(vn)) * material.reflection.y;
        if (has(REFLECT_MASKED)) {
            k = k * select(0.0, gloss_texel.g, has(HAS_GLOSS));
        }
        env = pow(sky_colour(dir), vec3<f32>(2.2)) * k;
    }
    if (model == MODEL_GLASS) {
        // Not lit; blended premultiplied, letting (1 − front.a) · tint.w through.
        colour = d * detail.a + env;
        alpha = 1.0 - (1.0 - detail.a) * material.tint.a;
    } else if (model == MODEL_WINDOW) {
        colour = d * lit_light + clamp(env * detail.a, vec3<f32>(0.0), vec3<f32>(1.0))
            + material.spec.rgb * light.a;
    } else if (model == MODEL_OVERLAY) {
        // Darkens what is below towards the fog: black without fog.
        colour = vec3<f32>(0.0);
        alpha = 1.0 - material.world.w * ao;
    } else {
        colour = colour + env;
    }
    return vec4<f32>(game_fog(colour, v.world), alpha);
}

// The game's light buffer at a point: the sun, wrapped (saturate(n·l + wrap)),
// and the ambient light of the three matrices, clamped to 0..1 like the 8-bit
// buffer; alpha: the sun's highlight.
fn game_light(n: vec3<f32>, world: vec3<f32>) -> vec4<f32> {
    return game_light_power(n, world, 24.0);
}

// game_light with the highlight's exponent (the game: s³·255 from the normal
// pass, renderer.md §10.7).
fn game_light_power(n: vec3<f32>, world: vec3<f32>, power: f32) -> vec4<f32> {
    let g = globals.game;
    let l = normalize(globals.light.xyz);
    let ndl = dot(n, l);
    let sun = clamp(ndl + g.sun.w, 0.0, 1.0) * g.sun.rgb;
    let n4 = vec4<f32>(n, 1.0);
    let r = dot(n4, n.x * g.ambient[0] + n.y * g.ambient[1] + n.z * g.ambient[2] + g.ambient[3]);
    let gg = dot(n4, n.x * g.ambient[4] + n.y * g.ambient[5] + n.z * g.ambient[6] + g.ambient[7]);
    let b = dot(n4, n.x * g.ambient[8] + n.y * g.ambient[9] + n.z * g.ambient[10] + g.ambient[11]);
    let ambient = max(vec3<f32>(r, gg, b), vec3<f32>(0.0)) * g.params.y;
    let view_dir = normalize(globals.eye.xyz - world);
    let h = normalize(l + view_dir);
    var highlight = 0.0;
    if (ndl > 0.0) {
        highlight = pow(max(dot(n, h), 0.0), power) * clamp(ndl + g.sun.w, 0.0, 1.0);
    }
    return vec4<f32>(min(sun + ambient, vec3<f32>(1.0)), highlight);
}

// m_FogColor → m_FogEndColor over the fog's range; how much it hides goes from
// the amount at the start to the amount at the end, none before the start.
fn game_fog(c: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let g = globals.game;
    if (g.params.z == 0.0) {
        return c;
    }
    let d = length(world - globals.eye.xyz);
    let f = clamp((d - g.fog_range.x) * g.fog_range.y, 0.0, 1.0);
    if (f <= 0.0) {
        return c;
    }
    let colour = mix(g.fog_near.rgb, g.fog_far.rgb, f);
    let amount = mix(g.fog_near.w, g.fog_far.w, f);
    return mix(c, colour, amount);
}

// The sky dome (sky_dome pixel shader): colour by the height above the
// horizon within the haze band, plus a glow around the sun.
struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> SkyOut {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var out: SkyOut;
    out.clip = vec4<f32>(xy, 1.0, 1.0);
    out.ndc = xy;
    return out;
}

@fragment
fn fs_sky(v: SkyOut) -> @location(0) vec4<f32> {
    let g = globals.game;
    // Two points along the pixel's ray (not the far plane: it may be at infinity).
    let a = g.inv_view_proj * vec4<f32>(v.ndc, 0.5, 1.0);
    let b = g.inv_view_proj * vec4<f32>(v.ndc, 0.0, 1.0);
    let dir = normalize(a.xyz / a.w - b.xyz / b.w);
    return vec4<f32>(sky_colour(dir), 1.0);
}

// Colour of the sky dome in a direction (gamma-encoded, as the game stores
// it); world surfaces reflect it in place of the environment map the game
// renders at run time.
fn sky_colour(dir: vec3<f32>) -> vec3<f32> {
    let g = globals.game;
    let horizontal = length(dir.xz);
    let elevation = atan2(dir.y, max(horizontal, 1e-5));
    let h = clamp(elevation / max(g.sky_params.x, 1e-3), 0.0, 1.0);
    let k = 1.0 - (1.0 - h) * (1.0 - h);
    let sky = mix(g.sky_low.rgb, g.sky_high.rgb, k);
    let power = mix(g.sky_params.w, g.sky_params.z, h);
    let to_sun = normalize(globals.light.xyz);
    let glow = pow(clamp(dot(dir, to_sun), 0.0, 1.0), max(power, 1e-3)) * g.sky_params.y;
    let colour = sky + mix(g.glow_low.rgb, g.glow_high.rgb, h) * glow;
    return clamp(colour, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let low = x * 12.92;
    let high = 1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, x <= vec3<f32>(0.0031308));
}

@fragment
fn fs_pick(v: VertexOut) -> @location(0) vec2<u32> {
    let color = base_color(v);
    if ((globals.params.x & FLAG_ALPHA_TEST) != 0u && color.a < 0.5) {
        discard;
    }
    return vec2<u32>(v.ids.x, v.part);
}

// Wireframe: the mesh vertices drawn as lines, pulled slightly towards the camera.
@vertex
fn vs_wire(v: VertexIn, inst: InstanceIn) -> @builtin(position) vec4<f32> {
    let model = mat4x4<f32>(inst.m0, inst.m1, inst.m2, inst.m3);
    var clip = globals.view_proj * place(v, model).world;
    clip.z -= globals.line.x * clip.w;
    return clip;
}

@fragment
fn fs_wire() -> @location(0) vec4<f32> {
    return vec4<f32>(0.05, 0.05, 0.05, 1.0);
}

struct LineIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(v: LineIn, inst: InstanceIn) -> LineOut {
    let model = mat4x4<f32>(inst.m0, inst.m1, inst.m2, inst.m3);
    var out: LineOut;
    out.clip = globals.view_proj * model * vec4<f32>(v.position, 1.0);
    out.clip.z -= globals.line.x * out.clip.w;
    out.color = v.color;
    return out;
}

@fragment
fn fs_line(v: LineOut) -> @location(0) vec4<f32> {
    return v.color;
}
