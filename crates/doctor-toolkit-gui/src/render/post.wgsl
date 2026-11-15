// Colour grading of the game look (post.rs, docs/analysis/renderer.md §10.9):
// the finished frame through up to two 32³ colour tables, weighed, as the
// game's AlchemyLUTShader does (c0x5B00 and on).

struct Params {
    // x: strength (gLUT_params1.x), y, z: weights of the two tables.
    weights: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var frame: texture_2d<f32>;
@group(0) @binding(2) var table_a: texture_3d<f32>;
@group(0) @binding(3) var table_b: texture_3d<f32>;
@group(0) @binding(4) var table_sampler: sampler;

@vertex
fn vs_post(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4<f32>(xy, 0.0, 1.0);
}

@fragment
fn fs_post(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(frame, vec2<i32>(pixel.xy), 0);
    // The game's addressing of a 32³ table: c · 31/32 + 1/64.
    let at = c.rgb * 0.96875 + 0.015625;
    let graded = textureSampleLevel(table_a, table_sampler, at, 0.0).rgb * params.weights.y
        + textureSampleLevel(table_b, table_sampler, at, 0.0).rgb * params.weights.z;
    return vec4<f32>(mix(c.rgb, graded, params.weights.x), c.a);
}
