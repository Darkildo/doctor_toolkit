// FXAA of the graphics module (stage 59): anti-aliasing over the finished
// frame.
//
// A new implementation of the published idea of FXAA (fast approximate
// anti-aliasing, Timothy Lottes, NVIDIA): measure the contrast around the
// pixel; on an edge, find whether it runs across or along, walk along it to
// both ends and move the sample across the edge by how close the pixel is
// to the nearer end; a smaller blend smooths detail finer than a pixel.
//
// Copyright (C) the doctor_toolkit authors. GPL-3.0-only.

sampler2D frame : register(s0);
// 1 / width, 1 / height, width, height of the frame.
float4 texel : register(c0);

static const float EDGE_MIN = 0.0312;
static const float EDGE_RELATIVE = 0.125;
static const float SUBPIXEL = 0.75;

float luma(float3 c)
{
    return dot(c, float3(0.299, 0.587, 0.114));
}

float3 at(float2 uv)
{
    return tex2Dlod(frame, float4(uv, 0.0, 0.0)).rgb;
}

float luma_at(float2 uv)
{
    return luma(at(uv));
}

float4 main(float2 uv : TEXCOORD0) : COLOR0
{
    float2 t = texel.xy;
    float3 centre = at(uv);
    float m = luma(centre);
    float n = luma_at(uv + float2(0.0, -t.y));
    float s = luma_at(uv + float2(0.0, t.y));
    float e = luma_at(uv + float2(t.x, 0.0));
    float w = luma_at(uv + float2(-t.x, 0.0));
    float lo = min(m, min(min(n, s), min(e, w)));
    float hi = max(m, max(max(n, s), max(e, w)));
    float range = hi - lo;
    if (range < max(EDGE_MIN, hi * EDGE_RELATIVE))
    {
        return float4(centre, 1.0);
    }

    float nw = luma_at(uv + float2(-t.x, -t.y));
    float ne = luma_at(uv + float2(t.x, -t.y));
    float sw = luma_at(uv + float2(-t.x, t.y));
    float se = luma_at(uv + float2(t.x, t.y));

    // Detail finer than a pixel: how far the centre is from the average
    // around it.
    float around = (2.0 * (n + s + e + w) + (nw + ne + sw + se)) / 12.0;
    float sub = smoothstep(0.0, 1.0, saturate(abs(around - m) / range));
    sub = sub * sub * SUBPIXEL;

    // A horizontal edge changes along y.
    float horizontal = abs(n + s - 2.0 * m) * 2.0 + abs(ne + se - 2.0 * e) + abs(nw + sw - 2.0 * w);
    float vertical = abs(e + w - 2.0 * m) * 2.0 + abs(ne + nw - 2.0 * n) + abs(se + sw - 2.0 * s);
    bool is_h = horizontal >= vertical;

    float before = is_h ? n : w;
    float after = is_h ? s : e;
    float g1 = abs(before - m);
    float g2 = abs(after - m);
    float across = is_h ? t.y : t.x;
    float other = after;
    if (g1 >= g2)
    {
        across = -across;
        other = before;
    }
    float gradient = 0.25 * max(g1, g2);
    float edge_luma = 0.5 * (m + other);

    // Walk along the edge, half a pixel towards the other side.
    float2 start = uv;
    if (is_h)
    {
        start.y += 0.5 * across;
    }
    else
    {
        start.x += 0.5 * across;
    }
    float2 along = is_h ? float2(t.x, 0.0) : float2(0.0, t.y);
    float2 p1 = start - along;
    float2 p2 = start + along;
    float d1 = luma_at(p1) - edge_luma;
    float d2 = luma_at(p2) - edge_luma;
    bool end1 = abs(d1) >= gradient;
    bool end2 = abs(d2) >= gradient;
    float stride = 1.0;
    [unroll]
    for (int i = 0; i < 9; i++)
    {
        stride = i < 4 ? 1.0 : (i < 5 ? 1.5 : (i < 7 ? 2.0 : (i < 8 ? 4.0 : 8.0)));
        if (!end1)
        {
            p1 -= along * stride;
            d1 = luma_at(p1) - edge_luma;
            end1 = abs(d1) >= gradient;
        }
        if (!end2)
        {
            p2 += along * stride;
            d2 = luma_at(p2) - edge_luma;
            end2 = abs(d2) >= gradient;
        }
    }

    float dist1 = is_h ? uv.x - p1.x : uv.y - p1.y;
    float dist2 = is_h ? p2.x - uv.x : p2.y - uv.y;
    bool nearer1 = dist1 < dist2;
    float dist = min(dist1, dist2);
    float offset = 0.5 - dist / (dist1 + dist2);
    // Blend only when the nearer end turns the other way than the centre:
    // that end is where the edge stops.
    bool centre_below = m < edge_luma;
    bool turns = ((nearer1 ? d1 : d2) < 0.0) != centre_below;
    offset = max(turns ? offset : 0.0, sub);

    float2 final_uv = uv;
    if (is_h)
    {
        final_uv.y += offset * across;
    }
    else
    {
        final_uv.x += offset * across;
    }
    return float4(at(final_uv), 1.0);
}
