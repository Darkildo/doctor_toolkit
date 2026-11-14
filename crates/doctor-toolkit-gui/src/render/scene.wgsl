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
};

const FLAG_TEXTURES: u32 = 1u;
const FLAG_LIGHTING: u32 = 2u;
const FLAG_VERTEX_COLORS: u32 = 4u;
const FLAG_ALPHA_TEST: u32 = 8u;
const FLAG_BONE_WEIGHTS: u32 = 16u;
const FLAG_SURFACES: u32 = 32u;

// Surface models (Material.flags.y); 0 = plain texture × colour.
const MODEL_BASIC: u32 = 1u;
const MODEL_TINT_MASK: u32 = 2u;
const MODEL_HAIR: u32 = 3u;
const MODEL_SKIN: u32 = 4u;

// Textures a surface has (Material.flags.z).
const HAS_NORMAL: u32 = 1u;
const HAS_GLOSS: u32 = 2u;
const HAS_DETAIL_NORMAL: u32 = 4u;
const HAS_DETAIL_MASK: u32 = 8u;
const HAS_DETAIL_GLOSS: u32 = 16u;

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
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var diffuse: texture_2d<f32>;
@group(1) @binding(2) var diffuse_sampler: sampler;
@group(1) @binding(3) var normal_map: texture_2d<f32>;
@group(1) @binding(4) var gloss_map: texture_2d<f32>;
@group(1) @binding(5) var detail_normal_map: texture_2d<f32>;
@group(1) @binding(6) var detail_mask_map: texture_2d<f32>;
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

@vertex
fn vs_mesh(v: VertexIn, inst: InstanceIn) -> VertexOut {
    let model = mat4x4<f32>(inst.m0, inst.m1, inst.m2, inst.m3);
    let world = model * vec4<f32>(v.position, 1.0);
    var out: VertexOut;
    out.clip = globals.view_proj * world;
    out.world = world.xyz;
    out.normal = (model * vec4<f32>(v.normal, 0.0)).xyz;
    out.uv = v.uv;
    out.color = v.color;
    out.tint = inst.tint;
    out.ids = inst.ids;
    out.part = v.part;
    var weight = 0.0;
    for (var i = 0; i < 4; i++) {
        if (v.joints[i] == globals.params.y) {
            weight += v.weights[i];
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
        && (globals.params.x & FLAG_TEXTURES) != 0u;
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
    let normal_texel = textureSample(normal_map, diffuse_sampler, v.uv);
    let gloss_texel = textureSample(gloss_map, diffuse_sampler, v.uv);
    let detail_normal_texel = textureSample(detail_normal_map, diffuse_sampler, detail_uv);
    let mask_texel = textureSample(detail_mask_map, diffuse_sampler, mask_uv);
    let detail_gloss_texel = textureSample(detail_gloss_map, diffuse_sampler, detail_uv);
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
    if ((globals.params.x & FLAG_LIGHTING) != 0u && surface && (globals.params.x & FLAG_BONE_WEIGHTS) == 0u) {
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
    if (globals.fog.z > 0.0) {
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
    return vec4<f32>(linear_to_srgb(lit), alpha);
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
    var clip = globals.view_proj * model * vec4<f32>(v.position, 1.0);
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
