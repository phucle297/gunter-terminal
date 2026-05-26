struct Uniforms {
    viewport: vec2<f32>,
    cell_size: vec2<f32>,
};
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_samp: sampler;

struct VertIn {
    @location(0) pos: vec2<f32>,
};

struct InstIn {
    @location(1) cell_pos: vec2<f32>,
    @location(2) bg: vec3<f32>,
    @location(3) fg: vec3<f32>,
    @location(4) uv_min: vec2<f32>,
    @location(5) uv_max: vec2<f32>,
    @location(6) flags: u32,
};

struct FragIn {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) bg: vec3<f32>,
    @location(2) fg: vec3<f32>,
    @location(3) @interpolate(flat) flags: u32,
};

@vertex
fn vs_main(v: VertIn, i: InstIn) -> FragIn {
    let is_wide = (i.flags & 2u) != 0u;
    let x_scale = select(1.0, 2.0, is_wide);
    let px = (i.cell_pos.x + v.pos.x * x_scale) * uniforms.cell_size.x;
    let py = (i.cell_pos.y + v.pos.y) * uniforms.cell_size.y;
    let cx = (px / uniforms.viewport.x) * 2.0 - 1.0;
    let cy = 1.0 - (py / uniforms.viewport.y) * 2.0;

    var out: FragIn;
    out.clip_pos = vec4<f32>(cx, cy, 0.0, 1.0);
    out.uv = mix(i.uv_min, i.uv_max, v.pos);
    out.bg = i.bg;
    out.fg = i.fg;
    out.flags = i.flags;
    return out;
}

@fragment
fn fs_main(in: FragIn) -> @location(0) vec4<f32> {
    // Skip spacer cells (wide char right half)
    if (in.flags & 4u) != 0u {
        discard;
    }
    let alpha = textureSample(atlas_tex, atlas_samp, in.uv).r;
    var color = mix(in.bg, in.fg, alpha);
    // Underline: draw fg color in bottom ~15% of cell height
    let cell_frac_y = fract(in.clip_pos.y / uniforms.cell_size.y);
    let is_underline = (in.flags & 1u) != 0u;
    if is_underline && cell_frac_y >= 0.85 {
        color = in.fg;
    }
    return vec4<f32>(color, 1.0);
}
