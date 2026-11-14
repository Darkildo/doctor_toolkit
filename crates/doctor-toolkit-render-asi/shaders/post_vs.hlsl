// Vertex shader of the graphics module's full-screen passes (stage 59).
// A quad in clip space as a triangle strip; c0.xy moves it by half a pixel
// so that texel centres fall on pixel centres (Direct3D 9).
//
// Copyright (C) the doctor_toolkit authors. GPL-3.0-only.

float4 half_pixel : register(c0);

struct Out
{
    float4 position : POSITION;
    float2 uv : TEXCOORD0;
};

Out main(float4 position : POSITION, float2 uv : TEXCOORD0)
{
    Out o;
    o.position = float4(position.xy + half_pixel.xy, 0.0, 1.0);
    o.uv = uv;
    return o;
}
