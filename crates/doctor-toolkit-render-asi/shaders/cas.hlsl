// Sharpening of the graphics module (stage 59) over the finished frame.
//
// A new implementation of the idea of contrast adaptive sharpening (CAS,
// AMD FidelityFX, published under the MIT license): the pixel takes a
// negative share of its four neighbours, and the share shrinks where the
// 3x3 neighbourhood already has much contrast or is near black or white,
// so edges sharpen without halos and flat areas stay clean.
//
// Copyright (C) the doctor_toolkit authors. GPL-3.0-only.

sampler2D frame : register(s0);
// 1 / width, 1 / height, width, height of the frame.
float4 texel : register(c0);
// x: the strongest negative weight, -1 / lerp(8, 5, sharpness).
float4 strength : register(c1);

float3 at(float2 uv)
{
    return tex2Dlod(frame, float4(uv, 0.0, 0.0)).rgb;
}

float4 main(float2 uv : TEXCOORD0) : COLOR0
{
    float2 t = texel.xy;
    float3 a = at(uv + float2(-t.x, -t.y));
    float3 b = at(uv + float2(0.0, -t.y));
    float3 c = at(uv + float2(t.x, -t.y));
    float3 d = at(uv + float2(-t.x, 0.0));
    float3 e = at(uv);
    float3 f = at(uv + float2(t.x, 0.0));
    float3 g = at(uv + float2(-t.x, t.y));
    float3 h = at(uv + float2(0.0, t.y));
    float3 i = at(uv + float2(t.x, t.y));

    // Soft minimum and maximum: the cross plus the whole 3x3 (range 0-2).
    float3 lo = min(min(min(d, e), min(f, b)), h);
    lo += min(lo, min(min(a, c), min(g, i)));
    float3 hi = max(max(max(d, e), max(f, b)), h);
    hi += max(hi, max(max(a, c), max(g, i)));

    // How much room to sharpen: small near black, near white and on
    // strong contrast.
    float3 room = sqrt(saturate(min(lo, 2.0 - hi) / max(hi, 1.0 / 65536.0)));
    float3 weight = room * strength.x;
    float3 sharp = (b * weight + d * weight + f * weight + h * weight + e) / (1.0 + 4.0 * weight);
    return float4(saturate(sharp), 1.0);
}
